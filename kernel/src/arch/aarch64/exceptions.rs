//! AArch64 EL1 exception handling, `VBAR_EL1` setup, and IRQ dispatch.

use core::arch::asm;
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

use hal_core::{InterruptController, IrqSource, SerialConsole, Timer};

use crate::{arch, board, sched};

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(include_str!("vectors.S"));

unsafe extern "C" {
    static exception_vector_table: u8;
}

static TIMER_TICK_MS: AtomicU64 = AtomicU64::new(10);
static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

/// Saved register state pushed onto the EL1 stack on exception entry.
#[repr(C)]
pub struct TrapFrame {
    pub x: [u64; 31],
    pub elr_el1: u64,
    pub spsr_el1: u64,
    pub esr_el1: u64,
    pub far_el1: u64,
    _pad: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 288);

/// Installs `exception_vector_table` into `VBAR_EL1` with IRQs still masked.
pub fn init() {
    let table_addr = (&raw const exception_vector_table) as usize;
    // SAFETY: `exception_vector_table` is 2048-byte aligned and covers all 16 AArch64
    // EL1 exception vectors in `.text`.
    unsafe {
        asm!(
            "msr vbar_el1, {0}",
            "isb",
            in(reg) table_addr,
            options(nomem, nostack, preserves_flags)
        );
    }
}

/// Configures the periodic tick interval in milliseconds used when re-arming timer IRQs.
pub fn set_timer_interval_ms(ms: u64) {
    TIMER_TICK_MS.store(ms.max(1), Ordering::Release);
}

/// Returns the number of timer IRQs handled since boot.
pub fn timer_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
extern "C" fn rust_irq_handler(_tf: &mut TrapFrame, _kind: u64) {
    let core = usize::from(arch::core_id());
    let irq = board::irq_handle();

    match irq.pending_irq(core) {
        IrqSource::Timer => {
            let interval_ms = TIMER_TICK_MS.load(Ordering::Acquire);
            board::timer_handle().arm_after_ms(interval_ms);
            TIMER_TICKS.fetch_add(1, Ordering::Release);
            sched::on_timer_tick(core);
        }
        IrqSource::None => {}
        other => {
            let mut console = board::emergency_console();
            let _ = writeln!(console.writer(), "[irq]  Unhandled IRQ source: {other:?}");
            console.flush();
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn rust_sync_exception_handler(tf: &mut TrapFrame, kind: u64) -> ! {
    report_fatal_exception("Synchronous Exception", tf, kind)
}

#[unsafe(no_mangle)]
extern "C" fn rust_unhandled_exception_handler(tf: &mut TrapFrame, kind: u64) -> ! {
    report_fatal_exception("Unhandled Exception", tf, kind)
}

fn report_fatal_exception(label: &str, tf: &TrapFrame, kind: u64) -> ! {
    let ec = (tf.esr_el1 >> 26) & 0x3F;
    let iss = tf.esr_el1 & 0x01FF_FFFF;
    let mut console = board::emergency_console();
    {
        let mut out = console.writer();
        let _ = writeln!(
            out,
            "\nKERNEL PANIC: {label} (vector={kind}, EC={ec:#x}, ISS={iss:#x})"
        );
        let _ = writeln!(
            out,
            "  ELR_EL1={:#018x}  FAR_EL1={:#018x}  SPSR_EL1={:#010x}",
            tf.elr_el1, tf.far_el1, tf.spsr_el1
        );
        let _ = writeln!(
            out,
            "  x0={:#018x}  x1={:#018x}  x29={:#018x}  x30={:#018x}",
            tf.x[0], tf.x[1], tf.x[29], tf.x[30]
        );
    }
    console.flush();
    arch::halt()
}

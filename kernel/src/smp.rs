//! Symmetric Multiprocessing (SMP) secondary core bring-up for Cortex-A53.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use hal_core::{Board, InterruptController, PAGE_SIZE, Timer};

use crate::mem::{phys_to_virt, pmm};
use crate::{arch, board, sched};

const SECONDARY_STACK_PAGES: usize = 4; // 16 KiB per secondary core

unsafe extern "C" {
    static _start_secondary: u8;
}

/// Per-core EL1 initial stack tops read by `_start_secondary` in `boot.S`.
#[unsafe(no_mangle)]
#[used]
static SECONDARY_STACK_TOPS: [AtomicUsize; sched::MAX_CORES] =
    [const { AtomicUsize::new(0) }; sched::MAX_CORES];

static ONLINE_CORES_MASK: AtomicU8 = AtomicU8::new(1);

/// Brings up secondary CPU cores (`1..cpu_count`) using the board's spin-table/wakeup HAL.
///
/// Returns the bitmask of online CPU cores once all cores have checked in (or timeout expires).
pub fn boot_secondary_cores(active_board: &impl Board) -> u8 {
    let cpu_count = active_board.info().cpu_count.min(sched::MAX_CORES);
    let entry_phys = (&raw const _start_secondary) as usize;

    for slot in SECONDARY_STACK_TOPS.iter().take(cpu_count).skip(1) {
        let stack_phys = pmm::alloc_contiguous_frames(SECONDARY_STACK_PAGES)
            .expect("must allocate secondary core stack");
        // Secondary cores execute `_start_secondary` with `TTBR0` identity mapping initially,
        // then switch to shared `TTBR0`/`TTBR1` inside `secondary_kernel_main`.
        let stack_top_ident = stack_phys.as_usize() + SECONDARY_STACK_PAGES * PAGE_SIZE;

        slot.store(stack_top_ident, Ordering::Release);
        let slot_addr = core::ptr::from_ref(slot) as usize;
        arch::clean_dcache_line(slot_addr);
        // Ensure higher-half pointer mapping also compiles cleanly if referenced later.
        let _ = phys_to_virt(stack_phys);
    }

    for core in 1..cpu_count {
        let _ = active_board.wake_secondary_core(core, entry_phys);
    }

    let expected_mask = ((1u16 << cpu_count) - 1) as u8;
    let timer = board::timer_handle();
    let deadline = timer.counter().saturating_add(timer.frequency_hz() / 2); // 500 ms max wait

    while ONLINE_CORES_MASK.load(Ordering::Acquire) != expected_mask && timer.counter() < deadline {
        core::hint::spin_loop();
    }

    ONLINE_CORES_MASK.load(Ordering::Acquire)
}

/// Returns the bitmask of CPU cores currently online (`bit i = core i`).
pub fn online_mask() -> u8 {
    ONLINE_CORES_MASK.load(Ordering::Acquire)
}

/// Rust entry point for secondary Cortex-A53 cores woken via `_start_secondary`.
#[unsafe(no_mangle)]
pub extern "C" fn secondary_kernel_main(core_id: usize) -> ! {
    // SAFETY: Core 0 finished `mmu::init()` and flushed `BOOT_TTBR*` before waking secondary cores.
    unsafe {
        arch::mmu::enable_secondary();
    }
    arch::exceptions::init();
    sched::register_secondary(core_id);

    board::irq_handle().enable_timer_irq(core_id);
    board::timer_handle().arm_after_ms(10);

    if core_id < sched::MAX_CORES {
        ONLINE_CORES_MASK.fetch_or(1 << core_id, Ordering::Release);
    }

    arch::enable_irqs();

    loop {
        sched::yield_now();
        arch::wait_for_event();
    }
}

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Delta Kernel — strict microkernel entry point for Hollow Star OS+.

mod arch;
mod board;
mod mem;
mod sync;

use core::fmt::Write;
use core::panic::PanicInfo;

use hal_core::{Board, BoardInfo, Dtb, InterruptController, SerialConsole, Timer, VirtAddr};

use crate::mem::{HIGHER_HALF_OFFSET, KernelLayout, pmm};

const BANNER_MAGIC: u64 = 0x4445_4C54_415F_4D32; // "DELTA_M2"

#[unsafe(no_mangle)]
static M2_HIGHER_HALF_PROBE: u64 = BANNER_MAGIC;

/// Rust kernel entry called from AArch64 boot assembly in EL1 with BSS cleared.
///
/// # Safety
///
/// Must be entered only once on primary core 0 after EL1 initialization,
/// valid stack setup, and `.bss` zeroing.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb_ptr: usize) -> ! {
    let mut active_board = board::init();
    let info = active_board.info();
    let el = arch::current_el();
    let core = arch::core_id();
    let layout = KernelLayout::from_linker();

    // Print M1 boot banner before bringing up M2 memory & interrupts.
    {
        let mut out = active_board.console().writer();
        let _ = writeln!(
            out,
            "============================================================"
        );
        let _ = writeln!(
            out,
            "  Hollow Star OS+ / Delta Kernel (v{})",
            env!("CARGO_PKG_VERSION")
        );
        let _ = writeln!(out, "  Arch: aarch64 | EL: EL{el} | Core: {core}");
        let _ = writeln!(out, "  Board: {} | SoC: {}", info.name, info.soc);
        let _ = writeln!(out, "  UART: {}", info.uart_mode);
        let _ = writeln!(
            out,
            "============================================================"
        );
        let _ = writeln!(
            out,
            "[boot] EL1 active, BSS cleared (kernel={:#x}..{:#x})",
            layout.kernel_start, layout.kernel_end
        );
    }

    let (dtb_ram, dtb_span) = inspect_dtb(&mut active_board, dtb_ptr, &info);

    // M2 Step 1: Initialize 4 KiB physical frame allocator (PMM) and smoke-test alloc/free.
    pmm::init(active_board.memory_regions(), dtb_ram, dtb_span, &layout);
    let test_frame = pmm::alloc_frame().expect("PMM must yield a free frame");
    pmm::free_frame(test_frame).expect("PMM free_frame must succeed");
    let pmm_stats = pmm::stats();
    {
        let mut out = active_board.console().writer();
        let _ = writeln!(
            out,
            "[pmm]  Initialized 4 KiB frame allocator: total={} frames ({} MiB), free={}, probe={:#x}",
            pmm_stats.total_ram_frames,
            (pmm_stats.total_ram_frames * hal_core::PAGE_SIZE) / (1024 * 1024),
            pmm_stats.free_frames,
            test_frame.as_usize()
        );
    }

    // M2 Step 2: Build identity (TTBR0) + higher-half (TTBR1) W^X page tables and enable MMU.
    let mmu_cfg = arch::mmu::init(active_board.memory_regions(), dtb_ram, &layout)
        .expect("MMU initialization must succeed");

    let probe_phys = (&raw const M2_HIGHER_HALF_PROBE) as usize;
    let probe_hh_va = HIGHER_HALF_OFFSET.saturating_add(probe_phys);
    let translated_hh = arch::mmu::translate(mmu_cfg.ttbr1_root, VirtAddr(probe_hh_va))
        .expect("TTBR1 higher-half probe must translate");
    // SAFETY: `probe_hh_va` is mapped via `TTBR1_EL1` to `M2_HIGHER_HALF_PROBE` in `.rodata`.
    let probe_val = unsafe { core::ptr::read_volatile(probe_hh_va as *const u64) };
    assert_eq!(probe_val, BANNER_MAGIC);

    {
        let mut out = active_board.console().writer();
        let _ = writeln!(
            out,
            "[mmu]  MMU enabled (TTBR0={:#x}, TTBR1={:#x}, 4K_pages={}, 2M_blocks={})",
            mmu_cfg.ttbr0_root.as_usize(),
            mmu_cfg.ttbr1_root.as_usize(),
            mmu_cfg.pages_mapped_4k,
            mmu_cfg.blocks_mapped_2m
        );
        let _ = writeln!(
            out,
            "[mmu]  Higher-half probe {:#x} -> PA {:#x} (magic OK)",
            probe_hh_va,
            translated_hh.as_usize()
        );
    }

    // M2 Step 3: Install EL1 exception vectors (`VBAR_EL1`) and arm ARM Generic Timer IRQ.
    arch::exceptions::init();
    let timer_freq = active_board.timer().frequency_hz();
    arch::exceptions::set_timer_interval_ms(10);
    active_board
        .irq_controller()
        .enable_timer_irq(usize::from(core));
    active_board.timer().arm_after_ms(10);
    arch::enable_irqs();

    // Wait briefly until at least 3 real timer IRQs have fired to verify end-to-end IRQ delivery.
    let start_cnt = active_board.timer().counter();
    let timeout_cnt = start_cnt.saturating_add(timer_freq); // 1 second safety bound
    while arch::exceptions::timer_ticks() < 3 && active_board.timer().counter() < timeout_cnt {
        arch::wait_for_event();
    }

    let ticks = arch::exceptions::timer_ticks();
    assert!(ticks >= 3, "ARM Generic Timer IRQ failed to fire");

    {
        let mut out = active_board.console().writer();
        let _ = writeln!(
            out,
            "[irq]  VBAR_EL1 installed; ARM Generic Timer freq={} Hz, received {} ticks",
            timer_freq, ticks
        );
        let _ = writeln!(
            out,
            "[boot] Milestone M2 (Memory & Exceptions) ready. Entering idle loop."
        );
    }

    active_board.console().flush();
    active_board.halt()
}

type DtbResult = (Option<(u64, u64)>, Option<(usize, usize)>);

fn inspect_dtb(board: &mut impl Board, dtb_ptr: usize, info: &BoardInfo) -> DtbResult {
    let mut out = board.console().writer();
    if dtb_ptr == 0 {
        let _ = writeln!(
            out,
            "[dtb]  No DTB pointer passed (x0=0x0); using board defaults ({} MiB RAM)",
            info.ram_size_bytes / (1024 * 1024)
        );
        return (None, None);
    }

    let ram_end = info.ram_base.saturating_add(info.ram_size_bytes);
    if dtb_ptr < info.ram_base || dtb_ptr >= ram_end {
        let _ = writeln!(
            out,
            "[dtb]  DTB pointer {dtb_ptr:#x} outside default RAM range; using board defaults"
        );
        return (None, None);
    }

    // SAFETY: `dtb_ptr` is non-null and within identity-mapped physical RAM bounds;
    // `Dtb::from_raw_ptr` verifies alignment, magic (`0xd00dfeed`), and header size.
    match unsafe { Dtb::from_raw_ptr(dtb_ptr) } {
        Ok(dtb) => {
            let total = dtb.header().totalsize as usize;
            let model = dtb.model().unwrap_or("unknown");
            let _ = writeln!(
                out,
                "[dtb]  Parsed FDT at {dtb_ptr:#x} ({total} bytes), model=\"{model}\""
            );
            let mem = dtb.memory_region();
            if let Some((base, size)) = mem {
                let _ = writeln!(
                    out,
                    "[dtb]  Memory region: {base:#x}..{:#x} ({} MiB)",
                    base.saturating_add(size),
                    size / (1024 * 1024)
                );
            }
            (mem, Some((dtb_ptr, total)))
        }
        Err(err) => {
            let _ = writeln!(
                out,
                "[dtb]  No valid FDT at {dtb_ptr:#x} ({err:?}); using board defaults ({} MiB RAM)",
                info.ram_size_bytes / (1024 * 1024)
            );
            (None, None)
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut console = board::emergency_console();
    let _ = writeln!(console.writer(), "\nKERNEL PANIC: {info}");
    console.flush();
    arch::halt()
}

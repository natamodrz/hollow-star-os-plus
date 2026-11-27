#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Delta Kernel — strict microkernel entry point for Hollow Star OS+.

mod arch;
mod board;
mod ipc;
mod mem;
mod proc;
mod sched;
mod shm;
mod smp;
mod sync;
mod syscall;

use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use delta_abi::UartKind;
use hal_core::{Board, BoardInfo, Dtb, InterruptController, SerialConsole, Timer, VirtAddr};

use crate::mem::{HIGHER_HALF_OFFSET, KernelLayout, pmm};
use crate::proc::DeviceCaps;
use crate::sched::Priority;

const BANNER_MAGIC: u64 = 0x4445_4C54_415F_4D32; // "DELTA_M2"

#[unsafe(no_mangle)]
static M2_HIGHER_HALF_PROBE: u64 = BANNER_MAGIC;

static DRV_UART_ELF: &[u8] = include_bytes!(env!("DRV_UART_ELF"));
static DRV_SD_ELF: &[u8] = include_bytes!(env!("DRV_SD_ELF"));
static DRV_FB_ELF: &[u8] = include_bytes!(env!("DRV_FB_ELF"));
static DRV_AUDIO_ELF: &[u8] = include_bytes!(env!("DRV_AUDIO_ELF"));
static VFSD_ELF: &[u8] = include_bytes!(env!("VFSD_ELF"));
static INIT_ELF: &[u8] = include_bytes!(env!("INIT_ELF"));

static PRIO_TRACE_LEN: AtomicUsize = AtomicUsize::new(0);
static PRIO_TRACE: [AtomicU8; 5] = [const { AtomicU8::new(0) }; 5];
static SMP_EXEC_MASK: AtomicU8 = AtomicU8::new(0);

/// Rust kernel entry called from AArch64 boot assembly in EL1 higher-half (`TTBR1_EL1`).
///
/// # Safety
///
/// Must be entered only once on primary core 0 after EL1 initialization,
/// valid stack setup, `.bss` zeroing, and boot MMU activation.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(dtb_ptr: usize) -> ! {
    let mut active_board = board::init();
    let info = active_board.info();
    board::register_metadata(info, active_board.framebuffer_phys_window());
    let el = arch::current_el();
    let core = arch::core_id();
    let layout = KernelLayout::from_linker();

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
            "[boot] EL1 active in higher-half TTBR1, BSS cleared (kernel={:#x}..{:#x})",
            layout.kernel_start, layout.kernel_end
        );
    }

    let (dtb_ram, dtb_span) = inspect_dtb(&mut active_board, dtb_ptr, &info);

    // Initialize 4 KiB physical frame allocator (PMM).
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

    // Install full W^X TTBR1_EL1 kernel page tables and empty idle TTBR0_EL1.
    let mmu_cfg = arch::mmu::init(active_board.memory_regions(), dtb_ram, &layout)
        .expect("MMU initialization must succeed");

    let probe_hh_va = (&raw const M2_HIGHER_HALF_PROBE) as usize;
    let translated_hh = arch::mmu::translate(mmu_cfg.ttbr1_root, VirtAddr(probe_hh_va))
        .expect("TTBR1 higher-half probe must translate");
    // SAFETY: `probe_hh_va` is mapped via `TTBR1_EL1` to `M2_HIGHER_HALF_PROBE` in `.rodata`.
    let probe_val = unsafe { core::ptr::read_volatile(probe_hh_va as *const u64) };
    assert_eq!(probe_val, BANNER_MAGIC);

    {
        let mut out = active_board.console().writer();
        let _ = writeln!(
            out,
            "[mmu]  MMU enabled (TTBR0_idle={:#x}, TTBR1={:#x}, 4K_pages={}, 2M_blocks={})",
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

    // Install EL1 exception vectors (`VBAR_EL1`), init scheduler & arm timer IRQ.
    arch::exceptions::init();
    sched::init_primary();

    let timer_freq = active_board.timer().frequency_hz();
    arch::exceptions::set_timer_interval_ms(10);
    active_board
        .irq_controller()
        .enable_timer_irq(usize::from(core));
    active_board.timer().arm_after_ms(10);
    arch::enable_irqs();

    let start_cnt = active_board.timer().counter();
    let timeout_cnt = start_cnt.saturating_add(timer_freq);
    while arch::exceptions::timer_ticks() < 2 && active_board.timer().counter() < timeout_cnt {
        arch::wait_for_event();
    }
    let ticks = arch::exceptions::timer_ticks();
    assert!(ticks >= 2, "ARM Generic Timer IRQ failed to fire");

    {
        let mut out = active_board.console().writer();
        let _ = writeln!(
            out,
            "[irq]  VBAR_EL1 installed; ARM Generic Timer freq={} Hz, ticks={}",
            timer_freq, ticks
        );
    }

    // Wake secondary Cortex-A53 cores and verify BeOS-inspired SMP Delta Scheduler.
    let online_mask = smp::boot_secondary_cores(&active_board);
    assert_eq!(
        online_mask, 0b1111,
        "Expected all 4 Cortex-A53 cores online, got {online_mask:#b}"
    );

    verify_priority_and_rt_order();
    verify_smp_four_cores();

    let sched_stats = sched::stats();
    let online_verified = smp::online_mask();
    {
        let mut out = active_board.console().writer();
        let _ = writeln!(
            out,
            "[smp]  4/4 Cortex-A53 cores online (mask={online_verified:#06b}, sched_cores={:#06b}, exec_mask={:#06b})",
            sched_stats.active_cores_mask,
            SMP_EXEC_MASK.load(Ordering::Acquire)
        );
        let _ = writeln!(
            out,
            "[sched] Delta Scheduler verified: BeOS tiers 1..={} (RT 110/PI 100 -> UI 15 -> Normal 10 -> Low 5), threads={}, ctx_switches={}",
            Priority::REALTIME_MAX.as_u8(),
            sched_stats.total_threads_created,
            sched_stats.context_switches
        );
        let _ = writeln!(
            out,
            "[el0]  Starting EL0 servers (drv-uart, drv-sd, vfsd, drv-fb, drv-audio, init)..."
        );
    }
    active_board.console().flush();

    // Launch EL0 system servers (`drv-uart`, `drv-sd`, `vfsd`, `drv-fb`, `drv-audio`) + `init`.
    // Only driver servers receive their isolated 4 KiB Device MMIO pages.
    let uart_region = active_board.uart_mmio_region();
    let uart_kind = if cfg!(feature = "mini-uart") {
        UartKind::MiniUart
    } else {
        UartKind::Pl011
    };

    let _drv_uart_pid = proc::spawn_elf(
        "drv-uart",
        DRV_UART_ELF,
        Priority::URGENT,
        DeviceCaps {
            uart: Some((uart_region, uart_kind)),
            ..DeviceCaps::none()
        },
    )
    .expect("spawn EL0 drv-uart server");

    let _drv_sd_pid = proc::spawn_elf(
        "drv-sd",
        DRV_SD_ELF,
        Priority::URGENT,
        DeviceCaps {
            sd: active_board.sd_mmio_region(),
            ..DeviceCaps::none()
        },
    )
    .expect("spawn EL0 drv-sd server");

    let _vfsd_pid = proc::spawn_elf("vfsd", VFSD_ELF, Priority::URGENT, DeviceCaps::none())
        .expect("spawn EL0 vfsd server");

    let _drv_fb_pid = proc::spawn_elf(
        "drv-fb",
        DRV_FB_ELF,
        Priority::DISPLAY,
        DeviceCaps {
            display: active_board.mailbox_mmio_region(),
            ..DeviceCaps::none()
        },
    )
    .expect("spawn EL0 drv-fb server");

    let _drv_audio_pid = proc::spawn_elf(
        "drv-audio",
        DRV_AUDIO_ELF,
        Priority::new(110), // Real-Time Audio priority tier
        DeviceCaps {
            audio: active_board.audio_mmio_region(),
            ..DeviceCaps::none()
        },
    )
    .expect("spawn EL0 drv-audio server");

    let _init_pid = proc::spawn_elf("init", INIT_ELF, Priority::NORMAL, DeviceCaps::none())
        .expect("spawn EL0 init process");

    loop {
        sched::yield_now();
        arch::wait_for_event();
    }
}

fn verify_priority_and_rt_order() {
    PRIO_TRACE_LEN.store(0, Ordering::Release);

    let saved = arch::save_and_disable_irqs();
    let _normal = sched::spawn(
        "worker-normal",
        Priority::NORMAL,
        Some(0),
        prio_worker,
        b'N' as usize,
    )
    .expect("spawn normal");
    let _ui = sched::spawn(
        "worker-ui",
        Priority::DISPLAY,
        Some(0),
        prio_worker,
        b'U' as usize,
    )
    .expect("spawn ui");
    let pi_low = sched::spawn(
        "worker-pi",
        Priority::LOW,
        Some(0),
        prio_worker,
        b'P' as usize,
    )
    .expect("spawn pi");
    sched::boost_priority(pi_low, Priority::REALTIME_MIN);
    assert_eq!(
        sched::effective_priority(pi_low),
        Some(Priority::REALTIME_MIN)
    );
    assert_eq!(
        sched::thread_metadata(pi_low),
        Some(("worker-pi", Priority::LOW))
    );

    let pi_reset = sched::spawn(
        "worker-low",
        Priority::LOW,
        Some(0),
        prio_worker,
        b'L' as usize,
    )
    .expect("spawn low");
    sched::boost_priority(pi_reset, Priority::REALTIME_MAX);
    sched::clear_priority_boost(pi_reset);
    assert_eq!(sched::effective_priority(pi_reset), Some(Priority::LOW));

    let _rt = sched::spawn(
        "rt-audio",
        Priority::new(110),
        Some(0),
        prio_worker,
        b'R' as usize,
    )
    .expect("spawn rt");
    arch::restore_irqs(saved);

    while PRIO_TRACE_LEN.load(Ordering::Acquire) < 5 {
        sched::yield_now();
    }

    let order = [
        PRIO_TRACE[0].load(Ordering::Acquire),
        PRIO_TRACE[1].load(Ordering::Acquire),
        PRIO_TRACE[2].load(Ordering::Acquire),
        PRIO_TRACE[3].load(Ordering::Acquire),
        PRIO_TRACE[4].load(Ordering::Acquire),
    ];
    assert_eq!(
        &order, b"RPUNL",
        "Expected RT(110) -> PI(100) -> UI(15) -> Normal(10) -> Low(5), got {order:?}"
    );
}

fn verify_smp_four_cores() {
    SMP_EXEC_MASK.store(0, Ordering::Release);
    for core in 0..4u8 {
        let _ = sched::spawn(
            "smp-probe",
            Priority::URGENT,
            Some(core),
            smp_probe_worker,
            usize::from(core),
        )
        .expect("spawn smp probe");
    }

    let timer = board::timer_handle();
    let deadline = timer.counter().saturating_add(timer.frequency_hz());
    while SMP_EXEC_MASK.load(Ordering::Acquire) != 0b1111 && timer.counter() < deadline {
        sched::yield_now();
        arch::wait_for_event();
    }
    assert_eq!(
        SMP_EXEC_MASK.load(Ordering::Acquire),
        0b1111,
        "All 4 cores must execute scheduled threads"
    );
}

fn prio_worker(tag: usize) {
    let idx = PRIO_TRACE_LEN.fetch_add(1, Ordering::AcqRel);
    if idx < PRIO_TRACE.len() {
        PRIO_TRACE[idx].store(tag as u8, Ordering::Release);
    }
}

fn smp_probe_worker(expected_core: usize) {
    let actual_core = usize::from(arch::core_id());
    assert_eq!(actual_core, expected_core, "SMP affinity mismatch");
    SMP_EXEC_MASK.fetch_or(1 << actual_core, Ordering::Release);
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

    // SAFETY: `dtb_ptr` is non-null and within physical RAM bounds;
    // convert to higher-half virtual address since `TTBR1_EL1` is active.
    let dtb_virt = HIGHER_HALF_OFFSET.saturating_add(dtb_ptr);
    match unsafe { Dtb::from_raw_ptr(dtb_virt) } {
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

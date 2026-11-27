//! AArch64 architecture support, early boot, exceptions, MMU, and context switch.

pub mod context;
pub mod exceptions;
pub mod mmu;

use core::arch::asm;

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(include_str!("boot.S"));

const DAIF_IRQ_BIT: u64 = 1 << 7;

/// Returns the current ARM exception level (`0..=3`).
pub fn current_el() -> u8 {
    let raw: u64;
    // SAFETY: Reading `CurrentEL` is always valid at EL1+ and has no side effects.
    unsafe {
        asm!("mrs {0}, CurrentEL", out(reg) raw, options(nomem, nostack, preserves_flags));
    }
    ((raw >> 2) & 0x3) as u8
}

/// Returns the Aff0 core identifier of the executing CPU (`0..=3` on BCM2837).
pub fn core_id() -> u8 {
    let mpidr: u64;
    // SAFETY: Reading `MPIDR_EL1` is valid in EL1 and has no side effects.
    unsafe {
        asm!("mrs {0}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags));
    }
    (mpidr & 0xFF) as u8
}

/// Cleans the data cache line containing `addr` to the Point of Coherency (`dc cvac`).
pub fn clean_dcache_line(addr: usize) {
    // SAFETY: `dc cvac` cleans the cache line at `addr` to DRAM PoC without altering contents.
    unsafe {
        asm!(
            "dc cvac, {0}",
            "dsb sy",
            in(reg) addr,
            options(nostack)
        );
    }
}

/// Enables maskable IRQ interrupts on the current CPU core (`DAIF.I = 0`).
pub fn enable_irqs() {
    // SAFETY: Vector table (`VBAR_EL1`) and interrupt controller are initialized before unmasking.
    unsafe {
        asm!("msr daifclr, #2", "isb", options(nomem, nostack));
    }
}

/// Saves `DAIF` and masks IRQ interrupts on the current CPU core.
pub fn save_and_disable_irqs() -> u64 {
    let daif: u64;
    // SAFETY: Reading `DAIF` and setting `DAIFSet` masks IRQs atomically on this core.
    unsafe {
        asm!(
            "mrs {0}, daif",
            "msr daifset, #2",
            "isb",
            out(reg) daif,
            options(nomem, nostack)
        );
    }
    daif
}

/// Restores `DAIF` IRQ mask state captured by [`save_and_disable_irqs`].
pub fn restore_irqs(saved_daif: u64) {
    if (saved_daif & DAIF_IRQ_BIT) == 0 {
        enable_irqs();
    }
}

/// Suspends the core until an event or interrupt arrives (`wfe`).
pub fn wait_for_event() {
    // SAFETY: `wfe` yields the core until an event or unmasked interrupt occurs.
    unsafe {
        asm!("wfe", options(nomem, nostack, preserves_flags));
    }
}

/// Broadcasts a wakeup event to all cores sleeping in `wfe` (`sev`).
pub fn send_event() {
    // SAFETY: `sev` wakes cores waiting in `wfe` without modifying memory.
    unsafe {
        asm!("sev", options(nomem, nostack, preserves_flags));
    }
}

/// Permanently halts the current core in a low-power `wfe` loop.
pub fn halt() -> ! {
    let _ = save_and_disable_irqs();
    loop {
        wait_for_event();
    }
}

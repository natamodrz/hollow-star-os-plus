//! Architecture abstraction module.

#[cfg(target_arch = "aarch64")]
pub mod aarch64;

#[cfg(target_arch = "aarch64")]
pub use aarch64::{
    core_id, current_el, enable_irqs, exceptions, halt, mmu, restore_irqs, save_and_disable_irqs,
    wait_for_event,
};

#[cfg(not(target_arch = "aarch64"))]
pub fn current_el() -> u8 {
    1
}

#[cfg(not(target_arch = "aarch64"))]
pub fn core_id() -> u8 {
    0
}

#[cfg(not(target_arch = "aarch64"))]
pub fn save_and_disable_irqs() -> u64 {
    0
}

#[cfg(not(target_arch = "aarch64"))]
pub fn restore_irqs(_saved: u64) {}

#[cfg(not(target_arch = "aarch64"))]
pub fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

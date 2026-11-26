//! Thin volatile MMIO helper isolating raw pointer accesses for BCM2837 peripherals.

use core::ptr::{read_volatile, write_volatile};

/// Base of a BCM2837 memory-mapped peripheral register block.
#[derive(Clone, Copy)]
pub struct MmioBlock {
    base: usize,
}

impl MmioBlock {
    /// Creates a new MMIO block wrapper starting at physical address `base`.
    ///
    /// # Safety
    ///
    /// `base` must point to a valid, identity-mapped device MMIO region for the
    /// lifetime of the wrapper, and register offsets passed to methods must lie
    /// within that peripheral's 32-bit register window.
    pub const unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    /// Reads a 32-bit MMIO register at `base + offset`.
    pub fn read(&self, offset: usize) -> u32 {
        let addr = (self.base + offset) as *const u32;
        // SAFETY: `self.base` was validated on construction and `offset` selects
        // a 4-byte-aligned device register within this peripheral block.
        unsafe { read_volatile(addr) }
    }

    /// Writes `value` to a 32-bit MMIO register at `base + offset`.
    pub fn write(&self, offset: usize, value: u32) {
        let addr = (self.base + offset) as *mut u32;
        // SAFETY: `self.base` was validated on construction and `offset` selects
        // a 4-byte-aligned writable device register within this peripheral block.
        unsafe { write_volatile(addr, value) }
    }
}

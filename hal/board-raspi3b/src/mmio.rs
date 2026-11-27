//! Thin volatile MMIO helper isolating raw pointer accesses for BCM2837 peripherals.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};

static MMIO_VIRT_OFFSET: AtomicUsize = AtomicUsize::new(0);

/// Configures the virtual address offset applied to physical MMIO bases
/// (e.g. `0xFFFF_0000_0000_0000` when the kernel executes in higher-half `TTBR1_EL1`).
pub fn set_mmio_virt_offset(offset: usize) {
    MMIO_VIRT_OFFSET.store(offset, Ordering::Release);
}

/// Base of a BCM2837 memory-mapped peripheral register block.
#[derive(Clone, Copy)]
pub struct MmioBlock {
    phys_base: usize,
}

impl MmioBlock {
    /// Creates a new MMIO block wrapper starting at physical address `phys_base`.
    ///
    /// # Safety
    ///
    /// `phys_base` must point to a valid device MMIO region mapped either identically
    /// or at `MMIO_VIRT_OFFSET + phys_base`, and register offsets passed to methods
    /// must lie within that peripheral's 32-bit register window.
    pub const unsafe fn new(phys_base: usize) -> Self {
        Self { phys_base }
    }

    fn effective_addr(&self, offset: usize) -> usize {
        MMIO_VIRT_OFFSET
            .load(Ordering::Relaxed)
            .wrapping_add(self.phys_base)
            .wrapping_add(offset)
    }

    /// Reads a 32-bit MMIO register at `base + offset`.
    pub fn read(&self, offset: usize) -> u32 {
        let addr = self.effective_addr(offset) as *const u32;
        // SAFETY: `self.phys_base` + `MMIO_VIRT_OFFSET` selects a mapped 4-byte-aligned
        // Device-nGnRnE register within this peripheral block.
        unsafe { read_volatile(addr) }
    }

    /// Writes `value` to a 32-bit MMIO register at `base + offset`.
    pub fn write(&self, offset: usize, value: u32) {
        let addr = self.effective_addr(offset) as *mut u32;
        // SAFETY: `self.phys_base` + `MMIO_VIRT_OFFSET` selects a mapped 4-byte-aligned
        // writable Device-nGnRnE register within this peripheral block.
        unsafe { write_volatile(addr, value) }
    }
}

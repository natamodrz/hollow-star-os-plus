//! Physical and virtual memory management (4 KiB PMM + AArch64 MMU paging).

pub mod pmm;

use core::ptr::addr_of;
use core::sync::atomic::{AtomicBool, Ordering};

use hal_core::PhysAddr;

/// Higher-half direct physical map base in `TTBR1_EL1` (48-bit VA split).
pub const HIGHER_HALF_OFFSET: usize = 0xFFFF_0000_0000_0000;

static HIGHER_HALF_ACTIVE: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    static __kernel_start: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __rw_start: u8;
    static __kernel_end: u8;
}

/// Page-aligned physical section boundaries exported by the board linker script.
#[derive(Debug, Clone, Copy)]
pub struct KernelLayout {
    pub kernel_start: usize,
    pub text_start: usize,
    pub text_end: usize,
    pub rodata_start: usize,
    pub rodata_end: usize,
    pub rw_start: usize,
    pub kernel_end: usize,
}

impl KernelLayout {
    /// Reads the linker-defined physical boundaries of the running kernel image.
    pub fn from_linker() -> Self {
        Self {
            kernel_start: addr_of!(__kernel_start) as usize,
            text_start: addr_of!(__text_start) as usize,
            text_end: addr_of!(__text_end) as usize,
            rodata_start: addr_of!(__rodata_start) as usize,
            rodata_end: addr_of!(__rodata_end) as usize,
            rw_start: addr_of!(__rw_start) as usize,
            kernel_end: addr_of!(__kernel_end) as usize,
        }
    }
}

/// Marks the higher-half direct map (`TTBR1_EL1`) as active for [`phys_to_virt`].
pub fn mark_higher_half_active() {
    HIGHER_HALF_ACTIVE.store(true, Ordering::Release);
}

/// Converts a physical RAM address to an accessible virtual address.
///
/// Uses identity mapping prior to MMU activation and the `TTBR1_EL1` higher-half
/// alias afterwards.
pub fn phys_to_virt(pa: PhysAddr) -> usize {
    if HIGHER_HALF_ACTIVE.load(Ordering::Acquire) {
        HIGHER_HALF_OFFSET.saturating_add(pa.as_usize())
    } else {
        pa.as_usize()
    }
}

/// Converts a physical RAM address into a typed mutable pointer.
pub fn phys_to_mut_ptr<T>(pa: PhysAddr) -> *mut T {
    phys_to_virt(pa) as *mut T
}

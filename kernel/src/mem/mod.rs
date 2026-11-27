//! Physical and virtual memory management (4 KiB PMM + AArch64 MMU paging).

pub mod pmm;

use core::ptr::addr_of;

use hal_core::PhysAddr;

/// Higher-half direct physical map base in `TTBR1_EL1` (48-bit VA split).
pub const HIGHER_HALF_OFFSET: usize = 0xFFFF_0000_0000_0000;

unsafe extern "C" {
    static __kernel_start: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __rw_start: u8;
    static __kernel_end: u8;
}

/// Page-aligned physical section boundaries of the running kernel image.
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
    /// Reads the linker-defined virtual boundaries and converts them to physical addresses.
    pub fn from_linker() -> Self {
        Self {
            kernel_start: virt_to_phys_const(addr_of!(__kernel_start) as usize),
            text_start: virt_to_phys_const(addr_of!(__text_start) as usize),
            text_end: virt_to_phys_const(addr_of!(__text_end) as usize),
            rodata_start: virt_to_phys_const(addr_of!(__rodata_start) as usize),
            rodata_end: virt_to_phys_const(addr_of!(__rodata_end) as usize),
            rw_start: virt_to_phys_const(addr_of!(__rw_start) as usize),
            kernel_end: virt_to_phys_const(addr_of!(__kernel_end) as usize),
        }
    }
}

const fn virt_to_phys_const(va: usize) -> usize {
    va.saturating_sub(HIGHER_HALF_OFFSET)
}

/// Converts a physical RAM address to its `TTBR1_EL1` higher-half virtual address.
pub const fn phys_to_virt(pa: PhysAddr) -> usize {
    HIGHER_HALF_OFFSET.saturating_add(pa.as_usize())
}

/// Converts a higher-half kernel virtual address back to a physical address.
pub const fn virt_to_phys(va: usize) -> PhysAddr {
    PhysAddr(va.saturating_sub(HIGHER_HALF_OFFSET))
}

/// Converts a physical RAM address into a typed mutable pointer in higher-half space.
pub const fn phys_to_mut_ptr<T>(pa: PhysAddr) -> *mut T {
    phys_to_virt(pa) as *mut T
}

//! Hardware-independent memory types, page-size constants, and region descriptors.

/// Base page size used by Delta Kernel (4 KiB).
pub const PAGE_SIZE: usize = 4096;

/// Log2 of [`PAGE_SIZE`] (`2^12 = 4096`).
pub const PAGE_SHIFT: usize = 12;

/// Aligns `addr` downward to the nearest multiple of `align` (must be a power of two).
pub const fn align_down(addr: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    addr & !(align - 1)
}

/// Aligns `addr` upward to the nearest multiple of `align` (must be a power of two).
pub const fn align_up(addr: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (addr + align - 1) & !(align - 1)
}

/// Physical memory address wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub usize);

impl PhysAddr {
    /// Returns the raw physical address integer.
    pub const fn as_usize(self) -> usize {
        self.0
    }

    /// Returns `true` if aligned to [`PAGE_SIZE`].
    pub const fn is_page_aligned(self) -> bool {
        (self.0 & (PAGE_SIZE - 1)) == 0
    }

    /// Aligns downward to a 4 KiB page boundary.
    pub const fn align_down_page(self) -> Self {
        Self(align_down(self.0, PAGE_SIZE))
    }

    /// Aligns upward to a 4 KiB page boundary.
    pub const fn align_up_page(self) -> Self {
        Self(align_up(self.0, PAGE_SIZE))
    }

    /// Returns the 4 KiB physical frame index (`addr >> 12`).
    pub const fn frame_index(self) -> usize {
        self.0 >> PAGE_SHIFT
    }

    /// Constructs a [`PhysAddr`] from a 4 KiB frame index.
    pub const fn from_frame_index(index: usize) -> Self {
        Self(index << PAGE_SHIFT)
    }
}

/// Virtual memory address wrapper (48-bit canonical AArch64 split).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub usize);

impl VirtAddr {
    /// Returns the raw virtual address integer.
    pub const fn as_usize(self) -> usize {
        self.0
    }

    /// Returns `true` if aligned to [`PAGE_SIZE`].
    pub const fn is_page_aligned(self) -> bool {
        (self.0 & (PAGE_SIZE - 1)) == 0
    }

    /// Level 0 page table index (`bits [47:39]`).
    pub const fn l0_index(self) -> usize {
        (self.0 >> 39) & 0x1FF
    }

    /// Level 1 page table index (`bits [38:30]`).
    pub const fn l1_index(self) -> usize {
        (self.0 >> 30) & 0x1FF
    }

    /// Level 2 page table index (`bits [29:21]`).
    pub const fn l2_index(self) -> usize {
        (self.0 >> 21) & 0x1FF
    }

    /// Level 3 page table index (`bits [20:12]`).
    pub const fn l3_index(self) -> usize {
        (self.0 >> 12) & 0x1FF
    }

    /// Offset within the 4 KiB page (`bits [11:0]`).
    pub const fn page_offset(self) -> usize {
        self.0 & (PAGE_SIZE - 1)
    }
}

/// Classification of a physical memory range reported by board HAL or DTB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// Normal cacheable DRAM available for kernel/userspace allocation.
    UsableRam,
    /// Reserved memory (firmware spin tables, DTB, GPU memory split).
    Reserved,
    /// Memory-mapped peripheral I/O requiring uncached Device attributes.
    DeviceMmio,
}

/// Physical address range descriptor provided by a board crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    pub base: usize,
    pub size: usize,
    pub kind: MemoryKind,
}

impl MemoryRegion {
    /// Creates a new [`MemoryRegion`].
    pub const fn new(base: usize, size: usize, kind: MemoryKind) -> Self {
        Self { base, size, kind }
    }

    /// Returns the exclusive physical end address (`base + size`).
    pub const fn end(&self) -> usize {
        self.base.saturating_add(self.size)
    }
}

#[cfg(test)]
mod tests {
    use super::{PAGE_SIZE, PhysAddr, VirtAddr, align_down, align_up};

    #[test]
    fn alignment_helpers_work_for_page_boundaries() {
        assert_eq!(align_down(0x8_1234, PAGE_SIZE), 0x8_1000);
        assert_eq!(align_up(0x8_1000, PAGE_SIZE), 0x8_1000);
        assert_eq!(align_up(0x8_1001, PAGE_SIZE), 0x8_2000);
    }

    #[test]
    fn phys_and_virt_indices_match_4level_4k_paging() {
        let pa = PhysAddr(0x0008_3000);
        assert!(pa.is_page_aligned());
        assert_eq!(pa.frame_index(), 0x83);
        assert_eq!(PhysAddr::from_frame_index(0x83), pa);

        let va = VirtAddr(0xFFFF_0000_3F20_1ABC);
        assert!(!va.is_page_aligned());
        assert_eq!(va.l0_index(), 0);
        assert_eq!(va.l1_index(), 0);
        assert_eq!(va.l2_index(), 0x1F9);
        assert_eq!(va.l3_index(), 1);
        assert_eq!(va.page_offset(), 0xABC);
    }
}

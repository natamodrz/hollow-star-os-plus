//! Bitmap-based 4 KiB physical frame allocator (PMM).

use core::ptr::write_bytes;

use hal_core::{MemoryKind, MemoryRegion, PAGE_SIZE, PhysAddr, mem::align_down, mem::align_up};

use crate::mem::{KernelLayout, phys_to_mut_ptr};
use crate::sync::IrqSpinLock;

/// Maximum physical address tracked by the static bitmap (1 GiB).
const MAX_PHYS_BYTES: usize = 1024 * 1024 * 1024;
const MAX_FRAMES: usize = MAX_PHYS_BYTES / PAGE_SIZE;
const BITMAP_WORDS: usize = MAX_FRAMES / 64;

/// Summary statistics reported by the physical frame allocator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmmStats {
    pub total_ram_frames: usize,
    pub free_frames: usize,
    pub used_frames: usize,
}

/// Errors returned by [`free_frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmmError {
    UnalignedAddress,
    OutOfRange,
    DoubleFree,
}

struct BitmapAllocator {
    /// Bit `1` = frame allocated/reserved, bit `0` = frame free.
    used_bits: [u64; BITMAP_WORDS],
    total_ram_frames: usize,
    free_frames: usize,
    initialized: bool,
}

impl BitmapAllocator {
    const fn new() -> Self {
        Self {
            used_bits: [u64::MAX; BITMAP_WORDS],
            total_ram_frames: 0,
            free_frames: 0,
            initialized: false,
        }
    }

    fn init(
        &mut self,
        regions: &[MemoryRegion],
        dtb_ram: Option<(u64, u64)>,
        dtb_span: Option<(usize, usize)>,
        layout: &KernelLayout,
    ) {
        self.used_bits.fill(u64::MAX);
        self.total_ram_frames = 0;
        self.free_frames = 0;

        // Determine usable RAM upper bound if DTB supplies a stricter memory ceiling.
        let dtb_ram_end = dtb_ram.map(|(base, size)| base.saturating_add(size) as usize);

        for region in regions {
            if region.kind != MemoryKind::UsableRam {
                continue;
            }
            let start = align_up(region.base, PAGE_SIZE);
            let mut end = align_down(region.end(), PAGE_SIZE).min(MAX_PHYS_BYTES);
            if let Some(ceiling) = dtb_ram_end {
                end = end.min(align_down(ceiling, PAGE_SIZE));
            }
            if end > start {
                self.mark_range_free(start, end);
            }
        }

        // Re-reserve all non-usable / MMIO regions from the board memory map.
        for region in regions {
            if region.kind != MemoryKind::UsableRam {
                self.mark_range_used(region.base, region.end());
            }
        }

        // Protect the running kernel image (.text, .rodata, .data, .bss, .stack).
        self.mark_range_used(layout.kernel_start, layout.kernel_end);

        // Protect firmware-supplied DTB blob if present in RAM.
        if let Some((dtb_start, dtb_size)) = dtb_span {
            self.mark_range_used(dtb_start, dtb_start.saturating_add(dtb_size));
        }

        self.initialized = true;
    }

    fn mark_range_free(&mut self, start: usize, end: usize) {
        let first = align_up(start, PAGE_SIZE) / PAGE_SIZE;
        let last = (align_down(end, PAGE_SIZE) / PAGE_SIZE).min(MAX_FRAMES);
        for frame in first..last {
            if self.is_used(frame) {
                self.clear_used(frame);
                self.total_ram_frames += 1;
                self.free_frames += 1;
            }
        }
    }

    fn mark_range_used(&mut self, start: usize, end: usize) {
        let first = align_down(start, PAGE_SIZE) / PAGE_SIZE;
        let last = (align_up(end, PAGE_SIZE) / PAGE_SIZE).min(MAX_FRAMES);
        for frame in first..last {
            if !self.is_used(frame) {
                self.set_used(frame);
                self.free_frames = self.free_frames.saturating_sub(1);
            }
        }
    }

    fn alloc_frame(&mut self) -> Option<PhysAddr> {
        if !self.initialized || self.free_frames == 0 {
            return None;
        }
        for (word_idx, word) in self.used_bits.iter_mut().enumerate() {
            if *word != u64::MAX {
                let bit = (!*word).trailing_zeros() as usize;
                *word |= 1u64 << bit;
                self.free_frames -= 1;
                let frame_idx = word_idx * 64 + bit;
                return Some(PhysAddr::from_frame_index(frame_idx));
            }
        }
        None
    }

    fn alloc_contiguous(&mut self, count: usize) -> Option<PhysAddr> {
        if !self.initialized || count == 0 || self.free_frames < count {
            return None;
        }
        if count == 1 {
            return self.alloc_frame();
        }

        let mut run_start = 0usize;
        let mut run_len = 0usize;
        for frame in 0..MAX_FRAMES {
            if !self.is_used(frame) {
                if run_len == 0 {
                    run_start = frame;
                }
                run_len += 1;
                if run_len == count {
                    for f in run_start..(run_start + count) {
                        self.set_used(f);
                    }
                    self.free_frames -= count;
                    return Some(PhysAddr::from_frame_index(run_start));
                }
            } else {
                run_len = 0;
            }
        }
        None
    }

    fn free_frame(&mut self, addr: PhysAddr) -> Result<(), PmmError> {
        if !addr.is_page_aligned() {
            return Err(PmmError::UnalignedAddress);
        }
        let frame = addr.frame_index();
        if frame >= MAX_FRAMES {
            return Err(PmmError::OutOfRange);
        }
        if !self.is_used(frame) {
            return Err(PmmError::DoubleFree);
        }
        self.clear_used(frame);
        self.free_frames += 1;
        Ok(())
    }

    fn is_used(&self, frame: usize) -> bool {
        let word = frame / 64;
        let bit = frame % 64;
        (self.used_bits[word] & (1u64 << bit)) != 0
    }

    fn set_used(&mut self, frame: usize) {
        let word = frame / 64;
        let bit = frame % 64;
        self.used_bits[word] |= 1u64 << bit;
    }

    fn clear_used(&mut self, frame: usize) {
        let word = frame / 64;
        let bit = frame % 64;
        self.used_bits[word] &= !(1u64 << bit);
    }

    fn stats(&self) -> PmmStats {
        PmmStats {
            total_ram_frames: self.total_ram_frames,
            free_frames: self.free_frames,
            used_frames: self.total_ram_frames.saturating_sub(self.free_frames),
        }
    }
}

static PMM: IrqSpinLock<BitmapAllocator> = IrqSpinLock::new(BitmapAllocator::new());

/// Initializes the global physical frame allocator.
pub fn init(
    regions: &[MemoryRegion],
    dtb_ram: Option<(u64, u64)>,
    dtb_span: Option<(usize, usize)>,
    layout: &KernelLayout,
) {
    PMM.lock().init(regions, dtb_ram, dtb_span, layout);
}

/// Allocates a single 4 KiB physical frame.
pub fn alloc_frame() -> Option<PhysAddr> {
    PMM.lock().alloc_frame()
}

/// Allocates a single 4 KiB physical frame and fills all 4096 bytes with zero.
pub fn alloc_zeroed_frame() -> Option<PhysAddr> {
    let frame = alloc_frame()?;
    let ptr = phys_to_mut_ptr::<u8>(frame);
    // SAFETY: `frame` is a freshly allocated, exclusive 4 KiB physical page, and
    // `phys_to_mut_ptr` returns an accessible virtual pointer (identity before MMU,
    // higher-half `TTBR1_EL1` after MMU activation).
    unsafe {
        write_bytes(ptr, 0, PAGE_SIZE);
    }
    Some(frame)
}

/// Allocates `count` contiguous 4 KiB physical frames (e.g. for kernel thread stacks).
pub fn alloc_contiguous_frames(count: usize) -> Option<PhysAddr> {
    PMM.lock().alloc_contiguous(count)
}

/// Frees a previously allocated 4 KiB physical frame.
pub fn free_frame(addr: PhysAddr) -> Result<(), PmmError> {
    PMM.lock().free_frame(addr)
}

/// Returns current PMM frame counts.
pub fn stats() -> PmmStats {
    PMM.lock().stats()
}

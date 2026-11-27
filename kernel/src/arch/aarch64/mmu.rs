//! AArch64 4-level (4 KiB granule) MMU setup for higher-half kernel (`TTBR1_EL1`)
//! and isolated per-process EL0 address spaces (`TTBR0_EL1`) with W^X enforcement.

use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use hal_core::{
    MemoryKind, MemoryRegion, PAGE_SIZE, PhysAddr, VirtAddr, mem::align_down, mem::align_up,
};

use crate::mem::{HIGHER_HALF_OFFSET, KernelLayout, phys_to_mut_ptr, pmm};

const BLOCK_2M: usize = 2 * 1024 * 1024;
const ENTRIES_PER_TABLE: usize = 512;

/// Upper bound of lower-half EL0 virtual address space (`TTBR0_EL1`).
pub const USER_VA_LIMIT: usize = 0x0000_8000_0000_0000;

const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE: u64 = 1 << 1;
const DESC_PAGE: u64 = 1 << 1;
const DESC_BLOCK: u64 = 0 << 1;

const ATTR_NORMAL: u64 = 0 << 2;
const ATTR_DEVICE: u64 = 1 << 2;

const AP_SHIFT: usize = 6;
const AP_EL1_RW: u64 = 0b00 << AP_SHIFT;
const AP_EL0_RW: u64 = 0b01 << AP_SHIFT;
const AP_EL1_RO: u64 = 0b10 << AP_SHIFT;
const AP_EL0_RO: u64 = 0b11 << AP_SHIFT;

const SH_NON_SHAREABLE: u64 = 0b00 << 8;
const SH_INNER_SHAREABLE: u64 = 0b11 << 8;

const AF: u64 = 1 << 10;
const NG: u64 = 1 << 11;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

const PA_MASK: u64 = 0x0000_FFFF_FFFF_F000;

static KERNEL_TTBR0_IDLE: AtomicUsize = AtomicUsize::new(0);
static KERNEL_TTBR1_ROOT: AtomicUsize = AtomicUsize::new(0);

/// Memory and access attributes for kernel (`TTBR1_EL1`) page mappings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageAttrs {
    /// Executable, read-only kernel code (`.text`).
    KernelText,
    /// Non-executable, read-only kernel constants (`.rodata`).
    KernelRoData,
    /// Non-executable, read-write kernel data/RAM (`.data`, `.bss`, `.stack`, physmap).
    KernelRwData,
    /// Non-executable, non-cacheable `Device-nGnRnE` MMIO registers.
    DeviceMmio,
}

impl PageAttrs {
    const fn descriptor_bits(self) -> u64 {
        match self {
            Self::KernelText => ATTR_NORMAL | AP_EL1_RO | SH_INNER_SHAREABLE | AF | UXN,
            Self::KernelRoData => ATTR_NORMAL | AP_EL1_RO | SH_INNER_SHAREABLE | AF | PXN | UXN,
            Self::KernelRwData => ATTR_NORMAL | AP_EL1_RW | SH_INNER_SHAREABLE | AF | PXN | UXN,
            Self::DeviceMmio => ATTR_DEVICE | AP_EL1_RW | SH_NON_SHAREABLE | AF | PXN | UXN,
        }
    }
}

/// Memory and access attributes for EL0 userspace (`TTBR0_EL1`) 4 KiB pages.
///
/// Note that `PXN` is always set on all user pages so EL1 never executes EL0 code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserPageAttrs {
    /// Read-only, executable user code (`PF_R | PF_X`).
    Code,
    /// Read-only, non-executable user constants (`PF_R`).
    RoData,
    /// Read-write, non-executable user data/stack (`PF_R | PF_W`).
    RwData,
    /// Read-write, non-executable `Device-nGnRnE` MMIO mapped to a driver server.
    DeviceMmio,
}

impl UserPageAttrs {
    const fn descriptor_bits(self) -> u64 {
        match self {
            Self::Code => ATTR_NORMAL | AP_EL0_RO | SH_INNER_SHAREABLE | AF | NG | PXN,
            Self::RoData => ATTR_NORMAL | AP_EL0_RO | SH_INNER_SHAREABLE | AF | NG | PXN | UXN,
            Self::RwData => ATTR_NORMAL | AP_EL0_RW | SH_INNER_SHAREABLE | AF | NG | PXN | UXN,
            Self::DeviceMmio => ATTR_DEVICE | AP_EL0_RW | SH_NON_SHAREABLE | AF | NG | PXN | UXN,
        }
    }
}

/// Errors returned while constructing or querying page tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuError {
    OutOfFrames,
    UnalignedAddress,
    DescriptorConflict,
    InvalidUserAddress,
}

/// Active kernel page-table roots (`TTBR0_EL1` empty idle + `TTBR1_EL1` higher-half kernel).
#[derive(Debug, Clone, Copy)]
pub struct MmuConfig {
    pub ttbr0_root: PhysAddr,
    pub ttbr1_root: PhysAddr,
    pub pages_mapped_4k: usize,
    pub blocks_mapped_2m: usize,
}

#[repr(C, align(4096))]
struct PageTable {
    entries: [u64; ENTRIES_PER_TABLE],
}

#[derive(Default)]
struct MapCounters {
    pages_4k: usize,
    blocks_2m: usize,
}

/// Builds the full W^X higher-half (`TTBR1_EL1`) page table + empty kernel-idle `TTBR0_EL1`,
/// replacing the temporary boot tables created in `boot.S`.
pub fn init(
    regions: &[MemoryRegion],
    dtb_ram: Option<(u64, u64)>,
    layout: &KernelLayout,
) -> Result<MmuConfig, MmuError> {
    let ttbr0_idle = pmm::alloc_zeroed_frame().ok_or(MmuError::OutOfFrames)?;
    let ttbr1_root = pmm::alloc_zeroed_frame().ok_or(MmuError::OutOfFrames)?;
    let mut counters = MapCounters::default();

    let dtb_ram_end = dtb_ram.map(|(base, size)| base.saturating_add(size) as usize);

    // Map page 0 (`0x0..0x1000`) RW+XN for firmware SMP spin-table mailboxes (`0xE0/0xE8/0xF0`),
    // and `[0x1000..kernel_start)` RO+XN so ATAGs/DTB remain readable without accidental writes.
    if layout.kernel_start >= PAGE_SIZE {
        map_higher_half_range(
            ttbr1_root,
            0,
            PAGE_SIZE,
            PageAttrs::KernelRwData,
            &mut counters,
        )?;
        if layout.kernel_start > PAGE_SIZE {
            map_higher_half_range(
                ttbr1_root,
                PAGE_SIZE,
                layout.kernel_start - PAGE_SIZE,
                PageAttrs::KernelRoData,
                &mut counters,
            )?;
        }
    }

    // Map Delta Kernel sections with strict 4 KiB W^X boundaries in TTBR1_EL1.
    map_higher_half_range(
        ttbr1_root,
        layout.text_start,
        layout.text_end.saturating_sub(layout.text_start),
        PageAttrs::KernelText,
        &mut counters,
    )?;
    map_higher_half_range(
        ttbr1_root,
        layout.rodata_start,
        layout.rodata_end.saturating_sub(layout.rodata_start),
        PageAttrs::KernelRoData,
        &mut counters,
    )?;
    map_higher_half_range(
        ttbr1_root,
        layout.rw_start,
        layout.kernel_end.saturating_sub(layout.rw_start),
        PageAttrs::KernelRwData,
        &mut counters,
    )?;

    // Map remaining usable physical RAM and all board Device MMIO regions into TTBR1_EL1.
    for region in regions {
        match region.kind {
            MemoryKind::UsableRam => {
                let start = align_up(region.base.max(layout.kernel_end), PAGE_SIZE);
                let mut end = align_down(region.end(), PAGE_SIZE);
                if let Some(ceiling) = dtb_ram_end {
                    end = end.min(align_down(ceiling, PAGE_SIZE));
                }
                if end > start {
                    map_higher_half_range(
                        ttbr1_root,
                        start,
                        end - start,
                        PageAttrs::KernelRwData,
                        &mut counters,
                    )?;
                }
            }
            MemoryKind::DeviceMmio => {
                let start = align_down(region.base, PAGE_SIZE);
                let end = align_up(region.end(), PAGE_SIZE);
                map_higher_half_range(
                    ttbr1_root,
                    start,
                    end.saturating_sub(start),
                    PageAttrs::DeviceMmio,
                    &mut counters,
                )?;
            }
            MemoryKind::Reserved => {
                if region.base >= layout.kernel_end {
                    let start = align_down(region.base, PAGE_SIZE);
                    let end = align_up(region.end(), PAGE_SIZE);
                    map_higher_half_range(
                        ttbr1_root,
                        start,
                        end.saturating_sub(start),
                        PageAttrs::KernelRoData,
                        &mut counters,
                    )?;
                }
            }
        }
    }

    KERNEL_TTBR0_IDLE.store(ttbr0_idle.as_usize(), Ordering::Release);
    KERNEL_TTBR1_ROOT.store(ttbr1_root.as_usize(), Ordering::Release);

    // SAFETY: `ttbr1_root` maps all running EL1 code, stacks, vectors, RAM, and MMIO;
    // switching `TTBR0_EL1` to `ttbr0_idle` removes lower-half kernel aliases cleanly.
    unsafe {
        install_root_tables(ttbr0_idle, ttbr1_root);
    }

    Ok(MmuConfig {
        ttbr0_root: ttbr0_idle,
        ttbr1_root,
        pages_mapped_4k: counters.pages_4k,
        blocks_mapped_2m: counters.blocks_2m,
    })
}

/// Installs the shared higher-half `TTBR1_EL1` and empty idle `TTBR0_EL1` on a secondary core.
///
/// # Safety
///
/// Must be called only on a secondary core executing in higher-half `TTBR1` after [`init`]
/// completed on Core 0.
pub unsafe fn enable_secondary() {
    let ttbr0 = PhysAddr(KERNEL_TTBR0_IDLE.load(Ordering::Acquire));
    let ttbr1 = PhysAddr(KERNEL_TTBR1_ROOT.load(Ordering::Acquire));
    debug_assert!(ttbr0.as_usize() != 0 && ttbr1.as_usize() != 0);
    // SAFETY: Core 0 initialized both root tables before waking secondary cores.
    unsafe {
        install_root_tables(ttbr0, ttbr1);
    }
}

/// Returns the physical address of the empty kernel-idle `TTBR0_EL1` table.
pub fn kernel_idle_ttbr0() -> PhysAddr {
    PhysAddr(KERNEL_TTBR0_IDLE.load(Ordering::Acquire))
}

/// Allocates a fresh, empty `TTBR0_EL1` L0 root page table for an EL0 user process.
pub fn create_user_l0() -> Result<PhysAddr, MmuError> {
    pmm::alloc_zeroed_frame().ok_or(MmuError::OutOfFrames)
}

/// Maps a single 4 KiB page into an EL0 process's `TTBR0_EL1` table.
pub fn map_user_page(
    user_l0: PhysAddr,
    va: VirtAddr,
    pa: PhysAddr,
    attrs: UserPageAttrs,
) -> Result<(), MmuError> {
    if va.as_usize() >= USER_VA_LIMIT || !va.is_page_aligned() || !pa.is_page_aligned() {
        return Err(MmuError::InvalidUserAddress);
    }
    let l1_pa = ensure_table_entry(user_l0, va.l0_index())?;
    let l2_pa = ensure_table_entry(l1_pa, va.l1_index())?;
    let l3_pa = ensure_table_entry(l2_pa, va.l2_index())?;
    let l3 = table_mut(l3_pa);
    l3.entries[va.l3_index()] =
        (pa.as_usize() as u64 & PA_MASK) | attrs.descriptor_bits() | DESC_PAGE | DESC_VALID;
    Ok(())
}

/// Unmaps a single 4 KiB page from an EL0 process's `TTBR0_EL1` table and invalidates TLB.
pub fn unmap_user_page(user_l0: PhysAddr, va: VirtAddr) -> Option<PhysAddr> {
    if va.as_usize() >= USER_VA_LIMIT || !va.is_page_aligned() {
        return None;
    }
    let l0 = table_ref(user_l0);
    let e0 = l0.entries[va.l0_index()];
    if (e0 & (DESC_VALID | DESC_TABLE)) != (DESC_VALID | DESC_TABLE) {
        return None;
    }
    let l1 = table_ref(PhysAddr((e0 & PA_MASK) as usize));
    let e1 = l1.entries[va.l1_index()];
    if (e1 & (DESC_VALID | DESC_TABLE)) != (DESC_VALID | DESC_TABLE) {
        return None;
    }
    let l2 = table_ref(PhysAddr((e1 & PA_MASK) as usize));
    let e2 = l2.entries[va.l2_index()];
    if (e2 & (DESC_VALID | DESC_TABLE)) != (DESC_VALID | DESC_TABLE) {
        return None;
    }
    let l3 = table_mut(PhysAddr((e2 & PA_MASK) as usize));
    let old = l3.entries[va.l3_index()];
    if (old & DESC_VALID) == 0 {
        return None;
    }
    l3.entries[va.l3_index()] = 0;
    // SAFETY: `tlbi vmalle1is` invalidates stale EL0 translation entries after clearing L3 PTE.
    unsafe {
        asm!(
            "dsb ishst",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack)
        );
    }
    Some(PhysAddr((old & PA_MASK) as usize))
}

/// Resolves an EL0 virtual address inside `user_l0` and returns `(pa, is_user_writable)`.
pub fn translate_user_page(user_l0: PhysAddr, va: VirtAddr) -> Option<(PhysAddr, bool)> {
    if va.as_usize() >= USER_VA_LIMIT {
        return None;
    }
    let l0 = table_ref(user_l0);
    let e0 = l0.entries[va.l0_index()];
    if (e0 & (DESC_VALID | DESC_TABLE)) != (DESC_VALID | DESC_TABLE) {
        return None;
    }
    let l1 = table_ref(PhysAddr((e0 & PA_MASK) as usize));
    let e1 = l1.entries[va.l1_index()];
    if (e1 & (DESC_VALID | DESC_TABLE)) != (DESC_VALID | DESC_TABLE) {
        return None;
    }
    let l2 = table_ref(PhysAddr((e1 & PA_MASK) as usize));
    let e2 = l2.entries[va.l2_index()];
    if (e2 & (DESC_VALID | DESC_TABLE)) != (DESC_VALID | DESC_TABLE) {
        return None;
    }
    let l3 = table_ref(PhysAddr((e2 & PA_MASK) as usize));
    let e3 = l3.entries[va.l3_index()];
    if (e3 & (DESC_VALID | DESC_PAGE)) != (DESC_VALID | DESC_PAGE) {
        return None;
    }
    let ap = (e3 >> AP_SHIFT) & 0b11;
    let user_accessible = (ap & 0b01) != 0;
    if !user_accessible {
        return None;
    }
    let writable = ap == 0b01;
    let pa = PhysAddr(((e3 & PA_MASK) as usize) | va.page_offset());
    Some((pa, writable))
}

/// Switches the active `TTBR0_EL1` user address space on the current core and flushes local TLB.
///
/// # Safety
///
/// `ttbr0_root` must be a valid 4 KiB-aligned L0 page table frame (or [`kernel_idle_ttbr0`]).
pub unsafe fn switch_ttbr0(ttbr0_root: PhysAddr) {
    // SAFETY: Caller supplies a valid L0 table root; `tlbi vmalle1` ensures no stale EL0
    // translations persist across address space switches.
    unsafe {
        asm!(
            "msr ttbr0_el1, {ttbr0}",
            "dsb ish",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            ttbr0 = in(reg) ttbr0_root.as_usize() as u64,
            options(nostack)
        );
    }
}

/// Resolves a virtual address to its physical address by walking `root_l0`.
pub fn translate(root_l0: PhysAddr, va: VirtAddr) -> Option<PhysAddr> {
    let l0 = table_ref(root_l0);
    let e0 = l0.entries[va.l0_index()];
    if (e0 & DESC_VALID) == 0 || (e0 & DESC_TABLE) == 0 {
        return None;
    }

    let l1 = table_ref(PhysAddr((e0 & PA_MASK) as usize));
    let e1 = l1.entries[va.l1_index()];
    if (e1 & DESC_VALID) == 0 {
        return None;
    }
    if (e1 & DESC_TABLE) == 0 {
        let base = (e1 & 0x0000_FFFF_C000_0000) as usize;
        return Some(PhysAddr(base | (va.as_usize() & 0x3FFF_FFFF)));
    }

    let l2 = table_ref(PhysAddr((e1 & PA_MASK) as usize));
    let e2 = l2.entries[va.l2_index()];
    if (e2 & DESC_VALID) == 0 {
        return None;
    }
    if (e2 & DESC_TABLE) == 0 {
        let base = (e2 & 0x0000_FFFF_FFE0_0000) as usize;
        return Some(PhysAddr(base | (va.as_usize() & (BLOCK_2M - 1))));
    }

    let l3 = table_ref(PhysAddr((e2 & PA_MASK) as usize));
    let e3 = l3.entries[va.l3_index()];
    if (e3 & (DESC_VALID | DESC_PAGE)) != (DESC_VALID | DESC_PAGE) {
        return None;
    }
    let base = (e3 & PA_MASK) as usize;
    Some(PhysAddr(base | va.page_offset()))
}

fn map_higher_half_range(
    ttbr1_root: PhysAddr,
    phys_start: usize,
    size: usize,
    attrs: PageAttrs,
    counters: &mut MapCounters,
) -> Result<(), MmuError> {
    if size == 0 {
        return Ok(());
    }
    map_range(
        ttbr1_root,
        VirtAddr(HIGHER_HALF_OFFSET.saturating_add(phys_start)),
        PhysAddr(phys_start),
        size,
        attrs,
        counters,
    )
}

fn map_range(
    root_l0: PhysAddr,
    va_start: VirtAddr,
    pa_start: PhysAddr,
    size: usize,
    attrs: PageAttrs,
    counters: &mut MapCounters,
) -> Result<(), MmuError> {
    if !va_start.is_page_aligned() || !pa_start.is_page_aligned() || !size.is_multiple_of(PAGE_SIZE)
    {
        return Err(MmuError::UnalignedAddress);
    }

    let mut offset = 0usize;
    while offset < size {
        let cur_va = VirtAddr(va_start.as_usize() + offset);
        let cur_pa = PhysAddr(pa_start.as_usize() + offset);
        let remaining = size - offset;

        if cur_va.as_usize().is_multiple_of(BLOCK_2M)
            && cur_pa.as_usize().is_multiple_of(BLOCK_2M)
            && remaining >= BLOCK_2M
            && try_map_block_2m(root_l0, cur_va, cur_pa, attrs)?
        {
            counters.blocks_2m += 1;
            offset += BLOCK_2M;
            continue;
        }

        map_page_4k(root_l0, cur_va, cur_pa, attrs)?;
        counters.pages_4k += 1;
        offset += PAGE_SIZE;
    }

    Ok(())
}

fn try_map_block_2m(
    root_l0: PhysAddr,
    va: VirtAddr,
    pa: PhysAddr,
    attrs: PageAttrs,
) -> Result<bool, MmuError> {
    let l1_pa = ensure_table_entry(root_l0, va.l0_index())?;
    let l2_pa = ensure_table_entry(l1_pa, va.l1_index())?;
    let l2 = table_mut(l2_pa);
    let idx = va.l2_index();
    let existing = l2.entries[idx];

    if (existing & DESC_VALID) != 0 {
        return Ok(false);
    }

    l2.entries[idx] =
        (pa.as_usize() as u64 & PA_MASK) | attrs.descriptor_bits() | DESC_BLOCK | DESC_VALID;
    Ok(true)
}

fn map_page_4k(
    root_l0: PhysAddr,
    va: VirtAddr,
    pa: PhysAddr,
    attrs: PageAttrs,
) -> Result<(), MmuError> {
    let l1_pa = ensure_table_entry(root_l0, va.l0_index())?;
    let l2_pa = ensure_table_entry(l1_pa, va.l1_index())?;
    let l3_pa = ensure_table_entry(l2_pa, va.l2_index())?;
    let l3 = table_mut(l3_pa);
    l3.entries[va.l3_index()] =
        (pa.as_usize() as u64 & PA_MASK) | attrs.descriptor_bits() | DESC_PAGE | DESC_VALID;
    Ok(())
}

fn ensure_table_entry(table_pa: PhysAddr, index: usize) -> Result<PhysAddr, MmuError> {
    let table = table_mut(table_pa);
    let entry = table.entries[index];

    if (entry & DESC_VALID) == 0 {
        let next_pa = pmm::alloc_zeroed_frame().ok_or(MmuError::OutOfFrames)?;
        table.entries[index] = (next_pa.as_usize() as u64 & PA_MASK) | DESC_TABLE | DESC_VALID;
        return Ok(next_pa);
    }

    if (entry & DESC_TABLE) == 0 {
        return Err(MmuError::DescriptorConflict);
    }

    Ok(PhysAddr((entry & PA_MASK) as usize))
}

fn table_ref(pa: PhysAddr) -> &'static PageTable {
    let ptr = phys_to_mut_ptr::<PageTable>(pa);
    // SAFETY: `pa` points to a 4 KiB-aligned page table frame allocated by PMM and
    // accessible through the active `TTBR1_EL1` higher-half physmap.
    unsafe { &*ptr }
}

fn table_mut(pa: PhysAddr) -> &'static mut PageTable {
    let ptr = phys_to_mut_ptr::<PageTable>(pa);
    // SAFETY: `pa` points to an exclusive 4 KiB-aligned page table frame during
    // single-core or locked page-table updates.
    unsafe { &mut *ptr }
}

unsafe fn install_root_tables(ttbr0_root: PhysAddr, ttbr1_root: PhysAddr) {
    // SAFETY: Caller verified `ttbr1_root` maps all running EL1 code, stacks, vectors,
    // RAM, and MMIO; TLB invalidation drops any stale boot translations.
    unsafe {
        asm!(
            "dsb ishst",
            "msr ttbr0_el1, {ttbr0}",
            "msr ttbr1_el1, {ttbr1}",
            "dsb ish",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            ttbr0 = in(reg) ttbr0_root.as_usize() as u64,
            ttbr1 = in(reg) ttbr1_root.as_usize() as u64,
            options(nostack)
        );
    }
}

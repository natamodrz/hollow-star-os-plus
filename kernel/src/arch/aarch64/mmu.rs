//! AArch64 4-level (4 KiB granule) MMU setup for identity (`TTBR0_EL1`) and
//! higher-half (`TTBR1_EL1`) kernel paging with W^X section permissions.

use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use hal_core::{
    MemoryKind, MemoryRegion, PAGE_SIZE, PhysAddr, VirtAddr, mem::align_down, mem::align_up,
};

use crate::arch;
use crate::mem::{HIGHER_HALF_OFFSET, KernelLayout, mark_higher_half_active, phys_to_mut_ptr, pmm};

static BOOT_TTBR0_ROOT: AtomicUsize = AtomicUsize::new(0);
static BOOT_TTBR1_ROOT: AtomicUsize = AtomicUsize::new(0);

const BLOCK_2M: usize = 2 * 1024 * 1024;
const ENTRIES_PER_TABLE: usize = 512;

const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE: u64 = 1 << 1;
const DESC_PAGE: u64 = 1 << 1;
const DESC_BLOCK: u64 = 0 << 1;

const ATTR_NORMAL: u64 = 0 << 2;
const ATTR_DEVICE: u64 = 1 << 2;

const AP_EL1_RW: u64 = 0b00 << 6;
const AP_EL1_RO: u64 = 0b10 << 6;

const SH_NON_SHAREABLE: u64 = 0b00 << 8;
const SH_INNER_SHAREABLE: u64 = 0b11 << 8;

const AF: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

const PA_MASK: u64 = 0x0000_FFFF_FFFF_F000;

/// Memory and access attributes for virtual page mappings.
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

/// Errors returned while constructing or querying page tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuError {
    OutOfFrames,
    UnalignedAddress,
    DescriptorConflict,
}

/// Active kernel page-table roots (`TTBR0_EL1` identity + `TTBR1_EL1` higher-half).
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

/// Builds identity (`TTBR0_EL1`) and higher-half (`TTBR1_EL1`) page tables and enables the MMU.
pub fn init(
    regions: &[MemoryRegion],
    dtb_ram: Option<(u64, u64)>,
    layout: &KernelLayout,
) -> Result<MmuConfig, MmuError> {
    let ttbr0_root = pmm::alloc_zeroed_frame().ok_or(MmuError::OutOfFrames)?;
    let ttbr1_root = pmm::alloc_zeroed_frame().ok_or(MmuError::OutOfFrames)?;
    let mut counters = MapCounters::default();

    let dtb_ram_end = dtb_ram.map(|(base, size)| base.saturating_add(size) as usize);

    // Map page 0 (`0x0..0x1000`) RW+XN for firmware SMP spin-table mailboxes (`0xE0/0xE8/0xF0`),
    // and `[0x1000..kernel_start)` RO+XN so ATAGs/DTB remain readable without accidental writes.
    if layout.kernel_start >= PAGE_SIZE {
        map_dual_range(
            ttbr0_root,
            ttbr1_root,
            0,
            PAGE_SIZE,
            PageAttrs::KernelRwData,
            &mut counters,
        )?;
        if layout.kernel_start > PAGE_SIZE {
            map_dual_range(
                ttbr0_root,
                ttbr1_root,
                PAGE_SIZE,
                layout.kernel_start - PAGE_SIZE,
                PageAttrs::KernelRoData,
                &mut counters,
            )?;
        }
    }

    // Map Delta Kernel sections with strict 4 KiB W^X boundaries.
    map_dual_range(
        ttbr0_root,
        ttbr1_root,
        layout.text_start,
        layout.text_end.saturating_sub(layout.text_start),
        PageAttrs::KernelText,
        &mut counters,
    )?;
    map_dual_range(
        ttbr0_root,
        ttbr1_root,
        layout.rodata_start,
        layout.rodata_end.saturating_sub(layout.rodata_start),
        PageAttrs::KernelRoData,
        &mut counters,
    )?;
    map_dual_range(
        ttbr0_root,
        ttbr1_root,
        layout.rw_start,
        layout.kernel_end.saturating_sub(layout.rw_start),
        PageAttrs::KernelRwData,
        &mut counters,
    )?;

    // Map remaining usable physical RAM and all board Device MMIO regions.
    for region in regions {
        match region.kind {
            MemoryKind::UsableRam => {
                let start = align_up(region.base.max(layout.kernel_end), PAGE_SIZE);
                let mut end = align_down(region.end(), PAGE_SIZE);
                if let Some(ceiling) = dtb_ram_end {
                    end = end.min(align_down(ceiling, PAGE_SIZE));
                }
                if end > start {
                    map_dual_range(
                        ttbr0_root,
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
                map_dual_range(
                    ttbr0_root,
                    ttbr1_root,
                    start,
                    end.saturating_sub(start),
                    PageAttrs::DeviceMmio,
                    &mut counters,
                )?;
            }
            MemoryKind::Reserved => {
                // Low reserved range before `kernel_start` is already mapped above;
                // map any extra board reserved slices conservatively as RO+XN.
                if region.base >= layout.kernel_end {
                    let start = align_down(region.base, PAGE_SIZE);
                    let end = align_up(region.end(), PAGE_SIZE);
                    map_dual_range(
                        ttbr0_root,
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

    BOOT_TTBR0_ROOT.store(ttbr0_root.as_usize(), Ordering::Release);
    BOOT_TTBR1_ROOT.store(ttbr1_root.as_usize(), Ordering::Release);

    // SAFETY: Both `ttbr0_root` and `ttbr1_root` are fully populated, valid 4 KiB-aligned
    // L0 page-table hierarchies covering active code, stack, vectors, RAM, and MMIO.
    unsafe {
        enable_mmu(ttbr0_root, ttbr1_root);
    }
    mark_higher_half_active();

    // Flush root page-table pointers to DRAM PoC so secondary cores with MMU/caches off
    // can read `BOOT_TTBR0_ROOT` and `BOOT_TTBR1_ROOT` reliably on wakeup.
    arch::clean_dcache_line(&raw const BOOT_TTBR0_ROOT as usize);
    arch::clean_dcache_line(&raw const BOOT_TTBR1_ROOT as usize);

    Ok(MmuConfig {
        ttbr0_root,
        ttbr1_root,
        pages_mapped_4k: counters.pages_4k,
        blocks_mapped_2m: counters.blocks_2m,
    })
}

/// Enables the primary core's shared page tables on a newly woken secondary core.
///
/// # Safety
///
/// Must be called only on a secondary core in EL1 after [`init`] completed on Core 0.
pub unsafe fn enable_secondary() {
    let ttbr0 = PhysAddr(BOOT_TTBR0_ROOT.load(Ordering::Acquire));
    let ttbr1 = PhysAddr(BOOT_TTBR1_ROOT.load(Ordering::Acquire));
    debug_assert!(ttbr0.as_usize() != 0 && ttbr1.as_usize() != 0);
    // SAFETY: Primary core initialized `ttbr0`/`ttbr1` and flushed them to DRAM PoC.
    unsafe {
        enable_mmu(ttbr0, ttbr1);
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

fn map_dual_range(
    ttbr0_root: PhysAddr,
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
        ttbr0_root,
        VirtAddr(phys_start),
        PhysAddr(phys_start),
        size,
        attrs,
        counters,
    )?;
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
        // An existing L3 table covers part of this 2 MiB region; fall back to 4 KiB pages.
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
    // accessible through the active identity or higher-half kernel mapping.
    unsafe { &*ptr }
}

fn table_mut(pa: PhysAddr) -> &'static mut PageTable {
    let ptr = phys_to_mut_ptr::<PageTable>(pa);
    // SAFETY: `pa` points to an exclusive 4 KiB-aligned page table frame during
    // single-core MMU initialization.
    unsafe { &mut *ptr }
}

unsafe fn enable_mmu(ttbr0_root: PhysAddr, ttbr1_root: PhysAddr) {
    // Attr0=0xFF (Normal WB/WA), Attr1=0x00 (Device-nGnRnE), Attr2=0x44 (Normal NC)
    let mair: u64 = 0x0000_0000_0044_00FF;

    let mmfr0: u64;
    // SAFETY: Reading `ID_AA64MMFR0_EL1` at EL1 is read-only and returns supported PA range.
    unsafe {
        asm!("mrs {0}, id_aa64mmfr0_el1", out(reg) mmfr0, options(nomem, nostack, preserves_flags));
    }
    let ips = (mmfr0 & 0xF).min(5);

    // 48-bit VA (T0SZ=16, T1SZ=16), 4 KiB granules (TG0=0b00, TG1=0b10), Inner Shareable WB/WA.
    let tcr: u64 = 16
        | (0b01 << 8)
        | (0b01 << 10)
        | (0b11 << 12)
        | (16 << 16)
        | (0b01 << 24)
        | (0b01 << 26)
        | (0b11 << 28)
        | (0b10 << 30)
        | (ips << 32);

    let mut sctlr: u64;
    // SAFETY: Reading `SCTLR_EL1` at EL1 is valid before enabling translation.
    unsafe {
        asm!("mrs {0}, sctlr_el1", out(reg) sctlr, options(nomem, nostack, preserves_flags));
    }

    // Enable MMU (M), Data Cache (C), Stack Alignment (SA/SA0), Instruction Cache (I);
    // keep little-endian (EE=0, E0E=0) and per-page W^X (WXN=0).
    sctlr |= (1 << 0) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 12);
    sctlr &= !((1 << 19) | (1 << 24) | (1 << 25));

    // SAFETY: Caller verified `ttbr0_root` identity-maps all running EL1 code/stack/MMIO
    // and `ttbr1_root` maps the higher-half alias; barriers flush stale TLBs/I-cache
    // around enabling `SCTLR_EL1.M`.
    unsafe {
        asm!(
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {ttbr0}",
            "msr ttbr1_el1, {ttbr1}",
            "dsb ishst",
            "isb",
            "tlbi vmalle1",
            "ic iallu",
            "dsb ish",
            "isb",
            "msr sctlr_el1, {sctlr}",
            "isb",
            mair = in(reg) mair,
            tcr = in(reg) tcr,
            ttbr0 = in(reg) ttbr0_root.as_usize() as u64,
            ttbr1 = in(reg) ttbr1_root.as_usize() as u64,
            sctlr = in(reg) sctlr,
            options(nostack)
        );
    }
}

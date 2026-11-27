//! Minimal `no_std` ELF64 AArch64 loader for EL0 processes.

use core::ptr::copy_nonoverlapping;

use hal_core::{PAGE_SIZE, PhysAddr, VirtAddr, mem::align_down, mem::align_up};

use crate::arch::mmu::{self, MmuError, USER_VA_LIMIT, UserPageAttrs};
use crate::mem::{phys_to_mut_ptr, pmm};

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const EM_AARCH64: u16 = 183;
const PT_LOAD: u32 = 1;

const PF_X: u32 = 1 << 0;
const PF_W: u32 = 1 << 1;

const USER_STACK_BASE: usize = 0x0000_7FFF_F000_0000;
const USER_STACK_PAGES: usize = 4; // 16 KiB EL0 user stack

/// Errors returned while validating or loading an ELF64 executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    InvalidMagic,
    UnsupportedFormat,
    InvalidProgramHeader,
    InvalidSegmentRange,
    OutOfMemory,
    Mmu(MmuError),
}

impl From<MmuError> for ElfError {
    fn from(err: MmuError) -> Self {
        Self::Mmu(err)
    }
}

/// Result of loading an ELF64 executable into a fresh EL0 `TTBR0_EL1` space.
#[derive(Debug, Clone, Copy)]
pub struct LoadedElf {
    pub ttbr0_root: PhysAddr,
    pub entry_pc: usize,
    pub user_sp: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

/// Validates and maps an ELF64 AArch64 executable into a fresh `TTBR0_EL1` address space.
pub fn load_elf64(bytes: &[u8]) -> Result<LoadedElf, ElfError> {
    if bytes.len() < core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::TooSmall);
    }

    // SAFETY: Length >= `size_of::<Elf64Ehdr>()` checked above; `read_unaligned` avoids
    // alignment assumptions on embedded byte slices.
    let ehdr: Elf64Ehdr = unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast()) };

    if ehdr.e_ident[0..4] != ELF_MAGIC {
        return Err(ElfError::InvalidMagic);
    }
    if ehdr.e_ident[4] != ELFCLASS64
        || ehdr.e_ident[5] != ELFDATA2LSB
        || ehdr.e_type != ET_EXEC
        || ehdr.e_machine != EM_AARCH64
    {
        return Err(ElfError::UnsupportedFormat);
    }

    let phoff = ehdr.e_phoff as usize;
    let phentsize = ehdr.e_phentsize as usize;
    let phnum = ehdr.e_phnum as usize;
    if phentsize < core::mem::size_of::<Elf64Phdr>() {
        return Err(ElfError::InvalidProgramHeader);
    }
    let ph_end = phnum
        .checked_mul(phentsize)
        .and_then(|sz| phoff.checked_add(sz))
        .ok_or(ElfError::InvalidProgramHeader)?;
    if ph_end > bytes.len() {
        return Err(ElfError::InvalidProgramHeader);
    }

    let entry_pc = ehdr.e_entry as usize;
    if entry_pc == 0 || entry_pc >= USER_VA_LIMIT {
        return Err(ElfError::InvalidSegmentRange);
    }

    let ttbr0_root = mmu::create_user_l0()?;

    for i in 0..phnum {
        let off = phoff + i * phentsize;
        // SAFETY: `off + size_of::<Elf64Phdr>() <= bytes.len()` verified above.
        let phdr: Elf64Phdr = unsafe { core::ptr::read_unaligned(bytes[off..].as_ptr().cast()) };
        if phdr.p_type != PT_LOAD || phdr.p_memsz == 0 {
            continue;
        }
        map_load_segment(ttbr0_root, bytes, &phdr)?;
    }

    // Map EL0 user stack (`16 KiB`) at `USER_STACK_BASE`.
    for page_idx in 0..USER_STACK_PAGES {
        let frame = pmm::alloc_zeroed_frame().ok_or(ElfError::OutOfMemory)?;
        let va = VirtAddr(USER_STACK_BASE + page_idx * PAGE_SIZE);
        mmu::map_user_page(ttbr0_root, va, frame, UserPageAttrs::RwData)?;
    }

    Ok(LoadedElf {
        ttbr0_root,
        entry_pc,
        user_sp: USER_STACK_BASE + USER_STACK_PAGES * PAGE_SIZE,
    })
}

fn map_load_segment(
    ttbr0_root: PhysAddr,
    elf_bytes: &[u8],
    phdr: &Elf64Phdr,
) -> Result<(), ElfError> {
    let vaddr = phdr.p_vaddr as usize;
    let memsz = phdr.p_memsz as usize;
    let filesz = phdr.p_filesz as usize;
    let file_off = phdr.p_offset as usize;

    if filesz > memsz {
        return Err(ElfError::InvalidSegmentRange);
    }
    let file_end = file_off
        .checked_add(filesz)
        .ok_or(ElfError::InvalidSegmentRange)?;
    if file_end > elf_bytes.len() {
        return Err(ElfError::InvalidSegmentRange);
    }
    let vaddr_end = vaddr
        .checked_add(memsz)
        .ok_or(ElfError::InvalidSegmentRange)?;
    if vaddr == 0 || vaddr_end >= USER_VA_LIMIT {
        return Err(ElfError::InvalidSegmentRange);
    }

    let attrs = if (phdr.p_flags & PF_W) != 0 {
        UserPageAttrs::RwData
    } else if (phdr.p_flags & PF_X) != 0 {
        UserPageAttrs::Code
    } else {
        UserPageAttrs::RoData
    };

    let page_start = align_down(vaddr, PAGE_SIZE);
    let page_end = align_up(vaddr_end, PAGE_SIZE);

    let mut cur_page = page_start;
    while cur_page < page_end {
        let frame = match mmu::translate_user_page(ttbr0_root, VirtAddr(cur_page)) {
            Some((existing_pa, _)) => PhysAddr(align_down(existing_pa.as_usize(), PAGE_SIZE)),
            None => {
                let new_frame = pmm::alloc_zeroed_frame().ok_or(ElfError::OutOfMemory)?;
                mmu::map_user_page(ttbr0_root, VirtAddr(cur_page), new_frame, attrs)?;
                new_frame
            }
        };

        let copy_va_start = cur_page.max(vaddr);
        let copy_va_end = (cur_page + PAGE_SIZE).min(vaddr + filesz);
        if copy_va_end > copy_va_start {
            let page_offset = copy_va_start - cur_page;
            let src_offset = file_off + (copy_va_start - vaddr);
            let len = copy_va_end - copy_va_start;
            let dst_ptr = (phys_to_mut_ptr::<u8>(frame) as usize + page_offset) as *mut u8;
            // SAFETY: `src_offset + len <= elf_bytes.len()` and `page_offset + len <= PAGE_SIZE`
            // inside an exclusive zeroed frame accessible via `TTBR1_EL1`.
            unsafe {
                copy_nonoverlapping(elf_bytes[src_offset..].as_ptr(), dst_ptr, len);
            }
        }

        cur_page += PAGE_SIZE;
    }

    Ok(())
}

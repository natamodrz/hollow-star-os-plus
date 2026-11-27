//! OS X / Mach-inspired Dynamic Linker (`dyld`) and `.dkex` Info.plist parser (`EL0`).
//!
//! Loads genuine AArch64 `ET_DYN` ELF64 shared libraries (`/system/lib/*.dylib`) into
//! isolated `VmAllocate` pages with strict W^X enforcement via `VmProtect`, applies
//! `R_AARCH64_RELATIVE` relocations, and resolves exported C-ABI symbols (`dlsym`).

use core::cmp::max;
use core::ptr::copy_nonoverlapping;

use delta_abi::{errno, vm_prot};

const PAGE_SIZE: usize = 4096;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const DT_NULL: i64 = 0;
const DT_HASH: i64 = 4;
const DT_STRTAB: i64 = 5;
const DT_SYMTAB: i64 = 6;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_STRSZ: i64 = 10;
const R_AARCH64_RELATIVE: u32 = 1027;
const MAX_DYLIB_FILE_PAGES: usize = 16; // 64 KiB scratch for reading .dylib bytes

/// Loaded in-memory AArch64 ELF64 `.dylib` shared library (`ET_DYN`).
pub struct Dylib {
    base_va: usize,
    total_pages: usize,
    symtab_va: usize,
    strtab_va: usize,
    strtab_sz: usize,
    sym_count: usize,
}

impl Dylib {
    /// Opens and dynamically maps a `.dylib` file from VFS (e.g. `/system/lib/libdelta_kit.dylib`).
    pub fn open(vfs_cap: usize, path: &str) -> Result<Self, isize> {
        let scratch_va = crate::vm_allocate(MAX_DYLIB_FILE_PAGES)?;
        let scratch_len = MAX_DYLIB_FILE_PAGES * PAGE_SIZE;
        // SAFETY: `scratch_va` was just mapped `RW` for `MAX_DYLIB_FILE_PAGES` pages.
        let file_buf =
            unsafe { core::slice::from_raw_parts_mut(scratch_va as *mut u8, scratch_len) };

        let len = crate::vfs_read_file(vfs_cap, path, file_buf)?;
        Self::from_bytes(&file_buf[..len])
    }

    /// Maps an `ET_DYN` ELF64 `.dylib` image already resident in `bytes`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, isize> {
        if bytes.len() < 64
            || &bytes[0..4] != b"\x7fELF"
            || bytes[4] != 2
            || bytes[5] != 1
            || read_u16(bytes, 16) != 3
            || read_u16(bytes, 18) != 183
        {
            return Err(errno::EINVAL);
        }

        let phoff = read_u64(bytes, 32) as usize;
        let phentsize = usize::from(read_u16(bytes, 54));
        let phnum = usize::from(read_u16(bytes, 56));
        if phentsize < 56 || phoff.saturating_add(phentsize * phnum) > bytes.len() {
            return Err(errno::EINVAL);
        }

        let mut max_vaddr_end = 0usize;
        let mut dyn_vaddr = 0usize;
        let mut dyn_size = 0usize;

        for i in 0..phnum {
            let off = phoff + i * phentsize;
            let p_type = read_u32(bytes, off);
            let p_vaddr = read_u64(bytes, off + 16) as usize;
            let p_memsz = read_u64(bytes, off + 40) as usize;
            if p_type == PT_LOAD {
                max_vaddr_end = max(max_vaddr_end, p_vaddr.saturating_add(p_memsz));
            } else if p_type == PT_DYNAMIC {
                dyn_vaddr = p_vaddr;
                dyn_size = read_u64(bytes, off + 32) as usize;
            }
        }

        if max_vaddr_end == 0 {
            return Err(errno::EINVAL);
        }

        let total_pages = max_vaddr_end.div_ceil(PAGE_SIZE);
        let base_va = crate::vm_allocate(total_pages)?;

        // Map and copy every PT_LOAD segment while memory is still `RW`.
        for i in 0..phnum {
            let off = phoff + i * phentsize;
            if read_u32(bytes, off) != PT_LOAD {
                continue;
            }
            let p_offset = read_u64(bytes, off + 8) as usize;
            let p_vaddr = read_u64(bytes, off + 16) as usize;
            let p_filesz = read_u64(bytes, off + 32) as usize;
            let p_memsz = read_u64(bytes, off + 40) as usize;
            if p_offset.saturating_add(p_filesz) > bytes.len() || p_filesz > p_memsz {
                return Err(errno::EINVAL);
            }
            if p_filesz > 0 {
                // SAFETY: `[base_va + p_vaddr, base_va + p_vaddr + p_filesz)` lies inside allocated RW pages.
                unsafe {
                    copy_nonoverlapping(
                        bytes.as_ptr().add(p_offset),
                        (base_va + p_vaddr) as *mut u8,
                        p_filesz,
                    );
                }
            }
        }

        let (mut symtab_va, mut strtab_va, mut strtab_sz, mut hash_va) = (0, 0, 0, 0);
        let (mut rela_va, mut rela_sz) = (0usize, 0usize);

        if dyn_size >= 16 && dyn_vaddr < max_vaddr_end {
            let entries = dyn_size / 16;
            for i in 0..entries {
                let d_off = base_va + dyn_vaddr + i * 16;
                // SAFETY: `d_off` lies inside mapped PT_DYNAMIC pages.
                let (d_tag, d_val) = unsafe {
                    (
                        core::ptr::read(d_off as *const i64),
                        core::ptr::read((d_off + 8) as *const u64) as usize,
                    )
                };
                match d_tag {
                    DT_NULL => break,
                    DT_HASH => hash_va = base_va + d_val,
                    DT_STRTAB => strtab_va = base_va + d_val,
                    DT_SYMTAB => symtab_va = base_va + d_val,
                    DT_STRSZ => strtab_sz = d_val,
                    DT_RELA => rela_va = base_va + d_val,
                    DT_RELASZ => rela_sz = d_val,
                    _ => {}
                }
            }
        }

        // Apply `R_AARCH64_RELATIVE` relocations (`*(base + r_offset) = base + r_addend`).
        if rela_va != 0 && rela_sz >= 24 {
            for i in 0..(rela_sz / 24) {
                let ent = rela_va + i * 24;
                // SAFETY: `ent` points to an `Elf64_Rela` record inside allocated `.dylib` space.
                let (r_off, r_info, r_addend) = unsafe {
                    (
                        core::ptr::read(ent as *const u64) as usize,
                        core::ptr::read((ent + 8) as *const u64),
                        core::ptr::read((ent + 16) as *const i64),
                    )
                };
                if (r_info as u32) == R_AARCH64_RELATIVE && r_off + 8 <= max_vaddr_end {
                    let target = (base_va + r_off) as *mut u64;
                    let val = (base_va as i64).wrapping_add(r_addend) as u64;
                    // SAFETY: `target` is inside allocated RW `.dylib` segment.
                    unsafe {
                        core::ptr::write(target, val);
                    }
                }
            }
        }

        // Apply strict W^X protections per PT_LOAD segment (`RX`, `RW`, or `RO`).
        for i in 0..phnum {
            let off = phoff + i * phentsize;
            if read_u32(bytes, off) != PT_LOAD {
                continue;
            }
            let p_flags = read_u32(bytes, off + 4);
            let p_vaddr = read_u64(bytes, off + 16) as usize;
            let p_memsz = read_u64(bytes, off + 40) as usize;
            if p_memsz == 0 {
                continue;
            }
            let prot = if (p_flags & PF_X) != 0 {
                vm_prot::RX
            } else if (p_flags & PF_W) != 0 {
                vm_prot::RW
            } else {
                vm_prot::RO
            };
            let start_pg = p_vaddr / PAGE_SIZE;
            let end_pg = p_vaddr.saturating_add(p_memsz).div_ceil(PAGE_SIZE);
            crate::vm_protect(base_va + start_pg * PAGE_SIZE, end_pg - start_pg, prot)?;
        }

        let sym_count = if hash_va != 0 {
            // SAFETY: `DT_HASH` starts with `(nbucket: u32, nchain: u32)` where `nchain == sym_count`.
            unsafe { core::ptr::read((hash_va + 4) as *const u32) as usize }
        } else if strtab_va > symtab_va {
            (strtab_va - symtab_va) / 24
        } else {
            0
        };

        Ok(Self {
            base_va,
            total_pages,
            symtab_va,
            strtab_va,
            strtab_sz,
            sym_count,
        })
    }

    /// Returns the base EL0 virtual address where this `.dylib` is mapped.
    pub const fn base_va(&self) -> usize {
        self.base_va
    }

    /// Returns total mapped size in bytes.
    pub const fn mapped_bytes(&self) -> usize {
        self.total_pages * PAGE_SIZE
    }

    /// Returns the number of dynamic symbols exported in `.dynsym`.
    pub const fn symbol_count(&self) -> usize {
        self.sym_count
    }

    /// Resolves an exported symbol address inside this `.dylib` (`dlsym`).
    pub fn symbol(&self, name: &str) -> Option<usize> {
        if self.symtab_va == 0 || self.strtab_va == 0 {
            return None;
        }
        // SAFETY: `strtab_va` was mapped `RO` with length `strtab_sz` inside `ET_DYN`.
        let strtab =
            unsafe { core::slice::from_raw_parts(self.strtab_va as *const u8, self.strtab_sz) };

        for i in 0..self.sym_count {
            let sym = self.symtab_va + i * 24;
            // SAFETY: `sym` is inside `.dynsym` bounded by `DT_HASH.nchain`.
            let (st_name, st_shndx, st_value) = unsafe {
                (
                    core::ptr::read(sym as *const u32) as usize,
                    core::ptr::read((sym + 6) as *const u16),
                    core::ptr::read((sym + 8) as *const u64) as usize,
                )
            };
            if st_shndx == 0 || st_name >= strtab.len() {
                continue;
            }
            let tail = &strtab[st_name..];
            let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
            if &tail[..end] == name.as_bytes() {
                return Some(self.base_va + st_value);
            }
        }
        None
    }

    /// Invokes a zero-argument `extern "C" fn() -> u32` exported from this `.dylib`.
    ///
    /// # Safety
    ///
    /// Caller must ensure `sym_name` names an `extern "C" fn() -> u32` symbol.
    pub unsafe fn call_u32(&self, sym_name: &str) -> Option<u32> {
        let addr = self.symbol(sym_name)?;
        // SAFETY: Address was validated against `.dynsym` inside our `RX`-mapped `.dylib`.
        let func: extern "C" fn() -> u32 = unsafe { core::mem::transmute(addr) };
        Some(func())
    }
}

/// Extracts `<key>{key}</key>...<string>{value}</string>` from a lightweight OS X `.dkex` `Info.plist`.
pub fn parse_plist_string<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = plist;
    while let Some(k_start) = rest.find("<key>") {
        let after_k = &rest[k_start + 5..];
        let k_end = after_k.find("</key>")?;
        let found_key = after_k[..k_end].trim();
        let after_tag = &after_k[k_end + 6..];
        if found_key == key {
            let s_start = after_tag.find("<string>")?;
            let after_s = &after_tag[s_start + 8..];
            let s_end = after_s.find("</string>")?;
            return Some(after_s[..s_end].trim());
        }
        rest = after_tag;
    }
    None
}

fn read_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn read_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

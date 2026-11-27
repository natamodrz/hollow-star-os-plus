//! Shared memory regions (`Shm`) for zero-copy IPC between EL0 processes.

use delta_abi::errno;
use hal_core::{PAGE_SIZE, PhysAddr, VirtAddr, mem::align_up};

use crate::arch::mmu::{self, UserPageAttrs};
use crate::mem::pmm;
use crate::proc;
use crate::sync::IrqSpinLock;

const MAX_SHM_REGIONS: usize = 24;
const MAX_SHM_PAGES: usize = 320; // up to 1.25 MiB per shared buffer (covers 640x480x4 FB)
const MAX_SHM_PEERS: usize = 8;

#[derive(Clone, Copy)]
struct ShmMapping {
    pid: u64,
    ttbr0: PhysAddr,
    va: usize,
}

#[derive(Clone, Copy)]
struct ShmRegion {
    in_use: bool,
    id: u64,
    owner_pid: u64,
    size_bytes: usize,
    num_pages: usize,
    frames: [PhysAddr; MAX_SHM_PAGES],
    allowed_pids: [u64; MAX_SHM_PEERS],
    mappings: [Option<ShmMapping>; MAX_SHM_PEERS],
}

impl ShmRegion {
    const fn empty() -> Self {
        Self {
            in_use: false,
            id: 0,
            owner_pid: 0,
            size_bytes: 0,
            num_pages: 0,
            frames: [PhysAddr(0); MAX_SHM_PAGES],
            allowed_pids: [0; MAX_SHM_PEERS],
            mappings: [None; MAX_SHM_PEERS],
        }
    }

    fn is_authorized(&self, pid: u64) -> bool {
        self.owner_pid == pid || self.allowed_pids.contains(&pid)
    }

    fn has_mappings(&self) -> bool {
        self.mappings.iter().any(Option::is_some)
    }
}

struct ShmTable {
    regions: [ShmRegion; MAX_SHM_REGIONS],
    next_id: u64,
}

impl ShmTable {
    const fn new() -> Self {
        Self {
            regions: [const { ShmRegion::empty() }; MAX_SHM_REGIONS],
            next_id: 1,
        }
    }
}

static SHM: IrqSpinLock<ShmTable> = IrqSpinLock::new(ShmTable::new());

/// Allocates a shared memory region of `size_bytes` and maps it into `owner_pid`'s `TTBR0`.
pub fn create(owner_pid: u64, ttbr0: PhysAddr, size_bytes: usize) -> Result<(u64, usize), isize> {
    if size_bytes == 0 || size_bytes > MAX_SHM_PAGES * PAGE_SIZE {
        return Err(errno::EINVAL);
    }
    let num_pages = align_up(size_bytes, PAGE_SIZE) / PAGE_SIZE;
    let base_va = proc::alloc_shm_va(owner_pid, num_pages).ok_or(errno::ENOMEM)?;

    let mut table = SHM.lock();
    let Some(slot) = table.regions.iter().position(|r| !r.in_use) else {
        return Err(errno::ENOMEM);
    };

    let id = table.next_id;
    table.next_id += 1;
    let region = &mut table.regions[slot];
    *region = ShmRegion::empty();
    region.in_use = true;
    region.id = id;
    region.owner_pid = owner_pid;
    region.size_bytes = size_bytes;
    region.num_pages = num_pages;

    for i in 0..num_pages {
        let Some(frame) = pmm::alloc_zeroed_frame() else {
            for (j, old_frame) in region.frames.iter().enumerate().take(i) {
                let _ = mmu::unmap_user_page(ttbr0, VirtAddr(base_va + j * PAGE_SIZE));
                let _ = pmm::free_frame(*old_frame);
            }
            region.in_use = false;
            return Err(errno::ENOMEM);
        };
        region.frames[i] = frame;
        if mmu::map_user_page(
            ttbr0,
            VirtAddr(base_va + i * PAGE_SIZE),
            frame,
            UserPageAttrs::RwData,
        )
        .is_err()
        {
            for j in 0..i {
                let _ = mmu::unmap_user_page(ttbr0, VirtAddr(base_va + j * PAGE_SIZE));
            }
            for old_frame in region.frames.iter().take(i + 1) {
                let _ = pmm::free_frame(*old_frame);
            }
            region.in_use = false;
            return Err(errno::ENOMEM);
        }
    }

    region.mappings[0] = Some(ShmMapping {
        pid: owner_pid,
        ttbr0,
        va: base_va,
    });

    Ok((id, base_va))
}

/// Authorizes `target_pid` to map shared region `shm_id`.
pub fn allow_pid(owner_pid: u64, shm_id: u64, target_pid: u64) -> Result<(), isize> {
    let mut table = SHM.lock();
    let region = table
        .regions
        .iter_mut()
        .find(|r| r.in_use && r.id == shm_id)
        .ok_or(errno::ENOENT)?;
    if region.owner_pid != owner_pid {
        return Err(errno::EPERM);
    }
    if region.allowed_pids.contains(&target_pid) {
        return Ok(());
    }
    let slot = region
        .allowed_pids
        .iter_mut()
        .find(|p| **p == 0)
        .ok_or(errno::ENOMEM)?;
    *slot = target_pid;
    Ok(())
}

/// Maps an authorized `shm_id` into `caller_pid`'s `TTBR0` and returns `(va, size_bytes)`.
pub fn map(caller_pid: u64, ttbr0: PhysAddr, shm_id: u64) -> Result<(usize, usize), isize> {
    let num_pages = {
        let table = SHM.lock();
        let region = table
            .regions
            .iter()
            .find(|r| r.in_use && r.id == shm_id)
            .ok_or(errno::ENOENT)?;
        if !region.is_authorized(caller_pid) {
            return Err(errno::EPERM);
        }
        if let Some(existing) = region
            .mappings
            .iter()
            .flatten()
            .find(|m| m.pid == caller_pid)
        {
            return Ok((existing.va, region.size_bytes));
        }
        region.num_pages
    };

    let base_va = proc::alloc_shm_va(caller_pid, num_pages).ok_or(errno::ENOMEM)?;
    let mut table = SHM.lock();
    let region = table
        .regions
        .iter_mut()
        .find(|r| r.in_use && r.id == shm_id)
        .ok_or(errno::ENOENT)?;
    let size_bytes = region.size_bytes;

    for (i, frame) in region.frames.iter().enumerate().take(num_pages) {
        if mmu::map_user_page(
            ttbr0,
            VirtAddr(base_va + i * PAGE_SIZE),
            *frame,
            UserPageAttrs::RwData,
        )
        .is_err()
        {
            for j in 0..i {
                let _ = mmu::unmap_user_page(ttbr0, VirtAddr(base_va + j * PAGE_SIZE));
            }
            return Err(errno::ENOMEM);
        }
    }

    let map_slot = region
        .mappings
        .iter_mut()
        .find(|m| m.is_none())
        .ok_or(errno::ENOMEM)?;
    *map_slot = Some(ShmMapping {
        pid: caller_pid,
        ttbr0,
        va: base_va,
    });

    Ok((base_va, size_bytes))
}

/// Unmaps `shm_id` from `caller_pid` and frees backing physical frames when unreferenced.
pub fn unmap(caller_pid: u64, shm_id: u64) -> Result<(), isize> {
    let mut table = SHM.lock();
    let region = table
        .regions
        .iter_mut()
        .find(|r| r.in_use && r.id == shm_id)
        .ok_or(errno::ENOENT)?;

    let mut removed: Option<ShmMapping> = None;
    for slot in &mut region.mappings {
        if slot.is_some_and(|m| m.pid == caller_pid) {
            removed = slot.take();
            break;
        }
    }
    let mapping = removed.ok_or(errno::ENOENT)?;
    let num_pages = region.num_pages;

    for i in 0..num_pages {
        let _ = mmu::unmap_user_page(mapping.ttbr0, VirtAddr(mapping.va + i * PAGE_SIZE));
    }

    if !region.has_mappings() {
        for frame in region.frames.iter_mut().take(num_pages) {
            let _ = pmm::free_frame(*frame);
            *frame = PhysAddr(0);
        }
        region.in_use = false;
    }

    Ok(())
}

//! EL0 process management, ELF spawning, device capabilities, and user copies.

pub mod elf;

use core::ptr::copy_nonoverlapping;

use delta_abi::{
    AudioMmioInfo, DmaPageInfo, FbMmioInfo, SdMmioInfo, UartKind, UartMmioInfo, UsbMmioInfo, errno,
    vm_prot,
};
use hal_core::{MemoryRegion, PAGE_SIZE, PhysAddr, VirtAddr, mem::align_down, mem::align_up};

use crate::arch;
use crate::arch::mmu::{self, UserPageAttrs};
use crate::board;
use crate::mem::{phys_to_mut_ptr, pmm};
use crate::sched::{self, Priority, ThreadId};
use crate::sync::IrqSpinLock;

pub const MAX_PROCS: usize = 24;

/// Fixed virtual addresses where isolated EL0 driver servers receive Device MMIO pages.
const USER_UART_MMIO_VA: usize = 0x0000_1000_0000;
const USER_SD_MMIO_VA: usize = 0x0000_1001_0000;
const USER_FB_MBOX_VA: usize = 0x0000_1002_0000;
const USER_AUDIO_MMIO_VA: usize = 0x0000_1003_0000;
const USER_USB_MMIO_VA: usize = 0x0000_1004_0000;
const USER_DMA_BASE_VA: usize = 0x0000_1800_0000;
/// Starting virtual base for dynamic per-process shared memory (`Shm`) mappings.
const USER_SHM_BASE_VA: usize = 0x0000_2000_0000;
/// Starting virtual base for Mach-like dynamic allocations (`VmAllocate` / `.dylib` `dyld`).
const USER_VM_BASE_VA: usize = 0x0000_2800_0000;
const USER_VM_LIMIT_VA: usize = 0x0000_2FFF_0000;
const MAX_VM_ALLOC_PAGES: usize = 128;
/// Fixed virtual base where `drv-fb` maps the physical VideoCore framebuffer aperture.
const USER_FB_PHYS_VA: usize = 0x0000_3000_0000;
const MAX_FB_MAP_BYTES: usize = 8 * 1024 * 1024;
const MAX_USER_ELF_PAGES: usize = 128; // 512 KiB cap for user-spawned ELF images

/// Device MMIO / IRQ grants supplied exclusively to EL0 driver servers.
#[derive(Clone, Copy)]
pub struct DeviceCaps {
    pub uart: Option<(MemoryRegion, UartKind)>,
    pub sd: Option<(MemoryRegion, u32)>,
    pub display: Option<(MemoryRegion, usize)>,
    pub audio: Option<MemoryRegion>,
    pub usb: Option<(MemoryRegion, u32)>,
}

impl DeviceCaps {
    pub const fn none() -> Self {
        Self {
            uart: None,
            sd: None,
            display: None,
            audio: None,
            usb: None,
        }
    }
}

#[derive(Clone, Copy)]
struct ProcessControlBlock {
    pid: u64,
    tid: ThreadId,
    entry_pc: usize,
    user_sp: usize,
    next_shm_va: usize,
    next_dma_va: usize,
    next_vm_va: usize,
    uart_mmio: Option<UartMmioInfo>,
    sd_mmio: Option<SdMmioInfo>,
    fb_mmio: Option<FbMmioInfo>,
    audio_mmio: Option<AudioMmioInfo>,
    usb_mmio: Option<UsbMmioInfo>,
    waiter_tid: Option<ThreadId>,
    exited: bool,
    exit_code: usize,
    active: bool,
}

impl ProcessControlBlock {
    const fn empty() -> Self {
        Self {
            pid: 0,
            tid: ThreadId(0),
            entry_pc: 0,
            user_sp: 0,
            next_shm_va: USER_SHM_BASE_VA,
            next_dma_va: USER_DMA_BASE_VA,
            next_vm_va: USER_VM_BASE_VA,
            uart_mmio: None,
            sd_mmio: None,
            fb_mmio: None,
            audio_mmio: None,
            usb_mmio: None,
            waiter_tid: None,
            exited: false,
            exit_code: 0,
            active: false,
        }
    }
}

struct ProcessTable {
    procs: [ProcessControlBlock; MAX_PROCS],
    next_pid: u64,
}

impl ProcessTable {
    const fn new() -> Self {
        Self {
            procs: [const { ProcessControlBlock::empty() }; MAX_PROCS],
            next_pid: 1,
        }
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        self.procs.iter().position(|p| !p.active)
    }

    fn find_by_pid(&self, pid: u64) -> Option<&ProcessControlBlock> {
        self.procs.iter().find(|p| p.active && p.pid == pid)
    }

    fn find_by_pid_mut(&mut self, pid: u64) -> Option<&mut ProcessControlBlock> {
        self.procs.iter_mut().find(|p| p.active && p.pid == pid)
    }
}

static PROCS: IrqSpinLock<ProcessTable> = IrqSpinLock::new(ProcessTable::new());

/// Helper mapping one or more 4 KiB device MMIO pages for an EL0 driver process.
fn map_device_mmio(
    ttbr0_root: PhysAddr,
    user_va_page: usize,
    region: MemoryRegion,
) -> Result<usize, elf::ElfError> {
    let aligned_pa = align_down(region.base, PAGE_SIZE);
    let offset = region.base - aligned_pa;
    let total_bytes = align_up(offset + region.size.max(PAGE_SIZE), PAGE_SIZE);
    let pages = total_bytes / PAGE_SIZE;
    for i in 0..pages {
        mmu::map_user_page(
            ttbr0_root,
            VirtAddr(user_va_page + i * PAGE_SIZE),
            PhysAddr(aligned_pa + i * PAGE_SIZE),
            UserPageAttrs::DeviceMmio,
        )?;
    }
    Ok(user_va_page + offset)
}

/// Spawns an EL0 user process from an ELF64 binary slice with optional device grants.
pub fn spawn_elf(
    name: &'static str,
    elf_bytes: &[u8],
    priority: Priority,
    devices: DeviceCaps,
) -> Result<u64, elf::ElfError> {
    let loaded = elf::load_elf64(elf_bytes)?;

    let uart_mmio = if let Some((region, kind)) = devices.uart {
        Some(UartMmioInfo {
            virt_base: map_device_mmio(loaded.ttbr0_root, USER_UART_MMIO_VA, region)?,
            kind: kind as u32,
            _pad: 0,
        })
    } else {
        None
    };

    let sd_mmio = if let Some((region, irq_num)) = devices.sd {
        Some(SdMmioInfo {
            virt_base: map_device_mmio(loaded.ttbr0_root, USER_SD_MMIO_VA, region)?,
            irq_num,
            _pad: 0,
        })
    } else {
        None
    };

    let fb_mmio = if let Some((region, vc_bus_offset)) = devices.display {
        Some(FbMmioInfo {
            mbox_virt_base: map_device_mmio(loaded.ttbr0_root, USER_FB_MBOX_VA, region)?,
            vc_bus_offset,
        })
    } else {
        None
    };

    let audio_mmio = if let Some(region) = devices.audio {
        Some(AudioMmioInfo {
            virt_base: map_device_mmio(loaded.ttbr0_root, USER_AUDIO_MMIO_VA, region)?,
        })
    } else {
        None
    };

    let usb_mmio = if let Some((region, irq_num)) = devices.usb {
        Some(UsbMmioInfo {
            virt_base: map_device_mmio(loaded.ttbr0_root, USER_USB_MMIO_VA, region)?,
            irq_num,
            _pad: 0,
        })
    } else {
        None
    };

    let pid = {
        let mut table = PROCS.lock();
        let slot = table.alloc_slot().ok_or(elf::ElfError::OutOfMemory)?;
        let pid = table.next_pid;
        table.next_pid += 1;

        table.procs[slot] = ProcessControlBlock {
            pid,
            tid: ThreadId(0),
            entry_pc: loaded.entry_pc,
            user_sp: loaded.user_sp,
            next_shm_va: USER_SHM_BASE_VA,
            next_dma_va: USER_DMA_BASE_VA,
            next_vm_va: USER_VM_BASE_VA,
            uart_mmio,
            sd_mmio,
            fb_mmio,
            audio_mmio,
            usb_mmio,
            waiter_tid: None,
            exited: false,
            exit_code: 0,
            active: true,
        };
        pid
    };

    let tid = sched::spawn_user(
        name,
        pid,
        loaded.ttbr0_root,
        priority,
        user_thread_trampoline,
        pid as usize,
    )
    .map_err(|_| elf::ElfError::OutOfMemory)?;

    {
        let mut table = PROCS.lock();
        if let Some(p) = table.find_by_pid_mut(pid) {
            p.tid = tid;
        }
    }

    Ok(pid)
}

/// Spawns a child EL0 process from an ELF64 buffer residing in the caller's `TTBR0` space.
pub fn spawn_elf_from_user(
    caller_ttbr0: PhysAddr,
    user_elf_ptr: usize,
    elf_len: usize,
) -> Result<u64, isize> {
    if elf_len == 0 || elf_len > MAX_USER_ELF_PAGES * PAGE_SIZE {
        return Err(errno::EINVAL);
    }
    let pages = align_up(elf_len, PAGE_SIZE) / PAGE_SIZE;
    let staging_pa = pmm::alloc_contiguous_frames(pages).ok_or(errno::ENOMEM)?;
    let staging_ptr = phys_to_mut_ptr::<u8>(staging_pa);

    // SAFETY: `staging_pa` is an exclusive contiguous `pages * PAGE_SIZE` kernel buffer
    // accessible via higher-half `TTBR1_EL1`.
    let staging_slice = unsafe { core::slice::from_raw_parts_mut(staging_ptr, elf_len) };
    let copy_res = copy_from_user(caller_ttbr0, user_elf_ptr, staging_slice);

    let spawn_res = if copy_res.is_ok() {
        spawn_elf(
            "user-elf",
            staging_slice,
            Priority::NORMAL,
            DeviceCaps::none(),
        )
        .map_err(|_| errno::EINVAL)
    } else {
        Err(errno::EFAULT)
    };

    for i in 0..pages {
        let _ = pmm::free_frame(PhysAddr(staging_pa.as_usize() + i * PAGE_SIZE));
    }

    spawn_res
}

fn user_thread_trampoline(pid: usize) {
    let (entry_pc, user_sp) = {
        let table = PROCS.lock();
        let proc_entry = table
            .find_by_pid(pid as u64)
            .expect("spawned process PCB must exist");
        (proc_entry.entry_pc, proc_entry.user_sp)
    };

    // SAFETY: Scheduler loaded this process's `TTBR0_EL1` prior to `switch_context`,
    // and `elf::load_elf64` mapped `entry_pc` (`Code`) and `user_sp` stack (`RwData`).
    unsafe {
        arch::context::enter_usermode(entry_pc, user_sp);
    }
}

/// Marks `pid` as exited with `exit_code`, wakes any waiter, and terminates the thread.
pub fn exit_current(code: usize) -> ! {
    let (_, pid, _) = sched::current_thread_info();
    let waiter = {
        let mut table = PROCS.lock();
        if let Some(p) = table.find_by_pid_mut(pid) {
            p.exited = true;
            p.exit_code = code;
            p.waiter_tid.take()
        } else {
            None
        }
    };
    if let Some(w_tid) = waiter {
        sched::unblock(w_tid);
    }
    arch::send_event();
    sched::exit_current()
}

/// Blocks the calling thread until `target_pid` exits and returns its exit code.
pub fn wait_pid(waiter_tid: ThreadId, target_pid: u64) -> Result<usize, isize> {
    loop {
        let done = {
            let mut table = PROCS.lock();
            let p = table.find_by_pid_mut(target_pid).ok_or(errno::ENOENT)?;
            if p.exited {
                let code = p.exit_code;
                // Reclaim the PCB slot once reaped so launching apps repeatedly
                // in a desktop session never exhausts `MAX_PROCS`.
                p.active = false;
                Some(code)
            } else {
                p.waiter_tid = Some(waiter_tid);
                sched::prepare_wait(waiter_tid);
                None
            }
        };
        if let Some(code) = done {
            return Ok(code);
        }
        sched::block_current();
    }
}

/// Allocates a non-overlapping virtual range inside `pid`'s `TTBR0` for shared memory.
pub fn alloc_shm_va(pid: u64, num_pages: usize) -> Option<usize> {
    let mut table = PROCS.lock();
    let p = table.find_by_pid_mut(pid)?;
    let va = p.next_shm_va;
    // Leave one unmapped guard page after every shared memory mapping.
    p.next_shm_va = va.checked_add((num_pages + 1) * PAGE_SIZE)?;
    Some(va)
}

/// Returns the isolated UART MMIO info if `pid` holds UART device capability.
pub fn uart_mmio_info(pid: u64) -> Option<UartMmioInfo> {
    let table = PROCS.lock();
    table.find_by_pid(pid).and_then(|p| p.uart_mmio)
}

/// Returns the isolated SD/EMMC MMIO + IRQ info if `pid` holds SD device capability.
pub fn sd_mmio_info(pid: u64) -> Option<SdMmioInfo> {
    let table = PROCS.lock();
    table.find_by_pid(pid).and_then(|p| p.sd_mmio)
}

/// Returns the isolated Mailbox/display MMIO info if `pid` holds display capability.
pub fn fb_mmio_info(pid: u64) -> Option<FbMmioInfo> {
    let table = PROCS.lock();
    table.find_by_pid(pid).and_then(|p| p.fb_mmio)
}

/// Returns the isolated PWM/audio MMIO info if `pid` holds audio capability.
pub fn audio_mmio_info(pid: u64) -> Option<AudioMmioInfo> {
    let table = PROCS.lock();
    table.find_by_pid(pid).and_then(|p| p.audio_mmio)
}

/// Returns the isolated USB Host (`DWC2`) MMIO + IRQ info if `pid` holds USB capability.
pub fn usb_mmio_info(pid: u64) -> Option<UsbMmioInfo> {
    let table = PROCS.lock();
    table.find_by_pid(pid).and_then(|p| p.usb_mmio)
}

/// Mach-style `vm_allocate`: maps `num_pages` contiguous zeroed `RW` 4 KiB pages into `pid`'s `TTBR0`.
pub fn vm_allocate(pid: u64, ttbr0_root: PhysAddr, num_pages: usize) -> Result<usize, isize> {
    if num_pages == 0 || num_pages > MAX_VM_ALLOC_PAGES {
        return Err(errno::EINVAL);
    }
    let base_va = {
        let mut table = PROCS.lock();
        let p = table.find_by_pid_mut(pid).ok_or(errno::ENOENT)?;
        let va = p.next_vm_va;
        let next = va
            .checked_add((num_pages + 1) * PAGE_SIZE)
            .ok_or(errno::ENOMEM)?;
        if next > USER_VM_LIMIT_VA {
            return Err(errno::ENOMEM);
        }
        p.next_vm_va = next;
        va
    };

    for i in 0..num_pages {
        let Some(pa) = pmm::alloc_zeroed_frame() else {
            for rollback in 0..i {
                if let Some(old_pa) =
                    mmu::unmap_user_page(ttbr0_root, VirtAddr(base_va + rollback * PAGE_SIZE))
                {
                    let _ = pmm::free_frame(old_pa);
                }
            }
            return Err(errno::ENOMEM);
        };
        if mmu::map_user_page(
            ttbr0_root,
            VirtAddr(base_va + i * PAGE_SIZE),
            pa,
            UserPageAttrs::RwData,
        )
        .is_err()
        {
            let _ = pmm::free_frame(pa);
            for rollback in 0..i {
                if let Some(old_pa) =
                    mmu::unmap_user_page(ttbr0_root, VirtAddr(base_va + rollback * PAGE_SIZE))
                {
                    let _ = pmm::free_frame(old_pa);
                }
            }
            return Err(errno::ENOMEM);
        }
    }
    Ok(base_va)
}

/// Mach-style `vm_protect`: transitions `num_pages` inside `USER_VM_BASE_VA..` with strict W^X (`RX`, `RO`, `RW`).
pub fn vm_protect(
    pid: u64,
    ttbr0_root: PhysAddr,
    va: usize,
    num_pages: usize,
    prot: usize,
) -> Result<(), isize> {
    if num_pages == 0 || num_pages > MAX_VM_ALLOC_PAGES || !va.is_multiple_of(PAGE_SIZE) {
        return Err(errno::EINVAL);
    }
    if (prot & vm_prot::WRITE) != 0 && (prot & vm_prot::EXEC) != 0 {
        return Err(errno::EPERM);
    }
    let attrs = match prot {
        vm_prot::RO => UserPageAttrs::RoData,
        vm_prot::RW => UserPageAttrs::RwData,
        vm_prot::RX => UserPageAttrs::Code,
        _ => return Err(errno::EINVAL),
    };
    let end_va = va.checked_add(num_pages * PAGE_SIZE).ok_or(errno::EINVAL)?;
    {
        let table = PROCS.lock();
        let p = table.find_by_pid(pid).ok_or(errno::ENOENT)?;
        if va < USER_VM_BASE_VA || end_va > p.next_vm_va {
            return Err(errno::EPERM);
        }
    }
    for i in 0..num_pages {
        mmu::protect_user_page(ttbr0_root, VirtAddr(va + i * PAGE_SIZE), attrs)
            .map_err(|_| errno::EFAULT)?;
    }
    Ok(())
}

/// Allocates one non-cacheable DMA 4 KiB page for an authorized driver server.
pub fn alloc_dma_page(pid: u64, ttbr0_root: PhysAddr) -> Result<DmaPageInfo, isize> {
    let va = {
        let mut table = PROCS.lock();
        let p = table.find_by_pid_mut(pid).ok_or(errno::ENOENT)?;
        if p.fb_mmio.is_none()
            && p.audio_mmio.is_none()
            && p.sd_mmio.is_none()
            && p.usb_mmio.is_none()
        {
            return Err(errno::EPERM);
        }
        let va = p.next_dma_va;
        p.next_dma_va = va.checked_add(2 * PAGE_SIZE).ok_or(errno::ENOMEM)?;
        va
    };

    let pa = pmm::alloc_zeroed_frame().ok_or(errno::ENOMEM)?;
    if mmu::map_user_page(ttbr0_root, VirtAddr(va), pa, UserPageAttrs::NormalNc).is_err() {
        let _ = pmm::free_frame(pa);
        return Err(errno::ENOMEM);
    }
    Ok(DmaPageInfo {
        phys_addr: pa.as_usize(),
        user_va: va,
    })
}

/// Maps the VideoCore/display framebuffer aperture into `drv-fb` if authorized and bounded.
pub fn map_fb_phys(
    pid: u64,
    ttbr0_root: PhysAddr,
    phys_addr: usize,
    size_bytes: usize,
) -> Result<usize, isize> {
    if fb_mmio_info(pid).is_none() {
        return Err(errno::EPERM);
    }
    let Some((win_base, win_size)) = board::framebuffer_phys_window() else {
        return Err(errno::ENOENT);
    };
    if size_bytes == 0 || size_bytes > MAX_FB_MAP_BYTES {
        return Err(errno::EINVAL);
    }

    let aligned_pa = align_down(phys_addr, PAGE_SIZE);
    let offset = phys_addr - aligned_pa;
    let map_bytes = align_up(offset + size_bytes, PAGE_SIZE);
    let end_pa = aligned_pa.checked_add(map_bytes).ok_or(errno::EINVAL)?;
    let win_end = win_base.saturating_add(win_size);
    if aligned_pa < win_base || end_pa > win_end {
        return Err(errno::EPERM);
    }

    let pages = map_bytes / PAGE_SIZE;
    for i in 0..pages {
        mmu::map_user_page(
            ttbr0_root,
            VirtAddr(USER_FB_PHYS_VA + i * PAGE_SIZE),
            PhysAddr(aligned_pa + i * PAGE_SIZE),
            UserPageAttrs::NormalNc,
        )
        .map_err(|_| errno::ENOMEM)?;
    }

    Ok(USER_FB_PHYS_VA + offset)
}

/// Returns `true` if `pid` was granted capability to handle peripheral IRQ `irq_num`.
pub fn has_irq_cap(pid: u64, irq_num: u32) -> bool {
    let table = PROCS.lock();
    let Some(p) = table.find_by_pid(pid) else {
        return false;
    };
    p.sd_mmio.is_some_and(|sd| sd.irq_num == irq_num)
        || p.usb_mmio.is_some_and(|usb| usb.irq_num == irq_num)
}

/// Safely copies `dst.len()` bytes from an EL0 virtual address `user_ptr` in `ttbr0_root`.
pub fn copy_from_user(ttbr0_root: PhysAddr, user_ptr: usize, dst: &mut [u8]) -> Result<(), isize> {
    if user_ptr == 0 || user_ptr.checked_add(dst.len()).is_none() {
        return Err(errno::EFAULT);
    }

    let mut copied = 0usize;
    while copied < dst.len() {
        let va = user_ptr + copied;
        let (pa, _) = mmu::translate_user_page(ttbr0_root, VirtAddr(va)).ok_or(errno::EFAULT)?;
        let page_rem = PAGE_SIZE - (va & (PAGE_SIZE - 1));
        let chunk = (dst.len() - copied).min(page_rem);
        let src_ptr = phys_to_mut_ptr::<u8>(pa) as *const u8;
        // SAFETY: `translate_user_page` confirmed EL0 readability for `[va, va + chunk)`
        // and `phys_to_mut_ptr` converts `pa` into the kernel `TTBR1_EL1` physmap.
        unsafe {
            copy_nonoverlapping(src_ptr, dst[copied..].as_mut_ptr(), chunk);
        }
        copied += chunk;
    }

    Ok(())
}

/// Safely copies `src.len()` bytes into a writable EL0 virtual address `user_ptr`.
pub fn copy_to_user(ttbr0_root: PhysAddr, user_ptr: usize, src: &[u8]) -> Result<(), isize> {
    if user_ptr == 0 || user_ptr.checked_add(src.len()).is_none() {
        return Err(errno::EFAULT);
    }

    let mut copied = 0usize;
    while copied < src.len() {
        let va = user_ptr + copied;
        let (pa, writable) =
            mmu::translate_user_page(ttbr0_root, VirtAddr(va)).ok_or(errno::EFAULT)?;
        if !writable {
            return Err(errno::EFAULT);
        }
        let page_rem = PAGE_SIZE - (va & (PAGE_SIZE - 1));
        let chunk = (src.len() - copied).min(page_rem);
        let dst_ptr = phys_to_mut_ptr::<u8>(pa);
        // SAFETY: `translate_user_page` confirmed EL0 write permission for `[va, va + chunk)`
        // and `phys_to_mut_ptr` converts `pa` into the kernel `TTBR1_EL1` physmap.
        unsafe {
            copy_nonoverlapping(src[copied..].as_ptr(), dst_ptr, chunk);
        }
        copied += chunk;
    }

    Ok(())
}

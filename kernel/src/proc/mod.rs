//! EL0 process management, ELF spawning, and validated user memory copies.

pub mod elf;

use core::ptr::copy_nonoverlapping;

use delta_abi::{UartKind, UartMmioInfo, errno};
use hal_core::{MemoryRegion, PAGE_SIZE, PhysAddr, VirtAddr, mem::align_down};

use crate::arch;
use crate::arch::mmu::{self, UserPageAttrs};
use crate::mem::phys_to_mut_ptr;
use crate::sched::{self, Priority, ThreadId};
use crate::sync::IrqSpinLock;

pub const MAX_PROCS: usize = 16;

/// Fixed virtual address where `drv-uart` receives its isolated 4 KiB UART MMIO page.
const USER_UART_MMIO_VA: usize = 0x0000_1000_0000;

#[derive(Clone, Copy)]
struct ProcessControlBlock {
    pid: u64,
    tid: ThreadId,
    name: &'static str,
    entry_pc: usize,
    user_sp: usize,
    uart_mmio: Option<UartMmioInfo>,
    exited: bool,
    exit_code: usize,
    active: bool,
}

impl ProcessControlBlock {
    const fn empty() -> Self {
        Self {
            pid: 0,
            tid: ThreadId(0),
            name: "",
            entry_pc: 0,
            user_sp: 0,
            uart_mmio: None,
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

    fn find_by_pid_mut(&mut self, pid: u64) -> Option<&mut ProcessControlBlock> {
        self.procs.iter_mut().find(|p| p.active && p.pid == pid)
    }
}

static PROCS: IrqSpinLock<ProcessTable> = IrqSpinLock::new(ProcessTable::new());

/// Spawns an EL0 user process from an embedded or loaded ELF64 binary.
pub fn spawn_elf(
    name: &'static str,
    elf_bytes: &[u8],
    priority: Priority,
    uart_device: Option<(MemoryRegion, UartKind)>,
) -> Result<u64, elf::ElfError> {
    let loaded = elf::load_elf64(elf_bytes)?;

    let uart_mmio = if let Some((region, kind)) = uart_device {
        let pa = PhysAddr(align_down(region.base, PAGE_SIZE));
        let offset = region.base - pa.as_usize();
        mmu::map_user_page(
            loaded.ttbr0_root,
            VirtAddr(USER_UART_MMIO_VA),
            pa,
            UserPageAttrs::DeviceMmio,
        )?;
        Some(UartMmioInfo {
            virt_base: USER_UART_MMIO_VA + offset,
            kind: kind as u32,
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
            name,
            entry_pc: loaded.entry_pc,
            user_sp: loaded.user_sp,
            uart_mmio,
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

fn user_thread_trampoline(pid: usize) {
    let (entry_pc, user_sp) = {
        let table = PROCS.lock();
        let proc_entry = table
            .procs
            .iter()
            .find(|p| p.active && p.pid == pid as u64)
            .expect("spawned process PCB must exist");
        (proc_entry.entry_pc, proc_entry.user_sp)
    };

    // SAFETY: Scheduler loaded this process's `TTBR0_EL1` prior to `switch_context`,
    // and `elf::load_elf64` mapped `entry_pc` (`Code`) and `user_sp` stack (`RwData`).
    unsafe {
        arch::context::enter_usermode(entry_pc, user_sp);
    }
}

/// Marks `pid` as exited with `exit_code` and terminates the current user thread.
pub fn exit_current(code: usize) -> ! {
    let (_, pid, _) = sched::current_thread_info();
    {
        let mut table = PROCS.lock();
        if let Some(p) = table.find_by_pid_mut(pid) {
            p.exited = true;
            p.exit_code = code;
        }
    }
    arch::send_event();
    sched::exit_current()
}

/// Returns `(exited, exit_code, name)` for `pid` if present.
pub fn status(pid: u64) -> Option<(bool, usize, &'static str)> {
    let table = PROCS.lock();
    table
        .procs
        .iter()
        .find(|p| p.active && p.pid == pid)
        .map(|p| (p.exited, p.exit_code, p.name))
}

/// Returns the isolated UART MMIO info if `pid` holds UART device capability.
pub fn uart_mmio_info(pid: u64) -> Option<UartMmioInfo> {
    let table = PROCS.lock();
    table
        .procs
        .iter()
        .find(|p| p.active && p.pid == pid)
        .and_then(|p| p.uart_mmio)
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

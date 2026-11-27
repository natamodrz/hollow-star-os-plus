//! EL0 -> EL1 system call dispatcher (`SVC #0`).

use core::mem::size_of;

use delta_abi::{
    AudioMmioInfo, FbMmioInfo, IpcMessage, SdMmioInfo, SysCaps, SyscallId, UartMmioInfo,
    UsbMmioInfo, errno,
};
use hal_core::{InterruptController, PAGE_SIZE};

use crate::arch::exceptions::TrapFrame;
use crate::mem::pmm;
use crate::{arch, board, ipc, proc, sched, shm};

/// Dispatches an AArch64 `svc #0` call from EL0 using `x8` as syscall ID
/// and `x0..x5` as arguments, returning the result value for `x0`.
pub fn dispatch(tf: &TrapFrame) -> isize {
    let Some(id) = SyscallId::from_raw(tf.x[8] as usize) else {
        return errno::ENOSYS;
    };

    let (tid, pid, ttbr0) = sched::current_thread_info();
    let a0 = tf.x[0] as usize;
    let a1 = tf.x[1] as usize;
    let a2 = tf.x[2] as usize;
    let a3 = tf.x[3] as usize;

    match id {
        SyscallId::Exit => proc::exit_current(a0),
        SyscallId::Yield => {
            sched::yield_now();
            0
        }
        SyscallId::GetPid => pid as isize,
        SyscallId::SpawnElf => {
            if a1 == 0 || a1 > 64 {
                return errno::EINVAL;
            }
            let mut name_buf = [0u8; 64];
            if let Err(e) = proc::copy_from_user(ttbr0, a0, &mut name_buf[..a1]) {
                return e;
            }
            match proc::spawn_elf_from_user(ttbr0, a2, a3) {
                Ok(child_pid) => child_pid as isize,
                Err(e) => e,
            }
        }
        SyscallId::WaitPid => match proc::wait_pid(tid, a0 as u64) {
            Ok(code) => code as isize,
            Err(e) => e,
        },
        SyscallId::GetSysCaps => {
            let mut caps = SysCaps::empty();
            let pmm_stats = pmm::stats();
            caps.ram_total_bytes = (pmm_stats.total_ram_frames * PAGE_SIZE) as u64;
            caps.ram_free_bytes = (pmm_stats.free_frames * PAGE_SIZE) as u64;
            caps.uptime_ms = arch::exceptions::timer_ticks() * 10;
            if let Some(info) = board::info() {
                caps.cpu_count = info.cpu_count as u8;
                caps.has_wifi = u8::from(info.has_wifi);
                caps.has_bluetooth = u8::from(info.has_bluetooth);
                let name_bytes = info.name.as_bytes();
                let n = name_bytes.len().min(caps.board_name.len());
                caps.board_name[..n].copy_from_slice(&name_bytes[..n]);
                caps.board_name_len = n as u8;
            }
            match copy_struct_to_user::<SysCaps>(ttbr0, a0, &caps) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::PortCreate => match ipc::port_create(pid, tid, a0) {
            Ok(cap) => cap as isize,
            Err(e) => e,
        },
        SyscallId::PortSend => {
            let mut msg = IpcMessage::new(0);
            if let Err(e) = copy_struct_from_user(ttbr0, a1, &mut msg) {
                return e;
            }
            match ipc::port_send(pid, tid, a0, msg) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::PortRecv => match ipc::port_recv(pid, tid, a0) {
            Ok(msg) => match copy_struct_to_user(ttbr0, a1, &msg) {
                Ok(()) => 0,
                Err(e) => e,
            },
            Err(e) => e,
        },
        SyscallId::PortCall => {
            let mut req = IpcMessage::new(0);
            if let Err(e) = copy_struct_from_user(ttbr0, a1, &mut req) {
                return e;
            }
            match ipc::port_call(pid, tid, a0, req) {
                Ok(reply) => match copy_struct_to_user(ttbr0, a2, &reply) {
                    Ok(()) => 0,
                    Err(e) => e,
                },
                Err(e) => e,
            }
        }
        SyscallId::PortReply => {
            let mut reply = IpcMessage::new(0);
            if let Err(e) = copy_struct_from_user(ttbr0, a1, &mut reply) {
                return e;
            }
            match ipc::port_reply(pid, a0, reply) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::ServiceRegister => {
            if a1 == 0 || a1 > 32 {
                return errno::EINVAL;
            }
            let mut name_buf = [0u8; 32];
            if let Err(e) = proc::copy_from_user(ttbr0, a0, &mut name_buf[..a1]) {
                return e;
            }
            match ipc::service_register(pid, &name_buf[..a1], a2) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::ServiceLookup => {
            if a1 == 0 || a1 > 32 {
                return errno::EINVAL;
            }
            let mut name_buf = [0u8; 32];
            if let Err(e) = proc::copy_from_user(ttbr0, a0, &mut name_buf[..a1]) {
                return e;
            }
            match ipc::service_lookup(pid, &name_buf[..a1]) {
                Ok(cap) => cap as isize,
                Err(e) => e,
            }
        }
        SyscallId::GetUartMmioInfo => {
            let Some(info) = proc::uart_mmio_info(pid) else {
                return errno::EPERM;
            };
            match copy_struct_to_user::<UartMmioInfo>(ttbr0, a0, &info) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::GetSdMmioInfo => {
            let Some(info) = proc::sd_mmio_info(pid) else {
                return errno::EPERM;
            };
            match copy_struct_to_user::<SdMmioInfo>(ttbr0, a0, &info) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::IrqBind => match ipc::irq_bind(pid, a0, a1 as u32) {
            Ok(()) => 0,
            Err(e) => e,
        },
        SyscallId::IrqAck => {
            let irq_num = a0 as u32;
            if !proc::has_irq_cap(pid, irq_num) {
                return errno::EPERM;
            }
            board::irq_handle().enable_peripheral_irq(irq_num);
            0
        }
        SyscallId::GetFbMmioInfo => {
            let Some(info) = proc::fb_mmio_info(pid) else {
                return errno::EPERM;
            };
            match copy_struct_to_user::<FbMmioInfo>(ttbr0, a0, &info) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::MapFbPhys => match proc::map_fb_phys(pid, ttbr0, a0, a1) {
            Ok(va) => va as isize,
            Err(e) => e,
        },
        SyscallId::AllocDmaPage => match proc::alloc_dma_page(pid, ttbr0) {
            Ok(info) => match copy_struct_to_user(ttbr0, a0, &info) {
                Ok(()) => 0,
                Err(e) => e,
            },
            Err(e) => e,
        },
        SyscallId::GetAudioMmioInfo => {
            let Some(info) = proc::audio_mmio_info(pid) else {
                return errno::EPERM;
            };
            match copy_struct_to_user::<AudioMmioInfo>(ttbr0, a0, &info) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::GetUsbMmioInfo => {
            let Some(info) = proc::usb_mmio_info(pid) else {
                return errno::EPERM;
            };
            match copy_struct_to_user::<UsbMmioInfo>(ttbr0, a0, &info) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        SyscallId::ShmCreate => match shm::create(pid, ttbr0, a0) {
            Ok((shm_id, va)) => match copy_struct_to_user::<usize>(ttbr0, a1, &va) {
                Ok(()) => shm_id as isize,
                Err(e) => {
                    let _ = shm::unmap(pid, shm_id);
                    e
                }
            },
            Err(e) => e,
        },
        SyscallId::ShmAllowPort => match ipc::port_owner_pid(pid, a1) {
            Ok(target_pid) => match shm::allow_pid(pid, a0 as u64, target_pid) {
                Ok(()) => 0,
                Err(e) => e,
            },
            Err(e) => e,
        },
        SyscallId::ShmMap => match shm::map(pid, ttbr0, a0 as u64) {
            Ok((va, size)) => match copy_struct_to_user::<usize>(ttbr0, a1, &va) {
                Ok(()) => size as isize,
                Err(e) => {
                    let _ = shm::unmap(pid, a0 as u64);
                    e
                }
            },
            Err(e) => e,
        },
        SyscallId::ShmUnmap => match shm::unmap(pid, a0 as u64) {
            Ok(()) => 0,
            Err(e) => e,
        },
    }
}

fn copy_struct_from_user<T: Copy>(
    ttbr0: hal_core::PhysAddr,
    user_ptr: usize,
    dst: &mut T,
) -> Result<(), isize> {
    let dst_bytes =
        // SAFETY: `dst` is a valid mutable stack value of `size_of::<T>()` bytes.
        unsafe { core::slice::from_raw_parts_mut((dst as *mut T).cast::<u8>(), size_of::<T>()) };
    proc::copy_from_user(ttbr0, user_ptr, dst_bytes)
}

fn copy_struct_to_user<T: Copy>(
    ttbr0: hal_core::PhysAddr,
    user_ptr: usize,
    src: &T,
) -> Result<(), isize> {
    let src_bytes =
        // SAFETY: `src` is a valid stack value of `size_of::<T>()` bytes.
        unsafe { core::slice::from_raw_parts((src as *const T).cast::<u8>(), size_of::<T>()) };
    proc::copy_to_user(ttbr0, user_ptr, src_bytes)
}

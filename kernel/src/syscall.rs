//! EL0 -> EL1 system call dispatcher (`SVC #0`).

use core::mem::size_of;

use delta_abi::{IpcMessage, SyscallId, UartMmioInfo, errno};

use crate::arch::exceptions::TrapFrame;
use crate::{ipc, proc, sched};

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

    match id {
        SyscallId::Exit => proc::exit_current(a0),
        SyscallId::Yield => {
            sched::yield_now();
            0
        }
        SyscallId::GetPid => pid as isize,
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

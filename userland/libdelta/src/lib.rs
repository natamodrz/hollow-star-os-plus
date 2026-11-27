#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace runtime and IPC library (`EL0`) for Hollow Star OS+.

pub use delta_abi;

use core::arch::asm;
use core::panic::PanicInfo;

use delta_abi::{IpcMessage, SyscallId, UartMmioInfo};

/// Raw AArch64 `svc #0` with 3 arguments.
#[inline(always)]
fn syscall3(id: SyscallId, a0: usize, a1: usize, a2: usize) -> isize {
    let ret: isize;
    #[cfg(target_arch = "aarch64")]
    // SAFETY: `svc #0` invokes the Delta Kernel EL0->EL1 syscall dispatcher per ABI
    // (`x8` = syscall number, `x0..x2` = arguments, `x0` = return value).
    unsafe {
        asm!(
            "svc #0",
            in("x8") id as usize,
            inout("x0") a0 => ret,
            in("x1") a1,
            in("x2") a2,
            options(nostack)
        );
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (id, a0, a1, a2);
        ret = -38;
    }
    ret
}

fn to_result(raw: isize) -> Result<usize, isize> {
    if raw < 0 { Err(raw) } else { Ok(raw as usize) }
}

/// Terminates the current EL0 process with `exit_code`.
pub fn exit(exit_code: usize) -> ! {
    let _ = syscall3(SyscallId::Exit, exit_code, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// Yields the CPU to the Delta Scheduler.
pub fn yield_now() {
    let _ = syscall3(SyscallId::Yield, 0, 0, 0);
}

/// Returns the calling thread/process ID.
pub fn getpid() -> usize {
    syscall3(SyscallId::GetPid, 0, 0, 0).max(0) as usize
}

/// Creates an IPC port with `capacity` queued messages and returns its capability handle.
pub fn port_create(capacity: usize) -> Result<usize, isize> {
    to_result(syscall3(SyscallId::PortCreate, capacity, 0, 0))
}

/// Sends an [`IpcMessage`] to the port referenced by `cap`.
pub fn port_send(cap: usize, msg: &IpcMessage) -> Result<(), isize> {
    to_result(syscall3(
        SyscallId::PortSend,
        cap,
        core::ptr::from_ref(msg) as usize,
        0,
    ))
    .map(|_| ())
}

/// Receives an [`IpcMessage`] from `cap` into `msg`.
pub fn port_recv(cap: usize, msg: &mut IpcMessage) -> Result<(), isize> {
    to_result(syscall3(
        SyscallId::PortRecv,
        cap,
        core::ptr::from_mut(msg) as usize,
        0,
    ))
    .map(|_| ())
}

/// Performs a synchronous IPC call: sends `req` and waits for `reply`.
pub fn port_call(cap: usize, req: &IpcMessage, reply: &mut IpcMessage) -> Result<(), isize> {
    to_result(syscall3(
        SyscallId::PortCall,
        cap,
        core::ptr::from_ref(req) as usize,
        core::ptr::from_mut(reply) as usize,
    ))
    .map(|_| ())
}

/// Sends a reply message to a `reply_port` obtained from [`port_recv`].
pub fn port_reply(reply_port: usize, reply: &IpcMessage) -> Result<(), isize> {
    to_result(syscall3(
        SyscallId::PortReply,
        reply_port,
        core::ptr::from_ref(reply) as usize,
        0,
    ))
    .map(|_| ())
}

/// Registers a named service backed by port capability `cap`.
pub fn service_register(name: &str, cap: usize) -> Result<(), isize> {
    to_result(syscall3(
        SyscallId::ServiceRegister,
        name.as_ptr() as usize,
        name.len(),
        cap,
    ))
    .map(|_| ())
}

/// Looks up a named service and returns a `SEND`-capable handle to it.
pub fn service_lookup(name: &str) -> Result<usize, isize> {
    to_result(syscall3(
        SyscallId::ServiceLookup,
        name.as_ptr() as usize,
        name.len(),
        0,
    ))
}

/// Retrieves the mapped UART MMIO info if this process holds UART device access.
pub fn uart_mmio_info() -> Result<UartMmioInfo, isize> {
    let mut info = UartMmioInfo {
        virt_base: 0,
        kind: 0,
        _pad: 0,
    };
    to_result(syscall3(
        SyscallId::GetUartMmioInfo,
        core::ptr::from_mut(&mut info) as usize,
        0,
        0,
    ))?;
    Ok(info)
}

unsafe extern "Rust" {
    fn delta_main() -> usize;
}

/// Standard EL0 CRT0 entry point invoking `delta_main()` and calling [`exit`].
#[unsafe(link_section = ".text.boot")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // SAFETY: `delta_main` is provided by each EL0 binary linked with `libdelta`.
    let code = unsafe { delta_main() };
    exit(code)
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1)
}

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace runtime, IPC, shared memory, and VFS/process helpers (`EL0`) for Hollow Star OS+.

pub use delta_abi;

use core::arch::asm;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

use delta_abi::{
    AudioMmioInfo, DmaPageInfo, FbMmioInfo, IpcMessage, MSG_UART_WRITE, MSG_VFS_EXPAND_USER,
    MSG_VFS_MKDIR, MSG_VFS_READ_FILE, MSG_VFS_READDIR, MSG_VFS_REMOVE, MSG_VFS_STAT,
    MSG_VFS_STATFS, MSG_VFS_WRITE_FILE, SVC_UART, SVC_VFS, SdMmioInfo, SysCaps, SyscallId,
    UartMmioInfo, UsbMmioInfo, VfsPacket, VfsPartitionStat, pack_msg, unpack_msg,
};

/// Raw AArch64 `svc #0` with up to 4 arguments.
#[inline(always)]
fn syscall4(id: SyscallId, a0: usize, a1: usize, a2: usize, a3: usize) -> isize {
    let ret: isize;
    #[cfg(target_arch = "aarch64")]
    // SAFETY: `svc #0` invokes the Delta Kernel EL0->EL1 syscall dispatcher per ABI
    // (`x8` = syscall number, `x0..x3` = arguments, `x0` = return value).
    unsafe {
        asm!(
            "svc #0",
            in("x8") id as usize,
            inout("x0") a0 => ret,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            options(nostack)
        );
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (id, a0, a1, a2, a3);
        ret = -38;
    }
    ret
}

fn to_result(raw: isize) -> Result<usize, isize> {
    if raw < 0 { Err(raw) } else { Ok(raw as usize) }
}

/// Terminates the current EL0 process with `exit_code`.
pub fn exit(exit_code: usize) -> ! {
    let _ = syscall4(SyscallId::Exit, exit_code, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// Yields the CPU to the Delta Scheduler.
pub fn yield_now() {
    let _ = syscall4(SyscallId::Yield, 0, 0, 0, 0);
}

/// Returns the calling process ID.
pub fn getpid() -> usize {
    syscall4(SyscallId::GetPid, 0, 0, 0, 0).max(0) as usize
}

/// Spawns a new EL0 process from an in-memory ELF64 binary slice.
pub fn spawn_elf(name: &str, elf_bytes: &[u8]) -> Result<usize, isize> {
    to_result(syscall4(
        SyscallId::SpawnElf,
        name.as_ptr() as usize,
        name.len(),
        elf_bytes.as_ptr() as usize,
        elf_bytes.len(),
    ))
}

/// Waits for child/target process `pid` to exit and returns its exit code.
pub fn wait_pid(pid: usize) -> Result<usize, isize> {
    to_result(syscall4(SyscallId::WaitPid, pid, 0, 0, 0))
}

/// Retrieves live RAM, uptime, board name, and wireless capabilities (`GetSysCaps`).
pub fn sys_caps() -> Result<SysCaps, isize> {
    let mut caps = SysCaps::empty();
    to_result(syscall4(
        SyscallId::GetSysCaps,
        core::ptr::from_mut(&mut caps) as usize,
        0,
        0,
        0,
    ))?;
    Ok(caps)
}

/// Creates an IPC port with `capacity` queued messages and returns its capability handle.
pub fn port_create(capacity: usize) -> Result<usize, isize> {
    to_result(syscall4(SyscallId::PortCreate, capacity, 0, 0, 0))
}

/// Sends an [`IpcMessage`] to the port referenced by `cap`.
pub fn port_send(cap: usize, msg: &IpcMessage) -> Result<(), isize> {
    to_result(syscall4(
        SyscallId::PortSend,
        cap,
        core::ptr::from_ref(msg) as usize,
        0,
        0,
    ))
    .map(|_| ())
}

/// Receives an [`IpcMessage`] from `cap` into `msg`.
pub fn port_recv(cap: usize, msg: &mut IpcMessage) -> Result<(), isize> {
    to_result(syscall4(
        SyscallId::PortRecv,
        cap,
        core::ptr::from_mut(msg) as usize,
        0,
        0,
    ))
    .map(|_| ())
}

/// Performs a synchronous IPC call: sends `req` and waits for `reply`.
pub fn port_call(cap: usize, req: &IpcMessage, reply: &mut IpcMessage) -> Result<(), isize> {
    to_result(syscall4(
        SyscallId::PortCall,
        cap,
        core::ptr::from_ref(req) as usize,
        core::ptr::from_mut(reply) as usize,
        0,
    ))
    .map(|_| ())
}

/// Sends a reply message to a `reply_port` obtained from [`port_recv`].
pub fn port_reply(reply_port: usize, reply: &IpcMessage) -> Result<(), isize> {
    to_result(syscall4(
        SyscallId::PortReply,
        reply_port,
        core::ptr::from_ref(reply) as usize,
        0,
        0,
    ))
    .map(|_| ())
}

/// Registers a named service backed by port capability `cap`.
pub fn service_register(name: &str, cap: usize) -> Result<(), isize> {
    to_result(syscall4(
        SyscallId::ServiceRegister,
        name.as_ptr() as usize,
        name.len(),
        cap,
        0,
    ))
    .map(|_| ())
}

/// Looks up a named service and returns a `SEND`-capable handle to it.
pub fn service_lookup(name: &str) -> Result<usize, isize> {
    to_result(syscall4(
        SyscallId::ServiceLookup,
        name.as_ptr() as usize,
        name.len(),
        0,
        0,
    ))
}

/// Waits until `name` is registered and returns its `SEND` capability handle.
pub fn wait_for_service(name: &str) -> usize {
    loop {
        if let Ok(cap) = service_lookup(name) {
            return cap;
        }
        yield_now();
    }
}

/// Retrieves the mapped UART MMIO info if this process holds UART device access.
pub fn uart_mmio_info() -> Result<UartMmioInfo, isize> {
    let mut info = UartMmioInfo {
        virt_base: 0,
        kind: 0,
        _pad: 0,
    };
    to_result(syscall4(
        SyscallId::GetUartMmioInfo,
        core::ptr::from_mut(&mut info) as usize,
        0,
        0,
        0,
    ))?;
    Ok(info)
}

/// Retrieves the mapped SD/EMMC MMIO + IRQ info if this process holds SD device access.
pub fn sd_mmio_info() -> Result<SdMmioInfo, isize> {
    let mut info = SdMmioInfo {
        virt_base: 0,
        irq_num: 0,
        _pad: 0,
    };
    to_result(syscall4(
        SyscallId::GetSdMmioInfo,
        core::ptr::from_mut(&mut info) as usize,
        0,
        0,
        0,
    ))?;
    Ok(info)
}

/// Binds a hardware peripheral IRQ to an IPC port capability held by the driver.
pub fn irq_bind(port_cap: usize, irq_num: u32) -> Result<(), isize> {
    to_result(syscall4(
        SyscallId::IrqBind,
        port_cap,
        irq_num as usize,
        0,
        0,
    ))
    .map(|_| ())
}

/// Re-enables a hardware peripheral IRQ line after handling the interrupt in EL0.
pub fn irq_ack(irq_num: u32) -> Result<(), isize> {
    to_result(syscall4(SyscallId::IrqAck, irq_num as usize, 0, 0, 0)).map(|_| ())
}

/// Retrieves isolated VideoCore Mailbox MMIO info (`drv-fb` capability).
pub fn fb_mmio_info() -> Result<FbMmioInfo, isize> {
    let mut info = FbMmioInfo {
        mbox_virt_base: 0,
        vc_bus_offset: 0,
    };
    to_result(syscall4(
        SyscallId::GetFbMmioInfo,
        core::ptr::from_mut(&mut info) as usize,
        0,
        0,
        0,
    ))?;
    Ok(info)
}

/// Maps physical VideoCore framebuffer aperture `[phys_addr, phys_addr + size_bytes)` in `drv-fb`.
pub fn map_fb_phys(phys_addr: usize, size_bytes: usize) -> Result<usize, isize> {
    to_result(syscall4(SyscallId::MapFbPhys, phys_addr, size_bytes, 0, 0))
}

/// Allocates a 4 KiB Non-Cacheable DMA page for EL0 hardware drivers.
pub fn alloc_dma_page() -> Result<DmaPageInfo, isize> {
    let mut info = DmaPageInfo {
        phys_addr: 0,
        user_va: 0,
    };
    to_result(syscall4(
        SyscallId::AllocDmaPage,
        core::ptr::from_mut(&mut info) as usize,
        0,
        0,
        0,
    ))?;
    Ok(info)
}

/// Retrieves isolated PWM/audio MMIO info (`drv-audio` capability).
pub fn audio_mmio_info() -> Result<AudioMmioInfo, isize> {
    let mut info = AudioMmioInfo { virt_base: 0 };
    to_result(syscall4(
        SyscallId::GetAudioMmioInfo,
        core::ptr::from_mut(&mut info) as usize,
        0,
        0,
        0,
    ))?;
    Ok(info)
}

/// Retrieves isolated USB Host (`DWC2`) MMIO + IRQ info (`drv-kbd` capability).
pub fn usb_mmio_info() -> Result<UsbMmioInfo, isize> {
    let mut info = UsbMmioInfo {
        virt_base: 0,
        irq_num: 0,
        _pad: 0,
    };
    to_result(syscall4(
        SyscallId::GetUsbMmioInfo,
        core::ptr::from_mut(&mut info) as usize,
        0,
        0,
        0,
    ))?;
    Ok(info)
}

/// Creates a shared memory buffer of `size_bytes` and returns `(shm_id, mapped_va)`.
pub fn shm_create(size_bytes: usize) -> Result<(u64, usize), isize> {
    let mut va = 0usize;
    let id = to_result(syscall4(
        SyscallId::ShmCreate,
        size_bytes,
        core::ptr::from_mut(&mut va) as usize,
        0,
        0,
    ))?;
    Ok((id as u64, va))
}

/// Authorizes the owner process of `port_cap` to map `shm_id`.
pub fn shm_allow_port(shm_id: u64, port_cap: usize) -> Result<(), isize> {
    to_result(syscall4(
        SyscallId::ShmAllowPort,
        shm_id as usize,
        port_cap,
        0,
        0,
    ))
    .map(|_| ())
}

/// Maps an authorized shared memory buffer `shm_id` into this process's `TTBR0`.
pub fn shm_map(shm_id: u64) -> Result<(usize, usize), isize> {
    let mut va = 0usize;
    let size = to_result(syscall4(
        SyscallId::ShmMap,
        shm_id as usize,
        core::ptr::from_mut(&mut va) as usize,
        0,
        0,
    ))?;
    Ok((va, size))
}

/// Unmaps and releases a shared memory mapping from this process.
pub fn shm_unmap(shm_id: u64) -> Result<(), isize> {
    to_result(syscall4(SyscallId::ShmUnmap, shm_id as usize, 0, 0, 0)).map(|_| ())
}

/// Convenience helper: prints text to the userspace UART server (`svc.uart`).
pub fn print_via(uart_cap: usize, text: &str) {
    for chunk in text.as_bytes().chunks(delta_abi::IPC_MAX_DATA) {
        let req = IpcMessage::from_bytes(MSG_UART_WRITE, chunk);
        let mut reply = IpcMessage::new(0);
        let _ = port_call(uart_cap, &req, &mut reply);
    }
}

/// Convenience helper: prints text by looking up `svc.uart` (if needed).
pub fn print(text: &str) {
    let cap = wait_for_service(SVC_UART);
    print_via(cap, text);
}

/// Reads up to `out.len()` bytes from a VFS file using shared memory (`svc.vfs`).
pub fn vfs_read_file(vfs_cap: usize, path: &str, out: &mut [u8]) -> Result<usize, isize> {
    let (shm_id, va) = shm_create(out.len().max(4096))?;
    if let Err(e) = shm_allow_port(shm_id, vfs_cap) {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }

    let mut pkt = VfsPacket::for_path(path.as_bytes());
    pkt.shm_id = shm_id;
    pkt.size = out.len() as u32;
    let req = pack_msg(MSG_VFS_READ_FILE, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    let call_res = port_call(vfs_cap, &req, &mut reply_msg);
    if let Err(e) = call_res {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }

    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        let _ = shm_unmap(shm_id);
        return Err(reply_pkt.status as isize);
    }

    let bytes_read = (reply_pkt.size as usize).min(out.len());
    // SAFETY: `va` is a valid shared memory mapping of at least `out.len()` bytes in our `TTBR0`.
    let shm_slice = unsafe { core::slice::from_raw_parts(va as *const u8, bytes_read) };
    out[..bytes_read].copy_from_slice(shm_slice);
    let _ = shm_unmap(shm_id);
    Ok(bytes_read)
}

/// Writes `data` to a VFS file (creating or overwriting) via `svc.vfs`.
pub fn vfs_write_file(vfs_cap: usize, path: &str, data: &[u8]) -> Result<usize, isize> {
    let (shm_id, va) = shm_create(data.len().max(4096))?;
    // SAFETY: `va` is a valid writable shared memory mapping in our `TTBR0`.
    unsafe {
        let dst = core::slice::from_raw_parts_mut(va as *mut u8, data.len());
        dst.copy_from_slice(data);
    }
    if let Err(e) = shm_allow_port(shm_id, vfs_cap) {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }

    let mut pkt = VfsPacket::for_path(path.as_bytes());
    pkt.shm_id = shm_id;
    pkt.size = data.len() as u32;
    let req = pack_msg(MSG_VFS_WRITE_FILE, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    let res = port_call(vfs_cap, &req, &mut reply_msg);
    let _ = shm_unmap(shm_id);
    res?;

    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        return Err(reply_pkt.status as isize);
    }
    Ok(reply_pkt.size as usize)
}

/// Reads directory listing text into `out` from `svc.vfs`.
pub fn vfs_readdir(vfs_cap: usize, path: &str, out: &mut [u8]) -> Result<usize, isize> {
    let (shm_id, va) = shm_create(out.len().max(4096))?;
    if let Err(e) = shm_allow_port(shm_id, vfs_cap) {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }

    let mut pkt = VfsPacket::for_path(path.as_bytes());
    pkt.shm_id = shm_id;
    pkt.size = out.len() as u32;
    let req = pack_msg(MSG_VFS_READDIR, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    let call_res = port_call(vfs_cap, &req, &mut reply_msg);
    if let Err(e) = call_res {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }

    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        let _ = shm_unmap(shm_id);
        return Err(reply_pkt.status as isize);
    }
    let len = (reply_pkt.size as usize).min(out.len());
    // SAFETY: `va` is mapped with at least `len` bytes in our `TTBR0`.
    let shm_slice = unsafe { core::slice::from_raw_parts(va as *const u8, len) };
    out[..len].copy_from_slice(shm_slice);
    let _ = shm_unmap(shm_id);
    Ok(len)
}

/// Checks whether a file/directory exists via `svc.vfs` and returns `(size, is_dir)`.
pub fn vfs_stat(vfs_cap: usize, path: &str) -> Result<(usize, bool), isize> {
    let pkt = VfsPacket::for_path(path.as_bytes());
    let req = pack_msg(MSG_VFS_STAT, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    port_call(vfs_cap, &req, &mut reply_msg)?;
    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        return Err(reply_pkt.status as isize);
    }
    Ok((reply_pkt.size as usize, reply_pkt.is_dir != 0))
}

/// Creates a directory at `path` via `svc.vfs`.
pub fn vfs_mkdir(vfs_cap: usize, path: &str) -> Result<(), isize> {
    let pkt = VfsPacket::for_path(path.as_bytes());
    let req = pack_msg(MSG_VFS_MKDIR, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    port_call(vfs_cap, &req, &mut reply_msg)?;
    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        Err(reply_pkt.status as isize)
    } else {
        Ok(())
    }
}

/// Removes a file at `path` via `svc.vfs`.
pub fn vfs_remove(vfs_cap: usize, path: &str) -> Result<(), isize> {
    let pkt = VfsPacket::for_path(path.as_bytes());
    let req = pack_msg(MSG_VFS_REMOVE, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    port_call(vfs_cap, &req, &mut reply_msg)?;
    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        Err(reply_pkt.status as isize)
    } else {
        Ok(())
    }
}

/// Queries mounted partition capacities and filesystem types (`VfsPartitionStat` slice).
pub fn vfs_statfs(vfs_cap: usize, out: &mut [VfsPartitionStat]) -> Result<usize, isize> {
    if out.is_empty() {
        return Ok(0);
    }
    let bytes_needed = core::mem::size_of_val(out).max(4096);
    let (shm_id, va) = shm_create(bytes_needed)?;
    if let Err(e) = shm_allow_port(shm_id, vfs_cap) {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }
    let mut pkt = VfsPacket::for_path(b"/");
    pkt.shm_id = shm_id;
    pkt.size = out.len() as u32;
    let req = pack_msg(MSG_VFS_STATFS, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    let res = port_call(vfs_cap, &req, &mut reply_msg);
    if let Err(e) = res {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }
    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        let _ = shm_unmap(shm_id);
        return Err(reply_pkt.status as isize);
    }
    let count = (reply_pkt.size as usize).min(out.len());
    // SAFETY: `va` holds `count` `VfsPartitionStat` records written by `svc.vfs`.
    let src = unsafe { core::slice::from_raw_parts(va as *const VfsPartitionStat, count) };
    out[..count].copy_from_slice(src);
    let _ = shm_unmap(shm_id);
    Ok(count)
}

/// Expands the `/user` DeltaFS partition bitmap to `target_sectors` sectors.
pub fn vfs_expand_user(vfs_cap: usize, target_sectors: u64) -> Result<u64, isize> {
    let mut pkt = VfsPacket::for_path(b"/user");
    pkt.offset = target_sectors;
    let req = pack_msg(MSG_VFS_EXPAND_USER, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    port_call(vfs_cap, &req, &mut reply_msg)?;
    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        Err(reply_pkt.status as isize)
    } else {
        Ok(reply_pkt.offset)
    }
}

/// Launches an ELF executable or `<App>.hsapp/exec` bundle from VFS (`svc.vfs`).
pub fn spawn_from_vfs(name: &str, path: &str) -> Result<usize, isize> {
    let vfs_cap = wait_for_service(SVC_VFS);
    let (size, is_dir) = vfs_stat(vfs_cap, path)?;
    if is_dir || size == 0 || size > 512 * 1024 {
        return Err(delta_abi::errno::EINVAL);
    }
    let (shm_id, va) = shm_create(size)?;
    if let Err(e) = shm_allow_port(shm_id, vfs_cap) {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }
    let mut pkt = VfsPacket::for_path(path.as_bytes());
    pkt.shm_id = shm_id;
    pkt.size = size as u32;
    let req = pack_msg(MSG_VFS_READ_FILE, &pkt);
    let mut reply_msg = IpcMessage::new(0);
    let call_res = port_call(vfs_cap, &req, &mut reply_msg);
    if let Err(e) = call_res {
        let _ = shm_unmap(shm_id);
        return Err(e);
    }
    let reply_pkt: VfsPacket = unpack_msg(&reply_msg).ok_or(-5isize)?;
    if reply_pkt.status < 0 {
        let _ = shm_unmap(shm_id);
        return Err(reply_pkt.status as isize);
    }
    let read_len = (reply_pkt.size as usize).min(size);
    // SAFETY: `va` holds `read_len` ELF bytes populated by `svc.vfs`.
    let elf_slice = unsafe { core::slice::from_raw_parts(va as *const u8, read_len) };
    let pid_res = spawn_elf(name, elf_slice);
    let _ = shm_unmap(shm_id);
    pid_res
}

#[cfg(target_os = "none")]
unsafe extern "Rust" {
    fn delta_main() -> usize;
}

/// Standard EL0 CRT0 entry point invoking `delta_main()` and calling [`exit`].
#[cfg(target_os = "none")]
#[unsafe(link_section = ".text.boot")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // SAFETY: `delta_main` is provided by each EL0 binary linked with `libdelta`.
    let code = unsafe { delta_main() };
    exit(code)
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1)
}

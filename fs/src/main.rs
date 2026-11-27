#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Userspace (`EL0`) VFS server (`vfsd`, `svc.vfs`) connecting to `drv-sd` (`svc.sd`).

use libdelta::delta_abi::{
    IpcMessage, MSG_SD_READ, MSG_SD_WRITE, MSG_VFS_ACK, MSG_VFS_READ_FILE, MSG_VFS_READDIR,
    MSG_VFS_STAT, MSG_VFS_WRITE_FILE, SVC_SD, SVC_VFS, SdIoPacket, VfsPacket, errno, pack_msg,
    unpack_msg,
};
use vfs_core::VfsRouter;
use vfs_core::block::{BlockDevice, FsError, SECTOR_SIZE};

const IO_SHM_BYTES: usize = 64 * 1024;

struct IpcBlockClient {
    sd_cap: usize,
    shm_id: u64,
    shm_va: usize,
}

impl IpcBlockClient {
    fn new(sd_cap: usize) -> Result<Self, isize> {
        let (shm_id, shm_va) = libdelta::shm_create(IO_SHM_BYTES)?;
        if let Err(e) = libdelta::shm_allow_port(shm_id, sd_cap) {
            let _ = libdelta::shm_unmap(shm_id);
            return Err(e);
        }
        Ok(Self {
            sd_cap,
            shm_id,
            shm_va,
        })
    }
}

impl BlockDevice for IpcBlockClient {
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let sectors = buf.len().div_ceil(SECTOR_SIZE);
        let max_sectors = IO_SHM_BYTES / SECTOR_SIZE;
        let mut done_sectors = 0usize;

        while done_sectors < sectors {
            let chunk_sectors = (sectors - done_sectors).min(max_sectors);
            let pkt = SdIoPacket {
                lba: lba + done_sectors as u64,
                shm_id: self.shm_id,
                count: chunk_sectors as u32,
                status: 0,
            };
            let req = pack_msg(MSG_SD_READ, &pkt);
            let mut reply = IpcMessage::new(0);
            libdelta::port_call(self.sd_cap, &req, &mut reply).map_err(|_| FsError::Io)?;
            let r_pkt: SdIoPacket = unpack_msg(&reply).ok_or(FsError::Io)?;
            if r_pkt.status < 0 {
                return Err(FsError::Io);
            }

            let byte_off = done_sectors * SECTOR_SIZE;
            let byte_len = (chunk_sectors * SECTOR_SIZE).min(buf.len() - byte_off);
            // SAFETY: `self.shm_va` maps `IO_SHM_BYTES` in our `TTBR0`.
            let shm_slice =
                unsafe { core::slice::from_raw_parts(self.shm_va as *const u8, byte_len) };
            buf[byte_off..byte_off + byte_len].copy_from_slice(shm_slice);
            done_sectors += chunk_sectors;
        }
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), FsError> {
        let sectors = buf.len().div_ceil(SECTOR_SIZE);
        let max_sectors = IO_SHM_BYTES / SECTOR_SIZE;
        let mut done_sectors = 0usize;

        while done_sectors < sectors {
            let chunk_sectors = (sectors - done_sectors).min(max_sectors);
            let byte_off = done_sectors * SECTOR_SIZE;
            let byte_len = (chunk_sectors * SECTOR_SIZE).min(buf.len() - byte_off);
            // SAFETY: `self.shm_va` maps writable `IO_SHM_BYTES` in our `TTBR0`.
            unsafe {
                let shm_dst = core::slice::from_raw_parts_mut(
                    self.shm_va as *mut u8,
                    chunk_sectors * SECTOR_SIZE,
                );
                shm_dst.fill(0);
                shm_dst[..byte_len].copy_from_slice(&buf[byte_off..byte_off + byte_len]);
            }

            let pkt = SdIoPacket {
                lba: lba + done_sectors as u64,
                shm_id: self.shm_id,
                count: chunk_sectors as u32,
                status: 0,
            };
            let req = pack_msg(MSG_SD_WRITE, &pkt);
            let mut reply = IpcMessage::new(0);
            libdelta::port_call(self.sd_cap, &req, &mut reply).map_err(|_| FsError::Io)?;
            let r_pkt: SdIoPacket = unpack_msg(&reply).ok_or(FsError::Io)?;
            if r_pkt.status < 0 {
                return Err(FsError::Io);
            }
            done_sectors += chunk_sectors;
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let sd_cap = libdelta::wait_for_service(SVC_SD);
    let Ok(mut blk) = IpcBlockClient::new(sd_cap) else {
        libdelta::print("[el0/vfsd] Failed to allocate Shm buffer\n");
        return 1;
    };

    let Ok(vfs) = VfsRouter::mount(&mut blk) else {
        libdelta::print("[el0/vfsd] Failed to mount MBR/FAT32 SD card\n");
        return 2;
    };

    let Ok(port) = libdelta::port_create(16) else {
        return 3;
    };
    if libdelta::service_register(SVC_VFS, port).is_err() {
        return 4;
    }

    if vfs.has_ext2() {
        libdelta::print(
            "[el0/vfsd] VFS online (EL0, svc.vfs: FAT32 / + ext2 /ext2 + /dev + /proc)\n",
        );
    } else {
        libdelta::print("[el0/vfsd] VFS online (EL0, svc.vfs: FAT32 / + /dev + /proc)\n");
    }

    let mut msg = IpcMessage::new(0);
    loop {
        if libdelta::port_recv(port, &mut msg).is_err() {
            libdelta::yield_now();
            continue;
        }

        let Some(mut pkt) = unpack_msg::<VfsPacket>(&msg) else {
            continue;
        };

        match msg.tag {
            MSG_VFS_STAT => {
                let path = core::str::from_utf8(pkt.path_bytes()).unwrap_or("");
                match vfs.stat(&mut blk, path) {
                    Ok((size, is_dir)) => {
                        pkt.size = size;
                        pkt.is_dir = u8::from(is_dir);
                        pkt.status = 0;
                    }
                    Err(e) => pkt.status = e.to_errno(),
                }
            }
            MSG_VFS_READDIR => handle_shm_read(&vfs, &mut blk, &mut pkt, true),
            MSG_VFS_READ_FILE => handle_shm_read(&vfs, &mut blk, &mut pkt, false),
            MSG_VFS_WRITE_FILE => handle_shm_write(&vfs, &mut blk, &mut pkt),
            _ => pkt.status = errno::ENOSYS as i32,
        }

        if msg.reply_port != 0 {
            let reply = pack_msg(MSG_VFS_ACK, &pkt);
            let _ = libdelta::port_reply(msg.reply_port as usize, &reply);
        }
    }
}

fn handle_shm_read(vfs: &VfsRouter, blk: &mut IpcBlockClient, pkt: &mut VfsPacket, is_dir: bool) {
    let Ok((va, mapped_size)) = libdelta::shm_map(pkt.shm_id) else {
        pkt.status = errno::EPERM as i32;
        return;
    };
    let max_len = (pkt.size as usize).min(mapped_size);
    // SAFETY: `va` is mapped with `mapped_size >= max_len` in our `TTBR0`.
    let out_buf = unsafe { core::slice::from_raw_parts_mut(va as *mut u8, max_len) };

    let res = {
        let path = core::str::from_utf8(pkt.path_bytes()).unwrap_or("");
        if is_dir {
            vfs.read_dir(blk, path, out_buf)
        } else {
            vfs.read_file(blk, path, out_buf)
        }
    };
    match res {
        Ok(n) => {
            pkt.size = n as u32;
            pkt.status = 0;
        }
        Err(e) => pkt.status = e.to_errno(),
    }
    let _ = libdelta::shm_unmap(pkt.shm_id);
}

fn handle_shm_write(vfs: &VfsRouter, blk: &mut IpcBlockClient, pkt: &mut VfsPacket) {
    let Ok((va, mapped_size)) = libdelta::shm_map(pkt.shm_id) else {
        pkt.status = errno::EPERM as i32;
        return;
    };
    let len = (pkt.size as usize).min(mapped_size);
    // SAFETY: `va` maps `mapped_size >= len` bytes in our `TTBR0`.
    let in_buf = unsafe { core::slice::from_raw_parts(va as *const u8, len) };

    let res = {
        let path = core::str::from_utf8(pkt.path_bytes()).unwrap_or("");
        vfs.write_file(blk, path, in_buf)
    };
    match res {
        Ok(n) => {
            pkt.size = n as u32;
            pkt.status = 0;
        }
        Err(e) => pkt.status = e.to_errno(),
    }
    let _ = libdelta::shm_unmap(pkt.shm_id);
}

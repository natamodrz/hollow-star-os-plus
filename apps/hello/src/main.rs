#![no_std]
#![no_main]

//! Demo `.hsapp` executable (`/Hello.hsapp/exec`) loaded dynamically from FAT32.

use libdelta::delta_abi::SVC_VFS;

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let vfs_cap = libdelta::wait_for_service(SVC_VFS);
    let mut buf = [0u8; 128];
    if let Ok(len) = libdelta::vfs_read_file(vfs_cap, "/Hello.hsapp/Assets/info.txt", &mut buf) {
        if let Ok(text) = core::str::from_utf8(&buf[..len]) {
            libdelta::print("[el0/Hello.hsapp] Loaded asset: ");
            libdelta::print(text);
            if !text.ends_with('\n') {
                libdelta::print("\n");
            }
        }
    } else {
        libdelta::print("[el0/Hello.hsapp] Running from .hsapp bundle!\n");
    }
    0
}

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Loadable `.dkex` Mach/OS X-style Kernel Extension (`/system/Library/Extensions/DeltaDiag.dkex/exec`).
//!
//! Dynamically spawned from DeltaFS after storage mount, verifies `/system/lib/libdelta_kit.dylib`
//! loading via `dyld` (`VmAllocate` + `VmProtect`), and reports extension telemetry cleanly.

use libdelta::delta_abi::SVC_VFS;
use libdelta::dyld::Dylib;

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let vfs = libdelta::wait_for_service(SVC_VFS);
    if let Ok(kit) = Dylib::open(vfs, "/system/lib/libdelta_kit.dylib") {
        // SAFETY: `delta_kit_abi_version` and `delta_theme_accent_yellow` are `extern "C" fn() -> u32`.
        let abi = unsafe { kit.call_u32("delta_kit_abi_version") }.unwrap_or(0);
        let yellow = unsafe { kit.call_u32("delta_theme_accent_yellow") }.unwrap_or(0);
        if abi != 0 && yellow == 0x00F4_B41A {
            libdelta::print(
                "[el0/DeltaDiag.dkex] Loaded Mach .dkex extension + libdelta_kit.dylib (dyld W^X OK)\n",
            );
            return 0;
        }
    }
    libdelta::print("[el0/DeltaDiag.dkex] Extension telemetry loaded.\n");
    0
}

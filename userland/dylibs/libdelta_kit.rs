#![no_std]
#![no_main]

//! Shared Mach/OS X-style dynamic library (`ET_DYN` `/system/lib/libdelta_kit.dylib`).
//!
//! Exports shared theme tokens, ABI compatibility identifiers, and fixed-point UI curves
//! loaded dynamically at runtime via `libdelta::dyld::Dylib`.

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Returns the canonical `Hollow Star OS+` shared runtime ABI version (`0.6.2`).
#[unsafe(no_mangle)]
pub extern "C" fn delta_kit_abi_version() -> u32 {
    0x0006_0200
}

/// Returns the lightened deep-slate desktop/cyberdeck background color (`#131926`).
#[unsafe(no_mangle)]
pub extern "C" fn delta_theme_bg() -> u32 {
    0x0013_1926
}

/// Returns the elevated slate surface color (`#1B2336`).
#[unsafe(no_mangle)]
pub extern "C" fn delta_theme_panel() -> u32 {
    0x001B_2336
}

/// Returns the Golden Yellow primary accent color (`#F4B41A`).
#[unsafe(no_mangle)]
pub extern "C" fn delta_theme_accent_yellow() -> u32 {
    0x00F4_B41A
}

/// Returns the warm Amber highlight color (`#FFD166`).
#[unsafe(no_mangle)]
pub extern "C" fn delta_theme_accent_amber() -> u32 {
    0x00FF_D166
}

/// Fixed-point cubic ease-out (`0..=255 -> 0..=255`).
#[unsafe(no_mangle)]
pub extern "C" fn delta_ease_out_cubic(t: u32) -> u32 {
    let t = t.min(255);
    let inv = 255 - t;
    255 - ((inv * inv * inv) / (255 * 255))
}

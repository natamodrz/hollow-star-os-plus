#![no_std]

//! Shared UI library for modular Hollow Star OS+ `.hsapp` applications and the Cyberdeck/PDA Compositor.

pub extern crate libdelta;
pub use libdelta::delta_abi;

pub mod calibri;
pub mod client;
pub mod fx;
pub mod gfx;
pub mod i18n;
pub mod icons;

pub use client::DesktopClient;
pub use fx::{ArcSharpStyle, CornerStyle, Rect, ease_in_out_quad, ease_out_cubic, lerp_color};
pub use gfx::{APP_H, APP_W, BAR_H, Canvas, FB_H, FB_W};
pub use i18n::{BufWriter, Lang, OsConfig, tr, wireless_enabled};
pub use icons::{IconKind, draw_icon_2x};

/// Sleeps for at least `ms` real milliseconds using 1 ms kernel timer ticks while yielding CPU.
pub fn sleep_ms(ms: u64) {
    if ms == 0 {
        libdelta::yield_now();
        return;
    }
    let start = libdelta::uptime_ms();
    let deadline = start.saturating_add(ms);
    let mut guard = ms.saturating_mul(60_000).max(60_000);
    while libdelta::uptime_ms() < deadline && guard > 0 {
        libdelta::yield_now();
        guard -= 1;
    }
}

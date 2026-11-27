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

/// Cooperatively sleeps for roughly `ms` milliseconds while yielding CPU time to other EL0 tasks.
pub fn sleep_ms(ms: u64) {
    let start = libdelta::sys_caps().map(|c| c.uptime_ms).unwrap_or(0);
    let mut yields = 0u32;
    while yields < 64 {
        libdelta::yield_now();
        let now = libdelta::sys_caps().map(|c| c.uptime_ms).unwrap_or(0);
        if now.saturating_sub(start) >= ms {
            break;
        }
        yields += 1;
    }
}

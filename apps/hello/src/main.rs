#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Demo `.hsapp` executable (`/system/apps/Hello.hsapp/exec`) supporting both
//! headless CLI execution and interactive Desktop `.hsapp` presentation.

use libdelta_ui::delta_abi::{KeyCode, SVC_DESKTOP, SVC_VFS};
use libdelta_ui::gfx::{
    ACCENT, ACCENT_AMBER, ACCENT_YELLOW, APP_H, APP_W, BG, BORDER, CYAN, MUTED, OK_GREEN, PANEL,
    TEXT,
};
use libdelta_ui::{ArcSharpStyle, BufWriter, DesktopClient, IconKind, draw_icon_2x};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let vfs_cap = libdelta::wait_for_service(SVC_VFS);
    let mut buf = [0u8; 128];
    let asset_str = if let Ok(len) = libdelta::vfs_read_file(
        vfs_cap,
        "/system/apps/Hello.hsapp/Assets/info.txt",
        &mut buf,
    ) {
        if let Ok(text) = core::str::from_utf8(&buf[..len]) {
            libdelta::print("[el0/Hello.hsapp] Loaded asset: ");
            libdelta::print(text);
            if !text.ends_with('\n') {
                libdelta::print("\n");
            }
            text.trim()
        } else {
            ".hsapp asset binary"
        }
    } else {
        libdelta::print("[el0/Hello.hsapp] Running from .hsapp bundle!\n");
        "Hello.hsapp running from DeltaFS"
    };

    if libdelta::service_lookup(SVC_DESKTOP).is_ok()
        && let Ok(mut desktop) = DesktopClient::connect()
    {
        let mut dylib_buf = [0u8; 48];
        let mut dw = BufWriter::new(&mut dylib_buf);
        if let Ok(kit) = libdelta::dyld::Dylib::open(vfs_cap, "/system/lib/libdelta_kit.dylib") {
            // SAFETY: `delta_kit_abi_version` is an exported `extern "C" fn() -> u32`.
            let abi = unsafe { kit.call_u32("delta_kit_abi_version") }.unwrap_or(0);
            dw.str("libdelta_kit.dylib (ABI 0x");
            let hex = b"0123456789ABCDEF";
            for sh in (0..8).rev() {
                let b = [hex[((abi >> (sh * 4)) & 0xF) as usize]];
                if let Ok(s) = core::str::from_utf8(&b) {
                    dw.str(s);
                }
            }
            dw.str(")");
        } else {
            dw.str("static fallback");
        }

        let c = &mut desktop.canvas;
        c.clear(BG);
        c.draw_arc_box(
            56,
            54,
            (APP_W - 112) as i32,
            (APP_H - 108) as i32,
            ArcSharpStyle::modal(),
            PANEL,
            ACCENT_YELLOW,
            ACCENT_AMBER,
        );
        draw_icon_2x(c, 84, 82, IconKind::Hsapp, ACCENT_YELLOW);
        c.draw_text_scaled(132, 86, "Hello.hsapp Bundle Inspector", ACCENT_YELLOW, 1);
        c.draw_text(
            132,
            110,
            "Mach / OS X-style bundle: /system/apps/Hello.hsapp/",
            MUTED,
        );

        c.draw_box(84, 148, (APP_W - 168) as i32, 136, BG, BORDER);
        c.draw_text(102, 162, "Bundle Layout & Dynamic Linker:", TEXT);
        c.draw_text(
            102,
            186,
            "  exec             -> AArch64 EL0 ELF executable",
            CYAN,
        );
        c.draw_text(
            102,
            208,
            "  icon.png         -> Bundle application emblem",
            CYAN,
        );
        c.draw_text(102, 230, asset_str, OK_GREEN);
        c.draw_text(102, 254, dw.as_str(), ACCENT_AMBER);

        c.draw_box(180, 314, 280, 42, BG, ACCENT);
        c.draw_text_centered(
            (APP_W / 2) as i32,
            328,
            "[Enter / Esc] Return to Launcher",
            ACCENT,
            1,
        );
        desktop.present();

        loop {
            if let Some((key, _)) = desktop.poll_key()
                && matches!(key, KeyCode::Enter | KeyCode::Esc)
            {
                desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                return 0;
            }
            libdelta_ui::sleep_ms(16);
        }
    }

    0
}

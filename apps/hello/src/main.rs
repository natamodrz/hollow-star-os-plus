#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Demo `.hsapp` executable (`/system/apps/Hello.hsapp/exec`) supporting both
//! headless CLI execution and interactive Desktop `.hsapp` presentation.

use libdelta_ui::delta_abi::{KeyCode, SVC_DESKTOP, SVC_VFS};
use libdelta_ui::gfx::{
    ACCENT, ACCENT_BLUE, ACCENT_MAGENTA, APP_H, APP_W, BG, BORDER, CYAN, MUTED, OK_GREEN, PANEL,
    TEXT,
};
use libdelta_ui::{ArcSharpStyle, DesktopClient, IconKind, draw_icon_2x};

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
        let c = &mut desktop.canvas;
        c.clear(BG);
        c.draw_arc_box(
            56,
            54,
            (APP_W - 112) as i32,
            (APP_H - 108) as i32,
            ArcSharpStyle::modal(),
            PANEL,
            ACCENT_BLUE,
            ACCENT_MAGENTA,
        );
        draw_icon_2x(c, 84, 82, IconKind::Hsapp, ACCENT_BLUE);
        c.draw_text_scaled(132, 86, "Hello.hsapp Bundle Inspector", ACCENT_BLUE, 1);
        c.draw_text(
            132,
            110,
            "macOS-style bundle: /system/apps/Hello.hsapp/",
            MUTED,
        );

        c.draw_box(84, 148, (APP_W - 168) as i32, 130, BG, BORDER);
        c.draw_text(102, 166, "Bundle Layout:", TEXT);
        c.draw_text(
            102,
            190,
            "  exec             -> AArch64 EL0 ELF executable",
            CYAN,
        );
        c.draw_text(
            102,
            212,
            "  icon.png         -> Bundle application emblem",
            CYAN,
        );
        c.draw_text(102, 234, "  Assets/info.txt  -> Live loaded string:", CYAN);
        c.draw_text(114, 256, asset_str, OK_GREEN);

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

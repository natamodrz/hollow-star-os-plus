#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Hollow Star OS+ Modular Desktop Compositor (`/system/bin/compositor.elf`).
//!
//! Responsibilities (`svc.desktop`):
//! - Owns `svc.fb` hardware framebuffer (`640x480` XRGB8888) and maps each focused `.hsapp`'s
//!   `640x452` surface `Shm` under the persistent top Status Bar.
//! - Draws the top Status Bar (5-point Hollow Star, `Calibri` font, live board info,
//!   audio volume/route, SoC temperature, clock, and `[F1]` quick help toggle).
//! - Reads hardware keyboard input via `svc.kbd` (USB HID Boot Keyboard).
//! - Orchestrates session transitions (`Setup.hsapp` -> `Launcher.hsapp` <-> apps).

use libdelta_ui::delta_abi::{
    AudioPacket, DesktopPacket, FbPacket, IpcMessage, KbdPacket, KeyCode, MSG_AUDIO_PLAY_TONE,
    MSG_DESKTOP_ACK, MSG_DESKTOP_CONNECT, MSG_DESKTOP_EXIT_TO_SHELL, MSG_DESKTOP_LAUNCH,
    MSG_DESKTOP_POLL_KEY, MSG_DESKTOP_PRESENT, MSG_DESKTOP_REFRESH_CFG, MSG_FB_GET_INFO,
    MSG_FB_PRESENT, MSG_KBD_ACK, MSG_KBD_READ, SVC_AUDIO, SVC_DESKTOP, SVC_FB, SVC_KBD, SVC_UART,
    SVC_VFS, SysCaps, errno, pack_msg, unpack_msg,
};
use libdelta_ui::gfx::{
    ACCENT, ACCENT_AMBER, ACCENT_YELLOW, APP_H, APP_W, BAR_H, BG, CYAN, Canvas, FB_H, FB_W, MUTED,
    OK_GREEN, PANEL, PANEL_ALT, TEXT,
};
use libdelta_ui::{
    ArcSharpStyle, BufWriter, Lang, OsConfig, Rect, ease_out_cubic, sleep_ms, tr, wireless_enabled,
};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart = libdelta::wait_for_service(SVC_UART);
    let vfs = libdelta::wait_for_service(SVC_VFS);
    let fb_port = libdelta::wait_for_service(SVC_FB);
    let audio = libdelta::wait_for_service(SVC_AUDIO);
    let _ = libdelta::wait_for_service(SVC_KBD);

    let fb_bytes = FB_W * FB_H * 4;
    let Ok((fb_shm_id, fb_va)) = libdelta::shm_create(fb_bytes) else {
        libdelta::print_via(uart, "[el0/compositor] Failed to allocate FB Shm\n");
        return 1;
    };
    if libdelta::shm_allow_port(fb_shm_id, fb_port).is_err() {
        libdelta::print_via(uart, "[el0/compositor] Failed to authorize drv-fb Shm\n");
        let _ = libdelta::shm_unmap(fb_shm_id);
        return 2;
    }

    let desktop_port = libdelta::port_create(16).expect("create svc.desktop port");
    if libdelta::service_register(SVC_DESKTOP, desktop_port).is_err() {
        libdelta::print_via(uart, "[el0/compositor] Failed to register svc.desktop\n");
        let _ = libdelta::shm_unmap(fb_shm_id);
        return 3;
    }

    // SAFETY: `fb_va` maps `FB_W * FB_H * 4` writable bytes owned by the compositor.
    let mut fb_canvas = unsafe { Canvas::from_shm(fb_va, FB_W, FB_H) };
    fb_canvas.clear(BG);

    let caps = libdelta::sys_caps().unwrap_or_else(|_| SysCaps::empty());
    let blur_shm = libdelta::shm_create(fb_bytes).ok();
    let mut comp = CompositorState::new(vfs, fb_port, fb_shm_id, audio, caps, blur_shm);
    comp.cfg.load_from_vfs(vfs);
    comp.temp_mc = query_soc_temp_mc(fb_port).unwrap_or(42_000);

    play_boot_chime(audio, comp.cfg.volume, comp.cfg.audio_route);
    comp.present_splash(&mut fb_canvas, "Starting Hollow Star Cyberdeck...");

    libdelta::print_via(
        uart,
        "[el0/compositor] Desktop compositor ready (svc.desktop, USB HID kbd, Calibri UI).\n",
    );

    let mut current_exec = [0u8; 64];
    let initial_path = if comp.cfg.setup_done {
        "/system/apps/Launcher.hsapp/exec"
    } else {
        "/system/apps/Setup.hsapp/exec"
    };
    let mut current_len = copy_path(&mut current_exec, initial_path);
    let mut current_arg = 0u64;

    loop {
        comp.present_splash(&mut fb_canvas, "Launching .hsapp...");

        let path_str = core::str::from_utf8(&current_exec[..current_len])
            .unwrap_or("/system/apps/Launcher.hsapp/exec");
        let Ok(child_pid) = libdelta::spawn_from_vfs("hsapp", path_str) else {
            libdelta::print_via(uart, "[el0/compositor] Failed to spawn: ");
            libdelta::print_via(uart, path_str);
            libdelta::print_via(uart, "\n");
            break;
        };

        let mut next_exec = [0u8; 64];
        let mut next_len = copy_path(&mut next_exec, "/system/apps/Launcher.hsapp/exec");
        let mut next_arg = 0u64;
        let mut session_done = false;
        let mut child_shm: Option<(u64, usize)> = None;
        let mut msg = IpcMessage::new(0);

        while !session_done {
            if libdelta::port_recv(desktop_port, &mut msg).is_err() {
                libdelta::yield_now();
                continue;
            }
            match msg.tag {
                MSG_DESKTOP_CONNECT => {
                    let mut reply_pkt = DesktopPacket::empty();
                    if let Some(req) = unpack_msg::<DesktopPacket>(&msg) {
                        if let Some((old_id, _)) = child_shm.take() {
                            let _ = libdelta::shm_unmap(old_id);
                        }
                        match libdelta::shm_map(req.shm_id) {
                            Ok((va, size)) if size >= APP_W * APP_H * 4 => {
                                child_shm = Some((req.shm_id, va));
                                reply_pkt.width = APP_W as u32;
                                reply_pkt.height = APP_H as u32;
                                reply_pkt.arg = current_arg;
                            }
                            _ => {
                                reply_pkt.status = errno::ENOMEM as i32;
                            }
                        }
                    } else {
                        reply_pkt.status = errno::EINVAL as i32;
                    }
                    reply_pkt_fn(msg.reply_port, &reply_pkt);
                }
                MSG_DESKTOP_PRESENT => {
                    comp.composite_and_present(&mut fb_canvas, child_shm);
                    if !comp.boot_banner_sent {
                        comp.boot_banner_sent = true;
                        libdelta::print_via(
                            uart,
                            "[boot] Milestone M6 (Multimedia, DeltaFS & UI) ready.\n",
                        );
                    }
                    reply_pkt_fn(msg.reply_port, &DesktopPacket::empty());
                }
                MSG_DESKTOP_POLL_KEY => {
                    comp.tick(&mut fb_canvas, child_shm);
                    let mut pkt = DesktopPacket::empty();
                    if let Some((kind, ch)) = comp.poll_unified_key(&mut fb_canvas, child_shm) {
                        pkt.key_kind = kind as u8;
                        pkt.key_ch = ch;
                    }
                    reply_pkt_fn(msg.reply_port, &pkt);
                }
                MSG_DESKTOP_REFRESH_CFG => {
                    comp.cfg.load_from_vfs(vfs);
                    comp.composite_and_present(&mut fb_canvas, child_shm);
                    reply_pkt_fn(msg.reply_port, &DesktopPacket::empty());
                }
                MSG_DESKTOP_LAUNCH => {
                    if let Some(req) = unpack_msg::<DesktopPacket>(&msg) {
                        let target = req.text_str();
                        if !target.is_empty() {
                            next_len = copy_path(&mut next_exec, target);
                            next_arg = req.arg;
                        }
                    }
                    reply_pkt_fn(msg.reply_port, &DesktopPacket::empty());
                    session_done = true;
                }
                MSG_DESKTOP_EXIT_TO_SHELL => {
                    next_len = copy_path(&mut next_exec, "/system/apps/Launcher.hsapp/exec");
                    next_arg = 0;
                    reply_pkt_fn(msg.reply_port, &DesktopPacket::empty());
                    session_done = true;
                }
                _ => {
                    let mut pkt = DesktopPacket::empty();
                    pkt.status = errno::ENOSYS as i32;
                    reply_pkt_fn(msg.reply_port, &pkt);
                }
            }
        }

        if let Some((old_id, _)) = child_shm.take() {
            let _ = libdelta::shm_unmap(old_id);
        }
        let _ = libdelta::wait_pid(child_pid);
        comp.cfg.load_from_vfs(vfs);
        current_exec = next_exec;
        current_len = next_len;
        current_arg = next_arg;
    }

    if let Some((blur_id, _)) = blur_shm {
        let _ = libdelta::shm_unmap(blur_id);
    }
    let _ = libdelta::shm_unmap(fb_shm_id);
    0
}

struct CompositorState {
    _vfs: usize,
    fb_port: usize,
    fb_shm_id: u64,
    _audio: usize,
    kbd_port: Option<usize>,
    caps: SysCaps,
    blur_shm: Option<(u64, usize)>,
    cfg: OsConfig,
    temp_mc: u32,
    help_open: bool,
    boot_banner_sent: bool,
    last_clock_ms: u64,
    last_temp_ms: u64,
}

impl CompositorState {
    fn new(
        vfs: usize,
        fb_port: usize,
        fb_shm_id: u64,
        audio: usize,
        caps: SysCaps,
        blur_shm: Option<(u64, usize)>,
    ) -> Self {
        let now = libdelta::uptime_ms();
        Self {
            _vfs: vfs,
            fb_port,
            fb_shm_id,
            _audio: audio,
            kbd_port: libdelta::service_lookup(SVC_KBD).ok(),
            caps,
            blur_shm,
            cfg: OsConfig::defaults(),
            temp_mc: 42_000,
            help_open: false,
            boot_banner_sent: false,
            last_clock_ms: now,
            last_temp_ms: now,
        }
    }

    fn blur_scratch(&self) -> Option<&'static mut [u32]> {
        let (_, va) = self.blur_shm?;
        // SAFETY: `blur_shm` was allocated with `FB_W * FB_H * 4` bytes, mapped into our
        // address space, and only accessed transiently inside single-threaded compositor draws.
        Some(unsafe { core::slice::from_raw_parts_mut(va as *mut u32, FB_W * FB_H) })
    }

    fn flush_fb(&self, brightness: u8) {
        let mut pkt = FbPacket {
            shm_id: self.fb_shm_id,
            width: FB_W as u32,
            height: FB_H as u32,
            pitch: (FB_W * 4) as u32,
            bpp: 32,
            shm_bytes: (FB_W * FB_H * 4) as u32,
            soc_temp_mc: 0,
            brightness,
            hw_active: 0,
            _pad: [0; 2],
            status: 0,
        };
        let req = pack_msg(MSG_FB_PRESENT, &pkt);
        let mut reply = IpcMessage::new(0);
        let _ = libdelta::port_call(self.fb_port, &req, &mut reply);
        let _ = &mut pkt;
    }

    fn present_splash(&self, fb_canvas: &mut Canvas, msg: &str) {
        fb_canvas.clear(BG);
        self.draw_status_bar(fb_canvas);
        fb_canvas.draw_arc_star((FB_W / 2) as i32, 192, 24, ACCENT_YELLOW, ACCENT_AMBER);
        fb_canvas.fill_gradient_h(210, 226, 220, 2, ACCENT_YELLOW, ACCENT_AMBER);
        fb_canvas.draw_text_centered((FB_W / 2) as i32, 244, msg, MUTED, 1);
        self.flush_fb(self.cfg.brightness);
    }

    fn tick(&mut self, fb_canvas: &mut Canvas, child_shm: Option<(u64, usize)>) {
        let now = libdelta::uptime_ms();
        let mut dirty = false;

        if now.saturating_sub(self.last_clock_ms) >= 60_000 {
            self.last_clock_ms = now;
            self.cfg.clock_mm = (self.cfg.clock_mm + 1) % 60;
            if self.cfg.clock_mm == 0 {
                self.cfg.clock_hh = (self.cfg.clock_hh + 1) % 24;
            }
            dirty = true;
        }

        if now.saturating_sub(self.last_temp_ms) >= 10_000 {
            self.last_temp_ms = now;
            if let Some(new_temp) = query_soc_temp_mc(self.fb_port)
                && new_temp / 1000 != self.temp_mc / 1000
            {
                self.temp_mc = new_temp;
                dirty = true;
            }
        }

        if dirty {
            self.composite_and_present(fb_canvas, child_shm);
        }
    }

    fn composite_and_present(&self, fb_canvas: &mut Canvas, child_shm: Option<(u64, usize)>) {
        if let Some((_, app_va)) = child_shm {
            let copy_pixels = APP_W * APP_H;
            // SAFETY: `app_va` was mapped and verified to hold at least `APP_W * APP_H * 4` bytes.
            let src = unsafe { core::slice::from_raw_parts(app_va as *const u32, copy_pixels) };
            let dst = &mut fb_canvas.pixels_mut()[BAR_H * FB_W..FB_H * FB_W];
            dst.copy_from_slice(src);
        }

        self.draw_status_bar(fb_canvas);

        if self.help_open {
            self.draw_help_overlay(fb_canvas);
        }

        self.flush_fb(self.cfg.brightness);
    }

    fn draw_status_bar(&self, c: &mut Canvas) {
        c.fill_rect(0, 0, FB_W as i32, BAR_H as i32, PANEL);
        c.fill_gradient_h(
            0,
            (BAR_H - 2) as i32,
            FB_W as i32,
            2,
            ACCENT_YELLOW,
            ACCENT_AMBER,
        );

        c.draw_arc_star(15, 13, 9, ACCENT_YELLOW, ACCENT_AMBER);
        c.draw_text(29, 6, "Hollow Star OS+", TEXT);
        c.draw_arc_box(
            148,
            4,
            82,
            19,
            ArcSharpStyle::sharp(),
            PANEL_ALT,
            ACCENT_YELLOW,
            ACCENT_AMBER,
        );
        c.draw_text_centered(189, 6, "CYBERDECK", ACCENT_YELLOW, 1);

        let mut line = [0u8; 96];
        let mut w = BufWriter::new(&mut line);
        w.str(if self.cfg.lang == Lang::Pl {
            "PL | "
        } else {
            "EN | "
        });
        if wireless_enabled(&self.caps) {
            w.str("BT/WiFi | ");
        }
        w.str(if self.cfg.audio_route == 1 {
            "HDMI "
        } else {
            "JACK "
        });
        w.u32(u32::from(self.cfg.volume));
        w.str("% | ");
        w.u32(self.temp_mc / 1000);
        w.str("C | ");
        w.u32_pad2(u32::from(self.cfg.clock_hh));
        w.str(":");
        w.u32_pad2(u32::from(self.cfg.clock_mm));
        w.str(" | [F1]");

        c.draw_text_right((FB_W - 10) as i32, 6, w.as_str(), MUTED);
    }

    fn animate_help_reveal(&self, fb_canvas: &mut Canvas) {
        if let Some(scratch) = self.blur_scratch() {
            fb_canvas.blur_tinted(
                Rect {
                    x: 74,
                    y: 82,
                    w: 492,
                    h: 290,
                },
                3,
                PANEL,
                155,
                scratch,
            );
        }
        for step in 1..=3u32 {
            let p = ease_out_cubic((step * 255) / 3) as i32;
            let cur_w = 120 + ((492 - 120) * p) / 255;
            let cur_h = 64 + ((290 - 64) * p) / 255;
            let cx = 320 - cur_w / 2;
            let cy = 227 - cur_h / 2;
            fb_canvas.draw_arc_box(
                cx,
                cy,
                cur_w,
                cur_h,
                ArcSharpStyle::modal(),
                PANEL,
                ACCENT_YELLOW,
                ACCENT_AMBER,
            );
            self.flush_fb(self.cfg.brightness);
            sleep_ms(12);
        }
    }

    fn draw_help_overlay(&self, c: &mut Canvas) {
        let lang = self.cfg.lang;
        if let Some(scratch) = self.blur_scratch() {
            c.blur_tinted(
                Rect {
                    x: 74,
                    y: 82,
                    w: 492,
                    h: 290,
                },
                3,
                PANEL,
                155,
                scratch,
            );
        }
        c.draw_arc_box(
            74,
            82,
            492,
            290,
            ArcSharpStyle::modal(),
            PANEL,
            ACCENT_YELLOW,
            ACCENT_AMBER,
        );
        c.fill_rect(82, 86, 476, 28, PANEL_ALT);
        c.fill_gradient_h(82, 114, 476, 2, ACCENT_YELLOW, ACCENT_AMBER);
        c.draw_arc_star(96, 100, 8, ACCENT_YELLOW, ACCENT_AMBER);
        c.draw_text(
            110,
            91,
            tr(
                lang,
                "Quick Help [F1] - Keyboard Controls",
                "Pomoc [F1] - Sterowanie Klawiatura",
            ),
            ACCENT,
        );

        let mut y = 128;
        for line in [
            tr(
                lang,
                "Arrows / Tab  : Move focus or scroll items",
                "Strzalki / Tab: Wybor elementu lub przewijanie",
            ),
            tr(
                lang,
                "Enter         : Launch app / Confirm action",
                "Enter         : Uruchom aplikacje / Zatwierdz",
            ),
            tr(
                lang,
                "Esc / Q       : Return to Launcher / Back",
                "Esc / Q       : Wroc do Launchera / Wstecz",
            ),
            tr(
                lang,
                "Terminal.hsapp: Type POSIX/VFS commands + Enter",
                "Terminal.hsapp: Wpisz komendy VFS i nacisnij Enter",
            ),
            tr(
                lang,
                "Ctrl+S        : Save document (Markdown Notes)",
                "Ctrl+S        : Zapisz plik (Notatki Markdown)",
            ),
            tr(
                lang,
                "Ctrl+N / Ctrl+F : New note / Find text",
                "Ctrl+N / Ctrl+F : Nowa notatka / Szukaj tekstu",
            ),
            tr(
                lang,
                "F1            : Open or close this Help overlay",
                "F1            : Pokaz lub zamknij to okno Pomocy",
            ),
        ] {
            c.draw_text(98, y, line, TEXT);
            y += 26;
        }

        c.draw_text_centered(
            320,
            340,
            tr(
                lang,
                "Press [F1], [Enter] or [Esc] to resume",
                "Nacisnij [F1], [Enter] lub [Esc] aby kontynuowac",
            ),
            OK_GREEN,
            1,
        );
        c.draw_text_right(552, 340, "USB HID Cyberdeck", CYAN);
    }

    fn poll_unified_key(
        &mut self,
        fb_canvas: &mut Canvas,
        child_shm: Option<(u64, usize)>,
    ) -> Option<(KeyCode, u8)> {
        let key = self.poll_raw_key()?;
        if key.0 == KeyCode::F1 {
            self.help_open = !self.help_open;
            if self.help_open {
                self.animate_help_reveal(fb_canvas);
            }
            self.composite_and_present(fb_canvas, child_shm);
            return None;
        }
        if self.help_open {
            if matches!(key.0, KeyCode::Esc | KeyCode::Enter) {
                self.help_open = false;
                self.composite_and_present(fb_canvas, child_shm);
            }
            return None;
        }
        Some(key)
    }

    fn poll_raw_key(&mut self) -> Option<(KeyCode, u8)> {
        if self.kbd_port.is_none() {
            self.kbd_port = libdelta::service_lookup(SVC_KBD).ok();
        }
        if let Some(kbd) = self.kbd_port {
            let req = IpcMessage::new(MSG_KBD_READ);
            let mut reply = IpcMessage::new(0);
            if libdelta::port_call(kbd, &req, &mut reply).is_ok()
                && reply.tag == MSG_KBD_ACK
                && let Some(pkt) = unpack_msg::<KbdPacket>(&reply)
            {
                let kind = KeyCode::from_u8(pkt.kind);
                if kind != KeyCode::None {
                    return Some((kind, pkt.ch));
                }
            }
        }
        None
    }
}

fn reply_pkt_fn(reply_port: u64, pkt: &DesktopPacket) {
    if reply_port != 0 {
        let reply = pack_msg(MSG_DESKTOP_ACK, pkt);
        let _ = libdelta::port_reply(reply_port as usize, &reply);
    }
}

fn query_soc_temp_mc(fb_port: usize) -> Option<u32> {
    let req_pkt = FbPacket {
        shm_id: 0,
        width: 0,
        height: 0,
        pitch: 0,
        bpp: 0,
        shm_bytes: 0,
        soc_temp_mc: 0,
        brightness: 100,
        hw_active: 0,
        _pad: [0; 2],
        status: 0,
    };
    let req = pack_msg(MSG_FB_GET_INFO, &req_pkt);
    let mut reply = IpcMessage::new(0);
    libdelta::port_call(fb_port, &req, &mut reply).ok()?;
    let pkt = unpack_msg::<FbPacket>(&reply)?;
    if pkt.soc_temp_mc > 0 {
        Some(pkt.soc_temp_mc as u32)
    } else {
        None
    }
}

fn play_boot_chime(audio: usize, volume: u8, route: u8) {
    for (freq, ms) in [(523, 55), (659, 55), (784, 80)] {
        let pkt = AudioPacket {
            shm_id: 0,
            frames_mixed: 0,
            sample_count: 0,
            freq_hz: freq,
            duration_ms: ms,
            volume: volume.max(20),
            route,
            playing: 0,
            jack_plugged: 0,
            rt_priority: 110,
            _pad: [0; 3],
            status: 0,
        };
        let req = pack_msg(MSG_AUDIO_PLAY_TONE, &pkt);
        let mut reply = IpcMessage::new(0);
        let _ = libdelta::port_call(audio, &req, &mut reply);
    }
}

fn copy_path(dst: &mut [u8; 64], path: &str) -> usize {
    let n = path.len().min(dst.len());
    dst[..n].copy_from_slice(&path.as_bytes()[..n]);
    n
}

//! Modernized Hollow Star UI pixel canvas (`XRGB8888`) backed by `Shm`
//! featuring lightened slate palette (`#131926`/`#1B2336`), Golden Yellow (`#F4B41A`) +
//! Amber (`#FFD166`) accents, subtly rounded panels + concave Bezier star emblem,
//! frosted blur, and Calibri/Monospace fonts.

use crate::calibri::{FONT_HEIGHT, MONO_WIDTH, glyph_calibri, glyph_mono, measure_calibri};
use crate::fx::{ArcSharpStyle, Rect, blur_tinted_panel, lerp_color, point_in_arc_sharp};

pub const FB_W: usize = 640;
pub const FB_H: usize = 480;
pub const BAR_H: usize = 28;
pub const APP_W: usize = 640;
pub const APP_H: usize = FB_H - BAR_H;

/// Lightened deep-slate canvas background (`#131926`).
pub const BG: u32 = 0x0013_1926;
/// Elevated panel / tile surface (`#1B2336`).
pub const PANEL: u32 = 0x001B_2336;
/// Frosted / hover elevated surface (`#222C42`).
pub const PANEL_ALT: u32 = 0x0022_2C42;
/// Subtle structural border (`#384766`).
pub const BORDER: u32 = 0x0038_4766;

/// Primary Golden Yellow accent (`#F4B41A`).
pub const ACCENT_YELLOW: u32 = 0x00F4_B41A;
/// Warm Amber highlight (`#FFD166`) paired with Golden Yellow gradients.
pub const ACCENT_AMBER: u32 = 0x00FF_D166;
/// Muted warm Gold border/divider (`#8A6210`).
pub const ACCENT_GOLD_DIM: u32 = 0x008A_6210;

/// Canonical primary accent alias (Golden Yellow).
pub const ACCENT: u32 = ACCENT_YELLOW;
/// Canonical dim accent alias (Warm Gold).
pub const ACCENT_DIM: u32 = ACCENT_GOLD_DIM;
/// Secondary slate-cyan for metadata/paths (`#5CE1E6`).
pub const CYAN: u32 = 0x005C_E1E6;

/// Primary crisp foreground text (`#EDF2FA`).
pub const TEXT: u32 = 0x00ED_F2FA;
/// Secondary muted text (`#94A3B8`).
pub const MUTED: u32 = 0x0094_A3B8;
pub const OK_GREEN: u32 = 0x003F_B950;
pub const WARN_RED: u32 = 0x00FF_5370;

pub struct Canvas {
    buf: &'static mut [u32],
    pub width: usize,
    pub height: usize,
}

impl Canvas {
    /// Wraps a `Shm`-mapped pixel buffer of size `width * height` pixels.
    ///
    /// # Safety
    /// `shm_va` must map at least `width * height * 4` writable bytes for the lifetime of `Canvas`.
    pub unsafe fn from_shm(shm_va: usize, width: usize, height: usize) -> Self {
        // SAFETY: Caller guarantees exclusive mapped `Shm` pixel memory of size `width * height`.
        let buf = unsafe { core::slice::from_raw_parts_mut(shm_va as *mut u32, width * height) };
        Self { buf, width, height }
    }

    pub fn pixels(&self) -> &[u32] {
        self.buf
    }

    pub fn pixels_mut(&mut self) -> &mut [u32] {
        self.buf
    }

    pub fn clear(&mut self, color: u32) {
        self.buf.fill(color);
    }

    #[inline]
    pub fn put_px(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.buf[(y as usize) * self.width + (x as usize)] = color;
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x + w).min(self.width as i32).max(0) as usize;
        let y1 = (y + h).min(self.height as i32).max(0) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for py in y0..y1 {
            let row = py * self.width;
            self.buf[row + x0..row + x1].fill(color);
        }
    }

    pub fn draw_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
    }

    /// Draws an Arc-Sharp stylized box (rounded arcs meeting sharp pointed cusps) cleanly
    /// matching the Hollow Star aesthetic.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_arc_box(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        style: ArcSharpStyle,
        fill: u32,
        border_a: u32,
        border_b: u32,
    ) {
        if w <= 0 || h <= 0 {
            return;
        }
        let max_sum = (w + h).max(1);
        for ly in 0..h {
            let py = y + ly;
            if py < 0 || py >= self.height as i32 {
                continue;
            }
            for lx in 0..w {
                let px = x + lx;
                if px < 0 || px >= self.width as i32 {
                    continue;
                }
                if point_in_arc_sharp(lx, ly, w, h, style) {
                    let is_edge = !point_in_arc_sharp(lx - 1, ly, w, h, style)
                        || !point_in_arc_sharp(lx + 1, ly, w, h, style)
                        || !point_in_arc_sharp(lx, ly - 1, w, h, style)
                        || !point_in_arc_sharp(lx, ly + 1, w, h, style);
                    let col = if is_edge {
                        if border_a == border_b {
                            border_a
                        } else {
                            let t = (((lx + ly) * 255) / max_sum).clamp(0, 255) as u8;
                            lerp_color(border_a, border_b, t)
                        }
                    } else {
                        fill
                    };
                    self.buf[(py as usize) * self.width + (px as usize)] = col;
                }
            }
        }
    }

    /// Standard card/panel box — uses `ArcSharpStyle::cyberdeck_card()` so the entire OS
    /// adopts the new Arc-Sharp logo aesthetic automatically.
    pub fn draw_box(&mut self, x: i32, y: i32, w: i32, h: i32, fill: u32, border: u32) {
        self.draw_arc_box(
            x,
            y,
            w,
            h,
            ArcSharpStyle::cyberdeck_card(),
            fill,
            border,
            border,
        );
    }

    /// Draws a horizontal linear gradient bar between `c0` and `c1`.
    pub fn fill_gradient_h(&mut self, x: i32, y: i32, w: i32, h: i32, c0: u32, c1: u32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let denom = (w - 1).max(1);
        for lx in 0..w {
            let t = ((lx * 255) / denom).clamp(0, 255) as u8;
            let col = lerp_color(c0, c1, t);
            self.fill_rect(x + lx, y, 1, h, col);
        }
    }

    /// Applies frosted glass blur tinted with `tint` over `rect`.
    pub fn blur_tinted(
        &mut self,
        rect: Rect,
        radius: usize,
        tint: u32,
        alpha: u8,
        scratch: &mut [u32],
    ) {
        blur_tinted_panel(
            self.buf,
            self.width,
            self.height,
            rect,
            radius,
            tint,
            alpha,
            scratch,
        );
    }

    pub fn draw_line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: u32) {
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.put_px(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draws the 5-pointed **Hollow Star** emblem using smooth concave quadratic Bezier arcs
    /// meeting at crisp, sharp star tips with a Golden Yellow -> Warm Amber gradient.
    pub fn draw_hollow_star(&mut self, cx: i32, cy: i32, r_outer: i32, _color_unused: u32) {
        self.draw_arc_star(cx, cy, r_outer, ACCENT_YELLOW, ACCENT_AMBER);
    }

    /// Draws the concave-arc Hollow Star logo with an explicit two-color gradient.
    pub fn draw_arc_star(&mut self, cx: i32, cy: i32, r_outer: i32, color_a: u32, color_b: u32) {
        if r_outer <= 2 {
            return;
        }
        const TIPS: [(i32, i32); 5] = [(0, -256), (243, -79), (150, 207), (-150, 207), (-243, -79)];
        const CTRL: [(i32, i32); 5] = [(58, -80), (94, 31), (0, 98), (-94, 31), (-58, -80)];

        for i in 0..5 {
            let next = (i + 1) % 5;
            let p0x = cx + (TIPS[i].0 * r_outer) / 256;
            let p0y = cy + (TIPS[i].1 * r_outer) / 256;
            let p1x = cx + (CTRL[i].0 * r_outer) / 256;
            let p1y = cy + (CTRL[i].1 * r_outer) / 256;
            let p2x = cx + (TIPS[next].0 * r_outer) / 256;
            let p2y = cy + (TIPS[next].1 * r_outer) / 256;

            let mut prev_x = p0x;
            let mut prev_y = p0y;
            for step in 1..=28 {
                let t = step;
                let inv = 28 - t;
                let qx = (inv * inv * p0x + 2 * inv * t * p1x + t * t * p2x) / 784;
                let qy = (inv * inv * p0y + 2 * inv * t * p1y + t * t * p2y) / 784;
                let grad_t = (((i as i32) * 28 + step) * 255 / 140) as u8;
                let col = lerp_color(color_a, color_b, grad_t);
                self.draw_line(prev_x, prev_y, qx, qy, col);
                prev_x = qx;
                prev_y = qy;
            }
        }
    }

    /// Draws text using the Calibri-inspired proportional font at `scale`.
    pub fn draw_text_scaled(&mut self, x: i32, y: i32, text: &str, color: u32, scale: i32) {
        let mut cx = x;
        let s = scale.max(1);
        let max_x = self.width as i32 - 4;
        for ch in text.chars() {
            let g = glyph_calibri(ch);
            for (row, &mask) in g.rows.iter().enumerate() {
                if mask == 0 {
                    continue;
                }
                for col in 0..i32::from(g.width) {
                    if (mask & (0x8000u16 >> col)) != 0 {
                        self.fill_rect(cx + col * s, y + (row as i32) * s, s, s, color);
                    }
                }
            }
            cx += i32::from(g.advance) * s;
            if cx >= max_x {
                break;
            }
        }
    }

    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: u32) {
        self.draw_text_scaled(x, y, text, color, 1);
    }

    pub fn draw_text_centered(&mut self, cx: i32, y: i32, text: &str, color: u32, scale: i32) {
        let w = measure_calibri(text, scale);
        self.draw_text_scaled(cx - w / 2, y, text, color, scale);
    }

    pub fn draw_text_right(&mut self, x_right: i32, y: i32, text: &str, color: u32) {
        let w = measure_calibri(text, 1);
        self.draw_text(x_right - w, y, text, color);
    }

    /// Draws a single fixed-width `8x14` monospace character at `(x, y)`.
    pub fn draw_mono_char(&mut self, x: i32, y: i32, ch: char, color: u32) {
        let rows = glyph_mono(ch);
        for (r, &mask) in rows.iter().enumerate() {
            if mask == 0 {
                continue;
            }
            for c in 0..8 {
                if (mask & (0x80 >> c)) != 0 {
                    self.put_px(x + c, y + r as i32, color);
                }
            }
        }
    }

    /// Draws a line of fixed-width `8x14` monospace text (for Terminal Emulator & hex tables).
    pub fn draw_mono_text(&mut self, x: i32, y: i32, text: &str, color: u32) {
        let mut cx = x;
        let max_x = self.width as i32 - MONO_WIDTH;
        for ch in text.chars() {
            if cx > max_x {
                break;
            }
            self.draw_mono_char(cx, y, ch, color);
            cx += MONO_WIDTH;
        }
    }

    pub const fn text_height() -> i32 {
        FONT_HEIGHT
    }
}

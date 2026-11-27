//! Sharp-edged pixel canvas (`XRGB8888`) backed by `Shm` with Calibri & Monospace rendering.

use crate::calibri::{FONT_HEIGHT, MONO_WIDTH, glyph_calibri, glyph_mono, measure_calibri};

pub const FB_W: usize = 640;
pub const FB_H: usize = 480;
pub const BAR_H: usize = 28;
pub const APP_W: usize = 640;
pub const APP_H: usize = FB_H - BAR_H;

pub const BG: u32 = 0x000A_0E14;
pub const PANEL: u32 = 0x0012_1822;
pub const PANEL_ALT: u32 = 0x0017_1F2C;
pub const ACCENT: u32 = 0x00F4_B41A;
pub const ACCENT_DIM: u32 = 0x008A_6510;
pub const BORDER: u32 = 0x003D_4B60;
pub const TEXT: u32 = 0x00E6_EDF3;
pub const MUTED: u32 = 0x008B_949E;
pub const OK_GREEN: u32 = 0x003F_B950;
pub const WARN_RED: u32 = 0x00F8_5149;
pub const CYAN: u32 = 0x0058_A6FF;

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

    pub fn draw_box(&mut self, x: i32, y: i32, w: i32, h: i32, fill: u32, border: u32) {
        self.fill_rect(x, y, w, h, fill);
        self.draw_rect(x, y, w, h, border);
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

    /// Draws the 5-pointed **Hollow Star** emblem centered at `(cx, cy)`.
    pub fn draw_hollow_star(&mut self, cx: i32, cy: i32, r_outer: i32, color: u32) {
        let r_inner = (r_outer * 38) / 100;
        const UNIT_10: [(i32, i32); 10] = [
            (0, -256),
            (150, -207),
            (243, -79),
            (243, 79),
            (150, 207),
            (0, 256),
            (-150, 207),
            (-243, 79),
            (-243, -79),
            (-150, -207),
        ];
        let mut coords = [(0i32, 0i32); 10];
        for i in 0..10 {
            let r = if i % 2 == 0 { r_outer } else { r_inner };
            coords[i] = (cx + (UNIT_10[i].0 * r) / 256, cy + (UNIT_10[i].1 * r) / 256);
        }
        for i in 0..10 {
            let next = (i + 1) % 10;
            self.draw_line(
                coords[i].0,
                coords[i].1,
                coords[next].0,
                coords[next].1,
                color,
            );
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

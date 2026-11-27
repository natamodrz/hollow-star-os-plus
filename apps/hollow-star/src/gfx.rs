//! Sharp-edged retro pixel canvas (`640x480` XRGB8888) backed by `Shm` for zero-tearing present.

use crate::font::glyph_for;

pub const SCREEN_W: usize = 640;
pub const SCREEN_H: usize = 480;

pub const BG: u32 = 0x000A_0E14;
pub const PANEL: u32 = 0x0012_1822;
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
}

impl Canvas {
    /// # Safety
    /// `shm_va` must map at least `SCREEN_W * SCREEN_H * 4` writable bytes for the lifetime of `Canvas`.
    pub unsafe fn from_shm(shm_va: usize) -> Self {
        // SAFETY: Caller guarantees exclusive mapped `Shm` pixel memory of size `SCREEN_W * SCREEN_H`.
        let buf =
            unsafe { core::slice::from_raw_parts_mut(shm_va as *mut u32, SCREEN_W * SCREEN_H) };
        Self { buf }
    }

    pub fn clear(&mut self, color: u32) {
        self.buf.fill(color);
    }

    #[inline]
    pub fn put_px(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && (x as usize) < SCREEN_W && (y as usize) < SCREEN_H {
            self.buf[(y as usize) * SCREEN_W + (x as usize)] = color;
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x + w).min(SCREEN_W as i32).max(0) as usize;
        let y1 = (y + h).min(SCREEN_H as i32).max(0) as usize;
        for py in y0..y1 {
            let row = py * SCREEN_W;
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
        // Fixed-point 10-vertex angles (cos*256, sin*256) starting from -90 deg (top point).
        const PTS: [(i32, i32); 10] = [
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
        // Even indices = outer radius, odd indices = inner radius; actually standard 5-star uses alternating vertices:
        const STAR_VERTS: [(i32, i32); 10] = [
            (0, -256),   // outer 0 (top)
            (150, -207), // will be scaled with r_inner? Wait: 36 deg intervals!
            (243, -79),  // outer 72 deg
            (150, 207),  // wait: 108 deg
            (0, 256),    // inner bottom
            (0, 256),
            (-150, 207),
            (-243, -79),
            (-243, -79),
            (-150, -207),
        ];
        let _ = (PTS, STAR_VERTS);
        // Exact 36-degree step unit vectors (scaled by 256):
        // k=0: -90, k=1: -54, k=2: -18, k=3: +18, k=4: +54, k=5: +90, k=6: +126, k=7: +162, k=8: +198, k=9: +234
        const UNIT_10: [(i32, i32); 10] = [
            (0, -256),    // -90 deg (outer)
            (150, -207),  // -54 deg (inner)
            (243, -79),   // -18 deg (outer)
            (243, 79),    // +18 deg (inner)
            (150, 207),   // +54 deg (outer)
            (0, 256),     // +90 deg (inner)
            (-150, 207),  // +126 deg (outer)
            (-243, 79),   // +162 deg (inner)
            (-243, -79),  // +198 deg (outer)
            (-150, -207), // +234 deg (inner)
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

    pub fn draw_text_scaled(&mut self, x: i32, y: i32, text: &str, color: u32, scale: i32) {
        let mut cx = x;
        let s = scale.max(1);
        for ch in text.chars() {
            let bits = glyph_for(ch);
            for (row, &mask) in bits.iter().enumerate() {
                for col in 0..8 {
                    if (mask & (0x80 >> col)) != 0 {
                        self.fill_rect(cx + col * s, y + (row as i32) * s, s, s, color);
                    }
                }
            }
            cx += 8 * s;
            if cx >= SCREEN_W as i32 - 8 {
                break;
            }
        }
    }

    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: u32) {
        self.draw_text_scaled(x, y, text, color, 1);
    }
}

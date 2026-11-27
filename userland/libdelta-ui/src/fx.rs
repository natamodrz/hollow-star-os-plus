//! Pure-integer frosted blur, fixed-point easing animations, and Arc-Sharp rasterizers.

/// Fixed-point `1.0` scale (`Q16.16` normalized parameter `0..=65536`).
pub const Q16_ONE: u32 = 65_536;

/// Corner geometry for UI panels, tiles, and badges (subtle convex rounding or crisp 90° corners).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CornerStyle {
    /// Smooth subtle convex outer rounding of `radius` pixels (no inward bites).
    Rounded(u16),
    /// Crisp 90-degree sharp corner for technical badges and structural panels.
    Sharp,
}

/// 4-corner descriptor for cyberdeck cards, panels, and pills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArcSharpStyle {
    pub top_left: CornerStyle,
    pub top_right: CornerStyle,
    pub bottom_right: CornerStyle,
    pub bottom_left: CornerStyle,
}

impl ArcSharpStyle {
    /// Clean, lightly rounded card container (`radius = 6`).
    pub const fn cyberdeck_card() -> Self {
        Self {
            top_left: CornerStyle::Rounded(6),
            top_right: CornerStyle::Rounded(6),
            bottom_right: CornerStyle::Rounded(6),
            bottom_left: CornerStyle::Rounded(6),
        }
    }

    /// Clean lightly rounded tile style for the fullscreen cyberdeck & DIY PDA launcher.
    pub const fn pda_tile() -> Self {
        Self {
            top_left: CornerStyle::Rounded(6),
            top_right: CornerStyle::Rounded(6),
            bottom_right: CornerStyle::Rounded(6),
            bottom_left: CornerStyle::Rounded(6),
        }
    }

    /// Crisp sharp-edged technical badge or terminal panel.
    pub const fn sharp() -> Self {
        Self {
            top_left: CornerStyle::Sharp,
            top_right: CornerStyle::Sharp,
            bottom_right: CornerStyle::Sharp,
            bottom_left: CornerStyle::Sharp,
        }
    }

    /// Modal dialog style with subtle convex rounding (`radius = 8`).
    pub const fn modal() -> Self {
        Self {
            top_left: CornerStyle::Rounded(8),
            top_right: CornerStyle::Rounded(8),
            bottom_right: CornerStyle::Rounded(8),
            bottom_left: CornerStyle::Rounded(8),
        }
    }
}

/// Axis-aligned pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
}

/// Linearly interpolates between `c0` and `c1` (`XRGB8888`), where `t` is `0..=255`.
#[inline]
pub const fn lerp_color(c0: u32, c1: u32, t: u8) -> u32 {
    let t32 = t as u32;
    let inv = 255 - t32;
    let r = ((((c0 >> 16) & 0xFF) * inv) + (((c1 >> 16) & 0xFF) * t32) + 127) / 255;
    let g = ((((c0 >> 8) & 0xFF) * inv) + (((c1 >> 8) & 0xFF) * t32) + 127) / 255;
    let b = (((c0 & 0xFF) * inv) + ((c1 & 0xFF) * t32) + 127) / 255;
    (r << 16) | (g << 8) | b
}

/// Cubic ease-out animation curve (`1 - (1 - t)^3`) in `0..=65536` fixed-point.
pub const fn ease_out_cubic(t_q16: u32) -> u32 {
    let t = if t_q16 > Q16_ONE { Q16_ONE } else { t_q16 };
    let inv = (Q16_ONE - t) as u64;
    let inv2 = (inv * inv) >> 16;
    let inv3 = (inv2 * inv) >> 16;
    Q16_ONE - (inv3 as u32)
}

/// Quadratic ease-in-out animation curve in `0..=65536` fixed-point.
pub const fn ease_in_out_quad(t_q16: u32) -> u32 {
    let t = if t_q16 > Q16_ONE { Q16_ONE } else { t_q16 } as u64;
    if t < (Q16_ONE as u64) / 2 {
        ((2 * t * t) >> 16) as u32
    } else {
        let inv = (Q16_ONE as u64) - t;
        Q16_ONE - (((2 * inv * inv) >> 16) as u32)
    }
}

/// Applies a fast 2-pass integer box blur over `rect` inside `pixels` (`width x height`).
///
/// Requires a caller-provided `scratch` slice of length at least `cw * ch`.
pub fn box_blur_region(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    rect: Rect,
    radius: usize,
    scratch: &mut [u32],
) {
    if radius == 0 || width == 0 || height == 0 || rect.w <= 0 || rect.h <= 0 {
        return;
    }
    let rx = rect.x.max(0) as usize;
    let ry = rect.y.max(0) as usize;
    let x1 = (rect.x + rect.w).max(0).min(width as i32) as usize;
    let y1 = (rect.y + rect.h).max(0).min(height as i32) as usize;
    if rx >= x1 || ry >= y1 {
        return;
    }
    let cw = x1 - rx;
    let ch = y1 - ry;
    if pixels.len() < width * height || scratch.len() < cw * ch {
        return;
    }

    let r = radius as isize;
    for py in 0..ch {
        let row_off = (ry + py) * width;
        let out_off = py * cw;
        for px in 0..cw {
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            let mut count = 0u32;
            let sx0 = (px as isize - r).max(0) as usize;
            let sx1 = (px as isize + r).min((cw - 1) as isize) as usize;
            for sx in sx0..=sx1 {
                let c = pixels[row_off + rx + sx];
                sr += (c >> 16) & 0xFF;
                sg += (c >> 8) & 0xFF;
                sb += c & 0xFF;
                count += 1;
            }
            scratch[out_off + px] = ((sr / count) << 16) | ((sg / count) << 8) | (sb / count);
        }
    }

    for py in 0..ch {
        let row_off = (ry + py) * width;
        let sy0 = (py as isize - r).max(0) as usize;
        let sy1 = (py as isize + r).min((ch - 1) as isize) as usize;
        for px in 0..cw {
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            let mut count = 0u32;
            for sy in sy0..=sy1 {
                let c = scratch[sy * cw + px];
                sr += (c >> 16) & 0xFF;
                sg += (c >> 8) & 0xFF;
                sb += c & 0xFF;
                count += 1;
            }
            pixels[row_off + rx + px] = ((sr / count) << 16) | ((sg / count) << 8) | (sb / count);
        }
    }
}

/// Blurs `rect` and tints it with `tint` at opacity `alpha` (`0..=255`) for frosted panels.
#[allow(clippy::too_many_arguments)]
pub fn blur_tinted_panel(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    rect: Rect,
    radius: usize,
    tint: u32,
    alpha: u8,
    scratch: &mut [u32],
) {
    box_blur_region(pixels, width, height, rect, radius, scratch);
    let rx = rect.x.max(0) as usize;
    let ry = rect.y.max(0) as usize;
    let x1 = (rect.x + rect.w).max(0).min(width as i32) as usize;
    let y1 = (rect.y + rect.h).max(0).min(height as i32) as usize;
    if rx >= x1 || ry >= y1 || pixels.len() < width * height {
        return;
    }
    for py in ry..y1 {
        let row = &mut pixels[py * width..py * width + width];
        for px_mut in &mut row[rx..x1] {
            *px_mut = lerp_color(*px_mut, tint, alpha);
        }
    }
}

/// Checks whether local `(lx, ly)` inside `(w, h)` lies inside an `ArcSharpStyle` silhouette.
pub fn point_in_arc_sharp(lx: i32, ly: i32, w: i32, h: i32, style: ArcSharpStyle) -> bool {
    if lx < 0 || ly < 0 || lx >= w || ly >= h {
        return false;
    }
    corner_contains(lx, ly, style.top_left)
        && corner_contains(w - 1 - lx, ly, style.top_right)
        && corner_contains(w - 1 - lx, h - 1 - ly, style.bottom_right)
        && corner_contains(lx, h - 1 - ly, style.bottom_left)
}

const fn corner_contains(dx: i32, dy: i32, corner: CornerStyle) -> bool {
    match corner {
        CornerStyle::Rounded(r_u16) => {
            let r = r_u16 as i32;
            if r <= 0 || dx >= r || dy >= r {
                return true;
            }
            let cx = r - 1 - dx;
            let cy = r - 1 - dy;
            cx * cx + cy * cy <= r * r
        }
        CornerStyle::Sharp => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_and_easing_curves_are_monotonic() {
        assert_eq!(lerp_color(0x00F4_B41A, 0x00FF_D166, 0), 0x00F4_B41A);
        assert_eq!(lerp_color(0x00F4_B41A, 0x00FF_D166, 255), 0x00FF_D166);
        assert_eq!(ease_out_cubic(0), 0);
        assert_eq!(ease_out_cubic(Q16_ONE), Q16_ONE);
        assert!(ease_out_cubic(Q16_ONE / 2) > Q16_ONE / 2);
        assert_eq!(ease_in_out_quad(Q16_ONE / 2), Q16_ONE / 2);
    }

    #[test]
    fn blur_smooths_impulse_within_bounds() {
        let mut buf = [0u32; 16 * 16];
        buf[8 * 16 + 8] = 0x00FF_FFFF;
        let mut scratch = [0u32; 16 * 16];
        blur_tinted_panel(
            &mut buf,
            16,
            16,
            Rect::new(4, 4, 8, 8),
            2,
            0x001B_2336,
            64,
            &mut scratch,
        );
        assert!(buf[8 * 16 + 8] < 0x00FF_FFFF);
        assert!(buf[8 * 16 + 9] > 0);
    }

    #[test]
    fn rounded_and_sharp_corners_render_cleanly() {
        let s = ArcSharpStyle::cyberdeck_card();
        assert!(!point_in_arc_sharp(0, 0, 80, 60, s));
        assert!(point_in_arc_sharp(6, 0, 80, 60, s));
        assert!(point_in_arc_sharp(40, 30, 80, 60, s));
        assert!(point_in_arc_sharp(0, 0, 80, 60, ArcSharpStyle::sharp()));
    }
}

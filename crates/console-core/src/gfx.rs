//! Software framebuffer and the drawing primitives behind the Lua API.
//!
//! Everything here is integer-only (no trig, no floats) so results are
//! bit-identical on every target. Coordinates arriving from Lua are floored
//! before they reach these functions.

use crate::font;

/// Logical screen width in pixels.
pub const SCREEN_W: usize = 144;
/// Logical screen height in pixels.
pub const SCREEN_H: usize = 256;
/// Framebuffer length in bytes (one palette index per pixel, row-major).
pub const FB_LEN: usize = SCREEN_W * SCREEN_H;

/// Sprite sheet edge length in pixels (16x16 sprites of 8x8).
pub const SHEET_W: usize = 128;
/// Sprite sheet length in bytes.
pub const SHEET_LEN: usize = SHEET_W * SHEET_W;
/// Edge length of a single sprite in pixels.
pub const SPRITE_SIZE: i32 = 8;
/// Sprites per sheet row.
pub const SPRITES_PER_ROW: i32 = 16;

/// A screen's worth of palette indices.
pub type Framebuffer = [u8; FB_LEN];
/// A full 128x128 sprite sheet of palette indices.
pub type SpriteSheet = [u8; SHEET_LEN];

/// The fixed 16-colour Sweetie-16 palette, as RGB triples.
pub const PALETTE: [[u8; 3]; 16] = [
    [0x1a, 0x1c, 0x2c],
    [0x5d, 0x27, 0x5d],
    [0xb1, 0x3e, 0x53],
    [0xef, 0x7d, 0x57],
    [0xff, 0xcd, 0x75],
    [0xa7, 0xf0, 0x70],
    [0x38, 0xb7, 0x64],
    [0x25, 0x71, 0x79],
    [0x29, 0x36, 0x6f],
    [0x3b, 0x5d, 0xc9],
    [0x41, 0xa6, 0xf6],
    [0x73, 0xef, 0xf7],
    [0xf4, 0xf4, 0xf4],
    [0x94, 0xb0, 0xc2],
    [0x56, 0x6c, 0x86],
    [0x33, 0x3c, 0x57],
];

/// Floor a Lua float to a screen coordinate. NaN maps to 0, infinities saturate.
pub fn fl(v: f64) -> i32 {
    let f = v.floor();
    if f.is_nan() { 0 } else { f as i32 }
}

/// Floor a Lua float and mask it into the 0..=15 palette range.
pub fn col(v: f64) -> u8 {
    let f = v.floor();
    let i = if f.is_nan() { 0i64 } else { f as i64 };
    (i & 0xf) as u8
}

#[inline]
fn in_bounds(x: i32, y: i32) -> bool {
    x >= 0 && y >= 0 && (x as usize) < SCREEN_W && (y as usize) < SCREEN_H
}

/// Clear the whole screen to `c`.
pub fn cls(fb: &mut Framebuffer, c: u8) {
    fb.fill(c & 0xf);
}

/// Set one pixel; out of bounds is a no-op.
pub fn pset(fb: &mut Framebuffer, x: i32, y: i32, c: u8) {
    if in_bounds(x, y) {
        fb[y as usize * SCREEN_W + x as usize] = c & 0xf;
    }
}

/// Read one pixel; out of bounds reads as 0.
pub fn pget(fb: &Framebuffer, x: i32, y: i32) -> u8 {
    if in_bounds(x, y) {
        fb[y as usize * SCREEN_W + x as usize]
    } else {
        0
    }
}

/// Horizontal span, inclusive of both ends, clipped to the screen.
fn hline(fb: &mut Framebuffer, x0: i32, x1: i32, y: i32, c: u8) {
    if y < 0 || y as usize >= SCREEN_H {
        return;
    }
    let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let lo = lo.max(0);
    let hi = hi.min(SCREEN_W as i32 - 1);
    if lo > hi {
        return;
    }
    let row = y as usize * SCREEN_W;
    fb[row + lo as usize..=row + hi as usize].fill(c & 0xf);
}

/// Vertical span, inclusive of both ends, clipped to the screen.
fn vline(fb: &mut Framebuffer, x: i32, y0: i32, y1: i32, c: u8) {
    if x < 0 || x as usize >= SCREEN_W {
        return;
    }
    let (lo, hi) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    for y in lo.max(0)..=hi.min(SCREEN_H as i32 - 1) {
        fb[y as usize * SCREEN_W + x as usize] = c & 0xf;
    }
}

/// Bresenham line, endpoints inclusive.
pub fn line(fb: &mut Framebuffer, x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        pset(fb, x, y, c);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Rectangle outline, coordinates inclusive.
pub fn rect(fb: &mut Framebuffer, x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    let (lx, rx) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (ty, by) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    hline(fb, lx, rx, ty, c);
    hline(fb, lx, rx, by, c);
    vline(fb, lx, ty, by, c);
    vline(fb, rx, ty, by, c);
}

/// Filled rectangle, coordinates inclusive.
pub fn rectfill(fb: &mut Framebuffer, x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    let (lx, rx) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (ty, by) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    for y in ty.max(0)..=by.min(SCREEN_H as i32 - 1) {
        hline(fb, lx, rx, y, c);
    }
}

/// Midpoint circle outline.
pub fn circ(fb: &mut Framebuffer, cx: i32, cy: i32, r: i32, c: u8) {
    if r < 0 {
        return;
    }
    let (mut x, mut y) = (r, 0);
    let mut err = 1 - r;
    while x >= y {
        pset(fb, cx + x, cy + y, c);
        pset(fb, cx + y, cy + x, c);
        pset(fb, cx - y, cy + x, c);
        pset(fb, cx - x, cy + y, c);
        pset(fb, cx - x, cy - y, c);
        pset(fb, cx - y, cy - x, c);
        pset(fb, cx + y, cy - x, c);
        pset(fb, cx + x, cy - y, c);
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

/// Filled circle, same rasterisation as [`circ`] with the spans filled in.
pub fn circfill(fb: &mut Framebuffer, cx: i32, cy: i32, r: i32, c: u8) {
    if r < 0 {
        return;
    }
    let (mut x, mut y) = (r, 0);
    let mut err = 1 - r;
    while x >= y {
        hline(fb, cx - x, cx + x, cy + y, c);
        hline(fb, cx - x, cx + x, cy - y, c);
        hline(fb, cx - y, cx + y, cy + x, c);
        hline(fb, cx - y, cx + y, cy - x, c);
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

/// Draw sprite `n` (a `w`x`h` block of 8x8 sprites) at `(x, y)`.
///
/// Colour 0 is transparent. `flip` mirrors the whole block. Sprites whose
/// source pixels fall outside the sheet read as 0 (transparent).
pub fn spr(
    fb: &mut Framebuffer,
    sheet: &SpriteSheet,
    n: i32,
    x: i32,
    y: i32,
    size: (i32, i32),
    flip: (bool, bool),
) {
    let (w, h) = size;
    if w <= 0 || h <= 0 || n < 0 {
        return;
    }
    let (flip_x, flip_y) = flip;
    let base_x = (n % SPRITES_PER_ROW) * SPRITE_SIZE;
    let base_y = (n / SPRITES_PER_ROW) * SPRITE_SIZE;
    let px_w = w * SPRITE_SIZE;
    let px_h = h * SPRITE_SIZE;

    for dy in 0..px_h {
        let dest_y = y + dy;
        if dest_y < 0 || dest_y as usize >= SCREEN_H {
            continue;
        }
        let src_y = base_y + if flip_y { px_h - 1 - dy } else { dy };
        if src_y < 0 || src_y as usize >= SHEET_W {
            continue;
        }
        for dx in 0..px_w {
            let dest_x = x + dx;
            if dest_x < 0 || dest_x as usize >= SCREEN_W {
                continue;
            }
            let src_x = base_x + if flip_x { px_w - 1 - dx } else { dx };
            if src_x < 0 || src_x as usize >= SHEET_W {
                continue;
            }
            let c = sheet[src_y as usize * SHEET_W + src_x as usize];
            if c != 0 {
                fb[dest_y as usize * SCREEN_W + dest_x as usize] = c & 0xf;
            }
        }
    }
}

/// Draw `text` with the built-in 4x6 font. `\n` starts a new line at `x`.
pub fn print(fb: &mut Framebuffer, text: &str, x: i32, y: i32, c: u8) {
    let mut cx = x;
    let mut cy = y;
    for ch in text.bytes() {
        if ch == b'\n' {
            cx = x;
            cy += font::GLYPH_H;
            continue;
        }
        if let Some(bits) = font::glyph(ch) {
            for row in 0..font::GLYPH_H - 1 {
                for colx in 0..font::GLYPH_W - 1 {
                    if font::pixel(bits, colx, row) {
                        pset(fb, cx + colx, cy + row, c);
                    }
                }
            }
        }
        cx += font::GLYPH_W;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Box<Framebuffer> {
        Box::new([0u8; FB_LEN])
    }

    #[test]
    fn floor_and_mask() {
        assert_eq!(fl(3.9), 3);
        assert_eq!(fl(-0.1), -1);
        assert_eq!(fl(f64::NAN), 0);
        assert_eq!(col(17.0), 1);
        assert_eq!(col(-1.0), 15);
        assert_eq!(col(f64::NAN), 0);
    }

    #[test]
    fn pset_pget_and_clipping() {
        let mut fb = blank();
        pset(&mut fb, 5, 7, 9);
        assert_eq!(pget(&fb, 5, 7), 9);
        pset(&mut fb, -1, 0, 3);
        pset(&mut fb, 0, -1, 3);
        pset(&mut fb, SCREEN_W as i32, 0, 3);
        pset(&mut fb, 0, SCREEN_H as i32, 3);
        assert_eq!(pget(&fb, -1, 0), 0);
        assert_eq!(pget(&fb, 1000, 1000), 0);
        assert_eq!(fb.iter().filter(|&&p| p != 0).count(), 1);
    }

    #[test]
    fn rectfill_inclusive_and_clipped() {
        let mut fb = blank();
        rectfill(&mut fb, 2, 3, 4, 5, 7);
        assert_eq!(fb.iter().filter(|&&p| p == 7).count(), 9);
        cls(&mut fb, 0);
        rectfill(&mut fb, -10, -10, 1, 1, 5);
        assert_eq!(fb.iter().filter(|&&p| p == 5).count(), 4);
    }

    #[test]
    fn circ_and_circfill_stay_in_bounds() {
        let mut fb = blank();
        circ(&mut fb, 0, 0, 30, 4);
        circfill(&mut fb, (SCREEN_W as i32) - 1, (SCREEN_H as i32) - 1, 40, 6);
        assert!(fb.contains(&4));
        assert!(fb.contains(&6));
    }

    #[test]
    fn line_endpoints_inclusive() {
        let mut fb = blank();
        line(&mut fb, 1, 1, 1, 1, 3);
        assert_eq!(pget(&fb, 1, 1), 3);
        line(&mut fb, 0, 0, 10, 5, 2);
        assert_eq!(pget(&fb, 0, 0), 2);
        assert_eq!(pget(&fb, 10, 5), 2);
    }
}

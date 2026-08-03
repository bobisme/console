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

/// Tile map width in cells.
pub const MAP_W: usize = 128;
/// Tile map height in cells.
pub const MAP_H: usize = 64;
/// Tile map length in bytes (one tile id per cell, row-major).
pub const MAP_LEN: usize = MAP_W * MAP_H;

/// A screen's worth of palette indices.
pub type Framebuffer = [u8; FB_LEN];
/// One horizontal shift per scanline, already reduced to `0..SCREEN_W` (see
/// [`DrawState::set_rshift`]). All zeros = no raster displacement.
pub type ShiftTable = [u8; SCREEN_H];
/// A full 128x128 sprite sheet of palette indices.
pub type SpriteSheet = [u8; SHEET_LEN];
/// A full 128x64 tile map: one sprite index per cell. Tile 0 is the empty
/// cell — [`map`] skips it entirely rather than drawing sprite 0.
pub type TileMap = [u8; MAP_LEN];

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
///
/// This is the colour of everything that cannot carry a fill pattern: `cls`,
/// `print`, and the sprite path.
pub fn col(v: f64) -> u8 {
    let f = v.floor();
    let i = if f.is_nan() { 0i64 } else { f as i64 };
    (i & 0xf) as u8
}

/// Floor a Lua float to a **shape** colour: `c0 + c1 * 16`, masked into
/// 0..=255. The low nibble is the colour drawn where the fill pattern's bit is
/// clear, the high nibble the colour drawn where it is set (0 = "draw nothing
/// there"). With the default solid pattern the high nibble is never consulted,
/// so this is indistinguishable from [`col`] for every cart that ignores
/// `fillp`.
pub fn pat_col(v: f64) -> u8 {
    let f = v.floor();
    let i = if f.is_nan() { 0i64 } else { f as i64 };
    (i & 0xff) as u8
}

#[inline]
fn in_bounds(x: i32, y: i32) -> bool {
    x >= 0 && y >= 0 && (x as usize) < SCREEN_W && (y as usize) < SCREEN_H
}

/// The identity 16-entry palette map: colour `i` stays colour `i`.
pub const IDENTITY_PAL: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Default `palt` mask: only colour 0 is transparent in [`spr`].
const DEFAULT_PALT: u16 = 1;

/// Default fill pattern: every bit clear = solid.
const SOLID_FILLP: u16 = 0;

/// Largest `mosaic` block edge. 1 is "off"; 32 already reduces the screen to
/// 5x8 visible blocks, which is as coarse as a transition ever needs.
pub const MAX_MOSAIC: u8 = 32;

/// Edge of the [`DrawState::fillp`] pattern grid, in pixels.
pub const FILLP_SIZE: i32 = 4;

/// A resolved fill: the 4x4 pattern plus both already-remapped colours.
///
/// Built once per drawing call by [`DrawState::fill`], then sampled per pixel.
/// The pattern is anchored to **screen** space, so a shape that scrolls under a
/// moving camera shimmers through the dither — the classic PICO-8 look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fill {
    /// 4x4 bitmap, MSB = top-left, row-major. 0 = solid.
    pat: u16,
    /// Colour where the pattern bit is clear.
    c0: u8,
    /// Colour where the pattern bit is set; only meaningful when `!hole`.
    c1: u8,
    /// True when the colour argument carried no secondary nibble: set bits
    /// leave the framebuffer untouched instead of drawing `c1`.
    hole: bool,
}

impl Fill {
    /// A patternless fill of one already-remapped colour (`cls`, `print`).
    #[inline]
    fn solid(c: u8) -> Fill {
        Fill {
            pat: SOLID_FILLP,
            c0: c & 0xf,
            c1: 0,
            hole: false,
        }
    }

    #[inline]
    fn is_solid(&self) -> bool {
        self.pat == SOLID_FILLP
    }

    /// The colour for **screen** pixel `(x, y)`, or `None` where the pattern
    /// punches a hole. `x` and `y` must already be on screen (non-negative).
    #[inline]
    fn at(&self, x: i32, y: i32) -> Option<u8> {
        if self.pat == SOLID_FILLP {
            return Some(self.c0);
        }
        // MSB is the top-left cell, rows read left to right, top to bottom.
        let bit = 15 - (((y & 3) << 2) | (x & 3));
        if self.pat & (1u16 << bit) == 0 {
            Some(self.c0)
        } else if self.hole {
            None
        } else {
            Some(self.c1)
        }
    }
}

/// Persistent, PICO-8-style draw state: camera offset, clip rectangle, the two
/// palette maps and sprite transparency.
///
/// Every field survives across frames — nothing here is reset by `step()`, so a
/// cart that calls `camera()` once keeps that offset until it changes it. All
/// defaults are no-ops, so a cart that never touches this state draws exactly
/// as it did before draw state existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawState {
    /// Draw offset, subtracted from the coordinates of every drawing op.
    cam_x: i32,
    cam_y: i32,
    /// Inclusive clip rectangle in **screen** space, always clamped to the
    /// screen. Empty when `clip_x0 > clip_x1` or `clip_y0 > clip_y1`.
    clip_x0: i32,
    clip_y0: i32,
    clip_x1: i32,
    clip_y1: i32,
    /// Applied at draw time to the colour of every drawing op.
    draw_pal: [u8; 16],
    /// Applied at scanout by the host; never touches the framebuffer.
    display_pal: [u8; 16],
    /// Bit `c` set = colour `c` is transparent in [`spr`].
    palt: u16,
    /// 4x4 dither pattern for the shape primitives, MSB = top-left. 0 = solid.
    fillp: u16,
    /// End-of-frame pixelation block edge; 1 = off.
    mosaic: u8,
    /// End-of-frame per-scanline horizontal shift, each already reduced to
    /// `0..SCREEN_W` rightward pixels. All zeros = off.
    rshift: ShiftTable,
    /// How many entries of `rshift` are non-zero, so the whole pass can be
    /// skipped in O(1) for the overwhelmingly common "no raster effect" case.
    rshift_lines: u16,
}

impl Default for DrawState {
    fn default() -> Self {
        DrawState {
            cam_x: 0,
            cam_y: 0,
            clip_x0: 0,
            clip_y0: 0,
            clip_x1: SCREEN_W as i32 - 1,
            clip_y1: SCREEN_H as i32 - 1,
            draw_pal: IDENTITY_PAL,
            display_pal: IDENTITY_PAL,
            palt: DEFAULT_PALT,
            fillp: SOLID_FILLP,
            mosaic: 1,
            rshift: [0; SCREEN_H],
            rshift_lines: 0,
        }
    }
}

impl DrawState {
    /// Fresh state: no camera, full-screen clip, identity palettes, colour 0
    /// transparent.
    pub fn new() -> DrawState {
        DrawState::default()
    }

    /// `camera(x, y)`: subsequent draws are offset by `-(x, y)`.
    pub fn set_camera(&mut self, x: i32, y: i32) {
        self.cam_x = x;
        self.cam_y = y;
    }

    /// The current camera offset.
    pub fn camera(&self) -> (i32, i32) {
        (self.cam_x, self.cam_y)
    }

    /// `clip(x, y, w, h)`: screen-space clip rectangle, clamped to the screen.
    /// A non-positive width or height yields an empty (draws-nothing) clip.
    pub fn set_clip(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            // Deliberately empty: x0 > x1 makes every span test fail.
            self.clip_x0 = 0;
            self.clip_y0 = 0;
            self.clip_x1 = -1;
            self.clip_y1 = -1;
            return;
        }
        let x1 = x.saturating_add(w - 1);
        let y1 = y.saturating_add(h - 1);
        self.clip_x0 = x.max(0);
        self.clip_y0 = y.max(0);
        self.clip_x1 = x1.min(SCREEN_W as i32 - 1);
        self.clip_y1 = y1.min(SCREEN_H as i32 - 1);
    }

    /// `clip()`: back to the whole screen.
    pub fn reset_clip(&mut self) {
        self.clip_x0 = 0;
        self.clip_y0 = 0;
        self.clip_x1 = SCREEN_W as i32 - 1;
        self.clip_y1 = SCREEN_H as i32 - 1;
    }

    /// The clip rectangle as inclusive `(x0, y0, x1, y1)` screen coordinates.
    pub fn clip(&self) -> (i32, i32, i32, i32) {
        (self.clip_x0, self.clip_y0, self.clip_x1, self.clip_y1)
    }

    #[inline]
    fn clip_is_empty(&self) -> bool {
        self.clip_x0 > self.clip_x1 || self.clip_y0 > self.clip_y1
    }

    #[inline]
    fn clip_is_full(&self) -> bool {
        self.clip_x0 == 0
            && self.clip_y0 == 0
            && self.clip_x1 == SCREEN_W as i32 - 1
            && self.clip_y1 == SCREEN_H as i32 - 1
    }

    /// `pal(c0, c1)`: draw-palette remap, applied when pixels are written.
    pub fn set_draw_pal(&mut self, from: u8, to: u8) {
        self.draw_pal[(from & 0xf) as usize] = to & 0xf;
    }

    /// `pal(c0, c1, 1)`: display-palette remap, applied at scanout only.
    pub fn set_display_pal(&mut self, from: u8, to: u8) {
        self.display_pal[(from & 0xf) as usize] = to & 0xf;
    }

    /// `pal()`: reset both palette maps **and** `palt`.
    pub fn reset_pal(&mut self) {
        self.draw_pal = IDENTITY_PAL;
        self.display_pal = IDENTITY_PAL;
        self.palt = DEFAULT_PALT;
    }

    /// The draw palette: index -> index, applied at draw time.
    pub fn draw_palette(&self) -> &[u8; 16] {
        &self.draw_pal
    }

    /// The display palette: index -> index, applied by the host when the
    /// framebuffer is converted to RGB. The framebuffer itself is untouched.
    pub fn display_palette(&self) -> &[u8; 16] {
        &self.display_pal
    }

    /// `palt(c, flag)`: mark colour `c` transparent (or not) in [`spr`].
    pub fn set_palt(&mut self, c: u8, transparent: bool) {
        let bit = 1u16 << (c & 0xf);
        if transparent {
            self.palt |= bit;
        } else {
            self.palt &= !bit;
        }
    }

    /// `palt()`: back to "only colour 0 is transparent".
    pub fn reset_palt(&mut self) {
        self.palt = DEFAULT_PALT;
    }

    /// Transparency bitmask; bit `c` set = colour `c` is transparent.
    pub fn palt_mask(&self) -> u16 {
        self.palt
    }

    /// `fillp(p)`: the 4x4 dither pattern used by the shape primitives.
    /// Bit 15 is the top-left cell and the rows read left to right, top to
    /// bottom. `p = 0` is solid.
    pub fn set_fillp(&mut self, p: u16) {
        self.fillp = p;
    }

    /// `fillp()`: back to a solid fill.
    pub fn reset_fillp(&mut self) {
        self.fillp = SOLID_FILLP;
    }

    /// The current fill pattern. 0 = solid (the default).
    pub fn fillp(&self) -> u16 {
        self.fillp
    }

    /// `mosaic(f)`: end-of-frame pixelation, `f` clamped to `1..=`[`MAX_MOSAIC`].
    /// Anything below 1 (including a bare `mosaic()`) means "off".
    pub fn set_mosaic(&mut self, f: i32) {
        self.mosaic = f.clamp(1, MAX_MOSAIC as i32) as u8;
    }

    /// The mosaic block edge in pixels. 1 = off (the default).
    pub fn mosaic(&self) -> u8 {
        self.mosaic
    }

    /// `rshift(y, dx)`: displace scanline `y` by `dx` pixels at end of frame,
    /// positive = right, **wrapping** around the 144-pixel line.
    ///
    /// `dx` is reduced modulo [`SCREEN_W`] with a Euclidean remainder, so every
    /// argument is legal and `dx`, `dx + 144` and `dx - 144` are the same
    /// shift: `-1` is stored as 143 (a one-pixel left shift *is* a 143-pixel
    /// right shift on a wrapping line). `y` outside `0..SCREEN_H` is a no-op,
    /// exactly like a `pset` off the bottom of the screen.
    pub fn set_rshift(&mut self, y: i32, dx: i32) {
        if y < 0 || y as usize >= SCREEN_H {
            return;
        }
        let dx = dx.rem_euclid(SCREEN_W as i32) as u8;
        let slot = &mut self.rshift[y as usize];
        match (*slot == 0, dx == 0) {
            (true, false) => self.rshift_lines += 1,
            (false, true) => self.rshift_lines -= 1,
            _ => {}
        }
        *slot = dx;
    }

    /// `rshift()`: clear every scanline shift back to 0.
    pub fn reset_rshift(&mut self) {
        self.rshift = [0; SCREEN_H];
        self.rshift_lines = 0;
    }

    /// The shift applied to scanline `y`, as stored: `0..SCREEN_W` rightward
    /// pixels. Off-screen rows read as 0.
    pub fn rshift(&self, y: i32) -> u8 {
        if y < 0 || y as usize >= SCREEN_H {
            return 0;
        }
        self.rshift[y as usize]
    }

    /// The whole shift table, for the once-per-frame raster pass.
    pub fn rshift_table(&self) -> &ShiftTable {
        &self.rshift
    }

    /// Is any scanline shifted? False (the default) lets the frame pipeline
    /// skip the raster pass entirely.
    pub fn rshift_active(&self) -> bool {
        self.rshift_lines != 0
    }

    /// Resolve a shape colour argument (`c0 + c1 * 16`) against the current
    /// fill pattern and draw palette. Both nibbles are remapped by `pal`.
    #[inline]
    fn fill(&self, c: u8) -> Fill {
        let hi = c >> 4;
        Fill {
            pat: self.fillp,
            c0: self.remap(c),
            c1: self.remap(hi),
            hole: hi == 0,
        }
    }

    /// Is this screen-space pixel inside the clip rectangle? The clip is always
    /// clamped to the screen, so this doubles as the bounds check.
    #[inline]
    pub fn visible(&self, x: i32, y: i32) -> bool {
        x >= self.clip_x0 && x <= self.clip_x1 && y >= self.clip_y0 && y <= self.clip_y1
    }

    #[inline]
    fn remap(&self, c: u8) -> u8 {
        self.draw_pal[(c & 0xf) as usize]
    }

    #[inline]
    fn transparent(&self, c: u8) -> bool {
        self.palt & (1u16 << (c & 0xf)) != 0
    }

    /// World -> screen for a horizontal coordinate.
    #[inline]
    fn sx(&self, x: i32) -> i32 {
        x.saturating_sub(self.cam_x)
    }

    /// World -> screen for a vertical coordinate.
    #[inline]
    fn sy(&self, y: i32) -> i32 {
        y.saturating_sub(self.cam_y)
    }
}

/// Write one resolved fill at an already-camera-adjusted position.
#[inline]
fn put(fb: &mut Framebuffer, ds: &DrawState, x: i32, y: i32, f: &Fill) {
    if !ds.visible(x, y) {
        return;
    }
    if let Some(c) = f.at(x, y) {
        fb[y as usize * SCREEN_W + x as usize] = c;
    }
}

/// Clear the screen to `c`.
///
/// `cls` ignores the camera and the draw palette (it writes `c` literally) but
/// **respects the clip rectangle**, so it doubles as "clear this window".
pub fn cls(fb: &mut Framebuffer, ds: &DrawState, c: u8) {
    let c = c & 0xf;
    if ds.clip_is_full() {
        fb.fill(c);
        return;
    }
    if ds.clip_is_empty() {
        return;
    }
    let (x0, x1) = (ds.clip_x0 as usize, ds.clip_x1 as usize);
    for y in ds.clip_y0..=ds.clip_y1 {
        let row = y as usize * SCREEN_W;
        fb[row + x0..=row + x1].fill(c);
    }
}

/// Set one pixel; outside the screen or the clip rectangle is a no-op.
///
/// Like every shape primitive this goes through the fill pattern, so a `pset`
/// on a pattern hole draws nothing at all.
pub fn pset(fb: &mut Framebuffer, ds: &DrawState, x: i32, y: i32, c: u8) {
    put(fb, ds, ds.sx(x), ds.sy(y), &ds.fill(c));
}

/// Read one pixel; out of bounds reads as 0.
pub fn pget(fb: &Framebuffer, x: i32, y: i32) -> u8 {
    if in_bounds(x, y) {
        fb[y as usize * SCREEN_W + x as usize]
    } else {
        0
    }
}

/// Horizontal span, inclusive of both ends, in screen space, clipped to the
/// clip rectangle. The fill must already be resolved against the draw palette.
fn hline(fb: &mut Framebuffer, ds: &DrawState, x0: i32, x1: i32, y: i32, f: &Fill) {
    if y < ds.clip_y0 || y > ds.clip_y1 {
        return;
    }
    let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let lo = lo.max(ds.clip_x0);
    let hi = hi.min(ds.clip_x1);
    if lo > hi {
        return;
    }
    let row = y as usize * SCREEN_W;
    if f.is_solid() {
        fb[row + lo as usize..=row + hi as usize].fill(f.c0);
        return;
    }
    for x in lo..=hi {
        if let Some(c) = f.at(x, y) {
            fb[row + x as usize] = c;
        }
    }
}

/// Vertical span, inclusive of both ends, in screen space, clipped to the clip
/// rectangle. The fill must already be resolved against the draw palette.
fn vline(fb: &mut Framebuffer, ds: &DrawState, x: i32, y0: i32, y1: i32, f: &Fill) {
    if x < ds.clip_x0 || x > ds.clip_x1 {
        return;
    }
    let (lo, hi) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    let lo = lo.max(ds.clip_y0);
    let hi = hi.min(ds.clip_y1);
    if lo > hi {
        return;
    }
    for y in lo..=hi {
        if let Some(c) = f.at(x, y) {
            fb[y as usize * SCREEN_W + x as usize] = c;
        }
    }
}

/// Bresenham line, endpoints inclusive.
pub fn line(fb: &mut Framebuffer, ds: &DrawState, x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    let c = ds.fill(c);
    let (x0, y0) = (ds.sx(x0), ds.sy(y0));
    let (x1, y1) = (ds.sx(x1), ds.sy(y1));
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        put(fb, ds, x, y, &c);
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
pub fn rect(fb: &mut Framebuffer, ds: &DrawState, x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    let c = ds.fill(c);
    let (x0, y0) = (ds.sx(x0), ds.sy(y0));
    let (x1, y1) = (ds.sx(x1), ds.sy(y1));
    let (lx, rx) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (ty, by) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    hline(fb, ds, lx, rx, ty, &c);
    hline(fb, ds, lx, rx, by, &c);
    vline(fb, ds, lx, ty, by, &c);
    vline(fb, ds, rx, ty, by, &c);
}

/// Filled rectangle, coordinates inclusive.
pub fn rectfill(fb: &mut Framebuffer, ds: &DrawState, x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
    let c = ds.fill(c);
    let (x0, y0) = (ds.sx(x0), ds.sy(y0));
    let (x1, y1) = (ds.sx(x1), ds.sy(y1));
    let (lx, rx) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (ty, by) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    let (top, bot) = (ty.max(ds.clip_y0), by.min(ds.clip_y1));
    if top > bot {
        return;
    }
    for y in top..=bot {
        hline(fb, ds, lx, rx, y, &c);
    }
}

/// Midpoint circle outline.
pub fn circ(fb: &mut Framebuffer, ds: &DrawState, cx: i32, cy: i32, r: i32, c: u8) {
    if r < 0 {
        return;
    }
    let c = ds.fill(c);
    let (cx, cy) = (ds.sx(cx), ds.sy(cy));
    let (mut x, mut y) = (r, 0);
    let mut err = 1 - r;
    while x >= y {
        put(fb, ds, cx + x, cy + y, &c);
        put(fb, ds, cx + y, cy + x, &c);
        put(fb, ds, cx - y, cy + x, &c);
        put(fb, ds, cx - x, cy + y, &c);
        put(fb, ds, cx - x, cy - y, &c);
        put(fb, ds, cx - y, cy - x, &c);
        put(fb, ds, cx + y, cy - x, &c);
        put(fb, ds, cx + x, cy - y, &c);
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
pub fn circfill(fb: &mut Framebuffer, ds: &DrawState, cx: i32, cy: i32, r: i32, c: u8) {
    if r < 0 {
        return;
    }
    let c = ds.fill(c);
    let (cx, cy) = (ds.sx(cx), ds.sy(cy));
    let (mut x, mut y) = (r, 0);
    let mut err = 1 - r;
    while x >= y {
        hline(fb, ds, cx - x, cx + x, cy + y, &c);
        hline(fb, ds, cx - x, cx + x, cy - y, &c);
        hline(fb, ds, cx - y, cx + y, cy + x, &c);
        hline(fb, ds, cx - y, cx + y, cy - x, &c);
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

/// Top-left pixel of sprite `n` on the sheet.
#[inline]
fn sheet_origin(n: i32) -> (i32, i32) {
    (
        (n % SPRITES_PER_ROW) * SPRITE_SIZE,
        (n / SPRITES_PER_ROW) * SPRITE_SIZE,
    )
}

/// Blit a `px_w`x`px_h` rectangle of sheet pixels to **already camera-adjusted**
/// screen coordinates.
///
/// The single low-level sprite path: [`spr`] and [`map`] both funnel through it,
/// so they agree on clipping, `palt` and the draw palette by construction. It
/// allocates nothing and touches no state beyond the framebuffer.
///
/// Transparency is decided by `palt` on the **source** colour, before the draw
/// palette remaps it (PICO-8 semantics). Source pixels outside the sheet are
/// skipped.
#[allow(clippy::too_many_arguments)]
#[inline]
fn blit(
    fb: &mut Framebuffer,
    ds: &DrawState,
    sheet: &SpriteSheet,
    src: (i32, i32),
    dest: (i32, i32),
    px: (i32, i32),
    flip: (bool, bool),
) {
    let (base_x, base_y) = src;
    let (x, y) = dest;
    let (px_w, px_h) = px;
    let (flip_x, flip_y) = flip;

    for dy in 0..px_h {
        let dest_y = y + dy;
        if dest_y < ds.clip_y0 || dest_y > ds.clip_y1 {
            continue;
        }
        let src_y = base_y + if flip_y { px_h - 1 - dy } else { dy };
        if src_y < 0 || src_y as usize >= SHEET_W {
            continue;
        }
        for dx in 0..px_w {
            let dest_x = x + dx;
            if dest_x < ds.clip_x0 || dest_x > ds.clip_x1 {
                continue;
            }
            let src_x = base_x + if flip_x { px_w - 1 - dx } else { dx };
            if src_x < 0 || src_x as usize >= SHEET_W {
                continue;
            }
            let c = sheet[src_y as usize * SHEET_W + src_x as usize];
            if !ds.transparent(c) {
                fb[dest_y as usize * SCREEN_W + dest_x as usize] = ds.remap(c);
            }
        }
    }
}

/// Draw sprite `n` (a `w`x`h` block of 8x8 sprites) at `(x, y)`.
///
/// Transparency is decided by `palt` on the sprite's **source** colour, before
/// the draw palette remaps it (PICO-8 semantics). By default only colour 0 is
/// transparent. `flip` mirrors the whole block. Sprites whose source pixels
/// fall outside the sheet read as 0.
// Positional by design: this mirrors Lua's `spr(n, x, y, w, h, fx, fy)`.
#[allow(clippy::too_many_arguments)]
pub fn spr(
    fb: &mut Framebuffer,
    ds: &DrawState,
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
    blit(
        fb,
        ds,
        sheet,
        sheet_origin(n),
        (ds.sx(x), ds.sy(y)),
        (w * SPRITE_SIZE, h * SPRITE_SIZE),
        flip,
    );
}

/// Draw an arbitrary `px`-sized rectangle of sheet pixels, whose top-left is
/// the sheet pixel `src`, with its top-left at the world position `dest`.
///
/// This is [`spr`] with the source rectangle addressed in **pixels** instead of
/// by sprite id: same `blit`, so the camera, the clip rect, `palt` and the draw
/// palette all behave identically by construction (and `fillp`, a shape effect,
/// is exempt exactly as it is for `spr`). It exists for `aspr`, whose source
/// rect comes from `AnimDef::resolve_frame` and need not be sprite-id aligned.
pub fn spr_rect(
    fb: &mut Framebuffer,
    ds: &DrawState,
    sheet: &SpriteSheet,
    src: (i32, i32),
    dest: (i32, i32),
    px: (i32, i32),
    flip: (bool, bool),
) {
    if px.0 <= 0 || px.1 <= 0 {
        return;
    }
    blit(fb, ds, sheet, src, (ds.sx(dest.0), ds.sy(dest.1)), px, flip);
}

/// Fractional bits in [`sspr`]'s source-stepping fixed point (u32.16).
const SSPR_SHIFT: u32 = 16;

/// Blit the sprite-sheet rectangle `src = (sx, sy, sw, sh)` into the
/// destination rectangle `dest = (dx, dy, dw, dh)`, scaling with
/// nearest-neighbour sampling.
///
/// The stepping is fixed point (`sw / dw` as a u32.16 fraction) so the pixel
/// loop is integer-only and bit-identical everywhere. When `dw == sw` and
/// `dh == sh` the step is exactly 1.0 and the result is byte-identical to the
/// equivalent [`spr`] call, flips included.
///
/// Everything the sprite path respects, this respects: the camera offsets
/// `dest`, the clip rectangle bounds it, `palt` decides transparency on the
/// **source** colour and the draw palette remaps what lands in the
/// framebuffer. `fillp` does not apply — it is a shape effect.
///
/// A degenerate rectangle — zero or negative `sw`/`sh`/`dw`/`dh` — draws
/// nothing. Negative sizes do **not** mirror (PICO-8 does not either); use the
/// `flip` flags. Source pixels outside the 128x128 sheet are skipped.
pub fn sspr(
    fb: &mut Framebuffer,
    ds: &DrawState,
    sheet: &SpriteSheet,
    src: (i32, i32, i32, i32),
    dest: (i32, i32, i32, i32),
    flip: (bool, bool),
) {
    let (src_x, src_y, sw, sh) = src;
    let (dst_x, dst_y, dw, dh) = dest;
    if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 {
        return;
    }
    // Range maths in i64: Lua can hand us any i32 for either rectangle.
    let (sw, sh) = (i64::from(sw), i64::from(sh));
    let (dw, dh) = (i64::from(dw), i64::from(dh));
    let x0 = i64::from(ds.sx(dst_x));
    let y0 = i64::from(ds.sy(dst_y));
    // `j * step_y < sh << SHIFT` for every `j < dh`, so nothing here overflows.
    let step_x = (sw << SSPR_SHIFT) / dw;
    let step_y = (sh << SSPR_SHIFT) / dh;

    // Destination columns and rows that survive the clip rectangle. The clip is
    // always clamped to the screen, so these bounds double as the screen test.
    let i0 = (i64::from(ds.clip_x0) - x0).max(0);
    let i1 = (i64::from(ds.clip_x1) - x0 + 1).min(dw);
    let j0 = (i64::from(ds.clip_y0) - y0).max(0);
    let j1 = (i64::from(ds.clip_y1) - y0 + 1).min(dh);
    if i0 >= i1 || j0 >= j1 {
        return;
    }
    let (flip_x, flip_y) = flip;

    for j in j0..j1 {
        let t = (j * step_y) >> SSPR_SHIFT;
        let sy = i64::from(src_y) + if flip_y { sh - 1 - t } else { t };
        if sy < 0 || sy >= SHEET_W as i64 {
            continue;
        }
        let src_row = sy as usize * SHEET_W;
        let dest_row = (y0 + j) as usize * SCREEN_W;
        for i in i0..i1 {
            let s = (i * step_x) >> SSPR_SHIFT;
            let sx = i64::from(src_x) + if flip_x { sw - 1 - s } else { s };
            if sx < 0 || sx >= SHEET_W as i64 {
                continue;
            }
            let c = sheet[src_row + sx as usize];
            if !ds.transparent(c) {
                fb[dest_row + (x0 + i) as usize] = ds.remap(c);
            }
        }
    }
}

/// Draw a `cel_w`x`cel_h` block of map cells starting at map cell `cel` to the
/// world position `dest`.
///
/// Every cell is an ordinary 8x8 sprite blit through the same path as [`spr`],
/// so the camera offsets `dest`, the clip rectangle bounds the result, `palt`
/// decides per-pixel transparency and the draw palette remaps what lands in the
/// framebuffer. **Tile 0 is skipped entirely** (PICO-8's empty cell) — it is not
/// drawn as sprite 0, which is what makes an unset map cell free and invisible.
/// Cells outside the 128x64 map are simply not drawn: no wrap, no error.
pub fn map(
    fb: &mut Framebuffer,
    ds: &DrawState,
    sheet: &SpriteSheet,
    tiles: &TileMap,
    cel: (i32, i32),
    dest: (i32, i32),
    size: (i32, i32),
) {
    if ds.clip_is_empty() {
        return;
    }
    // Range maths in i64: Lua can hand us any i32, including ones that would
    // overflow when scaled to pixels.
    let (cel_x, cel_y) = (i64::from(cel.0), i64::from(cel.1));
    let (cel_w, cel_h) = (i64::from(size.0), i64::from(size.1));
    if cel_w <= 0 || cel_h <= 0 {
        return;
    }
    // Clamp the cel block to the cells that actually exist, so the loops are
    // bounded by the map and the pixel arithmetic below cannot overflow.
    let i0 = (-cel_x).max(0);
    let i1 = (MAP_W as i64 - cel_x).min(cel_w);
    let j0 = (-cel_y).max(0);
    let j1 = (MAP_H as i64 - cel_y).min(cel_h);
    if i0 >= i1 || j0 >= j1 {
        return;
    }

    let step = i64::from(SPRITE_SIZE);
    let sx = i64::from(ds.sx(dest.0));
    let sy = i64::from(ds.sy(dest.1));
    let (clip_x0, clip_y0) = (i64::from(ds.clip_x0), i64::from(ds.clip_y0));
    let (clip_x1, clip_y1) = (i64::from(ds.clip_x1), i64::from(ds.clip_y1));

    for j in j0..j1 {
        let dest_y = sy + j * step;
        // Whole tile row outside the clip window: skip its cells wholesale.
        if dest_y + step - 1 < clip_y0 || dest_y > clip_y1 {
            continue;
        }
        let row = (cel_y + j) as usize * MAP_W;
        for i in i0..i1 {
            let t = tiles[row + (cel_x + i) as usize];
            if t == 0 {
                continue; // the empty cell
            }
            let dest_x = sx + i * step;
            if dest_x + step - 1 < clip_x0 || dest_x > clip_x1 {
                continue;
            }
            // Both coordinates now intersect the clip rect, which is always
            // clamped to the screen, so these casts are in range.
            blit(
                fb,
                ds,
                sheet,
                sheet_origin(i32::from(t)),
                (dest_x as i32, dest_y as i32),
                (SPRITE_SIZE, SPRITE_SIZE),
                (false, false),
            );
        }
    }
}

/// Draw `text` with the built-in 4x6 font. `\n` starts a new line at `x`.
///
/// Text is never dithered: `fillp` applies to the shape primitives only.
pub fn print(fb: &mut Framebuffer, ds: &DrawState, text: &str, x: i32, y: i32, c: u8) {
    let c = Fill::solid(ds.remap(c));
    let (x, y) = (ds.sx(x), ds.sy(y));
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
                        put(fb, ds, cx + colx, cy + row, &c);
                    }
                }
            }
        }
        cx += font::GLYPH_W;
    }
}

/// Pixelate the framebuffer in place: every `f`x`f` block becomes its top-left
/// pixel's colour. `f <= 1` is a no-op.
///
/// This is `mosaic(f)`, run once at the end of every frame on the finished
/// frame — it is a **framebuffer** effect, not a scanout one, so `screen_text`,
/// PNG screenshots and framebuffer goldens all show the blocks. Blocks are
/// anchored at screen (0, 0), ignore the camera and the clip rectangle, and a
/// factor that does not divide 144 or 256 leaves a narrower block at the right
/// and bottom edges.
///
/// Top-left, not average: the framebuffer holds palette indices, and averaging
/// indices is meaningless.
pub fn apply_mosaic(fb: &mut Framebuffer, f: u8) {
    let f = f.clamp(1, MAX_MOSAIC) as usize;
    if f <= 1 {
        return;
    }
    let mut by = 0;
    while by < SCREEN_H {
        // Flatten the block row's top line...
        let row = by * SCREEN_W;
        let mut bx = 0;
        while bx < SCREEN_W {
            let w = f.min(SCREEN_W - bx);
            let c = fb[row + bx];
            fb[row + bx..row + bx + w].fill(c);
            bx += w;
        }
        // ...then copy it down over the rest of the block.
        let h = f.min(SCREEN_H - by);
        for j in 1..h {
            let (top, rest) = fb.split_at_mut((by + j) * SCREEN_W);
            rest[..SCREEN_W].copy_from_slice(&top[row..row + SCREEN_W]);
        }
        by += f;
    }
}

/// Displace each scanline of the framebuffer horizontally, in place, wrapping
/// around the 144-pixel line. `shifts[y]` is the rightward shift for row `y`,
/// already reduced to `0..SCREEN_W`; an all-zero table is a no-op.
///
/// This is `rshift(y, dx)` — the HDMA-style raster trick — run once at the end
/// of every frame, **after** [`apply_mosaic`]: the frame is pixelated first and
/// the finished pixels are then displaced, so a shift never slices a mosaic
/// block in half. Like mosaic it is a **framebuffer** effect, so `screen_text`,
/// PNG screenshots and framebuffer goldens all show it, and it is computed from
/// the pristine draw buffer every frame, so it never compounds.
///
/// Wrap, not clip: pixels pushed off one edge reappear at the other, which is
/// what makes a sine sweep read as water or heat haze instead of a torn edge.
/// No allocation — each row is rotated in place.
pub fn apply_rshift(fb: &mut Framebuffer, shifts: &ShiftTable) {
    for (y, &dx) in shifts.iter().enumerate() {
        let dx = dx as usize % SCREEN_W;
        if dx == 0 {
            continue;
        }
        let row = y * SCREEN_W;
        fb[row..row + SCREEN_W].rotate_right(dx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank() -> Box<Framebuffer> {
        Box::new([0u8; FB_LEN])
    }

    fn ds() -> DrawState {
        DrawState::new()
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
        let d = ds();
        let mut fb = blank();
        pset(&mut fb, &d, 5, 7, 9);
        assert_eq!(pget(&fb, 5, 7), 9);
        pset(&mut fb, &d, -1, 0, 3);
        pset(&mut fb, &d, 0, -1, 3);
        pset(&mut fb, &d, SCREEN_W as i32, 0, 3);
        pset(&mut fb, &d, 0, SCREEN_H as i32, 3);
        assert_eq!(pget(&fb, -1, 0), 0);
        assert_eq!(pget(&fb, 1000, 1000), 0);
        assert_eq!(fb.iter().filter(|&&p| p != 0).count(), 1);
    }

    #[test]
    fn rectfill_inclusive_and_clipped() {
        let d = ds();
        let mut fb = blank();
        rectfill(&mut fb, &d, 2, 3, 4, 5, 7);
        assert_eq!(fb.iter().filter(|&&p| p == 7).count(), 9);
        cls(&mut fb, &d, 0);
        rectfill(&mut fb, &d, -10, -10, 1, 1, 5);
        assert_eq!(fb.iter().filter(|&&p| p == 5).count(), 4);
    }

    #[test]
    fn circ_and_circfill_stay_in_bounds() {
        let d = ds();
        let mut fb = blank();
        circ(&mut fb, &d, 0, 0, 30, 4);
        circfill(
            &mut fb,
            &d,
            (SCREEN_W as i32) - 1,
            (SCREEN_H as i32) - 1,
            40,
            6,
        );
        assert!(fb.contains(&4));
        assert!(fb.contains(&6));
    }

    #[test]
    fn line_endpoints_inclusive() {
        let d = ds();
        let mut fb = blank();
        line(&mut fb, &d, 1, 1, 1, 1, 3);
        assert_eq!(pget(&fb, 1, 1), 3);
        line(&mut fb, &d, 0, 0, 10, 5, 2);
        assert_eq!(pget(&fb, 0, 0), 2);
        assert_eq!(pget(&fb, 10, 5), 2);
    }

    #[test]
    fn draw_state_defaults_are_no_ops() {
        let d = ds();
        assert_eq!(d.camera(), (0, 0));
        assert_eq!(d.clip(), (0, 0, SCREEN_W as i32 - 1, SCREEN_H as i32 - 1));
        assert_eq!(d.draw_palette(), &IDENTITY_PAL);
        assert_eq!(d.display_palette(), &IDENTITY_PAL);
        assert!(d.transparent(0));
        assert!(!d.transparent(1));
    }

    #[test]
    fn clip_clamps_and_can_be_empty() {
        let mut d = ds();
        d.set_clip(-10, -10, 20, 20);
        assert_eq!(d.clip(), (0, 0, 9, 9));
        d.set_clip(140, 250, 1000, 1000);
        assert_eq!(d.clip(), (140, 250, 143, 255));
        d.set_clip(5, 5, 0, 10);
        assert!(d.clip_is_empty());
        d.set_clip(5, 5, -4, -4);
        assert!(d.clip_is_empty());
        // A rect entirely off-screen clamps to an empty region, not a wrap.
        d.set_clip(200, 300, 10, 10);
        assert!(d.clip_is_empty());
        d.reset_clip();
        assert!(d.clip_is_full());
    }

    #[test]
    fn cls_respects_the_clip_rect() {
        let mut d = ds();
        let mut fb = blank();
        d.set_clip(10, 20, 4, 3);
        cls(&mut fb, &d, 7);
        assert_eq!(fb.iter().filter(|&&p| p == 7).count(), 12);
        assert_eq!(pget(&fb, 10, 20), 7);
        assert_eq!(pget(&fb, 13, 22), 7);
        assert_eq!(pget(&fb, 14, 22), 0);
        // An empty clip clears nothing at all.
        let mut fb = blank();
        d.set_clip(0, 0, 0, 0);
        cls(&mut fb, &d, 7);
        assert!(fb.iter().all(|&p| p == 0));
    }
}

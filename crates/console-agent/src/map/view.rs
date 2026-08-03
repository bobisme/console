//! Map inspection tools (SPEC.md "Tile map authoring" plus this bone's
//! agent-tooling addition): `render` (PNG), `dump` (hex rows) and `lint`
//! (JSON quality numbers), mirroring `sprite::view`'s inspection-tools shape
//! one level up (cells instead of pixels, tile ids instead of palette
//! indices).
//!
//! `render` reuses `sprite::view`'s canvas/checkerboard/pixel primitives
//! directly (bumped to `pub(crate)` there for exactly this purpose) rather
//! than re-implementing them: a map cell is nothing but an 8x8 sprite blit,
//! so `map render`'s output for a non-empty cell is pixel-identical to
//! `sprite render <tx,ty,8,8>` of that cell's tile. The one map-specific
//! rule — tile 0 skips the cell entirely rather than drawing sprite 0's
//! actual pixels — is applied here, before `sprite::view::draw_frame` is
//! ever called for that cell, matching `console_core::gfx::map`'s own
//! per-cell tile-0 check.

use std::collections::BTreeMap;

use console_core::{Cart, MAP_H, MAP_W, SHEET_W, SPRITE_SIZE, SpriteSheet, TileMap};
use serde_json::{Value, json};

use crate::sprite::view::{self, DEFAULT_ZOOM, Image};

/// How many `tile_counts` entries `lint` reports, most-referenced first.
const TOP_N: usize = 16;

/// Flags shared by `map render` and the (future) `map_render` RPC verb.
#[derive(Debug, Clone, Copy)]
pub struct MapRenderOpts {
    pub zoom: u32,
    pub grid: bool,
    pub ids: bool,
}

impl Default for MapRenderOpts {
    fn default() -> MapRenderOpts {
        MapRenderOpts {
            zoom: DEFAULT_ZOOM,
            grid: false,
            ids: false,
        }
    }
}

/// The pixel-space rect on the 128x128 sheet for tile id `t`: sprite `t`'s
/// own rect, same convention as sprite frame 0 (`(t%16*8, t//16*8)`).
fn tile_rect(t: u8) -> (u32, u32, u32, u32) {
    let per_row = (SHEET_W / SPRITE_SIZE as usize) as u32;
    let t = u32::from(t);
    (
        (t % per_row) * SPRITE_SIZE as u32,
        (t / per_row) * SPRITE_SIZE as u32,
        SPRITE_SIZE as u32,
        SPRITE_SIZE as u32,
    )
}

/// `map render` — a `region` of the map, one 8x8 sprite blit per non-empty
/// cell, on the same dark checkerboard the sprite tools use for
/// transparency. Tile 0 cells are skipped entirely (left as checkerboard),
/// exactly as `map()` skips them at runtime; within a non-empty cell, the
/// sprite's own color-0 pixels are still individually transparent (the
/// ordinary `spr()` pixel path), so a tile whose art has "holes" shows the
/// checkerboard through those holes too.
pub fn render(
    cart: &Cart,
    region: (u32, u32, u32, u32),
    opts: &MapRenderOpts,
) -> Result<Image, String> {
    let zoom = view::check_zoom(opts.zoom)?;
    let (cx, cy, cw, ch) = region;
    super::validate_region(cx, cy, cw, ch)?;
    let extra = u32::from(opts.grid);
    let (pw, ph) = (cw * 8 * zoom + extra, ch * 8 * zoom + extra);
    let mut canvas = view::Canvas::new(pw, ph, zoom);
    let tiles = cart.map();

    for j in 0..ch {
        for i in 0..cw {
            let t = tiles[((cy + j) as usize) * MAP_W + (cx + i) as usize];
            if t == 0 {
                continue;
            }
            let frame = view::read_rect(cart, tile_rect(t));
            view::draw_frame(&mut canvas, &frame, (i * 8 * zoom, j * 8 * zoom), zoom);
        }
    }
    if opts.grid {
        let cell = view::Rect {
            x: 0,
            y: 0,
            w: cw * 8 * zoom,
            h: ch * 8 * zoom,
        };
        view::draw_grid(&mut canvas, cell, zoom);
    }
    if opts.ids {
        draw_cell_ids(&mut canvas, tiles, region, zoom);
    }
    Ok(canvas.finish(1))
}

/// Overlay each non-empty cell's tile id as two hex-digit glyphs (reusing
/// the sprite tools' 3x5 `--indices` glyph font), scaled with `zoom` so the
/// label stays legible without ever outgrowing its 8x8 cell, and inked by
/// the same background-luminance rule `--indices` uses.
fn draw_cell_ids(
    canvas: &mut view::Canvas,
    tiles: &TileMap,
    region: (u32, u32, u32, u32),
    zoom: u32,
) {
    let (cx, cy, cw, ch) = region;
    let cell_px = 8 * zoom;
    // Two 3-wide glyphs plus a 1-unit gap must fit in an 8*zoom cell; scaling
    // with zoom (rather than a fixed size) keeps the label proportionate at
    // both tiny and huge zoom levels instead of a hard "too small" cutoff.
    let scale = (zoom / 4).max(1);
    let (glyph_w, glyph_h) = (7 * scale, 5 * scale);

    for j in 0..ch {
        for i in 0..cw {
            let t = tiles[((cy + j) as usize) * MAP_W + (cx + i) as usize];
            if t == 0 {
                continue;
            }
            let ox = i * cell_px;
            let oy = j * cell_px;
            let bg = canvas.get(ox + cell_px / 2, oy + cell_px / 2);
            let ink = if view::luminance(bg) < 128.0 {
                view::INK_LIGHT
            } else {
                view::INK_DARK
            };
            let gx = ox + cell_px.saturating_sub(glyph_w) / 2;
            let gy = oy + cell_px.saturating_sub(glyph_h) / 2;
            draw_hex_glyph(canvas, gx, gy, (t >> 4) & 0xF, scale, ink);
            draw_hex_glyph(canvas, gx + 4 * scale, gy, t & 0xF, scale, ink);
        }
    }
}

/// One 3x5 hex-digit glyph (from `sprite::view::GLYPHS`) at device pixel
/// `(x, y)`, each glyph pixel drawn as a `scale`x`scale` block.
fn draw_hex_glyph(canvas: &mut view::Canvas, x: u32, y: u32, digit: u8, scale: u32, ink: [u8; 3]) {
    for (row, bits) in view::GLYPHS[digit as usize].iter().enumerate() {
        for bit in 0..3u32 {
            if bits & (1 << (2 - bit)) != 0 {
                canvas.fill(
                    view::Rect {
                        x: x + bit * scale,
                        y: y + row as u32 * scale,
                        w: scale,
                        h: scale,
                    },
                    ink,
                );
            }
        }
    }
}

/// `map dump` — print `region` as hex rows, top to bottom, 2 lowercase hex
/// chars per cell (the `__map__` alphabet), preceded by a `#`-comment
/// header naming the region's cell-space coordinates. The header is a
/// comment specifically so `map dump | map poke --stdin` round-trips
/// without the caller stripping it first — `poke --stdin` skips
/// `#`-prefixed lines for exactly this reason, mirroring `sprite dump`/
/// `sprite poke`.
pub fn dump(cart: &Cart, region: (u32, u32, u32, u32)) -> Result<String, String> {
    let (cx, cy, cw, ch) = region;
    super::validate_region(cx, cy, cw, ch)?;
    let tiles = cart.map();
    let mut out = format!("# cx={cx} cy={cy} cw={cw} ch={ch}\n");
    for j in 0..ch {
        let mut row = String::with_capacity((cw * 2) as usize);
        for i in 0..cw {
            let t = tiles[((cy + j) as usize) * MAP_W + (cx + i) as usize];
            row.push_str(&format!("{t:02x}"));
        }
        out.push_str(&row);
        out.push('\n');
    }
    Ok(out)
}

/// `map lint` — pure numbers, no judgements, over the *whole* map (cheap:
/// at most 128x64 = 8192 cells): the used extent, cell counts by tile id
/// (top [`TOP_N`]), tile ids referenced whose sprite-sheet region is
/// entirely color 0 (the map analog of "color unique to a single frame" —
/// almost always a typo'd id, since a real tile is drawn with something),
/// and `%` fill.
pub fn lint(cart: &Cart) -> Value {
    let tiles = cart.map();
    let sheet = cart.sprites();

    let mut hist: BTreeMap<u8, u32> = BTreeMap::new();
    let mut nonzero = 0u32;
    let mut bbox: Option<(u32, u32, u32, u32)> = None;
    for y in 0..MAP_H {
        for x in 0..MAP_W {
            let t = tiles[y * MAP_W + x];
            if t == 0 {
                continue;
            }
            nonzero += 1;
            *hist.entry(t).or_insert(0) += 1;
            let (x, y) = (x as u32, y as u32);
            bbox = Some(match bbox {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }

    let mut counts: Vec<(u8, u32)> = hist.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let top: Vec<Value> = counts
        .iter()
        .take(TOP_N)
        .map(|&(t, c)| json!({"tile": t, "count": c}))
        .collect();

    let blank: Vec<Value> = counts
        .iter()
        .filter(|&&(t, _)| sprite_region_is_blank(sheet, t))
        .map(|&(t, c)| json!({"tile": t, "count": c}))
        .collect();

    let used_extent = bbox
        .map(|(x0, y0, x1, y1)| json!({"cx": x0, "cy": y0, "cw": x1 - x0 + 1, "ch": y1 - y0 + 1}));

    let total = (MAP_W * MAP_H) as u32;
    json!({
        "map_w": MAP_W,
        "map_h": MAP_H,
        "total_cells": total,
        "nonzero_cells": nonzero,
        "fill_pct": round2(f64::from(nonzero) / f64::from(total) * 100.0),
        "used_extent": used_extent,
        "distinct_tiles": counts.len(),
        "tile_counts": top,
        "blank_sprite_tiles": blank,
    })
}

/// True when every pixel of tile `t`'s 8x8 sheet rect is color 0 — a tile
/// referenced by the map but never actually drawn on the sheet, almost
/// always a typo'd id (transposed digits, off-by-one).
fn sprite_region_is_blank(sheet: &SpriteSheet, t: u8) -> bool {
    let (x0, y0, w, h) = tile_rect(t);
    (0..h)
        .all(|dy| (0..w).all(|dx| sheet[((y0 + dy) as usize) * SHEET_W + (x0 + dx) as usize] == 0))
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// CLI entry for the view-side map commands (`render`, `dump`, `lint`).
/// `args[0]` is the command name, `args[1]` the cart path. Returns the
/// process exit code.
pub fn cli_view(args: &[String]) -> i32 {
    match run_view(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("{}", super::MAP_USAGE);
            2
        }
    }
}

struct Flags {
    zoom: u32,
    grid: bool,
    ids: bool,
    out: Option<String>,
    positional: Vec<String>,
}

fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags {
        zoom: DEFAULT_ZOOM,
        grid: false,
        ids: false,
        out: None,
        positional: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--zoom" => f.zoom = next_u32(&mut it, "--zoom")?,
            "--grid" => f.grid = true,
            "--ids" => f.ids = true,
            "-o" | "--out" => {
                f.out = Some(it.next().ok_or("-o requires an output path")?.clone());
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown flag {other:?}"));
            }
            other => f.positional.push(other.to_string()),
        }
    }
    Ok(f)
}

fn next_u32<'a>(it: &mut impl Iterator<Item = &'a String>, what: &str) -> Result<u32, String> {
    let v = it
        .next()
        .ok_or_else(|| format!("{what} requires a value"))?;
    v.parse()
        .map_err(|_| format!("invalid {what} value {v:?} (want a non-negative integer)"))
}

fn run_view(args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).unwrap_or_default();
    let flags = parse_flags(&args[1..])?;
    let cart_path = flags
        .positional
        .first()
        .ok_or_else(|| format!("map {cmd} requires a cart path"))?;
    let text = std::fs::read_to_string(cart_path)
        .map_err(|e| format!("cannot read {cart_path:?}: {e}"))?;
    let cart = Cart::parse(&text).map_err(|e| e.to_string())?;

    if cmd == "lint" {
        if flags.positional.len() > 1 {
            return Err(format!(
                "map lint takes no region argument, got {:?}",
                &flags.positional[1..]
            ));
        }
        let value = lint(&cart);
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    if flags.positional.len() > 2 {
        return Err(format!(
            "map {cmd}: unexpected extra arguments: {:?}",
            &flags.positional[2..]
        ));
    }
    let region_arg = flags.positional.get(1).map(String::as_str);
    let region = super::parse_region(region_arg, cart.map())?;

    if cmd == "dump" {
        print!("{}", dump(&cart, region)?);
        return Ok(());
    }

    if cmd == "render" {
        let out = flags
            .out
            .as_deref()
            .ok_or("map render requires -o <out.png>")?;
        let opts = MapRenderOpts {
            zoom: flags.zoom,
            grid: flags.grid,
            ids: flags.ids,
        };
        let image = render(&cart, region, &opts)?;
        crate::artifact::write(out, &image.png)?;
        println!(
            "wrote {out} ({}x{} px, region {},{},{}x{} cells, zoom {})",
            image.width, image.height, region.0, region.1, region.2, region.3, flags.zoom
        );
        return Ok(());
    }

    Err(format!("unknown map command {cmd:?}"))
}

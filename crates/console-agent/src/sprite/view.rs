//! Inspection renders (render/atlas/strip/onion/diff/ghost) and numeric lints for
//! `__gfx_meta__` sprites and anims — SPEC.md "Sprite & animation authoring
//! (PoC v1)", the inspection-tools table.
//!
//! Everything here reads a [`Cart`] directly; nothing steps the console. The
//! same entry points back both the CLI (`console sprite ...`, via
//! [`cli_view`]) and the RPC verbs in `rpc.rs`, so the two surfaces can never
//! drift: [`render`], [`strip`], [`onion`], [`diff`], [`ghost`], [`lint`].
//! [`gif`] is CLI-only (an animated preview has no single-frame RPC shape to
//! mirror) but otherwise follows the same conventions. [`atlas`] pairs an
//! annotated whole-sheet image with a semantic allocation report.
//!
//! Render conventions, shared by every image command:
//!
//! - **zoom** defaults to [`DEFAULT_ZOOM`] (8) — each sheet pixel becomes a
//!   `zoom`x`zoom` block.
//! - **color 0 is transparent** and lets a dark checkerboard show through.
//!   Checker cells are 4 *logical* (sheet) pixels square, so the backdrop
//!   stays legible instead of shimmering at high zoom.
//! - `--grid` overlays tile (8px) boundaries, `--indices` writes the palette
//!   index into each pixel cell as two 3x5 hex glyphs (only when `zoom >= 8`,
//!   otherwise silently skipped), `--anchor` draws a crosshair in palette
//!   color 4 at the sprite's anchor pixel. `onion` and `ghost` accept
//!   `--grid`/`--anchor` too (mirroring `render`/`strip`), via
//!   [`OverlayOpts`]. `onion --all` renders a contact sheet — every frame of
//!   the anim side by side, each with its own onion skin and a frame-number
//!   caption (see [`onion_all`]).

use std::collections::{BTreeMap, BTreeSet};

use console_core::{
    AnimDef, COLOR_MASK, Cart, FrameSpec, PALETTE, PreviewPalette, SHEET_W, SpriteDef,
};
use serde_json::{Value, json};

use super::{Target, frame_pixel_rect, parse_target, resolve_rect, target_sprite};

/// Zoom used when `--zoom` / the RPC `zoom` param is omitted.
pub const DEFAULT_ZOOM: u32 = 8;
/// Upper bound on zoom, so a typo cannot ask for a gigabyte of PNG.
const MAX_ZOOM: u32 = 64;
/// Checkerboard cell size in *sheet* pixels (not device pixels).
const CHECKER_LOGICAL: u32 = 4;
const CHECKER_A: [u8; 3] = [0x14, 0x16, 0x1f];
const CHECKER_B: [u8; 3] = [0x1d, 0x21, 0x30];
/// Solid fill for the 2px gutters between `strip` frames.
const SEPARATOR: [u8; 3] = [0x0a, 0x0b, 0x10];
const SEPARATOR_PX: u32 = 2;
const GRID_RGB: [u8; 3] = [0x56, 0x6c, 0x86];
const GRID_ALPHA: f32 = 0.6;
/// Onion-skin ghost strength for the previous/next frame.
const GHOST_ALPHA: f32 = 0.35;
const GHOST_PREV: [u8; 3] = [255, 0, 0];
const GHOST_NEXT: [u8; 3] = [0, 255, 0];
/// `diff` renders frame B at this fraction of its palette brightness.
const DIFF_DIM: f32 = 0.35;
const DIFF_MARK: [u8; 3] = [255, 0, 255];
/// Glyph ink for `--indices`, picked per cell by background luminance.
/// `pub(crate)`: also used by `map::view` to ink its `--ids` cell-id glyphs
/// against the same background-luminance rule `--indices` uses here.
pub(crate) const INK_LIGHT: [u8; 3] = [0xf4, 0xf4, 0xf4];
pub(crate) const INK_DARK: [u8; 3] = [0x1a, 0x1c, 0x2c];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Flags shared by `sprite render` and the `sprite_render` RPC verb.
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    /// Frame selector. For an anim target this indexes the anim's frame
    /// list; for a bare sprite it is the raw frame index (the rect displaced
    /// `n` sprite-widths right). Raw `tx,ty,w,h` rects accept only 0.
    pub frame: Option<u32>,
    pub zoom: u32,
    pub grid: bool,
    pub indices: bool,
    pub anchor: bool,
}

impl Default for RenderOpts {
    fn default() -> RenderOpts {
        RenderOpts {
            frame: None,
            zoom: DEFAULT_ZOOM,
            grid: false,
            indices: false,
            anchor: false,
        }
    }
}

/// Flags shared by `sprite onion` / `sprite ghost` and their RPC mirrors —
/// both are anim-wide overlays with no per-target `--indices` concept, so
/// they share this smaller options struct rather than [`RenderOpts`].
#[derive(Debug, Clone, Copy)]
pub struct OverlayOpts {
    pub zoom: u32,
    pub grid: bool,
    pub anchor: bool,
}

impl Default for OverlayOpts {
    fn default() -> OverlayOpts {
        OverlayOpts {
            zoom: DEFAULT_ZOOM,
            grid: false,
            anchor: false,
        }
    }
}

/// A finished render: the encoded PNG plus the raw RGBA it came from (handy
/// for assertions without a decode round-trip).
pub struct Image {
    pub png: Vec<u8>,
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// How many sprite frames the image depicts.
    pub frames: usize,
}

/// A semantic atlas: the annotated full sprite sheet plus a deterministic
/// machine-readable inventory of every declaration and resolved frame.
pub struct AtlasResult {
    pub image: Image,
    pub report: Value,
}

/// Debug by shape, not by content — the pixel buffers are far too big to
/// print, and it is the dimensions that matter in an assertion message.
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frames", &self.frames)
            .field("png_bytes", &self.png.len())
            .finish()
    }
}

/// `sprite render` — one frame of `target`, zoomed, on the checkerboard.
pub fn render(cart: &Cart, target: &str, opts: &RenderOpts) -> Result<Image, String> {
    let zoom = check_zoom(opts.zoom)?;
    let target = parse_target(target, cart.gfx_meta())?;
    let rect = render_rect(cart, &target, opts.frame.unwrap_or(0))?;
    let frame = read_rect(cart, rect);

    // With --grid the closing boundary lines need one extra device pixel on
    // the right/bottom edge, so the sprite's own outline is fully drawn.
    let extra = u32::from(opts.grid);
    let mut canvas = Canvas::new(frame.w * zoom + extra, frame.h * zoom + extra, zoom);
    let cell = Rect {
        x: 0,
        y: 0,
        w: frame.w * zoom,
        h: frame.h * zoom,
    };
    draw_frame(&mut canvas, &frame, (0, 0), zoom, cart.preview_palette());
    if opts.indices {
        draw_indices(&mut canvas, &frame, (0, 0), zoom);
    }
    if opts.grid {
        draw_grid(&mut canvas, cell, zoom);
    }
    if opts.anchor {
        let anchor = target_sprite(cart, &target)
            .map(|d| d.anchor)
            .ok_or("--anchor needs a sprite or anim target; raw tx,ty,w,h rects have no anchor")?;
        draw_anchor(&mut canvas, cell, anchor, zoom);
    }
    Ok(canvas.finish(1))
}

/// `sprite strip` — every frame of `anim` left to right, separated by a 2px
/// gutter, each in a common bounding box so the anchors line up vertically
/// (the "baseline" of a walk cycle stays flat across the strip).
pub fn strip(cart: &Cart, anim: &str, zoom: u32, anchor: bool) -> Result<Image, String> {
    let zoom = check_zoom(zoom)?;
    let (_, sprite, frames) = anim_frames(cart, anim)?;
    let layout = align(&frames, sprite.anchor);

    let n = frames.len() as u32;
    let width = n * layout.cell_w * zoom + n.saturating_sub(1) * SEPARATOR_PX;
    let height = layout.cell_h * zoom;
    let mut canvas = Canvas::new(width, height, zoom);

    for (i, frame) in frames.iter().enumerate() {
        let cell_x = i as u32 * (layout.cell_w * zoom + SEPARATOR_PX);
        if i > 0 {
            canvas.fill(
                Rect {
                    x: cell_x - SEPARATOR_PX,
                    y: 0,
                    w: SEPARATOR_PX,
                    h: height,
                },
                SEPARATOR,
            );
        }
        let (ox, oy) = layout.offsets[i];
        draw_frame(
            &mut canvas,
            frame,
            (cell_x + ox * zoom, oy * zoom),
            zoom,
            cart.preview_palette(),
        );
        if anchor {
            let cell = Rect {
                x: cell_x,
                y: 0,
                w: layout.cell_w * zoom,
                h: height,
            };
            draw_anchor(
                &mut canvas,
                cell,
                (layout.anchor.0 as i32, layout.anchor.1 as i32),
                zoom,
            );
        }
    }
    Ok(canvas.finish(frames.len()))
}

/// The onion-skin neighbours of frame `pos` among `n` frames: the previous
/// frame (red ghost) and the next (green ghost), wrapping around for `loop`
/// anims and simply absent at the ends otherwise. Shared by [`onion`] (one
/// frame centred) and [`onion_all`] (every frame gets this treatment).
fn onion_neighbours(looped: bool, pos: usize, n: usize) -> (Option<usize>, Option<usize>) {
    let last = n - 1;
    let prev = if pos > 0 {
        Some(pos - 1)
    } else if looped && n > 1 {
        Some(last)
    } else {
        None
    };
    let next = if pos < last {
        Some(pos + 1)
    } else if looped && n > 1 {
        Some(0)
    } else {
        None
    };
    (prev, next)
}

/// Paint `prev`/`next` (if present) as red/green ghosts at ~35% opacity,
/// offset by `origin` (device pixels) on top of `layout`'s per-frame cell
/// offsets. Does not paint the solid frame itself — callers do that last, so
/// it always wins where the silhouettes overlap.
fn paint_onion_ghosts(
    canvas: &mut Canvas,
    frames: &[Frame],
    layout: &Layout,
    prev: Option<usize>,
    next: Option<usize>,
    origin: (u32, u32),
    zoom: u32,
) {
    for (which, tint) in [(prev, GHOST_PREV), (next, GHOST_NEXT)] {
        if let Some(i) = which {
            let (ox, oy) = layout.offsets[i];
            tint_frame(
                canvas,
                &frames[i],
                (origin.0 + ox * zoom, origin.1 + oy * zoom),
                zoom,
                tint,
                GHOST_ALPHA,
            );
        }
    }
}

/// `sprite onion` — frame `pos` at full opacity over its neighbours: the
/// previous frame tinted red, the next tinted green, both at ~35% and both
/// skipping color-0 pixels. Neighbour choice wraps for `loop` anims and is
/// simply absent at the ends otherwise. The solid frame is painted last, so
/// it always wins where the silhouettes overlap. `opts.grid`/`opts.anchor`
/// overlay the same tile-boundary grid and anchor crosshair `render`/`strip`
/// draw.
pub fn onion(cart: &Cart, anim: &str, pos: u32, opts: &OverlayOpts) -> Result<Image, String> {
    let zoom = check_zoom(opts.zoom)?;
    let (def, sprite, frames) = anim_frames(cart, anim)?;
    let pos = check_pos(pos, frames.len(), anim, "--frame")?;
    let layout = align(&frames, sprite.anchor);

    let mut canvas = Canvas::new(layout.cell_w * zoom, layout.cell_h * zoom, zoom);
    let (prev, next) = onion_neighbours(def.looped, pos, frames.len());
    paint_onion_ghosts(&mut canvas, &frames, &layout, prev, next, (0, 0), zoom);
    let (ox, oy) = layout.offsets[pos];
    draw_frame(
        &mut canvas,
        &frames[pos],
        (ox * zoom, oy * zoom),
        zoom,
        cart.preview_palette(),
    );

    let cell = Rect {
        x: 0,
        y: 0,
        w: layout.cell_w * zoom,
        h: layout.cell_h * zoom,
    };
    if opts.grid {
        draw_grid(&mut canvas, cell, zoom);
    }
    if opts.anchor {
        draw_anchor(
            &mut canvas,
            cell,
            (layout.anchor.0 as i32, layout.anchor.1 as i32),
            zoom,
        );
    }
    Ok(canvas.finish(1 + usize::from(prev.is_some()) + usize::from(next.is_some())))
}

/// `sprite onion --all` — a contact sheet: every frame of `anim` side by
/// side (same 2px gutter as `strip`), each rendered exactly like a single
/// `onion` call centred on that frame (its own red/green neighbour ghosts),
/// and labelled with its frame index in a caption band below. `opts.grid`/
/// `opts.anchor` overlay every cell, same as `onion`.
pub fn onion_all(cart: &Cart, anim: &str, opts: &OverlayOpts) -> Result<Image, String> {
    let zoom = check_zoom(opts.zoom)?;
    let (def, sprite, frames) = anim_frames(cart, anim)?;
    let layout = align(&frames, sprite.anchor);
    let n = frames.len();

    let cell_w = layout.cell_w * zoom;
    let cell_h = layout.cell_h * zoom;
    let width = n as u32 * cell_w + (n as u32).saturating_sub(1) * SEPARATOR_PX;
    let height = cell_h + LABEL_HEIGHT;
    let mut canvas = Canvas::new(width, height, zoom);
    canvas.fill(
        Rect {
            x: 0,
            y: cell_h,
            w: width,
            h: LABEL_HEIGHT,
        },
        SEPARATOR,
    );

    for i in 0..n {
        let cell_x = i as u32 * (cell_w + SEPARATOR_PX);
        if i > 0 {
            canvas.fill(
                Rect {
                    x: cell_x - SEPARATOR_PX,
                    y: 0,
                    w: SEPARATOR_PX,
                    h: height,
                },
                SEPARATOR,
            );
        }
        let (prev, next) = onion_neighbours(def.looped, i, n);
        paint_onion_ghosts(&mut canvas, &frames, &layout, prev, next, (cell_x, 0), zoom);
        let (ox, oy) = layout.offsets[i];
        draw_frame(
            &mut canvas,
            &frames[i],
            (cell_x + ox * zoom, oy * zoom),
            zoom,
            cart.preview_palette(),
        );

        let cell = Rect {
            x: cell_x,
            y: 0,
            w: cell_w,
            h: cell_h,
        };
        if opts.grid {
            draw_grid(&mut canvas, cell, zoom);
        }
        if opts.anchor {
            draw_anchor(
                &mut canvas,
                cell,
                (layout.anchor.0 as i32, layout.anchor.1 as i32),
                zoom,
            );
        }
        draw_label(
            &mut canvas,
            Rect {
                x: cell_x,
                y: cell_h,
                w: cell_w,
                h: LABEL_HEIGHT,
            },
            &i.to_string(),
        );
    }
    Ok(canvas.finish(n))
}

/// `sprite diff` — frame B dimmed to ~35% brightness, with every pixel whose
/// palette index differs from frame A's overpainted in bright magenta.
pub fn diff(cart: &Cart, anim: &str, a: u32, b: u32, zoom: u32) -> Result<Image, String> {
    let zoom = check_zoom(zoom)?;
    let (_, sprite, frames) = anim_frames(cart, anim)?;
    let a = check_pos(a, frames.len(), anim, "<frameA>")?;
    let b = check_pos(b, frames.len(), anim, "<frameB>")?;
    let layout = align(&frames, sprite.anchor);

    let mut canvas = Canvas::new(layout.cell_w * zoom, layout.cell_h * zoom, zoom);
    let (fa, fb) = (&frames[a], &frames[b]);
    let (ox, oy) = layout.offsets[b];
    for y in 0..fb.h {
        for x in 0..fb.w {
            let vb = fb.at(x, y);
            let block = Rect {
                x: (ox + x) * zoom,
                y: (oy + y) * zoom,
                w: zoom,
                h: zoom,
            };
            if vb != 0 {
                canvas.fill(
                    block,
                    dim(preview_rgb(cart.preview_palette(), vb), DIFF_DIM),
                );
            }
            if fa.at(x, y) != vb {
                canvas.fill(block, DIFF_MARK);
            }
        }
    }
    Ok(canvas.finish(2))
}

/// `sprite ghost` — every frame overlaid at low alpha, so the areas the
/// animation actually moves through accumulate brightness. `opts.grid`/
/// `opts.anchor` overlay the same tile-boundary grid and anchor crosshair
/// `render`/`strip`/`onion` draw.
pub fn ghost(cart: &Cart, anim: &str, opts: &OverlayOpts) -> Result<Image, String> {
    let zoom = check_zoom(opts.zoom)?;
    let (_, sprite, frames) = anim_frames(cart, anim)?;
    let layout = align(&frames, sprite.anchor);
    let mut canvas = Canvas::new(layout.cell_w * zoom, layout.cell_h * zoom, zoom);

    let alpha = (0.85 / frames.len() as f32).clamp(0.12, 0.45);
    for (i, frame) in frames.iter().enumerate() {
        let (ox, oy) = layout.offsets[i];
        for y in 0..frame.h {
            for x in 0..frame.w {
                let v = frame.at(x, y);
                if v == 0 {
                    continue;
                }
                canvas.blend_fill(
                    Rect {
                        x: (ox + x) * zoom,
                        y: (oy + y) * zoom,
                        w: zoom,
                        h: zoom,
                    },
                    preview_rgb(cart.preview_palette(), v),
                    alpha,
                );
            }
        }
    }

    let cell = Rect {
        x: 0,
        y: 0,
        w: layout.cell_w * zoom,
        h: layout.cell_h * zoom,
    };
    if opts.grid {
        draw_grid(&mut canvas, cell, zoom);
    }
    if opts.anchor {
        draw_anchor(
            &mut canvas,
            cell,
            (layout.anchor.0 as i32, layout.anchor.1 as i32),
            zoom,
        );
    }
    Ok(canvas.finish(frames.len()))
}

/// `sprite atlas` — render the whole sheet as one annotated allocation map
/// and report how `__gfx_meta__` declarations consume it. Multiple uses by
/// one sprite namespace are intentional aliases; uses by different sprites
/// are conflicts. This makes relocated/explicit animation frames visible
/// without changing their runtime resolution rules.
pub fn atlas(cart: &Cart, zoom: u32, grid: bool) -> Result<AtlasResult, String> {
    let zoom = check_zoom(zoom)?;
    let meta = cart.gfx_meta();
    let mut owners: BTreeMap<u8, Vec<(String, String)>> = BTreeMap::new();
    let mut sprites = Vec::new();
    let mut animations = Vec::new();
    let mut blank_allocations = Vec::new();

    for sprite in meta.sprites() {
        let rect = (
            u32::from(sprite.rect.0) * 8,
            u32::from(sprite.rect.1) * 8,
            u32::from(sprite.size.0) * 8,
            u32::from(sprite.size.1) * 8,
        );
        mark_atlas_rect(
            &mut owners,
            sprite.rect.0,
            sprite.rect.1,
            sprite.size.0,
            sprite.size.1,
            &sprite.name,
            format!("sprite:{}", sprite.name),
        );
        let frame = read_rect(cart, rect);
        let stats = frame_stats(&frame, (0, 0));
        let blank = stats.area == 0;
        if blank {
            blank_allocations.push(json!({"kind":"sprite", "name":sprite.name}));
        }
        sprites.push(json!({
            "name": sprite.name,
            "rect": tile_rect_json(sprite.rect.0, sprite.rect.1, sprite.size.0, sprite.size.1),
            "anchor": {"x":sprite.anchor.0, "y":sprite.anchor.1},
            "blank": blank,
            "palette_counts": hist_json(&stats.hist),
        }));
    }

    for anim in meta.anims() {
        let sprite = meta.sprite(&anim.sprite).ok_or_else(|| {
            format!(
                "anim {:?} names unknown sprite {:?}",
                anim.name, anim.sprite
            )
        })?;
        let mut frames = Vec::with_capacity(anim.frames.len());
        for position in 0..anim.frames.len() {
            let (x, y, w, h) = anim.resolve_frame(sprite, position).ok_or_else(|| {
                format!(
                    "frame {position} of anim {:?} falls off the sheet",
                    anim.name
                )
            })?;
            let (tx, ty, tw, th) = ((x / 8) as u8, (y / 8) as u8, (w / 8) as u8, (h / 8) as u8);
            mark_atlas_rect(
                &mut owners,
                tx,
                ty,
                tw,
                th,
                &anim.sprite,
                format!("anim:{}[{position}]", anim.name),
            );
            let frame = read_rect(cart, (x, y, w, h));
            let stats = frame_stats(&frame, (0, 0));
            let blank = stats.area == 0;
            if blank {
                blank_allocations.push(json!({
                    "kind":"animation_frame", "name":anim.name, "position":position
                }));
            }
            frames.push(json!({
                "position": position,
                "spec": frame_spec_json(anim.frames[position]),
                "rect": tile_rect_json(tx, ty, tw, th),
                "tiles": atlas_tile_ids(tx, ty, tw, th),
                "blank": blank,
                "palette_counts": hist_json(&stats.hist),
            }));
        }
        animations.push(json!({
            "name": anim.name,
            "sprite": anim.sprite,
            "fps": anim.fps,
            "loop": anim.looped,
            "frames": frames,
        }));
    }

    let used_tiles: Vec<u8> = owners.keys().copied().collect();
    let used_set: BTreeSet<u8> = used_tiles.iter().copied().collect();
    let unused_tiles: Vec<u8> = (0..=u8::MAX)
        .filter(|tile| !used_set.contains(tile))
        .collect();
    let mut overlaps = Vec::new();
    let mut alias_tiles = BTreeSet::new();
    let mut conflict_tiles = BTreeSet::new();
    for (&tile, uses) in &owners {
        if uses.len() < 2 {
            continue;
        }
        let namespaces: BTreeSet<&str> = uses
            .iter()
            .map(|(namespace, _)| namespace.as_str())
            .collect();
        let conflict = namespaces.len() > 1;
        if conflict {
            conflict_tiles.insert(tile);
        } else {
            alias_tiles.insert(tile);
        }
        overlaps.push(json!({
            "tile": tile,
            "classification": if conflict { "conflict" } else { "alias" },
            "sprites": namespaces,
            "uses": uses.iter().map(|(_, usage)| usage).collect::<Vec<_>>(),
        }));
    }

    let extra = u32::from(grid);
    let mut canvas = Canvas::new(128 * zoom + extra, 128 * zoom + extra, zoom);
    let sheet = read_rect(cart, (0, 0, 128, 128));
    draw_frame(&mut canvas, &sheet, (0, 0), zoom, cart.preview_palette());
    for tile in &unused_tiles {
        canvas.blend_fill(atlas_tile_cell(*tile, zoom), [4, 7, 12], 0.58);
    }
    for tile in &alias_tiles {
        canvas.blend_fill(atlas_tile_cell(*tile, zoom), [0, 210, 220], 0.16);
    }
    for tile in &conflict_tiles {
        canvas.blend_fill(atlas_tile_cell(*tile, zoom), [255, 0, 190], 0.32);
    }
    if grid {
        draw_grid(
            &mut canvas,
            Rect {
                x: 0,
                y: 0,
                w: 128 * zoom,
                h: 128 * zoom,
            },
            zoom,
        );
    }
    for (index, sprite) in meta.sprites().enumerate() {
        let cell = Rect {
            x: u32::from(sprite.rect.0) * 8 * zoom,
            y: u32::from(sprite.rect.1) * 8 * zoom,
            w: u32::from(sprite.size.0) * 8 * zoom,
            h: u32::from(sprite.size.1) * 8 * zoom,
        };
        outline(&mut canvas, cell, PALETTE[atlas_outline_color(index)]);
        draw_anchor(&mut canvas, cell, sprite.anchor, zoom);
    }
    for tile in 0..=u8::MAX {
        draw_atlas_tile_id(&mut canvas, tile, zoom);
    }

    let report = json!({
        "sheet": {"tiles_w":16, "tiles_h":16, "tile_count":256},
        "sprites": sprites,
        "animations": animations,
        "overlaps": overlaps,
        "blank_allocations": blank_allocations,
        "used_tiles": used_tiles,
        "unused_tiles": unused_tiles,
    });
    Ok(AtlasResult {
        image: canvas.finish(1),
        report,
    })
}

fn mark_atlas_rect(
    owners: &mut BTreeMap<u8, Vec<(String, String)>>,
    tx: u8,
    ty: u8,
    w: u8,
    h: u8,
    namespace: &str,
    usage: String,
) {
    for tile in atlas_tile_ids(tx, ty, w, h) {
        owners
            .entry(tile)
            .or_default()
            .push((namespace.to_string(), usage.clone()));
    }
}

fn atlas_tile_ids(tx: u8, ty: u8, w: u8, h: u8) -> Vec<u8> {
    (0..h)
        .flat_map(|dy| (0..w).map(move |dx| (ty + dy) * 16 + tx + dx))
        .collect()
}

fn tile_rect_json(tx: u8, ty: u8, w: u8, h: u8) -> Value {
    json!({"tx":tx, "ty":ty, "w":w, "h":h})
}

fn atlas_tile_cell(tile: u8, zoom: u32) -> Rect {
    Rect {
        x: u32::from(tile % 16) * 8 * zoom,
        y: u32::from(tile / 16) * 8 * zoom,
        w: 8 * zoom,
        h: 8 * zoom,
    }
}

fn atlas_outline_color(index: usize) -> usize {
    const COLORS: [usize; 8] = [14, 21, 28, 33, 42, 48, 54, 60];
    COLORS[index % COLORS.len()]
}

fn outline(canvas: &mut Canvas, rect: Rect, color: [u8; 3]) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    for x in rect.x..=rect.x + rect.w {
        canvas.set(x, rect.y, color);
        canvas.set(x, rect.y + rect.h, color);
    }
    for y in rect.y..=rect.y + rect.h {
        canvas.set(rect.x, y, color);
        canvas.set(rect.x + rect.w, y, color);
    }
}

fn draw_atlas_tile_id(canvas: &mut Canvas, tile: u8, zoom: u32) {
    let cell = atlas_tile_cell(tile, zoom);
    let scale = (zoom / 2).max(1);
    let x = cell.x + scale;
    let y = cell.y + scale;
    let ink = if luminance(canvas.get(cell.x + cell.w / 2, cell.y + cell.h / 2)) < 128.0 {
        INK_LIGHT
    } else {
        INK_DARK
    };
    for (digit, dx) in [(tile >> 4, 0), (tile & 0x0f, 4 * scale)] {
        for (row, bits) in GLYPHS[usize::from(digit)].iter().enumerate() {
            for bit in 0..3u32 {
                if bits & (1 << (2 - bit)) != 0 {
                    canvas.fill(
                        Rect {
                            x: x + dx + bit * scale,
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
}

/// A finished GIF: the encoded bytes plus the dimensions/frame count it
/// depicts (mirrors [`Image`] for the PNG-producing commands, minus the raw
/// RGBA buffer — a GIF's frames don't share one).
pub struct GifOutput {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
}

/// Debug by shape, not by content — same rationale as [`Image`]'s impl.
impl std::fmt::Debug for GifOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GifOutput")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frames", &self.frames)
            .field("gif_bytes", &self.bytes.len())
            .finish()
    }
}

/// Flags for `sprite gif` — the animated counterpart of the PNG view
/// commands. `--zoom` defaults to [`DEFAULT_ZOOM`]; `grid`/`anchor` overlay
/// the same tile-boundary grid and anchor crosshair the other views draw.
#[derive(Debug, Clone, Copy)]
pub struct GifOpts {
    pub zoom: u32,
    pub grid: bool,
    pub anchor: bool,
}

impl Default for GifOpts {
    fn default() -> GifOpts {
        GifOpts {
            zoom: DEFAULT_ZOOM,
            grid: false,
            anchor: false,
        }
    }
}

/// `sprite gif` — an animated preview of `anim`, played at its declared
/// `fps`. GIF has no "play once" — every viewer loops a GIF regardless of
/// its own loop count — so this always encodes `Repeat::Infinite`, even for
/// an anim without `loop` in `__gfx_meta__` (that flag governs in-engine
/// playback only; there is nothing in the GIF container that could honor
/// it). Color 0 is baked into each frame as the same checkerboard the PNG
/// views use — there is no GIF transparency involved, so `--grid`'s
/// alpha-blended lines and any background tone survive the encode exactly
/// as rendered.
pub fn gif(cart: &Cart, anim: &str, opts: &GifOpts) -> Result<GifOutput, String> {
    let zoom = check_zoom(opts.zoom)?;
    let (def, sprite, frames) = anim_frames(cart, anim)?;
    let layout = align(&frames, sprite.anchor);
    let width = layout.cell_w * zoom;
    let height = layout.cell_h * zoom;
    let width16 =
        u16::try_from(width).map_err(|_| format!("gif: {width}px frame is too wide to encode"))?;
    let height16 = u16::try_from(height)
        .map_err(|_| format!("gif: {height}px frame is too tall to encode"))?;
    let delay = gif_delay_from_fps(def.fps);
    let cell = Rect {
        x: 0,
        y: 0,
        w: width,
        h: height,
    };

    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, width16, height16, &[])
            .map_err(|e| format!("gif: {e}"))?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .map_err(|e| format!("gif: {e}"))?;

        for (i, frame) in frames.iter().enumerate() {
            let (ox, oy) = layout.offsets[i];
            let mut canvas = Canvas::new(width, height, zoom);
            draw_frame(
                &mut canvas,
                frame,
                (ox * zoom, oy * zoom),
                zoom,
                cart.preview_palette(),
            );
            if opts.grid {
                draw_grid(&mut canvas, cell, zoom);
            }
            if opts.anchor {
                draw_anchor(
                    &mut canvas,
                    cell,
                    (layout.anchor.0 as i32, layout.anchor.1 as i32),
                    zoom,
                );
            }
            let mut rgba = canvas.into_rgba();
            let mut gif_frame = gif::Frame::from_rgba_speed(width16, height16, &mut rgba, 10);
            gif_frame.delay = delay;
            encoder
                .write_frame(&gif_frame)
                .map_err(|e| format!("gif: writing frame {i}: {e}"))?;
        }
        // Consumes the encoder to write the trailer now, rather than relying
        // on its Drop impl; the returned writer (our `&mut bytes` borrow) is
        // of no further use once the borrow it represents ends here.
        encoder.into_inner().map_err(|e| format!("gif: {e}"))?;
    }
    Ok(GifOutput {
        bytes,
        width,
        height,
        frames: frames.len(),
    })
}

/// GIF frame delay is in units of 10ms. `fps` converts as `1000/fps`,
/// rounded to the nearest 10ms step, with a floor of 2 (20ms) — GIF cannot
/// express anything finer, and a delay of 0 or 1 is "as fast as possible" in
/// most viewers rather than the intended frame rate.
fn gif_delay_from_fps(fps: u8) -> u16 {
    let hundredths = (1000.0 / f64::from(fps.max(1)) / 10.0).round();
    (hundredths as u16).max(2)
}

/// CI-gate thresholds for `sprite lint` — SPEC.md "Sprite & animation
/// authoring (PoC v1)". Every field `None`/`false` (i.e. `default()`) means
/// "report only": [`lint_gated`] returns `violated = false` and its JSON
/// carries no `violations` key at all — the exact pre-existing [`lint`]
/// behavior, so carts/callers that pass no thresholds see no change.
#[derive(Debug, Clone, Copy, Default)]
pub struct LintThresholds {
    /// Max allowed centroid-drift distance, in pixels, between consecutive
    /// (or wrap) frames.
    pub max_drift: Option<f64>,
    /// Max allowed silhouette-area drift, as an absolute percentage.
    pub max_area_var: Option<f64>,
    /// Max allowed changed-pixel count between consecutive (or wrap) frames.
    pub max_changed: Option<u32>,
    /// Any color that appears in exactly one of at least two frames is a
    /// violation. The comparison is not applicable to one-frame animations.
    pub no_unique_colors: bool,
}

impl LintThresholds {
    /// `pub(crate)`: `rpc.rs`'s `sprite_lint` mirror uses this to decide
    /// whether to include a `"violations"` key, same rule the CLI uses.
    pub(crate) fn is_active(&self) -> bool {
        self.max_drift.is_some()
            || self.max_area_var.is_some()
            || self.max_changed.is_some()
            || self.no_unique_colors
    }
}

/// One anim's `--summary` line worth of numbers.
#[derive(Debug, Clone)]
pub struct AnimSummary {
    pub anim: String,
    pub frame_count: usize,
    /// Largest centroid-drift distance across the anim's pairs (`None` when
    /// it has fewer than two comparable frames).
    pub worst_drift: Option<f64>,
    /// Largest changed-pixel count across the anim's pairs.
    pub worst_changed: Option<u32>,
    /// How many colors appear in exactly one frame.
    pub unique_colors: usize,
    /// False when the animation has fewer than two frames to compare.
    pub unique_colors_applicable: bool,
}

impl AnimSummary {
    /// The one printable line SPEC.md's `--summary` describes: name,
    /// frames, worst drift, worst changed, unique-color count.
    pub fn line(&self) -> String {
        let drift = self
            .worst_drift
            .map_or_else(|| "-".to_string(), |d| format!("{:.2}px", round2(d)));
        let changed = self
            .worst_changed
            .map_or_else(|| "-".to_string(), |c| format!("{c}px"));
        let unique = if self.unique_colors_applicable {
            self.unique_colors.to_string()
        } else {
            "n/a".to_string()
        };
        format!(
            "{}: frames={} worst_drift={drift} worst_changed={changed} unique_colors={unique}",
            self.anim, self.frame_count,
        )
    }
}

/// `sprite lint` — pure numbers, no judgements: per-frame silhouette area,
/// anchor-relative bbox and centroid, palette histogram; per consecutive
/// (loop-aware) pair the changed-pixel count and the area/centroid/bbox
/// drift; plus the colors that appear in exactly one frame.
///
/// `names` empty means "every anim in the cart". The result is
/// `{"anims": [ ... ]}` — one object per anim, in the order requested (or
/// sorted by name when reporting all of them). Report-only, no CI gate; see
/// [`lint_gated`] for the thresholded form the CLI's `--max-*`/
/// `--no-unique-colors` flags and the `sprite_lint` RPC verb use.
pub fn lint(cart: &Cart, names: &[String]) -> Result<Value, String> {
    lint_gated(cart, names, &LintThresholds::default()).map(|(value, _violated)| value)
}

/// [`lint`], plus a `sprite_id` (the resolved sheet tile origin `[tx, ty]`)
/// on every frame entry, plus — when `thresholds` has anything set — a
/// top-level `violations` array, one entry per threshold breach: `{"anim",
/// "frame", "metric", "value", "limit"}`. The returned `bool` is `true` iff
/// there was at least one violation; the CLI turns that into exit code 1,
/// the RPC verb into a `"violated"` field, since JSON-RPC has no process
/// exit code to reuse. An inactive (`default()`) `thresholds` always
/// returns `false` with no `violations` key — [`lint`]'s report-only
/// contract, unchanged.
pub fn lint_gated(
    cart: &Cart,
    names: &[String],
    thresholds: &LintThresholds,
) -> Result<(Value, bool), String> {
    let wanted: Vec<String> = if names.is_empty() {
        cart.gfx_meta().anims().map(|a| a.name.clone()).collect()
    } else {
        names.to_vec()
    };

    let mut anims = Vec::with_capacity(wanted.len());
    let mut violations = Vec::new();
    for name in &wanted {
        let (anim_json, anim_violations, _summary) = lint_anim_gated(cart, name, thresholds)?;
        anims.push(anim_json);
        violations.extend(anim_violations);
    }
    let violated = !violations.is_empty();
    let mut result = json!({ "anims": anims });
    if thresholds.is_active() {
        result["violations"] = json!(violations);
    }
    Ok((result, violated))
}

/// `--summary` shape: one [`AnimSummary`] per requested anim, plus the same
/// `violations` list [`lint_gated`] would put in its JSON (computed against
/// the identical per-frame/per-pair numbers — summarizing is a presentation
/// choice, not a different gate).
pub fn lint_summary(
    cart: &Cart,
    names: &[String],
    thresholds: &LintThresholds,
) -> Result<(Vec<AnimSummary>, Vec<Value>, bool), String> {
    let wanted: Vec<String> = if names.is_empty() {
        cart.gfx_meta().anims().map(|a| a.name.clone()).collect()
    } else {
        names.to_vec()
    };

    let mut summaries = Vec::with_capacity(wanted.len());
    let mut violations = Vec::new();
    for name in &wanted {
        let (_json, anim_violations, summary) = lint_anim_gated(cart, name, thresholds)?;
        summaries.push(summary);
        violations.extend(anim_violations);
    }
    let violated = !violations.is_empty();
    Ok((summaries, violations, violated))
}

/// JSON for one `frames=` entry: a plain number for the classic `frames=i`
/// form (back-compat: unchanged shape for carts using no new syntax), or a
/// `"tx:ty"` string for an explicit tile-coordinate frame.
fn frame_spec_json(spec: FrameSpec) -> Value {
    match spec {
        FrameSpec::Index(i) => json!(i),
        FrameSpec::Rect(tx, ty) => json!(format!("{tx}:{ty}")),
    }
}

/// Computes one anim's full lint JSON, its threshold violations (empty when
/// `thresholds` is inactive), and its `--summary` numbers, all from one pass
/// over the frame data — the single place [`lint_gated`] and
/// [`lint_summary`] both go through, so the two presentations can never
/// disagree about what counts as a violation.
fn lint_anim_gated(
    cart: &Cart,
    name: &str,
    thresholds: &LintThresholds,
) -> Result<(Value, Vec<Value>, AnimSummary), String> {
    let (def, sprite, frames) = anim_frames(cart, name)?;
    let anchor = sprite.anchor;
    let stats: Vec<FrameStats> = frames.iter().map(|f| frame_stats(f, anchor)).collect();

    let frames_json: Vec<Value> = def
        .frames
        .iter()
        .zip(&stats)
        .enumerate()
        .map(|(i, (&sheet_frame, s))| {
            // The resolved sheet tile this frame renders from — computed
            // through `AnimDef::resolve_frame` so `frames_rect=` relocation
            // and explicit `tx:ty` forms report the tile that actually
            // draws, not the classic sprite-relative displacement.
            let sprite_id = def
                .resolve_frame(sprite, i)
                .map(|(x, y, _, _)| json!([x / 8, y / 8]));
            json!({
                "index": i,
                "sprite_frame": frame_spec_json(sheet_frame),
                "sprite_id": sprite_id,
                "silhouette_area": s.area,
                "bbox": bbox_json(s.bbox),
                "centroid": s.centroid.map(|(x, y)| json!([round2(x), round2(y)])),
                "palette": hist_json(&s.hist),
            })
        })
        .collect();

    // Consecutive pairs, plus the wrap-around pair when the anim loops.
    let n = frames.len();
    let mut metrics = Vec::new();
    for i in 0..n.saturating_sub(1) {
        metrics.push(pair_metrics(
            &frames[i],
            &frames[i + 1],
            &stats[i],
            &stats[i + 1],
            (i, i + 1),
        ));
    }
    if def.looped && n > 1 {
        metrics.push(pair_metrics(
            &frames[n - 1],
            &frames[0],
            &stats[n - 1],
            &stats[0],
            (n - 1, 0),
        ));
    }
    let pairs: Vec<Value> = metrics.iter().map(pair_json).collect();

    // A color used by exactly one frame is usually either a highlight the
    // other frames forgot or a stray pixel; report it, don't judge it.
    let mut seen: BTreeMap<u8, (usize, usize, u32)> = BTreeMap::new();
    for (i, s) in stats.iter().enumerate() {
        for (&color, &count) in &s.hist {
            if color == 0 {
                continue;
            }
            let e = seen.entry(color).or_insert((0, i, 0));
            e.0 += 1;
            e.1 = i;
            e.2 = count;
        }
    }
    let unique_colors_applicable = n > 1;
    let unique_entries: Vec<(u8, usize, u32)> = if unique_colors_applicable {
        seen.iter()
            .filter(|(_, (frames_using, _, _))| *frames_using == 1)
            .map(|(&color, &(_, frame, count))| (color, frame, count))
            .collect()
    } else {
        Vec::new()
    };
    let unique: Vec<Value> = unique_entries
        .iter()
        .map(|&(color, frame, count)| json!({"color": color, "frame": frame, "count": count}))
        .collect();

    // -----------------------------------------------------------------
    // Threshold gate — every violation names the anim, the offending
    // frame (the pair's "to" frame, or the frame a unique color lives in),
    // the metric, its value and the configured limit.
    // -----------------------------------------------------------------
    let violation = |frame: usize, metric: &str, value: Value, limit: Value| json!({ "anim": def.name, "frame": frame, "metric": metric, "value": value, "limit": limit });
    let mut violations = Vec::new();
    for m in &metrics {
        if let Some(max_drift) = thresholds.max_drift {
            if let Some(distance) = m.centroid_drift.map(|(_, _, d)| d) {
                if distance > max_drift {
                    violations.push(violation(
                        m.to,
                        "centroid_drift",
                        json!(round2(distance)),
                        json!(max_drift),
                    ));
                }
            }
        }
        if let Some(max_area_var) = thresholds.max_area_var {
            if let Some(pct) = m.area_drift_pct {
                if pct.abs() > max_area_var {
                    violations.push(violation(
                        m.to,
                        "area_drift_pct",
                        json!(round2(pct.abs())),
                        json!(max_area_var),
                    ));
                }
            }
        }
        if let Some(max_changed) = thresholds.max_changed {
            if m.changed_pixels as u32 > max_changed {
                violations.push(violation(
                    m.to,
                    "changed_pixels",
                    json!(m.changed_pixels),
                    json!(max_changed),
                ));
            }
        }
    }
    if thresholds.no_unique_colors {
        for &(color, frame, _count) in &unique_entries {
            violations.push(violation(frame, "unique_color", json!(color), json!(0)));
        }
    }

    let worst_drift = metrics
        .iter()
        .filter_map(|m| m.centroid_drift.map(|(_, _, d)| d))
        .fold(None, |acc: Option<f64>, d| {
            Some(acc.map_or(d, |a| a.max(d)))
        });
    let worst_changed = metrics.iter().map(|m| m.changed_pixels as u32).max();
    let summary = AnimSummary {
        anim: def.name.clone(),
        frame_count: n,
        worst_drift: worst_drift.map(round2),
        worst_changed,
        unique_colors: unique_entries.len(),
        unique_colors_applicable,
    };

    let value = json!({
        "anim": def.name,
        "sprite": def.sprite,
        "fps": def.fps,
        "loop": def.looped,
        "size": [sprite.size.0, sprite.size.1],
        "frame_size": [u32::from(sprite.size.0) * 8, u32::from(sprite.size.1) * 8],
        "anchor": [anchor.0, anchor.1],
        "frames": frames_json,
        "pairs": pairs,
        "colors_unique_to_single_frame": unique,
        "unique_color_analysis": {
            "applicable": unique_colors_applicable,
            "reason": if unique_colors_applicable { Value::Null } else { json!("requires_at_least_two_frames") },
        },
    });

    Ok((value, violations, summary))
}

/// `sprite dump` — print `target`'s resolved region as palette text, top
/// to bottom, exactly the cart's own `__sprites__` alphabet (one character
/// per pixel), preceded by a `#`-comment header naming the
/// region's pixel-space coordinates on the 128x128 sheet. Frame resolution
/// matches `sprite edit`, `sprite poke`, and `sprite render`: animation
/// targets index their declared frame list (including `frames_rect` and
/// explicit `tx:ty` entries), while plain sprite targets retain raw
/// sprite-relative frame semantics. A `dump` and matching `poke` therefore
/// always agree on where the pixels live.
///
/// The header is a comment specifically so `sprite dump | sprite poke
/// --stdin` round-trips without the caller having to strip it first —
/// `poke --stdin` skips `#`-prefixed lines for exactly this reason.
pub fn dump(cart: &Cart, target: &str, frame: u8) -> Result<String, String> {
    let (x0, y0, w, h) = resolve_rect(cart, target, frame)?;
    let sheet = cart.sprites();
    let mut out = format!("# x={x0} y={y0} w={w} h={h}\n");
    for j in 0..h {
        let row: String = (0..w)
            .map(|i| {
                let v = sheet[(y0 + j) as usize * SHEET_W + (x0 + i) as usize];
                console_core::color_char(v)
            })
            .collect();
        out.push_str(&row);
        out.push('\n');
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// CLI entry for the view-side sprite commands. `args[0]` is the command
/// name (`render`, `strip`, ...), `args[1]` the cart path. Returns the
/// process exit code.
pub fn cli_view(args: &[String]) -> i32 {
    match run_view(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("{}", super::SPRITE_USAGE);
            2
        }
    }
}

/// Flags accepted across the view commands (each command validates which of
/// them make sense for it). `max_drift`/`max_area_var`/`max_changed`/
/// `no_unique_colors`/`summary` are `lint`-only.
struct Flags {
    frame: Option<u32>,
    zoom: u32,
    grid: bool,
    indices: bool,
    anchor: bool,
    all: bool,
    out: Option<String>,
    max_drift: Option<f64>,
    max_area_var: Option<f64>,
    max_changed: Option<u32>,
    no_unique_colors: bool,
    summary: bool,
    positional: Vec<String>,
}

fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags {
        frame: None,
        zoom: DEFAULT_ZOOM,
        grid: false,
        indices: false,
        anchor: false,
        all: false,
        out: None,
        max_drift: None,
        max_area_var: None,
        max_changed: None,
        no_unique_colors: false,
        summary: false,
        positional: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--frame" => f.frame = Some(next_u32(&mut it, "--frame")?),
            "--zoom" => f.zoom = next_u32(&mut it, "--zoom")?,
            "--grid" => f.grid = true,
            "--indices" => f.indices = true,
            "--anchor" => f.anchor = true,
            "--all" => f.all = true,
            "--max-drift" => f.max_drift = Some(next_f64(&mut it, "--max-drift")?),
            "--max-area-var" => f.max_area_var = Some(next_f64(&mut it, "--max-area-var")?),
            "--max-changed" => f.max_changed = Some(next_u32(&mut it, "--max-changed")?),
            "--no-unique-colors" => f.no_unique_colors = true,
            "--summary" => f.summary = true,
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

fn next_f64<'a>(it: &mut impl Iterator<Item = &'a String>, what: &str) -> Result<f64, String> {
    let v = it
        .next()
        .ok_or_else(|| format!("{what} requires a value"))?;
    v.parse()
        .map_err(|_| format!("invalid {what} value {v:?} (want a number)"))
}

/// Runs one `sprite <cmd> ...` invocation and returns the process exit code
/// on success (0 for every command except a threshold-gated `lint` that
/// found a violation, which is 1) — errors (bad args, cart parse failures,
/// ...) are `Err` and become exit code 2 in [`cli_view`].
fn run_view(args: &[String]) -> Result<i32, String> {
    let cmd = args.first().map(String::as_str).unwrap_or_default();
    let flags = parse_flags(&args[1..])?;
    let cart_path = flags
        .positional
        .first()
        .ok_or_else(|| format!("sprite {cmd} requires a cart path"))?;
    let text = std::fs::read_to_string(cart_path)
        .map_err(|e| format!("cannot read {cart_path:?}: {e}"))?;
    let cart = Cart::parse(&text).map_err(|e| e.to_string())?;
    let rest = &flags.positional[1..];

    if cmd == "lint" {
        let thresholds = LintThresholds {
            max_drift: flags.max_drift,
            max_area_var: flags.max_area_var,
            max_changed: flags.max_changed,
            no_unique_colors: flags.no_unique_colors,
        };
        if flags.summary {
            let (summaries, violations, violated) = lint_summary(&cart, rest, &thresholds)?;
            for s in &summaries {
                println!("{}", s.line());
            }
            for v in &violations {
                println!(
                    "  violation: anim={} frame={} metric={} value={} limit={}",
                    v["anim"], v["frame"], v["metric"], v["value"], v["limit"]
                );
            }
            return Ok(i32::from(violated));
        }
        let (value, violated) = lint_gated(&cart, rest, &thresholds)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
        );
        return Ok(i32::from(violated));
    }

    if cmd == "gif" {
        let anim = one_positional(rest, cmd, "<anim>")?;
        let out = flags
            .out
            .as_deref()
            .ok_or_else(|| format!("sprite {cmd} requires -o <out.gif>"))?;
        let opts = GifOpts {
            zoom: flags.zoom,
            grid: flags.grid,
            anchor: flags.anchor,
        };
        let result = gif(&cart, anim, &opts)?;
        crate::artifact::write(out, &result.bytes)?;
        println!(
            "wrote {out} ({}x{}, {} frame(s), zoom {})",
            result.width, result.height, result.frames, flags.zoom
        );
        return Ok(0);
    }

    if cmd == "dump" {
        let target = one_positional(rest, cmd, "<target>")?;
        let frame = match flags.frame {
            Some(f) => u8::try_from(f).map_err(|_| format!("--frame {f} out of range 0-255"))?,
            None => 0,
        };
        print!("{}", dump(&cart, target, frame)?);
        return Ok(0);
    }

    if cmd == "atlas" {
        if !rest.is_empty() {
            return Err(format!("sprite atlas takes no target, got {rest:?}"));
        }
        let out = flags
            .out
            .as_deref()
            .ok_or("sprite atlas requires -o <out.png>")?;
        let result = atlas(&cart, flags.zoom, flags.grid)?;
        crate::artifact::write(out, &result.image.png)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&result.report).map_err(|e| e.to_string())?
        );
        return Ok(0);
    }

    let out = flags
        .out
        .as_deref()
        .ok_or_else(|| format!("sprite {cmd} requires -o <out.png>"))?;

    let image = match cmd {
        "render" => {
            let target = one_positional(rest, cmd, "<target>")?;
            render(
                &cart,
                target,
                &RenderOpts {
                    frame: flags.frame,
                    zoom: flags.zoom,
                    grid: flags.grid,
                    indices: flags.indices,
                    anchor: flags.anchor,
                },
            )?
        }
        "strip" => {
            let anim = one_positional(rest, cmd, "<anim>")?;
            strip(&cart, anim, flags.zoom, flags.anchor)?
        }
        "onion" => {
            let anim = one_positional(rest, cmd, "<anim>")?;
            let opts = OverlayOpts {
                zoom: flags.zoom,
                grid: flags.grid,
                anchor: flags.anchor,
            };
            if flags.all {
                if flags.frame.is_some() {
                    return Err("sprite onion --all covers every frame; omit --frame".into());
                }
                onion_all(&cart, anim, &opts)?
            } else {
                let frame = flags.frame.ok_or(
                    "sprite onion requires --frame N (which frame to centre the skin on), or --all for a contact sheet of every frame",
                )?;
                onion(&cart, anim, frame, &opts)?
            }
        }
        "diff" => {
            if rest.len() != 3 {
                return Err("sprite diff wants <anim> <frameA> <frameB>".into());
            }
            let a = rest[1]
                .parse()
                .map_err(|_| format!("invalid <frameA> {:?}", rest[1]))?;
            let b = rest[2]
                .parse()
                .map_err(|_| format!("invalid <frameB> {:?}", rest[2]))?;
            diff(&cart, &rest[0], a, b, flags.zoom)?
        }
        "ghost" => {
            let anim = one_positional(rest, cmd, "<anim>")?;
            ghost(
                &cart,
                anim,
                &OverlayOpts {
                    zoom: flags.zoom,
                    grid: flags.grid,
                    anchor: flags.anchor,
                },
            )?
        }
        other => return Err(format!("unknown sprite command {other:?}")),
    };

    crate::artifact::write(out, &image.png)?;
    println!(
        "wrote {out} ({}x{}, {} frame(s), zoom {})",
        image.width, image.height, image.frames, flags.zoom
    );
    Ok(0)
}

fn one_positional<'a>(rest: &'a [String], cmd: &str, what: &str) -> Result<&'a str, String> {
    match rest {
        [only] => Ok(only.as_str()),
        [] => Err(format!("sprite {cmd} requires {what}")),
        _ => Err(format!(
            "sprite {cmd} takes exactly one {what}, got {rest:?}"
        )),
    }
}

// ---------------------------------------------------------------------------
// Target / frame resolution
// ---------------------------------------------------------------------------

/// `pub(crate)`: `map::view` validates its own `--zoom` against the same
/// bound this module uses.
pub(crate) fn check_zoom(zoom: u32) -> Result<u32, String> {
    if zoom == 0 || zoom > MAX_ZOOM {
        return Err(format!("zoom must be 1-{MAX_ZOOM}, got {zoom}"));
    }
    Ok(zoom)
}

fn check_pos(pos: u32, len: usize, anim: &str, what: &str) -> Result<usize, String> {
    let pos = pos as usize;
    if pos >= len {
        return Err(format!(
            "{what} {pos} out of range: anim {anim:?} has {len} frame(s) (0-{})",
            len.saturating_sub(1)
        ));
    }
    Ok(pos)
}

/// Pixel rect for `render`: an anim target's `--frame` indexes the anim's
/// own frame list (going through [`AnimDef::resolve_frame`], the single
/// source of truth that also honors `frames_rect` relocation and explicit
/// `tx:ty` frames), anything else uses the raw sprite frame index.
fn render_rect(cart: &Cart, target: &Target, frame: u32) -> Result<(u32, u32, u32, u32), String> {
    if let Target::Sprite {
        anim: Some(name), ..
    } = target
    {
        let meta = cart.gfx_meta();
        let def = meta
            .anim(name)
            .ok_or_else(|| format!("anim {name:?} vanished from __gfx_meta__"))?;
        let sprite = meta
            .sprite(&def.sprite)
            .ok_or_else(|| format!("anim {name:?} names unknown sprite {:?}", def.sprite))?;
        let pos = check_pos(frame, def.frames.len(), name, "--frame")?;
        return def
            .resolve_frame(sprite, pos)
            .ok_or_else(|| format!("frame {pos} of anim {name:?} falls off the sheet"));
    }
    let frame = u8::try_from(frame).map_err(|_| format!("--frame {frame} out of range 0-255"))?;
    frame_pixel_rect(cart, target, frame)
}

/// Resolve an anim-only target (`strip`/`onion`/`diff`/`ghost`/`lint`) to its
/// def, its sprite and the decoded pixels of every frame.
fn anim_frames<'c>(
    cart: &'c Cart,
    name: &str,
) -> Result<(&'c AnimDef, &'c SpriteDef, Vec<Frame>), String> {
    let meta = cart.gfx_meta();
    let def = meta.anim(name).ok_or_else(|| {
        let known: Vec<&str> = meta.anims().map(|a| a.name.as_str()).collect();
        if known.is_empty() {
            format!(
                "{name:?} is not an anim, and this cart's __gfx_meta__ declares none \
                 (strip/onion/diff/ghost/lint need an anim, e.g. `player.walk`)"
            )
        } else {
            format!(
                "{name:?} is not an anim (strip/onion/diff/ghost/lint need one of: {})",
                known.join(", ")
            )
        }
    })?;
    let sprite = meta
        .sprite(&def.sprite)
        .ok_or_else(|| format!("anim {name:?} names unknown sprite {:?}", def.sprite))?;
    // Goes through `AnimDef::resolve_frame` (the single source of truth for
    // anim frame resolution), not `SpriteDef::frame_rect` directly, so
    // `frames_rect` relocation and explicit `tx:ty` frames are honored here
    // exactly like everywhere else (`render`, the parse-time validator).
    let frames = (0..def.frames.len())
        .map(|pos| {
            def.resolve_frame(sprite, pos)
                .map(|rect| read_rect(cart, rect))
                .ok_or_else(|| format!("frame {pos} of anim {name:?} falls off the sheet"))
        })
        .collect::<Result<Vec<Frame>, String>>()?;
    Ok((def, sprite, frames))
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// One frame's palette indices, `w`x`h`, copied out of the sheet.
///
/// `pub(crate)`, along with [`read_rect`] and [`draw_frame`] below: `map::view`
/// reuses exactly this pixel path for one map cell's 8x8 tile, so a rendered
/// map tile is pixel-identical to `sprite render <tx,ty,8,8>` of the same
/// sheet rect.
pub(crate) struct Frame {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Frame {
    fn at(&self, x: u32, y: u32) -> u8 {
        self.px[(y * self.w + x) as usize]
    }
}

pub(crate) fn read_rect(cart: &Cart, rect: (u32, u32, u32, u32)) -> Frame {
    let (x0, y0, w, h) = rect;
    let sheet = cart.sprites();
    let mut px = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let row = (y0 + y) as usize * SHEET_W;
        for x in 0..w {
            px.push(sheet[row + (x0 + x) as usize]);
        }
    }
    Frame { w, h, px }
}

/// A common bounding box for a set of frames in which every frame's anchor
/// lands on the same cell coordinate — that is what keeps a strip's
/// baselines flat and an onion skin's frames registered.
struct Layout {
    cell_w: u32,
    cell_h: u32,
    /// Anchor position inside a cell, in sheet pixels.
    anchor: (u32, u32),
    /// Per-frame top-left offset inside its cell, in sheet pixels.
    offsets: Vec<(u32, u32)>,
}

fn align(frames: &[Frame], anchor: (i32, i32)) -> Layout {
    // Work in anchor-relative coordinates: frame i spans [-ax, w-ax). The
    // anchor point itself (0,0) is always inside the box so the crosshair
    // has somewhere to land even for out-of-bounds anchors.
    let (ax, ay) = anchor;
    let mut min = (0.min(-ax), 0.min(-ay));
    let mut max = (1.max(-ax), 1.max(-ay));
    for f in frames {
        max.0 = max.0.max(f.w as i32 - ax);
        max.1 = max.1.max(f.h as i32 - ay);
    }
    min.0 = min.0.min(-ax);
    min.1 = min.1.min(-ay);
    Layout {
        cell_w: (max.0 - min.0) as u32,
        cell_h: (max.1 - min.1) as u32,
        anchor: ((-min.0) as u32, (-min.1) as u32),
        offsets: frames
            .iter()
            .map(|_| ((-ax - min.0) as u32, (-ay - min.1) as u32))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

/// `pub(crate)`: `map::view` builds `Rect`s of its own to reuse [`Canvas`]'s
/// `fill`/`blend_fill` and [`draw_grid`] directly.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rect {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) w: u32,
    pub(crate) h: u32,
}

/// `pub(crate)`: the checkerboard-backed RGBA canvas every render command
/// draws into. `map::view` reuses it as-is (checkerboard, `fill`/`blend_fill`,
/// PNG `finish`) so a map render shares byte-for-byte the same background and
/// PNG encoding as the sprite tools, per SPEC's "tile 0 = checkerboard like
/// sprite tools render transparency".
pub(crate) struct Canvas {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

impl Canvas {
    /// A canvas pre-filled with the dark checkerboard. `zoom` sets the
    /// checker cell size: [`CHECKER_LOGICAL`] sheet pixels square.
    pub(crate) fn new(w: u32, h: u32, zoom: u32) -> Canvas {
        let cell = (CHECKER_LOGICAL * zoom).max(1);
        let mut c = Canvas {
            w,
            h,
            rgba: vec![255; (w as usize) * (h as usize) * 4],
        };
        for y in 0..h {
            for x in 0..w {
                let tone = if ((x / cell) + (y / cell)) % 2 == 0 {
                    CHECKER_A
                } else {
                    CHECKER_B
                };
                c.set(x, y, tone);
            }
        }
        c
    }

    fn idx(&self, x: u32, y: u32) -> usize {
        ((y as usize) * (self.w as usize) + x as usize) * 4
    }

    fn set(&mut self, x: u32, y: u32, rgb: [u8; 3]) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = self.idx(x, y);
        self.rgba[i] = rgb[0];
        self.rgba[i + 1] = rgb[1];
        self.rgba[i + 2] = rgb[2];
        self.rgba[i + 3] = 255;
    }

    pub(crate) fn get(&self, x: u32, y: u32) -> [u8; 3] {
        let i = self.idx(x, y);
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]]
    }

    fn blend(&mut self, x: u32, y: u32, rgb: [u8; 3], alpha: f32) {
        if x >= self.w || y >= self.h {
            return;
        }
        let dst = self.get(x, y);
        let out = [
            mix(dst[0], rgb[0], alpha),
            mix(dst[1], rgb[1], alpha),
            mix(dst[2], rgb[2], alpha),
        ];
        self.set(x, y, out);
    }

    pub(crate) fn fill(&mut self, r: Rect, rgb: [u8; 3]) {
        for y in r.y..r.y.saturating_add(r.h) {
            for x in r.x..r.x.saturating_add(r.w) {
                self.set(x, y, rgb);
            }
        }
    }

    fn blend_fill(&mut self, r: Rect, rgb: [u8; 3], alpha: f32) {
        for y in r.y..r.y.saturating_add(r.h) {
            for x in r.x..r.x.saturating_add(r.w) {
                self.blend(x, y, rgb, alpha);
            }
        }
    }

    /// Consume the canvas for its raw RGBA buffer, discarding the dimensions
    /// (the caller already has them) — used by [`gif`], which needs a `&mut
    /// [u8]` to hand `gif::Frame::from_rgba_speed` rather than an encoded PNG.
    fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }

    pub(crate) fn finish(self, frames: usize) -> Image {
        let png = encode_png_rgba(&self.rgba, self.w, self.h);
        Image {
            png,
            rgba: self.rgba,
            width: self.w,
            height: self.h,
            frames,
        }
    }
}

fn mix(dst: u8, src: u8, alpha: f32) -> u8 {
    let v = f32::from(dst) * (1.0 - alpha) + f32::from(src) * alpha;
    v.round().clamp(0.0, 255.0) as u8
}

fn dim(rgb: [u8; 3], factor: f32) -> [u8; 3] {
    [
        (f32::from(rgb[0]) * factor).round().clamp(0.0, 255.0) as u8,
        (f32::from(rgb[1]) * factor).round().clamp(0.0, 255.0) as u8,
        (f32::from(rgb[2]) * factor).round().clamp(0.0, 255.0) as u8,
    ]
}

fn encode_png_rgba(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .expect("PNG header write cannot fail on an in-memory buffer");
        writer
            .write_image_data(rgba)
            .expect("PNG data write cannot fail on an in-memory buffer");
    }
    bytes
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Opaque frame pixels; color 0 is left as-is so the checkerboard (or a
/// ghost already painted underneath) shows through.
///
/// `pub(crate)`: `map::view` calls this directly for each non-empty map
/// cell's 8x8 tile, so per-pixel color-0 transparency within a tile matches
/// `spr()`'s rules exactly (only the whole-cell tile-0 skip is map-specific,
/// and that check happens before this is ever called).
pub(crate) fn draw_frame(
    canvas: &mut Canvas,
    frame: &Frame,
    origin: (u32, u32),
    zoom: u32,
    palette: &PreviewPalette,
) {
    for y in 0..frame.h {
        for x in 0..frame.w {
            let v = frame.at(x, y);
            if v == 0 {
                continue;
            }
            canvas.fill(
                Rect {
                    x: origin.0 + x * zoom,
                    y: origin.1 + y * zoom,
                    w: zoom,
                    h: zoom,
                },
                preview_rgb(palette, v),
            );
        }
    }
}

fn preview_rgb(palette: &PreviewPalette, source: u8) -> [u8; 3] {
    PALETTE[usize::from(palette.resolve(source & COLOR_MASK))]
}

/// A frame's silhouette painted in one flat tint at `alpha` (onion skins).
fn tint_frame(
    canvas: &mut Canvas,
    frame: &Frame,
    origin: (u32, u32),
    zoom: u32,
    tint: [u8; 3],
    alpha: f32,
) {
    for y in 0..frame.h {
        for x in 0..frame.w {
            if frame.at(x, y) == 0 {
                continue;
            }
            canvas.blend_fill(
                Rect {
                    x: origin.0 + x * zoom,
                    y: origin.1 + y * zoom,
                    w: zoom,
                    h: zoom,
                },
                tint,
                alpha,
            );
        }
    }
}

/// Tile (8 sheet pixels) boundaries, including the closing edges.
///
/// `pub(crate)`: `map::view`'s `--grid` reuses this unmodified — a map cell
/// is exactly one tile, so the same 8-sheet-pixel step draws cell boundaries.
pub(crate) fn draw_grid(canvas: &mut Canvas, cell: Rect, zoom: u32) {
    let step = 8 * zoom;
    let mut x = cell.x;
    while x <= cell.x + cell.w {
        for y in cell.y..=cell.y + cell.h {
            canvas.blend(x, y, GRID_RGB, GRID_ALPHA);
        }
        x += step;
    }
    let mut y = cell.y;
    while y <= cell.y + cell.h {
        for x in cell.x..=cell.x + cell.w {
            canvas.blend(x, y, GRID_RGB, GRID_ALPHA);
        }
        y += step;
    }
}

/// Crosshair in palette color 4 through the anchor pixel, clipped to `cell`,
/// with the anchor's own pixel cell outlined.
fn draw_anchor(canvas: &mut Canvas, cell: Rect, anchor: (i32, i32), zoom: u32) {
    let color = PALETTE[4];
    // Anchors deliberately accept the full i32 domain and may sit outside a
    // sprite. Widen before multiplying by zoom so diagnostics never panic or
    // wrap merely because metadata names a far-away anchor.
    let z = i64::from(zoom);
    let cx = i64::from(cell.x) + i64::from(anchor.0) * z + z / 2;
    let cy = i64::from(cell.y) + i64::from(anchor.1) * z + z / 2;
    let (x0, x1) = (i64::from(cell.x), i64::from(cell.x + cell.w) - 1);
    let (y0, y1) = (i64::from(cell.y), i64::from(cell.y + cell.h) - 1);
    let arm = (z * 2).max(4);

    for d in -arm..=arm {
        let x = cx + d;
        if x >= x0 && x <= x1 && cy >= y0 && cy <= y1 {
            canvas.set(x as u32, cy as u32, color);
        }
        let y = cy + d;
        if y >= y0 && y <= y1 && cx >= x0 && cx <= x1 {
            canvas.set(cx as u32, y as u32, color);
        }
    }
    // Outline of the anchor's pixel cell, so its exact pixel is unambiguous.
    let (bx, by) = (
        i64::from(cell.x) + i64::from(anchor.0) * z,
        i64::from(cell.y) + i64::from(anchor.1) * z,
    );
    for d in 0..z {
        for (px, py) in [
            (bx + d, by),
            (bx + d, by + z - 1),
            (bx, by + d),
            (bx + z - 1, by + d),
        ] {
            if px >= x0 && px <= x1 && py >= y0 && py <= y1 {
                canvas.set(px as u32, py as u32, color);
            }
        }
    }
}

/// Two 3x5 hex glyphs per pixel cell. Needs `zoom >= 8` to fit; below that the
/// flag is silently a no-op (the render is still useful, just unlabelled).
fn draw_indices(canvas: &mut Canvas, frame: &Frame, origin: (u32, u32), zoom: u32) {
    if zoom < 8 {
        return;
    }
    for y in 0..frame.h {
        for x in 0..frame.w {
            let v = usize::from(frame.at(x, y));
            let cx = origin.0 + x * zoom;
            let cy = origin.1 + y * zoom;
            let ink = if luminance(canvas.get(cx + zoom / 2, cy + zoom / 2)) < 128.0 {
                INK_LIGHT
            } else {
                INK_DARK
            };
            let gx = cx + (zoom - 7) / 2;
            let gy = cy + (zoom - 5) / 2;
            for (digit, dx) in [(v >> 4, 0), (v & 0xf, 4)] {
                for (row, bits) in GLYPHS[digit].iter().enumerate() {
                    for bit in 0..3u32 {
                        if bits & (1 << (2 - bit)) != 0 {
                            canvas.set(gx + dx + bit, gy + row as u32, ink);
                        }
                    }
                }
            }
        }
    }
}

/// Scale factor for [`draw_label`]'s glyphs (independent of the image's own
/// `zoom`, so frame numbers stay legible at any zoom level).
const LABEL_SCALE: u32 = 2;
/// Height in device pixels of the caption band `onion --all` reserves below
/// each cell: a 5-row glyph at [`LABEL_SCALE`] plus a pixel of margin above
/// and below.
const LABEL_HEIGHT: u32 = 5 * LABEL_SCALE + 2 * LABEL_SCALE;

/// Draw `label` (read as decimal digits; anything else is skipped) left
/// aligned near the top of `cell`, reusing the `--indices` 3x5 digit glyphs
/// scaled up by [`LABEL_SCALE`] so they read at a glance. Used by `onion
/// --all` to number each frame in its contact sheet.
fn draw_label(canvas: &mut Canvas, cell: Rect, label: &str) {
    let mut x = cell.x + LABEL_SCALE;
    let y = cell.y + LABEL_SCALE;
    for ch in label.chars() {
        let Some(d) = ch.to_digit(10) else { continue };
        for (row, bits) in GLYPHS[d as usize].iter().enumerate() {
            for bit in 0..3u32 {
                if bits & (1 << (2 - bit)) != 0 {
                    canvas.fill(
                        Rect {
                            x: x + bit * LABEL_SCALE,
                            y: y + row as u32 * LABEL_SCALE,
                            w: LABEL_SCALE,
                            h: LABEL_SCALE,
                        },
                        INK_LIGHT,
                    );
                }
            }
        }
        x += 4 * LABEL_SCALE; // glyph width (3) + 1-unit gap, scaled
    }
}

/// `pub(crate)`: `map::view` picks its `--ids` glyph ink the same way
/// `--indices` does here — light ink on a dark cell, dark ink on a light one.
pub(crate) fn luminance(rgb: [u8; 3]) -> f32 {
    0.299 * f32::from(rgb[0]) + 0.587 * f32::from(rgb[1]) + 0.114 * f32::from(rgb[2])
}

/// 3x5 hex digits `0`-`F`, one `u8` bitmask per row (bit 2 = leftmost pixel).
///
/// `pub(crate)`: `map::view`'s `--ids` overlay draws two of these side by
/// side (high/low nibble) per non-empty cell to label its tile id.
pub(crate) const GLYPHS: [[u8; 5]; 16] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b001, 0b001, 0b001], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
    [0b111, 0b101, 0b111, 0b101, 0b101], // A
    [0b110, 0b101, 0b110, 0b101, 0b110], // B
    [0b111, 0b100, 0b100, 0b100, 0b111], // C
    [0b110, 0b101, 0b101, 0b101, 0b110], // D
    [0b111, 0b100, 0b111, 0b100, 0b111], // E
    [0b111, 0b100, 0b111, 0b100, 0b100], // F
];

// ---------------------------------------------------------------------------
// Lint maths
// ---------------------------------------------------------------------------

struct FrameStats {
    area: u32,
    /// `[x0, y0, x1, y1]`, inclusive, relative to the anchor.
    bbox: Option<[i32; 4]>,
    /// Mean of the non-zero pixel coordinates, relative to the anchor.
    centroid: Option<(f64, f64)>,
    hist: BTreeMap<u8, u32>,
}

fn frame_stats(frame: &Frame, anchor: (i32, i32)) -> FrameStats {
    let mut hist: BTreeMap<u8, u32> = BTreeMap::new();
    let mut area = 0u32;
    let mut sum = (0i64, 0i64);
    let mut bounds: Option<[i32; 4]> = None;
    for y in 0..frame.h {
        for x in 0..frame.w {
            let v = frame.at(x, y);
            *hist.entry(v).or_insert(0) += 1;
            if v == 0 {
                continue;
            }
            area += 1;
            sum.0 += i64::from(x);
            sum.1 += i64::from(y);
            let (px, py) = (x as i32, y as i32);
            bounds = Some(match bounds {
                None => [px, py, px, py],
                Some([x0, y0, x1, y1]) => [x0.min(px), y0.min(py), x1.max(px), y1.max(py)],
            });
        }
    }
    let centroid = (area > 0).then(|| {
        (
            sum.0 as f64 / f64::from(area) - f64::from(anchor.0),
            sum.1 as f64 / f64::from(area) - f64::from(anchor.1),
        )
    });
    FrameStats {
        area,
        bbox: bounds
            .map(|[x0, y0, x1, y1]| [x0 - anchor.0, y0 - anchor.1, x1 - anchor.0, y1 - anchor.1]),
        centroid,
        hist,
    }
}

fn bbox_json(bbox: Option<[i32; 4]>) -> Value {
    match bbox {
        None => Value::Null,
        Some([x0, y0, x1, y1]) => json!({
            "x0": x0, "y0": y0, "x1": x1, "y1": y1,
            "w": x1 - x0 + 1, "h": y1 - y0 + 1,
        }),
    }
}

fn hist_json(hist: &BTreeMap<u8, u32>) -> Value {
    let mut map = serde_json::Map::new();
    for (&color, &count) in hist {
        map.insert(color.to_string(), json!(count));
    }
    Value::Object(map)
}

/// The numbers behind one `pairs[]` entry, computed once and shared by
/// [`pair_json`] (the report) and `lint_anim_gated`'s threshold gate (the
/// CI-friendly reading of the same numbers) — so a metric's JSON rendering
/// and its violation check can never disagree about the underlying value.
struct PairMetrics {
    from: usize,
    to: usize,
    changed_pixels: usize,
    area_from: u32,
    area_to: u32,
    area_drift_pct: Option<f64>,
    /// `(dx, dy, distance)`, unrounded.
    centroid_drift: Option<(f64, f64, f64)>,
    /// `[dx0, dy0, dx1, dy1, dw, dh]`.
    bbox_drift: Option<[i32; 6]>,
}

fn pair_metrics(
    a: &Frame,
    b: &Frame,
    sa: &FrameStats,
    sb: &FrameStats,
    (from, to): (usize, usize),
) -> PairMetrics {
    let changed_pixels = if a.w == b.w && a.h == b.h {
        a.px.iter().zip(&b.px).filter(|(x, y)| x != y).count()
    } else {
        // Different-sized frames cannot happen for one sprite today, but be
        // explicit rather than panicking if that ever changes.
        a.px.len().max(b.px.len())
    };
    let area_drift_pct = (sa.area > 0)
        .then(|| (f64::from(sb.area) - f64::from(sa.area)) / f64::from(sa.area) * 100.0);
    let centroid_drift = match (sa.centroid, sb.centroid) {
        (Some(ca), Some(cb)) => {
            let (dx, dy) = (cb.0 - ca.0, cb.1 - ca.1);
            Some((dx, dy, (dx * dx + dy * dy).sqrt()))
        }
        _ => None,
    };
    let bbox_drift = match (sa.bbox, sb.bbox) {
        (Some(ba), Some(bb)) => Some([
            bb[0] - ba[0],
            bb[1] - ba[1],
            bb[2] - ba[2],
            bb[3] - ba[3],
            (bb[2] - bb[0]) - (ba[2] - ba[0]),
            (bb[3] - bb[1]) - (ba[3] - ba[1]),
        ]),
        _ => None,
    };
    PairMetrics {
        from,
        to,
        changed_pixels,
        area_from: sa.area,
        area_to: sb.area,
        area_drift_pct,
        centroid_drift,
        bbox_drift,
    }
}

fn pair_json(m: &PairMetrics) -> Value {
    json!({
        "from": m.from,
        "to": m.to,
        "changed_pixels": m.changed_pixels,
        "area_from": m.area_from,
        "area_to": m.area_to,
        "area_drift_pct": m.area_drift_pct.map(round2),
        "centroid_drift": m.centroid_drift.map(|(dx, dy, distance)| json!({
            "dx": round2(dx),
            "dy": round2(dy),
            "distance": round2(distance),
        })).unwrap_or(Value::Null),
        "bbox_drift": m.bbox_drift.map(|[dx0, dy0, dx1, dy1, dw, dh]| json!({
            "dx0": dx0, "dy0": dy0, "dx1": dx1, "dy1": dy1, "dw": dw, "dh": dh,
        })).unwrap_or(Value::Null),
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

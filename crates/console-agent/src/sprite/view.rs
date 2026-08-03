//! Inspection renders (render/strip/onion/diff/ghost) and numeric lints for
//! `__gfx_meta__` sprites and anims — SPEC.md "Sprite & animation authoring
//! (PoC v1)", the inspection-tools table.
//!
//! Everything here reads a [`Cart`] directly; nothing steps the console. The
//! same entry points back both the CLI (`console-agent sprite ...`, via
//! [`cli_view`]) and the RPC verbs in `rpc.rs`, so the two surfaces can never
//! drift: [`render`], [`strip`], [`onion`], [`diff`], [`ghost`], [`lint`].
//! [`gif`] is CLI-only (an animated preview has no single-frame RPC shape to
//! mirror) but otherwise follows the same conventions.
//!
//! Render conventions, shared by every image command:
//!
//! - **zoom** defaults to [`DEFAULT_ZOOM`] (8) — each sheet pixel becomes a
//!   `zoom`x`zoom` block.
//! - **color 0 is transparent** and lets a dark checkerboard show through.
//!   Checker cells are 4 *logical* (sheet) pixels square, so the backdrop
//!   stays legible instead of shimmering at high zoom.
//! - `--grid` overlays tile (8px) boundaries, `--indices` writes the palette
//!   index into each pixel cell as a 3x5 hex glyph (only when `zoom >= 6`,
//!   otherwise silently skipped), `--anchor` draws a crosshair in palette
//!   color 4 at the sprite's anchor pixel. `onion` and `ghost` accept
//!   `--grid`/`--anchor` too (mirroring `render`/`strip`), via
//!   [`OverlayOpts`]. `onion --all` renders a contact sheet — every frame of
//!   the anim side by side, each with its own onion skin and a frame-number
//!   caption (see [`onion_all`]).

use std::collections::BTreeMap;

use console_core::{AnimDef, Cart, PALETTE, SHEET_W, SpriteDef};
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
const INK_LIGHT: [u8; 3] = [0xf4, 0xf4, 0xf4];
const INK_DARK: [u8; 3] = [0x1a, 0x1c, 0x2c];

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
    draw_frame(&mut canvas, &frame, (0, 0), zoom);
    if opts.indices {
        draw_indices(&mut canvas, &frame, (0, 0), zoom);
    }
    if opts.grid {
        draw_grid(&mut canvas, cell, zoom);
    }
    if opts.anchor {
        let anchor = target_sprite(cart, &target).map(|d| d.anchor).ok_or(
            "--anchor needs a sprite or anim target; raw tx,ty,w,h rects have no anchor",
        )?;
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
        draw_frame(&mut canvas, frame, (cell_x + ox * zoom, oy * zoom), zoom);
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
    draw_frame(&mut canvas, &frames[pos], (ox * zoom, oy * zoom), zoom);

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
        draw_anchor(&mut canvas, cell, (layout.anchor.0 as i32, layout.anchor.1 as i32), zoom);
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
        draw_frame(&mut canvas, &frames[i], (cell_x + ox * zoom, oy * zoom), zoom);

        let cell = Rect { x: cell_x, y: 0, w: cell_w, h: cell_h };
        if opts.grid {
            draw_grid(&mut canvas, cell, zoom);
        }
        if opts.anchor {
            draw_anchor(&mut canvas, cell, (layout.anchor.0 as i32, layout.anchor.1 as i32), zoom);
        }
        draw_label(
            &mut canvas,
            Rect { x: cell_x, y: cell_h, w: cell_w, h: LABEL_HEIGHT },
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
                canvas.fill(block, dim(PALETTE[usize::from(vb) & 15], DIFF_DIM));
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
                    PALETTE[usize::from(v) & 15],
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
        draw_anchor(&mut canvas, cell, (layout.anchor.0 as i32, layout.anchor.1 as i32), zoom);
    }
    Ok(canvas.finish(frames.len()))
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
        GifOpts { zoom: DEFAULT_ZOOM, grid: false, anchor: false }
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
    let cell = Rect { x: 0, y: 0, w: width, h: height };

    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, width16, height16, &[])
            .map_err(|e| format!("gif: {e}"))?;
        encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| format!("gif: {e}"))?;

        for (i, frame) in frames.iter().enumerate() {
            let (ox, oy) = layout.offsets[i];
            let mut canvas = Canvas::new(width, height, zoom);
            draw_frame(&mut canvas, frame, (ox * zoom, oy * zoom), zoom);
            if opts.grid {
                draw_grid(&mut canvas, cell, zoom);
            }
            if opts.anchor {
                draw_anchor(&mut canvas, cell, (layout.anchor.0 as i32, layout.anchor.1 as i32), zoom);
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
    Ok(GifOutput { bytes, width, height, frames: frames.len() })
}

/// GIF frame delay is in units of 10ms. `fps` converts as `1000/fps`,
/// rounded to the nearest 10ms step, with a floor of 2 (20ms) — GIF cannot
/// express anything finer, and a delay of 0 or 1 is "as fast as possible" in
/// most viewers rather than the intended frame rate.
fn gif_delay_from_fps(fps: u8) -> u16 {
    let hundredths = (1000.0 / f64::from(fps.max(1)) / 10.0).round();
    (hundredths as u16).max(2)
}

/// `sprite lint` — pure numbers, no judgements: per-frame silhouette area,
/// anchor-relative bbox and centroid, palette histogram; per consecutive
/// (loop-aware) pair the changed-pixel count and the area/centroid/bbox
/// drift; plus the colors that appear in exactly one frame.
///
/// `names` empty means "every anim in the cart". The result is
/// `{"anims": [ ... ]}` — one object per anim, in the order requested (or
/// sorted by name when reporting all of them).
pub fn lint(cart: &Cart, names: &[String]) -> Result<Value, String> {
    let wanted: Vec<String> = if names.is_empty() {
        cart.gfx_meta().anims().map(|a| a.name.clone()).collect()
    } else {
        names.to_vec()
    };

    let mut out = Vec::with_capacity(wanted.len());
    for name in &wanted {
        out.push(lint_anim(cart, name)?);
    }
    Ok(json!({ "anims": out }))
}

fn lint_anim(cart: &Cart, name: &str) -> Result<Value, String> {
    let (def, sprite, frames) = anim_frames(cart, name)?;
    let anchor = sprite.anchor;
    let stats: Vec<FrameStats> = frames.iter().map(|f| frame_stats(f, anchor)).collect();

    let frames_json: Vec<Value> = def
        .frames
        .iter()
        .zip(&stats)
        .enumerate()
        .map(|(i, (sheet_frame, s))| {
            json!({
                "index": i,
                "sprite_frame": sheet_frame,
                "silhouette_area": s.area,
                "bbox": bbox_json(s.bbox),
                "centroid": s.centroid.map(|(x, y)| json!([round2(x), round2(y)])),
                "palette": hist_json(&s.hist),
            })
        })
        .collect();

    // Consecutive pairs, plus the wrap-around pair when the anim loops.
    let n = frames.len();
    let mut pairs = Vec::new();
    for i in 0..n.saturating_sub(1) {
        pairs.push(pair_json(&frames[i], &frames[i + 1], &stats[i], &stats[i + 1], (i, i + 1)));
    }
    if def.looped && n > 1 {
        pairs.push(pair_json(
            &frames[n - 1],
            &frames[0],
            &stats[n - 1],
            &stats[0],
            (n - 1, 0),
        ));
    }

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
    let unique: Vec<Value> = seen
        .iter()
        .filter(|(_, (frames_using, _, _))| *frames_using == 1)
        .map(|(&color, &(_, frame, count))| json!({"color": color, "frame": frame, "count": count}))
        .collect();

    Ok(json!({
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
    }))
}

/// `sprite dump` — print `target`'s resolved region as rows of hex text, top
/// to bottom, exactly the cart's own `__sprites__` alphabet (lowercase hex
/// digits, one char per pixel), preceded by a `#`-comment header naming the
/// region's pixel-space coordinates on the 128x128 sheet. Frame resolution
/// matches `sprite edit`'s and `sprite poke`'s: `frame` is the raw sheet
/// frame index (an anim's own frame list is not consulted), so a `dump`'d
/// region and the `poke` that writes it back always agree on where the
/// pixels live.
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
                char::from_digit(u32::from(v & 0xF), 16).unwrap()
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
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("{}", super::SPRITE_USAGE);
            2
        }
    }
}

/// Flags accepted across the view commands (each command validates which of
/// them make sense for it).
struct Flags {
    frame: Option<u32>,
    zoom: u32,
    grid: bool,
    indices: bool,
    anchor: bool,
    all: bool,
    out: Option<String>,
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
            "-o" | "--out" => {
                f.out = Some(
                    it.next()
                        .ok_or("-o requires an output path")?
                        .clone(),
                );
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
    let v = it.next().ok_or_else(|| format!("{what} requires a value"))?;
    v.parse()
        .map_err(|_| format!("invalid {what} value {v:?} (want a non-negative integer)"))
}

fn run_view(args: &[String]) -> Result<(), String> {
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
        let value = lint(&cart, rest)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    if cmd == "gif" {
        let anim = one_positional(rest, cmd, "<anim>")?;
        let out = flags
            .out
            .as_deref()
            .ok_or_else(|| format!("sprite {cmd} requires -o <out.gif>"))?;
        let opts = GifOpts { zoom: flags.zoom, grid: flags.grid, anchor: flags.anchor };
        let result = gif(&cart, anim, &opts)?;
        std::fs::write(out, &result.bytes).map_err(|e| format!("cannot write {out:?}: {e}"))?;
        println!(
            "wrote {out} ({}x{}, {} frame(s), zoom {})",
            result.width, result.height, result.frames, flags.zoom
        );
        return Ok(());
    }

    if cmd == "dump" {
        let target = one_positional(rest, cmd, "<target>")?;
        let frame = match flags.frame {
            Some(f) => u8::try_from(f).map_err(|_| format!("--frame {f} out of range 0-255"))?,
            None => 0,
        };
        print!("{}", dump(&cart, target, frame)?);
        return Ok(());
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
            let opts = OverlayOpts { zoom: flags.zoom, grid: flags.grid, anchor: flags.anchor };
            if flags.all {
                if flags.frame.is_some() {
                    return Err(
                        "sprite onion --all covers every frame; omit --frame".into()
                    );
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
            ghost(&cart, anim, &OverlayOpts { zoom: flags.zoom, grid: flags.grid, anchor: flags.anchor })?
        }
        other => return Err(format!("unknown sprite command {other:?}")),
    };

    std::fs::write(out, &image.png).map_err(|e| format!("cannot write {out:?}: {e}"))?;
    println!(
        "wrote {out} ({}x{}, {} frame(s), zoom {})",
        image.width, image.height, image.frames, flags.zoom
    );
    Ok(())
}

fn one_positional<'a>(rest: &'a [String], cmd: &str, what: &str) -> Result<&'a str, String> {
    match rest {
        [only] => Ok(only.as_str()),
        [] => Err(format!("sprite {cmd} requires {what}")),
        _ => Err(format!("sprite {cmd} takes exactly one {what}, got {rest:?}")),
    }
}

// ---------------------------------------------------------------------------
// Target / frame resolution
// ---------------------------------------------------------------------------

fn check_zoom(zoom: u32) -> Result<u32, String> {
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
/// own frame list, anything else uses the raw sprite frame index.
fn render_rect(cart: &Cart, target: &Target, frame: u32) -> Result<(u32, u32, u32, u32), String> {
    if let Target::Sprite {
        anim: Some(name), ..
    } = target
    {
        let def = cart
            .gfx_meta()
            .anim(name)
            .ok_or_else(|| format!("anim {name:?} vanished from __gfx_meta__"))?;
        let pos = check_pos(frame, def.frames.len(), name, "--frame")?;
        return frame_pixel_rect(cart, target, def.frames[pos]);
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
    let frames = def
        .frames
        .iter()
        .map(|&i| {
            sprite
                .frame_rect(i)
                .map(|rect| read_rect(cart, rect))
                .ok_or_else(|| {
                    format!("frame {i} of sprite {:?} falls off the sheet", sprite.name)
                })
        })
        .collect::<Result<Vec<Frame>, String>>()?;
    Ok((def, sprite, frames))
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// One frame's palette indices, `w`x`h`, copied out of the sheet.
struct Frame {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Frame {
    fn at(&self, x: u32, y: u32) -> u8 {
        self.px[(y * self.w + x) as usize]
    }
}

fn read_rect(cart: &Cart, rect: (u32, u32, u32, u32)) -> Frame {
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
        offsets: frames.iter().map(|_| ((-ax - min.0) as u32, (-ay - min.1) as u32)).collect(),
    }
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

struct Canvas {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

impl Canvas {
    /// A canvas pre-filled with the dark checkerboard. `zoom` sets the
    /// checker cell size: [`CHECKER_LOGICAL`] sheet pixels square.
    fn new(w: u32, h: u32, zoom: u32) -> Canvas {
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

    fn get(&self, x: u32, y: u32) -> [u8; 3] {
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

    fn fill(&mut self, r: Rect, rgb: [u8; 3]) {
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

    fn finish(self, frames: usize) -> Image {
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
fn draw_frame(canvas: &mut Canvas, frame: &Frame, origin: (u32, u32), zoom: u32) {
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
                PALETTE[usize::from(v) & 15],
            );
        }
    }
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
fn draw_grid(canvas: &mut Canvas, cell: Rect, zoom: u32) {
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
    let z = zoom as i32;
    let cx = cell.x as i32 + anchor.0 * z + z / 2;
    let cy = cell.y as i32 + anchor.1 * z + z / 2;
    let (x0, x1) = (cell.x as i32, (cell.x + cell.w) as i32 - 1);
    let (y0, y1) = (cell.y as i32, (cell.y + cell.h) as i32 - 1);
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
    let (bx, by) = (cell.x as i32 + anchor.0 * z, cell.y as i32 + anchor.1 * z);
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

/// A 3x5 hex glyph per pixel cell. Needs `zoom >= 6` to fit; below that the
/// flag is silently a no-op (the render is still useful, just unlabelled).
fn draw_indices(canvas: &mut Canvas, frame: &Frame, origin: (u32, u32), zoom: u32) {
    if zoom < 6 {
        return;
    }
    for y in 0..frame.h {
        for x in 0..frame.w {
            let v = usize::from(frame.at(x, y)) & 15;
            let cx = origin.0 + x * zoom;
            let cy = origin.1 + y * zoom;
            let ink = if luminance(canvas.get(cx + zoom / 2, cy + zoom / 2)) < 128.0 {
                INK_LIGHT
            } else {
                INK_DARK
            };
            let gx = cx + (zoom - 3) / 2;
            let gy = cy + (zoom - 5) / 2;
            for (row, bits) in GLYPHS[v].iter().enumerate() {
                for bit in 0..3u32 {
                    if bits & (1 << (2 - bit)) != 0 {
                        canvas.set(gx + bit, gy + row as u32, ink);
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

fn luminance(rgb: [u8; 3]) -> f32 {
    0.299 * f32::from(rgb[0]) + 0.587 * f32::from(rgb[1]) + 0.114 * f32::from(rgb[2])
}

/// 3x5 hex digits `0`-`F`, one `u8` bitmask per row (bit 2 = leftmost pixel).
const GLYPHS: [[u8; 5]; 16] = [
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
        bbox: bounds.map(|[x0, y0, x1, y1]| {
            [
                x0 - anchor.0,
                y0 - anchor.1,
                x1 - anchor.0,
                y1 - anchor.1,
            ]
        }),
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

fn pair_json(
    a: &Frame,
    b: &Frame,
    sa: &FrameStats,
    sb: &FrameStats,
    (from, to): (usize, usize),
) -> Value {
    let changed = if a.w == b.w && a.h == b.h {
        a.px.iter().zip(&b.px).filter(|(x, y)| x != y).count()
    } else {
        // Different-sized frames cannot happen for one sprite today, but be
        // explicit rather than panicking if that ever changes.
        a.px.len().max(b.px.len())
    };
    let area_drift = (sa.area > 0).then(|| {
        round2((f64::from(sb.area) - f64::from(sa.area)) / f64::from(sa.area) * 100.0)
    });
    let centroid_drift = match (sa.centroid, sb.centroid) {
        (Some(ca), Some(cb)) => {
            let (dx, dy) = (cb.0 - ca.0, cb.1 - ca.1);
            json!({
                "dx": round2(dx),
                "dy": round2(dy),
                "distance": round2((dx * dx + dy * dy).sqrt()),
            })
        }
        _ => Value::Null,
    };
    let bbox_drift = match (sa.bbox, sb.bbox) {
        (Some(ba), Some(bb)) => json!({
            "dx0": bb[0] - ba[0],
            "dy0": bb[1] - ba[1],
            "dx1": bb[2] - ba[2],
            "dy1": bb[3] - ba[3],
            "dw": (bb[2] - bb[0]) - (ba[2] - ba[0]),
            "dh": (bb[3] - bb[1]) - (ba[3] - ba[1]),
        }),
        _ => Value::Null,
    };
    json!({
        "from": from,
        "to": to,
        "changed_pixels": changed,
        "area_from": sa.area,
        "area_to": sb.area,
        "area_drift_pct": area_drift,
        "centroid_drift": centroid_drift,
        "bbox_drift": bbox_drift,
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

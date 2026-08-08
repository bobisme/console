//! `__gfx_meta__`: sprite and animation declarations.
//!
//! This is pure indexing information over the `__sprites__` sheet — names,
//! tile rects and frame lists for tooling (`console`'s `sprite`
//! subcommands) to render, lint and edit pixels without agents ever
//! hand-shifting hex. See SPEC.md "Sprite & animation authoring (PoC v1)"
//! for the grammar.
//!
//! The runtime reads it too, but only through the three Lua functions
//! `aspr`/`anim_len`/`anim_done`: `spr()` and every other primitive are
//! untouched by this section, so a cart that never calls those three renders
//! exactly as if the section were absent.

use std::collections::BTreeMap;

use crate::error::Error;
use crate::gfx::SHEET_TILES;

/// Sheet edge length in tiles (256px sheet / 8px tiles).
const TILE_GRID: u8 = SHEET_TILES as u8;

/// One `sprite <name> rect=<tx>,<ty> size=<w>x<h> [anchor=<px>,<py>]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpriteDef {
    /// `[a-z0-9_]+`, unique within the cart.
    pub name: String,
    /// Top-left tile coordinate, `(tx, ty)`, each `0..=31`.
    pub rect: (u8, u8),
    /// Size in tiles, `(w, h)`, each `1..=32`.
    pub size: (u8, u8),
    /// Pixel offset from the sprite's top-left, relative to which frames are
    /// authored and aligned. Outside the sprite's own pixel bounds is legal
    /// (that is a lint concern for `sprite lint`, not a parse error).
    pub anchor: (i32, i32),
}

impl SpriteDef {
    /// Pixel-space rect `(x, y, w_px, h_px)` on the 256x256 sheet for
    /// animation frame `i`, per the SPEC wrap formula:
    ///
    /// ```text
    /// tx' = (tx + i*w) % 32
    /// ty' = ty + ((tx + i*w) / 32) * h
    /// ```
    ///
    /// Returns `None` when the resolved rect does not fit the 32x32 tile
    /// sheet (either edge would spill past it).
    ///
    /// This is the sprite's OWN rect used as the frame-run origin — the
    /// classic addressing scheme, and still what raw (non-anim) frame
    /// addressing (`sprite dump`/`poke`/`edit`) always uses. An anim can
    /// override the origin per-anim with `frames_rect=`; see
    /// [`AnimDef::resolve_frame`], which is the one place that composes this
    /// wrap math with an anim's `frames_rect` and explicit `tx:ty` frames.
    pub fn frame_rect(&self, i: u8) -> Option<(u32, u32, u32, u32)> {
        wrap_frame_rect(self.rect, self.size, i)
    }
}

/// The wrap-displacement math shared by [`SpriteDef::frame_rect`] and anim
/// frame resolution: frame `i` is the `size` rect displaced `i` widths to
/// the right of `origin`, wrapping down a row band. `None` when the
/// resolved rect does not fit the 32x32 tile sheet.
fn wrap_frame_rect(origin: (u8, u8), size: (u8, u8), i: u8) -> Option<(u32, u32, u32, u32)> {
    let (tx, ty) = (u32::from(origin.0), u32::from(origin.1));
    let (w, h) = (u32::from(size.0), u32::from(size.1));
    let grid = u32::from(TILE_GRID);

    let shift = tx + u32::from(i) * w;
    let tx2 = shift % grid;
    let ty2 = ty + (shift / grid) * h;
    if tx2 + w > grid || ty2 + h > grid {
        return None;
    }
    Some((tx2 * 8, ty2 * 8, w * 8, h * 8))
}

/// An explicit `tx:ty` frame: the sprite's `size` rect anchored directly at
/// tile `(tx, ty)`, no wrap math, no relation to the sprite's own rect or
/// any `frames_rect`. `None` when it does not fit the 32x32 tile sheet.
fn explicit_frame_rect(tile: (u8, u8), size: (u8, u8)) -> Option<(u32, u32, u32, u32)> {
    let (tx, ty) = (u32::from(tile.0), u32::from(tile.1));
    let (w, h) = (u32::from(size.0), u32::from(size.1));
    let grid = u32::from(TILE_GRID);
    if tx + w > grid || ty + h > grid {
        return None;
    }
    Some((tx * 8, ty * 8, w * 8, h * 8))
}

/// One entry in an anim's `frames=` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSpec {
    /// `frames=i` — the sprite's (or `frames_rect`'s) rect displaced `i`
    /// widths right, wrapping down a row band; the classic addressing.
    Index(u8),
    /// `frames=tx:ty` — explicit tile coordinates for this one frame, same
    /// `WxH` as the sprite. Ignores `frames_rect`.
    Rect(u8, u8),
}

/// Resolve one [`FrameSpec`] against `sprite` (and an optional per-anim
/// `frames_rect` override) to a pixel-space rect. The single place that
/// composes the wrap-displacement rule with `frames_rect` relocation and
/// explicit tile coords — [`AnimDef::resolve_frame`] and the parse-time
/// validator both go through this.
fn resolve_frame_spec(
    spec: FrameSpec,
    sprite: &SpriteDef,
    frames_rect: Option<(u8, u8)>,
) -> Option<(u32, u32, u32, u32)> {
    match spec {
        FrameSpec::Index(i) => wrap_frame_rect(frames_rect.unwrap_or(sprite.rect), sprite.size, i),
        FrameSpec::Rect(tx, ty) => explicit_frame_rect((tx, ty), sprite.size),
    }
}

/// One `anim <sprite>.<label> frames=<i0,i1,...> fps=<1-60> [loop]
/// [frames_rect=<tx>,<ty>]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimDef {
    /// Full namespaced name, `<sprite>.<label>`, unique within the cart.
    pub name: String,
    /// The sprite this anim's frames are relative to (validated to exist).
    pub sprite: String,
    /// Frame entries, at least one, each already validated to resolve to an
    /// in-sheet rect via [`AnimDef::resolve_frame`].
    pub frames: Vec<FrameSpec>,
    /// Optional override for the frame-0 origin used by [`FrameSpec::Index`]
    /// entries in this anim: frame `i` counts from `(tx, ty)` instead of the
    /// sprite's own `rect`, same displacement/wrap rule, same `WxH` as the
    /// sprite. Lets two anims of one sprite live in different sheet regions.
    /// `None` means "use the sprite's own rect", today's behavior.
    pub frames_rect: Option<(u8, u8)>,
    /// Playback rate, `1..=60`.
    pub fps: u8,
    /// Whether playback should wrap back to frame 0 at the end.
    pub looped: bool,
}

impl AnimDef {
    /// Resolve frame-list position `pos` (an index into `self.frames`, NOT a
    /// raw sheet frame index) to a pixel-space rect on `sprite`'s sheet.
    ///
    /// This is the single source of truth for anim frame resolution: every
    /// sprite tool (render/strip/onion/ghost/diff/gif/lint) and the
    /// parse-time validator go through this (or the identical logic in
    /// [`resolve_frame_spec`], which this delegates to), so relocation via
    /// `frames_rect` and explicit `tx:ty` frames behave identically
    /// everywhere. `sprite` must be the anim's own sprite
    /// (`GfxMeta::sprite(&self.sprite)`); passing a different one produces
    /// nonsense. Returns `None` for an out-of-range `pos` or a frame that
    /// does not fit the 32x32 tile sheet.
    pub fn resolve_frame(&self, sprite: &SpriteDef, pos: usize) -> Option<(u32, u32, u32, u32)> {
        let spec = *self.frames.get(pos)?;
        resolve_frame_spec(spec, sprite, self.frames_rect)
    }

    /// How many frames this anim's `frames=` list holds (always >= 1).
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Always false — an anim with no frames does not parse. Present only
    /// because clippy insists a `len` has an `is_empty`.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Playback position *before* looping or clamping: how many whole frame
    /// steps `elapsed` console frames of playback are worth, i.e.
    /// `floor(elapsed * fps / 60)`.
    ///
    /// Floored, not truncated, so a negative `elapsed` (a `t0` in the future)
    /// keeps stepping backwards in even strides. Saturating, so no `t0` a cart
    /// can hand us overflows.
    pub fn step_at(&self, elapsed: i64) -> i64 {
        elapsed.saturating_mul(i64::from(self.fps)).div_euclid(60)
    }

    /// The frame-list position showing `elapsed` console frames after the
    /// anim's origin: looping anims wrap (Euclidean, so negatives wrap too),
    /// one-shots clamp to the first/last frame.
    ///
    /// This is the whole of `aspr`'s statefulness: a pure function of
    /// `elapsed`, which is why replaying the same inputs replays the same
    /// pixels with no animation state stored anywhere.
    pub fn frame_at(&self, elapsed: i64) -> usize {
        let n = self.frames.len() as i64;
        let step = self.step_at(elapsed);
        if self.looped {
            step.rem_euclid(n) as usize
        } else {
            step.clamp(0, n - 1) as usize
        }
    }

    /// True once a one-shot anim has run past its last frame — that is, the
    /// last frame has been shown for its full `1/fps` and playback is now
    /// clamped there. Always false for a looping anim.
    pub fn done_at(&self, elapsed: i64) -> bool {
        !self.looped && self.step_at(elapsed) >= self.frames.len() as i64
    }
}

/// Everything a cart's `__gfx_meta__` section describes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GfxMeta {
    sprites: BTreeMap<String, SpriteDef>,
    anims: BTreeMap<String, AnimDef>,
}

impl GfxMeta {
    /// The sprite named `name`, if defined.
    pub fn sprite(&self, name: &str) -> Option<&SpriteDef> {
        self.sprites.get(name)
    }

    /// The anim named `name` (e.g. `"player.walk"`), if defined.
    pub fn anim(&self, name: &str) -> Option<&AnimDef> {
        self.anims.get(name)
    }

    /// Every sprite, ordered by name.
    pub fn sprites(&self) -> impl Iterator<Item = &SpriteDef> {
        self.sprites.values()
    }

    /// Every anim, ordered by (full) name.
    pub fn anims(&self) -> impl Iterator<Item = &AnimDef> {
        self.anims.values()
    }

    /// True when the cart defines neither sprites nor anims.
    pub fn is_empty(&self) -> bool {
        self.sprites.is_empty() && self.anims.is_empty()
    }

    /// Parse the raw text of `__gfx_meta__` (absent -> an empty `GfxMeta`).
    pub(crate) fn parse(text: Option<&str>) -> Result<GfxMeta, Error> {
        let Some(text) = text else {
            return Ok(GfxMeta::default());
        };

        let mut sprites: BTreeMap<String, SpriteDef> = BTreeMap::new();
        let mut raw_anims: Vec<RawAnim> = Vec::new();
        let mut anim_names: BTreeMap<String, usize> = BTreeMap::new();

        for (i, raw) in text.lines().enumerate() {
            let line = i + 1;
            let body = clean(raw);
            if body.is_empty() {
                continue;
            }
            let tokens: Vec<&str> = body.split_whitespace().collect();

            if tokens[0].eq_ignore_ascii_case("sprite") {
                let def = parse_sprite_line(line, &tokens)?;
                if sprites.contains_key(&def.name) {
                    return Err(cart_err(
                        line,
                        format!("duplicate sprite name {:?}", def.name),
                    ));
                }
                sprites.insert(def.name.clone(), def);
            } else if tokens[0].eq_ignore_ascii_case("anim") {
                let raw_anim = parse_anim_line(line, &tokens)?;
                if anim_names.contains_key(&raw_anim.name) {
                    return Err(cart_err(
                        line,
                        format!("duplicate anim name {:?}", raw_anim.name),
                    ));
                }
                anim_names.insert(raw_anim.name.clone(), line);
                raw_anims.push(raw_anim);
            } else {
                return Err(cart_err(
                    line,
                    format!("expected `sprite` or `anim`, found {:?}", tokens[0]),
                ));
            }
        }

        // Validation after the whole section parses: forward references from
        // an `anim` to a `sprite` defined later in the file are fine.
        let mut anims: BTreeMap<String, AnimDef> = BTreeMap::new();
        for raw_anim in raw_anims {
            let Some(sprite_def) = sprites.get(&raw_anim.sprite) else {
                return Err(cart_err(
                    raw_anim.line,
                    format!(
                        "anim {:?} refers to sprite {:?}, which is not defined in __gfx_meta__",
                        raw_anim.name, raw_anim.sprite
                    ),
                ));
            };
            for (pos, &spec) in raw_anim.frames.iter().enumerate() {
                if resolve_frame_spec(spec, sprite_def, raw_anim.frames_rect).is_none() {
                    let desc = match spec {
                        FrameSpec::Index(i) => format!("index {i}"),
                        FrameSpec::Rect(tx, ty) => format!("tile {tx}:{ty}"),
                    };
                    return Err(cart_err(
                        raw_anim.line,
                        format!(
                            "anim {:?} frame {pos} ({desc}) falls outside the 32x32 tile sheet",
                            raw_anim.name
                        ),
                    ));
                }
            }
            anims.insert(
                raw_anim.name.clone(),
                AnimDef {
                    name: raw_anim.name,
                    sprite: raw_anim.sprite,
                    frames: raw_anim.frames,
                    frames_rect: raw_anim.frames_rect,
                    fps: raw_anim.fps,
                    looped: raw_anim.looped,
                },
            );
        }

        Ok(GfxMeta { sprites, anims })
    }
}

// ---------------------------------------------------------------------------
// Text parsing
// ---------------------------------------------------------------------------

const SEC: &str = "__gfx_meta__";

fn cart_err(line: usize, msg: impl AsRef<str>) -> Error {
    Error::Cart(format!("{SEC} line {line}: {}", msg.as_ref()))
}

/// Strip a `#`-comment (only when `#` starts the line) and surrounding
/// whitespace, same convention as the audio section parsers.
fn clean(raw: &str) -> &str {
    let t = raw.trim();
    if t.starts_with('#') { "" } else { t }
}

/// `[a-z0-9_]+`.
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn parse_u8(line: usize, what: &str, text: &str, min: u8, max: u8) -> Result<u8, Error> {
    let v: u32 = text
        .parse()
        .map_err(|_| cart_err(line, format!("{what} must be a number, found {text:?}")))?;
    if v < u32::from(min) || v > u32::from(max) {
        return Err(cart_err(
            line,
            format!("{what} must be {min}-{max}, found {v}"),
        ));
    }
    Ok(v as u8)
}

fn parse_i32(line: usize, what: &str, text: &str) -> Result<i32, Error> {
    text.parse()
        .map_err(|_| cart_err(line, format!("{what} must be a number, found {text:?}")))
}

fn parse_sprite_line(line: usize, tokens: &[&str]) -> Result<SpriteDef, Error> {
    if tokens.len() < 2 {
        return Err(cart_err(
            line,
            "expected `sprite <name> rect=<tx>,<ty> size=<w>x<h> [anchor=<px>,<py>]`",
        ));
    }
    let name = tokens[1];
    if !valid_name(name) {
        return Err(cart_err(
            line,
            format!("sprite name {name:?} must match [a-z0-9_]+"),
        ));
    }

    let mut rect: Option<(u8, u8)> = None;
    let mut size: Option<(u8, u8)> = None;
    let mut anchor: Option<(i32, i32)> = None;

    for tok in &tokens[2..] {
        let Some((key, value)) = tok.split_once('=') else {
            return Err(cart_err(
                line,
                format!("unexpected {tok:?} in sprite line (want `rect=`, `size=` or `anchor=`)"),
            ));
        };
        match key.to_ascii_lowercase().as_str() {
            "rect" => {
                let Some((a, b)) = value.split_once(',') else {
                    return Err(cart_err(
                        line,
                        format!("rect must be `rect=<tx>,<ty>`, found {value:?}"),
                    ));
                };
                let tx = parse_u8(line, "rect tx", a, 0, TILE_GRID - 1)?;
                let ty = parse_u8(line, "rect ty", b, 0, TILE_GRID - 1)?;
                rect = Some((tx, ty));
            }
            "size" => {
                let Some((a, b)) = value.split_once('x') else {
                    return Err(cart_err(
                        line,
                        format!("size must be `size=<w>x<h>`, found {value:?}"),
                    ));
                };
                let w = parse_u8(line, "size w", a, 1, TILE_GRID)?;
                let h = parse_u8(line, "size h", b, 1, TILE_GRID)?;
                size = Some((w, h));
            }
            "anchor" => {
                let Some((a, b)) = value.split_once(',') else {
                    return Err(cart_err(
                        line,
                        format!("anchor must be `anchor=<px>,<py>`, found {value:?}"),
                    ));
                };
                let px = parse_i32(line, "anchor px", a)?;
                let py = parse_i32(line, "anchor py", b)?;
                anchor = Some((px, py));
            }
            other => {
                return Err(cart_err(
                    line,
                    format!("unknown sprite key {other:?} (want `rect`, `size` or `anchor`)"),
                ));
            }
        }
    }

    let Some(rect) = rect else {
        return Err(cart_err(
            line,
            format!("sprite {name} is missing `rect=<tx>,<ty>`"),
        ));
    };
    let Some(size) = size else {
        return Err(cart_err(
            line,
            format!("sprite {name} is missing `size=<w>x<h>`"),
        ));
    };
    // Default: bottom-center, i.e. ground contact.
    let anchor = anchor.unwrap_or((i32::from(size.0) * 8 / 2, i32::from(size.1) * 8 - 1));

    Ok(SpriteDef {
        name: name.to_string(),
        rect,
        size,
        anchor,
    })
}

/// A parsed `anim` line before sprite-existence and frame-range validation
/// (both of which need the whole section parsed first).
struct RawAnim {
    name: String,
    sprite: String,
    frames: Vec<FrameSpec>,
    frames_rect: Option<(u8, u8)>,
    fps: u8,
    looped: bool,
    line: usize,
}

/// One `frames=` list entry: `i` (a frame index) or `tx:ty` (explicit tile
/// coords for that single frame).
fn parse_frame_spec(line: usize, part: &str) -> Result<FrameSpec, Error> {
    if let Some((a, b)) = part.split_once(':') {
        let tx = parse_u8(line, "frame tile tx", a, 0, TILE_GRID - 1)?;
        let ty = parse_u8(line, "frame tile ty", b, 0, TILE_GRID - 1)?;
        Ok(FrameSpec::Rect(tx, ty))
    } else {
        Ok(FrameSpec::Index(parse_u8(
            line,
            "frame index",
            part,
            0,
            u8::MAX,
        )?))
    }
}

fn parse_anim_line(line: usize, tokens: &[&str]) -> Result<RawAnim, Error> {
    if tokens.len() < 2 {
        return Err(cart_err(
            line,
            "expected `anim <sprite>.<label> frames=<i0,i1,...> fps=<1-60> [loop] [frames_rect=<tx>,<ty>]`",
        ));
    }
    let full = tokens[1];
    let Some((sprite, label)) = full.split_once('.') else {
        return Err(cart_err(
            line,
            format!("anim name {full:?} must be `<sprite>.<label>`"),
        ));
    };
    if !valid_name(sprite) {
        return Err(cart_err(
            line,
            format!("anim sprite name {sprite:?} must match [a-z0-9_]+"),
        ));
    }
    if !valid_name(label) {
        return Err(cart_err(
            line,
            format!("anim label {label:?} must match [a-z0-9_]+"),
        ));
    }

    let mut frames: Option<Vec<FrameSpec>> = None;
    let mut fps: Option<u8> = None;
    let mut looped = false;
    let mut frames_rect: Option<(u8, u8)> = None;

    for tok in &tokens[2..] {
        if tok.eq_ignore_ascii_case("loop") {
            looped = true;
            continue;
        }
        let Some((key, value)) = tok.split_once('=') else {
            return Err(cart_err(
                line,
                format!(
                    "unexpected {tok:?} in anim line (want `frames=`, `fps=`, `frames_rect=` or `loop`)"
                ),
            ));
        };
        match key.to_ascii_lowercase().as_str() {
            "frames" => {
                if value.trim().is_empty() {
                    return Err(cart_err(line, "frames must be nonempty"));
                }
                let mut v = Vec::new();
                for part in value.split(',') {
                    v.push(parse_frame_spec(line, part)?);
                }
                frames = Some(v);
            }
            "fps" => {
                fps = Some(parse_u8(line, "fps", value, 1, 60)?);
            }
            "frames_rect" => {
                let Some((a, b)) = value.split_once(',') else {
                    return Err(cart_err(
                        line,
                        format!("frames_rect must be `frames_rect=<tx>,<ty>`, found {value:?}"),
                    ));
                };
                let tx = parse_u8(line, "frames_rect tx", a, 0, TILE_GRID - 1)?;
                let ty = parse_u8(line, "frames_rect ty", b, 0, TILE_GRID - 1)?;
                frames_rect = Some((tx, ty));
            }
            other => {
                return Err(cart_err(
                    line,
                    format!(
                        "unknown anim key {other:?} (want `frames`, `fps`, `frames_rect` or `loop`)"
                    ),
                ));
            }
        }
    }

    let Some(frames) = frames else {
        return Err(cart_err(
            line,
            format!("anim {full} is missing `frames=<i0,i1,...>`"),
        ));
    };
    let Some(fps) = fps else {
        return Err(cart_err(
            line,
            format!("anim {full} is missing `fps=<1-60>`"),
        ));
    };

    Ok(RawAnim {
        name: full.to_string(),
        sprite: sprite.to_string(),
        frames,
        frames_rect,
        fps,
        looped,
        line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_section_is_empty() {
        let meta = GfxMeta::parse(None).unwrap();
        assert!(meta.is_empty());
    }

    #[test]
    fn default_anchor_is_bottom_center() {
        let meta = GfxMeta::parse(Some("sprite p rect=1,0 size=1x1\n")).unwrap();
        let p = meta.sprite("p").unwrap();
        assert_eq!(p.anchor, (4, 7));
    }

    #[test]
    fn frame_rect_identity_and_wrap() {
        let meta = GfxMeta::parse(Some("sprite p rect=1,0 size=1x1\n")).unwrap();
        let p = meta.sprite("p").unwrap();
        assert_eq!(p.frame_rect(0), Some((8, 0, 8, 8)));
        assert_eq!(p.frame_rect(1), Some((16, 0, 8, 8)));
        // tx=1 + 31*1 = 32 -> wraps to tx'=0, ty'=1 band.
        assert_eq!(p.frame_rect(31), Some((0, 8, 8, 8)));
        // A u8 frame index remains addressable from the top of this larger
        // atlas (frame 255 lands in tile row 8).
        assert_eq!(p.frame_rect(255), Some((0, 64, 8, 8)));
    }

    // -----------------------------------------------------------------
    // frames_rect / explicit tx:ty frames (bn-2op)
    // -----------------------------------------------------------------

    #[test]
    fn classic_frames_parse_and_resolve_unchanged() {
        // Back-compat: no frames_rect, all-index frames behave exactly like
        // before this bone.
        let meta = GfxMeta::parse(Some(
            "sprite p rect=1,0 size=1x1\nanim p.a frames=0,1,31 fps=4 loop\n",
        ))
        .unwrap();
        let a = meta.anim("p.a").unwrap();
        let p = meta.sprite("p").unwrap();
        assert_eq!(
            a.frames,
            vec![
                FrameSpec::Index(0),
                FrameSpec::Index(1),
                FrameSpec::Index(31)
            ]
        );
        assert_eq!(a.frames_rect, None);
        assert_eq!(a.resolve_frame(p, 0), Some((8, 0, 8, 8)));
        assert_eq!(a.resolve_frame(p, 1), Some((16, 0, 8, 8)));
        assert_eq!(a.resolve_frame(p, 2), Some((0, 8, 8, 8)));
        assert_eq!(a.resolve_frame(p, 3), None); // out of range pos
    }

    #[test]
    fn frames_rect_relocates_frame_zero_origin() {
        // Sprite's own rect is (1,0); the anim's frames_rect moves frame 0's
        // origin to (4,4), same wrap rule, same size, from there.
        let meta = GfxMeta::parse(Some(
            "sprite p rect=1,0 size=1x1\nanim p.a frames=0,1,2 fps=4 frames_rect=4,4\n",
        ))
        .unwrap();
        let a = meta.anim("p.a").unwrap();
        let p = meta.sprite("p").unwrap();
        assert_eq!(a.frames_rect, Some((4, 4)));
        assert_eq!(a.resolve_frame(p, 0), Some((32, 32, 8, 8)));
        assert_eq!(a.resolve_frame(p, 1), Some((40, 32, 8, 8)));
        assert_eq!(a.resolve_frame(p, 2), Some((48, 32, 8, 8)));
    }

    #[test]
    fn frames_rect_wraps_down_a_row_band() {
        // Origin near the right edge: frame 1 should wrap to the next row
        // band, exactly like the sprite's-own-rect case does.
        let meta = GfxMeta::parse(Some(
            "sprite p rect=0,0 size=1x1\nanim p.a frames=0,1 fps=4 frames_rect=31,2\n",
        ))
        .unwrap();
        let a = meta.anim("p.a").unwrap();
        let p = meta.sprite("p").unwrap();
        assert_eq!(a.resolve_frame(p, 0), Some((31 * 8, 2 * 8, 8, 8)));
        assert_eq!(a.resolve_frame(p, 1), Some((0, 3 * 8, 8, 8)));
    }

    #[test]
    fn explicit_tile_frame_ignores_frames_rect() {
        let meta = GfxMeta::parse(Some(
            "sprite p rect=0,0 size=1x1\nanim p.a frames=0,12:4,3 fps=4 frames_rect=8,8\n",
        ))
        .unwrap();
        let a = meta.anim("p.a").unwrap();
        let p = meta.sprite("p").unwrap();
        // frames=0 (index) resolves via frames_rect=8,8.
        assert_eq!(a.resolve_frame(p, 0), Some((64, 64, 8, 8)));
        // frames=12:4 (explicit tile) ignores frames_rect entirely.
        assert_eq!(a.resolve_frame(p, 1), Some((12 * 8, 4 * 8, 8, 8)));
        // frames=3 (index again) still goes through frames_rect.
        assert_eq!(a.resolve_frame(p, 2), Some((8 * 8 + 3 * 8, 8 * 8, 8, 8)));
        assert_eq!(
            a.frames,
            vec![
                FrameSpec::Index(0),
                FrameSpec::Rect(12, 4),
                FrameSpec::Index(3),
            ]
        );
    }

    #[test]
    fn frames_rect_off_sheet_is_a_parse_error() {
        let err = GfxMeta::parse(Some(
            "sprite p rect=0,0 size=2x2\nanim p.a frames=0 fps=4 frames_rect=31,31\n",
        ))
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("p.a"), "{msg}");
        assert!(msg.contains("frame 0"), "{msg}");
    }

    #[test]
    fn explicit_tile_frame_off_sheet_is_a_parse_error() {
        // Sprite is 2x2 tiles; explicit tile (31,31) is a valid in-range
        // coordinate on its own but the 2x2 rect anchored there spills past
        // the sheet edge.
        let err = GfxMeta::parse(Some(
            "sprite p rect=0,0 size=2x2\nanim p.a frames=0,31:31 fps=4\n",
        ))
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("p.a"), "{msg}");
        // Position 1 (0-based) is the offending 31:31 entry.
        assert!(msg.contains("frame 1"), "{msg}");
    }

    #[test]
    fn explicit_tile_frame_bad_syntax_is_a_parse_error() {
        // Missing the `:` separator entirely falls through to the plain
        // index parser and fails as "not a number".
        let err = GfxMeta::parse(Some(
            "sprite p rect=0,0 size=1x1\nanim p.a frames=1x2 fps=4\n",
        ))
        .unwrap_err();
        assert!(err.to_string().contains("frame index"), "{err}");
    }

    #[test]
    fn frames_rect_bad_syntax_is_a_parse_error() {
        let err = GfxMeta::parse(Some(
            "sprite p rect=0,0 size=1x1\nanim p.a frames=0 fps=4 frames_rect=oops\n",
        ))
        .unwrap_err();
        assert!(err.to_string().contains("frames_rect"), "{err}");
    }

    // -----------------------------------------------------------------
    // Playback maths (bn-2dq): what `aspr`/`anim_done` are built on
    // -----------------------------------------------------------------

    fn anim(line: &str) -> AnimDef {
        let text = format!("sprite p rect=0,0 size=1x1\n{line}\n");
        GfxMeta::parse(Some(&text))
            .unwrap()
            .anim("p.a")
            .unwrap()
            .clone()
    }

    #[test]
    fn frame_selection_maps_fps_onto_the_60hz_frame_counter() {
        // 8 fps over 4 frames: a step every 60/8 = 7.5 console frames, so the
        // boundaries land on 0, 8, 15, 23, 30 — never drifting, because the
        // maths is one floored multiply-divide of the elapsed frame count.
        let a = anim("anim p.a frames=0,1,2,3 fps=8 loop");
        for (elapsed, want) in [
            (0, 0),
            (7, 0),
            (8, 1),
            (14, 1),
            (15, 2),
            (22, 2),
            (23, 3),
            (29, 3),
            (30, 0),
            (60, 0),
            (68, 1),
        ] {
            assert_eq!(a.frame_at(elapsed), want, "elapsed {elapsed}");
        }
        // One console frame per anim frame at 60 fps.
        let fast = anim("anim p.a frames=0,1,2 fps=60 loop");
        for elapsed in 0..12i64 {
            assert_eq!(fast.frame_at(elapsed), (elapsed % 3) as usize);
        }
        // ... and one anim frame per second at 1 fps.
        let slow = anim("anim p.a frames=0,1 fps=1 loop");
        assert_eq!(slow.frame_at(59), 0);
        assert_eq!(slow.frame_at(60), 1);
        assert_eq!(slow.frame_at(120), 0);
    }

    #[test]
    fn looping_wraps_and_one_shot_clamps() {
        let looped = anim("anim p.a frames=0,1,2,3 fps=60 loop");
        let once = anim("anim p.a frames=0,1,2,3 fps=60");
        assert_eq!(looped.frame_at(4), 0);
        assert_eq!(looped.frame_at(4001), 1);
        assert_eq!(once.frame_at(4), 3);
        assert_eq!(once.frame_at(4001), 3);
        // A `t0` in the future is legal: the loop keeps stepping backwards in
        // even strides (floored, so -1 elapsed is one step back, not zero),
        // while the one-shot clamps to its first frame.
        assert_eq!(looped.frame_at(-1), 3);
        assert_eq!(looped.frame_at(-5), 3);
        assert_eq!(once.frame_at(-1), 0);
    }

    #[test]
    fn done_is_one_shot_only_and_waits_out_the_last_frame() {
        let once = anim("anim p.a frames=0,1,2,3 fps=8");
        // Frame 3 starts showing at elapsed 23 ...
        assert_eq!(once.frame_at(23), 3);
        assert!(!once.done_at(23));
        // ... and is done only once its own 1/8 s has run out, at elapsed 30.
        assert!(!once.done_at(29));
        assert!(once.done_at(30));
        assert!(once.done_at(10_000));
        assert!(!once.done_at(-1));

        let looped = anim("anim p.a frames=0,1,2,3 fps=8 loop");
        for elapsed in [0, 23, 30, 10_000] {
            assert!(!looped.done_at(elapsed), "loops are never done");
        }
    }

    #[test]
    fn playback_maths_saturates_instead_of_overflowing() {
        let a = anim("anim p.a frames=0,1,2,3 fps=60 loop");
        // Whatever `t0` a cart hands us, this must not panic in debug builds.
        let _ = a.frame_at(i64::MAX);
        let _ = a.frame_at(i64::MIN);
        assert!(a.frame_at(i64::MAX) < 4);
        assert!(a.frame_at(i64::MIN) < 4);
    }

    #[test]
    fn len_counts_declared_frames() {
        assert_eq!(anim("anim p.a frames=0,1,2,3 fps=8").len(), 4);
        assert_eq!(anim("anim p.a frames=7:2 fps=8").len(), 1);
        assert!(!anim("anim p.a frames=0 fps=8").is_empty());
    }

    #[test]
    fn two_anims_of_one_sprite_can_live_in_different_regions() {
        // The friction bn-2op fixes: two anims of the same sprite, entirely
        // disjoint sheet regions, neither one contiguous with the sprite's
        // own rect.
        let meta = GfxMeta::parse(Some(
            "sprite p rect=0,0 size=1x1\n\
             anim p.a frames=0,1 fps=4 frames_rect=5,0\n\
             anim p.b frames=0,1 fps=4 frames_rect=9,9\n",
        ))
        .unwrap();
        let p = meta.sprite("p").unwrap();
        let a = meta.anim("p.a").unwrap();
        let b = meta.anim("p.b").unwrap();
        assert_eq!(a.resolve_frame(p, 0), Some((5 * 8, 0, 8, 8)));
        assert_eq!(a.resolve_frame(p, 1), Some((6 * 8, 0, 8, 8)));
        assert_eq!(b.resolve_frame(p, 0), Some((9 * 8, 9 * 8, 8, 8)));
        assert_eq!(b.resolve_frame(p, 1), Some((10 * 8, 9 * 8, 8, 8)));
    }
}

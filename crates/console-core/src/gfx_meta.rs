//! `__gfx_meta__`: authoring metadata for sprites and animations.
//!
//! This is pure indexing information over the `__sprites__` sheet — names,
//! tile rects and frame lists for tooling (`console-agent`'s `sprite`
//! subcommands) to render, lint and edit pixels without agents ever
//! hand-shifting hex. The runtime never reads it: it has no effect on
//! `spr()` or on the rendered picture. See SPEC.md "Sprite & animation
//! authoring (PoC v1)" for the grammar.

use std::collections::BTreeMap;

use crate::error::Error;

/// Sheet edge length in tiles (128px sheet / 8px tiles).
const TILE_GRID: u8 = 16;

/// One `sprite <name> rect=<tx>,<ty> size=<w>x<h> [anchor=<px>,<py>]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpriteDef {
    /// `[a-z0-9_]+`, unique within the cart.
    pub name: String,
    /// Top-left tile coordinate, `(tx, ty)`, each `0..=15`.
    pub rect: (u8, u8),
    /// Size in tiles, `(w, h)`, each `1..=16`.
    pub size: (u8, u8),
    /// Pixel offset from the sprite's top-left, relative to which frames are
    /// authored and aligned. Outside the sprite's own pixel bounds is legal
    /// (that is a lint concern for `sprite lint`, not a parse error).
    pub anchor: (i32, i32),
}

impl SpriteDef {
    /// Pixel-space rect `(x, y, w_px, h_px)` on the 128x128 sheet for
    /// animation frame `i`, per the SPEC wrap formula:
    ///
    /// ```text
    /// tx' = (tx + i*w) % 16
    /// ty' = ty + ((tx + i*w) / 16) * h
    /// ```
    ///
    /// Returns `None` when the resolved rect does not fit the 16x16 tile
    /// sheet (either edge would spill past it).
    pub fn frame_rect(&self, i: u8) -> Option<(u32, u32, u32, u32)> {
        let (tx, ty) = (u32::from(self.rect.0), u32::from(self.rect.1));
        let (w, h) = (u32::from(self.size.0), u32::from(self.size.1));
        let grid = u32::from(TILE_GRID);

        let shift = tx + u32::from(i) * w;
        let tx2 = shift % grid;
        let ty2 = ty + (shift / grid) * h;
        if tx2 + w > grid || ty2 + h > grid {
            return None;
        }
        Some((tx2 * 8, ty2 * 8, w * 8, h * 8))
    }
}

/// One `anim <sprite>.<label> frames=<i0,i1,...> fps=<1-60> [loop]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimDef {
    /// Full namespaced name, `<sprite>.<label>`, unique within the cart.
    pub name: String,
    /// The sprite this anim's frames are relative to (validated to exist).
    pub sprite: String,
    /// Frame indices, at least one, each already validated to resolve to an
    /// in-sheet rect via [`SpriteDef::frame_rect`].
    pub frames: Vec<u8>,
    /// Playback rate, `1..=60`.
    pub fps: u8,
    /// Whether playback should wrap back to frame 0 at the end.
    pub looped: bool,
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
                    return Err(cart_err(line, format!("duplicate sprite name {:?}", def.name)));
                }
                sprites.insert(def.name.clone(), def);
            } else if tokens[0].eq_ignore_ascii_case("anim") {
                let raw_anim = parse_anim_line(line, &tokens)?;
                if anim_names.contains_key(&raw_anim.name) {
                    return Err(cart_err(line, format!("duplicate anim name {:?}", raw_anim.name)));
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
            for &frame in &raw_anim.frames {
                if sprite_def.frame_rect(frame).is_none() {
                    return Err(cart_err(
                        raw_anim.line,
                        format!(
                            "anim {:?} frame index {frame} falls outside the 16x16 tile sheet",
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
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
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
        return Err(cart_err(line, format!("sprite {name} is missing `rect=<tx>,<ty>`")));
    };
    let Some(size) = size else {
        return Err(cart_err(line, format!("sprite {name} is missing `size=<w>x<h>`")));
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
    frames: Vec<u8>,
    fps: u8,
    looped: bool,
    line: usize,
}

fn parse_anim_line(line: usize, tokens: &[&str]) -> Result<RawAnim, Error> {
    if tokens.len() < 2 {
        return Err(cart_err(
            line,
            "expected `anim <sprite>.<label> frames=<i0,i1,...> fps=<1-60> [loop]`",
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

    let mut frames: Option<Vec<u8>> = None;
    let mut fps: Option<u8> = None;
    let mut looped = false;

    for tok in &tokens[2..] {
        if tok.eq_ignore_ascii_case("loop") {
            looped = true;
            continue;
        }
        let Some((key, value)) = tok.split_once('=') else {
            return Err(cart_err(
                line,
                format!("unexpected {tok:?} in anim line (want `frames=`, `fps=` or `loop`)"),
            ));
        };
        match key.to_ascii_lowercase().as_str() {
            "frames" => {
                if value.trim().is_empty() {
                    return Err(cart_err(line, "frames must be nonempty"));
                }
                let mut v = Vec::new();
                for part in value.split(',') {
                    v.push(parse_u8(line, "frame index", part, 0, u8::MAX)?);
                }
                frames = Some(v);
            }
            "fps" => {
                fps = Some(parse_u8(line, "fps", value, 1, 60)?);
            }
            other => {
                return Err(cart_err(
                    line,
                    format!("unknown anim key {other:?} (want `frames`, `fps` or `loop`)"),
                ));
            }
        }
    }

    let Some(frames) = frames else {
        return Err(cart_err(line, format!("anim {full} is missing `frames=<i0,i1,...>`")));
    };
    let Some(fps) = fps else {
        return Err(cart_err(line, format!("anim {full} is missing `fps=<1-60>`")));
    };

    Ok(RawAnim {
        name: full.to_string(),
        sprite: sprite.to_string(),
        frames,
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
        // tx=1 + 15*1 = 16 -> wraps to tx'=0, ty'=1 band.
        assert_eq!(p.frame_rect(15), Some((0, 8, 8, 8)));
        // Off the bottom of the sheet.
        assert_eq!(p.frame_rect(255), None);
    }
}

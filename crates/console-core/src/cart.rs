//! Cart text format parser (`__meta__` / `__lua__` / `__sprites__` / `__map__`
//! / `__instruments__` / `__sfx__` / `__music__`).

use std::collections::BTreeMap;

use crate::audio::{AudioBank, Echo, Instrument, Master, Pattern, Sfx, Wavetable};
use crate::error::Error;
use crate::gfx::{MAP_H, MAP_LEN, MAP_W, SHEET_LEN, SHEET_W, SpriteSheet, TileMap};
use crate::gfx_meta::GfxMeta;

/// A parsed cart.
#[derive(Debug, Clone)]
pub struct Cart {
    meta: BTreeMap<String, String>,
    lua: String,
    sprites: Box<SpriteSheet>,
    map: Box<TileMap>,
    audio: AudioBank,
    gfx_meta: GfxMeta,
    /// Raw text of every section, keyed by section name (without the `__`
    /// markers), including unknown ones so tools can round-trip them.
    sections: BTreeMap<String, String>,
}

impl Cart {
    /// Parse cart text. Tolerates CRLF, trailing whitespace, missing optional
    /// sections and unknown sections. Only `__lua__` is required.
    pub fn parse(text: &str) -> Result<Cart, Error> {
        let mut sections: BTreeMap<String, String> = BTreeMap::new();
        let mut current: Option<String> = None;
        let mut buf = String::new();

        for raw in text.split('\n') {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if let Some(name) = section_header(line) {
                if let Some(prev) = current.take() {
                    push_section(&mut sections, prev, std::mem::take(&mut buf));
                }
                current = Some(name);
                buf.clear();
            } else if current.is_some() {
                buf.push_str(line);
                buf.push('\n');
            }
            // Lines before the first header are ignored (leading comments).
        }
        if let Some(prev) = current.take() {
            push_section(&mut sections, prev, buf);
        }

        let lua = sections
            .get("lua")
            .ok_or_else(|| Error::Cart("cart has no __lua__ section".into()))?
            .clone();

        let meta = match sections.get("meta") {
            Some(text) => parse_meta(text),
            None => BTreeMap::new(),
        };

        let sprites = match sections.get("sprites") {
            Some(text) => parse_sprites(text)?,
            None => Box::new([0u8; SHEET_LEN]),
        };

        let map = match sections.get("map") {
            Some(text) => parse_map(text)?,
            None => Box::new([0u8; MAP_LEN]),
        };

        let audio = AudioBank::parse(
            sections.get("instruments").map(String::as_str),
            sections.get("sfx").map(String::as_str),
            sections.get("music").map(String::as_str),
        )?;

        let gfx_meta = GfxMeta::parse(sections.get("gfx_meta").map(String::as_str))?;

        Ok(Cart {
            meta,
            lua,
            sprites,
            map,
            audio,
            gfx_meta,
            sections,
        })
    }

    /// The cart's Lua source.
    pub fn lua(&self) -> &str {
        &self.lua
    }

    /// The 128x128 sprite sheet (all zeros if the cart had no sprites).
    pub fn sprites(&self) -> &SpriteSheet {
        &self.sprites
    }

    /// The cart's 128x64 tile map from `__map__` (all zeros — i.e. every cell
    /// empty — if the cart had no `__map__` section).
    ///
    /// This is the *initial* map. The running console owns a mutable copy that
    /// `mset` writes to; the cart's own copy never changes, so a `reset` always
    /// replays from the authored map.
    pub fn map(&self) -> &TileMap {
        &self.map
    }

    /// The cart's `__sfx__` + `__music__` data (empty if it had neither).
    pub fn audio(&self) -> &AudioBank {
        &self.audio
    }

    /// Sfx `id` from `__sfx__`.
    pub fn sfx(&self, id: u8) -> Option<&Sfx> {
        self.audio.sfx(id)
    }

    /// Pattern `id` from `__music__`.
    pub fn pattern(&self, id: u8) -> Option<&Pattern> {
        self.audio.pattern(id)
    }

    /// The cart's `__instruments__` entries, in declaration order (empty if
    /// the section is absent).
    pub fn instruments(&self) -> &[Instrument] {
        self.audio.instruments()
    }

    /// One instrument by name.
    pub fn instrument(&self, name: &str) -> Option<&Instrument> {
        self.audio.instrument(name)
    }

    /// The cart's `wavetable <slot> <32 nibbles>` entry for slot `id` (`w<id>`),
    /// if it declared one.
    pub fn wavetable(&self, id: u8) -> Option<&Wavetable> {
        self.audio.wavetable(id)
    }

    /// The cart's `master` line from `__instruments__`, or the all-zero
    /// default (every master stage bypassed) when it has none.
    pub fn master(&self) -> Master {
        self.audio.master()
    }

    /// The cart's `echo` line from `__instruments__`, or the all-zero default
    /// (the echo bus switched off) when it has none.
    pub fn echo(&self) -> Echo {
        self.audio.echo()
    }

    /// The cart's `__gfx_meta__` sprite/anim declarations (empty if the
    /// section is absent). Read by the sprite tools and, at runtime, by Lua's
    /// `aspr`/`anim_len`/`anim_done`; nothing else in the console looks at it.
    pub fn gfx_meta(&self) -> &GfxMeta {
        &self.gfx_meta
    }

    /// All `key=value` pairs from `__meta__`.
    pub fn meta(&self) -> &BTreeMap<String, String> {
        &self.meta
    }

    /// One `__meta__` value.
    pub fn meta_get(&self, key: &str) -> Option<&str> {
        self.meta.get(key).map(String::as_str)
    }

    /// `title` from `__meta__`, or `"untitled"`.
    pub fn title(&self) -> &str {
        self.meta_get("title").unwrap_or("untitled")
    }

    /// Raw text of any section, including unknown ones.
    pub fn section(&self, name: &str) -> Option<&str> {
        self.sections.get(name).map(String::as_str)
    }

    /// Names of every section present in the cart, sorted.
    pub fn section_names(&self) -> impl Iterator<Item = &str> {
        self.sections.keys().map(String::as_str)
    }
}

/// `__name__` on a line of its own (surrounding whitespace tolerated).
fn section_header(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix("__")?.strip_suffix("__")?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(inner.to_ascii_lowercase())
}

/// Repeated sections are concatenated rather than dropped.
fn push_section(sections: &mut BTreeMap<String, String>, name: String, body: String) {
    sections.entry(name).or_default().push_str(&body);
}

fn parse_meta(text: &str) -> BTreeMap<String, String> {
    let mut meta = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("--") || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            meta.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    meta
}

fn parse_sprites(text: &str) -> Result<Box<SpriteSheet>, Error> {
    let mut sheet = Box::new([0u8; SHEET_LEN]);
    let mut y = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if y >= SHEET_W {
            break;
        }
        for (x, ch) in line.chars().take(SHEET_W).enumerate() {
            let v = ch.to_digit(16).ok_or_else(|| {
                Error::Cart(format!(
                    "__sprites__ row {y}: expected hex digit, found {ch:?}"
                ))
            })?;
            sheet[y * SHEET_W + x] = v as u8;
        }
        y += 1;
    }
    Ok(sheet)
}

const MAP_SEC: &str = "__map__";

fn map_err(line: usize, msg: impl AsRef<str>) -> Error {
    Error::Cart(format!("{MAP_SEC} line {line}: {}", msg.as_ref()))
}

/// `__map__`: rows of tile ids, **two hex digits per cell**, following the same
/// hex-grid conventions as `__sprites__`.
///
/// Short rows pad with tile 0 and missing rows are all tile 0, so the common
/// case (a small map at the top-left) stays a small block of text. Unlike
/// `__sprites__`, overlong input is an error rather than silent truncation: a
/// map row that runs past the edge is nearly always a miscount, and losing
/// terrain quietly is much worse than failing to load.
fn parse_map(text: &str) -> Result<Box<TileMap>, Error> {
    let mut tiles = Box::new([0u8; MAP_LEN]);
    let mut y = 0usize;

    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if y >= MAP_H {
            return Err(map_err(
                lineno,
                format!("map is at most {MAP_H} rows tall, found row {}", y + 1),
            ));
        }
        let bytes = line.as_bytes();
        if bytes.len() % 2 != 0 {
            return Err(map_err(
                lineno,
                format!(
                    "each cell is 2 hex digits, so a row needs an even length, found {}",
                    bytes.len()
                ),
            ));
        }
        let cells = bytes.len() / 2;
        if cells > MAP_W {
            return Err(map_err(
                lineno,
                format!("map is {MAP_W} cells wide, found {cells}"),
            ));
        }
        let row = y * MAP_W;
        for (x, pair) in bytes.chunks_exact(2).enumerate() {
            let hi = map_hex(lineno, pair[0])?;
            let lo = map_hex(lineno, pair[1])?;
            tiles[row + x] = hi * 16 + lo;
        }
        y += 1;
    }
    Ok(tiles)
}

fn map_hex(line: usize, b: u8) -> Result<u8, Error> {
    char::from(b).to_digit(16).map(|v| v as u8).ok_or_else(|| {
        map_err(
            line,
            format!("expected hex digit, found {:?}", char::from(b)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_detection() {
        assert_eq!(section_header("__lua__"), Some("lua".into()));
        assert_eq!(section_header("  __MeTa__  "), Some("meta".into()));
        assert_eq!(section_header("__x y__"), None);
        assert_eq!(section_header("____"), None);
        assert_eq!(section_header("not a header"), None);
    }

    #[test]
    fn missing_lua_is_an_error() {
        let err = Cart::parse("__meta__\ntitle=x\n").unwrap_err();
        assert!(matches!(err, Error::Cart(_)));
    }

    #[test]
    fn bad_sprite_char_is_an_error() {
        let err = Cart::parse("__lua__\n\n__sprites__\n00zz\n").unwrap_err();
        assert!(err.to_string().contains("hex digit"));
    }
}

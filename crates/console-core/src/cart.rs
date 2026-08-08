//! Cart text format parser (`__meta__` / `__lua__` / `__sprites__` / `__map__`
//! / `__instruments__` / `__sfx__` / `__music__`).

use std::collections::BTreeMap;

use crate::audio::{AudioBank, Echo, Instrument, Master, Pattern, Sfx, Wavetable};
use crate::error::Error;
use crate::gfx::{
    COLOR_COUNT, GfxFlags, IDENTITY_PAL, MAP_CELL_HEX_DIGITS, MAP_FORMAT_MARKER, MAP_H, MAP_LEN,
    MAP_W, SHEET_LEN, SHEET_TILES, SHEET_W, SpriteSheet, TILE_COUNT, TILE_ID_MAX, TileId, TileMap,
    parse_color_char,
};
use crate::gfx_meta::GfxMeta;
use crate::save::SaveConfig;

/// Tooling-only source-index to display-index mapping for static art previews.
///
/// The runtime deliberately ignores this mapping: carts can keep a compact
/// authored ink vocabulary while applying their actual draw palette in Lua.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPalette {
    indices: [u8; COLOR_COUNT],
}

impl PreviewPalette {
    /// All 64 source-index mappings, in source-index order.
    pub fn indices(&self) -> &[u8; COLOR_COUNT] {
        &self.indices
    }

    /// Resolve one valid six-bit authored color index for a static preview.
    pub fn resolve(&self, source: u8) -> u8 {
        debug_assert!(usize::from(source) < COLOR_COUNT);
        self.indices[usize::from(source)]
    }
}

impl Default for PreviewPalette {
    fn default() -> Self {
        Self {
            indices: IDENTITY_PAL,
        }
    }
}

/// A parsed cart.
#[derive(Debug, Clone)]
pub struct Cart {
    meta: BTreeMap<String, String>,
    lua: String,
    sprites: Box<SpriteSheet>,
    gfx_flags: Box<GfxFlags>,
    map: Box<TileMap>,
    audio: AudioBank,
    gfx_meta: GfxMeta,
    preview_palette: PreviewPalette,
    save_config: Option<SaveConfig>,
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
        let preview_palette = parse_preview_palette(meta.get("preview_palette"))?;
        let save_config = SaveConfig::from_meta(&meta)?;

        let sprites = match sections.get("sprites") {
            Some(text) => parse_sprites(text)?,
            None => Box::new([0u8; SHEET_LEN]),
        };

        let gfx_flags = match sections.get("gfx_flags") {
            Some(text) => parse_gfx_flags(text)?,
            None => Box::new([0u8; TILE_COUNT]),
        };

        let map = match sections.get("map") {
            Some(text) => parse_map(text)?,
            None => Box::new([0 as TileId; MAP_LEN]),
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
            gfx_flags,
            map,
            audio,
            gfx_meta,
            preview_palette,
            save_config,
            sections,
        })
    }

    /// The cart's Lua source.
    pub fn lua(&self) -> &str {
        &self.lua
    }

    /// The 256x256 sprite sheet (all zeros if the cart had no sprites).
    pub fn sprites(&self) -> &SpriteSheet {
        &self.sprites
    }

    /// Initial eight-bit flags for all 1024 sprite tiles. A running console
    /// owns a mutable copy for `fset`; resetting restores these authored bits.
    pub fn gfx_flags(&self) -> &GfxFlags {
        &self.gfx_flags
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

    /// Tooling-only source-to-display palette used by static sprite and map
    /// previews. Missing entries, and carts without the metadata key, map to
    /// themselves. The running console does not consume this value.
    pub fn preview_palette(&self) -> &PreviewPalette {
        &self.preview_palette
    }

    /// Stable persistence identity and current schema declared in `__meta__`.
    pub fn save_config(&self) -> Option<&SaveConfig> {
        self.save_config.as_ref()
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

fn parse_preview_palette(value: Option<&String>) -> Result<PreviewPalette, Error> {
    let Some(value) = value else {
        return Ok(PreviewPalette::default());
    };
    if value.trim().is_empty() {
        return Err(Error::Cart(
            "__meta__ preview_palette must contain at least one index".into(),
        ));
    }

    let mut indices = IDENTITY_PAL;
    let entries: Vec<&str> = value.split(',').collect();
    if entries.len() > COLOR_COUNT {
        return Err(Error::Cart(format!(
            "__meta__ preview_palette has {} entries; expected at most {COLOR_COUNT}",
            entries.len()
        )));
    }
    for (source, raw) in entries.into_iter().enumerate() {
        let raw = raw.trim();
        let display = raw.parse::<u8>().map_err(|_| {
            Error::Cart(format!(
                "__meta__ preview_palette entry {source} must be an integer in 0..{}, found {raw:?}",
                COLOR_COUNT - 1
            ))
        })?;
        if usize::from(display) >= COLOR_COUNT {
            return Err(Error::Cart(format!(
                "__meta__ preview_palette entry {source} is {display}; expected 0..{}",
                COLOR_COUNT - 1
            )));
        }
        indices[source] = display;
    }
    Ok(PreviewPalette { indices })
}

fn parse_sprites(text: &str) -> Result<Box<SpriteSheet>, Error> {
    let mut sheet = Box::new([0u8; SHEET_LEN]);
    let mut y = 0usize;
    for (line_index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if y >= SHEET_W {
            return Err(Error::Cart(format!(
                "__sprites__ line {}: sprite sheet is at most {SHEET_W} rows tall",
                line_index + 1
            )));
        }
        let width = line.chars().count();
        if width > SHEET_W {
            return Err(Error::Cart(format!(
                "__sprites__ line {}: sprite sheet is {SHEET_W} pixels wide, found {width}",
                line_index + 1
            )));
        }
        for (x, ch) in line.chars().enumerate() {
            let v = parse_color_char(ch).ok_or_else(|| {
                Error::Cart(format!(
                    "__sprites__ line {}: expected a 64-color palette character, found {ch:?}",
                    line_index + 1
                ))
            })?;
            sheet[y * SHEET_W + x] = v;
        }
        y += 1;
    }
    Ok(sheet)
}

fn parse_gfx_flags(text: &str) -> Result<Box<GfxFlags>, Error> {
    let mut flags = Box::new([0u8; TILE_COUNT]);
    let mut y = 0usize;
    for (line_index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if y >= SHEET_TILES {
            return Err(Error::Cart(format!(
                "__gfx_flags__ line {}: flag grid is at most {SHEET_TILES} rows tall",
                line_index + 1
            )));
        }
        if line.len() % 2 != 0 {
            return Err(Error::Cart(format!(
                "__gfx_flags__ line {}: each flag is 2 hex digits, found odd row length {}",
                line_index + 1,
                line.len()
            )));
        }
        let cells = line.len() / 2;
        if cells > SHEET_TILES {
            return Err(Error::Cart(format!(
                "__gfx_flags__ line {}: flag grid is {SHEET_TILES} cells wide, found {cells}",
                line_index + 1
            )));
        }
        for (x, pair) in line.as_bytes().chunks_exact(2).enumerate() {
            let hi = hex_nibble("__gfx_flags__", line_index + 1, pair[0])? as u8;
            let lo = hex_nibble("__gfx_flags__", line_index + 1, pair[1])? as u8;
            flags[y * SHEET_TILES + x] = hi * 16 + lo;
        }
        y += 1;
    }
    Ok(flags)
}

const MAP_SEC: &str = "__map__";

fn map_err(line: usize, msg: impl AsRef<str>) -> Error {
    Error::Cart(format!("{MAP_SEC} line {line}: {}", msg.as_ref()))
}

/// `__map__`: rows of tile ids, **three hex digits per cell**. It shares the
/// sprite grid's comment, blank-line, padding, and row-count conventions, but
/// deliberately keeps a hex alphabet because tile IDs are wider than palette
/// indices.
///
/// Short rows pad with tile 0 and missing rows are all tile 0, so the common
/// case (a small map at the top-left) stays a small block of text. Unlike
/// `__sprites__`, overlong input is an error rather than silent truncation: a
/// map row that runs past the edge is nearly always a miscount, and losing
/// terrain quietly is much worse than failing to load.
fn parse_map(text: &str) -> Result<Box<TileMap>, Error> {
    let mut tiles = Box::new([0 as TileId; MAP_LEN]);
    let mut y = 0usize;
    let mut format_seen = false;

    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        let line = raw.trim();
        if line == MAP_FORMAT_MARKER {
            format_seen = true;
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !format_seen {
            return Err(map_err(
                lineno,
                format!(
                    "missing required {MAP_FORMAT_MARKER:?} before map data; this may be a legacy two-hex-digit row. Convert every cell to three digits (for example 01 -> 001) and add the marker"
                ),
            ));
        }
        if y >= MAP_H {
            return Err(map_err(
                lineno,
                format!("map is at most {MAP_H} rows tall, found row {}", y + 1),
            ));
        }
        let bytes = line.as_bytes();
        if bytes.len() % MAP_CELL_HEX_DIGITS != 0 {
            return Err(map_err(
                lineno,
                format!(
                    "each cell is {MAP_CELL_HEX_DIGITS} hex digits, so a row length must be a multiple of {MAP_CELL_HEX_DIGITS}, found {}",
                    bytes.len(),
                ),
            ));
        }
        let cells = bytes.len() / MAP_CELL_HEX_DIGITS;
        if cells > MAP_W {
            return Err(map_err(
                lineno,
                format!("map is {MAP_W} cells wide, found {cells}"),
            ));
        }
        let row = y * MAP_W;
        for (x, digits) in bytes.chunks_exact(MAP_CELL_HEX_DIGITS).enumerate() {
            let value = digits.iter().try_fold(0 as TileId, |value, &digit| {
                Ok::<TileId, Error>(value * 16 + map_hex(lineno, digit)?)
            })?;
            if value > TILE_ID_MAX {
                return Err(map_err(
                    lineno,
                    format!("cell {x} has tile id {value:03x}; expected 000-{TILE_ID_MAX:03x}"),
                ));
            }
            tiles[row + x] = value;
        }
        y += 1;
    }
    Ok(tiles)
}

fn map_hex(line: usize, b: u8) -> Result<TileId, Error> {
    hex_nibble(MAP_SEC, line, b)
}

fn hex_nibble(section: &str, line: usize, b: u8) -> Result<TileId, Error> {
    char::from(b)
        .to_digit(16)
        .map(|v| v as TileId)
        .ok_or_else(|| {
            Error::Cart(format!(
                "{section} line {line}: expected hex digit, found {:?}",
                char::from(b)
            ))
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
        let err = Cart::parse("__lua__\n\n__sprites__\n00@@\n").unwrap_err();
        assert!(err.to_string().contains("palette character"));
    }
}

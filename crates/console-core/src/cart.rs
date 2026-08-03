//! Cart text format parser (`__meta__` / `__lua__` / `__sprites__` /
//! `__instruments__` / `__sfx__` / `__music__`).

use std::collections::BTreeMap;

use crate::audio::{AudioBank, Instrument, Master, Pattern, Sfx};
use crate::error::Error;
use crate::gfx::{SHEET_LEN, SHEET_W, SpriteSheet};
use crate::gfx_meta::GfxMeta;

/// A parsed cart.
#[derive(Debug, Clone)]
pub struct Cart {
    meta: BTreeMap<String, String>,
    lua: String,
    sprites: Box<SpriteSheet>,
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

    /// The cart's `master` line from `__instruments__`, or the all-zero
    /// default (every master stage bypassed) when it has none.
    pub fn master(&self) -> Master {
        self.audio.master()
    }

    /// The cart's `__gfx_meta__` sprite/anim authoring data (empty if the
    /// section is absent). Purely descriptive: the runtime never reads it.
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

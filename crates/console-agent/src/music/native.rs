//! Standalone, lossless console music bundles.
//!
//! A `.cmusic` file is deliberately only a container around the cart's
//! existing audio sections.  It does not add a second tracker grammar: the
//! bodies of `__instruments__`, `__sfx__`, and `__music__` are parsed by
//! `console-core` exactly as they are in a cart.

use std::collections::BTreeMap;

use console_core::Cart;

pub const MAGIC: &str = "console-music 1";
const AUDIO_SECTIONS: [&str; 3] = ["instruments", "sfx", "music"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeMusic {
    sections: BTreeMap<String, String>,
}

impl NativeMusic {
    /// Parse and validate a versioned `.cmusic` document.
    pub fn parse(source: &str) -> Result<Self, String> {
        let normalized = source
            .strip_prefix('\u{feff}')
            .unwrap_or(source)
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let mut lines = normalized.split('\n');
        let header = lines.next().unwrap_or_default().trim();
        if header != MAGIC {
            if let Some(version) = header.strip_prefix("console-music ") {
                return Err(format!(
                    "unsupported console-music version {version:?}; expected 1"
                ));
            }
            return Err(format!("missing {MAGIC:?} header on the first line"));
        }

        let mut sections = BTreeMap::<String, String>::new();
        let mut current = None::<String>;
        let mut body = Vec::<String>::new();
        for (line_index, line) in lines.enumerate() {
            if let Some(name) = section_marker(line) {
                finish_section(&mut sections, current.take(), &mut body)?;
                if !AUDIO_SECTIONS.contains(&name) {
                    return Err(format!(
                        "line {} uses unsupported section __{name}__; .cmusic supports only __instruments__, __sfx__, and __music__",
                        line_index + 2
                    ));
                }
                if sections.contains_key(name) {
                    return Err(format!("duplicate __{name}__ section"));
                }
                current = Some(name.to_string());
            } else if current.is_some() {
                body.push(line.to_string());
            } else if !line.trim().is_empty() {
                return Err(format!(
                    "line {} must be an audio section header",
                    line_index + 2
                ));
            }
        }
        finish_section(&mut sections, current, &mut body)?;
        if sections.is_empty() {
            return Err(".cmusic file has no audio sections".to_string());
        }

        let native = Self { sections };
        Cart::parse(&native.cart_text())
            .map_err(|error| format!("invalid console-music audio: {error}"))?;
        Ok(native)
    }

    /// The section bodies, suitable for insertion by `console build`.
    pub(crate) fn into_sections(self) -> BTreeMap<String, String> {
        self.sections
    }

    /// A minimal cart that preserves the bundle's native audio semantics.
    pub fn cart_text(&self) -> String {
        let mut output = String::from("__lua__\n");
        for name in AUDIO_SECTIONS {
            if let Some(body) = self.sections.get(name) {
                output.push_str("__");
                output.push_str(name);
                output.push_str("__\n");
                output.push_str(body);
                output.push('\n');
            }
        }
        output
    }
}

pub fn has_magic(source: &str) -> bool {
    source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .lines()
        .next()
        .is_some_and(|line| line.trim().starts_with("console-music "))
}

fn finish_section(
    sections: &mut BTreeMap<String, String>,
    current: Option<String>,
    body: &mut Vec<String>,
) -> Result<(), String> {
    let Some(name) = current else {
        return Ok(());
    };
    if sections.contains_key(&name) {
        return Err(format!("duplicate __{name}__ section"));
    }
    while body.last().is_some_and(|line| line.is_empty()) {
        body.pop();
    }
    sections.insert(name, body.join("\n"));
    body.clear();
    Ok(())
}

fn section_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix("__")?.strip_suffix("__")?;
    (!inner.is_empty()
        && inner
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_'))
    .then_some(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACK: &str = "console-music 1\n\
        \n\
        __instruments__\n\
        inst lead wave=1 env=0,8,3 vib=12,3,2 echo=3\n\
        master drive=1 tone=1 hiss=0\n\
        echo delay=12 feedback=4 level=3\n\
        __sfx__\n\
        sfx 0 speed=auto\n\
        C4 lead 6 vib\n\
        E4 lead 6\n\
        G4 lead 6 fade-2\n\
        __music__\n\
        bpm=120 rows_per_beat=4\n\
        pat 0 loop=0 : 0 - - -\n";

    #[test]
    fn bundles_existing_audio_grammar_without_translation() {
        let native = NativeMusic::parse(TRACK).unwrap();
        let cart = Cart::parse(&native.cart_text()).unwrap();
        assert!(cart.audio().instrument("lead").is_some());
        assert_eq!(cart.audio().echo().delay, 12);
        assert!(cart.audio().pattern(0).is_some());
        assert_eq!(
            native.sections["sfx"],
            "sfx 0 speed=auto\nC4 lead 6 vib\nE4 lead 6\nG4 lead 6 fade-2"
        );
    }

    #[test]
    fn rejects_versions_unknown_sections_duplicates_and_invalid_audio() {
        for (source, expected) in [
            ("console-music 2\n__music__\nbpm=120\n", "version"),
            (
                "console-music 1\n__lua__\nprint('no')\n",
                "unsupported section",
            ),
            (
                "console-music 1\n__music__\nbpm=120\n__music__\nbpm=90\n",
                "duplicate",
            ),
            (
                "console-music 1\n__music__\nbpm=120\npat 0 : 99 - - -\n",
                "invalid console-music audio",
            ),
        ] {
            let error = NativeMusic::parse(source).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn recognizes_magic_even_with_a_bom() {
        assert!(has_magic("\u{feff}console-music 1\n__music__\n"));
        assert!(!has_magic("X:1\nK:C\nC\n"));
    }
}

//! `console music import-abc` — ABC notation into `__sfx__` rows.
//!
//! ABC is the lingua franca for exchanging melodies as plain text: thousands
//! of public-domain tunes exist as ABC, humans write it fluently, and it is
//! the one music format an agent can paste into a prompt. Turning it into
//! tracker rows is the shortest path from "here is the tune" to "the console
//! plays it", and every step of the mapping is reported so the agent can
//! reason about what it got.
//!
//! ## The mapping
//!
//! 1. **Durations are rational.** Every note length is a `Frac` of the `L:`
//!    default note length, so `a3/2`, a tie across a bar line and a `Q:`
//!    tempo compose without floating-point drift.
//! 2. **One row = the greatest common divisor of the tune's note lengths.**
//!    Not "the shortest note", which breaks the moment a tune mixes 2- and
//!    3-unit notes; the gcd makes *every* note an exact whole number of rows,
//!    and it equals the shortest note in the common case.
//! 3. **A held note repeats its row.** The console has no note-off and no
//!    "continue" row, so a 4-row note is the same note row four times. For a
//!    bare wave digit or a flat instrument that is *sample-identical* to a
//!    held note (`apply_row` sets the same freq/wave/vol, phase is continuous
//!    and the 64-sample ramp never fires); on an `env`/`sweep` instrument each
//!    repeat re-attacks, which is why the report says so.
//! 4. **Splitting is free.** Because held rows are just repeated note rows, a
//!    tune longer than [`MAX_SFX_ROWS`] splits at an exact row boundary into
//!    consecutive sfx ids with no special case — the split points are
//!    reported and the suggested `pat` lines chain them.
//!
//! ## The ABC subset
//!
//! Supported: `X: T: M: L: Q: K: V:` fields (inline `[K:…]` too), notes with
//! octave marks (`C,` `c` `c'`), `^ ^^ _ __ =` accidentals with key-signature
//! and bar-local memory, all seven modes in `K:`, rests (`z`/`x`/`Z`), length
//! multipliers and divisors (`a2`, `a/`, `a/2`, `a3/2`), ties (`-`, merged
//! into one longer note), broken rhythm (`>` `<`), bar lines (validated
//! against `M:` when both are present, but never required), repeats and
//! endings (played **once**, with a warning — `__music__` patterns are where
//! repeats belong), chords (reduced to the first note, with a warning), grace
//! notes / decorations / annotations (dropped), and `%` comments.
//!
//! Rejected with a clear message, rather than mis-imported: tuplets (`(3`),
//! voice overlays (`&`), and anything else unrecognised.

use std::collections::{BTreeMap, HashMap, HashSet};

use console_core::{Cart, MAX_ID, MAX_SFX_ROWS, MAX_VOL, WAVE_FM, WAVE_TABLE_BASE};

use crate::audio::note_name;

use super::sfxtext::{self, EditResult, Rewrite, apply_edit_result};

// ---------------------------------------------------------------------------
// Rational arithmetic
// ---------------------------------------------------------------------------

/// A non-negative rational duration. Every length in this module is one of
/// these, in whole notes, so nothing rounds until the very last step
/// (frames-per-row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frac {
    pub num: i64,
    pub den: i64,
}

impl Frac {
    pub fn new(num: i64, den: i64) -> Frac {
        assert!(den != 0, "zero denominator");
        let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
        let g = gcd(num.unsigned_abs(), den.unsigned_abs()).max(1) as i64;
        Frac {
            num: num / g,
            den: den / g,
        }
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// `1/8` — how ABC spells a length, and how the report prints one.
    pub fn text(self) -> String {
        if self.den == 1 {
            self.num.to_string()
        } else {
            format!("{}/{}", self.num, self.den)
        }
    }
}

impl std::ops::Mul for Frac {
    type Output = Frac;
    fn mul(self, other: Frac) -> Frac {
        Frac::new(self.num * other.num, self.den * other.den)
    }
}

impl std::ops::Add for Frac {
    type Output = Frac;
    fn add(self, other: Frac) -> Frac {
        Frac::new(
            self.num * other.den + other.num * self.den,
            self.den * other.den,
        )
    }
}

impl std::ops::Sub for Frac {
    type Output = Frac;
    fn sub(self, other: Frac) -> Frac {
        Frac::new(
            self.num * other.den - other.num * self.den,
            self.den * other.den,
        )
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn lcm(a: i64, b: i64) -> i64 {
    a / gcd(a.unsigned_abs(), b.unsigned_abs()).max(1) as i64 * b
}

// ---------------------------------------------------------------------------
// The parsed tune
// ---------------------------------------------------------------------------

/// One monophonic event: a pitch or a silence, with an exact duration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbcEvent {
    /// `note` is a console note index (0 = C0 … 95 = B7); ABC's `C` is C4.
    /// Values outside that range are kept so the range check can *name* them.
    Note {
        note: i32,
        dur: Frac,
        token: String,
        line: usize,
    },
    Rest {
        dur: Frac,
    },
}

impl AbcEvent {
    pub fn dur(&self) -> Frac {
        match self {
            AbcEvent::Note { dur, .. } | AbcEvent::Rest { dur } => *dur,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AbcTune {
    pub title: Option<String>,
    /// `L:` default note length, in whole notes.
    pub unit: Frac,
    /// `M:` meter as a fraction of a whole note (`None` for `M:none`).
    pub meter: Option<Frac>,
    pub meter_text: String,
    /// `K:` as a human-readable description, e.g. `D major (F#, C#)`.
    pub key_text: String,
    /// `Q:` as `(unit length, beats per minute)`.
    pub tempo: Option<(Frac, u32)>,
    pub events: Vec<AbcEvent>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Pitch class of each ABC note letter, indexed C D E F G A B.
const LETTER_PC: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
/// Order the sharps appear in a key signature: F C G D A E B.
const SHARP_ORDER: [usize; 7] = [3, 0, 4, 1, 5, 2, 6];
/// Order the flats appear: B E A D G C F.
const FLAT_ORDER: [usize; 7] = [6, 2, 5, 1, 4, 0, 3];

fn letter_index(c: char) -> Option<usize> {
    match c.to_ascii_uppercase() {
        'C' => Some(0),
        'D' => Some(1),
        'E' => Some(2),
        'F' => Some(3),
        'G' => Some(4),
        'A' => Some(5),
        'B' => Some(6),
        _ => None,
    }
}

struct Parser {
    events: Vec<AbcEvent>,
    warnings: Vec<String>,
    warned: HashSet<String>,
    title: Option<String>,
    unit: Option<Frac>,
    meter: Option<Frac>,
    meter_text: String,
    tempo: Option<(Frac, u32)>,
    key_text: String,
    key_acc: [i32; 7],
    /// Accidentals in force for the rest of the bar, keyed by
    /// `(letter index, octave)` — the classical rule, and the one ABC states.
    bar_acc: HashMap<(usize, i32), i32>,
    bar_len: Frac,
    bars: usize,
    bad_bars: Vec<(usize, String)>,
    voices: Vec<String>,
    voice: Option<String>,
    tie_pending: bool,
    /// Length multiplier a `>`/`<` left for the next note.
    broken: Option<Frac>,
    line: usize,
    body_started: bool,
}

/// Parse an ABC tune. Errors name the offending line and token; everything
/// recoverable becomes a warning instead.
pub fn parse_abc(text: &str) -> Result<AbcTune, String> {
    let mut p = Parser {
        events: Vec::new(),
        warnings: Vec::new(),
        warned: HashSet::new(),
        title: None,
        unit: None,
        meter: None,
        meter_text: "none".to_string(),
        tempo: None,
        key_text: "C major".to_string(),
        key_acc: [0; 7],
        bar_acc: HashMap::new(),
        bar_len: Frac::new(0, 1),
        bars: 0,
        bad_bars: Vec::new(),
        voices: Vec::new(),
        voice: None,
        tie_pending: false,
        broken: None,
        line: 0,
        body_started: false,
    };

    for (n, raw) in text.lines().enumerate() {
        p.line = n + 1;
        let line = raw.trim_end();
        if line.trim_start().starts_with("%%") || line.trim().is_empty() {
            continue;
        }
        // A `%` comment runs to end of line (ABC has no escape for it).
        let line = match line.find('%') {
            Some(at) => &line[..at],
            None => line,
        };
        if line.trim().is_empty() {
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            p.field(line[0..1].chars().next().unwrap(), line[2..].trim())?;
            continue;
        }
        p.start_body();
        p.body(line)?;
    }

    if !p.bad_bars.is_empty() {
        let shown: Vec<String> = p
            .bad_bars
            .iter()
            .take(3)
            .map(|(bar, len)| format!("bar {bar} is {len}"))
            .collect();
        let more = if p.bad_bars.len() > 3 {
            format!(" (+{} more)", p.bad_bars.len() - 3)
        } else {
            String::new()
        };
        p.warnings.push(format!(
            "{} bar(s) do not add up to M:{}: {}{more} — imported as written",
            p.bad_bars.len(),
            p.meter_text,
            shown.join(", ")
        ));
    }
    if p.events.is_empty() {
        return Err("the ABC input has no notes or rests".to_string());
    }

    Ok(AbcTune {
        title: p.title,
        unit: p.unit.unwrap_or_else(|| Frac::new(1, 8)),
        meter: p.meter,
        meter_text: p.meter_text,
        key_text: p.key_text,
        tempo: p.tempo,
        events: p.events,
        warnings: p.warnings,
    })
}

impl Parser {
    fn err(&self, msg: impl AsRef<str>) -> String {
        format!("abc line {}: {}", self.line, msg.as_ref())
    }

    fn warn(&mut self, msg: String) {
        self.warnings.push(msg);
    }

    /// A warning that should appear at most once however many times it fires
    /// (repeats, extra voices, dropped ornaments).
    fn warn_once(&mut self, key: &str, msg: String) {
        if self.warned.insert(key.to_string()) {
            self.warnings.push(msg);
        }
    }

    /// `L:` defaults from `M:` the moment the body starts, per the ABC
    /// standard: 1/16 under a meter shorter than 3/4, else 1/8.
    fn start_body(&mut self) {
        if self.body_started {
            return;
        }
        self.body_started = true;
        if self.unit.is_none() {
            let short = self.meter.is_some_and(|m| m.as_f64() < 0.75);
            self.unit = Some(if short {
                Frac::new(1, 16)
            } else {
                Frac::new(1, 8)
            });
        }
    }

    fn unit(&self) -> Frac {
        self.unit.unwrap_or_else(|| Frac::new(1, 8))
    }

    fn field(&mut self, key: char, value: &str) -> Result<(), String> {
        match key {
            'T' => {
                if self.title.is_none() {
                    self.title = Some(value.to_string());
                }
            }
            'M' => {
                let (meter, text) = parse_meter(value).ok_or_else(|| {
                    self.err(format!(
                        "bad meter M:{value} (want `4/4`, `C`, `C|` or `none`)"
                    ))
                })?;
                self.meter = meter;
                self.meter_text = text;
            }
            'L' => {
                self.unit = Some(
                    parse_frac(value)
                        .ok_or_else(|| self.err(format!("bad note length L:{value}")))?,
                );
            }
            'Q' => {
                self.tempo = parse_tempo(value, self.unit());
                if self.tempo.is_none() {
                    self.warn(format!("Q:{value} not understood; tempo ignored"));
                }
            }
            'K' => {
                let (acc, text) =
                    parse_key(value).ok_or_else(|| self.err(format!("bad key K:{value}")))?;
                self.key_acc = acc;
                self.key_text = text;
                self.bar_acc.clear();
            }
            'V' => {
                let name = value.split_whitespace().next().unwrap_or("1").to_string();
                if !self.voices.contains(&name) {
                    self.voices.push(name.clone());
                }
                if self.voices.first() != Some(&name) {
                    let first = self.voices[0].clone();
                    self.warn_once(
                        "voices",
                        format!(
                            "the tune has more than one voice ({}); only V:{first} was imported \
                             — import the others separately with their own --sfx",
                            self.voices.join(", ")
                        ),
                    );
                }
                self.voice = Some(name);
            }
            // X, C, R, N, O, S, Z, W, w, I, P, U … carry no pitch information.
            _ => {}
        }
        Ok(())
    }

    /// True when the line/token belongs to the voice being imported.
    fn active(&self) -> bool {
        match (&self.voice, self.voices.first()) {
            (Some(v), Some(first)) => v == first,
            _ => true,
        }
    }

    fn body(&mut self, line: &str) -> Result<(), String> {
        let b: Vec<char> = line.chars().collect();
        let mut i = 0usize;
        while i < b.len() {
            let c = b[i];
            match c {
                ' ' | '\t' | '\\' | '*' | '$' | '.' | '~' => i += 1,
                ')' => i += 1,
                '(' => {
                    if b.get(i + 1).is_some_and(char::is_ascii_digit) {
                        return Err(self.err(format!(
                            "tuplet `({}` is not supported; write the notes at their true lengths \
                             (e.g. `a/3b/3c/3` for a triplet of eighths)",
                            b[i + 1]
                        )));
                    }
                    i += 1; // slur
                }
                '&' => {
                    return Err(self.err(
                        "voice overlay `&` is not supported; split the parts into V: voices",
                    ));
                }
                '{' => {
                    i = skip_to(&b, i + 1, '}');
                    self.warn_once(
                        "grace",
                        "grace notes `{…}` were dropped (they carry no row of their own)"
                            .to_string(),
                    );
                }
                '"' => i = skip_to(&b, i + 1, '"'),
                '!' => i = skip_to(&b, i + 1, '!'),
                '+' => i = skip_to(&b, i + 1, '+'),
                '-' => {
                    self.tie_pending = true;
                    i += 1;
                }
                '>' | '<' => {
                    let mut n = 0;
                    while b.get(i).copied() == Some(c) {
                        n += 1;
                        i += 1;
                    }
                    // `>` = 3:1, `>>` = 7:1, `>>>` = 15:1 (and `<` mirrored).
                    let long = Frac::new((1 << (n + 1)) - 1, 1 << n);
                    let short = Frac::new(1, 1 << n);
                    let (prev, next) = if c == '>' {
                        (long, short)
                    } else {
                        (short, long)
                    };
                    self.apply_broken_to_last(prev)?;
                    self.broken = Some(next);
                }
                'Z' => {
                    // A whole-measure rest, `Z` or `Z3`.
                    let (mult, j) = read_length(&b, i + 1);
                    let bar = self.meter.unwrap_or_else(|| Frac::new(1, 1));
                    let dur = bar * mult;
                    self.push_rest(dur);
                    i = j;
                }
                'H'..='Y' => i += 1, // single-letter decorations
                '|' | ':' | '[' | ']' => {
                    if c == '[' && is_inline_field(&b, i) {
                        let end = skip_to(&b, i + 1, ']');
                        let field: String =
                            b[i + 1..end.saturating_sub(1).max(i + 1)].iter().collect();
                        if let Some((k, v)) = field.split_once(':') {
                            let key = k.chars().next().unwrap_or('X');
                            self.field(key, v.trim())?;
                        }
                        i = end;
                    } else if let Some(end) = bar_at(&b, i) {
                        let text: String = b[i..end].iter().collect();
                        self.bar(&text);
                        i = end;
                    } else if c == '[' {
                        i = self.chord(&b, i)?;
                    } else {
                        return Err(self.err(format!("unexpected {c:?} in the tune body")));
                    }
                }
                '^' | '_' | '=' | 'A'..='G' | 'a'..='g' | 'z' | 'x' => {
                    i = self.note(&b, i, None)?;
                }
                other => {
                    return Err(self.err(format!("unexpected {other:?} in the tune body")));
                }
            }
        }
        Ok(())
    }

    fn bar(&mut self, text: &str) {
        self.bar_acc.clear();
        if text.contains(':') {
            self.warn_once(
                "repeat",
                format!(
                    "repeat mark `{}` unrolled once: the repeated section was imported a single \
                     time. Repeats belong in __music__ (`pat … loop=<id>`), not in an sfx",
                    text.trim()
                ),
            );
        }
        if text.chars().any(|c| c.is_ascii_digit()) {
            self.warn_once(
                "ending",
                "first/second endings were imported inline, one after the other".to_string(),
            );
        }
        if !self.active() {
            return;
        }
        self.bars += 1;
        if let Some(meter) = self.meter
            && !self.bar_len.is_zero()
            && self.bar_len != meter
            // A pickup (first bar) and the final bar are allowed to be short.
            && self.bars > 1
        {
            self.bad_bars.push((self.bars, self.bar_len.text()));
        }
        self.bar_len = Frac::new(0, 1);
    }

    /// `[CEG]` — monophonic import keeps the lowest-written (first) note.
    fn chord(&mut self, b: &[char], i: usize) -> Result<usize, String> {
        let close = b[i..]
            .iter()
            .position(|&c| c == ']')
            .map(|p| i + p)
            .ok_or_else(|| self.err("unterminated chord `[`"))?;
        let (mult, after) = read_length(b, close + 1);
        let text: String = b[i..=close].iter().collect();
        self.warn_once(
            "chord",
            format!(
                "chord {text} reduced to its first note (the console's sfx rows are monophonic)"
            ),
        );
        let mut j = i + 1;
        let mut first = true;
        while j < close {
            if b[j].is_whitespace() {
                j += 1;
                continue;
            }
            let next = self.note(b, j, if first { Some(mult) } else { None })?;
            if !first {
                // Drop everything but the first chord tone.
                self.events.pop();
            }
            first = false;
            j = next;
        }
        Ok(after)
    }

    /// Parse one note or rest starting at `i`. `scale` multiplies the parsed
    /// length (a chord's post-`]` multiplier).
    fn note(&mut self, b: &[char], i: usize, scale: Option<Frac>) -> Result<usize, String> {
        let mut j = i;
        let (mut sharps, mut flats, mut natural) = (0i32, 0i32, false);
        while j < b.len() {
            match b[j] {
                '^' => {
                    sharps += 1;
                    j += 1;
                }
                '_' => {
                    flats += 1;
                    j += 1;
                }
                '=' => {
                    natural = true;
                    j += 1;
                }
                _ => break,
            }
        }
        let explicit = if sharps > 0 {
            Some(sharps)
        } else if flats > 0 {
            Some(-flats)
        } else if natural {
            Some(0)
        } else {
            None
        };

        let c = *b
            .get(j)
            .ok_or_else(|| self.err("an accidental with no note after it"))?;
        let rest = matches!(c, 'z' | 'x');
        let (pc_index, mut octave) = if rest {
            (0usize, 0i32)
        } else {
            let idx = letter_index(c)
                .ok_or_else(|| self.err(format!("{c:?} is not a note letter (want A-G or a-g)")))?;
            (idx, if c.is_ascii_uppercase() { 4 } else { 5 })
        };
        j += 1;
        while j < b.len() {
            match b[j] {
                ',' => {
                    octave -= 1;
                    j += 1;
                }
                '\'' => {
                    octave += 1;
                    j += 1;
                }
                _ => break,
            }
        }
        let (mut mult, end) = read_length(b, j);
        if let Some(s) = scale {
            mult = mult * s;
        }
        if let Some(bk) = self.broken.take() {
            mult = mult * bk;
        }
        let dur = self.unit() * mult;
        let token: String = b[i..end].iter().collect();

        if !self.active() {
            self.tie_pending = false;
            return Ok(end);
        }
        self.bar_len = self.bar_len + dur;

        if rest {
            self.tie_pending = false;
            self.push_rest(dur);
            return Ok(end);
        }

        // Accidental resolution: explicit wins and is remembered for the rest
        // of the bar; otherwise a bar-local accidental; otherwise the key.
        let acc = match explicit {
            Some(a) => {
                self.bar_acc.insert((pc_index, octave), a);
                a
            }
            None => *self
                .bar_acc
                .get(&(pc_index, octave))
                .unwrap_or(&self.key_acc[pc_index]),
        };
        let note = octave * 12 + LETTER_PC[pc_index] + acc;

        if self.tie_pending {
            self.tie_pending = false;
            let merged = match self.events.last_mut() {
                Some(AbcEvent::Note {
                    note: prev,
                    dur: pd,
                    ..
                }) if *prev == note => {
                    *pd = *pd + dur;
                    true
                }
                _ => false,
            };
            if merged {
                return Ok(end);
            }
            self.warn_once(
                "tie",
                format!("a tie into {token:?} joined two different pitches; it was ignored"),
            );
        }
        self.events.push(AbcEvent::Note {
            note,
            dur,
            token,
            line: self.line,
        });
        Ok(end)
    }

    fn push_rest(&mut self, dur: Frac) {
        if !self.active() || dur.is_zero() {
            return;
        }
        // Adjacent rests merge: they are one silence, and merging keeps the
        // gcd (and therefore the row grid) as coarse as the music allows.
        if let Some(AbcEvent::Rest { dur: prev }) = self.events.last_mut() {
            *prev = *prev + dur;
        } else {
            self.events.push(AbcEvent::Rest { dur });
        }
    }

    /// `>` / `<` reach *backwards*: they lengthen the note already emitted and
    /// leave a multiplier for the next one.
    fn apply_broken_to_last(&mut self, mult: Frac) -> Result<(), String> {
        if self.events.is_empty() {
            return Err(self.err("a broken-rhythm mark (`>`/`<`) with no note before it"));
        }
        let delta = {
            let dur = match self.events.last_mut().expect("non-empty") {
                AbcEvent::Note { dur, .. } | AbcEvent::Rest { dur } => dur,
            };
            let before = *dur;
            *dur = *dur * mult;
            *dur - before
        };
        self.bar_len = self.bar_len + delta;
        Ok(())
    }
}

fn skip_to(b: &[char], from: usize, close: char) -> usize {
    match b[from..].iter().position(|&c| c == close) {
        Some(p) => from + p + 1,
        None => b.len(),
    }
}

/// `[K:Am]` and friends: `[`, a field letter, `:`.
fn is_inline_field(b: &[char], i: usize) -> bool {
    b.get(i + 1).is_some_and(char::is_ascii_alphabetic) && b.get(i + 2) == Some(&':')
}

/// The extent of a bar line at `i`, or `None` when `[` opens a chord.
fn bar_at(b: &[char], i: usize) -> Option<usize> {
    match b[i] {
        '|' => {}
        ':' if matches!(b.get(i + 1), Some('|') | Some(':')) => {}
        '[' if matches!(b.get(i + 1), Some('|')) => {}
        '[' if b.get(i + 1).is_some_and(char::is_ascii_digit) => {}
        ']' => {}
        _ => return None,
    }
    let mut j = i;
    while j < b.len() && matches!(b[j], '|' | ':' | '[' | ']') {
        j += 1;
    }
    // Ending numbers: `|1`, `[2`, `|1,3`.
    while j < b.len() && (b[j].is_ascii_digit() || b[j] == ',') {
        j += 1;
    }
    Some(j)
}

/// ABC's length suffix: `` (1), `2`, `/`, `//`, `/2`, `3/2`.
fn read_length(b: &[char], from: usize) -> (Frac, usize) {
    let mut i = from;
    let mut num: i64 = 0;
    let mut saw_num = false;
    while i < b.len() && b[i].is_ascii_digit() {
        num = num * 10 + b[i].to_digit(10).unwrap() as i64;
        saw_num = true;
        i += 1;
    }
    let mut den: i64 = 1;
    while i < b.len() && b[i] == '/' {
        i += 1;
        let mut d: i64 = 0;
        let mut saw = false;
        while i < b.len() && b[i].is_ascii_digit() {
            d = d * 10 + b[i].to_digit(10).unwrap() as i64;
            saw = true;
            i += 1;
        }
        den *= if saw { d.max(1) } else { 2 };
    }
    (Frac::new(if saw_num { num.max(1) } else { 1 }, den), i)
}

fn parse_frac(s: &str) -> Option<Frac> {
    let s = s.trim();
    match s.split_once('/') {
        Some((a, b)) => Some(Frac::new(a.trim().parse().ok()?, b.trim().parse().ok()?)),
        None => Some(Frac::new(s.parse().ok()?, 1)),
    }
}

fn parse_meter(spec: &str) -> Option<(Option<Frac>, String)> {
    let s = spec.trim();
    match s {
        "C" => Some((Some(Frac::new(4, 4)), "4/4".to_string())),
        "C|" => Some((Some(Frac::new(2, 2)), "2/2".to_string())),
        "none" | "None" => Some((None, "none".to_string())),
        _ => {
            let f = parse_frac(s)?;
            Some((Some(f), s.to_string()))
        }
    }
}

/// `Q:120`, `Q:1/4=120`, `Q:"Allegro" 1/4=120`, `Q:3/8=60`.
fn parse_tempo(spec: &str, unit: Frac) -> Option<(Frac, u32)> {
    // Strip any quoted text ("Allegro").
    let mut clean = String::new();
    let mut in_quotes = false;
    for c in spec.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            _ if !in_quotes => clean.push(c),
            _ => {}
        }
    }
    let clean = clean.trim();
    match clean.split_once('=') {
        Some((left, right)) => {
            let bpm: u32 = right.trim().parse().ok()?;
            // The left side may be several lengths summed: `1/4 1/4=60`.
            let mut total = Frac::new(0, 1);
            for tok in left.split_whitespace() {
                total = total + parse_frac(tok)?;
            }
            if total.is_zero() || bpm == 0 {
                return None;
            }
            Some((total, bpm))
        }
        None => {
            let bpm: u32 = clean.parse().ok()?;
            if bpm == 0 { None } else { Some((unit, bpm)) }
        }
    }
}

/// `K:` — tonic, optional accidental, optional mode. Returns the per-letter
/// accidental table plus a human description.
fn parse_key(spec: &str) -> Option<([i32; 7], String)> {
    let s = spec.trim();
    let head = s.split_whitespace().next().unwrap_or("");
    if head.is_empty() || head.eq_ignore_ascii_case("none") {
        return Some(([0; 7], "none (no key signature)".to_string()));
    }
    let mut chars = head.chars();
    let tonic_char = chars.next()?;
    let tonic_idx = letter_index(tonic_char)?;
    let mut rest: String = chars.collect();
    // Position on the circle of fifths of each natural tonic's major key:
    // F=-1 C=0 G=1 D=2 A=3 E=4 B=5 (indexed C D E F G A B).
    let mut fifths: i64 = [0, 2, 4, -1, 1, 3, 5][tonic_idx];
    let mut tonic = tonic_char.to_ascii_uppercase().to_string();
    if rest.starts_with('#') {
        fifths += 7;
        tonic.push('#');
        rest = rest[1..].to_string();
    } else if rest.starts_with('b') && rest.len() > 1 || rest == "b" {
        // `Bb` vs `Bmin`: a lone `b`, or `b` followed by a mode, is a flat.
        let after = &rest[1..];
        if after.is_empty()
            || !after.starts_with(|c: char| c.is_ascii_alphabetic())
            || is_mode(after)
        {
            fifths -= 7;
            tonic.push('b');
            rest = after.to_string();
        }
    }

    let mode_key: String = rest
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    let (offset, mode_name) = mode_offset(&mode_key)?;
    fifths += offset;

    let mut acc = [0i32; 7];
    if fifths > 0 {
        for &l in SHARP_ORDER.iter().take((fifths as usize).min(7)) {
            acc[l] = 1;
        }
    } else if fifths < 0 {
        for &l in FLAT_ORDER.iter().take(((-fifths) as usize).min(7)) {
            acc[l] = -1;
        }
    }

    let names = ["C", "D", "E", "F", "G", "A", "B"];
    let listed: Vec<String> = (0..7)
        .filter(|&i| acc[i] != 0)
        .map(|i| format!("{}{}", names[i], if acc[i] > 0 { "#" } else { "b" }))
        .collect();
    let text = if listed.is_empty() {
        format!("{tonic} {mode_name} (no accidentals)")
    } else {
        format!("{tonic} {mode_name} ({})", listed.join(", "))
    };
    Some((acc, text))
}

fn is_mode(s: &str) -> bool {
    mode_offset(
        &s.chars()
            .filter(|c| c.is_ascii_alphabetic())
            .collect::<String>()
            .to_ascii_lowercase(),
    )
    .is_some()
}

/// Mode → offset in fifths from the same-tonic major, plus a display name.
fn mode_offset(mode: &str) -> Option<(i64, &'static str)> {
    let m: String = mode.chars().take(3).collect();
    Some(match m.as_str() {
        "" => (0, "major"),
        "maj" | "ion" => (0, "major"),
        "m" | "min" | "aeo" => (-3, "minor"),
        "mix" => (-1, "mixolydian"),
        "dor" => (-2, "dorian"),
        "phr" => (-4, "phrygian"),
        "lyd" => (1, "lydian"),
        "loc" => (-5, "locrian"),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Import planning
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedRow {
    Note(u8),
    Rest,
}

#[derive(Debug, Clone)]
pub struct ImportOpts {
    pub sfx_start: u8,
    pub voice: String,
    pub vol: u8,
    pub speed: Option<u8>,
    pub transpose: i32,
    pub force: bool,
}

impl Default for ImportOpts {
    fn default() -> ImportOpts {
        ImportOpts {
            sfx_start: 0,
            // Wave 2 (50% square) is the plainest melodic voice on the
            // console and needs no `__instruments__` section to exist.
            voice: "2".to_string(),
            vol: 5,
            speed: None,
            transpose: 0,
            force: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportPlan {
    pub rows: Vec<PlannedRow>,
    /// Duration of one row, in whole notes.
    pub row_dur: Frac,
    pub speed: u8,
    pub speed_source: String,
    /// `(sfx id, row count)`, in order.
    pub chunks: Vec<(u8, usize)>,
    pub notes: usize,
    pub rests: usize,
    /// Suggested `bpm=`/`rows_per_beat=` header for `__music__`.
    pub tempo_hint: Option<String>,
    /// Suggested `pat` lines, one per sfx (they chain by "next existing id").
    pub pattern_hint: Vec<String>,
    pub warnings: Vec<String>,
}

/// Turn a parsed tune into rows, sfx ids and a speed.
pub fn plan_import(tune: &AbcTune, cart: &Cart, opts: &ImportOpts) -> Result<ImportPlan, String> {
    let mut warnings = tune.warnings.clone();

    // One row = gcd of every event's duration.
    let den = tune
        .events
        .iter()
        .fold(1i64, |acc, e| lcm(acc, e.dur().den));
    let nums: Vec<i64> = tune
        .events
        .iter()
        .map(|e| e.dur().num * (den / e.dur().den))
        .collect();
    let g = nums
        .iter()
        .fold(0u64, |acc, &n| gcd(acc, n.unsigned_abs()))
        .max(1) as i64;
    let row_dur = Frac::new(g, den);

    range_check(tune, opts)?;

    let mut rows: Vec<PlannedRow> = Vec::new();
    let (mut notes, mut rests) = (0usize, 0usize);
    for (event, n) in tune.events.iter().zip(&nums) {
        let count = (n / g) as usize;
        match event {
            AbcEvent::Note { note, .. } => {
                notes += 1;
                let value = (note + opts.transpose) as u8;
                for _ in 0..count {
                    rows.push(PlannedRow::Note(value));
                }
            }
            AbcEvent::Rest { .. } => {
                rests += 1;
                for _ in 0..count {
                    rows.push(PlannedRow::Rest);
                }
            }
        }
    }
    if rows.is_empty() {
        return Err("the ABC input produced no rows".to_string());
    }

    // Split at the platform's row cap. Held rows are repeated note rows, so a
    // split never lands "inside" a note: the next sfx simply restates it.
    let chunk_count = rows.len().div_ceil(MAX_SFX_ROWS);
    let last_id = usize::from(opts.sfx_start) + chunk_count - 1;
    if last_id > usize::from(MAX_ID) {
        return Err(format!(
            "the tune needs {chunk_count} sfx ({} rows at {MAX_SFX_ROWS} rows each) starting at \
             {}, which runs past the last sfx id {MAX_ID}",
            rows.len(),
            opts.sfx_start
        ));
    }
    let mut chunks: Vec<(u8, usize)> = Vec::new();
    let mut left = rows.len();
    for k in 0..chunk_count {
        let id = opts.sfx_start + k as u8;
        let n = left.min(MAX_SFX_ROWS);
        chunks.push((id, n));
        left -= n;
    }

    for &(id, _) in &chunks {
        if cart.sfx(id).is_some() && !opts.force {
            return Err(format!(
                "sfx {id} already exists; pass --force to overwrite it (the tune needs sfx {})",
                chunks
                    .iter()
                    .map(|(i, _)| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    let (speed, speed_source) = resolve_speed(tune, row_dur, opts, &mut warnings);
    let tempo_hint = tempo_hint(tune, row_dur, speed);
    let pattern_hint = pattern_hint(cart, &chunks);

    Ok(ImportPlan {
        rows,
        row_dur,
        speed,
        speed_source,
        chunks,
        notes,
        rests,
        tempo_hint,
        pattern_hint,
        warnings,
    })
}

/// Every note must land inside C0-B7 after `--transpose`. The error computes
/// the nearest transpose that *would* fit, so the retry is one flag away.
fn range_check(tune: &AbcTune, opts: &ImportOpts) -> Result<(), String> {
    let (mut lo, mut hi) = (i32::MAX, i32::MIN);
    for e in &tune.events {
        if let AbcEvent::Note { note, .. } = e {
            lo = lo.min(*note);
            hi = hi.max(*note);
        }
    }
    if lo == i32::MAX {
        return Ok(());
    }
    for e in &tune.events {
        let AbcEvent::Note {
            note, token, line, ..
        } = e
        else {
            continue;
        };
        let value = note + opts.transpose;
        if (0..=95).contains(&value) {
            continue;
        }
        let (fit_lo, fit_hi) = (-lo, 95 - hi);
        let where_ = format!("abc line {line}: {token:?}");
        return Err(if fit_lo <= fit_hi {
            let nearest = opts.transpose.clamp(fit_lo, fit_hi);
            format!(
                "{where_} maps to semitone {value}{}, outside the console's C0-B7 note table \
                 (0-95). The tune fits any --transpose in {fit_lo:+}..={fit_hi:+}; {} \
                 --transpose {nearest}",
                if opts.transpose == 0 {
                    String::new()
                } else {
                    format!(" (after --transpose {:+})", opts.transpose)
                },
                if opts.transpose == 0 {
                    "the nearest fitting one is"
                } else {
                    "nearest to the one requested is"
                },
            )
        } else {
            format!(
                "{where_} maps to {value}, outside C0-B7, and the tune's own range \
                 ({} semitones) is wider than the 96-semitone note table, so no --transpose \
                 fits it whole",
                hi - lo + 1
            )
        });
    }
    Ok(())
}

/// Frames per row: `--speed` wins, else the `Q:` tempo, else an assumed
/// quarter = 120.
fn resolve_speed(
    tune: &AbcTune,
    row_dur: Frac,
    opts: &ImportOpts,
    warnings: &mut Vec<String>,
) -> (u8, String) {
    if let Some(s) = opts.speed {
        return (s, "--speed".to_string());
    }
    let (unit, bpm, source) = match tune.tempo {
        Some((unit, bpm)) => (unit, bpm, format!("Q:{}={bpm}", unit.text())),
        None => (
            Frac::new(1, 4),
            120,
            "no Q: field, assuming 1/4=120".to_string(),
        ),
    };
    // One row lasts `row_dur / unit` tempo units, each `60/bpm` seconds, at 60
    // frames per second: frames = row_dur * 3600 / (unit * bpm).
    let frames = row_dur.as_f64() * 3600.0 / (unit.as_f64() * f64::from(bpm));
    let rounded = frames.round();
    if !(1.0..=255.0).contains(&rounded) {
        warnings.push(format!(
            "{source} works out to {frames:.2} frames per row, outside speed=1-255; \
             clamped — pass --speed to choose one"
        ));
    }
    let speed = rounded.clamp(1.0, 255.0) as u8;
    if (frames - f64::from(speed)).abs() > 0.01 {
        warnings.push(format!(
            "speed={speed} is {frames:.2} frames per row rounded; the tune plays \
             {:+.1}% off the notated tempo",
            (f64::from(speed) / frames - 1.0) * 100.0
        ));
    }
    (speed, source)
}

/// A `bpm=`/`rows_per_beat=` line that reproduces this row rate exactly, when
/// one exists — the sugar `__music__` already understands.
fn tempo_hint(tune: &AbcTune, row_dur: Frac, speed: u8) -> Option<String> {
    let (unit, bpm) = tune.tempo.unwrap_or((Frac::new(1, 4), 120));
    // Quarter-note bpm, and rows per quarter note.
    let quarter = Frac::new(1, 4);
    let qbpm = (f64::from(bpm) * unit.as_f64() / quarter.as_f64()).round();
    let per_beat = Frac::new(quarter.num * row_dur.den, quarter.den * row_dur.num);
    if per_beat.den != 1
        || per_beat.num < 1
        || per_beat.num > 255
        || !(1.0..=1000.0).contains(&qbpm)
    {
        return None;
    }
    let r = per_beat.num;
    let resolved = (3600.0 / (qbpm * r as f64)).round() as i64;
    let note = if resolved == i64::from(speed) {
        format!(" (speed=auto then resolves to {speed})")
    } else {
        format!(" (speed=auto would resolve to {resolved}, not {speed})")
    };
    Some(format!("bpm={} rows_per_beat={r}{note}", qbpm as i64))
}

/// One pattern per sfx, using free pattern ids. Consecutive ids chain by the
/// sequencer's "play the next existing pattern id" rule, so no `loop=` is
/// needed to hear the whole tune.
fn pattern_hint(cart: &Cart, chunks: &[(u8, usize)]) -> Vec<String> {
    let used: Vec<u8> = cart.audio().pattern_ids().collect();
    let mut next = 0u8;
    let mut out = Vec::new();
    for (k, (sfx, _)) in chunks.iter().enumerate() {
        while used.contains(&next) && next < MAX_ID {
            next += 1;
        }
        let last = k + 1 == chunks.len();
        let tail = if last { " stop" } else { "" };
        out.push(format!("pat {next}{tail} : {sfx} - - -"));
        next = next.saturating_add(1);
    }
    out
}

impl ImportPlan {
    /// The `__sfx__` text for one chunk.
    fn block_text(&self, index: usize, opts: &ImportOpts) -> Vec<String> {
        let (id, count) = self.chunks[index];
        let start: usize = self.chunks[..index].iter().map(|(_, n)| n).sum();
        let mut out = vec![format!("sfx {id} speed={}", self.speed)];
        for row in &self.rows[start..start + count] {
            out.push(match row {
                PlannedRow::Note(n) => format!("{} {} {}", note_name(*n), opts.voice, opts.vol),
                PlannedRow::Rest => "---".to_string(),
            });
        }
        out
    }

    /// The human/agent-facing summary — the whole point of the command, and
    /// identical under `--dry-run`.
    pub fn summary(&self, tune: &AbcTune, opts: &ImportOpts) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!(
            "import-abc: {}{} note(s), {} rest(s) -> {} row(s)",
            tune.title
                .as_ref()
                .map(|t| format!("{t:?}: "))
                .unwrap_or_default(),
            self.notes,
            self.rests,
            self.rows.len()
        ));
        out.push(format!(
            "  key: {} | meter: {} | L:{} default note",
            tune.key_text,
            tune.meter_text,
            tune.unit.text()
        ));
        out.push(format!(
            "  1 row = {} note; speed={} frames per row ({})",
            self.row_dur.text(),
            self.speed,
            self.speed_source
        ));
        let ids: Vec<String> = self
            .chunks
            .iter()
            .map(|(id, n)| format!("{id} ({n} rows)"))
            .collect();
        out.push(format!("  sfx ids: {}", ids.join(", ")));
        if self.chunks.len() > 1 {
            let mut at = 0usize;
            let points: Vec<String> = self.chunks[..self.chunks.len() - 1]
                .iter()
                .map(|(_, n)| {
                    at += n;
                    format!("row {at}")
                })
                .collect();
            out.push(format!(
                "  split at {} (the {}-row cap per sfx); a held note simply restates its row \
                 in the next sfx",
                points.join(", "),
                MAX_SFX_ROWS
            ));
        }
        if opts.transpose != 0 {
            out.push(format!("  transposed {:+} semitone(s)", opts.transpose));
        }
        out.push(format!(
            "  voice: {} at vol {} on every note row",
            opts.voice, opts.vol
        ));
        if let Some(hint) = &self.tempo_hint {
            out.push(format!("  suggested __music__ tempo header: {hint}"));
        }
        out.push("  suggested __music__ pattern(s):".to_string());
        for line in &self.pattern_hint {
            out.push(format!("    {line}"));
        }
        if self.rows.len() > self.notes + self.rests {
            out.push(
                "  note: held notes repeat their row (the console has no note-off). That is \
                 sample-identical to a sustain on a wave digit or a flat instrument; an `env`, \
                 `sweep` or `duck` instrument re-attacks on every repeat."
                    .to_string(),
            );
        }
        for w in &self.warnings {
            out.push(format!("  warning: {w}"));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Cart rewrite
// ---------------------------------------------------------------------------

/// Splice the planned sfx entries into `text`. Pure — no file I/O.
pub fn run_import(text: &str, abc: &str, opts: &ImportOpts) -> Result<EditResult, String> {
    let cart = Cart::parse(text).map_err(|e| format!("cart: {e}"))?;
    validate_voice(&cart, &opts.voice)?;
    if opts.vol > MAX_VOL {
        return Err(format!("bad --vol {} (want 0-{MAX_VOL})", opts.vol));
    }
    let tune = parse_abc(abc)?;
    let plan = plan_import(&tune, &cart, opts)?;

    let lines: Vec<&str> = text.split('\n').collect();
    let layout = sfxtext::locate(&lines);
    let mut rw = Rewrite::default();
    // Group the new entries by insertion point so several new ids landing in
    // the same gap stay in ascending order.
    let mut pending: BTreeMap<Option<usize>, Vec<String>> = BTreeMap::new();
    for (index, &(id, _)) in plan.chunks.iter().enumerate() {
        let block = plan.block_text(index, opts);
        match layout.block(id) {
            Some(existing) => {
                let (header, rows) = block.split_first().expect("header plus rows");
                sfxtext::set_block(
                    &mut rw,
                    &lines,
                    existing,
                    Some(header.clone()),
                    rows.to_vec(),
                );
            }
            None => {
                let slot = pending.entry(layout.insert_point_for(id)).or_default();
                if !slot.is_empty() {
                    // One blank line between consecutive new entries, the way
                    // `__sfx__` is conventionally laid out.
                    slot.push(String::new());
                }
                slot.extend(block);
            }
        }
    }
    for (at, block) in pending {
        match at {
            Some(at) => rw.insert_entry(&lines, at, block),
            None => {
                let anchor = sfxtext::new_section_anchor(&lines);
                rw.insert_new_section(&lines, anchor, block);
            }
        }
    }

    sfxtext::finish(text, &rw, plan.summary(&tune, opts), "music import-abc")
}

/// The `--inst` value must name something the cart can play. Same rule as
/// `music edit set-inst`.
fn validate_voice(cart: &Cart, token: &str) -> Result<(), String> {
    if let Ok(digit) = token.parse::<u8>() {
        if digit == WAVE_FM {
            return Err(format!(
                "--inst {WAVE_FM}: wave {WAVE_FM} is the 2-op FM oscillator, which a bare digit \
                 cannot describe; name an `inst … wave={WAVE_FM} fm=…` instrument instead"
            ));
        }
        if digit > 5 {
            return Err(format!("--inst {digit}: bad wave digit (want 0-5)"));
        }
        return Ok(());
    }
    if let Some(slot) = token.strip_prefix('w')
        && let Ok(slot) = slot.parse::<u8>()
        && slot < (WAVE_TABLE_BASE)
    {
        return match cart.audio().wavetable(slot) {
            Some(_) => Ok(()),
            None => Err(format!(
                "--inst w{slot}: this cart defines no wavetable in slot {slot}"
            )),
        };
    }
    if cart.audio().instrument(token).is_some() {
        return Ok(());
    }
    let names: Vec<&str> = cart
        .audio()
        .instruments()
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    Err(format!(
        "--inst {token:?}: want a wave digit 0-5, a wavetable w0-w7, or one of: {}",
        if names.is_empty() {
            "this cart has no __instruments__".to_string()
        } else {
            names.join(", ")
        }
    ))
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub const IMPORT_USAGE: &str = "\
usage:
  console music import-abc <cart> <file.abc|-> --sfx <start-id>
      [--inst <name|0-5|w0-w7>] [--vol <0-7>] [--speed <1-255>]
      [--transpose <n>] [--force] [--dry-run]
  (imports a MONOPHONIC ABC tune into consecutive sfx ids; `-` reads the tune
   from stdin. Prints the rows used, the sfx ids written, the split points and
   a suggested `pat` line for __music__.)";

pub fn cli_import(args: &[String]) -> i32 {
    let (cart_path, abc_path, opts, dry_run) = match parse_import_args(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}\n{IMPORT_USAGE}");
            return 2;
        }
    };
    let text = match std::fs::read_to_string(&cart_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read {cart_path:?}: {e}");
            return 2;
        }
    };
    let abc = match read_abc(&abc_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    apply_edit_result(&cart_path, run_import(&text, &abc, &opts), dry_run)
}

fn read_abc(path: &str) -> Result<String, String> {
    if path == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("reading ABC from stdin: {e}"))?;
        Ok(buf)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {path:?}: {e}"))
    }
}

type ImportArgs = (String, String, ImportOpts, bool);

fn parse_import_args(args: &[String]) -> Result<ImportArgs, String> {
    let mut opts = ImportOpts::default();
    let mut dry_run = false;
    let mut positional: Vec<String> = Vec::new();
    let mut sfx_seen = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |what: &str| -> Result<String, String> {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{what} requires a value"))
        };
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--force" => opts.force = true,
            "--sfx" => {
                opts.sfx_start = parse_u8(&value("--sfx")?, "--sfx", MAX_ID)?;
                sfx_seen = true;
            }
            "--inst" => opts.voice = value("--inst")?,
            "--vol" => opts.vol = parse_u8(&value("--vol")?, "--vol", MAX_VOL)?,
            "--speed" => {
                let s = parse_u8(&value("--speed")?, "--speed", 255)?;
                if s == 0 {
                    return Err("--speed must be 1-255".to_string());
                }
                opts.speed = Some(s);
            }
            "--transpose" => {
                let v = value("--transpose")?;
                opts.transpose = v
                    .strip_prefix('+')
                    .unwrap_or(&v)
                    .parse()
                    .map_err(|_| format!("bad --transpose {v:?} (want a signed integer)"))?;
            }
            other if other.starts_with("--") => return Err(format!("unknown flag {other:?}")),
            other => positional.push(other.to_string()),
        }
    }
    if !sfx_seen {
        return Err("import-abc requires --sfx <start-id>".to_string());
    }
    match positional.len() {
        2 => Ok((positional[0].clone(), positional[1].clone(), opts, dry_run)),
        n => Err(format!(
            "import-abc takes a cart path and an ABC path (or `-`), got {n} positional argument(s)"
        )),
    }
}

fn parse_u8(s: &str, what: &str, max: u8) -> Result<u8, String> {
    let v: u8 = s
        .parse()
        .map_err(|_| format!("bad {what} {s:?} (want 0-{max})"))?;
    if v > max {
        return Err(format!("bad {what} {v} (want 0-{max})"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frac_normalizes() {
        assert_eq!(Frac::new(4, 8), Frac::new(1, 2));
        assert_eq!((Frac::new(1, 4) * Frac::new(3, 2)), Frac::new(3, 8));
        assert_eq!((Frac::new(1, 8) + Frac::new(1, 8)), Frac::new(1, 4));
    }

    #[test]
    fn read_length_covers_the_abc_forms() {
        let chars: Vec<char> = "2 / // /2 3/2 ".chars().collect();
        assert_eq!(read_length(&chars, 0).0, Frac::new(2, 1));
        assert_eq!(read_length(&chars, 2).0, Frac::new(1, 2));
        assert_eq!(read_length(&chars, 4).0, Frac::new(1, 4));
        assert_eq!(read_length(&chars, 7).0, Frac::new(1, 2));
        assert_eq!(read_length(&chars, 10).0, Frac::new(3, 2));
        // No suffix at all is one default length.
        assert_eq!(read_length(&chars, 1).0, Frac::new(1, 1));
    }

    #[test]
    fn key_signatures_follow_the_circle_of_fifths() {
        let (acc, text) = parse_key("D").unwrap();
        // F# and C#.
        assert_eq!(acc, [1, 0, 0, 1, 0, 0, 0]);
        assert_eq!(text, "D major (C#, F#)");

        let (acc, _) = parse_key("Bb").unwrap();
        // Bb and Eb.
        assert_eq!(acc, [0, 0, -1, 0, 0, 0, -1]);

        // A dorian = 2 fifths flat of A major (3 sharps) = 1 sharp (F#).
        let (acc, text) = parse_key("Ador").unwrap();
        assert_eq!(acc, [0, 0, 0, 1, 0, 0, 0]);
        assert!(text.starts_with("A dorian"), "{text}");

        // E minor = 1 sharp.
        let (acc, text) = parse_key("Em").unwrap();
        assert_eq!(acc, [0, 0, 0, 1, 0, 0, 0]);
        assert!(text.starts_with("E minor"), "{text}");

        assert_eq!(parse_key("none").unwrap().0, [0; 7]);
    }

    #[test]
    fn tempo_forms() {
        let l = Frac::new(1, 8);
        assert_eq!(parse_tempo("1/4=120", l), Some((Frac::new(1, 4), 120)));
        assert_eq!(
            parse_tempo("\"Allegro\" 1/4=132", l),
            Some((Frac::new(1, 4), 132))
        );
        assert_eq!(parse_tempo("90", l), Some((Frac::new(1, 8), 90)));
    }
}

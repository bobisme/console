//! Music authoring tools (SPEC.md "Music authoring (PoC v2)"), the third
//! member of the `sprite` / `map` family and the same three-layer split:
//! ground truth as text ([`score`]), numbers ([`lint`]), pictures and ears
//! ([`roll`], [`render`]).
//!
//! Where the sprite tools let an agent *see* pixels it cannot see, these let
//! an agent read a song it cannot hear **at cart level** — no stepping, no
//! input scripts, no diffing `audio_events` frame by frame. Everything here
//! except [`render`] works straight off a parsed [`Cart`], so it is cheap and
//! has no side effects.
//!
//! [`transform`] and [`abc`] are the write half, the music counterparts of
//! `sprite edit` / `map edit`: score-level transforms (`music edit`) and ABC
//! import (`music import-abc`), both rewriting only the changed lines of
//! `__sfx__` through [`sfxtext`]. Like every other cart-mutating tool here
//! they are **CLI only** — see the note in `rpc.rs`.
//!
//! The one concept the whole module shares is the **song plan**
//! ([`SongPlan`]): `__music__` is not a flat list of patterns but a chain —
//! each pattern says `stop`, `loop=<id>` or (by default) "play the next
//! existing id" — so a song has a *form*, an intro that plays once and a body
//! that repeats. Every tool here resolves that chain first and reports it,
//! because form is the thing that is hardest to see in the cart text and
//! easiest to get wrong.

pub mod abc;
pub mod lint;
pub mod render;
pub mod roll;
pub mod score;
pub mod sfxtext;
pub mod transform;

use std::collections::BTreeMap;

use console_core::{CHANNEL_COUNT, Cart, PatternEnd, SAMPLE_RATE, SAMPLES_PER_FRAME};

/// Frames per second — the sequencer's own clock (`speed=` is in frames).
pub const FPS: f64 = SAMPLE_RATE as f64 / SAMPLES_PER_FRAME as f64;

/// One visit to a pattern in a song's play order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayStep {
    pub pattern: u8,
    /// Frames this pattern occupies: one pass of its longest slot (SPEC:
    /// "Pattern duration = one pass of `max(rows*speed)` over its sfx").
    pub frames: u32,
}

/// How a song's chain finishes, once the plan stops discovering new patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SongEnd {
    /// The last pattern says `stop` — a one-shot jingle.
    Stop,
    /// The last pattern falls through to "the next existing pattern id" and
    /// there is none, so music simply halts. Legal, but almost always a
    /// missing `loop=`/`stop` (see the `chain_has_no_terminator` lint).
    FellOffEnd,
    /// The chain re-enters a pattern it already played: the loop body.
    Loop { target: u8 },
    /// The chain names a pattern the cart never defines. Only reachable via a
    /// bad `--song` id, since `loop=` targets are validated at parse time.
    Undefined(u8),
}

/// A song resolved from its `__music__` chain: every pattern it visits, in
/// play order, plus where (if anywhere) it starts repeating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongPlan {
    /// The pattern `music(n)` was called with.
    pub start: u8,
    /// Distinct pattern visits in play order. A looping song visits each of
    /// its patterns exactly once here; the repeat lives in `loop_index`.
    pub steps: Vec<PlayStep>,
    /// Index into `steps` where the loop body begins, when the song loops.
    pub loop_index: Option<usize>,
    pub end: SongEnd,
}

impl SongPlan {
    /// Frames of the intro — everything before the loop body (the whole song
    /// when it does not loop).
    pub fn intro_frames(&self) -> u64 {
        let upto = self.loop_index.unwrap_or(self.steps.len());
        self.steps[..upto].iter().map(|s| u64::from(s.frames)).sum()
    }

    /// Frames of one pass of the loop body (0 when the song does not loop).
    pub fn loop_frames(&self) -> u64 {
        match self.loop_index {
            Some(i) => self.steps[i..].iter().map(|s| u64::from(s.frames)).sum(),
            None => 0,
        }
    }

    /// Total frames for the intro plus `loops` passes of the loop body. A
    /// non-looping song ignores `loops` — it plays once and stops.
    pub fn frames_for(&self, loops: u32) -> u64 {
        self.intro_frames() + self.loop_frames() * u64::from(loops)
    }

    /// The pattern the loop jumps back to.
    pub fn loop_pattern(&self) -> Option<u8> {
        self.loop_index.map(|i| self.steps[i].pattern)
    }

    /// The song's form on one line — the whole point of `music score`:
    ///
    /// ```text
    /// pat 0 -> pat 1 -> [pat 2 -> pat 3 ->] loop to 2
    /// pat 8 -> pat 9 -> stop
    /// ```
    ///
    /// The bracketed span is the loop body; everything before it is the
    /// intro. A song with no intro is all brackets, which reads correctly
    /// too.
    pub fn chain_text(&self) -> String {
        let mut out = String::new();
        for (i, step) in self.steps.iter().enumerate() {
            if self.loop_index == Some(i) {
                out.push('[');
            }
            out.push_str(&format!("pat {} -> ", step.pattern));
        }
        match self.end {
            SongEnd::Loop { target } => {
                // Close the bracket right after the last arrow, so the body
                // reads as one repeating span: `[pat 2 -> pat 3 ->] loop to 2`.
                out.truncate(out.trim_end().len());
                out.push_str(&format!("] loop to {target}"));
            }
            SongEnd::Stop => out.push_str("stop"),
            SongEnd::FellOffEnd => out.push_str("end (no further pattern)"),
            SongEnd::Undefined(id) => out.push_str(&format!("pat {id} (UNDEFINED)")),
        }
        out
    }

    /// Every pattern the song visits, ascending — what `piano-roll` draws and
    /// `lint` calls "reachable".
    pub fn pattern_ids(&self) -> Vec<u8> {
        self.steps.iter().map(|s| s.pattern).collect()
    }
}

/// Frames one pass of pattern `id` takes: the longest of its slots' sfx
/// durations (`rows * speed`), floored at 1 — exactly what the sequencer
/// computes in `start_pattern`. Sfx `loop=` flags are ignored under music,
/// per SPEC.
pub fn pattern_frames(cart: &Cart, id: u8) -> u32 {
    let Some(pat) = cart.pattern(id) else {
        return 0;
    };
    let mut frames = 1u32;
    for slot in pat.slots.iter().flatten() {
        if let Some(sfx) = cart.sfx(*slot) {
            frames = frames.max(sfx.duration());
        }
    }
    frames
}

/// Resolve the `__music__` chain starting at pattern `start` into a
/// [`SongPlan`].
///
/// The walk follows exactly the rule the sequencer follows when a pattern
/// ends — `stop` halts, `loop=<id>` jumps, otherwise the next existing
/// pattern id — and terminates as soon as it would revisit a pattern, which
/// is the loop point. `PatternEnd::Next` can never cycle (it only ever moves
/// to a strictly higher id), so a cycle always means an explicit `loop=`.
pub fn plan_song(cart: &Cart, start: u8) -> Result<SongPlan, String> {
    if cart.pattern(start).is_none() {
        return Err(format!(
            "cart has no pattern {start} (defined: {})",
            pattern_id_list(cart)
        ));
    }

    let mut steps: Vec<PlayStep> = Vec::new();
    let mut index_of: BTreeMap<u8, usize> = BTreeMap::new();
    let mut cur = start;
    let mut loop_index = None;
    let end;

    loop {
        index_of.insert(cur, steps.len());
        steps.push(PlayStep {
            pattern: cur,
            frames: pattern_frames(cart, cur),
        });
        let pat = cart.pattern(cur).expect("walked into a defined pattern");
        let next = match pat.end {
            PatternEnd::Stop => {
                end = SongEnd::Stop;
                break;
            }
            PatternEnd::Loop(target) => target,
            PatternEnd::Next => match cart.audio().next_pattern_after(cur) {
                Some(n) => n,
                None => {
                    end = SongEnd::FellOffEnd;
                    break;
                }
            },
        };
        if cart.pattern(next).is_none() {
            end = SongEnd::Undefined(next);
            break;
        }
        if let Some(&i) = index_of.get(&next) {
            loop_index = Some(i);
            end = SongEnd::Loop { target: next };
            break;
        }
        cur = next;
    }

    Ok(SongPlan {
        start,
        steps,
        loop_index,
        end,
    })
}

/// The song a command uses when `--song` is omitted: the lowest defined
/// pattern id, i.e. what `music(0)`-style carts mean by "the song".
pub fn default_song(cart: &Cart) -> Result<u8, String> {
    cart.audio()
        .pattern_ids()
        .next()
        .ok_or_else(|| "cart has no __music__ patterns".to_string())
}

/// Defined pattern ids as `"0, 1, 2"`, for error messages.
pub fn pattern_id_list(cart: &Cart) -> String {
    let ids: Vec<String> = cart.audio().pattern_ids().map(|i| i.to_string()).collect();
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.join(", ")
    }
}

/// Frames as seconds, rounded to hundredths — every duration this module
/// prints goes through here so the units are consistent.
pub fn seconds(frames: u64) -> f64 {
    ((frames as f64 / FPS) * 100.0).round() / 100.0
}

/// The integer literals a cart's Lua passes to `music(...)` — the only
/// evidence in a cart of which patterns are actually *entered* (nothing else
/// distinguishes a song head from a pattern in the middle of a chain).
///
/// Deliberately a lexical scan, not an evaluation: it finds `music(3)` and
/// misses `music(n)`, which is exactly the honest signal. `lint` reports how
/// it derived its entry points so a reader can tell which case they are in.
pub fn music_call_args(lua: &str) -> Vec<i64> {
    scan_calls(lua, "music").args
}

/// The integer literals a cart's Lua passes as `sfx(...)`'s first argument.
pub fn sfx_call_args(lua: &str) -> Vec<i64> {
    scan_calls(lua, "sfx").args
}

/// True when the cart calls `music(...)` with something that is not an
/// integer literal — `music(entry)`, `music(songs[i])`.
///
/// This is the "I cannot know" signal, and it matters: with a computed
/// argument, *any* pattern might be a song head, so the reachability lint has
/// no ground to stand on and says so instead of crying wolf.
pub fn has_dynamic_music_call(lua: &str) -> bool {
    let scan = scan_calls(lua, "music");
    scan.total > scan.args.len()
}

/// What [`scan_calls`] found: the integer literals, plus how many calls there
/// were in total (so a caller can tell "no calls" from "calls it could not
/// read").
struct CallScan {
    args: Vec<i64>,
    total: usize,
}

/// Shared scanner for `<name>(<int literal>` occurrences. Requires the call
/// name to be a whole identifier (so `mymusic(2)` does not match) and only
/// reads the first argument.
fn scan_calls(lua: &str, name: &str) -> CallScan {
    let bytes = lua.as_bytes();
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut from = 0usize;
    while let Some(hit) = lua[from..].find(name) {
        let at = from + hit;
        from = at + name.len();
        // Whole-identifier check on the left (`a.music(` and `mymusic(` are
        // not calls to the API's `music`).
        if at > 0 {
            let prev = bytes[at - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.' || prev == b':' {
                continue;
            }
        }
        let mut i = from;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'(' {
            continue;
        }
        total += 1;
        i += 1;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let neg = i < bytes.len() && bytes[i] == b'-';
        if neg {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            continue;
        }
        // Reject `music(1.5)` / `music(1e3)` and identifiers glued to digits.
        if i < bytes.len()
            && (bytes[i] == b'.' || bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
        {
            continue;
        }
        if let Ok(v) = lua[start..i].parse::<i64>() {
            out.push(if neg { -v } else { v });
        }
    }
    CallScan { args: out, total }
}

/// A minimal cart carrying this cart's audio sections verbatim plus `lua` as
/// its whole program — the vehicle for rendering audio without running the
/// game.
///
/// `lint`'s per-pattern peak and `render`'s `--pattern`-style probes both
/// need "just the synth, please": the real cart's `_update` draws, reads
/// input and may start its own sfx, none of which belongs in a measurement
/// of what one pattern sounds like. Copying the raw section text (rather than
/// re-serializing the parsed bank) keeps instruments, wavetables, the master
/// line and the echo line bit-identical to the original, so the samples are
/// the samples the game would produce.
pub fn audio_only_cart(cart: &Cart, lua: &str) -> String {
    let mut out = String::from("__lua__\n");
    out.push_str(lua);
    if !lua.ends_with('\n') {
        out.push('\n');
    }
    for section in ["instruments", "sfx", "music"] {
        if let Some(text) = cart.section(section) {
            out.push_str(&format!("__{section}__\n"));
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// Read and parse a cart file, with the error messages the CLI wants.
pub fn read_cart(path: &str) -> Result<(String, Cart), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path:?}: {e}"))?;
    let cart = Cart::parse(&text).map_err(|e| e.to_string())?;
    Ok((text, cart))
}

/// Parse a `--patterns a,b,c` list.
pub fn parse_pattern_list(s: &str, cart: &Cart) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for tok in s.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        let id: u8 = tok
            .parse()
            .map_err(|_| format!("bad pattern id {tok:?} in --patterns (want 0-63)"))?;
        if cart.pattern(id).is_none() {
            return Err(format!(
                "cart has no pattern {id} (defined: {})",
                pattern_id_list(cart)
            ));
        }
        out.push(id);
    }
    if out.is_empty() {
        return Err("--patterns needs at least one pattern id".to_string());
    }
    Ok(out)
}

/// Channels a pattern occupies, as `"ch0=00 ch1=03 ch2=- ..."`.
pub fn slots_text(cart: &Cart, id: u8) -> String {
    let Some(pat) = cart.pattern(id) else {
        return String::new();
    };
    (0..CHANNEL_COUNT)
        .map(|ch| match pat.slots[ch] {
            Some(sid) => format!("ch{ch}={sid:02}"),
            None => format!("ch{ch}=-"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How many of a pattern's six slots are occupied.
pub fn occupied_slots(cart: &Cart, id: u8) -> usize {
    cart.pattern(id)
        .map(|p| p.slots.iter().flatten().count())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// CLI dispatch for `console-agent music <cmd> ...`. Returns the process exit
/// code.
pub fn cli_music(args: &[String]) -> i32 {
    let cmd = args.first().map(String::as_str).unwrap_or_default();
    let result = match cmd {
        "score" => score::cli_score(&args[1..]),
        "lint" => lint::cli_lint(&args[1..]),
        "piano-roll" => roll::cli_piano_roll(&args[1..]),
        "render" => render::cli_render(&args[1..]),
        // The two write verbs own their exit codes and their own usage text,
        // exactly like `sprite edit` / `map edit`.
        "edit" => return transform::cli_edit(&args[1..]),
        "import-abc" => return abc::cli_import(&args[1..]),
        _ => {
            eprintln!("{MUSIC_USAGE}");
            return 2;
        }
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("{MUSIC_USAGE}");
            2
        }
    }
}

pub const MUSIC_USAGE: &str = "\
usage:
  console-agent music score      <cart> [--song N]
  console-agent music lint       <cart> [--strict]
  console-agent music piano-roll <cart> [--song N | --patterns a,b,c] [--cell N] [--row-h N] -o out.png
  console-agent music render     <cart> [--song N] [--loops K | --frames F] [--seed N] -o out.wav
  console-agent music edit       <cart> <transpose|copy|shift-rows|set-vol|set-inst|stretch> ...
  console-agent music import-abc <cart> <file.abc|-> --sfx <start-id> [--inst NAME] [--speed N]
  (--song N means music(N): the chain is followed from pattern N, so `score`
   and `piano-roll` show the whole song, intro and loop body. --song defaults
   to the lowest defined pattern id. `edit` and `import-abc` rewrite the cart
   file in place and take --dry-run; run them with no arguments for their own
   usage.)";

/// The flag set shared by the four `music` subcommands — small enough that
/// one parser beats four, and it keeps the flag spellings identical across
/// them.
#[derive(Debug, Default)]
pub struct Flags {
    pub song: Option<u8>,
    pub patterns: Option<String>,
    pub loops: Option<u32>,
    pub frames: Option<u64>,
    pub seed: Option<u64>,
    pub cell: Option<u32>,
    pub row_h: Option<u32>,
    pub strict: bool,
    pub out: Option<String>,
    pub positional: Vec<String>,
}

pub fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--song" => f.song = Some(next_num(&mut it, "--song")?),
            "--patterns" => {
                f.patterns = Some(it.next().ok_or("--patterns requires a value")?.clone());
            }
            "--loops" => f.loops = Some(next_num(&mut it, "--loops")?),
            "--frames" => f.frames = Some(next_num(&mut it, "--frames")?),
            "--seed" => f.seed = Some(next_num(&mut it, "--seed")?),
            "--cell" => f.cell = Some(next_num(&mut it, "--cell")?),
            "--row-h" => f.row_h = Some(next_num(&mut it, "--row-h")?),
            "--strict" => f.strict = true,
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

fn next_num<'a, T>(it: &mut impl Iterator<Item = &'a String>, what: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    let v = it
        .next()
        .ok_or_else(|| format!("{what} requires a value"))?;
    v.parse()
        .map_err(|_| format!("invalid {what} value {v:?} (want a non-negative integer)"))
}

/// The cart path every `music` subcommand takes as its first positional.
pub fn cart_arg(flags: &Flags, cmd: &str) -> Result<String, String> {
    match flags.positional.len() {
        0 => Err(format!("music {cmd} requires a cart path")),
        1 => Ok(flags.positional[0].clone()),
        _ => Err(format!(
            "music {cmd}: unexpected extra arguments: {:?}",
            &flags.positional[1..]
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_args_reads_whole_identifier_integer_calls_only() {
        let lua = "\
function _init() music(0) end
function go() music( 8 ) sfx(3) sfx(-1) end
local mymusic = 1 mymusic(9)
snd.music(7)
music(n)
music(1.5)
";
        // `mymusic(9)` and `snd.music(7)` are not the API's `music`; `music(n)`
        // and `music(1.5)` are not integer literals.
        assert_eq!(music_call_args(lua), vec![0, 8]);
        assert_eq!(sfx_call_args(lua), vec![3, -1]);
    }

    #[test]
    fn seconds_uses_the_sequencer_clock() {
        assert_eq!(seconds(60), 1.0);
        assert_eq!(seconds(30), 0.5);
    }
}

//! `music score` — the whole song as time-aligned text.
//!
//! This is layer 1 (ground truth as data) for music, and it replaces the
//! thing agents actually do today: step a console and read `audio_events`
//! back one note at a time. A score is the cart's own `__sfx__` rows,
//! re-laid-out the way a tracker lays them out — one line per row, one column
//! per channel — so a melody, a harmony and a drum part can be read *against*
//! each other.
//!
//! The header above each grid carries the thing `__music__` hides worst: the
//! **form**. `pat 0 -> pat 1 -> [pat 2 -> pat 3 ->] loop to 2` says at a
//! glance that patterns 0 and 1 are an intro and 2-3 repeat forever, which is
//! otherwise spread across four `loop=`/`stop` flags at the ends of four
//! lines.
//!
//! ## Time alignment when slot speeds differ
//!
//! A pattern's slots each run at their own `speed=`, so "row 4" is not one
//! moment in time unless every slot agrees. The grid's real axis is therefore
//! the **frame** — one line per frame at which *any* slot starts a row — and
//! the `row` column is filled in only when all slots share a speed (the
//! common case, where it is exactly `frame / speed`). A channel that does not
//! start a row on a given line shows `:` (still holding the row above), and
//! one whose sfx has run out of rows shows `.` (claimed by music, silent
//! until the pattern ends — what the sequencer actually does).

use std::collections::BTreeSet;

use console_core::{Cart, Fx, Sfx, SfxRow, WAVE_TABLE_BASE};

use crate::audio::note_name;

use super::{
    SongEnd, SongPlan, cart_arg, occupied_slots, parse_flags, plan_song, seconds, slots_text,
};

/// Placeholder for "this channel is still holding the row above".
const HELD: &str = ":";
/// Placeholder for "this channel's sfx has ended; music still owns it".
const ENDED: &str = ".";

/// Render the full score for the song starting at `song` (or the cart's
/// lowest pattern id when `None`).
pub fn score(cart: &Cart, song: Option<u8>) -> Result<String, String> {
    let start = match song {
        Some(id) => id,
        None => super::default_song(cart)?,
    };
    let plan = plan_song(cart, start)?;
    let mut out = String::new();
    write_form(&mut out, cart, &plan);
    for (i, step) in plan.steps.iter().enumerate() {
        out.push('\n');
        write_pattern(&mut out, cart, &plan, i, step.pattern);
    }
    Ok(out)
}

/// The song header: what `music(n)` was asked for, the chain, and how long
/// the intro and one loop pass last.
fn write_form(out: &mut String, cart: &Cart, plan: &SongPlan) {
    let intro = plan.intro_frames();
    out.push_str(&format!("song:  music({})\n", plan.start));
    out.push_str(&format!("form:  {}\n", plan.chain_text()));
    match plan.loop_index {
        Some(_) => {
            let body = plan.loop_frames();
            out.push_str(&format!(
                "       intro {intro} frames ({:.2}s), loop {body} frames ({:.2}s) per pass\n",
                seconds(intro),
                seconds(body),
            ));
        }
        None => {
            out.push_str(&format!(
                "       plays once: {intro} frames ({:.2}s)\n",
                seconds(intro)
            ));
        }
    }
    if let Some(t) = cart.audio().tempo() {
        out.push_str(&format!(
            "tempo: bpm={} rows_per_beat={} (speed=auto -> {})\n",
            t.bpm, t.rows_per_beat, t.speed
        ));
    }
    if let SongEnd::FellOffEnd = plan.end {
        out.push_str("note:  the chain has no `loop=` or `stop`; music halts when it runs out\n");
    }
}

/// One pattern: header (role in the form, slots, length, end rule) followed
/// by its grid.
fn write_pattern(out: &mut String, cart: &Cart, plan: &SongPlan, index: usize, id: u8) {
    let role = match plan.loop_index {
        Some(i) if index == i => " [loop start]",
        Some(i) if index > i => " [loop body]",
        Some(_) => " [intro]",
        None => "",
    };
    let frames = plan.steps[index].frames;
    out.push_str(&format!("== pat {id}{role}\n"));
    out.push_str(&format!("   slots:  {}\n", slots_text(cart, id)));

    let cols = columns(cart, id);
    let speeds: BTreeSet<u8> = cols.iter().map(|c| c.sfx.speed).collect();
    let rows = cols.iter().map(|c| c.sfx.rows.len()).max().unwrap_or(0);
    let speed_text = speeds
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join("/");
    out.push_str(&format!(
        "   length: {frames} frames ({:.2}s) | rows: {rows} | speed: {}\n",
        seconds(u64::from(frames)),
        if speed_text.is_empty() {
            "-".into()
        } else {
            speed_text
        },
    ));
    out.push_str(&format!("   end:    {}\n", end_text(cart, plan, index, id)));

    if cols.is_empty() {
        out.push_str("   (every slot is `-`: this pattern is silence)\n");
        return;
    }
    write_grid(out, cart, &cols, frames);
}

/// How this pattern hands off, spelled out rather than left as a flag.
fn end_text(cart: &Cart, plan: &SongPlan, index: usize, id: u8) -> String {
    use console_core::PatternEnd;
    match cart.pattern(id).map(|p| p.end) {
        Some(PatternEnd::Stop) => "stop (music halts)".to_string(),
        Some(PatternEnd::Loop(t)) => format!("loop={t} (jump back to pat {t})"),
        Some(PatternEnd::Next) => match cart.audio().next_pattern_after(id) {
            Some(n) => format!("next -> pat {n}"),
            None => "next -> nothing defined, music halts".to_string(),
        },
        None => match plan.end {
            SongEnd::Undefined(u) if index + 1 == plan.steps.len() => {
                format!("-> pat {u} (UNDEFINED)")
            }
            _ => "-".to_string(),
        },
    }
}

/// One printed column: an occupied slot and the sfx it plays.
struct Column<'c> {
    channel: usize,
    sfx_id: u8,
    sfx: &'c Sfx,
}

fn columns<'c>(cart: &'c Cart, id: u8) -> Vec<Column<'c>> {
    let Some(pat) = cart.pattern(id) else {
        return Vec::new();
    };
    (0..console_core::CHANNEL_COUNT)
        .filter_map(|ch| {
            let sid = pat.slots[ch]?;
            let sfx = cart.sfx(sid)?;
            Some(Column {
                channel: ch,
                sfx_id: sid,
                sfx,
            })
        })
        .collect()
}

/// The grid itself. Columns are sized to their widest cell so every channel
/// lines up vertically, which is the only reason this is readable at all.
fn write_grid(out: &mut String, cart: &Cart, cols: &[Column<'_>], frames: u32) {
    // Time points: every frame at which any slot starts a row, inside the
    // pattern's own length (a slot longer than the pattern is truncated by
    // the sequencer, so the score truncates it too).
    let mut times: BTreeSet<u32> = BTreeSet::new();
    for col in cols {
        let speed = u32::from(col.sfx.speed);
        for row in 0..col.sfx.rows.len() as u32 {
            let at = row * speed;
            if at < frames {
                times.insert(at);
            }
        }
    }
    if times.is_empty() {
        return;
    }

    let uniform_speed = {
        let speeds: BTreeSet<u8> = cols.iter().map(|c| c.sfx.speed).collect();
        if speeds.len() == 1 {
            speeds.into_iter().next()
        } else {
            None
        }
    };

    let headers: Vec<String> = cols
        .iter()
        .map(|c| format!("ch{} sfx{:02} sp{}", c.channel, c.sfx_id, c.sfx.speed))
        .collect();

    // Cell text for every (time, column), then column widths.
    let times: Vec<u32> = times.into_iter().collect();
    let mut body: Vec<Vec<String>> = Vec::with_capacity(times.len());
    for &at in &times {
        body.push(cols.iter().map(|c| cell_text(cart, c, at)).collect());
    }
    let widths: Vec<usize> = (0..cols.len())
        .map(|i| {
            body.iter()
                .map(|r| r[i].len())
                .chain(std::iter::once(headers[i].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let row_w = 4;
    let frame_w = 6;
    let mut header = format!("  {:>row_w$} {:>frame_w$} |", "row", "frame");
    for (h, w) in headers.iter().zip(&widths) {
        header.push_str(&format!(" {h:<w$} |", w = *w));
    }
    out.push_str(&header);
    out.push('\n');
    out.push_str(&format!(
        "  {}\n",
        "-".repeat(header.len().saturating_sub(2))
    ));

    for (at, cells) in times.iter().zip(&body) {
        let row = match uniform_speed {
            Some(sp) if at % u32::from(sp) == 0 => (at / u32::from(sp)).to_string(),
            _ => "-".to_string(),
        };
        let mut line = format!("  {row:>row_w$} {at:>frame_w$} |");
        for (text, w) in cells.iter().zip(&widths) {
            line.push_str(&format!(" {text:<w$} |", w = *w));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
}

/// One cell: the row this column starts at frame `at`, or a placeholder.
fn cell_text(cart: &Cart, col: &Column<'_>, at: u32) -> String {
    let speed = u32::from(col.sfx.speed);
    if at % speed != 0 {
        return HELD.to_string();
    }
    let row = (at / speed) as usize;
    match col.sfx.rows.get(row) {
        None => ENDED.to_string(),
        Some(SfxRow::Rest) => "---".to_string(),
        Some(SfxRow::Note { note, wave, vol }) => {
            let m = col.sfx.row_mod(row);
            let voice = match m.inst.and_then(|i| cart.audio().instrument_at(i)) {
                Some(inst) => inst.name.clone(),
                None if *wave >= WAVE_TABLE_BASE => format!("w{}", wave - WAVE_TABLE_BASE),
                None => wave.to_string(),
            };
            let fx = m.fx.map(fx_text).unwrap_or_default();
            let cell = format!("{:<3} {voice} {vol} {fx}", note_name(*note));
            cell.trim_end().to_string()
        }
    }
}

/// An effect column back in the spelling a cart row uses.
pub fn fx_text(fx: Fx) -> String {
    match fx {
        Fx::Arp { a, b } => format!("arp{a},{b}"),
        Fx::Slide { semis } => format!("sl{semis:+}"),
        Fx::Vibrato(v) => format!("vib{},{}", v.cents, v.rate),
        Fx::Fade { levels } => format!("fade{levels:+}"),
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub fn cli_score(args: &[String]) -> Result<i32, String> {
    let flags = parse_flags(args)?;
    let path = cart_arg(&flags, "score")?;
    let (_text, cart) = super::read_cart(&path)?;
    if occupied_slots_total(&cart) == 0 && cart.audio().pattern_ids().next().is_none() {
        return Err("cart has no __music__ patterns".to_string());
    }
    print!("{}", score(&cart, flags.song)?);
    Ok(0)
}

fn occupied_slots_total(cart: &Cart) -> usize {
    cart.audio()
        .pattern_ids()
        .map(|id| occupied_slots(cart, id))
        .sum()
}

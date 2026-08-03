//! `console-agent music edit <cart> <verb> …` — score-level transforms that
//! rewrite `__sfx__` in place (SPEC.md "Music authoring (PoC v2)" >
//! "Transforms"), the third member of the `sprite edit` / `map edit` family
//! and the same contract: CLI only, atomic, `--dry-run` everywhere, and only
//! the changed lines of the cart file are touched.
//!
//! Six verbs, deliberately no more — they are the operations an agent
//! actually performs while iterating on a groove, and each one is a thing that
//! is tedious and error-prone to do by hand-editing tracker text:
//!
//! | verb | what it saves you |
//! |------|-------------------|
//! | `transpose` | re-spelling every note name in an sfx (and noticing when one falls off the note table) |
//! | `copy` | duplicating an entry to variate it, without a duplicate-id parse error |
//! | `shift-rows` | rotating a phrase so it starts on a different beat |
//! | `set-vol` | velocity scaling a part against the rest of the mix |
//! | `set-inst` | re-voicing a part after auditioning instruments |
//! | `stretch` | half/double time, with the `speed=` compensation done for you |
//!
//! Everything here is row-*text* surgery, not a re-serialization of the parsed
//! bank: `transpose`/`set-vol`/`set-inst` swap one whitespace-separated token
//! and leave the rest of the line byte-identical, `shift-rows` and `copy` move
//! original line text around. An agent's column alignment, effect columns and
//! comments therefore survive, and the diff is exactly the notes that moved.
//! See [`sfxtext`](super::sfxtext) for the rewrite machinery.

use console_core::{
    Cart, MAX_ID, MAX_SFX_ROWS, MAX_VOL, Sfx, SfxRow, WAVE_FM, WAVE_PERIODIC, WAVE_TABLE_BASE,
};

use crate::audio::note_name;

use super::sfxtext::{
    self, EditResult, Rewrite, SfxBlock, SfxLayout, apply_edit_result, replace_id, replace_speed,
    replace_token, set_block,
};

pub const EDIT_USAGE: &str = "\
usage:
  console-agent music edit <cart> transpose  <sfx-ids> <semitones> [--clamp] [--dry-run]
  console-agent music edit <cart> copy       <src-sfx> <dst-sfx> [--force] [--dry-run]
  console-agent music edit <cart> shift-rows <sfx-id> <n> [--dry-run]
  console-agent music edit <cart> set-vol    <sfx-id> <0-7|+n|-n> [--dry-run]
  console-agent music edit <cart> set-inst   <sfx-id> <inst|0-5|w0-w7> [--where <old>] [--dry-run]
  console-agent music edit <cart> stretch    <sfx-id> <2|0.5> [--force] [--dry-run]
  (<sfx-ids> is an id `3`, a range `0-5`, or a comma list `0,2,5-7`;
   <semitones> and <n> are signed, e.g. `-12` or `+2`)";

/// CLI entry for `music edit`. `args[0]` is the cart path, `args[1]` the
/// verb; flags may appear anywhere.
pub fn cli_edit(args: &[String]) -> i32 {
    let parsed = match EditArgs::parse(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}\n{EDIT_USAGE}");
            return 2;
        }
    };
    if parsed.positional.len() < 2 {
        eprintln!("{EDIT_USAGE}");
        return 2;
    }
    let cart_path = parsed.positional[0].clone();
    let text = match std::fs::read_to_string(&cart_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read {cart_path:?}: {e}");
            return 2;
        }
    };
    apply_edit_result(&cart_path, run_edit(&text, &parsed), parsed.dry_run)
}

/// `music edit`'s own argument parser. It cannot reuse
/// [`parse_flags`](super::parse_flags) because half these verbs take a
/// **signed** operand — `-12` is a transpose, not an unknown flag.
#[derive(Debug, Default)]
pub struct EditArgs {
    pub dry_run: bool,
    pub clamp: bool,
    pub force: bool,
    pub where_voice: Option<String>,
    pub positional: Vec<String>,
}

impl EditArgs {
    pub fn parse(args: &[String]) -> Result<EditArgs, String> {
        let mut out = EditArgs::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--dry-run" => out.dry_run = true,
                "--clamp" => out.clamp = true,
                "--force" => out.force = true,
                "--where" => {
                    out.where_voice = Some(it.next().ok_or("--where requires a value")?.clone());
                }
                // A long flag is a flag; anything else — including `-12` and
                // `+2` — is an operand.
                other if other.starts_with("--") => {
                    return Err(format!("unknown flag {other:?}"));
                }
                other => out.positional.push(other.to_string()),
            }
        }
        Ok(out)
    }
}

/// Pure core: compute the rewrite for `args` against `text`. No file I/O, so
/// tests drive this directly.
pub fn run_edit(text: &str, args: &EditArgs) -> Result<EditResult, String> {
    let cart = Cart::parse(text).map_err(|e| format!("cart: {e}"))?;
    let lines: Vec<&str> = text.split('\n').collect();
    let layout = sfxtext::locate(&lines);
    let verb = args.positional[1].as_str();
    let rest = &args.positional[2..];

    let ctx = Ctx {
        cart: &cart,
        lines: &lines,
        layout: &layout,
    };
    let (rw, summary) = match verb {
        "transpose" => op_transpose(&ctx, rest, args)?,
        "copy" => op_copy(&ctx, rest, args)?,
        "shift-rows" => op_shift_rows(&ctx, rest)?,
        "set-vol" => op_set_vol(&ctx, rest)?,
        "set-inst" => op_set_inst(&ctx, rest, args)?,
        "stretch" => op_stretch(&ctx, rest, args)?,
        other => {
            return Err(format!(
                "unknown music edit verb {other:?}; expected transpose|copy|shift-rows|set-vol|set-inst|stretch"
            ));
        }
    };
    sfxtext::finish(text, &rw, summary, &format!("music edit {verb}"))
}

/// Everything a verb needs: the parsed cart (semantics) and the located raw
/// text (where to write).
struct Ctx<'a> {
    cart: &'a Cart,
    lines: &'a [&'a str],
    layout: &'a SfxLayout,
}

impl<'a> Ctx<'a> {
    /// The parsed sfx and its located text block, with the error messages the
    /// CLI wants.
    fn entry(&self, id: u8) -> Result<(&'a Sfx, &'a SfxBlock), String> {
        let sfx = self
            .cart
            .sfx(id)
            .ok_or_else(|| format!("cart has no sfx {id} (defined: {})", sfx_id_list(self.cart)))?;
        let block = self.layout.block(id).ok_or_else(|| {
            format!("sfx {id} parses but its `__sfx__` lines could not be located")
        })?;
        if block.row_lines.len() != sfx.rows.len() {
            return Err(format!(
                "sfx {id}: located {} row line(s) but the parser read {} row(s); \
                 the `__sfx__` text is not in a shape this tool can rewrite safely",
                block.row_lines.len(),
                sfx.rows.len()
            ));
        }
        Ok((sfx, block))
    }

    fn line(&self, i: usize) -> &'a str {
        self.lines[i]
    }
}

/// Defined sfx ids as `"0, 1, 3"`, for error messages.
pub fn sfx_id_list(cart: &Cart) -> String {
    let ids: Vec<String> = cart.audio().sfx_ids().map(|i| i.to_string()).collect();
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.join(", ")
    }
}

// ---------------------------------------------------------------------------
// Operand parsing
// ---------------------------------------------------------------------------

/// `3`, a range `0-5`, or a comma list mixing both (`0,2,5-7`). Every id must
/// name an sfx the cart defines — a transform that silently skipped a missing
/// id would be worse than one that stops.
pub fn parse_id_list(spec: &str, cart: &Cart) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (lo, hi) = match part.split_once('-') {
            Some((a, b)) => (parse_id(a.trim())?, parse_id(b.trim())?),
            None => {
                let id = parse_id(part)?;
                (id, id)
            }
        };
        if lo > hi {
            return Err(format!("bad sfx range {part:?}: {lo} is above {hi}"));
        }
        for id in lo..=hi {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    if out.is_empty() {
        return Err("expected at least one sfx id".to_string());
    }
    for &id in &out {
        if cart.sfx(id).is_none() {
            return Err(format!(
                "cart has no sfx {id} (defined: {})",
                sfx_id_list(cart)
            ));
        }
    }
    Ok(out)
}

fn parse_id(s: &str) -> Result<u8, String> {
    let id: u8 = s.parse().map_err(|_| {
        if s.contains('-') || s.contains(',') {
            // `transpose` is the only verb that takes a set; saying so beats
            // "want 0-63" when the agent has just used a range on `set-vol`.
            format!(
                "bad sfx id {s:?}: only `transpose` takes a range or list;                  every other verb takes a single id 0-{MAX_ID}"
            )
        } else {
            format!("bad sfx id {s:?} (want 0-{MAX_ID})")
        }
    })?;
    if id > MAX_ID {
        return Err(format!("bad sfx id {id} (want 0-{MAX_ID})"));
    }
    Ok(id)
}

/// A signed operand, accepting a leading `+`.
fn parse_signed(s: &str, what: &str) -> Result<i32, String> {
    s.strip_prefix('+')
        .unwrap_or(s)
        .parse::<i32>()
        .map_err(|_| format!("bad {what} {s:?} (want a signed integer, e.g. -12 or +2)"))
}

fn operands<'a>(
    rest: &'a [String],
    verb: &str,
    want: &str,
    n: usize,
) -> Result<&'a [String], String> {
    if rest.len() != n {
        return Err(format!(
            "music edit {verb} takes {n} operand(s): {want} (got {rest:?})"
        ));
    }
    Ok(rest)
}

// ---------------------------------------------------------------------------
// Row inspection helpers
// ---------------------------------------------------------------------------

/// The voice column of a row as the cart spells it: the instrument's name, a
/// `w<slot>` wavetable, or a bare wave digit. Identical to what `music score`
/// prints, so `--where` matches what an agent just read.
fn voice_text(cart: &Cart, sfx: &Sfx, row: usize) -> Option<String> {
    let SfxRow::Note { wave, .. } = sfx.rows[row] else {
        return None;
    };
    let m = sfx.row_mod(row);
    Some(match m.inst.and_then(|i| cart.audio().instrument_at(i)) {
        Some(inst) => inst.name.clone(),
        None if wave >= WAVE_TABLE_BASE => format!("w{}", wave - WAVE_TABLE_BASE),
        None => wave.to_string(),
    })
}

/// Validate a voice token for `set-inst`: a bare wave digit `0-5`, a defined
/// wavetable `w0`-`w7`, or a defined instrument name. Rejecting here (rather
/// than letting `Cart::parse` catch it after the rewrite) buys a message that
/// names the alternatives.
fn validate_voice(cart: &Cart, token: &str) -> Result<(), String> {
    if let Ok(digit) = token.parse::<u8>() {
        if digit == WAVE_FM {
            return Err(format!(
                "wave {WAVE_FM} is the 2-op FM oscillator, which a bare digit cannot describe: \
                 name an `inst … wave={WAVE_FM} fm=…` instrument instead"
            ));
        }
        if digit > 5 && digit != WAVE_PERIODIC {
            return Err(format!("bad wave digit {digit} (want 0-5 or {WAVE_PERIODIC})"));
        }
        return Ok(());
    }
    if let Some(slot) = token.strip_prefix('w')
        && let Ok(slot) = slot.parse::<u8>()
    {
        return match cart.audio().wavetable(slot) {
            Some(_) => Ok(()),
            None => Err(format!(
                "this cart defines no wavetable in slot {slot} (`wavetable {slot} <32 nibbles>` in __instruments__)"
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
        "unknown voice {token:?} (want a wave digit 0-5 or 7, a wavetable w0-w7, or one of: {})",
        if names.is_empty() {
            "this cart has no __instruments__".to_string()
        } else {
            names.join(", ")
        }
    ))
}

// ---------------------------------------------------------------------------
// transpose
// ---------------------------------------------------------------------------

fn op_transpose(
    ctx: &Ctx<'_>,
    rest: &[String],
    args: &EditArgs,
) -> Result<(Rewrite, Vec<String>), String> {
    let rest = operands(rest, "transpose", "<sfx-ids> <semitones>", 2)?;
    let ids = parse_id_list(&rest[0], ctx.cart)?;
    let semis = parse_signed(&rest[1], "semitone count")?;

    // Range of shifts under which every selected note stays inside C0-B7 —
    // computed up front so the error can *suggest* a shift instead of just
    // refusing one.
    let mut lo_note = u8::MAX;
    let mut hi_note = 0u8;
    for &id in &ids {
        let (sfx, _) = ctx.entry(id)?;
        for row in &sfx.rows {
            if let SfxRow::Note { note, .. } = row {
                lo_note = lo_note.min(*note);
                hi_note = hi_note.max(*note);
            }
        }
    }
    let fit = if lo_note == u8::MAX {
        None
    } else {
        Some((-(i32::from(lo_note)), 95 - i32::from(hi_note)))
    };

    let mut rw = Rewrite::default();
    let mut moved = 0usize;
    let mut clamped = 0usize;
    for &id in &ids {
        let (sfx, block) = ctx.entry(id)?;
        for (r, row) in sfx.rows.iter().enumerate() {
            let SfxRow::Note { note, .. } = row else {
                continue;
            };
            let target = i32::from(*note) + semis;
            let new = if (0..=95).contains(&target) {
                target as u8
            } else if args.clamp {
                clamped += 1;
                target.clamp(0, 95) as u8
            } else {
                return Err(transpose_range_error(id, r, *note, semis, fit));
            };
            if new == *note {
                continue;
            }
            let line = ctx.line(block.row_lines[r]);
            let new_line = replace_token(line, 0, &note_name(new)).ok_or_else(|| {
                format!("sfx {id} row {r}: cannot find the note column in {line:?}")
            })?;
            rw.set_line(block.row_lines[r], new_line);
            moved += 1;
        }
    }

    let mut summary = vec![format!(
        "transpose: sfx {} by {semis:+} semitone(s): {moved} note row(s) changed",
        id_text(&ids)
    )];
    if clamped > 0 {
        summary.push(format!(
            "  {clamped} note(s) clamped to the C0-B7 note table (--clamp)"
        ));
    }
    Ok((rw, summary))
}

/// The transpose range error, with the nearest shift that *would* fit — the
/// agent-friendly half of the message.
fn transpose_range_error(
    id: u8,
    row: usize,
    note: u8,
    semis: i32,
    fit: Option<(i32, i32)>,
) -> String {
    let mut msg = format!(
        "sfx {id} row {row}: {} {semis:+} leaves the note table (C0-B7)",
        note_name(note)
    );
    match fit {
        Some((lo, hi)) if lo <= hi => {
            let nearest = semis.clamp(lo, hi);
            msg.push_str(&format!(
                "; the selection fits any shift in {lo:+}..={hi:+} — nearest to {semis:+} is \
                 {nearest:+}. Pass --clamp to clamp out-of-range notes instead"
            ));
        }
        _ => msg.push_str(
            "; the selection already spans more than the 96-semitone note table, \
             so no shift fits it whole. Pass --clamp to clamp out-of-range notes instead",
        ),
    }
    msg
}

fn id_text(ids: &[u8]) -> String {
    ids.iter().map(u8::to_string).collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------------

fn op_copy(
    ctx: &Ctx<'_>,
    rest: &[String],
    args: &EditArgs,
) -> Result<(Rewrite, Vec<String>), String> {
    let rest = operands(rest, "copy", "<src-sfx> <dst-sfx>", 2)?;
    let src = parse_id(&rest[0])?;
    let dst = parse_id(&rest[1])?;
    if src == dst {
        return Err(format!("copy: source and destination are both sfx {src}"));
    }
    let (src_sfx, src_block) = ctx.entry(src)?;

    let header = replace_id(ctx.line(src_block.header_line), dst)
        .ok_or_else(|| format!("sfx {src}: cannot find the id column in its header line"))?;
    let rows: Vec<String> = src_block
        .row_lines
        .iter()
        .map(|&i| ctx.line(i).to_string())
        .collect();

    let mut rw = Rewrite::default();
    let mut summary = Vec::new();
    match ctx.layout.block(dst) {
        Some(existing) if !args.force => {
            return Err(format!(
                "copy: sfx {dst} already exists ({} row(s)); pass --force to overwrite it",
                existing.row_lines.len()
            ));
        }
        Some(existing) => {
            set_block(&mut rw, ctx.lines, existing, Some(header), rows);
            summary.push(format!(
                "copy: sfx {src} -> sfx {dst} ({} row(s)), overwriting the existing sfx {dst} (--force)",
                src_sfx.rows.len()
            ));
        }
        None => {
            let mut block = vec![header];
            block.extend(rows);
            match ctx.layout.insert_point_for(dst) {
                Some(at) => rw.insert_entry(ctx.lines, at, block),
                None => {
                    let anchor = sfxtext::new_section_anchor(ctx.lines);
                    rw.insert_new_section(ctx.lines, anchor, block);
                }
            }
            summary.push(format!(
                "copy: sfx {src} -> new sfx {dst} ({} row(s), speed={})",
                src_sfx.rows.len(),
                src_sfx.speed
            ));
        }
    }
    Ok((rw, summary))
}

// ---------------------------------------------------------------------------
// shift-rows
// ---------------------------------------------------------------------------

fn op_shift_rows(ctx: &Ctx<'_>, rest: &[String]) -> Result<(Rewrite, Vec<String>), String> {
    let rest = operands(rest, "shift-rows", "<sfx-id> <n>", 2)?;
    let id = parse_id(&rest[0])?;
    let n = parse_signed(&rest[1], "row count")?;
    let (sfx, block) = ctx.entry(id)?;

    let len = sfx.rows.len() as i32;
    let shift = n.rem_euclid(len);
    if shift == 0 {
        return Ok((
            Rewrite::default(),
            vec![format!(
                "shift-rows: sfx {id} has {len} row(s); {n:+} is a whole number of rotations (no change)"
            )],
        ));
    }

    // Rotate the *original line text*, so alignment and effect columns follow
    // their rows exactly. Row i takes the row `shift` places above it.
    let originals: Vec<String> = block
        .row_lines
        .iter()
        .map(|&i| ctx.line(i).to_string())
        .collect();
    let mut rw = Rewrite::default();
    for r in 0..len {
        let from = (r - shift).rem_euclid(len) as usize;
        rw.set_line(block.row_lines[r as usize], originals[from].clone());
    }

    let mut summary = vec![format!(
        "shift-rows: sfx {id} rotated {n:+} row(s) (row 0 now plays what row {} played)",
        (-shift).rem_euclid(len)
    )];
    if sfx.loop_range.is_some() {
        summary.push(
            "  note: this sfx has a `loop=` range; the rows moved but the range did not"
                .to_string(),
        );
    }
    Ok((rw, summary))
}

// ---------------------------------------------------------------------------
// set-vol
// ---------------------------------------------------------------------------

fn op_set_vol(ctx: &Ctx<'_>, rest: &[String]) -> Result<(Rewrite, Vec<String>), String> {
    let rest = operands(rest, "set-vol", "<sfx-id> <0-7|+n|-n>", 2)?;
    let id = parse_id(&rest[0])?;
    let spec = rest[1].as_str();
    // A leading sign is what makes it relative: `3` sets, `+3`/`-3` scale.
    let relative = spec.starts_with('+') || spec.starts_with('-');
    let value = parse_signed(spec, "volume")?;
    if !relative && !(0..=i32::from(MAX_VOL)).contains(&value) {
        return Err(format!(
            "bad volume {value} (want 0-{MAX_VOL} absolute, or a signed +n/-n change)"
        ));
    }

    let (sfx, block) = ctx.entry(id)?;
    let mut rw = Rewrite::default();
    let (mut changed, mut rests) = (0usize, 0usize);
    for (r, row) in sfx.rows.iter().enumerate() {
        // Rests carry no velocity: scaling one would mean turning silence into
        // a note, so they are preserved verbatim.
        let SfxRow::Note { vol, .. } = row else {
            rests += 1;
            continue;
        };
        let new = if relative {
            (i32::from(*vol) + value).clamp(0, i32::from(MAX_VOL)) as u8
        } else {
            value as u8
        };
        if new == *vol {
            continue;
        }
        let line = ctx.line(block.row_lines[r]);
        let new_line = replace_token(line, 2, &new.to_string())
            .ok_or_else(|| format!("sfx {id} row {r}: cannot find the vol column in {line:?}"))?;
        rw.set_line(block.row_lines[r], new_line);
        changed += 1;
    }

    let what = if relative {
        format!("{value:+}")
    } else {
        format!("= {value}")
    };
    Ok((
        rw,
        vec![format!(
            "set-vol: sfx {id} vol {what}: {changed} note row(s) changed, {rests} rest(s) preserved"
        )],
    ))
}

// ---------------------------------------------------------------------------
// set-inst
// ---------------------------------------------------------------------------

fn op_set_inst(
    ctx: &Ctx<'_>,
    rest: &[String],
    args: &EditArgs,
) -> Result<(Rewrite, Vec<String>), String> {
    let rest = operands(rest, "set-inst", "<sfx-id> <inst|0-5|w0-w7>", 2)?;
    let id = parse_id(&rest[0])?;
    let voice = rest[1].as_str();
    validate_voice(ctx.cart, voice)?;

    let (sfx, block) = ctx.entry(id)?;
    if let Some(old) = &args.where_voice {
        let present: Vec<String> = (0..sfx.rows.len())
            .filter_map(|r| voice_text(ctx.cart, sfx, r))
            .collect();
        if !present.iter().any(|v| v == old) {
            return Err(format!(
                "--where {old:?}: no row of sfx {id} uses that voice (rows use: {})",
                dedup_join(&present)
            ));
        }
    }

    let mut rw = Rewrite::default();
    let mut changed = 0usize;
    for r in 0..sfx.rows.len() {
        let Some(current) = voice_text(ctx.cart, sfx, r) else {
            continue;
        };
        if let Some(old) = &args.where_voice
            && &current != old
        {
            continue;
        }
        if current == voice {
            continue;
        }
        let line = ctx.line(block.row_lines[r]);
        let new_line = replace_token(line, 1, voice)
            .ok_or_else(|| format!("sfx {id} row {r}: cannot find the voice column in {line:?}"))?;
        rw.set_line(block.row_lines[r], new_line);
        changed += 1;
    }

    let scope = match &args.where_voice {
        Some(old) => format!(" (only rows using {old:?})"),
        None => String::new(),
    };
    Ok((
        rw,
        vec![format!(
            "set-inst: sfx {id} voice -> {voice}{scope}: {changed} note row(s) changed"
        )],
    ))
}

fn dedup_join(values: &[String]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for v in values {
        if !seen.contains(&v.as_str()) {
            seen.push(v);
        }
    }
    if seen.is_empty() {
        "none".to_string()
    } else {
        seen.join(", ")
    }
}

// ---------------------------------------------------------------------------
// stretch
// ---------------------------------------------------------------------------

/// `stretch <id> 2` doubles the rhythmic grid (a rest between every row) and
/// `stretch <id> 0.5` halves it (odd rows dropped). Both compensate `speed=`
/// so the sfx keeps its wall-clock length, which is the whole point: an agent
/// wants the same groove at a finer or coarser resolution, not a tempo change.
///
/// **Rounding.** `speed` is an integer count of frames per row, so the
/// compensation is exact only when it divides. Doubling uses
/// `new = max(1, (speed + 1) / 2)` (round half **up**, so `speed=5` becomes 3,
/// not 2) and halving uses `new = speed * 2`. When the compensated value would
/// leave the legal 1..=255 range — `speed=1` cannot be halved, `speed>127`
/// cannot be doubled — the speed is left alone and the summary reports the
/// wall-clock change instead of hiding it. Every run prints the before/after
/// frame count, so the rounding is never silent.
fn op_stretch(
    ctx: &Ctx<'_>,
    rest: &[String],
    args: &EditArgs,
) -> Result<(Rewrite, Vec<String>), String> {
    let rest = operands(rest, "stretch", "<sfx-id> <2|0.5>", 2)?;
    let id = parse_id(&rest[0])?;
    let double = match rest[1].as_str() {
        "2" | "2.0" => true,
        "0.5" | ".5" | "1/2" => false,
        other => {
            return Err(format!(
                "bad stretch factor {other:?} (want `2` to double the grid or `0.5` to halve it)"
            ));
        }
    };
    let (sfx, block) = ctx.entry(id)?;
    let old_rows = sfx.rows.len();
    let old_speed = sfx.speed;
    let old_frames = sfx.duration();

    let originals: Vec<String> = block
        .row_lines
        .iter()
        .map(|&i| ctx.line(i).to_string())
        .collect();
    // Match the rest spelling the cart already uses, so a doubled sfx looks
    // hand-written rather than machine-generated.
    let rest_text = originals
        .iter()
        .find(|l| matches!(l.trim(), "---"))
        .map(|l| l.trim_end_matches('\r').to_string())
        .unwrap_or_else(|| "---".to_string());

    let (new_rows, new_speed, mut notes): (Vec<String>, u8, Vec<String>) = if double {
        if old_rows * 2 > MAX_SFX_ROWS {
            return Err(format!(
                "stretch 2: sfx {id} has {old_rows} rows; doubling would need {} and the platform \
                 maximum is {MAX_SFX_ROWS} rows per sfx",
                old_rows * 2
            ));
        }
        let mut rows = Vec::with_capacity(old_rows * 2);
        for line in &originals {
            rows.push(line.clone());
            rows.push(rest_text.clone());
        }
        let mut notes = Vec::new();
        let speed = if old_speed >= 2 {
            old_speed.div_ceil(2)
        } else {
            notes.push(
                "  speed=1 cannot be halved, so the sfx is now twice as long in wall-clock time"
                    .to_string(),
            );
            1
        };
        (rows, speed, notes)
    } else {
        let dropped: Vec<usize> = (1..old_rows)
            .step_by(2)
            .filter(|&r| matches!(sfx.rows[r], SfxRow::Note { .. }))
            .collect();
        if !dropped.is_empty() && !args.force {
            return Err(format!(
                "stretch 0.5: sfx {id} would drop {} odd row(s) that carry notes (row(s) {}); \
                 pass --force to drop them anyway",
                dropped.len(),
                dropped
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let rows: Vec<String> = originals.iter().step_by(2).cloned().collect();
        let mut notes = Vec::new();
        if !dropped.is_empty() {
            notes.push(format!(
                "  dropped {} note row(s) on odd rows (--force)",
                dropped.len()
            ));
        }
        let speed = match old_speed.checked_mul(2) {
            Some(s) => s,
            None => {
                notes.push(format!(
                    "  speed={old_speed} cannot be doubled (max 255), so the sfx is now half as \
                     long in wall-clock time"
                ));
                old_speed
            }
        };
        (rows, speed, notes)
    };

    let mut rw = Rewrite::default();
    let header = if new_speed == old_speed && header_speed_is_numeric(ctx.line(block.header_line)) {
        None
    } else {
        Some(
            adjust_header(
                ctx.line(block.header_line),
                new_speed,
                sfx,
                double,
                old_rows,
            )
            .ok_or_else(|| format!("sfx {id}: cannot find `speed=` in its header line"))?,
        )
    };
    let new_len = new_rows.len();
    set_block(&mut rw, ctx.lines, block, header, new_rows);

    let new_frames = new_len as u32 * u32::from(new_speed);
    let mut summary = vec![format!(
        "stretch: sfx {id} {} {old_rows} -> {new_len} rows, speed {old_speed} -> {new_speed}",
        if double { "doubled:" } else { "halved:" },
    )];
    summary.push(format!(
        "  length {old_frames} -> {new_frames} frames ({})",
        if new_frames == old_frames {
            "unchanged".to_string()
        } else {
            format!(
                "{:+} frames from integer speed rounding",
                new_frames as i64 - old_frames as i64
            )
        }
    ));
    summary.append(&mut notes);
    Ok((rw, summary))
}

/// True when the header's `speed=` is already a plain number (so an unchanged
/// speed needs no header rewrite). `speed=auto` always gets resolved to a
/// number, because the row count changed under it.
fn header_speed_is_numeric(line: &str) -> bool {
    sfxtext::token_spans(line)
        .iter()
        .filter_map(|&(s, e)| line[s..e].strip_prefix("speed="))
        .next_back()
        .is_some_and(|v| v.parse::<u8>().is_ok())
}

/// Rewrite the header's `speed=` and, when the sfx has one, its `loop=` row
/// range — the range indexes rows, and stretching moved them.
fn adjust_header(
    line: &str,
    speed: u8,
    sfx: &Sfx,
    double: bool,
    old_rows: usize,
) -> Option<String> {
    let out = replace_speed(line, speed)?;
    let Some((start, end)) = sfx.loop_range else {
        return Some(out);
    };
    let last = if double {
        old_rows * 2 - 1
    } else {
        old_rows.div_ceil(2).saturating_sub(1)
    };
    let (ns, ne) = if double {
        // Cover the same music: the inserted rest belongs to the loop too.
        (usize::from(start) * 2, usize::from(end) * 2 + 1)
    } else {
        (usize::from(start) / 2, usize::from(end) / 2)
    };
    let (ns, ne) = (ns.min(last), ne.min(last).max(ns.min(last)));
    let spans = sfxtext::token_spans(&out);
    let n = spans
        .iter()
        .position(|&(s, e)| out[s..e].starts_with("loop="))?;
    replace_token(&out, n, &format!("loop={ns},{ne}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CART: &str = "\
__lua__
function _init() end

__sfx__
sfx 0 speed=8
C4 2 5
E4 2 5
---
G4 2 5

__music__
pat 0 : 0 - - -
";

    fn edit(text: &str, argv: &[&str]) -> Result<EditResult, String> {
        let args = EditArgs::parse(
            &argv
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<String>>(),
        )
        .unwrap();
        run_edit(text, &args)
    }

    fn changed(text: &str, argv: &[&str]) -> String {
        match edit(text, argv).expect("edit succeeds") {
            EditResult::Changed { new_text, .. } => new_text,
            EditResult::Unchanged => panic!("expected a change"),
        }
    }

    #[test]
    fn parse_id_list_handles_ids_ranges_and_lists() {
        let cart = Cart::parse(CART).unwrap();
        assert_eq!(parse_id_list("0", &cart).unwrap(), vec![0]);
        assert_eq!(parse_id_list("0-0", &cart).unwrap(), vec![0]);
        assert!(parse_id_list("1", &cart).unwrap_err().contains("no sfx 1"));
        assert!(parse_id_list("3-1", &cart).unwrap_err().contains("above"));
    }

    #[test]
    fn transpose_only_rewrites_note_columns() {
        let out = changed(CART, &["c", "transpose", "0", "+2"]);
        assert!(out.contains("D4 2 5"), "{out}");
        assert!(out.contains("F#4 2 5"), "{out}");
        assert!(out.contains("A4 2 5"), "{out}");
        // The rest and every other section are untouched.
        assert!(out.contains("\n---\n"));
        assert!(out.contains("pat 0 : 0 - - -"));
    }

    #[test]
    fn transpose_range_error_suggests_a_shift() {
        let e = edit(CART, &["c", "transpose", "0", "-60"]).unwrap_err();
        assert!(e.contains("leaves the note table"), "{e}");
        assert!(e.contains("nearest to -60 is -48"), "{e}");
    }

    #[test]
    fn signed_operands_are_not_flags() {
        let args = EditArgs::parse(&["c", "transpose", "0", "-12"].map(String::from)).unwrap();
        assert_eq!(args.positional.len(), 4);
        assert_eq!(args.positional[3], "-12");
    }
}

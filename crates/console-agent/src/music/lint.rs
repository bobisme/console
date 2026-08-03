//! `music lint` — the numeric layer for music, and a checklist of the
//! mistakes that have actually bitten.
//!
//! Every rule here exists because it is invisible in the cart text and
//! inaudible to an agent. None of them is a matter of taste: each one is
//! either a documented engine behaviour the author probably did not intend
//! (an `env` sustain that swells a quiet row *up*, a vibrato delay longer
//! than the row it is on) or a measurable number (a wavetable's DC offset, a
//! pattern's peak level).
//!
//! Output is JSON on stdout, always. The exit code is 0 unless `--strict`,
//! because a lint that fails a build by default gets switched off; the point
//! is that an agent reads the diagnostics.
//!
//! ## Rules
//!
//! | rule | severity | what it catches |
//! |------|----------|-----------------|
//! | `env_sustain_swell` | warn | an `env` instrument with a non-zero sustain played at more than one row volume — the sustain is an *absolute* level, so the quiet rows swell **up** to it |
//! | `vib_delay_exceeds_row` | warn | `vib=...,<delay>` at least as long as the row it plays on: the vibrato never speaks |
//! | `note_out_of_range` | warn | a row whose note plus its `sweep`/`arp`/`sl` offset leaves the C0-B7 table, where the synth clamps |
//! | `fm_aliasing` | info | an FM instrument whose modulator (note x ratio) passes Nyquist at a note it actually plays: deterministic, but it will fold back |
//! | `wavetable_dc_offset` | warn | a `wavetable` whose codes do not pair around the centre (`dc_sum != 0`), i.e. a constant DC offset |
//! | `no_sfx_headroom` | warn | *every* pattern fills all six slots, so the first auto-allocated `sfx()` must steal channel 5 from the music |
//! | `undefined_reference` | error | the Lua calls `music(n)` / `sfx(n)` with an id the cart never defines (a runtime halt; the parser cannot see it) |
//! | `unreachable_pattern` | warn | a defined pattern no chain and no `music()` call ever reaches |
//! | `chain_has_no_terminator` | warn | a song that neither loops nor stops — it just runs out of pattern ids and goes quiet |
//! | `pattern_clipping` | warn | a pattern whose own headless render hits the output clamp |
//!
//! Parse-time errors are deliberately *not* re-checked here: a pattern slot
//! naming an undefined sfx, a `loop=` to an undefined pattern, an unknown
//! instrument on a row and a `w<slot>` reference to an undefined wavetable
//! are all rejected by `Cart::parse`, so a cart that reaches this code cannot
//! contain them.

use std::collections::{BTreeMap, BTreeSet};

use console_core::{CHANNEL_COUNT, Cart, Fx, NOTE_FREQ, SAMPLE_RATE, SfxRow, WAVETABLE_SLOTS};
use serde_json::{Value, json};

use crate::audio::note_name;

use super::{SongEnd, cart_arg, occupied_slots, parse_flags, plan_song, seconds, slots_text};

/// Highest note index the `NOTE_FREQ` table covers (B7).
const MAX_NOTE: i32 = NOTE_FREQ.len() as i32 - 1;
/// Above this the synth's modulator folds back into the audible band.
const NYQUIST_HZ: f32 = SAMPLE_RATE as f32 / 2.0;

/// One diagnostic. `severity` is `"error"`, `"warn"` or `"info"`; the
/// remaining fields vary by rule and are merged in as a flat object so a
/// caller can `jq` for `.diagnostics[] | select(.sfx == 3)`.
fn diag(rule: &str, severity: &str, message: String, extra: Value) -> Value {
    let mut v = json!({ "rule": rule, "severity": severity, "message": message });
    if let (Some(obj), Some(add)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in add {
            obj.insert(k.clone(), val.clone());
        }
    }
    v
}

/// Run every rule and return the full JSON report.
pub fn lint(cart: &Cart) -> Value {
    let mut d: Vec<Value> = Vec::new();

    let entries = entry_points(cart);
    let songs = songs_report(cart, &entries.ids, &mut d);
    instrument_rules(cart, &mut d);
    wavetable_rules(cart, &mut d);
    headroom_rule(cart, &mut d);
    reference_rules(cart, &mut d);
    reachability_rule(cart, &entries, &mut d);
    let patterns = pattern_report(cart, &mut d);

    let mut counts = BTreeMap::from([("error", 0u32), ("warn", 0u32), ("info", 0u32)]);
    for item in &d {
        if let Some(s) = item.get("severity").and_then(Value::as_str)
            && let Some(c) = counts.get_mut(s)
        {
            *c += 1;
        }
    }

    json!({
        "title": cart.title(),
        "entry_points": entries.ids,
        "entry_source": entries.source,
        "entry_dynamic": entries.dynamic,
        "songs": songs,
        "patterns": patterns,
        "instruments": cart.instruments().len(),
        "sfx": cart.audio().sfx_ids().count(),
        "diagnostics": d,
        "counts": {
            "error": counts["error"],
            "warn": counts["warn"],
            "info": counts["info"],
        },
    })
}

/// True when the report has anything `--strict` should fail on.
pub fn has_findings(report: &Value) -> bool {
    let c = &report["counts"];
    c["error"].as_u64().unwrap_or(0) > 0 || c["warn"].as_u64().unwrap_or(0) > 0
}

// ---------------------------------------------------------------------------
// Entry points and song chains
// ---------------------------------------------------------------------------

struct Entries {
    ids: Vec<u8>,
    source: &'static str,
    /// The cart calls `music(<something computed>)`, so any pattern could be
    /// a song head and reachability is unknowable.
    dynamic: bool,
}

/// Which patterns are *song heads*.
///
/// `__music__` alone cannot answer this — a pattern in the middle of a chain
/// looks exactly like one at its start — so the cart's Lua is the evidence:
/// the integer literals it passes to `music(...)`. That is the real answer
/// whenever it exists (`music(0)` title, `music(8)` gameplay is the documented
/// multi-song idiom). Failing that, fall back to patterns nothing chains
/// into, and failing *that* (a cart that is one pure cycle) the lowest id.
/// The report says which of the three it used, so "unreachable" can be read
/// with the right amount of trust.
fn entry_points(cart: &Cart) -> Entries {
    let defined: BTreeSet<u8> = cart.audio().pattern_ids().collect();
    let dynamic = super::has_dynamic_music_call(cart.lua());
    let from_lua: Vec<u8> = super::music_call_args(cart.lua())
        .into_iter()
        .filter_map(|n| u8::try_from(n).ok())
        .filter(|id| defined.contains(id))
        .collect::<BTreeSet<u8>>()
        .into_iter()
        .collect();
    if !from_lua.is_empty() {
        return Entries {
            ids: from_lua,
            source: if dynamic {
                "music() calls in __lua__ (plus at least one computed argument)"
            } else {
                "music() calls in __lua__"
            },
            dynamic,
        };
    }

    let mut has_predecessor: BTreeSet<u8> = BTreeSet::new();
    for id in &defined {
        for succ in successors(cart, *id) {
            has_predecessor.insert(succ);
        }
    }
    let heads: Vec<u8> = defined
        .iter()
        .copied()
        .filter(|id| !has_predecessor.contains(id))
        .collect();
    if !heads.is_empty() {
        return Entries {
            ids: heads,
            source: "patterns nothing chains into",
            dynamic,
        };
    }
    Entries {
        ids: defined.iter().copied().take(1).collect(),
        source: "lowest pattern id (every pattern is inside a cycle)",
        dynamic,
    }
}

/// The pattern(s) `id` can hand off to.
fn successors(cart: &Cart, id: u8) -> Vec<u8> {
    use console_core::PatternEnd;
    match cart.pattern(id).map(|p| p.end) {
        Some(PatternEnd::Stop) | None => Vec::new(),
        Some(PatternEnd::Loop(t)) => vec![t],
        Some(PatternEnd::Next) => cart.audio().next_pattern_after(id).into_iter().collect(),
    }
}

/// One entry per song head: its resolved chain, its timings, and the
/// `chain_has_no_terminator` rule.
fn songs_report(cart: &Cart, entries: &[u8], d: &mut Vec<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for &start in entries {
        let Ok(plan) = plan_song(cart, start) else {
            continue;
        };
        if plan.end == SongEnd::FellOffEnd {
            let last = plan.steps.last().map(|s| s.pattern).unwrap_or(start);
            d.push(diag(
                "chain_has_no_terminator",
                "warn",
                format!(
                    "song music({start}) ends by running out of pattern ids at pat {last}: \
                     no `loop=<id>` and no `stop`, so the music just goes quiet. Add \
                     `loop=<id>` to repeat or `stop` to make the silence deliberate."
                ),
                json!({ "song": start, "last_pattern": last }),
            ));
        }
        out.push(json!({
            "start": start,
            "chain": plan.chain_text(),
            "patterns": plan.pattern_ids(),
            "loop_pattern": plan.loop_pattern(),
            "intro_frames": plan.intro_frames(),
            "loop_frames": plan.loop_frames(),
            "intro_seconds": seconds(plan.intro_frames()),
            "loop_seconds": seconds(plan.loop_frames()),
            "terminator": match plan.end {
                SongEnd::Stop => "stop".to_string(),
                SongEnd::Loop { target } => format!("loop={target}"),
                SongEnd::FellOffEnd => "none".to_string(),
                SongEnd::Undefined(id) => format!("undefined pattern {id}"),
            },
        }));
    }
    out
}

// ---------------------------------------------------------------------------
// Instrument / row rules
// ---------------------------------------------------------------------------

/// Per-sfx, per-row rules: the env-sustain swell trap, the vibrato-delay
/// trap, out-of-range notes, and FM aliasing.
fn instrument_rules(cart: &Cart, d: &mut Vec<Value>) {
    let bank = cart.audio();
    for sid in bank.sfx_ids() {
        let Some(sfx) = cart.sfx(sid) else { continue };
        let speed = u32::from(sfx.speed);
        // Volumes each instrument is played at within this sfx, for the
        // env-sustain rule.
        let mut vols_by_inst: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();

        for (row, entry) in sfx.rows.iter().enumerate() {
            let SfxRow::Note { note, vol, .. } = entry else {
                continue;
            };
            let m = sfx.row_mod(row);
            let inst = m.inst.and_then(|i| bank.instrument_at(i));
            if let (Some(idx), Some(i)) = (m.inst, inst)
                && i.env.is_some()
            {
                vols_by_inst.entry(idx).or_default().insert(*vol);
            }

            // Vibrato that never speaks: the delay outlives the row.
            let vib_delay = match m.fx {
                Some(Fx::Vibrato(v)) => Some((v.delay, "row fx")),
                _ => inst.and_then(|i| i.vib).map(|v| (v.delay, "instrument")),
            };
            if let Some((delay, from)) = vib_delay
                && u32::from(delay) >= speed
                && delay > 0
            {
                d.push(diag(
                    "vib_delay_exceeds_row",
                    "warn",
                    format!(
                        "sfx {sid} row {row}: vibrato delay {delay} frames ({from}) is not shorter \
                         than the row's {speed} frames, so the vibrato never starts. Lower the \
                         delay below {speed} or slow the sfx down."
                    ),
                    json!({
                        "sfx": sid, "row": row, "delay": delay, "row_frames": speed,
                        "instrument": inst.map(|i| i.name.clone()),
                    }),
                ));
            }

            // Notes pushed off the end of the note table by sweep/arp/slide.
            let (lo, hi) = offset_span(&m.fx, inst.and_then(|i| i.sweep));
            let base = i32::from(*note);
            if base + lo < 0 || base + hi > MAX_NOTE {
                d.push(diag(
                    "note_out_of_range",
                    "warn",
                    format!(
                        "sfx {sid} row {row}: {} with offsets {lo}..{hi} semitones leaves the \
                         C0-B7 table ({}..{}), where the synth clamps to the end of the range.",
                        note_name(*note),
                        base + lo,
                        base + hi,
                    ),
                    json!({
                        "sfx": sid, "row": row, "note": note_name(*note),
                        "min_semitone": base + lo, "max_semitone": base + hi,
                    }),
                ));
            }

            // FM modulators past Nyquist at the notes they actually play.
            if let Some(i) = inst
                && let Some(fm) = i.fm
            {
                let top = (base + hi).clamp(0, MAX_NOTE) as usize;
                let mod_hz = NOTE_FREQ[top] * fm.ratio();
                if mod_hz > NYQUIST_HZ {
                    d.push(diag(
                        "fm_aliasing",
                        "info",
                        format!(
                            "sfx {sid} row {row}: instrument {:?} at {} has a modulator at \
                             {mod_hz:.0} Hz (note x ratio {}), past the {NYQUIST_HZ:.0} Hz \
                             Nyquist limit, so its sidebands fold back. Deterministic and often \
                             the point for metallic timbres - drop the ratio or play lower if it \
                             is not.",
                            i.name,
                            note_name(top as u8),
                            fm.ratio_text(),
                        ),
                        json!({
                            "sfx": sid, "row": row, "instrument": i.name.clone(),
                            "ratio": fm.ratio(), "modulator_hz": mod_hz.round(),
                        }),
                    ));
                }
            }
        }

        for (idx, vols) in vols_by_inst {
            if vols.len() < 2 {
                continue;
            }
            let Some(i) = bank.instrument_at(idx) else {
                continue;
            };
            let Some(env) = i.env else { continue };
            if env.sustain == 0 {
                continue;
            }
            let vols: Vec<u8> = vols.into_iter().collect();
            d.push(diag(
                "env_sustain_swell",
                "warn",
                format!(
                    "sfx {sid} plays instrument {:?} at volumes {vols:?}, but its env sustain \
                     ({}) is an ABSOLUTE level: every one of those rows ends up at {} regardless \
                     of what the row asked for, so the quiet rows swell UP. Give the voice no \
                     env if it needs per-row dynamics, or keep its rows at one volume.",
                    i.name, env.sustain, env.sustain,
                ),
                json!({
                    "sfx": sid, "instrument": i.name.clone(),
                    "sustain": env.sustain, "row_volumes": vols,
                }),
            ));
        }
    }
}

/// The semitone offsets a row can reach, as `(min, max)` relative to its own
/// note: the fx column plus the instrument's sweep.
fn offset_span(fx: &Option<Fx>, sweep: Option<console_core::Sweep>) -> (i32, i32) {
    let (mut lo, mut hi) = (0i32, 0i32);
    if let Some(s) = sweep {
        lo = lo.min(i32::from(s.semis));
        hi = hi.max(i32::from(s.semis));
    }
    match fx {
        Some(Fx::Arp { a, b }) => {
            hi = hi.max(i32::from(*a)).max(i32::from(*b));
        }
        Some(Fx::Slide { semis }) => {
            lo = lo.min(i32::from(*semis));
            hi = hi.max(i32::from(*semis));
        }
        _ => {}
    }
    (lo, hi)
}

/// Wavetables whose codes do not pair around the centre. `dc_sum` is exact
/// integer arithmetic (`sum of 2n - 15`), so this is a decidable question,
/// not a float comparison.
fn wavetable_rules(cart: &Cart, d: &mut Vec<Value>) {
    for slot in 0..WAVETABLE_SLOTS as u8 {
        let Some(table) = cart.wavetable(slot) else {
            continue;
        };
        let dc = table.dc_sum();
        if dc == 0 {
            continue;
        }
        // Amplitude of the constant offset: dc/32 codes, each code 1/15 of
        // full scale.
        let offset = f64::from(dc) / (32.0 * 15.0);
        d.push(diag(
            "wavetable_dc_offset",
            "warn",
            format!(
                "wavetable {slot} has dc_sum {dc} (not 0), a constant {offset:+.3} DC offset on \
                 every note it plays. It eats headroom, thumps on note-on and does not sound \
                 like anything. Pair every `8` with a `7` (the codes must balance around the \
                 centre) to make the table DC-free."
            ),
            json!({ "wavetable": slot, "dc_sum": dc, "dc_offset": (offset * 1000.0).round() / 1000.0 }),
        ));
    }
}

/// The channel budget: a song that fills all six slots in every pattern
/// leaves `sfx()` nowhere to go.
fn headroom_rule(cart: &Cart, d: &mut Vec<Value>) {
    let ids: Vec<u8> = cart.audio().pattern_ids().collect();
    if ids.is_empty() {
        return;
    }
    let full: Vec<u8> = ids
        .iter()
        .copied()
        .filter(|id| occupied_slots(cart, *id) == CHANNEL_COUNT)
        .collect();
    if full.len() != ids.len() {
        return;
    }
    d.push(diag(
        "no_sfx_headroom",
        "warn",
        format!(
            "every pattern ({}) fills all {CHANNEL_COUNT} channels, so the first auto-allocated \
             `sfx(n)` has to steal channel 5 out from under the music. Leave 1-2 slots as `-` in \
             the patterns that play during gameplay, or always pass an explicit channel to \
             `sfx()`.",
            full.iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        json!({ "patterns": full, "channels": CHANNEL_COUNT }),
    ));
}

/// `music(n)` / `sfx(n)` literals in the Lua naming ids the cart never
/// defines. The parser cannot catch these — they are arguments, not cart data
/// — and each one is a hard runtime halt the moment that line executes.
fn reference_rules(cart: &Cart, d: &mut Vec<Value>) {
    let patterns: BTreeSet<u8> = cart.audio().pattern_ids().collect();
    let sfx: BTreeSet<u8> = cart.audio().sfx_ids().collect();

    for n in super::music_call_args(cart.lua()) {
        if n < 0 {
            continue; // music(-1) stops music
        }
        if u8::try_from(n)
            .ok()
            .is_none_or(|id| !patterns.contains(&id))
        {
            d.push(diag(
                "undefined_reference",
                "error",
                format!(
                    "__lua__ calls music({n}) but the cart defines no pattern {n} (defined: {}). \
                     This halts the cart the moment it runs.",
                    super::pattern_id_list(cart)
                ),
                json!({ "kind": "music", "id": n }),
            ));
        }
    }
    for n in super::sfx_call_args(cart.lua()) {
        if n < 0 {
            continue; // sfx(-1) stops channels
        }
        if u8::try_from(n).ok().is_none_or(|id| !sfx.contains(&id)) {
            let defined: Vec<String> = sfx.iter().map(|i| i.to_string()).collect();
            d.push(diag(
                "undefined_reference",
                "error",
                format!(
                    "__lua__ calls sfx({n}) but the cart defines no sfx {n} (defined: {}). This \
                     halts the cart the moment it runs.",
                    if defined.is_empty() {
                        "none".to_string()
                    } else {
                        defined.join(", ")
                    },
                ),
                json!({ "kind": "sfx", "id": n }),
            ));
        }
    }
}

/// Patterns no chain from any entry point ever reaches — dead weight, or (far
/// more often) a song whose `loop=` points at the wrong id.
fn reachability_rule(cart: &Cart, entries: &Entries, d: &mut Vec<Value>) {
    // A cart that calls `music(<computed>)` can enter any pattern it likes,
    // so "unreachable" is not a question this rule can answer. Staying quiet
    // beats a wall of false positives — a jukebox cart like `soundtest.cart`
    // would otherwise flag fifteen of its sixteen patterns.
    if entries.dynamic {
        return;
    }
    let entries = &entries.ids;
    let defined: BTreeSet<u8> = cart.audio().pattern_ids().collect();
    if defined.is_empty() {
        return;
    }
    let mut reached: BTreeSet<u8> = BTreeSet::new();
    for &start in entries {
        if let Ok(plan) = plan_song(cart, start) {
            reached.extend(plan.pattern_ids());
        }
    }
    let orphans: Vec<u8> = defined.difference(&reached).copied().collect();
    if orphans.is_empty() {
        return;
    }
    d.push(diag(
        "unreachable_pattern",
        "warn",
        format!(
            "pattern(s) {} are never reached from any song head ({}): nothing chains into them \
             and no `music()` call names them, so they never play.",
            orphans
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            if entries.is_empty() {
                "none found".to_string()
            } else {
                entries
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ),
        json!({ "patterns": orphans, "entry_points": entries }),
    ));
}

// ---------------------------------------------------------------------------
// Per-pattern measurement
// ---------------------------------------------------------------------------

/// Every pattern's shape plus its measured level, from a headless render of
/// that pattern on its own (see [`super::render::pattern_samples`]).
fn pattern_report(cart: &Cart, d: &mut Vec<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for id in cart.audio().pattern_ids() {
        let frames = super::pattern_frames(cart, id);
        let mut entry = json!({
            "id": id,
            "frames": frames,
            "seconds": seconds(u64::from(frames)),
            "slots": slots_text(cart, id),
            "channels_used": occupied_slots(cart, id),
        });
        match super::render::pattern_samples(cart, id) {
            Ok(samples) => {
                let (peak, rms, clipped) = level(&samples);
                entry["peak"] = json!(round3(f64::from(peak)));
                entry["rms"] = json!(round3(f64::from(rms)));
                entry["clipped"] = json!(clipped);
                if clipped > 0 {
                    d.push(diag(
                        "pattern_clipping",
                        "warn",
                        format!(
                            "pattern {id} renders {clipped} clipped samples (peak {peak:.3}) on \
                             its own. Six voices sum past full scale at the fixed 0.25 per-channel \
                             mix gain: drop the busy rows to vol 4-5, or set `master drive=1`, \
                             which soft-limits below the clamp."
                        ),
                        json!({ "pattern": id, "clipped": clipped, "peak": round3(f64::from(peak)) }),
                    ));
                }
            }
            Err(e) => entry["render_error"] = json!(e),
        }
        out.push(entry);
    }
    out
}

fn level(samples: &[f32]) -> (f32, f32, u32) {
    let mut peak = 0f32;
    let mut sum_sq = 0f64;
    let mut clipped = 0u32;
    for &s in samples {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        sum_sq += f64::from(s) * f64::from(s);
        if a >= 0.999 {
            clipped += 1;
        }
    }
    let rms = (sum_sq / samples.len().max(1) as f64).sqrt() as f32;
    (peak, rms, clipped)
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub fn cli_lint(args: &[String]) -> Result<i32, String> {
    let flags = parse_flags(args)?;
    let path = cart_arg(&flags, "lint")?;
    let (_text, cart) = super::read_cart(&path)?;
    let report = lint(&cart);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(if flags.strict && has_findings(&report) {
        1
    } else {
        0
    })
}

//! Tests for the music authoring tools (`console_agent::music`) and their
//! RPC mirrors — SPEC.md "Music authoring (PoC v2)".
//!
//! Fixtures are inline cart text, like the sprite/map tests, so each one
//! isolates exactly the thing under test. The workhorse is [`SONG_CART`], a
//! three-pattern song with a one-pattern intro and a two-pattern loop body
//! and deliberately *different* durations per pattern (20 / 6 / 6 frames), so
//! any confusion between "patterns" and "frames" shows up immediately in the
//! chain math:
//!
//! ```text
//! pat 0 -> [pat 1 -> pat 2 ->] loop to 1
//! intro = 20 frames, loop body = 12 frames
//! ```

use console_agent::music::{self, SongEnd, roll::RollOpts};
use console_agent::rpc::handle;
use console_agent::session::Session;
use console_core::{Cart, PALETTE};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Intro (pat 0, 4 rows x speed 5 = 20 frames) then a two-pattern loop body
/// (pat 1 = 2 x 3 = 6 frames, pat 2 = 3 x 2 = 6 frames) that jumps back to
/// pat 1.
const SONG_CART: &str = "\
__lua__
function _init() music(0) end

__sfx__
sfx 0 speed=5
C4 2 5
E4 2 5
G4 2 5
---
sfx 1 speed=3
A2 3 6
---
sfx 2 speed=2
C3 1 4
D3 1 4
E3 1 4

__music__
pat 0 : 0 - - -
pat 1 : 1 - - -
pat 2 loop=1 : 2 - - -
";

/// A one-shot jingle: one pattern, 20 frames, `stop`.
const STOP_CART: &str = "\
__lua__
function _init() end

__sfx__
sfx 0 speed=5
C4 2 5
E4 2 5
G4 2 5
---

__music__
pat 0 stop : 0 - - -
";

fn cart(text: &str) -> Cart {
    Cart::parse(text).expect("fixture cart parses")
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("console-music-{}-{name}", std::process::id()))
}

/// Assemble a cart from optional audio sections plus a Lua body — the lint
/// fixtures differ only in which sections they carry.
fn build_cart(lua: &str, instruments: &str, sfx: &str, music: &str) -> String {
    let mut s = format!("__lua__\n{lua}\n");
    for (name, body) in [("instruments", instruments), ("sfx", sfx), ("music", music)] {
        if !body.trim().is_empty() {
            s.push_str(&format!("__{name}__\n{body}\n"));
        }
    }
    s
}

/// Every diagnostic of one rule in a lint report.
fn rule_hits<'r>(report: &'r Value, rule: &str) -> Vec<&'r Value> {
    report["diagnostics"]
        .as_array()
        .expect("diagnostics is an array")
        .iter()
        .filter(|d| d["rule"] == rule)
        .collect()
}

fn lint_text(text: &str) -> Value {
    music::lint::lint(&cart(text))
}

// ---------------------------------------------------------------------------
// Song plan (the chain every other tool is built on)
// ---------------------------------------------------------------------------

#[test]
fn plan_resolves_intro_and_loop_body_with_their_own_frame_counts() {
    let cart = cart(SONG_CART);
    let plan = music::plan_song(&cart, 0).expect("plan song 0");

    assert_eq!(plan.pattern_ids(), vec![0, 1, 2]);
    assert_eq!(plan.loop_index, Some(1));
    assert_eq!(plan.loop_pattern(), Some(1));
    assert_eq!(plan.end, SongEnd::Loop { target: 1 });
    assert_eq!(plan.intro_frames(), 20);
    assert_eq!(plan.loop_frames(), 12);
    assert_eq!(plan.frames_for(2), 44);
    assert_eq!(plan.frames_for(0), 20);
}

#[test]
fn plan_starting_inside_the_loop_has_no_intro() {
    let cart = cart(SONG_CART);
    let plan = music::plan_song(&cart, 1).expect("plan song 1");
    assert_eq!(plan.pattern_ids(), vec![1, 2]);
    assert_eq!(plan.loop_index, Some(0));
    assert_eq!(plan.intro_frames(), 0);
    assert_eq!(plan.loop_frames(), 12);
}

#[test]
fn plan_reports_stop_and_falling_off_the_end() {
    let stop = cart(STOP_CART);
    let plan = music::plan_song(&stop, 0).expect("plan");
    assert_eq!(plan.end, SongEnd::Stop);
    assert_eq!(plan.loop_index, None);
    assert_eq!(plan.frames_for(9), 20, "loops do not apply to a stop song");

    let fell = cart(&STOP_CART.replace("pat 0 stop :", "pat 0 :"));
    assert_eq!(
        music::plan_song(&fell, 0).expect("plan").end,
        SongEnd::FellOffEnd
    );
}

#[test]
fn plan_rejects_an_undefined_song_id() {
    let err = music::plan_song(&cart(SONG_CART), 9).unwrap_err();
    assert!(err.contains("no pattern 9"), "{err}");
    assert!(err.contains("0, 1, 2"), "{err}");
}

// ---------------------------------------------------------------------------
// score
// ---------------------------------------------------------------------------

#[test]
fn score_prints_the_form_chain_with_the_loop_body_bracketed() {
    let text = music::score::score(&cart(SONG_CART), None).expect("score");
    assert!(
        text.contains("form:  pat 0 -> [pat 1 -> pat 2 ->] loop to 1"),
        "chain line missing from:\n{text}"
    );
    assert!(text.contains("intro 20 frames"), "{text}");
    assert!(text.contains("loop 12 frames"), "{text}");
}

#[test]
fn score_labels_each_pattern_with_its_role_and_hand_off() {
    let text = music::score::score(&cart(SONG_CART), None).expect("score");
    assert!(text.contains("== pat 0 [intro]"), "{text}");
    assert!(text.contains("== pat 1 [loop start]"), "{text}");
    assert!(text.contains("== pat 2 [loop body]"), "{text}");
    assert!(text.contains("end:    next -> pat 1"), "{text}");
    assert!(
        text.contains("end:    loop=1 (jump back to pat 1)"),
        "{text}"
    );
}

#[test]
fn score_grid_is_row_indexed_and_carries_note_voice_volume_and_fx() {
    let text = music::score::score(&cart(SONG_CART), None).expect("score");
    // Pattern 0, row 0: C4 on wave 2 at volume 5, at frame 0.
    assert!(
        text.lines()
            .any(|l| l.starts_with("     0      0 |") && l.contains("C4  2 5")),
        "row 0 line missing from:\n{text}"
    );
    // Row 3 of sfx 0 is a rest.
    assert!(
        text.lines()
            .any(|l| l.starts_with("     3     15 |") && l.contains("---")),
        "rest row missing from:\n{text}"
    );
    assert!(text.contains("ch0 sfx00 sp5"), "column header missing");
}

#[test]
fn score_renders_an_fx_column_and_a_named_instrument() {
    let text = build_cart(
        "function _init() music(0) end",
        "inst lead wave=2 env=0,4,3",
        "sfx 0 speed=6\nA4 lead 5 arp3,7\nC5 lead 5 sl+2\nE5 lead 5 fade-2\nG5 w0 4",
        "pat 0 stop : 0 - - -",
    );
    // `w0` needs a table to point at.
    let text = text.replace(
        "inst lead wave=2 env=0,4,3",
        "wavetable 0 ffffffff000000000ffffffff0000000\ninst lead wave=2 env=0,4,3",
    );
    let score = music::score::score(&cart(&text), None).expect("score");
    assert!(score.contains("A4  lead 5 arp3,7"), "{score}");
    assert!(score.contains("C5  lead 5 sl+2"), "{score}");
    assert!(score.contains("E5  lead 5 fade-2"), "{score}");
    assert!(
        score.contains("G5  w0 4"),
        "a bare wavetable slot prints as w0:\n{score}"
    );
}

#[test]
fn score_time_aligns_slots_that_run_at_different_speeds() {
    let text = build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=4\nC4 2 5\nD4 2 5\nsfx 1 speed=6\nA2 3 6\n---",
        "pat 0 stop : 0 1 - -",
    );
    let score = music::score::score(&cart(&text), None).expect("score");
    // Slot speeds differ (4 and 6), so rows are not a shared time unit: the
    // grid falls back to frames and the `row` column goes blank.
    assert!(score.contains("speed: 4/6"), "{score}");
    let grid: Vec<&str> = score
        .lines()
        .filter(|l| l.contains(" | ") && !l.contains("ch0 sfx"))
        .collect();
    // Row starts land at frames 0 (both), 4 (ch0), 6 (ch1).
    assert!(
        grid.iter().any(|l| l.contains("     -      4 |")),
        "{score}"
    );
    assert!(
        grid.iter().any(|l| l.contains("     -      6 |")),
        "{score}"
    );
    // ch1 holds through frame 4; ch0 holds through frame 6.
    let at4 = grid
        .iter()
        .find(|l| l.contains("      4 |"))
        .expect("frame 4");
    let cells: Vec<&str> = at4.split('|').map(str::trim).collect();
    assert_eq!(cells[1], "D4  2 5", "ch0 starts row 1 at frame 4: {at4}");
    assert_eq!(cells[2], ":", "ch1 is still holding its row 0: {at4}");
}

#[test]
fn score_song_flag_starts_the_chain_where_it_is_told() {
    let cart = cart(SONG_CART);
    let from_one = music::score::score(&cart, Some(1)).expect("score");
    assert!(from_one.contains("song:  music(1)"), "{from_one}");
    assert!(
        from_one.contains("form:  [pat 1 -> pat 2 ->] loop to 1"),
        "{from_one}"
    );
    assert!(!from_one.contains("== pat 0"), "pat 0 is not in this song");
}

// ---------------------------------------------------------------------------
// lint — one firing case and one quiet case per rule
// ---------------------------------------------------------------------------

#[test]
fn lint_env_sustain_swell() {
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst pad wave=2 env=2,4,3",
        "sfx 0 speed=8\nC4 pad 5\nE4 pad 3",
        "pat 0 stop : 0 - - -",
    ));
    let hits = rule_hits(&fires, "env_sustain_swell");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(hits[0]["sustain"], 3);
    assert_eq!(hits[0]["row_volumes"], json!([3, 5]));

    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst pad wave=2 env=2,4,3",
        "sfx 0 speed=8\nC4 pad 5\nE4 pad 5",
        "pat 0 stop : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "env_sustain_swell").is_empty(),
        "{quiet:#}"
    );
}

#[test]
fn lint_vib_delay_exceeds_row() {
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst lead wave=2 vib=30,8,12",
        "sfx 0 speed=8\nC4 lead 5",
        "pat 0 stop : 0 - - -",
    ));
    let hits = rule_hits(&fires, "vib_delay_exceeds_row");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(hits[0]["delay"], 12);
    assert_eq!(hits[0]["row_frames"], 8);

    // Same instrument on a slower sfx: the delay now fits inside the row.
    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst lead wave=2 vib=30,8,12",
        "sfx 0 speed=16\nC4 lead 5",
        "pat 0 stop : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "vib_delay_exceeds_row").is_empty(),
        "{quiet:#}"
    );
}

#[test]
fn lint_trem_delay_exceeds_row() {
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst pad wave=2 trem=8,4,12",
        "sfx 0 speed=8\nC4 pad 5",
        "pat 0 stop : 0 - - -",
    ));
    let hits = rule_hits(&fires, "trem_delay_exceeds_row");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(hits[0]["delay"], 12);
    assert_eq!(hits[0]["row_frames"], 8);

    // Same instrument on a slower sfx: the delay now fits inside the row.
    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst pad wave=2 trem=8,4,12",
        "sfx 0 speed=16\nC4 pad 5",
        "pat 0 stop : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "trem_delay_exceeds_row").is_empty(),
        "{quiet:#}"
    );
}

#[test]
fn lint_note_out_of_range() {
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst kick wave=3 sweep=-14,5",
        "sfx 0 speed=8\nC0 kick 7",
        "pat 0 stop : 0 - - -",
    ));
    let hits = rule_hits(&fires, "note_out_of_range");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(hits[0]["min_semitone"], -14);

    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst kick wave=3 sweep=-14,5",
        "sfx 0 speed=8\nD2 kick 7",
        "pat 0 stop : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "note_out_of_range").is_empty(),
        "{quiet:#}"
    );
}

#[test]
fn lint_fm_aliasing() {
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst bell wave=6 fm=15,11,2",
        "sfx 0 speed=8\nB7 bell 5",
        "pat 0 stop : 0 - - -",
    ));
    let hits = rule_hits(&fires, "fm_aliasing");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(
        hits[0]["severity"], "info",
        "aliasing is a flag, not a failure"
    );
    assert!(
        hits[0]["modulator_hz"].as_f64().expect("a number") > 22050.0,
        "{fires:#}"
    );

    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "inst bell wave=6 fm=15,11,2",
        "sfx 0 speed=8\nC2 bell 5",
        "pat 0 stop : 0 - - -",
    ));
    assert!(rule_hits(&quiet, "fm_aliasing").is_empty(), "{quiet:#}");
}

#[test]
fn lint_wavetable_dc_offset() {
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "wavetable 0 88888888888888888888888888888888\ninst pad wave=w0",
        "sfx 0 speed=8\nC4 pad 5",
        "pat 0 stop : 0 - - -",
    ));
    let hits = rule_hits(&fires, "wavetable_dc_offset");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(hits[0]["dc_sum"], 32);

    // The square wave: sixteen `f`s against sixteen `0`s, exactly DC-free.
    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "wavetable 0 ffffffffffffffff0000000000000000\ninst pad wave=w0",
        "sfx 0 speed=8\nC4 pad 5",
        "pat 0 stop : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "wavetable_dc_offset").is_empty(),
        "{quiet:#}"
    );
}

#[test]
fn lint_no_sfx_headroom() {
    let six = "pat 0 stop : 0 0 0 0 0 0";
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        six,
    ));
    let hits = rule_hits(&fires, "no_sfx_headroom");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(hits[0]["patterns"], json!([0]));

    // One four-slot pattern is enough headroom for the rule to stay quiet.
    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        "pat 0 : 0 0 0 0 0 0\npat 1 stop : 0 - - -",
    ));
    assert!(rule_hits(&quiet, "no_sfx_headroom").is_empty(), "{quiet:#}");
}

#[test]
fn lint_undefined_reference_from_lua() {
    let fires = lint_text(&build_cart(
        "function _init() music(7) end\nfunction _update() sfx(9) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        "pat 0 stop : 0 - - -",
    ));
    let hits = rule_hits(&fires, "undefined_reference");
    assert_eq!(hits.len(), 2, "{fires:#}");
    assert!(hits.iter().all(|h| h["severity"] == "error"));
    assert_eq!(hits[0]["kind"], "music");
    assert_eq!(hits[1]["kind"], "sfx");

    // `music(-1)` / `sfx(-1)` are the documented stop calls, not references.
    let quiet = lint_text(&build_cart(
        "function _init() music(0) end\nfunction _update() sfx(-1) music(-1) sfx(0) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        "pat 0 stop : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "undefined_reference").is_empty(),
        "{quiet:#}"
    );
}

#[test]
fn lint_unreachable_pattern() {
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        "pat 0 stop : 0 - - -\npat 5 stop : 0 - - -",
    ));
    let hits = rule_hits(&fires, "unreachable_pattern");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(hits[0]["patterns"], json!([5]));

    // Chain pat 0 into pat 5 and nothing is orphaned.
    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        "pat 0 : 0 - - -\npat 5 stop : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "unreachable_pattern").is_empty(),
        "{quiet:#}"
    );
}

#[test]
fn lint_stays_quiet_about_reachability_when_music_is_called_dynamically() {
    // A jukebox cart (`soundtest.cart`'s shape): the pattern is computed, so
    // every pattern is a plausible song head and the rule has nothing to say.
    let report = lint_text(&build_cart(
        "local n = 0\nfunction _update() music(n) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        "pat 0 stop : 0 - - -\npat 5 stop : 0 - - -",
    ));
    assert_eq!(report["entry_dynamic"], true);
    assert!(
        rule_hits(&report, "unreachable_pattern").is_empty(),
        "{report:#}"
    );
}

#[test]
fn lint_chain_has_no_terminator() {
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        "pat 0 : 0 - - -",
    ));
    let hits = rule_hits(&fires, "chain_has_no_terminator");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert_eq!(hits[0]["last_pattern"], 0);

    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=8\nC4 2 3",
        "pat 0 loop=0 : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "chain_has_no_terminator").is_empty(),
        "{quiet:#}"
    );
}

#[test]
fn lint_pattern_clipping_measures_a_headless_render() {
    // Six full-scale voices on the same note sum to 1.5 at the fixed 0.25
    // per-channel mix gain, and the output clamp catches the difference.
    let fires = lint_text(&build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=20\nA4 2 7",
        "pat 0 stop : 0 0 0 0 0 0",
    ));
    let hits = rule_hits(&fires, "pattern_clipping");
    assert_eq!(hits.len(), 1, "{fires:#}");
    assert!(hits[0]["clipped"].as_u64().expect("a count") > 0);

    let quiet = lint_text(&build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=20\nA4 2 3",
        "pat 0 stop : 0 - - -",
    ));
    assert!(
        rule_hits(&quiet, "pattern_clipping").is_empty(),
        "{quiet:#}"
    );
    let peak = quiet["patterns"][0]["peak"].as_f64().expect("a peak");
    assert!((0.0..1.0).contains(&peak), "peak {peak} should be measured");
}

#[test]
fn lint_reports_the_song_chain_and_counts() {
    let report = lint_text(SONG_CART);
    assert_eq!(report["songs"][0]["start"], 0);
    assert_eq!(
        report["songs"][0]["chain"],
        "pat 0 -> [pat 1 -> pat 2 ->] loop to 1"
    );
    assert_eq!(report["songs"][0]["intro_frames"], 20);
    assert_eq!(report["songs"][0]["loop_frames"], 12);
    assert_eq!(report["entry_source"], "music() calls in __lua__");
    assert_eq!(report["counts"]["error"], 0);
    assert!(!music::lint::has_findings(&report), "{report:#}");
}

#[test]
fn lint_exit_code_is_zero_unless_strict() {
    let path = temp_path("strict.cart");
    // A `chain_has_no_terminator` warning and nothing else.
    std::fs::write(
        &path,
        build_cart(
            "function _init() music(0) end",
            "",
            "sfx 0 speed=8\nC4 2 3",
            "pat 0 : 0 - - -",
        ),
    )
    .expect("write fixture");
    let p = path.to_str().expect("utf-8 path");

    assert_eq!(
        console_agent::cli_main(&args(&["console", "music", "lint", p])),
        0,
        "lint is informational by default"
    );
    assert_eq!(
        console_agent::cli_main(&args(&["console", "music", "lint", p, "--strict"])),
        1,
        "--strict turns warnings into a failing exit code"
    );
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// piano-roll
// ---------------------------------------------------------------------------

#[test]
fn piano_roll_dimensions_follow_the_used_note_range_and_frame_count() {
    let cart = cart(SONG_CART);
    let (order, loop_at) = music::roll::resolve_order(&cart, None, None).expect("order");
    assert_eq!(order, vec![0, 1, 2]);
    assert_eq!(loop_at, Some(1));

    let img = music::roll::piano_roll(&cart, &order, loop_at, &RollOpts::default()).expect("roll");
    // 20 + 6 + 6 = 32 frames at 2 px each, plus the label gutter.
    assert_eq!(img.width, music::roll::GUTTER + 32 * 2);
    // Notes run A2 (33) to G4 (55): 23 semitones plus 2 of padding each side.
    assert_eq!(img.height, 27 * 5);
    assert_eq!(img.frames, 3);

    let big = music::roll::piano_roll(&cart, &order, loop_at, &RollOpts { cell: 4, row_h: 10 })
        .expect("roll");
    assert_eq!(big.width, music::roll::GUTTER + 32 * 4);
    assert_eq!(big.height, 27 * 10);
}

#[test]
fn piano_roll_colors_notes_by_channel_and_marks_the_loop() {
    let cart = cart(SONG_CART);
    let img =
        music::roll::piano_roll(&cart, &[0, 1, 2], Some(1), &RollOpts::default()).expect("roll");
    let px = |x: u32, y: u32| {
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    };

    // Pattern 0 row 0 is C4 (48) on channel 0 at volume 5. The top note drawn
    // is G4 (55) plus 2 semitones of padding, so C4 sits 9 rows down.
    let y = (55 + 2 - 48) * 5;
    assert_eq!(
        px(music::roll::GUTTER + 1, y + 1),
        music::roll::velocity_color(music::roll::CHANNEL_COLORS[0], 5),
        "C4 should be channel 0's color at velocity 5"
    );

    // Quieter notes of the same channel are dimmer, not a different hue.
    let loud = music::roll::velocity_color(music::roll::CHANNEL_COLORS[0], 7);
    let soft = music::roll::velocity_color(music::roll::CHANNEL_COLORS[0], 2);
    assert!(loud.iter().zip(&soft).all(|(l, s)| l >= s));
    assert_ne!(loud, soft);

    // The loop bar sits where pattern 1 begins: frame 20.
    assert_eq!(px(music::roll::GUTTER + 20 * 2, 0), PALETTE[12]);
    // The pattern 1 -> 2 boundary (frame 26) is an ordinary gridline.
    assert_eq!(px(music::roll::GUTTER + 26 * 2, 0), PALETTE[13]);
}

#[test]
fn piano_roll_patterns_flag_draws_exactly_what_it_lists() {
    let cart = cart(SONG_CART);
    let (order, loop_at) = music::roll::resolve_order(&cart, None, Some("2,1")).expect("order");
    assert_eq!(order, vec![2, 1]);
    assert_eq!(loop_at, None, "an explicit list has no loop point");

    let err = music::roll::resolve_order(&cart, Some(0), Some("1")).unwrap_err();
    assert!(err.contains("mutually exclusive"), "{err}");
    let err = music::roll::resolve_order(&cart, None, Some("9")).unwrap_err();
    assert!(err.contains("no pattern 9"), "{err}");
}

#[test]
fn piano_roll_refuses_a_song_with_nothing_to_draw() {
    let text = build_cart(
        "function _init() music(0) end",
        "",
        "sfx 0 speed=8\n---\n---",
        "pat 0 stop : 0 - - -",
    );
    let err = music::roll::piano_roll(&cart(&text), &[0], None, &RollOpts::default()).unwrap_err();
    assert!(err.contains("no notes"), "{err}");
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

#[test]
fn render_plays_the_intro_plus_exactly_k_loop_passes() {
    for (loops, expected) in [(0u32, 20u64), (1, 32), (2, 44), (3, 56)] {
        let opts = music::render::RenderOpts {
            loops,
            ..Default::default()
        };
        let (samples, report) = music::render::render_song(SONG_CART, &opts).expect("render");
        assert_eq!(
            report.frames, expected,
            "intro 20 + {loops} x loop 12 should be {expected} frames, got {report:?}"
        );
        assert_eq!(report.planned_frames, expected);
        assert_eq!(
            samples.len(),
            expected as usize * console_core::SAMPLES_PER_FRAME
        );
        assert!(!report.stopped_early, "{report:?}");
    }
}

#[test]
fn render_detects_a_pattern_that_loops_to_itself() {
    // The one-pattern song: `music_pattern()` never changes, so the loop has
    // to be detected from the pattern's own duration.
    let text = build_cart(
        "function _init() end",
        "",
        "sfx 0 speed=5\nC4 2 5\nE4 2 5",
        "pat 0 loop=0 : 0 - - -",
    );
    let (_, report) = music::render::render_song(
        &text,
        &music::render::RenderOpts {
            loops: 3,
            ..Default::default()
        },
    )
    .expect("render");
    assert_eq!(report.intro_frames, 0);
    assert_eq!(report.loop_frames, 10);
    assert_eq!(report.frames, 30);
    assert_eq!(report.loops_observed, 3);
}

#[test]
fn render_stops_where_the_song_stops() {
    let (samples, report) = music::render::render_song(
        STOP_CART,
        &music::render::RenderOpts {
            loops: 5,
            ..Default::default()
        },
    )
    .expect("render");
    assert_eq!(
        report.frames, 20,
        "a `stop` song ignores --loops: {report:?}"
    );
    assert_eq!(report.loop_frames, 0);
    assert_eq!(samples.len(), 20 * console_core::SAMPLES_PER_FRAME);
}

#[test]
fn render_frames_flag_overrides_loop_detection() {
    let (samples, report) = music::render::render_song(
        SONG_CART,
        &music::render::RenderOpts {
            frames: Some(7),
            ..Default::default()
        },
    )
    .expect("render");
    assert_eq!(report.frames, 7);
    assert_eq!(report.planned_frames, 0, "--frames means there is no plan");
    assert_eq!(samples.len(), 7 * console_core::SAMPLES_PER_FRAME);
}

#[test]
fn render_song_flag_selects_the_song_and_writes_a_wav() {
    let path = temp_path("song.cart");
    std::fs::write(&path, SONG_CART).expect("write fixture");
    let wav = temp_path("song.wav");
    let code = console_agent::cli_main(&args(&[
        "console",
        "music",
        "render",
        path.to_str().expect("utf-8"),
        "--song",
        "1",
        "--loops",
        "1",
        "-o",
        wav.to_str().expect("utf-8"),
    ]));
    assert_eq!(code, 0);

    // Song 1 has no intro and a 12-frame body: 44 bytes of header plus one
    // pass of 16-bit mono samples.
    let bytes = std::fs::read(&wav).expect("wav written");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(bytes.len(), 44 + 12 * console_core::SAMPLES_PER_FRAME * 2);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&wav);
}

#[test]
fn render_produces_audible_samples() {
    let (samples, _) = music::render::render_song(SONG_CART, &music::render::RenderOpts::default())
        .expect("render");
    let peak = samples.iter().fold(0f32, |a, s| a.max(s.abs()));
    assert!(
        peak > 0.05,
        "the render should not be silence (peak {peak})"
    );
}

// ---------------------------------------------------------------------------
// CLI dispatch
// ---------------------------------------------------------------------------

#[test]
fn cli_dispatches_the_music_subcommands() {
    let path = temp_path("cli.cart");
    std::fs::write(&path, SONG_CART).expect("write fixture");
    let p = path.to_str().expect("utf-8 path");
    let png = temp_path("cli.png");

    assert_eq!(
        console_agent::cli_main(&args(&["console", "music", "score", p])),
        0
    );
    assert_eq!(
        console_agent::cli_main(&args(&[
            "console",
            "music",
            "piano-roll",
            p,
            "-o",
            png.to_str().expect("utf-8"),
        ])),
        0
    );
    assert!(std::fs::metadata(&png).expect("png written").len() > 0);

    // Unknown subcommand and missing output path are usage errors, not panics.
    assert_eq!(
        console_agent::cli_main(&args(&["console", "music", "nope", p])),
        2
    );
    assert_eq!(
        console_agent::cli_main(&args(&["console", "music", "piano-roll", p])),
        2
    );
    assert_eq!(
        console_agent::cli_main(&args(&["console", "music", "score", p, "--song", "9"])),
        2
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&png);
}

// ---------------------------------------------------------------------------
// RPC mirrors
// ---------------------------------------------------------------------------

fn loaded_session() -> Session {
    let mut session = Session::new();
    session.load_cart(SONG_CART, 0).expect("load fixture cart");
    session
}

#[test]
fn rpc_music_score_mirrors_the_cli() {
    let mut session = loaded_session();
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "music_score", "params": {}}),
    );
    assert!(resp.get("error").is_none(), "{resp}");
    let text = resp["result"]["text"].as_str().expect("text");
    assert_eq!(text, music::score::score(&cart(SONG_CART), None).unwrap());

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 2, "method": "music_score", "params": {"song": 1}}),
    );
    assert!(
        resp["result"]["text"]
            .as_str()
            .expect("text")
            .contains("song:  music(1)"),
        "{resp}"
    );

    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 3, "method": "music_score", "params": {"song": 9}}),
    );
    assert_eq!(resp["error"]["code"], -32602, "{resp}");
}

#[test]
fn rpc_music_lint_mirrors_the_cli() {
    let mut session = loaded_session();
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "music_lint", "params": {}}),
    );
    assert!(resp.get("error").is_none(), "{resp}");
    assert_eq!(resp["result"], music::lint::lint(&cart(SONG_CART)));
}

#[test]
fn rpc_music_piano_roll_writes_a_png() {
    let mut session = loaded_session();
    let path = temp_path("rpc.png");
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 1, "method": "music_piano_roll",
               "params": {"path": path.to_str().expect("utf-8"), "cell": 3}}),
    );
    assert!(resp.get("error").is_none(), "{resp}");
    assert_eq!(resp["result"]["ok"], true);
    assert_eq!(resp["result"]["width"], music::roll::GUTTER + 32 * 3);
    assert_eq!(resp["result"]["patterns"], json!([0, 1, 2]));
    assert_eq!(resp["result"]["loop_pattern"], 1);

    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&path).expect("png file exists"),
    ));
    let reader = decoder.read_info().expect("valid png header");
    assert_eq!(reader.info().width, music::roll::GUTTER + 32 * 3);

    // A missing path is a params error, not a panic.
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 2, "method": "music_piano_roll", "params": {}}),
    );
    assert_eq!(resp["error"]["code"], -32602, "{resp}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn rpc_music_verbs_need_a_cart() {
    let mut session = Session::new();
    for method in ["music_score", "music_lint"] {
        let resp = handle(
            &mut session,
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": {}}),
        );
        assert_eq!(resp["error"]["code"], -32002, "{method}: {resp}");
    }
}

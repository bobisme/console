//! Integration tests for `console-agent music edit` — the six score-level
//! `__sfx__` transforms (SPEC.md "Music authoring (PoC v2)" > "Transforms"),
//! the music sibling of `sprite_edit.rs` / `map_edit.rs`.
//!
//! Every verb is asserted the same way: run the transform against inline cart
//! text, **re-parse the result with `Cart::parse`**, and read the notes back
//! out of the parsed bank. A transform that produces text the console cannot
//! load is the failure mode that matters, so the round trip is the test.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::music::sfxtext::EditResult;
use console_agent::music::transform::{EditArgs, cli_edit, run_edit};
use console_core::{Cart, Sfx, SfxRow};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Two sfx: a four-row melody on a named instrument and a four-row bass with
/// a `loop=` range, so the loop-range adjustment and the instrument column
/// both get exercised. The columns are hand-aligned with double spaces —
/// which is exactly what the token-surgery rewrite has to preserve.
const CART: &str = "\
__lua__
function _init() music(0) end

__instruments__
inst lead wave=2 env=1,4,3
inst bass wave=3

__sfx__
# the melody
sfx 0 speed=8
C4  lead 5
E4  lead 5
---
G4  lead 5 sl+2

sfx 1 speed=6 loop=0,3
C2  bass 6
---
G2  bass 6
---

__music__
pat 0 loop=0 : 0 1 - -
";

fn temp_cart(tag: &str, text: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "console-agent-music-edit-{}-{n}-{tag}.cart",
        std::process::id()
    ));
    std::fs::write(&path, text).expect("write temp cart");
    path
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read temp cart")
}

fn argv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

/// Run a verb against `text` and return the rewritten cart text.
fn edited(text: &str, args: &[&str]) -> String {
    let parsed = EditArgs::parse(&argv(args)).expect("args parse");
    match run_edit(text, &parsed).expect("edit succeeds") {
        EditResult::Changed { new_text, .. } => new_text,
        EditResult::Unchanged => panic!("expected a change from {args:?}"),
    }
}

fn edit_err(text: &str, args: &[&str]) -> String {
    let parsed = EditArgs::parse(&argv(args)).expect("args parse");
    match run_edit(text, &parsed) {
        Err(e) => e,
        Ok(_) => panic!("expected {args:?} to fail"),
    }
}

/// Re-parse and pull one sfx out — the round trip every test ends with.
fn sfx_of(text: &str, id: u8) -> Sfx {
    Cart::parse(text)
        .expect("the rewritten cart still parses")
        .sfx(id)
        .unwrap_or_else(|| panic!("sfx {id} exists"))
        .clone()
}

fn notes(sfx: &Sfx) -> Vec<Option<u8>> {
    sfx.rows
        .iter()
        .map(|r| match r {
            SfxRow::Rest => None,
            SfxRow::Note { note, .. } => Some(*note),
        })
        .collect()
}

fn vols(sfx: &Sfx) -> Vec<Option<u8>> {
    sfx.rows
        .iter()
        .map(|r| match r {
            SfxRow::Rest => None,
            SfxRow::Note { vol, .. } => Some(*vol),
        })
        .collect()
}

// ---------------------------------------------------------------------
// transpose
// ---------------------------------------------------------------------

#[test]
fn transpose_shifts_every_note_and_round_trips() {
    let out = edited(CART, &["c", "transpose", "0", "+2"]);
    // C4=48, E4=52, G4=55 -> D4=50, F#4=54, A4=57.
    assert_eq!(
        notes(&sfx_of(&out, 0)),
        vec![Some(50), Some(54), None, Some(57)]
    );
    // sfx 1 untouched.
    assert_eq!(notes(&sfx_of(&out, 1)), notes(&sfx_of(CART, 1)));
}

#[test]
fn transpose_accepts_a_range_and_a_comma_list() {
    let out = edited(CART, &["c", "transpose", "0-1", "-12"]);
    assert_eq!(
        notes(&sfx_of(&out, 0)),
        vec![Some(36), Some(40), None, Some(43)]
    );
    assert_eq!(
        notes(&sfx_of(&out, 1)),
        vec![Some(12), None, Some(19), None]
    );

    let out = edited(CART, &["c", "transpose", "0,1", "-12"]);
    assert_eq!(
        notes(&sfx_of(&out, 0)),
        vec![Some(36), Some(40), None, Some(43)]
    );
}

#[test]
fn transpose_preserves_the_effect_column_and_the_alignment() {
    let out = edited(CART, &["c", "transpose", "0", "+2"]);
    // The fx token rides along untouched and the vol column stays put.
    assert!(out.contains("A4  lead 5 sl+2"), "{out}");
    // The `#` comment line and the rest survive verbatim.
    assert!(out.contains("# the melody"), "{out}");
    assert!(out.contains("\n---\n"), "{out}");
}

#[test]
fn transpose_off_the_note_table_names_the_row_and_suggests_a_shift() {
    let e = edit_err(CART, &["c", "transpose", "0", "-60"]);
    assert!(e.contains("sfx 0 row 0: C4 -60"), "{e}");
    assert!(e.contains("leaves the note table (C0-B7)"), "{e}");
    // C4=48 is the lowest selected note, G4=55 the highest.
    assert!(e.contains("-48..=+40"), "{e}");
    assert!(e.contains("nearest to -60 is -48"), "{e}");
}

#[test]
fn transpose_clamp_pins_to_the_note_table() {
    let out = edited(CART, &["c", "transpose", "0", "-60", "--clamp"]);
    assert_eq!(
        notes(&sfx_of(&out, 0)),
        vec![Some(0), Some(0), None, Some(0)]
    );
}

// ---------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------

#[test]
fn copy_duplicates_an_sfx_under_a_new_id() {
    let out = edited(CART, &["c", "copy", "0", "5"]);
    let src = sfx_of(&out, 0);
    let dst = sfx_of(&out, 5);
    assert_eq!(src, dst);
    assert_eq!(dst.speed, 8);
    // Inserted in ascending id order, inside `__sfx__`, before `__music__`.
    let sfx_section = out.find("__sfx__").unwrap();
    let music_section = out.find("__music__").unwrap();
    let new_block = out.find("sfx 5 speed=8").unwrap();
    assert!(
        sfx_section < new_block && new_block < music_section,
        "{out}"
    );
}

#[test]
fn copy_refuses_an_occupied_id_without_force() {
    let e = edit_err(CART, &["c", "copy", "0", "1"]);
    assert!(e.contains("sfx 1 already exists"), "{e}");
    assert!(e.contains("--force"), "{e}");

    let out = edited(CART, &["c", "copy", "0", "1", "--force"]);
    assert_eq!(sfx_of(&out, 1), sfx_of(&out, 0));
    // Overwriting a 4-row sfx with a 4-row sfx leaves the pattern intact.
    assert!(Cart::parse(&out).unwrap().pattern(0).is_some());
}

#[test]
fn copy_into_a_shorter_and_a_longer_target_both_round_trip() {
    // sfx 1 has a `loop=0,3`; copying the 4-row sfx 0 over it keeps 4 rows.
    let out = edited(CART, &["c", "copy", "0", "1", "--force"]);
    assert_eq!(sfx_of(&out, 1).rows.len(), 4);

    // Fewer rows than the target: the surplus lines are deleted.
    let short = edited(CART, &["c", "stretch", "0", "0.5", "--force"]);
    let out = edited(&short, &["c", "copy", "0", "1", "--force"]);
    assert_eq!(sfx_of(&out, 1).rows.len(), 2);
    assert_eq!(sfx_of(&out, 1), sfx_of(&out, 0));

    // More rows than the target: the shortfall is inserted after the last
    // surviving row, and the following entry is untouched.
    let long = edited(CART, &["c", "stretch", "0", "2"]);
    let out = edited(&long, &["c", "copy", "0", "1", "--force"]);
    assert_eq!(sfx_of(&out, 1).rows.len(), 8);
    assert_eq!(sfx_of(&out, 1), sfx_of(&out, 0));
    assert!(Cart::parse(&out).unwrap().pattern(0).is_some());
}

// ---------------------------------------------------------------------
// shift-rows
// ---------------------------------------------------------------------

#[test]
fn shift_rows_rotates_and_wraps() {
    let before = notes(&sfx_of(CART, 0));
    let out = edited(CART, &["c", "shift-rows", "0", "1"]);
    // Row i now plays what row i-1 played; the last row wraps to the front.
    assert_eq!(
        notes(&sfx_of(&out, 0)),
        vec![before[3], before[0], before[1], before[2]]
    );

    // Negative rotates the other way, and +len is the identity.
    let back = edited(&out, &["c", "shift-rows", "0", "-1"]);
    assert_eq!(notes(&sfx_of(&back, 0)), before);
}

#[test]
fn shift_rows_by_a_whole_turn_is_a_no_op() {
    let parsed = EditArgs::parse(&argv(&["c", "shift-rows", "0", "4"])).unwrap();
    assert!(matches!(
        run_edit(CART, &parsed).unwrap(),
        EditResult::Unchanged
    ));
}

// ---------------------------------------------------------------------
// set-vol
// ---------------------------------------------------------------------

#[test]
fn set_vol_absolute_and_relative_clamp_and_keep_rests() {
    let out = edited(CART, &["c", "set-vol", "0", "2"]);
    assert_eq!(
        vols(&sfx_of(&out, 0)),
        vec![Some(2), Some(2), None, Some(2)]
    );

    let out = edited(CART, &["c", "set-vol", "0", "+9"]);
    assert_eq!(
        vols(&sfx_of(&out, 0)),
        vec![Some(7), Some(7), None, Some(7)]
    );

    let out = edited(CART, &["c", "set-vol", "0", "-9"]);
    assert_eq!(
        vols(&sfx_of(&out, 0)),
        vec![Some(0), Some(0), None, Some(0)]
    );
    // The rest is still a rest, not a vol-0 note.
    assert!(matches!(sfx_of(&out, 0).rows[2], SfxRow::Rest));
}

#[test]
fn set_vol_rejects_an_out_of_range_absolute() {
    let e = edit_err(CART, &["c", "set-vol", "0", "9"]);
    assert!(e.contains("bad volume 9"), "{e}");
}

// ---------------------------------------------------------------------
// set-inst
// ---------------------------------------------------------------------

#[test]
fn set_inst_reassigns_the_voice_column() {
    let out = edited(CART, &["c", "set-inst", "0", "bass"]);
    let cart = Cart::parse(&out).unwrap();
    let sfx = cart.sfx(0).unwrap();
    for row in 0..sfx.rows.len() {
        if let SfxRow::Note { wave, .. } = sfx.rows[row] {
            // `bass` is wave=3.
            assert_eq!(wave, 3, "row {row}");
        }
    }
}

#[test]
fn set_inst_where_only_touches_matching_rows() {
    // Give sfx 0 a mixed voice column first.
    let mixed = CART.replace("E4  lead 5", "E4  0    5");
    let out = edited(&mixed, &["c", "set-inst", "0", "bass", "--where", "0"]);
    let cart = Cart::parse(&out).unwrap();
    let waves: Vec<u8> = cart
        .sfx(0)
        .unwrap()
        .rows
        .iter()
        .filter_map(|r| match r {
            SfxRow::Note { wave, .. } => Some(*wave),
            SfxRow::Rest => None,
        })
        .collect();
    // Only the bare-`0` row became `bass` (wave 3); the `lead` rows (wave 2)
    // are untouched.
    assert_eq!(waves, vec![2, 3, 2]);
}

#[test]
fn set_inst_rejects_an_unknown_voice_and_an_unmatched_where() {
    let e = edit_err(CART, &["c", "set-inst", "0", "nope"]);
    assert!(e.contains("unknown voice"), "{e}");
    assert!(e.contains("lead, bass"), "{e}");

    let e = edit_err(CART, &["c", "set-inst", "0", "bass", "--where", "3"]);
    assert!(e.contains("no row of sfx 0 uses that voice"), "{e}");
    assert!(e.contains("lead"), "{e}");
}

// ---------------------------------------------------------------------
// stretch
// ---------------------------------------------------------------------

#[test]
fn stretch_double_inserts_rests_and_halves_the_speed() {
    let out = edited(CART, &["c", "stretch", "0", "2"]);
    let sfx = sfx_of(&out, 0);
    assert_eq!(sfx.rows.len(), 8);
    assert_eq!(sfx.speed, 4);
    // Wall clock preserved exactly: 4 x 8 == 8 x 4.
    assert_eq!(sfx.duration(), 32);
    assert_eq!(
        notes(&sfx),
        vec![Some(48), None, Some(52), None, None, None, Some(55), None]
    );
}

#[test]
fn stretch_halve_drops_odd_rows_and_doubles_the_speed() {
    // sfx 1's odd rows are rests, so halving is lossless.
    let out = edited(CART, &["c", "stretch", "1", "0.5"]);
    let sfx = sfx_of(&out, 1);
    assert_eq!(sfx.rows.len(), 2);
    assert_eq!(sfx.speed, 12);
    assert_eq!(sfx.duration(), 24);
    assert_eq!(notes(&sfx), vec![Some(24), Some(31)]);
    // The `loop=0,3` range followed the rows down to `loop=0,1`.
    assert_eq!(sfx.loop_range, Some((0, 1)));
}

#[test]
fn stretch_halve_refuses_to_drop_notes_without_force() {
    // sfx 0's row 1 and row 3 carry notes.
    let e = edit_err(CART, &["c", "stretch", "0", "0.5"]);
    assert!(
        e.contains("would drop 2 odd row(s) that carry notes"),
        "{e}"
    );
    assert!(e.contains("row(s) 1, 3"), "{e}");

    let out = edited(CART, &["c", "stretch", "0", "0.5", "--force"]);
    let sfx = sfx_of(&out, 0);
    assert_eq!(notes(&sfx), vec![Some(48), None]);
    assert_eq!(sfx.speed, 16);
}

#[test]
fn stretch_double_adjusts_a_loop_range_to_cover_the_same_music() {
    let out = edited(CART, &["c", "stretch", "1", "2"]);
    let sfx = sfx_of(&out, 1);
    assert_eq!(sfx.rows.len(), 8);
    // `loop=0,3` covered all four rows; doubled it covers all eight.
    assert_eq!(sfx.loop_range, Some((0, 7)));
    assert_eq!(sfx.speed, 3);
    assert_eq!(sfx.duration(), 24);
}

#[test]
fn stretch_double_refuses_to_exceed_the_row_cap() {
    let mut rows = String::from("__lua__\nfunction _init() end\n\n__sfx__\nsfx 0 speed=4\n");
    for _ in 0..17 {
        rows.push_str("C4 2 5\n");
    }
    let e = edit_err(&rows, &["c", "stretch", "0", "2"]);
    assert!(e.contains("doubling would need 34"), "{e}");
    assert!(e.contains("maximum is 32"), "{e}");
}

#[test]
fn stretch_reports_the_rounding_when_the_speed_is_odd() {
    let text = CART.replace("sfx 0 speed=8", "sfx 0 speed=5");
    let parsed = EditArgs::parse(&argv(&["c", "stretch", "0", "2"])).unwrap();
    let EditResult::Changed {
        summary, new_text, ..
    } = run_edit(&text, &parsed).unwrap()
    else {
        panic!("expected a change");
    };
    // 5 frames per row cannot halve exactly: round half up to 3.
    assert_eq!(sfx_of(&new_text, 0).speed, 3);
    assert!(
        summary.iter().any(|l| l.contains("length 20 -> 24 frames")
            && l.contains("+4 frames from integer speed rounding")),
        "{summary:?}"
    );
}

#[test]
fn stretch_reports_when_speed_1_cannot_be_halved() {
    let text = CART.replace("sfx 0 speed=8", "sfx 0 speed=1");
    let parsed = EditArgs::parse(&argv(&["c", "stretch", "0", "2"])).unwrap();
    let EditResult::Changed {
        summary, new_text, ..
    } = run_edit(&text, &parsed).unwrap()
    else {
        panic!("expected a change");
    };
    assert_eq!(sfx_of(&new_text, 0).speed, 1);
    assert!(
        summary
            .iter()
            .any(|l| l.contains("speed=1 cannot be halved")),
        "{summary:?}"
    );
}

// ---------------------------------------------------------------------
// CLI plumbing: files, --dry-run, exit codes
// ---------------------------------------------------------------------

#[test]
fn dry_run_leaves_the_file_alone() {
    let path = temp_cart("dry", CART);
    let code = cli_edit(&argv(&[
        path.to_str().unwrap(),
        "transpose",
        "0",
        "+2",
        "--dry-run",
    ]));
    assert_eq!(code, 0);
    assert_eq!(read(&path), CART);
}

#[test]
fn a_real_run_writes_only_the_sfx_lines() {
    let path = temp_cart("write", CART);
    let code = cli_edit(&argv(&[path.to_str().unwrap(), "transpose", "0", "+2"]));
    assert_eq!(code, 0);
    let after = read(&path);
    assert_ne!(after, CART);

    // Every line outside the three changed note rows is byte-identical.
    let changed: Vec<(usize, (&str, &str))> = CART
        .lines()
        .zip(after.lines())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .collect();
    assert_eq!(changed.len(), 3, "{changed:?}");
    assert_eq!(CART.lines().count(), after.lines().count());
}

#[test]
fn unknown_verbs_and_missing_ids_exit_two() {
    let path = temp_cart("bad", CART);
    assert_eq!(
        cli_edit(&argv(&[path.to_str().unwrap(), "reverse", "0"])),
        2
    );
    assert_eq!(
        cli_edit(&argv(&[path.to_str().unwrap(), "transpose", "9", "+1"])),
        2
    );
    assert_eq!(read(&path), CART);
}

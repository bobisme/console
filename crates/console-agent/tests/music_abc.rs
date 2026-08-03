//! Integration tests for `console-agent music import-abc` — SPEC.md "Music
//! authoring (PoC v2)" > "ABC import".
//!
//! The fixtures are four public-domain tunes written inline, each chosen for
//! the ABC feature it stresses:
//!
//! - [`SCALE`] — a D major scale exercise: key-signature accidentals and the
//!   simplest possible length grid.
//! - [`BUTTERFLY`] — a fragment of the Irish slip jig *The Butterfly*: 9/8,
//!   `K:Em`, a repeat, a tie across a bar line and multi-unit notes.
//! - [`SIXTEENTHS`] — a made-up étude in 16ths: explicit `^ _ =` accidentals
//!   with bar-local memory, octave marks in both directions, a `z` rest, a
//!   broken rhythm and a length grid finer than `L:`.
//! - [`ODE`] — the *Ode to Joy* opening: the tune everyone can check by ear,
//!   with a dotted-eighth/sixteenth figure and a tie.
//!
//! Pitches are pinned **note by note** against console note indices (C4 = 48),
//! because the whole value of the importer is that its pitches are right.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use console_agent::music::abc::{
    AbcEvent, Frac, ImportOpts, PlannedRow, parse_abc, plan_import, run_import,
};
use console_agent::music::sfxtext::EditResult;
use console_core::{Cart, SfxRow};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

const SCALE: &str = "\
X:1
T:D Major Scale Exercise
M:4/4
L:1/8
Q:1/4=120
K:D
DEFG ABcd|dcBA GFED|
";

/// *The Butterfly* (traditional Irish slip jig, public domain), first strain.
const BUTTERFLY: &str = "\
X:1
T:The Butterfly
R:slip jig
M:9/8
L:1/8
Q:3/8=100
K:Em
|:B2E G2E FED|B2E G2E FED|B2d e2f g3-|g3 gfe dBA:|
";

const SIXTEENTHS: &str = "\
X:1
T:Sixteenths
M:2/4
L:1/16
Q:1/4=90
K:F
% explicit accidentals, both octave directions, a rest and a broken rhythm
_B,2 =B,2 ^c2 c'2 | z4 A>G FE |
";

/// *Ode to Joy* (Beethoven, public domain), the opening eight bars.
const ODE: &str = "\
X:1
T:Ode to Joy
M:4/4
L:1/4
Q:1/4=120
K:C
BB c d|d c B A|G G A B|B3/2A/2A2|
";

fn temp_cart(tag: &str, text: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "console-agent-music-abc-{}-{n}-{tag}.cart",
        std::process::id()
    ));
    std::fs::write(&path, text).expect("write temp cart");
    path
}

const EMPTY_CART: &str = "__lua__\nfunction _init() end\n";

fn opts(sfx: u8) -> ImportOpts {
    ImportOpts {
        sfx_start: sfx,
        ..ImportOpts::default()
    }
}

/// The pitches a tune's events carry, as console note indices.
fn pitches(abc: &str) -> Vec<i32> {
    parse_abc(abc)
        .expect("tune parses")
        .events
        .iter()
        .filter_map(|e| match e {
            AbcEvent::Note { note, .. } => Some(*note),
            AbcEvent::Rest { .. } => None,
        })
        .collect()
}

/// Every event's duration, in whole notes.
fn durations(abc: &str) -> Vec<Frac> {
    parse_abc(abc)
        .expect("tune parses")
        .events
        .iter()
        .map(AbcEvent::dur)
        .collect()
}

fn imported(cart: &str, abc: &str, opts: &ImportOpts) -> String {
    match run_import(cart, abc, opts).expect("import succeeds") {
        EditResult::Changed { new_text, .. } => new_text,
        EditResult::Unchanged => panic!("expected a change"),
    }
}

/// The note index of every row of an sfx in a re-parsed cart (`None` = rest).
fn rows_of(text: &str, id: u8) -> Vec<Option<u8>> {
    Cart::parse(text)
        .expect("the rewritten cart parses")
        .sfx(id)
        .unwrap_or_else(|| panic!("sfx {id} exists"))
        .rows
        .iter()
        .map(|r| match r {
            SfxRow::Rest => None,
            SfxRow::Note { note, .. } => Some(*note),
        })
        .collect()
}

// ---------------------------------------------------------------------
// Pitch and key signature
// ---------------------------------------------------------------------

#[test]
fn scale_pitches_apply_the_key_signature() {
    // D E F# G A B C#5 D5 then back down. C4 = 48.
    let up = vec![50, 52, 54, 55, 57, 59, 61, 62];
    let mut want = up.clone();
    want.extend(up.iter().rev().copied());
    assert_eq!(pitches(SCALE), want);

    let tune = parse_abc(SCALE).unwrap();
    assert_eq!(tune.key_text, "D major (C#, F#)");
    assert_eq!(tune.meter_text, "4/4");
    assert_eq!(tune.unit, Frac::new(1, 8));
    assert_eq!(tune.tempo, Some((Frac::new(1, 4), 120)));
    assert!(tune.warnings.is_empty(), "{:?}", tune.warnings);
}

#[test]
fn octave_marks_and_explicit_accidentals_beat_the_key() {
    // K:F puts B flat. `_B,` = Bb3 = 46, `=B,` naturals it to B3 = 47,
    // `^c` = C#5 = 61, `c'` = C6 = 72 (the bar's `=` on B does not touch C).
    // Then, after the bar line, A4 = 57, G4 = 55, F4 = 53, E4 = 52.
    assert_eq!(pitches(SIXTEENTHS), vec![46, 47, 61, 72, 57, 55, 53, 52]);
    assert_eq!(parse_abc(SIXTEENTHS).unwrap().key_text, "F major (Bb)");
}

#[test]
fn a_bar_local_accidental_expires_at_the_bar_line() {
    // `^F` sharpens F4 for the rest of bar 1 only.
    let abc = "X:1\nL:1/4\nM:4/4\nK:C\n^F F F F|F F F F|\n";
    let p = pitches(abc);
    assert_eq!(p, vec![54, 54, 54, 54, 53, 53, 53, 53]);
}

#[test]
fn modes_resolve_through_the_circle_of_fifths() {
    // A dorian carries one sharp (F#), so the F is 54 not 53.
    let abc = "X:1\nL:1/4\nK:Ador\nA B c d|e f g a|\n";
    assert_eq!(pitches(abc), vec![57, 59, 60, 62, 64, 66, 67, 69]);
    assert!(parse_abc(abc).unwrap().key_text.starts_with("A dorian"));
}

// ---------------------------------------------------------------------
// Lengths, ties and broken rhythm
// ---------------------------------------------------------------------

#[test]
fn butterfly_lengths_merge_the_tie_across_the_bar_line() {
    let d = durations(BUTTERFLY);
    let eighth = Frac::new(1, 8);
    // Bar 1: B2 E G2 E F E D = 2,1,2,1,1,1,1 eighths.
    assert_eq!(
        &d[..7],
        &[
            (eighth * Frac::new(2, 1)),
            eighth,
            (eighth * Frac::new(2, 1)),
            eighth,
            eighth,
            eighth,
            eighth
        ]
    );
    // `g3-|g3` is one six-eighth note, not two of three.
    let tied = d.iter().find(|f| **f == Frac::new(6, 8)).copied();
    assert_eq!(tied, Some(Frac::new(3, 4)));
    // 25 events for 36 eighths of music (4 bars of 9/8).
    assert_eq!(d.len(), 25);
    let total = d.iter().fold(Frac::new(0, 1), |a, b| a + *b);
    assert_eq!(total, Frac::new(36, 8));
}

#[test]
fn broken_rhythm_dots_the_note_before_and_halves_the_note_after() {
    let d = durations(SIXTEENTHS);
    // `A>G` in L:1/16: A becomes 3/32, G becomes 1/32.
    assert_eq!(d[5], Frac::new(3, 32));
    assert_eq!(d[6], Frac::new(1, 32));
    // `<` is the mirror image.
    let back = durations("X:1\nL:1/8\nK:C\nA<G|\n");
    assert_eq!(back, vec![Frac::new(1, 16), Frac::new(3, 16)]);
}

#[test]
fn ode_to_joy_maps_note_for_note() {
    // B4 B4 C5 D5 | D5 C5 B4 A4 | G4 G4 A4 B4 | B4(dotted) A4(16th) A4(half)
    assert_eq!(
        pitches(ODE),
        vec![59, 59, 60, 62, 62, 60, 59, 57, 55, 55, 57, 59, 59, 57, 57]
    );
    let d = durations(ODE);
    // `B3/2A/2A2` at L:1/4 is a dotted eighth, a sixteenth and a half note.
    assert_eq!(
        &d[12..],
        &[Frac::new(3, 8), Frac::new(1, 8), Frac::new(1, 2)]
    );
}

// ---------------------------------------------------------------------
// Row grid, speed and the written cart
// ---------------------------------------------------------------------

#[test]
fn the_row_grid_is_the_gcd_of_the_note_lengths() {
    let tune = parse_abc(ODE).unwrap();
    let cart = Cart::parse(EMPTY_CART).unwrap();
    let plan = plan_import(&tune, &cart, &opts(0)).unwrap();
    // Bar 4's dotted eighth (3/8) and sixteenth (1/8 of a whole, since
    // `L:1/4`) pull the grid down from 1/4 to their gcd, 1/8 — and *not* all
    // the way to the shortest length, which is what makes gcd the right rule.
    assert_eq!(plan.row_dur, Frac::new(1, 8));
    // 4 bars of 4/4 = 4 whole notes = 32 eighths.
    assert_eq!(plan.rows.len(), 32);
    // Q:1/4=120 -> a quarter is 30 frames, an eighth exactly 15.
    assert_eq!(plan.speed, 15);
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    // The quarter notes become two rows each and the half note four.
    assert_eq!(
        &plan.rows[..4],
        &[
            PlannedRow::Note(59),
            PlannedRow::Note(59),
            PlannedRow::Note(59),
            PlannedRow::Note(59)
        ]
    );
    assert_eq!(
        &plan.rows[28..],
        &[
            PlannedRow::Note(57),
            PlannedRow::Note(57),
            PlannedRow::Note(57),
            PlannedRow::Note(57)
        ]
    );
}

#[test]
fn a_row_rate_that_is_not_a_whole_frame_rounds_and_says_so() {
    // `speed=` is a whole number of frames, so most tempos cannot be hit
    // exactly. A 1/16 grid at quarter = 140 wants 6.43 frames per row; the
    // report has to admit the rounding rather than quietly retune the tune.
    let tune = parse_abc("X:1\nM:4/4\nL:1/16\nQ:1/4=140\nK:C\nCDEF GABc|\n").unwrap();
    let cart = Cart::parse(EMPTY_CART).unwrap();
    let plan = plan_import(&tune, &cart, &opts(0)).unwrap();
    assert_eq!(plan.row_dur, Frac::new(1, 16));
    assert_eq!(plan.speed, 6);
    assert!(
        plan.warnings.iter().any(|w| w.contains("rounded")),
        "{:?}",
        plan.warnings
    );
    // Explicit --speed silences the guesswork entirely.
    let plan = plan_import(
        &tune,
        &cart,
        &ImportOpts {
            speed: Some(6),
            ..opts(0)
        },
    )
    .unwrap();
    assert_eq!(plan.speed_source, "--speed");
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
}

#[test]
fn a_held_note_becomes_repeated_rows() {
    let tune = parse_abc(SCALE).unwrap();
    let cart = Cart::parse(EMPTY_CART).unwrap();
    let plan = plan_import(&tune, &cart, &opts(0)).unwrap();
    // Every note is one eighth, so 16 notes = 16 rows, no repeats.
    assert_eq!(plan.row_dur, Frac::new(1, 8));
    assert_eq!(plan.rows.len(), 16);
    assert_eq!(plan.rows[0], PlannedRow::Note(50));
    // 1/8 at quarter = 120 is exactly 15 frames.
    assert_eq!(plan.speed, 15);

    // The Butterfly's `B2` is two rows of the same note.
    let tune = parse_abc(BUTTERFLY).unwrap();
    let plan = plan_import(&tune, &cart, &opts(0)).unwrap();
    assert_eq!(plan.rows[0], PlannedRow::Note(59));
    assert_eq!(plan.rows[1], PlannedRow::Note(59));
    assert_eq!(plan.rows[2], PlannedRow::Note(52));
}

#[test]
fn a_rest_becomes_rest_rows() {
    let out = imported(EMPTY_CART, SIXTEENTHS, &opts(0));
    let rows = rows_of(&out, 0);
    // `z4` at L:1/16 on a 1/32 grid is eight rest rows.
    assert_eq!(&rows[16..24], &[None; 8]);
    assert_eq!(rows[0], Some(46));
    assert_eq!(rows.len(), 32);
}

#[test]
fn importing_creates_the_sfx_section_and_the_cart_round_trips() {
    let out = imported(EMPTY_CART, SCALE, &opts(0));
    assert!(out.contains("__sfx__"), "{out}");
    let cart = Cart::parse(&out).expect("parses");
    let sfx = cart.sfx(0).unwrap();
    assert_eq!(sfx.speed, 15);
    assert_eq!(sfx.rows.len(), 16);
    assert_eq!(
        rows_of(&out, 0),
        vec![
            Some(50),
            Some(52),
            Some(54),
            Some(55),
            Some(57),
            Some(59),
            Some(61),
            Some(62),
            Some(62),
            Some(61),
            Some(59),
            Some(57),
            Some(55),
            Some(54),
            Some(52),
            Some(50)
        ]
    );
}

#[test]
fn a_long_tune_splits_into_consecutive_sfx_ids() {
    let tune = parse_abc(BUTTERFLY).unwrap();
    let cart = Cart::parse(EMPTY_CART).unwrap();
    let plan = plan_import(&tune, &cart, &opts(2)).unwrap();
    // 36 eighth-note rows: 32 + 4.
    assert_eq!(plan.rows.len(), 36);
    assert_eq!(plan.chunks, vec![(2, 32), (3, 4)]);

    let out = imported(EMPTY_CART, BUTTERFLY, &opts(2));
    assert_eq!(rows_of(&out, 2).len(), 32);
    assert_eq!(rows_of(&out, 3).len(), 4);
    // The split is a plain row boundary: row 32 of the tune opens sfx 3.
    assert_eq!(rows_of(&out, 3)[0], Some(64));

    let summary = plan.summary(&tune, &opts(2));
    assert!(
        summary.iter().any(|l| l.contains("split at row 32")),
        "{summary:?}"
    );
    // One suggested pattern per sfx; they chain by "next existing id".
    assert_eq!(
        plan.pattern_hint,
        vec![
            "pat 0 : 2 - - -".to_string(),
            "pat 1 stop : 3 - - -".to_string()
        ]
    );
}

#[test]
fn the_summary_reports_the_tempo_mapping() {
    let tune = parse_abc(BUTTERFLY).unwrap();
    let cart = Cart::parse(EMPTY_CART).unwrap();
    let plan = plan_import(&tune, &cart, &opts(0)).unwrap();
    assert_eq!(plan.speed, 12);
    // Q:3/8=100 is a dotted-quarter pulse; as quarters that is 150 bpm, and
    // a 1/8 row is 2 rows per beat, which `speed=auto` resolves to 12.
    assert_eq!(
        plan.tempo_hint.as_deref(),
        Some("bpm=150 rows_per_beat=2 (speed=auto then resolves to 12)")
    );
    let summary = plan.summary(&tune, &opts(0));
    assert!(
        summary
            .iter()
            .any(|l| l.contains("1 row = 1/8 note") && l.contains("speed=12")),
        "{summary:?}"
    );
}

// ---------------------------------------------------------------------
// Warnings and errors
// ---------------------------------------------------------------------

#[test]
fn a_repeat_is_unrolled_once_with_a_warning() {
    let tune = parse_abc(BUTTERFLY).unwrap();
    // 4 bars of 9/8 = 36 eighths, i.e. the repeated span played ONCE.
    let total = tune.events.iter().fold(Frac::new(0, 1), |a, e| a + e.dur());
    assert_eq!(total, Frac::new(36, 8));
    assert!(
        tune.warnings.iter().any(|w| w.contains("unrolled once")),
        "{:?}",
        tune.warnings
    );
}

#[test]
fn a_chord_keeps_its_first_note_with_a_warning() {
    let abc = "X:1\nL:1/4\nM:4/4\nK:G\n[GBd]2 A z F|\n";
    let tune = parse_abc(abc).unwrap();
    assert_eq!(pitches(abc), vec![55, 57, 54]);
    assert_eq!(tune.events[0].dur(), Frac::new(1, 2));
    assert!(
        tune.warnings
            .iter()
            .any(|w| w.contains("reduced to its first note")),
        "{:?}",
        tune.warnings
    );
}

#[test]
fn only_the_first_voice_is_imported() {
    let abc = "X:1\nL:1/4\nK:C\nV:1\nCDEF|\nV:2\nGABc|\n";
    assert_eq!(pitches(abc), vec![48, 50, 52, 53]);
    assert!(
        parse_abc(abc)
            .unwrap()
            .warnings
            .iter()
            .any(|w| w.contains("more than one voice")),
        "expected a multi-voice warning"
    );
}

#[test]
fn out_of_range_notes_name_the_token_and_suggest_a_transpose() {
    let abc = "X:1\nL:1/4\nK:C\nC,,,,,G,,,,,c|\n";
    let e = run_import(EMPTY_CART, abc, &opts(0)).unwrap_err();
    assert!(e.contains("\"C,,,,,\""), "{e}");
    assert!(e.contains("outside the console's C0-B7 note table"), "{e}");
    // C,,,,, = -12 and c = 60, so any shift in +12..=+35 fits.
    assert!(e.contains("+12..=+35"), "{e}");
    assert!(e.contains("--transpose 12"), "{e}");

    // Taking the advice works.
    let with = ImportOpts {
        transpose: 12,
        ..opts(0)
    };
    let out = imported(EMPTY_CART, abc, &with);
    assert_eq!(rows_of(&out, 0), vec![Some(0), Some(7), Some(72)]);
}

#[test]
fn an_impossible_range_says_so_instead_of_suggesting_a_shift() {
    // Nine octaves apart: nothing fits.
    let abc = "X:1\nL:1/4\nK:C\nC,,,,,c''''|\n";
    let e = run_import(EMPTY_CART, abc, &opts(0)).unwrap_err();
    assert!(e.contains("wider than the 96-semitone note table"), "{e}");
}

#[test]
fn tuplets_and_overlays_are_rejected_by_name() {
    let e = run_import(EMPTY_CART, "X:1\nK:C\n(3abc|\n", &opts(0)).unwrap_err();
    assert!(e.contains("tuplet `(3` is not supported"), "{e}");

    let e = run_import(EMPTY_CART, "X:1\nK:C\nab&cd|\n", &opts(0)).unwrap_err();
    assert!(e.contains("voice overlay"), "{e}");
}

#[test]
fn an_occupied_sfx_id_needs_force() {
    let existing = "__lua__\nfunction _init() end\n\n__sfx__\nsfx 0 speed=4\nC4 2 5\n";
    let e = run_import(existing, SCALE, &opts(0)).unwrap_err();
    assert!(e.contains("sfx 0 already exists"), "{e}");

    let forced = ImportOpts {
        force: true,
        ..opts(0)
    };
    let out = imported(existing, SCALE, &forced);
    assert_eq!(rows_of(&out, 0).len(), 16);
}

#[test]
fn the_instrument_column_is_validated_against_the_cart() {
    let bad = ImportOpts {
        voice: "lead".to_string(),
        ..opts(0)
    };
    let e = run_import(EMPTY_CART, SCALE, &bad).unwrap_err();
    assert!(e.contains("--inst \"lead\""), "{e}");

    let with_inst = "__lua__\nfunction _init() end\n\n__instruments__\ninst lead wave=2\n\n__sfx__\nsfx 9 speed=4\nC4 2 5\n";
    let out = imported(with_inst, SCALE, &bad);
    assert!(out.contains("D4 lead 5"), "{out}");
    // Inserted before the higher existing id, so `__sfx__` stays ordered.
    assert!(
        out.find("sfx 0 ").unwrap() < out.find("sfx 9 ").unwrap(),
        "{out}"
    );
}

// ---------------------------------------------------------------------
// CLI plumbing
// ---------------------------------------------------------------------

#[test]
fn the_cli_writes_the_file_and_dry_run_does_not() {
    use console_agent::music::cli_music;

    let abc_path = temp_cart("tune.abc", SCALE);
    let cart_path = temp_cart("cli", EMPTY_CART);
    let args: Vec<String> = [
        "import-abc",
        cart_path.to_str().unwrap(),
        abc_path.to_str().unwrap(),
        "--sfx",
        "0",
        "--dry-run",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(cli_music(&args), 0);
    assert_eq!(std::fs::read_to_string(&cart_path).unwrap(), EMPTY_CART);

    let args: Vec<String> = args.into_iter().filter(|a| a != "--dry-run").collect();
    assert_eq!(cli_music(&args), 0);
    let after = std::fs::read_to_string(&cart_path).unwrap();
    assert_eq!(rows_of(&after, 0).len(), 16);

    // A missing --sfx is a usage error, not a silent default.
    let args: Vec<String> = [
        "import-abc",
        cart_path.to_str().unwrap(),
        abc_path.to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(cli_music(&args), 2);
}

//! Tests for `sprite lint`'s CI-friendly quality gate (bn-fcw): SPEC.md
//! "Sprite & animation authoring (PoC v1)" — `--max-drift`, `--max-area-var`,
//! `--max-changed`, `--no-unique-colors`, `--summary`, and the `sprite_id`
//! field added to every frame entry.
//!
//! Reuses the same hand-built `dot` cart and worked-out numbers documented
//! in `sprite_view.rs` (dot rect=0,0 size=1x1 anchor=4,7; `dot.wave` is
//! frames 0,1, looped):
//!
//! ```text
//!   frame 0            frame 1
//!   (1,1)=3 (2,1)=3    same three, plus
//!   (1,2)=3            (5,5)=9 (6,5)=9 (6,6)=9
//!   area 3             area 6
//! ```
//!
//! `dot.wave`'s pairs (loop-aware, so 0->1 and 1->0):
//! - 0->1: changed_pixels=3, area_drift_pct=+100%, centroid_drift distance
//!   ~2.95px.
//! - 1->0: changed_pixels=3, area_drift_pct=-50%.
//!
//! Color 9 lives in frame 1 alone (`colors_unique_to_single_frame`).
//!
//! NOTE on bn-2op: this workspace was branched from `main` before bn-2op
//! (`frames_rect=`/explicit `tx:ty` anim frames) landed, so `sprite_id`
//! correctness here is only exercised against the CLASSIC frame form
//! (`AnimDef.frames: Vec<u8>`, index-only). Once bn-2op is merged, a
//! follow-up should add a `sprite_id` case over a `frames_rect=`/`tx:ty`
//! anim to confirm `sprite_id` reports the ACTUAL resolved tile in the new
//! forms too, not just the classic wrap-displacement tile.

use console_agent::rpc::handle;
use console_agent::session::Session;
use console_agent::sprite::view::{self, LintThresholds};
use console_core::Cart;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const CART: &str = "\
__meta__
title=Sprite Lint Threshold Test

__lua__
function _init() end
function _update() end
function _draw() end

__sprites__
000000000000000000000000
033000000330000000000000
030000000300000000000000
000000000000000000055000
000000000000000000000000
000000000000099000000000
000000000000009000000000
000000000000000000000000

__gfx_meta__
sprite dot rect=0,0 size=1x1 anchor=4,7
anim dot.wave frames=0,1 fps=4 loop
anim dot.tri frames=0,1,2 fps=4 loop
anim dot.still frames=0 fps=4
";

fn cart() -> Cart {
    Cart::parse(CART).expect("test cart parses")
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_cart() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "console-sprite-lint-thresholds-{}-{n}.cart",
        std::process::id()
    ));
    std::fs::write(&path, CART).expect("write temp cart");
    path
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn approx(value: &Value, expected: f64) {
    let got = value
        .as_f64()
        .unwrap_or_else(|| panic!("{value} is not a number"));
    assert!(
        (got - expected).abs() < 1e-9,
        "expected {expected}, got {got}"
    );
}

// ---------------------------------------------------------------------------
// No thresholds: unchanged report-only behavior.
// ---------------------------------------------------------------------------

#[test]
fn no_thresholds_means_no_violations_key_and_not_violated() {
    let cart = cart();
    let (value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &LintThresholds::default())
            .expect("lint_gated");
    assert!(!violated);
    assert!(value.get("violations").is_none(), "{value}");
    // Otherwise identical to the plain `lint()` report.
    let plain = view::lint(&cart, &["dot.wave".to_string()]).expect("lint");
    assert_eq!(value["anims"], plain["anims"]);
}

#[test]
fn active_thresholds_that_pass_still_report_an_empty_violations_array() {
    let cart = cart();
    let thresholds = LintThresholds {
        max_drift: Some(100.0),
        ..LintThresholds::default()
    };
    let (value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &thresholds).expect("lint_gated");
    assert!(!violated);
    assert_eq!(value["violations"], json!([]));
}

// ---------------------------------------------------------------------------
// sprite_id
// ---------------------------------------------------------------------------

#[test]
fn sprite_id_reports_the_resolved_tile_for_classic_index_frames() {
    // dot's rect is (0,0), size 1x1: frame i (classic index addressing)
    // resolves to tile (i, 0).
    let cart = cart();
    let (value, _) = view::lint_gated(&cart, &["dot.tri".to_string()], &LintThresholds::default())
        .expect("lint_gated");
    let frames = value["anims"][0]["frames"].as_array().expect("frames");
    assert_eq!(frames.len(), 3);
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f["sprite_frame"], i as u64);
        assert_eq!(f["sprite_id"], json!([i as u64, 0]), "frame {i}");
    }
}

#[test]
fn sprite_id_reports_the_resolved_tile_for_frames_rect_and_explicit_coords() {
    // frames_rect= relocates the frame-0 origin to (2,0); an explicit `1:0`
    // entry ignores frames_rect entirely. sprite_id must follow what renders.
    let cart_text = CART.replace(
        "anim dot.tri frames=0,1,2 fps=4 loop",
        "anim dot.tri frames=0,1:0,1 fps=4 loop frames_rect=2,0",
    );
    let cart = Cart::parse(&cart_text).expect("relocated cart parses");
    let (value, _) = view::lint_gated(&cart, &["dot.tri".to_string()], &LintThresholds::default())
        .expect("lint_gated");
    let frames = value["anims"][0]["frames"].as_array().expect("frames");
    assert_eq!(frames.len(), 3);
    // frame 0: index 0 from the (2,0) origin -> tile (2,0)
    assert_eq!(frames[0]["sprite_id"], json!([2, 0]));
    // frame 1: explicit 1:0 -> tile (1,0), frames_rect ignored
    assert_eq!(frames[1]["sprite_frame"], json!("1:0"));
    assert_eq!(frames[1]["sprite_id"], json!([1, 0]));
    // frame 2: index 1 from the (2,0) origin -> tile (3,0)
    assert_eq!(frames[2]["sprite_id"], json!([3, 0]));
}

// ---------------------------------------------------------------------------
// --max-drift
// ---------------------------------------------------------------------------

#[test]
fn max_drift_violation_names_anim_frame_metric_value_and_limit() {
    let cart = cart();
    let thresholds = LintThresholds {
        max_drift: Some(2.0),
        ..LintThresholds::default()
    };
    let (value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &thresholds).expect("lint_gated");
    assert!(violated);
    let violations = value["violations"].as_array().expect("violations");
    // Only the forward pair (0->1, distance ~2.95) breaks a 2.0px limit; the
    // wrap pair (1->0) has the same distance by symmetry, so both fire.
    assert_eq!(violations.len(), 2, "{violations:?}");
    let v = &violations[0];
    assert_eq!(v["anim"], "dot.wave");
    assert_eq!(v["metric"], "centroid_drift");
    assert_eq!(v["limit"], 2.0);
    approx(&v["value"], 2.95);
    // Named frame is the pair's "to" frame.
    assert_eq!(v["frame"], 1);
}

#[test]
fn max_drift_above_the_actual_drift_does_not_violate() {
    let cart = cart();
    let thresholds = LintThresholds {
        max_drift: Some(10.0),
        ..LintThresholds::default()
    };
    let (_value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &thresholds).expect("lint_gated");
    assert!(!violated);
}

// ---------------------------------------------------------------------------
// --max-area-var
// ---------------------------------------------------------------------------

#[test]
fn max_area_var_violation_uses_absolute_percentage() {
    let cart = cart();
    // Forward pair drifts +100%, wrap pair -50%: a 60% limit catches only
    // the forward one (|-50| = 50 <= 60).
    let thresholds = LintThresholds {
        max_area_var: Some(60.0),
        ..LintThresholds::default()
    };
    let (value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &thresholds).expect("lint_gated");
    assert!(violated);
    let violations = value["violations"].as_array().expect("violations");
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0]["metric"], "area_drift_pct");
    assert_eq!(violations[0]["frame"], 1);
    approx(&violations[0]["value"], 100.0);
    assert_eq!(violations[0]["limit"], 60.0);
}

// ---------------------------------------------------------------------------
// --max-changed
// ---------------------------------------------------------------------------

#[test]
fn max_changed_violation_fires_on_both_wrap_pairs() {
    let cart = cart();
    let thresholds = LintThresholds {
        max_changed: Some(2),
        ..LintThresholds::default()
    };
    let (value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &thresholds).expect("lint_gated");
    assert!(violated);
    let violations = value["violations"].as_array().expect("violations");
    // Both pairs (0->1 and 1->0) change exactly 3 pixels > limit 2.
    assert_eq!(violations.len(), 2, "{violations:?}");
    for v in violations {
        assert_eq!(v["metric"], "changed_pixels");
        assert_eq!(v["value"], 3);
        assert_eq!(v["limit"], 2);
    }
}

#[test]
fn max_changed_at_or_above_the_actual_count_does_not_violate() {
    let cart = cart();
    let thresholds = LintThresholds {
        max_changed: Some(3),
        ..LintThresholds::default()
    };
    let (_value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &thresholds).expect("lint_gated");
    assert!(
        !violated,
        "changed_pixels == limit must not violate (strictly greater only)"
    );
}

// ---------------------------------------------------------------------------
// --no-unique-colors
// ---------------------------------------------------------------------------

#[test]
fn no_unique_colors_flags_the_stray_color_by_anim_and_frame() {
    let cart = cart();
    let thresholds = LintThresholds {
        no_unique_colors: true,
        ..LintThresholds::default()
    };
    let (value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &thresholds).expect("lint_gated");
    assert!(violated);
    let violations = value["violations"].as_array().expect("violations");
    assert_eq!(
        violations,
        &vec![
            json!({"anim": "dot.wave", "frame": 1, "metric": "unique_color", "value": 9, "limit": 0})
        ]
    );
}

#[test]
fn no_unique_colors_off_by_default_is_not_a_violation() {
    let cart = cart();
    let (_value, violated) =
        view::lint_gated(&cart, &["dot.wave".to_string()], &LintThresholds::default())
            .expect("lint_gated");
    assert!(!violated);
}

#[test]
fn no_unique_colors_is_not_applicable_to_a_one_frame_animation() {
    let cart = cart();
    let thresholds = LintThresholds {
        no_unique_colors: true,
        ..LintThresholds::default()
    };
    let (value, violated) =
        view::lint_gated(&cart, &["dot.still".to_string()], &thresholds).expect("lint_gated");
    assert!(!violated);
    assert_eq!(value["violations"], json!([]));
    let anim = &value["anims"][0];
    assert_eq!(anim["colors_unique_to_single_frame"], json!([]));
    assert_eq!(anim["unique_color_analysis"]["applicable"], false);
    assert_eq!(
        anim["unique_color_analysis"]["reason"],
        "requires_at_least_two_frames"
    );
}

// ---------------------------------------------------------------------------
// --summary
// ---------------------------------------------------------------------------

#[test]
fn summary_reports_frame_count_worst_drift_worst_changed_and_unique_colors() {
    let cart = cart();
    let (summaries, violations, violated) =
        view::lint_summary(&cart, &["dot.wave".to_string()], &LintThresholds::default())
            .expect("lint_summary");
    assert!(!violated);
    assert!(violations.is_empty());
    assert_eq!(summaries.len(), 1);
    let s = &summaries[0];
    assert_eq!(s.anim, "dot.wave");
    assert_eq!(s.frame_count, 2);
    approx(&json!(s.worst_drift.unwrap()), 2.95);
    assert_eq!(s.worst_changed, Some(3));
    assert_eq!(s.unique_colors, 1);
    assert!(s.unique_colors_applicable);
    assert!(s.line().starts_with("dot.wave: frames=2"), "{}", s.line());
}

#[test]
fn summary_marks_one_frame_unique_color_analysis_not_applicable() {
    let cart = cart();
    let (summaries, violations, violated) = view::lint_summary(
        &cart,
        &["dot.still".to_string()],
        &LintThresholds::default(),
    )
    .expect("lint_summary");
    assert!(!violated);
    assert!(violations.is_empty());
    let summary = &summaries[0];
    assert_eq!(summary.unique_colors, 0);
    assert!(!summary.unique_colors_applicable);
    assert!(summary.line().ends_with("unique_colors=n/a"));
}

#[test]
fn summary_combines_with_thresholds_and_still_reports_violations() {
    let cart = cart();
    let thresholds = LintThresholds {
        max_changed: Some(2),
        ..LintThresholds::default()
    };
    let (summaries, violations, violated) =
        view::lint_summary(&cart, &["dot.wave".to_string()], &thresholds).expect("lint_summary");
    assert!(violated);
    assert_eq!(summaries.len(), 1);
    assert_eq!(violations.len(), 2);
}

// ---------------------------------------------------------------------------
// CLI-level exit codes (`console sprite lint`)
// ---------------------------------------------------------------------------

#[test]
fn cli_exit_code_is_zero_with_no_thresholds() {
    let path = temp_cart();
    let code = view::cli_view(&args(&["lint", path.to_str().unwrap(), "dot.wave"]));
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
}

#[test]
fn cli_exit_code_is_one_when_a_threshold_is_violated() {
    let path = temp_cart();
    let code = view::cli_view(&args(&[
        "lint",
        path.to_str().unwrap(),
        "dot.wave",
        "--max-changed",
        "2",
    ]));
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 1);
}

#[test]
fn cli_no_unique_colors_accepts_a_one_frame_animation() {
    let path = temp_cart();
    let code = view::cli_view(&args(&[
        "lint",
        path.to_str().unwrap(),
        "dot.still",
        "--no-unique-colors",
    ]));
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
}

#[test]
fn cli_exit_code_is_zero_when_the_threshold_passes() {
    let path = temp_cart();
    let code = view::cli_view(&args(&[
        "lint",
        path.to_str().unwrap(),
        "dot.wave",
        "--max-changed",
        "10",
    ]));
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
}

#[test]
fn cli_summary_flag_also_gates_the_exit_code() {
    let path = temp_cart();
    let code = view::cli_view(&args(&[
        "lint",
        path.to_str().unwrap(),
        "dot.wave",
        "--summary",
        "--max-changed",
        "2",
    ]));
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 1);
}

// ---------------------------------------------------------------------------
// RPC mirror (`sprite_lint`)
// ---------------------------------------------------------------------------

fn loaded_session() -> Session {
    let mut session = Session::new();
    let resp = handle(
        &mut session,
        json!({"jsonrpc": "2.0", "id": 0, "method": "load_cart", "params": {"text": CART}}),
    );
    assert!(resp.get("error").is_none(), "load_cart failed: {resp}");
    session
}

fn call(session: &mut Session, method: &str, params: Value) -> Value {
    handle(
        session,
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
    )
}

#[test]
fn rpc_sprite_lint_with_no_thresholds_reports_violated_false_and_no_violations_key() {
    let mut session = loaded_session();
    let resp = call(&mut session, "sprite_lint", json!({"anims": ["dot.wave"]}));
    assert!(resp.get("error").is_none(), "{resp}");
    assert_eq!(resp["result"]["violated"], false);
    assert!(resp["result"].get("violations").is_none(), "{resp}");
    // sprite_id is always present, regardless of thresholds.
    assert_eq!(
        resp["result"]["anims"][0]["frames"][0]["sprite_id"],
        json!([0, 0])
    );
    assert_eq!(
        resp["result"]["anims"][0]["frames"][1]["sprite_id"],
        json!([1, 0])
    );
}

#[test]
fn rpc_sprite_lint_max_changed_violates_and_reports_violated_true() {
    let mut session = loaded_session();
    let resp = call(
        &mut session,
        "sprite_lint",
        json!({"anims": ["dot.wave"], "max_changed": 2}),
    );
    assert!(resp.get("error").is_none(), "{resp}");
    assert_eq!(resp["result"]["violated"], true);
    let violations = resp["result"]["violations"].as_array().expect("violations");
    assert_eq!(violations.len(), 2);
    assert_eq!(violations[0]["metric"], "changed_pixels");
}

#[test]
fn rpc_sprite_lint_summary_mirrors_the_cli_summary_shape() {
    let mut session = loaded_session();
    let resp = call(
        &mut session,
        "sprite_lint",
        json!({"anims": ["dot.wave"], "summary": true}),
    );
    assert!(resp.get("error").is_none(), "{resp}");
    let anim = &resp["result"]["anims"][0];
    assert_eq!(anim["anim"], "dot.wave");
    assert_eq!(anim["frames"], 2);
    assert_eq!(anim["unique_colors"], 1);
    assert_eq!(anim["unique_colors_applicable"], true);
    approx(&anim["worst_drift"], 2.95);
    assert_eq!(anim["worst_changed"], 3);
    assert_eq!(resp["result"]["violated"], false);
}

#[test]
fn rpc_sprite_lint_no_unique_colors_param_violates() {
    let mut session = loaded_session();
    let resp = call(
        &mut session,
        "sprite_lint",
        json!({"anims": ["dot.wave"], "no_unique_colors": true}),
    );
    assert!(resp.get("error").is_none(), "{resp}");
    assert_eq!(resp["result"]["violated"], true);
    assert_eq!(
        resp["result"]["violations"][0],
        json!({"anim": "dot.wave", "frame": 1, "metric": "unique_color", "value": 9, "limit": 0})
    );
}

#[test]
fn rpc_sprite_lint_no_unique_colors_accepts_a_one_frame_animation() {
    let mut session = loaded_session();
    let resp = call(
        &mut session,
        "sprite_lint",
        json!({"anims": ["dot.still"], "no_unique_colors": true, "summary": true}),
    );
    assert!(resp.get("error").is_none(), "{resp}");
    assert_eq!(resp["result"]["violated"], false);
    assert_eq!(resp["result"]["violations"], json!([]));
    assert_eq!(
        resp["result"]["anims"][0]["unique_colors_applicable"],
        false
    );
    assert_eq!(resp["result"]["anims"][0]["unique_colors"], 0);
}

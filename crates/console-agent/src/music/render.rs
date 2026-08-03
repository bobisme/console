//! `music render` — a WAV of a whole song, without a hand-written input
//! script.
//!
//! Before this, hearing a cart's music meant scripting a navigation sequence
//! ("press DOWN sixteen times, then A") through whatever menu the cart puts
//! in front of its soundtrack, then `--wav`-ing the result. That is a lot of
//! ceremony for "play song 2 twice", and it silently breaks whenever the menu
//! changes.
//!
//! `music render` boots the cart headlessly, calls `music(n)` through `eval`
//! — the same entry point the game would use — and steps until the song has
//! played its intro plus `--loops` passes of its loop body.
//!
//! ## How the stop condition works
//!
//! Two mechanisms, and the run stops at whichever fires first:
//!
//! 1. **The plan.** [`super::plan_song`] resolves the `__music__` chain
//!    statically, so the exact frame count for intro + K loops is known
//!    before a single frame is stepped (`pattern duration = max(rows*speed)`
//!    over its slots, which is precisely what the sequencer computes). This
//!    is the frame budget, and for a cart that leaves the sequencer alone it
//!    is also the answer.
//! 2. **Observation.** Every frame, `Console::music_pattern()` is sampled.
//!    Re-entering the loop-start pattern counts one completed pass, so a cart
//!    whose Lua drives the sequencer itself still stops after K passes rather
//!    than at an arithmetic guess. Music halting (the pattern going `None`
//!    after having been `Some`) ends the render immediately — that is what a
//!    `stop` chain does, and rendering silence past it helps nobody.
//!
//! A safety ceiling of one extra loop pass past the planned budget keeps a
//! pathological cart from rendering forever. `--frames F` bypasses all of it
//! and simply steps F frames.

use console_core::Console;

use super::{SongPlan, audio_only_cart, cart_arg, parse_flags, plan_song, seconds};
use crate::audio::encode_wav;
use crate::session::Session;

/// What a render actually did — reported on stdout and asserted in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderReport {
    /// Frames the plan asked for (0 when `--frames` overrode it).
    pub planned_frames: u64,
    /// Frames actually rendered.
    pub frames: u64,
    pub intro_frames: u64,
    pub loop_frames: u64,
    pub loops: u32,
    /// Loop-body passes actually observed via `music_pattern()`.
    pub loops_observed: u32,
    /// True when music halted (or the cart did) before the budget ran out.
    pub stopped_early: bool,
    /// The cart halted with a Lua error.
    pub halted: Option<String>,
}

/// Options for [`render_song`].
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    pub song: Option<u8>,
    pub loops: u32,
    /// Manual frame-count override; skips loop detection entirely.
    pub frames: Option<u64>,
    pub seed: u64,
}

impl Default for RenderOpts {
    fn default() -> RenderOpts {
        RenderOpts {
            song: None,
            loops: 2,
            frames: None,
            seed: 0,
        }
    }
}

/// Boot `cart_text`, start its song and render it to raw samples.
///
/// Returns the samples plus a [`RenderReport`]. The cart's own `_update` runs
/// every frame exactly as it would in a game — this is the real cart, not a
/// synthesised one — so a cart that starts its own music from `_init` and one
/// that waits for a button press both render the same song here.
pub fn render_song(cart_text: &str, opts: &RenderOpts) -> Result<(Vec<f32>, RenderReport), String> {
    let cart = console_core::Cart::parse(cart_text).map_err(|e| e.to_string())?;
    let start = match opts.song {
        Some(id) => id,
        None => super::default_song(&cart)?,
    };
    let plan = plan_song(&cart, start)?;

    let mut session = Session::new();
    session
        .load_cart(cart_text, opts.seed)
        .map_err(|e| e.to_string())?;
    session
        .eval(&format!("music({start})"))
        .map_err(|e| format!("starting music({start}): {e}"))?;

    let report = drive(&mut session, &cart, &plan, opts)?;
    let (samples, _, _) = session.audio_slice(None, None).map_err(|e| e.to_string())?;
    Ok((samples.to_vec(), report))
}

/// Step the session frame by frame until the stop condition fires.
///
/// The one non-obvious bit of bookkeeping is detecting a **pattern start**.
/// `music_pattern()` changing is the easy half; the other half is a pattern
/// that loops to *itself* (`pat 4 loop=4`, the most common one-pattern song),
/// where the id never changes. So a start is "the id changed" **or** "the id
/// is the same and this pattern has now been playing for its full duration",
/// which is exactly the frame the sequencer restarts it on.
///
/// `step()` renders a frame's samples and *then* advances the sequencer, so
/// when a start is observed after stepping frame F, frame F still belongs to
/// the pass that just ended — every frame counted here is a frame that was
/// legitimately rendered, and the total lands exactly on
/// `intro + loops * loop_body`.
fn drive(
    session: &mut Session,
    cart: &console_core::Cart,
    plan: &SongPlan,
    opts: &RenderOpts,
) -> Result<RenderReport, String> {
    let planned = opts.frames.unwrap_or_else(|| plan.frames_for(opts.loops));
    // One spare loop pass (or one spare pattern for a non-looping song) so a
    // cart that drives the sequencer from Lua still finishes, and a broken one
    // still terminates.
    let ceiling = match opts.frames {
        Some(f) => f,
        None => {
            let slack = plan
                .loop_frames()
                .max(u64::from(plan.steps.last().map(|s| s.frames).unwrap_or(1)));
            planned + slack
        }
    };
    let loop_pattern = plan.loop_pattern();
    let manual = opts.frames.is_some();

    let mut current = session
        .console()
        .map_err(|e| e.to_string())?
        .music_pattern();
    let mut frames_in_pattern = 0u32;
    let mut loops_observed = if !manual && current.is_some() && current == loop_pattern {
        1
    } else {
        0
    };

    let mut frames = 0u64;
    let mut started = current.is_some();
    let mut music_stopped = false;
    let mut halted = None;

    while frames < ceiling {
        let outcome = session.step(1, 0).map_err(|e| e.to_string())?;
        frames += 1;
        frames_in_pattern += 1;
        if outcome.halted {
            halted = outcome.message;
            break;
        }
        let pattern = session
            .console()
            .map_err(|e| e.to_string())?
            .music_pattern();
        if pattern.is_some() {
            started = true;
        }
        if manual {
            continue;
        }

        let restarted = pattern.is_some()
            && pattern == current
            && frames_in_pattern >= super::pattern_frames(cart, pattern.expect("checked Some"));
        if pattern != current || restarted {
            current = pattern;
            frames_in_pattern = 0;
            if pattern.is_some() && pattern == loop_pattern {
                loops_observed += 1;
                if loops_observed > opts.loops {
                    break;
                }
            }
        }
        if started && pattern.is_none() {
            music_stopped = true;
            break;
        }
        if frames >= planned && loop_pattern.is_none() {
            break;
        }
    }

    Ok(RenderReport {
        planned_frames: if manual { 0 } else { planned },
        frames,
        intro_frames: plan.intro_frames(),
        loop_frames: plan.loop_frames(),
        loops: opts.loops,
        loops_observed: loops_observed.min(opts.loops),
        stopped_early: halted.is_some() || (music_stopped && frames < planned),
        halted,
    })
}

/// Render exactly one pass of one pattern, in isolation, and return its
/// samples.
///
/// Used by `music lint` for its per-pattern peak. The cart's Lua is replaced
/// with a two-line program that starts the pattern and nothing else, so the
/// measurement is of the *music* rather than of whatever the game happens to
/// be doing at the time; the `__instruments__`, `__sfx__` and `__music__`
/// sections are copied verbatim, so master drive, tone, hiss and the echo bus
/// all apply exactly as they would in the game.
///
/// The pattern is measured cold — no echo tail carried in from whatever
/// played before it — which is the right baseline for "is this pattern
/// clipping on its own".
pub fn pattern_samples(cart: &console_core::Cart, id: u8) -> Result<Vec<f32>, String> {
    let frames = super::pattern_frames(cart, id);
    if frames == 0 {
        return Err(format!("cart has no pattern {id}"));
    }
    let text = audio_only_cart(cart, &format!("function _init() music({id}) end"));
    let mut console = Console::new(&text, 0).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(frames as usize * console_core::SAMPLES_PER_FRAME);
    for _ in 0..frames {
        console.step(0).map_err(|e| e.to_string())?;
        out.extend_from_slice(console.audio_frame());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub fn cli_render(args: &[String]) -> Result<i32, String> {
    let flags = parse_flags(args)?;
    let path = cart_arg(&flags, "render")?;
    let out = flags
        .out
        .as_deref()
        .ok_or("music render requires -o <out.wav>")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path:?}: {e}"))?;

    let opts = RenderOpts {
        song: flags.song,
        loops: flags.loops.unwrap_or(2),
        frames: flags.frames,
        seed: flags.seed.unwrap_or(0),
    };
    let (samples, report) = render_song(&text, &opts)?;
    std::fs::write(out, encode_wav(&samples)).map_err(|e| format!("cannot write {out:?}: {e}"))?;

    println!(
        "wrote {out} ({} frames, {:.2}s, intro {} + {} x loop {})",
        report.frames,
        seconds(report.frames),
        report.intro_frames,
        report.loops_observed,
        report.loop_frames,
    );
    if report.stopped_early {
        match &report.halted {
            Some(msg) => println!("cart halted: {msg}"),
            None => println!("music stopped before the frame budget ran out"),
        }
    }
    Ok(0)
}

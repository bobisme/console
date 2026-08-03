//! `console-agent run <cart> [...]` — one-shot headless execution.

use crate::input_spec::{self, Segment};
use crate::session::Session;
use crate::value::lua_to_json;

/// Parsed `run` subcommand arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct RunArgs {
    pub cart_path: String,
    pub frames: Option<u64>,
    pub input_spec: String,
    pub screenshot: Option<String>,
    pub screenshot_zoom: u32,
    pub screen_text: bool,
    pub eval: Option<String>,
    pub seed: u64,
    pub wav: Option<String>,
    pub spectrogram: Option<String>,
    pub audio_events: bool,
    pub audio_stats: bool,
}

/// Parse the arguments following `run` (i.e. `args[2..]` of `argv`).
pub fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut cart_path: Option<String> = None;
    let mut frames: Option<u64> = None;
    let mut input_spec = String::new();
    let mut screenshot: Option<String> = None;
    let mut screenshot_zoom: u32 = 1;
    let mut screen_text = false;
    let mut eval: Option<String> = None;
    let mut seed: u64 = 0;
    let mut wav: Option<String> = None;
    let mut spectrogram: Option<String> = None;
    let mut audio_events = false;
    let mut audio_stats = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--frames" => {
                let v = iter.next().ok_or("--frames requires a value")?;
                frames = Some(
                    v.parse()
                        .map_err(|_| format!("invalid --frames value {v:?}"))?,
                );
            }
            "--input" => {
                let v = iter.next().ok_or("--input requires a value")?;
                input_spec = v.clone();
            }
            "--screenshot" => {
                let v = iter.next().ok_or("--screenshot requires a value")?;
                screenshot = Some(v.clone());
            }
            "--screenshot-zoom" => {
                let v = iter.next().ok_or("--screenshot-zoom requires a value")?;
                let z: u32 = v
                    .parse()
                    .map_err(|_| format!("invalid --screenshot-zoom value {v:?}"))?;
                if z < 1 {
                    return Err(format!("--screenshot-zoom must be >= 1, got {v:?}"));
                }
                screenshot_zoom = z;
            }
            "--screen-text" => screen_text = true,
            "--eval" => {
                let v = iter.next().ok_or("--eval requires a value")?;
                eval = Some(v.clone());
            }
            "--seed" => {
                let v = iter.next().ok_or("--seed requires a value")?;
                seed = v
                    .parse()
                    .map_err(|_| format!("invalid --seed value {v:?}"))?;
            }
            "--wav" => {
                let v = iter.next().ok_or("--wav requires a value")?;
                wav = Some(v.clone());
            }
            "--spectrogram" => {
                let v = iter.next().ok_or("--spectrogram requires a value")?;
                spectrogram = Some(v.clone());
            }
            "--audio-events" => audio_events = true,
            "--audio-stats" => audio_stats = true,
            other if other.starts_with("--") => return Err(format!("unknown flag {other:?}")),
            other => {
                if cart_path.is_some() {
                    return Err(format!("unexpected extra argument {other:?}"));
                }
                cart_path = Some(other.to_string());
            }
        }
    }

    Ok(RunArgs {
        cart_path: cart_path.ok_or("missing <cart> argument")?,
        frames,
        input_spec,
        screenshot,
        screenshot_zoom,
        screen_text,
        eval,
        seed,
        wav,
        spectrogram,
        audio_events,
        audio_stats,
    })
}

/// Run the oneshot flow, writing `printh` logs / errors to stderr and
/// `--screen-text` / `--eval` output to stdout. Returns the process exit
/// code (0 on success, nonzero if the cart errored/halted or the eval
/// expression itself failed).
pub fn run(args: &RunArgs) -> i32 {
    let cart_text = match std::fs::read_to_string(&args.cart_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read {:?}: {e}", args.cart_path);
            return 2;
        }
    };

    let segments: Vec<Segment> = match input_spec::parse_spec(&args.input_spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    let mut session = Session::new();
    if let Err(e) = session.load_cart(&cart_text, args.seed) {
        eprintln!("error: {e}");
        return 1;
    }

    let total_frames = args
        .frames
        .unwrap_or_else(|| input_spec::total_frames(&segments));

    let mut halt_message: Option<String> = None;
    for frame_idx in 0..total_frames {
        let mask = input_spec::mask_at(&segments, frame_idx);
        match session.step(1, mask) {
            Ok(outcome) if outcome.halted => {
                halt_message = outcome.message;
                break;
            }
            Ok(_) => {}
            Err(e) => {
                halt_message = Some(e.to_string());
                break;
            }
        }
    }

    // --eval runs after stepping, regardless of --frames/--input.
    let mut eval_error: Option<String> = None;
    let mut eval_result = None;
    if let Some(code) = &args.eval {
        match session.eval(code) {
            Ok(v) => eval_result = Some(lua_to_json(&v)),
            Err(e) => eval_error = Some(e.to_string()),
        }
    }

    if let Ok(logs) = session.logs() {
        for line in logs {
            eprintln!("[log] {line}");
        }
    }

    // --screenshot / --screen-text / --wav / --spectrogram / --audio-events
    // / --audio-stats all reflect the final frame (after --eval, in case the
    // evaluated code itself drew or played something).
    if let Some(path) = &args.screenshot {
        match session.screenshot_png_zoomed(args.screenshot_zoom) {
            Ok(bytes) => {
                if let Err(e) = crate::artifact::write(path, &bytes) {
                    eprintln!("error: {e}");
                    return 1;
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }

    if let Some(path) = &args.wav {
        match session.wav_bytes(None, None) {
            Ok((bytes, _frames, _samples)) => {
                if let Err(e) = crate::artifact::write(path, &bytes) {
                    eprintln!("error: {e}");
                    return 1;
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }

    if let Some(path) = &args.spectrogram {
        match session.spectrogram_png(None, None, 4) {
            Ok(spec) => {
                if let Err(e) = crate::artifact::write(path, &spec.png) {
                    eprintln!("error: {e}");
                    return 1;
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }

    if args.screen_text {
        match session.screen_text() {
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }

    if args.audio_events {
        match session.audio_events(None) {
            Ok(events) => {
                for event in events {
                    match serde_json::to_string(&event) {
                        Ok(line) => println!("{line}"),
                        Err(e) => {
                            eprintln!("error: {e}");
                            return 1;
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }

    if args.audio_stats {
        match session.audio_stats(6) {
            Ok(windows) => match serde_json::to_string(&windows) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    eprintln!("error: {e}");
                    return 1;
                }
            },
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }

    if let Some(v) = eval_result {
        println!("{v}");
    }

    if let Some(msg) = halt_message {
        eprintln!("error: {msg}");
        return 1;
    }
    if let Some(msg) = eval_error {
        eprintln!("error: {msg}");
        return 1;
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_flag_set() {
        let args = parse_run_args(&[
            "cart.cart".into(),
            "--frames".into(),
            "10".into(),
            "--input".into(),
            "30:,10:R".into(),
            "--screenshot".into(),
            "out.png".into(),
            "--screenshot-zoom".into(),
            "4".into(),
            "--screen-text".into(),
            "--eval".into(),
            "1+1".into(),
            "--seed".into(),
            "7".into(),
            "--wav".into(),
            "out.wav".into(),
            "--spectrogram".into(),
            "spec.png".into(),
            "--audio-events".into(),
            "--audio-stats".into(),
        ])
        .unwrap();

        assert_eq!(
            args,
            RunArgs {
                cart_path: "cart.cart".into(),
                frames: Some(10),
                input_spec: "30:,10:R".into(),
                screenshot: Some("out.png".into()),
                screenshot_zoom: 4,
                screen_text: true,
                eval: Some("1+1".into()),
                seed: 7,
                wav: Some("out.wav".into()),
                spectrogram: Some("spec.png".into()),
                audio_events: true,
                audio_stats: true,
            }
        );
    }

    #[test]
    fn missing_cart_path_is_an_error() {
        assert!(parse_run_args(&["--seed".into(), "1".into()]).is_err());
    }

    #[test]
    fn unknown_flag_is_an_error() {
        assert!(parse_run_args(&["cart.cart".into(), "--bogus".into()]).is_err());
    }

    #[test]
    fn screenshot_zoom_defaults_to_one() {
        let args =
            parse_run_args(&["cart.cart".into(), "--screenshot".into(), "out.png".into()]).unwrap();
        assert_eq!(args.screenshot_zoom, 1);
    }

    #[test]
    fn screenshot_zoom_zero_is_rejected() {
        let err = parse_run_args(&[
            "cart.cart".into(),
            "--screenshot".into(),
            "out.png".into(),
            "--screenshot-zoom".into(),
            "0".into(),
        ])
        .unwrap_err();
        assert!(err.contains("--screenshot-zoom"), "{err}");
    }
}

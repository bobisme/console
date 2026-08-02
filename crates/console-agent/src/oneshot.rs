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
    pub screen_text: bool,
    pub eval: Option<String>,
    pub seed: u64,
}

/// Parse the arguments following `run` (i.e. `args[2..]` of `argv`).
pub fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut cart_path: Option<String> = None;
    let mut frames: Option<u64> = None;
    let mut input_spec = String::new();
    let mut screenshot: Option<String> = None;
    let mut screen_text = false;
    let mut eval: Option<String> = None;
    let mut seed: u64 = 0;

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
        screen_text,
        eval,
        seed,
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

    // --screenshot / --screen-text reflect the final frame (after --eval,
    // in case the evaluated code itself drew something).
    if let Some(path) = &args.screenshot {
        match session.screenshot_png() {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(path, &bytes) {
                    eprintln!("error: cannot write {path:?}: {e}");
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
            "--screen-text".into(),
            "--eval".into(),
            "1+1".into(),
            "--seed".into(),
            "7".into(),
        ])
        .unwrap();

        assert_eq!(
            args,
            RunArgs {
                cart_path: "cart.cart".into(),
                frames: Some(10),
                input_spec: "30:,10:R".into(),
                screenshot: Some("out.png".into()),
                screen_text: true,
                eval: Some("1+1".into()),
                seed: 7,
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
}

//! Headless AI-agent harness for the fantasy console: a oneshot CLI and a
//! JSON-RPC-over-stdio `serve` mode, both built on the same [`Session`] /
//! dispatch layer so the RPC surface is unit-testable without spawning the
//! binary.
//!
//! - [`session`] — session state: the running console, its cart text/seed,
//!   the per-frame input log, a parallel audio sample log and sequencer
//!   event log, and named save states.
//! - [`rpc`] — JSON-RPC 2.0 request/response handling on top of a session.
//! - [`oneshot`] — the `run` subcommand.
//! - [`value`] — `mlua::Value` -> JSON, shared by `eval`/`get_global` in
//!   both modes.
//! - [`input_spec`] — the oneshot `--input` `COUNT:BUTTONS` mini-language.
//! - [`audio`] — audio inspection tooling: WAV encoding, sequencer event
//!   diffing, signal stats and the semitone-grid spectrogram.
//! - [`map`] — map authoring tooling: render/dump/lint plus poke/edit
//!   in-place cart transforms for the `__map__` tile grid.
//! - [`music`] — music authoring tooling: score (the song as text), lint
//!   (JSON diagnostics), piano-roll (score-level PNG) and render (a WAV of a
//!   whole song, loop detection included).

pub mod artifact;
pub mod audio;
pub mod input_spec;
pub mod map;
pub mod music;
pub mod oneshot;
pub mod rpc;
pub mod session;
pub mod sprite;
pub mod value;

pub const RUN_USAGE: &str = "\
usage:
  console-agent run <cart> [--frames N] [--input SPEC] [--screenshot out.png] [--screen-text] [--eval CODE] [--seed N]
                    [--wav out.wav] [--spectrogram out.png] [--audio-events] [--audio-stats]";

pub const SERVE_USAGE: &str = "usage:\n  console-agent serve";

/// Complete top-level help, generated from each command family's public
/// inventory so a newly-added leaf cannot silently disappear from discovery.
pub fn usage() -> String {
    format!(
        "{RUN_USAGE}\n  console-agent serve\n  console-agent sprite <{}> ...\n  console-agent map <{}> ...\n  console-agent music <{}> ...",
        sprite::COMMANDS.join("|"),
        map::COMMANDS.join("|"),
        music::COMMANDS.join("|")
    )
}

fn help_requested(args: &[String]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
}

/// Entry point shared by `main.rs` and integration tests: takes a full
/// `argv` (including `argv[0]`) and returns the process exit code.
pub fn cli_main(args: &[String]) -> i32 {
    match args.get(1).map(String::as_str) {
        Some("-h" | "--help") => {
            println!("{}", usage());
            0
        }
        Some("run") if help_requested(&args[2..]) => {
            println!("{RUN_USAGE}");
            0
        }
        Some("run") => match oneshot::parse_run_args(&args[2..]) {
            Ok(run_args) => oneshot::run(&run_args),
            Err(e) => {
                eprintln!("error: {e}");
                eprintln!("{}", usage());
                2
            }
        },
        Some("sprite") => sprite::cli_sprite(&args[2..]),
        Some("map") => map::cli_map(&args[2..]),
        Some("music") => music::cli_music(&args[2..]),
        Some("serve") if help_requested(&args[2..]) => {
            println!("{SERVE_USAGE}");
            0
        }
        Some("serve") => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            match rpc::run_serve(session::Session::new(), stdin.lock(), stdout.lock()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        Some(other) => {
            eprintln!("error: unknown subcommand {other:?}");
            eprintln!("{}", usage());
            2
        }
        None => {
            eprintln!("{}", usage());
            2
        }
    }
}

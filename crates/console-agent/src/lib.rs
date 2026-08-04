//! Unified CLI for the fantasy console: headless execution, authoring tools,
//! JSON-RPC automation, single-file HTML packing, and local browser serving.
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
//! - [`playtest`] — ordered, versioned scenario execution over one session.

pub mod artifact;
pub mod audio;
pub mod input_spec;
pub mod map;
pub mod music;
pub mod oneshot;
pub mod pack;
pub mod palette;
pub mod playtest;
pub mod project;
pub mod rpc;
pub mod serve;
pub mod session;
pub mod sprite;
pub mod value;

pub const RUN_USAGE: &str = "\
usage:
  console run <cart|project> [--frames N] [--input SPEC] [--screenshot out.png] [--screen-text] [--eval CODE] [--seed N]
                    [--wav out.wav] [--spectrogram out.png] [--audio-events] [--audio-stats] [--text-events]
                    [--draw-trace trace.json]";

pub const RPC_USAGE: &str = "usage:\n  console rpc";

/// Complete top-level help, generated from each command family's public
/// inventory so a newly-added leaf cannot silently disappear from discovery.
pub fn usage() -> String {
    format!(
        "{RUN_USAGE}\n  console playtest <cart|project> --scenario <scenario.json> [--artifacts DIR] [--seed N] [--format text|pretty|json]\n  console rpc\n  {}\n  console pack <cart|project> -o <out.html> [--engine FILE] [--template FILE]\n  console serve <cart|project> [--host HOST] [--port PORT] [--engine FILE] [--template FILE]\n  console palette <{}> ...\n  console sprite <{}> ...\n  console map <{}> ...\n  console music <{}> ...",
        project::BUILD_USAGE
            .lines()
            .next()
            .unwrap_or("console build"),
        palette::COMMANDS.join("|"),
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
        Some("playtest") => playtest::cli_playtest(&args[2..]),
        Some("build") => project::cli_build(&args[2..]),
        Some("palette") => palette::cli_palette(&args[2..]),
        Some("sprite") => sprite::cli_sprite(&args[2..]),
        Some("map") => map::cli_map(&args[2..]),
        Some("music") => music::cli_music(&args[2..]),
        Some("pack") => pack::cli_pack(&args[2..]),
        Some("serve") => serve::cli_serve(&args[2..]),
        Some("rpc") if help_requested(&args[2..]) => {
            println!("{RPC_USAGE}");
            0
        }
        Some("rpc") => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            match rpc::run_rpc(session::Session::new(), stdin.lock(), stdout.lock()) {
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

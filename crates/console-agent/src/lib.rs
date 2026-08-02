//! Headless AI-agent harness for the fantasy console: a oneshot CLI and a
//! JSON-RPC-over-stdio `serve` mode, both built on the same [`Session`] /
//! dispatch layer so the RPC surface is unit-testable without spawning the
//! binary.
//!
//! - [`session`] — session state: the running console, its cart text/seed,
//!   the per-frame input log, and named save states.
//! - [`rpc`] — JSON-RPC 2.0 request/response handling on top of a session.
//! - [`oneshot`] — the `run` subcommand.
//! - [`value`] — `mlua::Value` -> JSON, shared by `eval`/`get_global` in
//!   both modes.
//! - [`input_spec`] — the oneshot `--input` `COUNT:BUTTONS` mini-language.

pub mod input_spec;
pub mod oneshot;
pub mod rpc;
pub mod session;
pub mod value;

const USAGE: &str = "\
usage:
  console-agent run <cart> [--frames N] [--input SPEC] [--screenshot out.png] [--screen-text] [--eval CODE] [--seed N]
  console-agent serve";

/// Entry point shared by `main.rs` and integration tests: takes a full
/// `argv` (including `argv[0]`) and returns the process exit code.
pub fn cli_main(args: &[String]) -> i32 {
    match args.get(1).map(String::as_str) {
        Some("run") => match oneshot::parse_run_args(&args[2..]) {
            Ok(run_args) => oneshot::run(&run_args),
            Err(e) => {
                eprintln!("error: {e}");
                eprintln!("{USAGE}");
                2
            }
        },
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
            eprintln!("{USAGE}");
            2
        }
        None => {
            eprintln!("{USAGE}");
            2
        }
    }
}

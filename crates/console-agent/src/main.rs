//! Thin binary wrapper: all logic lives in the library (`src/lib.rs` and
//! friends) so it can be unit-tested without spawning a process.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(console_agent::cli_main(&args));
}

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .output()
        .expect("run console")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

#[test]
fn top_level_help_is_complete_and_successful() {
    for flag in ["--help", "-h"] {
        let output = run(&[flag]);
        assert!(output.status.success(), "{flag} should exit successfully");
        assert!(output.stderr.is_empty(), "help should not write stderr");
        let text = stdout(&output);
        assert!(text.contains("sprite <render|strip|onion|diff|ghost|gif|lint|edit|dump|poke>"));
        assert!(text.contains("map <render|dump|lint|edit|poke>"));
        assert!(text.contains("music <score|lint|piano-roll|render|edit|import-abc>"));
        assert!(text.contains("playtest <cart> --scenario <scenario.json>"));
        assert!(text.contains("rpc"));
        assert!(text.contains("pack <cart>"));
        assert!(text.contains("serve <cart>"));
    }
}

#[test]
fn command_family_and_leaf_help_use_stdout_and_exit_zero() {
    for (args, expected) in [
        (&["run", "--help"][..], "console run <cart>"),
        (&["rpc", "--help"][..], "console rpc"),
        (&["pack", "--help"][..], "console pack"),
        (&["serve", "--help"][..], "console serve"),
        (&["playtest", "--help"][..], "Scenario format (version 1)"),
        (&["sprite", "--help"][..], "console sprite gif"),
        (&["sprite", "render", "--help"][..], "console sprite render"),
        (&["map", "--help"][..], "console map render"),
        (&["music", "--help"][..], "console music import-abc"),
        (&["music", "edit", "--help"][..], "console music edit"),
    ] {
        let output = run(args);
        assert!(output.status.success(), "{args:?} should exit successfully");
        assert!(output.stderr.is_empty(), "{args:?} wrote stderr");
        assert!(
            stdout(&output).contains(expected),
            "{args:?} omitted {expected}"
        );
    }
}

#[test]
fn missing_or_unknown_command_remains_an_error() {
    let missing = run(&[]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("usage:"));

    let unknown = run(&["frobnicate"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown subcommand"));
}

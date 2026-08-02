//! Integration tests that exercise the real `console-agent` binary as a
//! subprocess, covering both `serve` (JSON-RPC over stdio) and the
//! `run` oneshot subcommand end to end.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn demo_cart_path() -> String {
    format!("{}/../../carts/demo.cart", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn serve_mode_handles_a_few_requests_in_order() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_console-agent"))
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `console-agent serve`");

    let mut stdin = child.stdin.take().expect("child stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("child stdout"));

    let cart_path = demo_cart_path();
    let requests = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "load_cart", "params": {"path": cart_path}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "step", "params": {"frames": 5, "input": "R"}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "info", "params": {}}),
    ];

    let mut responses = Vec::new();
    for req in &requests {
        writeln!(stdin, "{}", serde_json::to_string(req).unwrap()).expect("write request line");
        stdin.flush().expect("flush request");

        let mut line = String::new();
        reader.read_line(&mut line).expect("read response line");
        assert!(
            !line.trim().is_empty(),
            "expected exactly one response line per request"
        );
        responses.push(
            serde_json::from_str::<serde_json::Value>(&line)
                .unwrap_or_else(|e| panic!("response {line:?} did not parse as JSON: {e}")),
        );
    }

    drop(stdin); // EOF on the child's stdin ends the serve loop.
    let status = child.wait().expect("wait for console-agent to exit");
    assert!(
        status.success(),
        "serve loop should exit cleanly on stdin EOF"
    );

    assert_eq!(responses[0]["id"], 1);
    assert!(
        responses[0].get("error").is_none(),
        "load_cart failed: {:?}",
        responses[0]
    );

    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[1]["result"]["frame_count"], 5);
    assert_eq!(responses[1]["result"]["halted"], false);

    assert_eq!(responses[2]["id"], 3);
    assert_eq!(responses[2]["result"]["frame_count"], 5);
    assert_eq!(responses[2]["result"]["title"], "Micro Dash");
}

#[test]
fn oneshot_run_with_input_spec_and_screenshot() {
    let out_path =
        std::env::temp_dir().join(format!("console-agent-oneshot-{}.png", std::process::id()));

    let output = Command::new(env!("CARGO_BIN_EXE_console-agent"))
        .arg("run")
        .arg(demo_cart_path())
        .arg("--input")
        .arg("30:,10:R,5:RA")
        .arg("--screenshot")
        .arg(out_path.to_str().unwrap())
        .output()
        .expect("run `console-agent run`");

    assert!(
        output.status.success(),
        "oneshot run failed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let data = std::fs::read(&out_path).expect("screenshot file was written");
    assert_eq!(&data[1..4], b"PNG", "output file should be a PNG");
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn oneshot_run_reports_halt_with_nonzero_exit() {
    let cart_dir = std::env::temp_dir();
    let cart_path = cart_dir.join(format!("console-agent-broken-{}.cart", std::process::id()));
    std::fs::write(
        &cart_path,
        "__lua__\nfunction _update() error('boom') end\n",
    )
    .expect("write broken cart");

    let output = Command::new(env!("CARGO_BIN_EXE_console-agent"))
        .arg("run")
        .arg(&cart_path)
        .arg("--frames")
        .arg("3")
        .output()
        .expect("run `console-agent run`");

    let _ = std::fs::remove_file(&cart_path);

    assert!(
        !output.status.success(),
        "a halting cart should exit nonzero"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("boom"));
}

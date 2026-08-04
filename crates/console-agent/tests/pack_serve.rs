mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::TestProject;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

fn console_bin() -> &'static str {
    env!("CARGO_BIN_EXE_console")
}

fn demo_cart() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../carts/demo.cart")
}

fn temp_dir(tag: &str) -> PathBuf {
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "console-cli-test-{}-{sequence}-{tag}",
        std::process::id()
    ))
}

fn fetch(authority: &str) -> String {
    fetch_with_host(authority, authority)
}

fn fetch_with_host(authority: &str, host_header: &str) -> String {
    fetch_method_with_host(authority, host_header, "GET")
}

fn fetch_method_with_host(authority: &str, host_header: &str, method: &str) -> String {
    let mut stream = TcpStream::connect(authority).expect("connect to console serve");
    write!(
        stream,
        "{method} /index.html HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn pack_uses_embedded_assets_outside_the_repository() {
    let root = temp_dir("pack");
    std::fs::create_dir_all(&root).unwrap();
    let output_path = root.join("nested/game.html");
    let output = Command::new(console_bin())
        .current_dir(&root)
        .arg("pack")
        .arg(demo_cart())
        .arg("--output")
        .arg(&output_path)
        .output()
        .expect("run console pack");

    assert!(
        output.status.success(),
        "pack failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let html = std::fs::read_to_string(&output_path).expect("packed HTML exists");
    assert!(html.contains("<title>Micro Dash</title>"));
    assert!(html.contains("window.__console = Object.freeze"));
    assert!(html.contains("function _update()"));
    assert!(!html.contains("{{ENGINE_JS}}"));
    assert!(!html.contains("{{CART_TEXT}}"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pack_accepts_project_directories_and_explicit_manifests_without_intermediate_output() {
    let project = TestProject::new("pack", "Packed Project", 21);
    let compiled = console_agent::project::compile_project(project.root()).unwrap();
    let embedded_cart = compiled.cart_text.replace("</", "<\\/");

    for (index, input) in [project.root().to_path_buf(), project.manifest()]
        .into_iter()
        .enumerate()
    {
        let output_path = project.root().join(format!("packed-{index}.html"));
        let output = Command::new(console_bin())
            .arg("pack")
            .arg(input)
            .arg("-o")
            .arg(&output_path)
            .output()
            .expect("pack source project");
        assert!(
            output.status.success(),
            "project pack failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let html = std::fs::read_to_string(output_path).unwrap();
        assert!(html.contains("<title>Packed Project</title>"));
        assert!(html.contains(&embedded_cart));
    }
    assert!(!project.root().join("build/game.cart").exists());
}

#[test]
fn serve_prints_a_url_and_returns_the_packed_page() {
    let mut child = Command::new(console_bin())
        .arg("serve")
        .arg(demo_cart())
        .arg("--port")
        .arg("0")
        .arg("--once")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn console serve");

    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut url = String::new();
    stdout.read_line(&mut url).expect("read server URL");
    let authority = url
        .trim()
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix('/'))
        .expect("stdout contains one HTTP URL");

    let response = fetch(authority);

    let output = child.wait_with_output().expect("wait for console serve");
    assert!(
        output.status.success(),
        "server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("Cache-Control: no-store\r\n"));
    assert!(response.contains("Content-Type: text/html; charset=utf-8\r\n"));
    assert!(response.contains("<title>Micro Dash</title>"));
    assert!(response.contains("window.__console = Object.freeze"));
}

#[test]
fn serve_rebundles_the_cart_on_refresh() {
    let root = temp_dir("refresh");
    std::fs::create_dir_all(&root).unwrap();
    let cart = root.join("live.cart");
    let write_cart = |title: &str| {
        std::fs::write(
            &cart,
            format!(
                "__meta__\ntitle={title}\nauthor=test\nversion=0\n\n__lua__\nfunction _draw() cls(0) end\n"
            ),
        )
        .unwrap();
    };
    write_cart("First Title");

    let mut child = Command::new(console_bin())
        .arg("serve")
        .arg(&cart)
        .arg("--port=0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn console serve");
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut url = String::new();
    stdout.read_line(&mut url).unwrap();
    let authority = url
        .trim()
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix('/'))
        .unwrap();

    write_cart("Second Title");
    assert!(fetch(authority).contains("<title>Second Title</title>"));
    write_cart("Third Title");
    assert!(fetch(authority).contains("<title>Third Title</title>"));

    child.kill().expect("stop development server");
    child.wait().expect("reap development server");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn serve_recompiles_projects_on_get_and_head_without_serving_stale_invalid_output() {
    let project = TestProject::new("serve", "First Project", 31);
    let mut child = Command::new(console_bin())
        .arg("serve")
        .arg(project.root())
        .arg("--port=0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn project server");
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut url = String::new();
    stdout.read_line(&mut url).unwrap();
    let authority = url
        .trim()
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix('/'))
        .unwrap();

    project.set_title("Second Project");
    project.set_value(32);
    let refreshed = fetch(authority);
    assert!(refreshed.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(refreshed.contains("<title>Second Project</title>"));
    assert!(refreshed.contains("return 32"));

    project.break_module();
    let invalid = fetch_method_with_host(authority, authority, "HEAD");
    assert!(invalid.starts_with("HTTP/1.1 500 Internal Server Error\r\n"));
    assert!(!invalid.contains("Second Project"));
    assert!(!invalid.contains("return 32"));

    project.set_title("Third Project");
    project.set_value(33);
    let repaired = fetch(authority);
    assert!(repaired.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(repaired.contains("<title>Third Project</title>"));
    assert!(repaired.contains("return 33"));

    child.kill().expect("stop project server");
    child.wait().expect("reap project server");

    let mut explicit = Command::new(console_bin())
        .arg("serve")
        .arg(project.manifest())
        .arg("--port=0")
        .arg("--once")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn explicit-manifest server");
    let mut stdout = BufReader::new(explicit.stdout.take().unwrap());
    let mut url = String::new();
    stdout.read_line(&mut url).unwrap();
    let authority = url
        .trim()
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix('/'))
        .unwrap();
    assert!(fetch(authority).contains("<title>Third Project</title>"));
    assert!(explicit.wait().unwrap().success());
    assert!(!project.root().join("build/game.cart").exists());
}

#[test]
fn loopback_serve_rejects_an_unrelated_host_header() {
    let mut child = Command::new(console_bin())
        .arg("serve")
        .arg(demo_cart())
        .arg("--port=0")
        .arg("--once")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn console serve");
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut url = String::new();
    stdout.read_line(&mut url).unwrap();
    let authority = url
        .trim()
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix('/'))
        .unwrap();
    let port = authority.rsplit_once(':').unwrap().1;

    let response = fetch_with_host(authority, &format!("attacker.example:{port}"));
    assert!(response.starts_with("HTTP/1.1 421 Misdirected Request\r\n"));
    assert!(!response.contains("Micro Dash"));
    assert!(!response.contains("function _update()"));

    let output = child.wait_with_output().expect("wait for console serve");
    assert!(output.status.success());
}

#[test]
fn serve_validates_before_printing_a_url() {
    let output = Command::new(console_bin())
        .arg("serve")
        .arg("definitely-missing.cart")
        .arg("--port")
        .arg("0")
        .output()
        .expect("run console serve with a missing cart");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reading"));
}

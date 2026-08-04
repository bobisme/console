use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_PROJECT: AtomicUsize = AtomicUsize::new(0);

struct ExampleProject(PathBuf);

impl ExampleProject {
    fn copy() -> Self {
        let sequence = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
        let destination = std::env::temp_dir().join(format!(
            "console-agent-platformer-{}-{sequence}",
            std::process::id()
        ));
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/agent-platformer");
        copy_tree(&source, &destination);
        Self(destination)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ExampleProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    let mut entries = std::fs::read_dir(source)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        if entry.file_name() == "build" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .output()
        .expect("run console")
}

fn path(path: &Path) -> &str {
    path.to_str().expect("test path is UTF-8")
}

struct LiveServer {
    child: Child,
    authority: String,
}

impl LiveServer {
    fn spawn(project: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_console"))
            .arg("serve")
            .arg(project)
            .arg("--port=0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start project server");
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut url = String::new();
        stdout.read_line(&mut url).expect("read project URL");
        let authority = url
            .trim()
            .strip_prefix("http://")
            .and_then(|url| url.strip_suffix('/'))
            .expect("serve prints an HTTP URL")
            .to_owned();
        Self { child, authority }
    }

    fn fetch(&self) -> String {
        let mut stream = TcpStream::connect(&self.authority).expect("connect to project server");
        write!(
            stream,
            "GET /index.html HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.authority
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn source_hopper_is_a_deterministic_end_to_end_project() {
    let project = ExampleProject::copy();
    let first = console_agent::project::compile_project(project.path()).unwrap();
    let second = console_agent::project::compile_project(project.path()).unwrap();
    assert_eq!(first.cart_text, second.cart_text);
    assert_eq!(first.content_id, second.content_id);

    let cart = console_core::Cart::parse(&first.cart_text).unwrap();
    let sections = cart.section_names().collect::<BTreeSet<_>>();
    assert_eq!(
        sections,
        BTreeSet::from([
            "gfx_meta",
            "instruments",
            "lua",
            "map",
            "meta",
            "music",
            "sfx",
            "sprites",
        ])
    );
    assert_eq!(cart.title(), "Source Hopper");
    assert_eq!(first.lua_sources.len(), 4);
    assert_eq!(first.sprite_assets.len(), 4);
    assert!(cart.sprites().iter().any(|pixel| *pixel != 0));
    assert!(cart.map().iter().any(|tile| *tile != 0));
    assert_eq!(cart.instruments().len(), 3);
    assert!(cart.sfx(0).is_some());
    assert!(cart.pattern(0).is_some());

    let built = run(&["build", path(project.path()), "--format", "json"]);
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    assert_eq!(report["status"], "written");
    assert_eq!(report["content_id"], first.content_id);
    assert_eq!(report["lua_sources"].as_array().unwrap().len(), 4);
    assert_eq!(report["sprite_assets"].as_array().unwrap().len(), 4);

    let output_cart = project.path().join("build/source-hopper.cart");
    let published = std::fs::read(&output_cart).unwrap();
    let current = run(&["build", path(project.path()), "--check"]);
    assert!(current.status.success());

    let hud_path = project.path().join("lua/ui/hud.lua");
    let hud = std::fs::read_to_string(&hud_path).unwrap();
    std::fs::write(&hud_path, "return (\n").unwrap();
    let invalid = run(&["build", path(project.path())]);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("lua/ui/hud.lua"));
    assert_eq!(std::fs::read(&output_cart).unwrap(), published);

    let rebuilt_hud = format!("{hud}\n-- deterministic rebuild\n");
    std::fs::write(&hud_path, &rebuilt_hud).unwrap();
    let stale = run(&["build", path(project.path()), "--check"]);
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("is stale"));
    assert_eq!(std::fs::read(&output_cart).unwrap(), published);
    assert!(run(&["build", path(project.path())]).status.success());
    assert!(
        run(&["build", path(project.path()), "--check"])
            .status
            .success()
    );

    let executed = run(&[
        "run",
        path(project.path()),
        "--frames",
        "11",
        "--input",
        "10:R,1:A",
        "--eval",
        "return dev_status()",
    ]);
    assert!(
        executed.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&executed.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&executed.stdout).unwrap();
    assert_eq!(status["x"], 47);
    assert_eq!(status["jumps"], 1);

    let server = LiveServer::spawn(project.path());
    let initial_page = server.fetch();
    assert!(initial_page.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(initial_page.contains("<title>Source Hopper</title>"));
    let live_hud = format!("{rebuilt_hud}\n-- source-hopper-live-refresh\n");
    std::fs::write(&hud_path, &live_hud).unwrap();
    let refreshed_page = server.fetch();
    assert!(refreshed_page.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(refreshed_page.contains("source-hopper-live-refresh"));
    std::fs::write(&hud_path, "return (\n").unwrap();
    let invalid_page = server.fetch();
    assert!(invalid_page.starts_with("HTTP/1.1 500 Internal Server Error\r\n"));
    assert!(!invalid_page.contains("source-hopper-live-refresh"));
    std::fs::write(&hud_path, live_hud).unwrap();
    drop(server);

    let first_artifacts = project.path().join("artifacts-first");
    let second_artifacts = project.path().join("artifacts-second");
    for artifacts in [&first_artifacts, &second_artifacts] {
        let playtest = run(&[
            "playtest",
            path(project.path()),
            "--scenario",
            path(&project.path().join("playtest.json")),
            "--artifacts",
            path(artifacts),
            "--format",
            "json",
        ]);
        assert!(
            playtest.status.success(),
            "playtest failed: {}",
            String::from_utf8_lossy(&playtest.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&playtest.stdout).unwrap();
        assert_eq!(report["scenario"]["status"], "passed");
        assert_eq!(report["scenario"]["artifact_count"], 4);
    }
    for artifact in ["jump.png", "jump.txt", "audio.json", "audio.wav"] {
        assert_eq!(
            std::fs::read(first_artifacts.join(artifact)).unwrap(),
            std::fs::read(second_artifacts.join(artifact)).unwrap(),
            "{artifact} should be deterministic"
        );
    }
    assert!(
        std::fs::read(first_artifacts.join("jump.png"))
            .unwrap()
            .starts_with(b"\x89PNG")
    );
    assert!(
        std::fs::read(first_artifacts.join("audio.wav"))
            .unwrap()
            .starts_with(b"RIFF")
    );
    assert_eq!(
        std::fs::read_to_string(first_artifacts.join("jump.txt"))
            .unwrap()
            .lines()
            .count(),
        320
    );

    let html_path = project.path().join("source-hopper.html");
    let packed = run(&[
        "pack",
        path(&project.path().join("console.toml")),
        "-o",
        path(&html_path),
    ]);
    assert!(
        packed.status.success(),
        "pack failed: {}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let html = std::fs::read_to_string(html_path).unwrap();
    assert!(html.contains("<title>Source Hopper</title>"));
    assert!(html.contains("function dev_status()"));
    assert!(!html.contains("{{CART_TEXT}}"));
}

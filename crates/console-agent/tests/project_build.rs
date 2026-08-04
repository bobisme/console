use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "console-build-cli-test-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("lua")).unwrap();
        std::fs::write(
            path.join("console.toml"),
            "manifest_version = 1\n\n[cart]\ntitle = \"Build Test\"\n\n[lua]\nentry = \"lua/main.lua\"\n\n[build]\noutput = \"dist/test.cart\"\n\n[sections]\nnotes = \"notes.txt\"\n",
        )
        .unwrap();
        std::fs::write(path.join("lua/main.lua"), "function _draw() cls(1) end\n").unwrap();
        std::fs::write(path.join("notes.txt"), "source tree\n").unwrap();
        Self(path)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_console"))
        .args(args)
        .output()
        .expect("run console")
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn build_writes_default_output_and_check_detects_drift() {
    let project = Project::new();
    let output_path = project.0.join("dist/test.cart");

    let built = run(&["build", path(&project.0), "--format", "json"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    assert_eq!(report["status"], "written");
    assert_eq!(report["output"], path(&output_path));
    let first = std::fs::read(&output_path).unwrap();

    let checked = run(&["build", path(&project.0), "--check", "--format", "json"]);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(report["status"], "current");

    std::fs::write(project.0.join("notes.txt"), "changed\n").unwrap();
    let stale = run(&["build", path(&project.0), "--check"]);
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("is stale"));
    assert_eq!(std::fs::read(&output_path).unwrap(), first);
}

#[test]
fn explicit_output_and_text_report_are_supported() {
    let project = Project::new();
    let output = project.0.join("elsewhere.cart");
    let built = run(&[
        "build",
        path(&project.0.join("console.toml")),
        "-o",
        path(&output),
        "--format",
        "text",
    ]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let stdout = String::from_utf8(built.stdout).unwrap();
    assert!(stdout.contains("command=console build"));
    assert!(stdout.contains("status=written"));
    assert!(stdout.contains("content_id=fnv1a64:"));
    assert!(output.exists());
}

#[test]
fn pretty_report_includes_manifest_and_canonical_inputs() {
    let project = Project::new();
    let built = run(&["build", path(&project.0), "--format", "pretty"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let stdout = String::from_utf8(built.stdout).unwrap();
    for expected in [
        std::fs::canonicalize(project.0.join("console.toml")).unwrap(),
        std::fs::canonicalize(project.0.join("lua/main.lua")).unwrap(),
        std::fs::canonicalize(project.0.join("notes.txt")).unwrap(),
    ] {
        assert!(
            stdout.contains(path(&expected)),
            "pretty report omitted {}:\n{stdout}",
            expected.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn configured_output_symlink_is_rejected_without_reading_or_replacing_target() {
    use std::os::unix::fs::symlink;

    let project = Project::new();
    let output = project.0.join("dist/test.cart");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    let outside = project.0.with_extension("outside.cart");
    std::fs::write(&outside, "outside sentinel").unwrap();
    symlink(&outside, &output).unwrap();

    for extra in [&["--check"][..], &[][..]] {
        let mut args = vec!["build", path(&project.0)];
        args.extend_from_slice(extra);
        let built = run(&args);
        assert_eq!(built.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&built.stderr).contains("cannot be a symbolic link"));
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "outside sentinel"
        );
        assert!(
            std::fs::symlink_metadata(&output)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    std::fs::remove_file(outside).unwrap();
}

#[test]
fn build_help_and_usage_errors_follow_cli_conventions() {
    let help = run(&["build", "--help"]);
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert!(String::from_utf8_lossy(&help.stdout).contains("console build <project|console.toml>"));

    let missing = run(&["build"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("requires <project|console.toml>"));
}

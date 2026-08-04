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

    fn write(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
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

#[test]
fn static_modules_preserve_scope_returns_cache_and_source_provenance() {
    let project = Project::new();
    project.write(
        "lua/main.lua",
        "local counter = require('counter')\n\
         local value = require \"nested.value\"\n\
         local nil_a = require('nilmod')\n\
         local nil_b = require('nilmod')\n\
         local false_a = require('falsemod')\n\
         local false_b = require('falsemod')\n\
         function _init()\n\
           result = value.answer + counter.count\n\
           again = require('counter').count\n\
           nil_cached = nil_a == true and nil_b == true and nil_runs == 1\n\
           false_cached = false_a == false and false_b == false and false_runs == 1\n\
         end\n",
    );
    project.write(
        "lua/counter.lua",
        "local hidden = 40\nruns = (runs or 0) + 1\nreturn {count=hidden}\n",
    );
    project.write(
        "lua/nested/value.lua",
        "local counter = require('counter')\nreturn {answer=counter.count + 2}\n",
    );
    project.write("lua/nilmod.lua", "nil_runs = (nil_runs or 0) + 1\n");
    project.write(
        "lua/falsemod.lua",
        "false_runs = (false_runs or 0) + 1\nreturn false\n",
    );

    let compiled = console_agent::project::compile_project(&project.0).unwrap();
    assert_eq!(
        compiled
            .lua_sources
            .iter()
            .map(|source| source.module.as_str())
            .collect::<Vec<_>>(),
        vec!["counter", "falsemod", "nested.value", "nilmod", "<entry>"]
    );
    for source in &compiled.lua_sources {
        assert!(source.source.is_absolute());
        assert!(source.generated_start_line <= source.generated_end_line);
    }

    let console = console_core::Console::new(&compiled.cart_text, 0).unwrap();
    assert_eq!(console.get_global("result").unwrap().as_i64(), Some(82));
    assert_eq!(console.get_global("again").unwrap().as_i64(), Some(40));
    assert_eq!(console.get_global("runs").unwrap().as_i64(), Some(1));
    assert_eq!(
        console.get_global("nil_cached").unwrap().as_boolean(),
        Some(true)
    );
    assert_eq!(
        console.get_global("false_cached").unwrap().as_boolean(),
        Some(true)
    );
    assert!(matches!(
        console.get_global("hidden").unwrap(),
        console_core::mlua::Value::Nil
    ));
    assert!(matches!(
        console.get_global("require").unwrap(),
        console_core::mlua::Value::Nil
    ));
    assert!(matches!(
        console.get_global("package").unwrap(),
        console_core::mlua::Value::Nil
    ));

    let built = run(&["build", path(&project.0), "--format", "json"]);
    assert!(built.status.success());
    let report: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    assert_eq!(report["lua_sources"][0]["module"], "counter");
    assert_eq!(report["lua_sources"][1]["module"], "falsemod");
    assert_eq!(report["lua_sources"][2]["module"], "nested.value");
    assert_eq!(report["lua_sources"][3]["module"], "nilmod");
    assert_eq!(report["lua_sources"][4]["module"], "<entry>");
    assert!(report["lua_sources"][0]["generated_start_line"].is_number());
}

#[test]
fn concatenation_before_global_require_is_bundled() {
    let project = Project::new();
    project.write(
        "lua/main.lua",
        "::load_side_effect:: require('labeled')\n\
         local prefix = 'prefix-\\z\n\
           joined-'\n\
         local message = prefix .. require('suffix')\n\
         function _init() result = message end\n",
    );
    project.write("lua/labeled.lua", "label_loaded = true\n");
    project.write("lua/suffix.lua", "return 'suffix'\n");

    let compiled = console_agent::project::compile_project(&project.0).unwrap();
    assert_eq!(compiled.lua_sources[0].module, "labeled");
    assert_eq!(compiled.lua_sources[1].module, "suffix");
    let console = console_core::Console::new(&compiled.cart_text, 0).unwrap();
    assert_eq!(
        console.get_global("result").unwrap().as_string().unwrap(),
        "prefix-joined-suffix"
    );
    assert_eq!(
        console.get_global("label_loaded").unwrap().as_boolean(),
        Some(true)
    );
}

#[test]
fn module_build_errors_name_dynamic_missing_invalid_and_cyclic_imports() {
    let dynamic = Project::new();
    dynamic.write("lua/main.lua", "local name='counter'\nrequire(name)\n");
    let error = console_agent::project::compile_project(&dynamic.0).unwrap_err();
    assert!(error.contains("uses dynamic require"), "{error}");
    assert!(error.contains("main.lua:2"), "{error}");

    let missing = Project::new();
    missing.write("lua/main.lua", "require('does.not.exist')\n");
    let error = console_agent::project::compile_project(&missing.0).unwrap_err();
    assert!(
        error.contains("cannot resolve module \"does.not.exist\""),
        "{error}"
    );
    assert!(error.contains("does/not/exist.lua"), "{error}");

    let invalid = Project::new();
    invalid.write("lua/main.lua", "require('../escape')\n");
    let error = console_agent::project::compile_project(&invalid.0).unwrap_err();
    assert!(error.contains("invalid module name"), "{error}");

    let cycle = Project::new();
    cycle.write("lua/main.lua", "require('a')\n");
    cycle.write("lua/a.lua", "return require('nested.b')\n");
    cycle.write("lua/nested/b.lua", "return require('a')\n");
    let error = console_agent::project::compile_project(&cycle.0).unwrap_err();
    assert!(error.contains("a -> nested.b -> a"), "{error}");

    let cross_boundary = Project::new();
    cross_boundary.write("lua/main.lua", "require('a')\n");
    cross_boundary.write("lua/a.lua", "if true then\n  require('b')\n");
    cross_boundary.write("lua/b.lua", "end\n");
    let error = console_agent::project::compile_project(&cross_boundary.0).unwrap_err();
    assert!(error.contains("Lua syntax error"), "{error}");
    assert!(error.contains("lua/a.lua:2"), "{error}");
    assert!(error.contains("module a"), "{error}");

    let syntax = Project::new();
    syntax.write("lua/main.lua", "require('broken')\n");
    syntax.write("lua/broken.lua", "local okay = true\nlocal = nope\n");
    let error = console_agent::project::compile_project(&syntax.0).unwrap_err();
    assert!(error.contains("broken.lua:2"), "{error}");
    assert!(error.contains("module broken"), "{error}");
}

#[cfg(unix)]
#[test]
fn module_sources_reject_escape_symlinks_and_duplicate_canonical_files() {
    use std::os::unix::fs::symlink;

    let escaping = Project::new();
    escaping.write("lua/main.lua", "require('escape')\n");
    let outside = escaping.0.with_extension("outside.lua");
    std::fs::write(&outside, "return {}\n").unwrap();
    symlink(&outside, escaping.0.join("lua/escape.lua")).unwrap();
    let error = console_agent::project::compile_project(&escaping.0).unwrap_err();
    assert!(error.contains("escapes project root"), "{error}");
    std::fs::remove_file(outside).unwrap();

    let duplicate = Project::new();
    duplicate.write("lua/main.lua", "require('a')\nrequire('b')\n");
    duplicate.write("lua/a.lua", "return {}\n");
    symlink("a.lua", duplicate.0.join("lua/b.lua")).unwrap();
    let error = console_agent::project::compile_project(&duplicate.0).unwrap_err();
    assert!(error.contains("duplicates Lua source"), "{error}");
    assert!(error.contains("already owned by \"a\""), "{error}");
}

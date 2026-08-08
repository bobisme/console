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

    fn write_bytes(&self, relative: &str, contents: &[u8]) {
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

fn set_sprite_manifest(project: &Project, assets: &str, extra_sections: &str) {
    project.write(
        "console.toml",
        &format!(
            "manifest_version = 1\n\n[cart]\ntitle = \"Sprite Build Test\"\n\n[lua]\nentry = \"lua/main.lua\"\n\n[build]\noutput = \"dist/test.cart\"\n\n{assets}\n[sections]\nnotes = \"notes.txt\"\n{extra_sections}"
        ),
    );
}

fn write_png(project: &Project, relative: &str, rgba: &[u8], width: u32, height: u32) {
    project.write_bytes(
        relative,
        &console_agent::palette::encode_png_rgba(rgba, width, height),
    );
}

fn solid_rgba(width: u32, height: u32, rgb: [u8; 3], alpha: u8) -> Vec<u8> {
    [rgb[0], rgb[1], rgb[2], alpha].repeat((width * height) as usize)
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

#[test]
fn build_expands_a_native_music_bundle_and_tracks_it_as_an_input() {
    let project = Project::new();
    project.write(
        "console.toml",
        "manifest_version = 1\n\
         \n\
         [cart]\n\
         title = \"Native Music Build\"\n\
         \n\
         [lua]\n\
         entry = \"lua/main.lua\"\n\
         \n\
         [audio]\n\
         bundle = \"audio/game.cmusic\"\n\
         \n\
         [build]\n\
         output = \"dist/native.cart\"\n",
    );
    project.write(
        "audio/game.cmusic",
        "console-music 1\n\
         __instruments__\n\
         inst lead wave=6 fm=2,6,5 env=0,12,1 echo=3\n\
         master drive=1 tone=1 hiss=0\n\
         echo delay=10 feedback=3 level=2\n\
         __sfx__\n\
         sfx 0 speed=auto\n\
         C4 lead 6 vib8,2\n\
         E4 lead 6 arp4,7\n\
         __music__\n\
         bpm=120 rows_per_beat=4\n\
         pat 0 loop=0 : 0 - - -\n",
    );

    let built = run(&["build", path(&project.0), "--format", "json"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    let bundle = std::fs::canonicalize(project.0.join("audio/game.cmusic")).unwrap();
    assert!(
        report["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|input| input == path(&bundle))
    );
    let cart_text = std::fs::read_to_string(project.0.join("dist/native.cart")).unwrap();
    let cart = console_core::Cart::parse(&cart_text).unwrap();
    assert!(cart.audio().instrument("lead").is_some());
    assert_eq!(cart.audio().master().drive, 1);
    assert_eq!(cart.audio().echo().delay, 10);
    assert!(cart.audio().pattern(0).is_some());
}

#[test]
fn build_rejects_mixing_audio_bundle_and_raw_audio_sections() {
    let project = Project::new();
    project.write(
        "console.toml",
        "manifest_version = 1\n\
         [cart]\n\
         title = \"Conflicting Music Build\"\n\
         [lua]\n\
         entry = \"lua/main.lua\"\n\
         [audio]\n\
         bundle = \"audio/game.cmusic\"\n\
         [sections]\n\
         music = \"audio/music.txt\"\n",
    );
    project.write(
        "audio/game.cmusic",
        "console-music 1\n__music__\nbpm=120\npat 0 stop : - - - -\n",
    );
    project.write("audio/music.txt", "bpm=120\npat 0 stop : - - - -\n");

    let built = run(&["build", path(&project.0)]);
    assert_eq!(built.status.code(), Some(1));
    let error = String::from_utf8_lossy(&built.stderr);
    assert!(
        error.contains("[audio].bundle cannot be combined"),
        "{error}"
    );
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
fn png_assets_build_a_named_sheet_with_all_mapping_modes_and_authored_animation() {
    use console_core::{PALETTE, SHEET_W};

    let project = Project::new();
    let assets = r#"
[[sprites]]
name = "quant_blob"
source = "art/quant.png"
tile = [4, 2]
mapping = "quantize"
max_colors = 2

[[sprites]]
name = "hero"
source = "art/hero.png"
tile = [1, 2]
anchor = [3, 7]
mapping = "exact"
max_colors = 1

[[sprites]]
name = "nearest_blob"
source = "art/nearest.png"
tile = [3, 2]
mapping = "nearest"
max_colors = 1
"#;
    set_sprite_manifest(&project, assets, "gfx_meta = \"gfx-meta.txt\"\n");
    project.write("gfx-meta.txt", "anim hero.idle frames=0 fps=6 loop\n");

    let mut hero = solid_rgba(8, 8, PALETTE[14], 255);
    hero[..4].copy_from_slice(&[0, 0, 0, 0]);
    write_png(&project, "art/hero.png", &hero, 8, 8);
    write_png(
        &project,
        "art/nearest.png",
        &solid_rgba(8, 8, [1, 2, 3], 255),
        8,
        8,
    );
    let quant_colors = [[240, 10, 20], [10, 240, 20], [10, 20, 240], [220, 180, 40]];
    let mut quant = Vec::new();
    for pixel in 0..128 {
        let rgb = quant_colors[pixel % quant_colors.len()];
        quant.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    write_png(&project, "art/quant.png", &quant, 16, 8);

    let compiled = console_agent::project::compile_project(&project.0).unwrap();
    assert_eq!(
        compiled
            .sprite_assets
            .iter()
            .map(|asset| asset.name.as_str())
            .collect::<Vec<_>>(),
        ["hero", "nearest_blob", "quant_blob"]
    );
    let cart = console_core::Cart::parse(&compiled.cart_text).unwrap();
    let hero_def = cart.gfx_meta().sprite("hero").unwrap();
    assert_eq!(hero_def.rect, (1, 2));
    assert_eq!(hero_def.size, (1, 1));
    assert_eq!(hero_def.anchor, (3, 7));
    assert!(cart.gfx_meta().anim("hero.idle").is_some());
    assert_eq!(cart.sprites()[2 * 8 * SHEET_W + 8], 0);
    assert_eq!(cart.sprites()[2 * 8 * SHEET_W + 9], 14);
    assert_eq!(
        cart.sprites()[2 * 8 * SHEET_W + 3 * 8],
        console_agent::palette::nearest_opaque_index([1, 2, 3])
    );
    let expected_quant = console_agent::palette::quantize_rgba(&quant, 2, 128)
        .unwrap()
        .indices;
    let mut actual_quant = Vec::new();
    for y in 0..8 {
        let start = (2 * 8 + y) * SHEET_W + 4 * 8;
        actual_quant.extend_from_slice(&cart.sprites()[start..start + 16]);
    }
    assert_eq!(actual_quant, expected_quant);
    let quant_asset = compiled
        .sprite_assets
        .iter()
        .find(|asset| asset.name == "quant_blob")
        .unwrap();
    assert_eq!(quant_asset.mapping, "quantize");
    assert!(quant_asset.output_colors <= 2);
    assert_eq!(quant_asset.size_tiles, [2, 1]);

    let first = compiled.cart_text;
    let reordered = r#"
[[sprites]]
name = "nearest_blob"
source = "art/nearest.png"
tile = [3, 2]
mapping = "nearest"
max_colors = 1

[[sprites]]
name = "hero"
source = "art/hero.png"
tile = [1, 2]
anchor = [3, 7]
mapping = "exact"
max_colors = 1

[[sprites]]
name = "quant_blob"
source = "art/quant.png"
tile = [4, 2]
mapping = "quantize"
max_colors = 2
"#;
    set_sprite_manifest(&project, reordered, "gfx_meta = \"gfx-meta.txt\"\n");
    let second = console_agent::project::compile_project(&project.0).unwrap();
    assert_eq!(second.cart_text, first);

    let built = run(&["build", path(&project.0), "--format", "json"]);
    assert!(built.status.success());
    let report: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    assert_eq!(report["sprite_assets"][0]["name"], "hero");
    assert_eq!(report["sprite_assets"][2]["color_budget"], 2);
}

#[test]
fn named_sprite_and_flag_sources_reach_the_last_atlas_tile() {
    use console_core::{PALETTE, SHEET_W, TILE_ID_MAX};

    let project = Project::new();
    set_sprite_manifest(
        &project,
        "[[sprites]]\nname=\"last\"\nsource=\"last.png\"\ntile=[31,31]\nmapping=\"exact\"\n",
        "gfx_flags = \"gfx-flags.txt\"\n",
    );
    write_png(
        &project,
        "last.png",
        &solid_rgba(8, 8, PALETTE[14], 255),
        8,
        8,
    );
    let mut flags = format!("{}\n", "00".repeat(32)).repeat(31);
    flags.push_str(&"00".repeat(31));
    flags.push_str("a5\n");
    project.write("gfx-flags.txt", &flags);

    let compiled = console_agent::project::compile_project(&project.0).unwrap();
    assert_eq!(compiled.sprite_assets[0].tile_ids, vec![TILE_ID_MAX]);
    let cart = console_core::Cart::parse(&compiled.cart_text).unwrap();
    assert_eq!(cart.gfx_flags()[usize::from(TILE_ID_MAX)], 0xa5);
    assert_eq!(cart.sprites()[(SHEET_W - 1) * SHEET_W + (SHEET_W - 1)], 14);
    assert_eq!(cart.gfx_meta().sprite("last").unwrap().rect, (31, 31));

    let flags_pos = compiled.cart_text.find("__gfx_flags__").unwrap();
    let meta_pos = compiled.cart_text.find("__gfx_meta__").unwrap();
    assert!(
        flags_pos < meta_pos,
        "gfx flags keep canonical section order"
    );
}

#[test]
fn png_assets_reject_invalid_dimensions_bounds_duplicates_and_overlaps() {
    use console_core::PALETTE;

    let invalid_size = Project::new();
    set_sprite_manifest(
        &invalid_size,
        "[[sprites]]\nname=\"bad\"\nsource=\"bad.png\"\ntile=[0,0]\n",
        "",
    );
    write_png(
        &invalid_size,
        "bad.png",
        &solid_rgba(7, 8, PALETTE[2], 255),
        7,
        8,
    );
    let error = console_agent::project::compile_project(&invalid_size.0).unwrap_err();
    assert!(error.contains("nonzero multiples of 8"), "{error}");

    let bounds = Project::new();
    set_sprite_manifest(
        &bounds,
        "[[sprites]]\nname=\"wide\"\nsource=\"wide.png\"\ntile=[31,31]\n",
        "",
    );
    write_png(
        &bounds,
        "wide.png",
        &solid_rgba(16, 8, PALETTE[2], 255),
        16,
        8,
    );
    let error = console_agent::project::compile_project(&bounds.0).unwrap_err();
    assert!(error.contains("outside the 32x32 sprite sheet"), "{error}");

    let duplicate = Project::new();
    set_sprite_manifest(
        &duplicate,
        "[[sprites]]\nname=\"same\"\nsource=\"pixel.png\"\ntile=[0,0]\n\n[[sprites]]\nname=\"same\"\nsource=\"pixel.png\"\ntile=[1,0]\n",
        "",
    );
    write_png(
        &duplicate,
        "pixel.png",
        &solid_rgba(8, 8, PALETTE[2], 255),
        8,
        8,
    );
    let error = console_agent::project::compile_project(&duplicate.0).unwrap_err();
    assert!(error.contains("duplicate [[sprites]] name"), "{error}");

    let overlap = Project::new();
    set_sprite_manifest(
        &overlap,
        "[[sprites]]\nname=\"alpha\"\nsource=\"pixel.png\"\ntile=[2,3]\n\n[[sprites]]\nname=\"beta\"\nsource=\"pixel.png\"\ntile=[2,3]\n",
        "",
    );
    write_png(
        &overlap,
        "pixel.png",
        &solid_rgba(8, 8, PALETTE[2], 255),
        8,
        8,
    );
    let error = console_agent::project::compile_project(&overlap.0).unwrap_err();
    assert!(
        error.contains("overlaps sprite \"alpha\" at tile 2,3"),
        "{error}"
    );
}

#[test]
fn png_assets_require_explicit_lossy_conversion_and_enforce_color_budgets() {
    use console_core::PALETTE;

    let arbitrary = Project::new();
    set_sprite_manifest(
        &arbitrary,
        "[[sprites]]\nname=\"bad_rgb\"\nsource=\"bad.png\"\ntile=[0,0]\n",
        "",
    );
    write_png(
        &arbitrary,
        "bad.png",
        &solid_rgba(8, 8, [1, 2, 3], 255),
        8,
        8,
    );
    let error = console_agent::project::compile_project(&arbitrary.0).unwrap_err();
    assert!(error.contains("non-Apollo RGB"), "{error}");

    let opaque_zero = Project::new();
    set_sprite_manifest(
        &opaque_zero,
        "[[sprites]]\nname=\"zero\"\nsource=\"zero.png\"\ntile=[0,0]\n",
        "",
    );
    write_png(
        &opaque_zero,
        "zero.png",
        &solid_rgba(8, 8, PALETTE[0], 255),
        8,
        8,
    );
    let error = console_agent::project::compile_project(&opaque_zero.0).unwrap_err();
    assert!(error.contains("opaque Apollo index 0"), "{error}");

    let budget = Project::new();
    set_sprite_manifest(
        &budget,
        "[[sprites]]\nname=\"too_many\"\nsource=\"two.png\"\ntile=[0,0]\nmax_colors=1\n",
        "",
    );
    let mut two = Vec::new();
    for pixel in 0..64 {
        let rgb = PALETTE[2 + pixel % 2];
        two.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    write_png(&budget, "two.png", &two, 8, 8);
    let error = console_agent::project::compile_project(&budget.0).unwrap_err();
    assert!(error.contains("exceeding max_colors 1"), "{error}");
}

#[test]
fn png_assets_reject_a_competing_raw_sheet_and_duplicate_authored_metadata() {
    use console_core::PALETTE;

    let raw = Project::new();
    set_sprite_manifest(
        &raw,
        "[[sprites]]\nname=\"hero\"\nsource=\"hero.png\"\ntile=[0,0]\n",
        "sprites = \"sprites.txt\"\n",
    );
    write_png(&raw, "hero.png", &solid_rgba(8, 8, PALETTE[2], 255), 8, 8);
    raw.write("sprites.txt", "0\n");
    let error = console_agent::project::compile_project(&raw.0).unwrap_err();
    assert!(
        error.contains("cannot be combined with [[sprites]]"),
        "{error}"
    );

    let duplicate_meta = Project::new();
    set_sprite_manifest(
        &duplicate_meta,
        "[[sprites]]\nname=\"hero\"\nsource=\"hero.png\"\ntile=[0,0]\n",
        "gfx_meta = \"gfx-meta.txt\"\n",
    );
    write_png(
        &duplicate_meta,
        "hero.png",
        &solid_rgba(8, 8, PALETTE[2], 255),
        8,
        8,
    );
    duplicate_meta.write("gfx-meta.txt", "sprite hero rect=1,1 size=1x1\n");
    let error = console_agent::project::compile_project(&duplicate_meta.0).unwrap_err();
    assert!(error.contains("duplicate sprite name \"hero\""), "{error}");
}

#[cfg(unix)]
#[test]
fn png_asset_symlinks_cannot_escape_the_project() {
    use console_core::PALETTE;
    use std::os::unix::fs::symlink;

    let project = Project::new();
    set_sprite_manifest(
        &project,
        "[[sprites]]\nname=\"escape\"\nsource=\"escape.png\"\ntile=[0,0]\n",
        "",
    );
    let outside = project.0.with_extension("outside.png");
    std::fs::write(
        &outside,
        console_agent::palette::encode_png_rgba(&solid_rgba(8, 8, PALETTE[2], 255), 8, 8),
    )
    .unwrap();
    symlink(&outside, project.0.join("escape.png")).unwrap();
    let error = console_agent::project::compile_project(&project.0).unwrap_err();
    assert!(error.contains("escapes project root"), "{error}");
    std::fs::remove_file(outside).unwrap();
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

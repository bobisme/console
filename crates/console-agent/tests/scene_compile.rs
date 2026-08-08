use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use console_core::PALETTE;
use serde_json::{Value, json};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn scratch(name: &str) -> PathBuf {
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "console-scene-{}-{count}-{name}",
        std::process::id()
    ))
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

fn set_cell(rgba: &mut [u8], width: u32, cell_x: u32, cell_y: u32, colors: &[u8; 64]) {
    for y in 0..8 {
        for x in 0..8 {
            let index = colors[(y * 8 + x) as usize];
            let [r, g, b] = PALETTE[index as usize];
            let offset = (((cell_y * 8 + y) * width + cell_x * 8 + x) * 4) as usize;
            rgba[offset..offset + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
}

fn tile(fill: u8, accent: u8) -> [u8; 64] {
    let mut tile = [fill; 64];
    for i in 0..8 {
        tile[i] = accent;
        tile[56 + i] = accent;
        tile[i * 8] = accent;
        tile[i * 8 + 7] = accent;
    }
    tile[27] = accent;
    tile[28] = accent;
    tile[35] = accent;
    tile[36] = accent;
    tile
}

fn write_fixture(root: &Path, mapping: &str, arbitrary_rgb: bool) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    let width = 32;
    let height = 16;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let steel_a = tile(8, 48);
    let cells = [
        steel_a,
        tile(9, 49),
        steel_a,
        tile(14, 63),
        steel_a,
        tile(10, 50),
        tile(12, 31),
        tile(15, 62),
    ];
    for (index, cell) in cells.iter().enumerate() {
        set_cell(&mut rgba, width, index as u32 % 4, index as u32 / 4, cell);
    }
    if arbitrary_rgb {
        rgba[0..4].copy_from_slice(&[1, 2, 3, 255]);
    }
    fs::write(
        root.join("materials.png"),
        console_agent::palette::encode_png_rgba(&rgba, width, height),
    )
    .unwrap();
    fs::write(
        root.join("materials.semantic"),
        "solid solid hazard decor\nsolid solid hazard decor\n",
    )
    .unwrap();
    fs::write(
        root.join("play.grid"),
        "auto:steel_auto auto:steel_auto variant:steel_variant variant:steel_variant acid\nsteel_a steel_b variant:steel_variant variant:steel_variant acid_lip\n. . . . .\n",
    )
    .unwrap();

    let max_colors = if mapping == "quantize" {
        "max_colors = 8\n"
    } else {
        ""
    };
    let manifest = format!(
        r#"scene_version = 1
name = "layered_subset"
seed = 91

[atlas]
origin = [28, 30]
size = [4, 2]
mapping = "{mapping}"
alpha_threshold = 128
{max_colors}
[[classes]]
name = "solid"
solid = true
tags = ["grapple"]

[[classes]]
name = "hazard"
hazard = true
tags = ["damage"]

[[classes]]
name = "decor"
tags = ["background"]

[[layers]]
name = "library"
source = "materials.png"
semantics = "materials.semantic"
role = "library"

[[layers]]
name = "far"
source = "materials.png"
semantics = "materials.semantic"
role = "far"
offset = [0, 0]

[[layers]]
name = "play_base"
source = "materials.png"
semantics = "materials.semantic"
role = "play"
offset = [0, 0]

[[tiles]]
name = "steel_a"
layer = "library"
rect = [0, 0, 8, 8]
class = "solid"
edges = ["*", "*", "*", "*"]

[[tiles]]
name = "steel_b"
layer = "library"
rect = [8, 0, 8, 8]
class = "solid"
edges = ["*", "*", "*", "*"]

[[tiles]]
name = "acid"
layer = "library"
rect = [16, 0, 8, 8]
class = "hazard"
edges = ["*", "*", "*", "*"]

[[tiles]]
name = "lamp_top"
layer = "library"
rect = [24, 0, 8, 8]
class = "decor"
edges = ["*", "*", "*", "*"]

[[tiles]]
name = "steel_alt"
layer = "library"
rect = [0, 8, 8, 8]
class = "solid"
edges = ["*", "*", "*", "*"]

[[tiles]]
name = "steel_corner"
layer = "library"
rect = [8, 8, 8, 8]
class = "solid"
edges = ["*", "*", "*", "*"]

[[tiles]]
name = "acid_lip"
layer = "library"
rect = [16, 8, 8, 8]
class = "hazard"
edges = ["*", "*", "*", "*"]

[[tiles]]
name = "lamp_bottom"
layer = "library"
rect = [24, 8, 8, 8]
class = "decor"
edges = ["*", "*", "*", "*"]

[[metatiles]]
name = "lamp"
rows = ["lamp_top", "lamp_bottom"]

[[autotiles]]
name = "steel_auto"
class = "solid"
lookup = {{"2" = "steel_a", "8" = "steel_b"}}

[[variants]]
name = "steel_variant"
class = "solid"
choices = [{{tile="steel_a",weight=3}}, {{tile="steel_b",weight=1}}]

[play]
grid = "play.grid"
origin = [0, 0]

[[stamps]]
metatile = "lamp"
at = [6, 0]

[[overrides]]
at = [1, 1]
tile = "acid_lip"

[[objects]]
name = "frog_spawn"
kind = "spawn"
at = [16, 16]
anchor = [4, 4]
size = [8, 8]

[[objects]]
name = "fly_pickup"
kind = "pickup"
at = [56, 16]
anchor = [2, 2]
size = [4, 4]
"#
    );
    let manifest_path = root.join("scene.toml");
    fs::write(&manifest_path, manifest).unwrap();
    manifest_path
}

fn compile_scene(manifest: &Path, output: &Path) -> (Output, Value) {
    let result = run(&[
        "scene",
        "compile",
        path(manifest),
        "--out",
        path(output),
        "--format",
        "json",
    ]);
    let report = if result.status.success() {
        serde_json::from_slice(&result.stdout).expect("scene JSON report")
    } else {
        Value::Null
    };
    (result, report)
}

#[test]
fn scene_compile_exercises_semantics_metatiles_autotiles_variants_and_review_outputs() {
    let root = scratch("complete");
    let manifest = write_fixture(&root, "exact", false);
    let first = root.join("generated-a");
    let second = root.join("generated-b");
    let (result, mut report) = compile_scene(&manifest, &first);
    assert!(
        result.status.success(),
        "scene compile failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let (again, mut second_report) = compile_scene(&manifest, &second);
    assert!(
        again.status.success(),
        "second compile failed: {}",
        String::from_utf8_lossy(&again.stderr)
    );

    assert_eq!(report["scene"], "layered_subset");
    assert_eq!(report["mapping"], "exact");
    assert_eq!(report["atlas"]["capacity"], 8);
    assert!(report["atlas"]["used"].as_u64().unwrap() < 8);
    assert!(
        report["atlas"]["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tile| tile["id"].as_u64().unwrap() > 255),
        "fixture must exercise wide tile IDs: {}",
        report["atlas"]["tiles"]
    );
    assert_eq!(report["map"]["autotile_cells"], 2);
    assert_eq!(report["map"]["variant_cells"], 4);
    assert_eq!(report["map"]["stamps"], 1);
    assert_eq!(report["map"]["overrides"], 1);
    assert_eq!(report["objects"].as_array().unwrap().len(), 2);
    assert!(
        report["atlas"]["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tile| { tile["names"] == json!(["steel_a", "steel_alt"]) })
    );
    assert_eq!(
        report["lint"]["semantic_pixel_splits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let split = &report["lint"]["semantic_pixel_splits"][0];
    assert_eq!(split["classes"], json!(["hazard", "solid"]));
    assert_ne!(split["tile_ids"][0], split["tile_ids"][1]);
    assert_eq!(report["layers"][0]["mean_squared_error"], 0.0);
    for value in [&mut report, &mut second_report] {
        value.as_object_mut().unwrap().remove("output");
        value.as_object_mut().unwrap().remove("artifacts");
    }
    assert_eq!(report, second_report);

    let files = [
        "atlas.png",
        "map.txt",
        "tile_classes.lua",
        "decorative_layers.lua",
        "objects.lua",
        "provenance.json",
        "review/atlas.png",
        "review/live-shape.png",
        "review/repeat-3x3.png",
        "review/used-adjacency.png",
        "review/collision.png",
        "review/native-map.png",
    ];
    for file in files {
        assert_eq!(
            fs::read(first.join(file)).unwrap(),
            fs::read(second.join(file)).unwrap(),
            "{file} must be byte-stable"
        );
    }
    let map_text = fs::read_to_string(first.join("map.txt")).unwrap();
    assert!(map_text.starts_with("# map-format=hex3\n"), "{map_text}");
    assert!(map_text.lines().skip(1).any(|line| {
        line.as_bytes().chunks_exact(3).any(|digits| {
            u16::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap() > 255
        })
    }));
    let atlas =
        console_agent::palette::decode_png_rgba(&fs::read(first.join("atlas.png")).unwrap())
            .unwrap();
    assert_eq!((atlas.width, atlas.height), (32, 16));
    for review in [
        "atlas.png",
        "live-shape.png",
        "repeat-3x3.png",
        "used-adjacency.png",
        "collision.png",
        "native-map.png",
    ] {
        console_agent::palette::decode_png_rgba(
            &fs::read(first.join("review").join(review)).unwrap(),
        )
        .unwrap();
    }
    assert!(!first.join("review/lossy-heatmap.png").exists());
    let provenance: Value =
        serde_json::from_slice(&fs::read(first.join("provenance.json")).unwrap()).unwrap();
    assert_eq!(provenance["alpha_threshold"], 128);
    assert_eq!(provenance["max_colors"], Value::Null);
    assert!(
        provenance["generated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "provenance.json")
    );

    let check = run(&[
        "scene",
        "compile",
        path(&manifest),
        "--out",
        path(&first),
        "--check",
        "--format",
        "json",
    ]);
    assert!(check.status.success());
    let check_report: Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(check_report["status"], "current");

    let stale_path = first.join("map.txt");
    fs::write(&stale_path, "intentionally stale\n").unwrap();
    let stale_before = fs::read(&stale_path).unwrap();
    let stale = run(&[
        "scene",
        "compile",
        path(&manifest),
        "--out",
        path(&first),
        "--check",
        "--format",
        "json",
    ]);
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("is stale"));
    assert_eq!(fs::read(&stale_path).unwrap(), stale_before);
}

#[test]
fn generated_scene_assets_are_consumed_by_normal_build_run_and_playtest() {
    let project = scratch("project");
    let manifest = write_fixture(&project, "exact", false);
    let manifest_text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        manifest_text
            .replace("name = \"solid\"", "name = \"1solid\"")
            .replace("class = \"solid\"", "class = \"1solid\"")
            .replace("tags = [\"grapple\"]", "tags = [\"2grapple\"]")
            .replacen("name = \"far\"", "name = \"3far\"", 1),
    )
    .unwrap();
    let semantics = fs::read_to_string(project.join("materials.semantic")).unwrap();
    fs::write(
        project.join("materials.semantic"),
        semantics.replace("solid", "1solid"),
    )
    .unwrap();
    let generated = project.join("generated");
    let (result, _) = compile_scene(&manifest, &generated);
    assert!(result.status.success());
    fs::create_dir_all(project.join("lua")).unwrap();
    fs::write(
        project.join("console.toml"),
        r#"manifest_version = 1
[cart]
title = "Compiled Layered Scene"
[lua]
entry = "lua/main.lua"
root = "."
[[sprites]]
name = "scene_tiles"
source = "generated/atlas.png"
tile = [8, 8]
mapping = "exact"
[sections]
map = "generated/map.txt"
"#,
    )
    .unwrap();
    fs::write(
        project.join("lua/main.lua"),
        r#"local tiles=require("generated.tile_classes")
local layers=require("generated.decorative_layers")
local objects=require("generated.objects")
function _draw() cls(1) map(0,0,0,0,8,4) layers.draw_visible("3far",0,0,8,4) end
function dev_scene_status() return {solid=tiles.is_solid(mget(0,0)),objects=#objects} end
"#,
    )
    .unwrap();
    fs::write(
        project.join("scenario.json"),
        r#"{"version":1,"stages":[{"op":"input","frames":1},{"op":"assert","code":"return dev_scene_status()","equals":{"objects":2,"solid":true}},{"op":"capture","screenshot":"scene.png"}]}"#,
    )
    .unwrap();

    let build = run(&["build", path(&project), "--format", "json"]);
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run_result = run(&["run", path(&project), "--frames", "2", "--screen-text"]);
    assert!(
        run_result.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run_result.stderr)
    );
    let artifacts = project.join("playtest-artifacts");
    let playtest = run(&[
        "playtest",
        path(&project),
        "--scenario",
        path(&project.join("scenario.json")),
        "--artifacts",
        path(&artifacts),
        "--format",
        "json",
    ]);
    assert!(
        playtest.status.success(),
        "playtest failed: {}",
        String::from_utf8_lossy(&playtest.stderr)
    );
    assert!(
        fs::read(artifacts.join("scene.png"))
            .unwrap()
            .starts_with(b"\x89PNG")
    );
}

#[test]
fn lossy_mapping_is_explicit_evidence_backed_and_exact_rejects_non_apollo_pixels() {
    let exact_root = scratch("exact-rejects");
    let exact = write_fixture(&exact_root, "exact", true);
    let exact_out = exact_root.join("generated");
    let (result, _) = compile_scene(&exact, &exact_out);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("non-Apollo RGB"));
    assert!(!exact_out.exists());

    let nearest_root = scratch("nearest");
    let nearest = write_fixture(&nearest_root, "nearest", true);
    let nearest_out = nearest_root.join("generated");
    let (result, report) = compile_scene(&nearest, &nearest_out);
    assert!(
        result.status.success(),
        "nearest compile failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(report["mapping"], "nearest");
    assert!(report["layers"][0]["mean_squared_error"].as_f64().unwrap() > 0.0);
    assert!(nearest_out.join("review/lossy-heatmap.png").exists());

    let transition_root = scratch("lossy-to-exact");
    let transition = write_fixture(&transition_root, "nearest", false);
    let transition_out = transition_root.join("generated");
    let (nearest_result, _) = compile_scene(&transition, &transition_out);
    assert!(nearest_result.status.success());
    let stale_heat = fs::read(transition_out.join("review/lossy-heatmap.png")).unwrap();
    let text = fs::read_to_string(&transition).unwrap();
    fs::write(
        &transition,
        text.replacen("mapping = \"nearest\"", "mapping = \"exact\"", 1),
    )
    .unwrap();
    let (exact_result, _) = compile_scene(&transition, &transition_out);
    assert!(exact_result.status.success());
    assert!(!transition_out.join("review/lossy-heatmap.png").exists());
    fs::write(transition_out.join("review/lossy-heatmap.png"), stale_heat).unwrap();
    let stale = run(&[
        "scene",
        "compile",
        path(&transition),
        "--out",
        path(&transition_out),
        "--check",
    ]);
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("stale managed evidence"));
    let (exact_result, _) = compile_scene(&transition, &transition_out);
    assert!(exact_result.status.success());
    assert!(!transition_out.join("review/lossy-heatmap.png").exists());
    let current = run(&[
        "scene",
        "compile",
        path(&transition),
        "--out",
        path(&transition_out),
        "--check",
    ]);
    assert!(current.status.success());

    let quantize_root = scratch("quantize-report");
    let quantize = write_fixture(&quantize_root, "quantize", false);
    let quantize_out = quantize_root.join("generated");
    let (quantize_result, quantize_report) = compile_scene(&quantize, &quantize_out);
    assert!(quantize_result.status.success());
    assert_eq!(quantize_report["max_colors"], 8);
    assert_eq!(quantize_report["alpha_threshold"], 128);
    let provenance: Value =
        serde_json::from_slice(&fs::read(quantize_out.join("provenance.json")).unwrap()).unwrap();
    assert_eq!(provenance["max_colors"], 8);
    assert_eq!(provenance["alpha_threshold"], 128);
}

#[test]
fn quantize_budget_applies_to_the_union_of_layer_outputs() {
    let root = scratch("quantize-union");
    fs::create_dir_all(&root).unwrap();
    for (name, index) in [("a", 8u8), ("b", 14u8)] {
        let mut rgba = vec![0u8; 8 * 8 * 4];
        set_cell(&mut rgba, 8, 0, 0, &[index; 64]);
        fs::write(
            root.join(format!("{name}.png")),
            console_agent::palette::encode_png_rgba(&rgba, 8, 8),
        )
        .unwrap();
        fs::write(root.join(format!("{name}.semantic")), "solid\n").unwrap();
    }
    fs::write(
        root.join("scene.toml"),
        r#"scene_version = 1
name = "quantize_union"
seed = 1
[atlas]
origin = [1, 0]
size = [2, 1]
mapping = "quantize"
max_colors = 1
[[classes]]
name = "solid"
[[layers]]
name = "a"
source = "a.png"
semantics = "a.semantic"
role = "library"
[[layers]]
name = "b"
source = "b.png"
semantics = "b.semantic"
role = "library"
"#,
    )
    .unwrap();
    let output = root.join("generated");
    let (result, _) = compile_scene(&root.join("scene.toml"), &output);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("quantized layer union uses 2 Apollo64 indices")
    );
    assert!(!output.exists());
}

#[test]
fn scene_lint_reports_illegal_edges_orphans_and_periodic_variants() {
    let root = scratch("lint");
    let manifest = write_fixture(&root, "exact", false);
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replacen("role = \"far\"", "role = \"library\"", 1)
            .replacen("role = \"play\"", "role = \"library\"", 1)
            .replacen(
                "name = \"steel_b\"\nlayer = \"library\"\nrect = [8, 0, 8, 8]\nclass = \"solid\"\nedges = [\"*\", \"*\", \"*\", \"*\"]",
                "name = \"steel_b\"\nlayer = \"library\"\nrect = [8, 0, 8, 8]\nclass = \"solid\"\nedges = [\"closed\", \"closed\", \"closed\", \"closed\"]",
                1,
            )
            .replacen(
                "choices = [{tile=\"steel_a\",weight=3}, {tile=\"steel_b\",weight=1}]",
                "choices = [{tile=\"steel_a\",weight=1}]",
                1,
            ),
    )
    .unwrap();
    fs::write(
        root.join("play.grid"),
        "variant:steel_variant variant:steel_variant variant:steel_variant variant:steel_variant variant:steel_variant variant:steel_variant acid\nsteel_b . steel_b . . . acid_lip\n",
    )
    .unwrap();

    let output = root.join("generated");
    let (result, report) = compile_scene(&manifest, &output);
    assert!(
        result.status.success(),
        "lint fixture failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let warnings = report["lint"]["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|warning| {
            let warning = warning.as_str().unwrap();
            warning.contains("illegal ") && warning.contains(" edge pair")
        }),
        "{warnings:#?}"
    );
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap()
            .contains("orphan corner/endcap tile \"steel_corner\"")
    }));
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap()
            .contains("periodic variant cadence has a repeated run of 6 cells")
    }));
    assert_eq!(report["lint"]["max_variant_run"], 6);
}

#[test]
fn invalid_capacity_paths_masks_bounds_and_anchors_publish_nothing() {
    let cases = [
        (
            "capacity",
            "size = [4, 2]",
            "size = [1, 1]",
            "packing needs",
        ),
        (
            "atlas-overflow",
            "size = [4, 2]",
            "size = [4294967295, 2]",
            "falls outside the 32x32 sheet",
        ),
        (
            "mask",
            "lookup = {\"2\" = \"steel_a\", \"8\" = \"steel_b\"}",
            "lookup = {\"2\" = \"steel_a\"}",
            "no lookup for used four-neighbor mask 8",
        ),
        (
            "map-bounds",
            "origin = [0, 0]",
            "origin = [127, 63]",
            "exceeds the 128x64 map",
        ),
        (
            "anchors",
            "at = [56, 16]",
            "at = [16, 16]",
            "overlaps object",
        ),
        (
            "object-size",
            "size = [8, 8]",
            "size = [4294967295, 8]",
            "width exceeds i32",
        ),
    ];
    for (name, needle, replacement, expected) in cases {
        let root = scratch(name);
        let manifest = write_fixture(&root, "exact", false);
        let text = fs::read_to_string(&manifest).unwrap();
        assert!(text.contains(needle), "fixture missing {needle}");
        fs::write(&manifest, text.replacen(needle, replacement, 1)).unwrap();
        let output = root.join("generated");
        let (result, _) = compile_scene(&manifest, &output);
        assert_eq!(result.status.code(), Some(1), "case {name}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(expected), "case {name}: {stderr}");
        if name == "capacity" {
            assert!(stderr.contains("largest existing reuse groups"));
        }
        assert!(!output.exists(), "case {name} published partial output");
    }

    let root = scratch("path");
    let manifest = write_fixture(&root, "exact", false);
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replacen(
            "source = \"materials.png\"",
            "source = \"../escape.png\"",
            1,
        ),
    )
    .unwrap();
    let output = root.join("generated");
    let (result, _) = compile_scene(&manifest, &output);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("confined relative path"));
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn scene_compile_rejects_input_and_output_symlink_escapes() {
    use std::os::unix::fs::symlink;

    let input_root = scratch("input-symlink");
    let manifest = write_fixture(&input_root, "exact", false);
    let external = scratch("external-input.png");
    fs::copy(input_root.join("materials.png"), &external).unwrap();
    fs::remove_file(input_root.join("materials.png")).unwrap();
    symlink(&external, input_root.join("materials.png")).unwrap();
    let output = input_root.join("generated");
    let (result, _) = compile_scene(&manifest, &output);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("escapes the scene root"));
    assert!(!output.exists());

    let output_root = scratch("output-symlink-source");
    let output_manifest = write_fixture(&output_root, "exact", false);
    let external_output = scratch("external-output");
    fs::create_dir_all(&external_output).unwrap();
    let linked_output = output_root.join("generated");
    symlink(&external_output, &linked_output).unwrap();
    let (result, _) = compile_scene(&output_manifest, &linked_output);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("cannot be a symlink"));
    assert!(fs::read_dir(&external_output).unwrap().next().is_none());
}

#[test]
fn scene_compile_bounds_aggregate_layer_work_before_publication() {
    let root = scratch("aggregate-cells");
    fs::create_dir_all(&root).unwrap();
    let cells_w = 129u32;
    let cells_h = 128u32;
    let width = cells_w * 8;
    let height = cells_h * 8;
    let rgba = vec![0u8; (width * height * 4) as usize];
    fs::write(
        root.join("large.png"),
        console_agent::palette::encode_png_rgba(&rgba, width, height),
    )
    .unwrap();
    let row = std::iter::repeat_n(".", cells_w as usize)
        .collect::<Vec<_>>()
        .join(" ");
    let semantics = std::iter::repeat_n(row, cells_h as usize)
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(root.join("large.semantic"), semantics).unwrap();
    fs::write(
        root.join("scene.toml"),
        r#"scene_version = 1
name = "aggregate_cells"
seed = 1
[atlas]
origin = [1, 0]
size = [1, 1]
[[classes]]
name = "solid"
[[layers]]
name = "library_a"
source = "large.png"
semantics = "large.semantic"
role = "library"
[[layers]]
name = "library_b"
source = "large.png"
semantics = "large.semantic"
role = "library"
"#,
    )
    .unwrap();
    let output = root.join("generated");
    let (result, _) = compile_scene(&root.join("scene.toml"), &output);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("exceeding the 32768-cell safety limit")
    );
    assert!(!output.exists());
}

#[test]
fn output_component_type_errors_are_preflighted_before_any_replacement() {
    let root = scratch("output-component-type");
    let manifest = write_fixture(&root, "exact", false);
    let output = root.join("generated");
    let (first, _) = compile_scene(&manifest, &output);
    assert!(first.status.success());
    fs::remove_dir_all(output.join("review")).unwrap();
    fs::write(output.join("review"), "not a directory\n").unwrap();
    fs::write(output.join("atlas.png"), "sentinel atlas bytes\n").unwrap();
    let before = fs::read(output.join("atlas.png")).unwrap();

    let (result, _) = compile_scene(&manifest, &output);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("wrong file type"));
    assert_eq!(fs::read(output.join("atlas.png")).unwrap(), before);
}

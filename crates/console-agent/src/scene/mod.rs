//! Deterministic layered-scene, semantic tile, and metatile compilation.

mod compiler;
mod review;

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::palette::{ReportFormat, parse_report_format};

pub const COMMANDS: &[&str] = &["compile"];

pub const SCENE_USAGE: &str = "\
usage:
  console scene compile <scene.toml> --out <directory> [--check] [--format text|pretty|json]

Compiles versioned Apollo64 PNG layers and semantic grids into:
  atlas.png, map.txt, tile_classes.lua, decorative_layers.lua, objects.lua,
  provenance.json, and labeled review/*.png evidence.

The output directory must be explicit. --check compares every expected byte
without writing. Inputs are never resized or filtered; lossy palette mapping
must be selected explicitly in the manifest.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SceneManifest {
    pub scene_version: u32,
    pub name: String,
    pub seed: u64,
    pub atlas: AtlasConfig,
    pub classes: Vec<ClassConfig>,
    pub layers: Vec<LayerConfig>,
    #[serde(default)]
    pub tiles: Vec<TileConfig>,
    #[serde(default)]
    pub metatiles: Vec<MetatileConfig>,
    #[serde(default)]
    pub autotiles: Vec<AutotileConfig>,
    #[serde(default)]
    pub variants: Vec<VariantConfig>,
    #[serde(default)]
    pub play: PlayConfig,
    #[serde(default)]
    pub stamps: Vec<StampConfig>,
    #[serde(default)]
    pub overrides: Vec<OverrideConfig>,
    #[serde(default)]
    pub objects: Vec<ObjectConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AtlasConfig {
    pub origin: [u32; 2],
    pub size: [u32; 2],
    #[serde(default)]
    pub mapping: PaletteMapping,
    #[serde(default = "default_alpha_threshold")]
    pub alpha_threshold: u8,
    pub max_colors: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PaletteMapping {
    #[default]
    Exact,
    Nearest,
    Quantize,
}

impl PaletteMapping {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Nearest => "nearest",
            Self::Quantize => "quantize",
        }
    }
}

fn default_alpha_threshold() -> u8 {
    128
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClassConfig {
    pub name: String,
    #[serde(default)]
    pub solid: bool,
    #[serde(default)]
    pub hazard: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LayerRole {
    Far,
    Mid,
    Play,
    Foreground,
    Library,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LayerConfig {
    pub name: String,
    pub source: PathBuf,
    pub semantics: PathBuf,
    pub role: LayerRole,
    #[serde(default)]
    pub offset: [u32; 2],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TileConfig {
    pub name: String,
    pub layer: String,
    /// Pixel-space source rectangle. Version 1 named tiles are exactly 8x8.
    pub rect: [u32; 4],
    pub class: String,
    pub edges: Option<[String; 4]>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetatileConfig {
    pub name: String,
    pub rows: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AutotileConfig {
    pub name: String,
    pub class: String,
    /// Four-neighbor mask table: N=1, E=2, S=4, W=8.
    pub lookup: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VariantConfig {
    pub name: String,
    pub class: String,
    pub choices: Vec<WeightedChoice>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WeightedChoice {
    pub tile: String,
    pub weight: u32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PlayConfig {
    pub grid: Option<PathBuf>,
    pub origin: [u32; 2],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StampConfig {
    pub metatile: String,
    pub at: [u32; 2],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverrideConfig {
    pub at: [u32; 2],
    pub tile: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectConfig {
    pub name: String,
    pub kind: String,
    /// World-pixel anchor position.
    pub at: [i32; 2],
    #[serde(default)]
    pub anchor: [i32; 2],
    #[serde(default = "default_object_size")]
    pub size: [u32; 2],
}

fn default_object_size() -> [u32; 2] {
    [1, 1]
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SceneReport {
    pub command: &'static str,
    pub manifest: String,
    pub output: String,
    pub status: &'static str,
    pub scene: String,
    pub seed: u64,
    pub mapping: &'static str,
    pub alpha_threshold: u8,
    pub max_colors: Option<usize>,
    pub atlas: AtlasReport,
    pub map: MapReport,
    pub layers: Vec<LayerReport>,
    pub classes: Vec<ClassConfig>,
    pub objects: Vec<ObjectReport>,
    pub lint: LintReport,
    pub inputs: Vec<String>,
    pub artifacts: Vec<ArtifactSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AtlasReport {
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub capacity: usize,
    pub used: usize,
    pub available: usize,
    pub palette_indices: Vec<u8>,
    pub tiles: Vec<PackedTileReport>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PackedTileReport {
    pub id: console_core::TileId,
    pub atlas_cell: [u32; 2],
    pub class: String,
    pub names: Vec<String>,
    pub sources: Vec<String>,
    pub edges: [String; 4],
    pub blank: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MapReport {
    pub used_width: u32,
    pub used_height: u32,
    pub nonzero_cells: usize,
    pub autotile_cells: usize,
    pub variant_cells: usize,
    pub stamps: usize,
    pub overrides: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LayerReport {
    pub name: String,
    pub role: LayerRole,
    pub source: String,
    pub semantics: String,
    pub size_cells: [u32; 2],
    pub offset: [u32; 2],
    pub nonempty_cells: usize,
    pub source_colors: usize,
    pub palette_indices: Vec<u8>,
    pub partial_alpha_pixels: usize,
    pub mean_squared_error: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ObjectReport {
    pub name: String,
    pub kind: String,
    pub at: [i32; 2],
    pub anchor: [i32; 2],
    pub bounds: [i32; 4],
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct LintReport {
    pub warnings: Vec<String>,
    pub semantic_pixel_splits: Vec<SemanticSplit>,
    pub unused_named_tiles: Vec<String>,
    pub used_adjacencies: Vec<AdjacencyReport>,
    pub max_variant_run: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SemanticSplit {
    pub classes: Vec<String>,
    pub tile_ids: Vec<console_core::TileId>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AdjacencyReport {
    pub a: console_core::TileId,
    pub b: console_core::TileId,
    pub direction: &'static str,
    pub legal: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArtifactSummary {
    pub path: String,
    pub bytes: usize,
}

pub(crate) struct GeneratedFile {
    pub relative: &'static str,
    pub bytes: Vec<u8>,
}

const MANAGED_OPTIONAL_ARTIFACTS: &[&str] = &["review/lossy-heatmap.png"];

struct Args {
    manifest: PathBuf,
    output: PathBuf,
    check: bool,
    format: ReportFormat,
}

pub fn cli_scene(args: &[String]) -> i32 {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        println!("{SCENE_USAGE}");
        return 0;
    }
    let options = match parse_args(args) {
        Ok(options) => options,
        Err(error) => return usage_error(error),
    };
    let mut build = match compiler::compile(&options.manifest) {
        Ok(build) => build,
        Err(error) => return compile_error(error),
    };
    let status = match publish(&options.output, &build.files, options.check) {
        Ok(status) => status,
        Err(error) => return compile_error(error),
    };
    build.report.status = status;
    build.report.output = options.output.display().to_string();
    build.report.artifacts = build
        .files
        .iter()
        .map(|file| ArtifactSummary {
            path: options.output.join(file.relative).display().to_string(),
            bytes: file.bytes.len(),
        })
        .collect();
    print_report(&build.report, options.format);
    0
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    if args.first().map(String::as_str) != Some("compile") {
        return Err("console scene requires the `compile` command".to_string());
    }
    let mut manifest = None;
    let mut output = None;
    let mut check = false;
    let mut format = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "-o" | "--out" => {
                index += 1;
                output = Some(PathBuf::from(
                    args.get(index).ok_or("--out requires a directory")?,
                ));
            }
            "--check" => check = true,
            "--format" => {
                index += 1;
                let value = args.get(index).ok_or("--format requires a value")?;
                format = Some(parse_report_format(value, "--format")?);
            }
            "--json" => format = Some(ReportFormat::Json),
            flag if flag.starts_with('-') => {
                return Err(format!("unknown console scene flag {flag:?}"));
            }
            value if manifest.is_none() => manifest = Some(PathBuf::from(value)),
            value => return Err(format!("unexpected argument {value:?}")),
        }
        index += 1;
    }
    let format = match format {
        Some(format) => format,
        None => match std::env::var("FORMAT").as_deref() {
            Ok("text") => ReportFormat::Text,
            Ok("pretty") => ReportFormat::Pretty,
            Ok("json") => ReportFormat::Json,
            Ok(other) => {
                return Err(format!(
                    "FORMAT must be text, pretty, or json, got {other:?}"
                ));
            }
            Err(_) if std::io::stdout().is_terminal() => ReportFormat::Pretty,
            Err(_) => ReportFormat::Text,
        },
    };
    Ok(Args {
        manifest: manifest.ok_or("console scene compile requires <scene.toml>")?,
        output: output.ok_or("console scene compile requires --out <directory>")?,
        check,
        format,
    })
}

fn publish(root: &Path, files: &[GeneratedFile], check: bool) -> Result<&'static str, String> {
    validate_output_root(root, files)?;
    let expected = files
        .iter()
        .map(|file| file.relative)
        .collect::<std::collections::BTreeSet<_>>();
    if check {
        for file in files {
            let path = root.join(file.relative);
            match std::fs::read(&path) {
                Ok(current) if current == file.bytes => {}
                Ok(_) => {
                    return Err(format!(
                        "{} is stale; rerun console scene compile without --check",
                        path.display()
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(format!(
                        "{} does not exist; rerun console scene compile without --check",
                        path.display()
                    ));
                }
                Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
            }
        }
        for relative in MANAGED_OPTIONAL_ARTIFACTS {
            if !expected.contains(relative) {
                let path = root.join(relative);
                match std::fs::symlink_metadata(&path) {
                    Ok(_) => {
                        return Err(format!(
                            "{} is stale managed evidence; rerun console scene compile without --check",
                            path.display()
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("cannot inspect {}: {error}", path.display()));
                    }
                }
            }
        }
        return Ok("current");
    }
    // Compilation and path validation finish before the first publication.
    // Each individual artifact is replaced atomically in its destination.
    for file in files {
        atomic_write(&root.join(file.relative), &file.bytes)?;
    }
    for relative in MANAGED_OPTIONAL_ARTIFACTS {
        if !expected.contains(relative) {
            let path = root.join(relative);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("removing stale {}: {error}", path.display())),
            }
        }
    }
    Ok("written")
}

fn validate_output_root(root: &Path, files: &[GeneratedFile]) -> Result<(), String> {
    if root.as_os_str().is_empty() {
        return Err("--out directory cannot be empty".to_string());
    }
    if let Ok(metadata) = std::fs::symlink_metadata(root) {
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "output root {} cannot be a symlink",
                root.display()
            ));
        }
        if !metadata.is_dir() {
            return Err(format!("output root {} is not a directory", root.display()));
        }
    }
    let mut unique = std::collections::BTreeSet::new();
    for relative in files
        .iter()
        .map(|file| file.relative)
        .chain(MANAGED_OPTIONAL_ARTIFACTS.iter().copied())
    {
        if !unique.insert(relative) && !MANAGED_OPTIONAL_ARTIFACTS.contains(&relative) {
            return Err(format!("internal duplicate output path {:?}", relative));
        }
        let mut cursor = root.to_path_buf();
        let components = Path::new(relative).components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let std::path::Component::Normal(part) = component else {
                return Err(format!("internal unsafe output path {relative:?}"));
            };
            cursor.push(part);
            if let Ok(metadata) = std::fs::symlink_metadata(&cursor) {
                if metadata.file_type().is_symlink() {
                    return Err(format!(
                        "output path {} traverses a symlink",
                        cursor.display()
                    ));
                }
                let leaf = index + 1 == components.len();
                if (!leaf && !metadata.is_dir()) || (leaf && metadata.is_dir()) {
                    return Err(format!(
                        "output path {} has the wrong file type",
                        cursor.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    let parent = path
        .parent()
        .ok_or_else(|| format!("output {} has no parent", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("output {} has a non-UTF-8 name", path.display()))?;
    let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), nonce));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("creating temporary {}: {error}", temporary.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("writing temporary {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("syncing temporary {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, path).map_err(|error| {
            format!(
                "replacing {} from {}: {error}",
                path.display(),
                temporary.display()
            )
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn print_report(report: &SceneReport, format: ReportFormat) {
    match format {
        ReportFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(report).expect("scene report is serializable")
        ),
        ReportFormat::Text | ReportFormat::Pretty => println!(
            "{}  {}  mapping={}  alpha={}  max_colors={}  atlas={}/{}  map={}x{}  artifacts={}  warnings={}",
            report.scene,
            report.status,
            report.mapping,
            report.alpha_threshold,
            report
                .max_colors
                .map_or_else(|| "-".to_string(), |colors| colors.to_string()),
            report.atlas.used,
            report.atlas.capacity,
            report.map.used_width,
            report.map.used_height,
            report.artifacts.len(),
            report.lint.warnings.len()
        ),
    }
}

fn usage_error(error: String) -> i32 {
    eprintln!("error: {error}");
    eprintln!("{SCENE_USAGE}");
    2
}

fn compile_error(error: String) -> i32 {
    eprintln!("error: {error}");
    1
}

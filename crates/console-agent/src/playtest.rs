//! Declarative, deterministic playtest scenarios for agent-authored carts.
//!
//! A scenario is strict, versioned JSON containing ordered stages. It is a
//! thin orchestration layer over [`Session`]: no second stepping engine, no
//! hidden wall clock, and no browser-only semantics.

use std::collections::BTreeSet;
use std::fs;
use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};

use console_core::{SCREEN_H, SCREEN_W, input};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::artifact;
use crate::session::Session;
use crate::value::lua_to_json;

const MAX_SCREENSHOT_ZOOM: u32 = 16;
const MAX_SPECTROGRAM_CELL: u32 = 8;
const MAX_SCENARIO_FRAMES: u64 = 36_000;
const MAX_SPECTROGRAM_FRAMES: u64 = 3_600;

pub const USAGE: &str = "\
Run an ordered, deterministic cart playtest scenario

Usage:
  console playtest <cart> --scenario <scenario.json> [OPTIONS]

Options:
  --artifacts <DIR>  Root for capture paths (required when capturing files)
  --seed <N>         Override the scenario seed
  --format <FORMAT>  Output format: text|pretty|json [default: auto]
  -h, --help         Print this help

Scenario format (version 1):
  {\"version\":1,\"seed\":0,\"stages\":[
    {\"op\":\"input\",\"frames\":1,\"buttons\":\"A\"},
    {\"op\":\"eval\",\"code\":\"dev_warp(48,449)\"},
    {\"op\":\"assert\",\"code\":\"return dev_status().embers\",\"equals\":1},
    {\"op\":\"capture\",\"screenshot\":\"scene.png\",\"zoom\":4}
  ]}

Exit codes:
  0  all stages passed
  1  a stage assertion, execution, or capture failed
  2  invalid CLI arguments or scenario schema

Agent workflow:
  console playtest carts/game.cart --scenario playtests/game.json \
    --artifacts /tmp/game-playtest --format json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Text,
    Pretty,
    Json,
}

#[derive(Debug)]
struct Args {
    cart: PathBuf,
    scenario: PathBuf,
    artifacts: Option<PathBuf>,
    seed: Option<u64>,
    format: OutputFormat,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub version: u32,
    #[serde(default)]
    pub seed: u64,
    pub stages: Vec<Stage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Stage {
    Eval {
        #[serde(default)]
        name: Option<String>,
        code: String,
    },
    Input {
        #[serde(default)]
        name: Option<String>,
        frames: u64,
        #[serde(default)]
        buttons: String,
    },
    Assert {
        #[serde(default)]
        name: Option<String>,
        code: String,
        equals: Json,
    },
    Capture {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        screenshot: Option<String>,
        #[serde(default = "default_zoom")]
        zoom: u32,
        #[serde(default)]
        screen_text: Option<String>,
        #[serde(default)]
        wav: Option<String>,
        #[serde(default)]
        spectrogram: Option<String>,
        #[serde(default)]
        audio_events: Option<String>,
        #[serde(default)]
        audio_stats: Option<String>,
        #[serde(default)]
        text_events: Option<String>,
        #[serde(default)]
        from_frame: Option<u64>,
        #[serde(default)]
        to_frame: Option<u64>,
        #[serde(default = "default_window_frames")]
        window_frames: u64,
        #[serde(default = "default_cell")]
        cell: u32,
    },
}

fn default_zoom() -> u32 {
    1
}

fn default_window_frames() -> u64 {
    6
}

fn default_cell() -> u32 {
    4
}

impl Stage {
    fn op(&self) -> &'static str {
        match self {
            Stage::Eval { .. } => "eval",
            Stage::Input { .. } => "input",
            Stage::Assert { .. } => "assert",
            Stage::Capture { .. } => "capture",
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Stage::Eval { name, .. }
            | Stage::Input { name, .. }
            | Stage::Assert { name, .. }
            | Stage::Capture { name, .. } => name.as_deref(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PlaytestReport {
    pub scenario: ScenarioSummary,
    pub stages: Vec<StageReport>,
    pub advice: Vec<Advice>,
}

#[derive(Debug, Serialize)]
pub struct ScenarioSummary {
    pub path: String,
    pub cart: String,
    pub version: u32,
    pub seed: u64,
    pub status: &'static str,
    pub frame_count: u64,
    pub stage_count: usize,
    pub artifact_count: usize,
}

#[derive(Debug, Serialize)]
pub struct StageReport {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub op: &'static str,
    pub status: &'static str,
    pub frame_start: u64,
    pub frame_end: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub logs: Vec<String>,
    pub artifacts: Vec<ArtifactReport>,
}

#[derive(Debug, Serialize)]
pub struct ArtifactReport {
    pub kind: &'static str,
    pub path: String,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct Advice {
    pub level: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub message: String,
}

pub fn cli_playtest(args: &[String]) -> i32 {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        println!("{USAGE}");
        return 0;
    }
    let args = match parse_args(args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("{USAGE}");
            return 2;
        }
    };

    let report = match run_scenario(
        &args.cart,
        &args.scenario,
        args.artifacts.as_deref(),
        args.seed,
    ) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    print_report(&report, args.format);
    if report.scenario.status == "passed" {
        0
    } else {
        if let Some(stage) = report.stages.last()
            && let Some(error) = &stage.error
        {
            eprintln!("playtest: stage {} failed: {error}", stage.index);
        }
        1
    }
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut cart = None;
    let mut scenario = None;
    let mut artifacts = None;
    let mut seed = None;
    let mut format_flag = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = arg
            .split_once('=')
            .filter(|(flag, _)| flag.starts_with("--"))
            .map_or((arg.as_str(), None), |(flag, value)| (flag, Some(value)));
        let value = |label: &str, index: &mut usize| -> Result<String, String> {
            if let Some(value) = inline {
                return Ok(value.to_string());
            }
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| format!("{label} requires a value"))
        };
        match flag {
            "--scenario" => scenario = Some(PathBuf::from(value("--scenario", &mut index)?)),
            "--artifacts" => {
                artifacts = Some(PathBuf::from(value("--artifacts", &mut index)?));
            }
            "--seed" => {
                let raw = value("--seed", &mut index)?;
                seed = Some(
                    raw.parse()
                        .map_err(|_| format!("invalid --seed value {raw:?}"))?,
                );
            }
            "--format" => {
                let raw = value("--format", &mut index)?;
                format_flag = Some(match raw.as_str() {
                    "text" => OutputFormat::Text,
                    "pretty" => OutputFormat::Pretty,
                    "json" => OutputFormat::Json,
                    _ => {
                        return Err(format!(
                            "--format must be text, pretty, or json, got {raw:?}"
                        ));
                    }
                });
            }
            "--json" => format_flag = Some(OutputFormat::Json),
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other:?}"));
            }
            _ if cart.is_none() => cart = Some(PathBuf::from(arg)),
            _ => return Err(format!("unexpected extra argument {arg:?}")),
        }
        index += 1;
    }
    Ok(Args {
        cart: cart.ok_or("missing <cart> argument")?,
        scenario: scenario.ok_or("missing --scenario <scenario.json>")?,
        artifacts,
        seed,
        format: resolve_format(format_flag)?,
    })
}

fn resolve_format(flag: Option<OutputFormat>) -> Result<OutputFormat, String> {
    if let Some(format) = flag {
        return Ok(format);
    }
    if let Ok(raw) = std::env::var("FORMAT") {
        return match raw.as_str() {
            "text" => Ok(OutputFormat::Text),
            "pretty" => Ok(OutputFormat::Pretty),
            "json" => Ok(OutputFormat::Json),
            _ => Err(format!("FORMAT must be text, pretty, or json, got {raw:?}")),
        };
    }
    Ok(if std::io::stdout().is_terminal() {
        OutputFormat::Pretty
    } else {
        OutputFormat::Text
    })
}

pub fn run_scenario(
    cart_path: &Path,
    scenario_path: &Path,
    artifacts: Option<&Path>,
    seed_override: Option<u64>,
) -> Result<PlaytestReport, String> {
    let scenario_text = fs::read_to_string(scenario_path)
        .map_err(|error| format!("reading {}: {error}", scenario_path.display()))?;
    let scenario: Scenario = serde_json::from_str(&scenario_text)
        .map_err(|error| format!("parsing {}: {error}", scenario_path.display()))?;
    validate_scenario(&scenario, artifacts)?;

    let cart_text = fs::read_to_string(cart_path)
        .map_err(|error| format!("reading {}: {error}", cart_path.display()))?;
    let seed = seed_override.unwrap_or(scenario.seed);
    let mut session = Session::new();
    session
        .load_cart(&cart_text, seed)
        .map_err(|error| format!("loading {}: {error}", cart_path.display()))?;

    let artifact_root = match artifacts {
        Some(root) => {
            fs::create_dir_all(root)
                .map_err(|error| format!("creating artifact root {}: {error}", root.display()))?;
            Some(
                fs::canonicalize(root).map_err(|error| {
                    format!("resolving artifact root {}: {error}", root.display())
                })?,
            )
        }
        None => None,
    };

    let mut reports = Vec::with_capacity(scenario.stages.len());
    let mut artifact_count = 0usize;
    let mut passed = true;
    for (index, stage) in scenario.stages.iter().enumerate() {
        let frame_start = frame_count(&session);
        let mut report = StageReport {
            index,
            name: stage.name().map(str::to_string),
            op: stage.op(),
            status: "passed",
            frame_start,
            frame_end: frame_start,
            expected: None,
            actual: None,
            error: None,
            logs: Vec::new(),
            artifacts: Vec::new(),
        };
        if let Err(error) =
            execute_stage(stage, &mut session, artifact_root.as_deref(), &mut report)
        {
            report.status = "failed";
            report.error = Some(error);
            passed = false;
        }
        report.frame_end = frame_count(&session);
        report.logs = session.logs().unwrap_or_default();
        artifact_count += report.artifacts.len();
        reports.push(report);
        if !passed {
            break;
        }
    }

    Ok(PlaytestReport {
        scenario: ScenarioSummary {
            path: scenario_path.display().to_string(),
            cart: cart_path.display().to_string(),
            version: scenario.version,
            seed,
            status: if passed { "passed" } else { "failed" },
            frame_count: frame_count(&session),
            stage_count: reports.len(),
            artifact_count,
        },
        stages: reports,
        advice: Vec::new(),
    })
}

fn validate_scenario(scenario: &Scenario, artifacts: Option<&Path>) -> Result<(), String> {
    if scenario.version != 1 {
        return Err(format!(
            "unsupported scenario version {}; expected 1",
            scenario.version
        ));
    }
    if scenario.stages.is_empty() {
        return Err("scenario has no stages".to_string());
    }
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut stepped_frames = 0u64;
    for (index, stage) in scenario.stages.iter().enumerate() {
        if let Some(name) = stage.name() {
            if name.trim().is_empty() {
                return Err(format!("stage {index} has an empty name"));
            }
            if !names.insert(name) {
                return Err(format!("stage {index} repeats name {name:?}"));
            }
        }
        match stage {
            Stage::Input {
                frames, buttons, ..
            } => {
                if *frames == 0 {
                    return Err(format!("stage {index} input frames must be >= 1"));
                }
                input::parse(buttons).map_err(|button| {
                    format!("stage {index} has unknown input button {button:?} in {buttons:?}")
                })?;
                stepped_frames = stepped_frames
                    .checked_add(*frames)
                    .ok_or_else(|| format!("stage {index} input frame total overflows u64"))?;
                if stepped_frames > MAX_SCENARIO_FRAMES {
                    return Err(format!(
                        "stage {index} brings the scenario to {stepped_frames} frames; version 1 allows at most {MAX_SCENARIO_FRAMES}"
                    ));
                }
            }
            Stage::Capture {
                screenshot,
                zoom,
                screen_text,
                wav,
                spectrogram,
                audio_events,
                audio_stats,
                text_events,
                from_frame,
                to_frame,
                window_frames,
                cell,
                ..
            } => {
                let outputs = [
                    screenshot.as_deref(),
                    screen_text.as_deref(),
                    wav.as_deref(),
                    spectrogram.as_deref(),
                    audio_events.as_deref(),
                    audio_stats.as_deref(),
                    text_events.as_deref(),
                ];
                if outputs.iter().all(Option::is_none) {
                    return Err(format!("stage {index} capture has no outputs"));
                }
                if artifacts.is_none() {
                    return Err(format!(
                        "stage {index} captures files, so --artifacts <DIR> is required"
                    ));
                }
                if !(1..=MAX_SCREENSHOT_ZOOM).contains(zoom) {
                    return Err(format!(
                        "stage {index} capture zoom must be 1..={MAX_SCREENSHOT_ZOOM}, got {zoom}"
                    ));
                }
                if !(1..=MAX_SCENARIO_FRAMES).contains(window_frames) {
                    return Err(format!(
                        "stage {index} capture window_frames must be 1..={MAX_SCENARIO_FRAMES}, got {window_frames}"
                    ));
                }
                if !(1..=MAX_SPECTROGRAM_CELL).contains(cell) {
                    return Err(format!(
                        "stage {index} capture cell must be 1..={MAX_SPECTROGRAM_CELL}, got {cell}"
                    ));
                }
                validate_capture_dimensions(*zoom, *cell).map_err(|error| {
                    format!("stage {index} capture dimensions are invalid: {error}")
                })?;
                if spectrogram.is_some() {
                    let from = from_frame.unwrap_or(0).min(stepped_frames);
                    let to = to_frame
                        .unwrap_or(stepped_frames)
                        .clamp(from, stepped_frames);
                    if to - from > MAX_SPECTROGRAM_FRAMES {
                        return Err(format!(
                            "stage {index} spectrogram covers {} frames; version 1 allows at most {MAX_SPECTROGRAM_FRAMES}",
                            to - from
                        ));
                    }
                }
                for output in outputs.into_iter().flatten() {
                    let normalized = normalize_relative_path(output).map_err(|error| {
                        format!("stage {index} capture path {output:?}: {error}")
                    })?;
                    if !paths.insert(duplicate_path_key(&normalized)) {
                        return Err(format!(
                            "stage {index} capture path {output:?} aliases an earlier artifact; every normalized path must be unique"
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_capture_dimensions(zoom: u32, cell: u32) -> Result<(), String> {
    let screen_width = (SCREEN_W as u32)
        .checked_mul(zoom)
        .ok_or("screenshot width overflow")?;
    let screen_height = (SCREEN_H as u32)
        .checked_mul(zoom)
        .ok_or("screenshot height overflow")?;
    let _screen_bytes = usize::try_from(screen_width)
        .ok()
        .zip(usize::try_from(screen_height).ok())
        .and_then(|(width, height)| width.checked_mul(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("screenshot byte size overflow")?;

    let windows = MAX_SPECTROGRAM_FRAMES.div_ceil(3).max(1);
    let spectrogram_width = u32::try_from(windows)
        .ok()
        .and_then(|windows| windows.checked_mul(cell))
        .ok_or("spectrogram width overflow")?;
    let spectrogram_height = 96u32
        .checked_mul(cell)
        .ok_or("spectrogram height overflow")?;
    let _spectrogram_bytes = usize::try_from(spectrogram_width)
        .ok()
        .zip(usize::try_from(spectrogram_height).ok())
        .and_then(|(width, height)| width.checked_mul(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("spectrogram byte size overflow")?;
    Ok(())
}

fn execute_stage(
    stage: &Stage,
    session: &mut Session,
    artifact_root: Option<&Path>,
    report: &mut StageReport,
) -> Result<(), String> {
    match stage {
        Stage::Eval { code, .. } => {
            let value = session.eval(code).map_err(|error| error.to_string())?;
            report.actual = Some(lua_to_json(&value));
        }
        Stage::Input {
            frames, buttons, ..
        } => {
            let mask = input::parse(buttons)
                .map_err(|button| format!("unknown input button {button:?} in {buttons:?}"))?;
            let outcome = session
                .step(*frames, mask)
                .map_err(|error| error.to_string())?;
            if outcome.halted {
                return Err(format!(
                    "cart halted at frame {}: {}",
                    outcome.frame_count,
                    outcome
                        .message
                        .unwrap_or_else(|| "unknown error".to_string())
                ));
            }
            report.actual = Some(json!({"frames": frames, "buttons": buttons, "mask": mask}));
        }
        Stage::Assert { code, equals, .. } => {
            let value = session.eval(code).map_err(|error| error.to_string())?;
            let actual = lua_to_json(&value);
            report.expected = Some(equals.clone());
            report.actual = Some(actual.clone());
            if actual != *equals {
                return Err(format!(
                    "assertion failed: expected {}, got {}",
                    equals, actual
                ));
            }
        }
        Stage::Capture {
            screenshot,
            zoom,
            screen_text,
            wav,
            spectrogram,
            audio_events,
            audio_stats,
            text_events,
            from_frame,
            to_frame,
            window_frames,
            cell,
            ..
        } => {
            let root = artifact_root.expect("capture validation requires an artifact root");
            if let Some(name) = screenshot {
                let bytes = session
                    .screenshot_png_zoomed(*zoom)
                    .map_err(|error| error.to_string())?;
                report
                    .artifacts
                    .push(write_artifact(root, name, "screenshot", &bytes)?);
            }
            if let Some(name) = screen_text {
                let mut text = session
                    .screen_text()
                    .map_err(|error| error.to_string())?
                    .join("\n");
                text.push('\n');
                report
                    .artifacts
                    .push(write_artifact(root, name, "screen_text", text.as_bytes())?);
            }
            if let Some(name) = wav {
                let (bytes, _, _) = session
                    .wav_bytes(*from_frame, *to_frame)
                    .map_err(|error| error.to_string())?;
                report
                    .artifacts
                    .push(write_artifact(root, name, "wav", &bytes)?);
            }
            if let Some(name) = spectrogram {
                let image = session
                    .spectrogram_png(*from_frame, *to_frame, *cell)
                    .map_err(|error| error.to_string())?;
                report
                    .artifacts
                    .push(write_artifact(root, name, "spectrogram", &image.png)?);
            }
            if let Some(name) = audio_events {
                let events = session
                    .audio_events(*from_frame)
                    .map_err(|error| error.to_string())?;
                let bytes = serde_json::to_vec_pretty(&json!({
                    "events": events,
                    "advice": []
                }))
                .map_err(|error| format!("serializing audio events: {error}"))?;
                report
                    .artifacts
                    .push(write_artifact(root, name, "audio_events", &bytes)?);
            }
            if let Some(name) = audio_stats {
                let windows = session
                    .audio_stats(*window_frames)
                    .map_err(|error| error.to_string())?;
                let bytes = serde_json::to_vec_pretty(&json!({
                    "windows": windows,
                    "advice": []
                }))
                .map_err(|error| format!("serializing audio stats: {error}"))?;
                report
                    .artifacts
                    .push(write_artifact(root, name, "audio_stats", &bytes)?);
            }
            if let Some(name) = text_events {
                let events = session
                    .text_events(*from_frame)
                    .map_err(|error| error.to_string())?;
                let bytes = serde_json::to_vec_pretty(&json!({
                    "events": events,
                    "advice": []
                }))
                .map_err(|error| format!("serializing text events: {error}"))?;
                report
                    .artifacts
                    .push(write_artifact(root, name, "text_events", &bytes)?);
            }
        }
    }
    Ok(())
}

fn frame_count(session: &Session) -> u64 {
    session
        .console()
        .map(|console| console.frame_count())
        .unwrap_or(0)
}

fn normalize_relative_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("path is empty".to_string());
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => return Err("`.` path components are not allowed".to_string()),
            Component::ParentDir => {
                return Err("`..` path components are not allowed".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths are not allowed".to_string());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("path is empty".to_string());
    }
    Ok(normalized)
}

fn duplicate_path_key(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

fn artifact_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let normalized = normalize_relative_path(relative)?;
    let path = root.join(&normalized);
    let mut cursor = root.to_path_buf();
    for component in normalized.components() {
        let Component::Normal(part) = component else {
            unreachable!("relative path was validated")
        };
        cursor.push(part);
        if let Ok(metadata) = fs::symlink_metadata(&cursor)
            && metadata.file_type().is_symlink()
        {
            return Err(format!(
                "artifact path {} traverses symlink {}",
                path.display(),
                cursor.display()
            ));
        }
    }
    Ok(path)
}

fn write_artifact(
    root: &Path,
    relative: &str,
    kind: &'static str,
    bytes: &[u8],
) -> Result<ArtifactReport, String> {
    let path = artifact_path(root, relative)?;
    artifact::write(&path, bytes)?;
    Ok(ArtifactReport {
        kind,
        path: path.display().to_string(),
        bytes: bytes.len(),
    })
}

fn print_report(report: &PlaytestReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(report).expect("playtest report is serializable")
        ),
        OutputFormat::Text | OutputFormat::Pretty => {
            println!(
                "{}  {}  stages={}  frames={}  artifacts={}",
                report.scenario.path,
                report.scenario.status,
                report.scenario.stage_count,
                report.scenario.frame_count,
                report.scenario.artifact_count
            );
            for stage in &report.stages {
                let name = stage
                    .name
                    .as_deref()
                    .map(|name| format!("  name={name}"))
                    .unwrap_or_default();
                println!(
                    "{}  {}  {}  frames={}..{}{}",
                    stage.index, stage.op, stage.status, stage.frame_start, stage.frame_end, name
                );
                if let Some(error) = &stage.error {
                    println!("error  stage={}  {error}", stage.index);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_schema_rejects_unknown_fields_and_versions() {
        let unknown =
            r#"{"version":1,"stages":[{"op":"input","frames":1,"buttons":"A","mystery":7}]}"#;
        assert!(serde_json::from_str::<Scenario>(unknown).is_err());

        let wrong_version: Scenario =
            serde_json::from_str(r#"{"version":2,"stages":[{"op":"input","frames":1}]}"#).unwrap();
        assert!(validate_scenario(&wrong_version, None).is_err());
    }

    #[test]
    fn artifact_paths_cannot_escape_or_traverse_symlinks() {
        assert!(normalize_relative_path("scene/frame.png").is_ok());
        assert!(normalize_relative_path("../frame.png").is_err());
        assert!(normalize_relative_path("/tmp/frame.png").is_err());
        assert_eq!(
            normalize_relative_path("scene//frame.png").unwrap(),
            PathBuf::from("scene/frame.png")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let root = std::env::temp_dir()
                .join(format!("console-playtest-symlink-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            symlink(std::env::temp_dir(), root.join("outside")).unwrap();
            let root = fs::canonicalize(root).unwrap();
            assert!(artifact_path(&root, "outside/frame.png").is_err());
        }
    }
}

//! Declarative, deterministic playtest scenarios for agent-authored carts.
//!
//! A scenario is strict, versioned JSON containing ordered stages. It is a
//! thin orchestration layer over [`Session`]: no second stepping engine, no
//! hidden wall clock, and no browser-only semantics.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};

use console_core::{SCREEN_H, SCREEN_W, input};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::artifact;
use crate::map;
use crate::session::Session;
use crate::value::lua_to_json;
use crate::visual::{self, Rect, RgbaImage};

const MAX_SCREENSHOT_ZOOM: u32 = 16;
const MAX_SPECTROGRAM_CELL: u32 = 8;
const MAX_SCENARIO_FRAMES: u64 = 36_000;
const MAX_SPECTROGRAM_FRAMES: u64 = 3_600;
const MAX_MAP_ZOOM: u32 = 16;
const MAX_SEQUENCE_SAMPLES: u64 = 240;
const MAX_SEQUENCE_ZOOM: u32 = 16;
const MAX_REVIEW_COLUMNS: u32 = 16;

pub const USAGE: &str = "\
Run an ordered, deterministic cart playtest scenario

Usage:
  console playtest <cart|project> --scenario <scenario.json> [OPTIONS]

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
    {\"op\":\"sequence\",\"name\":\"hop\",\"frames\":12,\"buttons\":\"R\",\"every\":3,
      \"crop\":{\"x\":16,\"y\":24,\"w\":96,\"h\":80},\"zoom\":2,
      \"gif\":\"hop.gif\",\"strip\":\"hop-strip.png\",\"board\":\"hop-board.png\"},
    {\"op\":\"capture\",\"screenshot\":\"scene.png\",\"zoom\":4,\"draw_trace\":\"draw-trace.json\",
      \"layers\":{\"background\":\"layers/background.png\",\"terrain\":\"layers/terrain.png\"},
      \"map\":{\"source\":\"live\",\"png\":\"map.png\",\"dump\":\"map.txt\"}}
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
    Sequence {
        #[serde(default)]
        name: Option<String>,
        frames: u64,
        #[serde(default)]
        buttons: String,
        #[serde(default = "default_every")]
        every: u64,
        #[serde(default)]
        crop: Crop,
        #[serde(default = "default_zoom")]
        zoom: u32,
        #[serde(default = "default_columns")]
        columns: u32,
        #[serde(default)]
        gif: Option<String>,
        #[serde(default)]
        strip: Option<String>,
        #[serde(default)]
        board: Option<String>,
        #[serde(default)]
        reference: Option<String>,
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
        draw_trace: Option<String>,
        /// Explicit semantic tag -> artifact path mapping. The reserved key
        /// `__untagged__` selects draws made without `draw_tag()`.
        #[serde(default)]
        layers: BTreeMap<String, String>,
        #[serde(default)]
        map: Option<Box<MapCapture>>,
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

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Crop {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Default for Crop {
    fn default() -> Self {
        Crop {
            x: 0,
            y: 0,
            w: SCREEN_W as u32,
            h: SCREEN_H as u32,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MapSource {
    #[default]
    Authored,
    Live,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapCapture {
    #[serde(default)]
    source: MapSource,
    #[serde(default)]
    png: Option<String>,
    #[serde(default)]
    dump: Option<String>,
    #[serde(default)]
    lint: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default = "default_map_zoom")]
    zoom: u32,
    #[serde(default)]
    grid: bool,
    #[serde(default)]
    ids: bool,
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

fn default_map_zoom() -> u32 {
    4
}

fn default_every() -> u64 {
    1
}

fn default_columns() -> u32 {
    4
}

impl Stage {
    fn op(&self) -> &'static str {
        match self {
            Stage::Eval { .. } => "eval",
            Stage::Input { .. } => "input",
            Stage::Sequence { .. } => "sequence",
            Stage::Assert { .. } => "assert",
            Stage::Capture { .. } => "capture",
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Stage::Eval { name, .. }
            | Stage::Input { name, .. }
            | Stage::Sequence { name, .. }
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
        cart: cart.ok_or("missing <cart|project> argument")?,
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
    let scenario_dir = scenario_path.parent().unwrap_or_else(|| Path::new("."));

    let cart_text = crate::project::load_cart_text(cart_path).map_err(|error| error.to_string())?;
    let seed = seed_override.unwrap_or(scenario.seed);
    let mut session = Session::new();
    if scenario.stages.iter().any(|stage| {
        matches!(
            stage,
            Stage::Capture {
                draw_trace: Some(_),
                ..
            }
        )
    }) {
        session.set_draw_tracing(true);
    }
    if scenario.stages.iter().any(|stage| {
        matches!(
            stage,
            Stage::Capture { layers, .. } if !layers.is_empty()
        )
    }) {
        session.set_layer_capture(true);
    }
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
        if let Err(error) = execute_stage(
            stage,
            &mut session,
            artifact_root.as_deref(),
            scenario_dir,
            &mut report,
        ) {
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
            Stage::Sequence {
                name,
                frames,
                buttons,
                every,
                crop,
                zoom,
                columns,
                gif,
                strip,
                board,
                reference,
                ..
            } => {
                if *frames == 0 {
                    return Err(format!("stage {index} sequence frames must be >= 1"));
                }
                if *every == 0 {
                    return Err(format!("stage {index} sequence every must be >= 1"));
                }
                if frames % every != 0 {
                    return Err(format!(
                        "stage {index} sequence frames ({frames}) must be exactly divisible by every ({every})"
                    ));
                }
                let samples = frames / every;
                if samples > MAX_SEQUENCE_SAMPLES {
                    return Err(format!(
                        "stage {index} sequence samples {samples} frames; version 1 allows at most {MAX_SEQUENCE_SAMPLES}"
                    ));
                }
                input::parse(buttons).map_err(|button| {
                    format!("stage {index} has unknown input button {button:?} in {buttons:?}")
                })?;
                validate_crop(index, *crop)?;
                if !(1..=MAX_SEQUENCE_ZOOM).contains(zoom) {
                    return Err(format!(
                        "stage {index} sequence zoom must be 1..={MAX_SEQUENCE_ZOOM}, got {zoom}"
                    ));
                }
                if !(1..=MAX_REVIEW_COLUMNS).contains(columns) {
                    return Err(format!(
                        "stage {index} sequence columns must be 1..={MAX_REVIEW_COLUMNS}, got {columns}"
                    ));
                }
                if gif.is_none() && strip.is_none() && board.is_none() {
                    return Err(format!("stage {index} sequence has no outputs"));
                }
                if reference.is_some() && board.is_none() {
                    return Err(format!(
                        "stage {index} sequence reference requires a board output"
                    ));
                }
                if artifacts.is_none() {
                    return Err(format!(
                        "stage {index} captures files, so --artifacts <DIR> is required"
                    ));
                }
                let frame_numbers = (1..=samples)
                    .map(|sample| sample * *every)
                    .collect::<Vec<_>>();
                validate_sequence_dimensions(
                    index,
                    &visual::SequencePreflight {
                        stage: name.as_deref().unwrap_or("sequence"),
                        frame_numbers: &frame_numbers,
                        crop: Rect {
                            x: crop.x,
                            y: crop.y,
                            w: crop.w,
                            h: crop.h,
                        },
                        zoom: *zoom,
                        columns: *columns,
                        reference_dimensions: None,
                        cadence_frames: *every,
                        gif: gif.is_some(),
                        strip: strip.is_some(),
                        board: board.is_some(),
                    },
                )?;
                for output in [gif.as_deref(), strip.as_deref(), board.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    let normalized = normalize_relative_path(output).map_err(|error| {
                        format!("stage {index} sequence path {output:?}: {error}")
                    })?;
                    if !paths.insert(duplicate_path_key(&normalized)) {
                        return Err(format!(
                            "stage {index} sequence path {output:?} aliases an earlier artifact; every normalized path must be unique"
                        ));
                    }
                }
                stepped_frames = stepped_frames
                    .checked_add(*frames)
                    .ok_or_else(|| format!("stage {index} sequence frame total overflows u64"))?;
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
                draw_trace,
                layers,
                map,
                from_frame,
                to_frame,
                window_frames,
                cell,
                ..
            } => {
                let mut outputs = vec![
                    screenshot.as_deref(),
                    screen_text.as_deref(),
                    wav.as_deref(),
                    spectrogram.as_deref(),
                    audio_events.as_deref(),
                    audio_stats.as_deref(),
                    text_events.as_deref(),
                    draw_trace.as_deref(),
                ];
                for (tag, output) in layers {
                    if tag != "__untagged__" && tag.len() > 64 {
                        return Err(format!(
                            "stage {index} layer tag {tag:?} must be at most 64 UTF-8 bytes or __untagged__"
                        ));
                    }
                    outputs.push(Some(output.as_str()));
                }
                if let Some(map) = map {
                    let map_outputs =
                        [map.png.as_deref(), map.dump.as_deref(), map.lint.as_deref()];
                    if map_outputs.iter().all(Option::is_none) {
                        return Err(format!("stage {index} map capture has no outputs"));
                    }
                    if !(1..=MAX_MAP_ZOOM).contains(&map.zoom) {
                        return Err(format!(
                            "stage {index} map capture zoom must be 1..={MAX_MAP_ZOOM}, got {}",
                            map.zoom
                        ));
                    }
                    outputs.extend(map_outputs);
                }
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

fn validate_crop(index: usize, crop: Crop) -> Result<(), String> {
    if crop.w == 0 || crop.h == 0 {
        return Err(format!(
            "stage {index} sequence crop width and height must be >= 1"
        ));
    }
    let right = crop
        .x
        .checked_add(crop.w)
        .ok_or_else(|| format!("stage {index} sequence crop x+w overflows"))?;
    let bottom = crop
        .y
        .checked_add(crop.h)
        .ok_or_else(|| format!("stage {index} sequence crop y+h overflows"))?;
    if right > SCREEN_W as u32 || bottom > SCREEN_H as u32 {
        return Err(format!(
            "stage {index} sequence crop {},{},{},{} exceeds the {}x{} screen",
            crop.x, crop.y, crop.w, crop.h, SCREEN_W, SCREEN_H
        ));
    }
    Ok(())
}

fn validate_sequence_dimensions(
    index: usize,
    spec: &visual::SequencePreflight<'_>,
) -> Result<(), String> {
    let native_bytes = u64::from(spec.crop.w)
        .checked_mul(u64::from(spec.crop.h))
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| bytes.checked_mul(spec.frame_numbers.len() as u64))
        .ok_or_else(|| format!("stage {index} sequence sampled RGBA size overflows"))?;
    if native_bytes > visual::MAX_VISUAL_RGBA_BYTES as u64 {
        return Err(format!(
            "stage {index} sequence samples need {native_bytes} RGBA bytes, exceeding the {} byte limit",
            visual::MAX_VISUAL_RGBA_BYTES
        ));
    }
    visual::preflight_sequence(spec)
        .map_err(|error| format!("stage {index} sequence dimensions are invalid: {error}"))
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
    scenario_dir: &Path,
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
        Stage::Sequence {
            name,
            frames,
            buttons,
            every,
            crop,
            zoom,
            columns,
            gif,
            strip,
            board,
            reference,
        } => {
            let root = artifact_root.expect("sequence validation requires an artifact root");
            // Resolve and decode the optional comparison image before stepping.
            // A missing/bad reference must not consume part of the requested input.
            let reference_image = reference
                .as_deref()
                .map(|reference| load_reference(scenario_dir, reference))
                .transpose()?;
            let rect = Rect {
                x: crop.x,
                y: crop.y,
                w: crop.w,
                h: crop.h,
            };
            let sample_count = frames / every;
            let stage_label = name
                .clone()
                .unwrap_or_else(|| format!("sequence-{}", report.index));
            let frame_start = frame_count(session);
            let expected_frame_numbers = (1..=sample_count)
                .map(|sample| {
                    frame_start
                        .checked_add(sample * *every)
                        .ok_or("sequence frame number overflows")
                })
                .collect::<Result<Vec<_>, _>>()?;
            visual::preflight_sequence(&visual::SequencePreflight {
                stage: &stage_label,
                frame_numbers: &expected_frame_numbers,
                crop: rect,
                zoom: *zoom,
                columns: *columns,
                reference_dimensions: reference_image
                    .as_ref()
                    .map(|image| (image.width, image.height)),
                cadence_frames: *every,
                gif: gif.is_some(),
                strip: strip.is_some(),
                board: board.is_some(),
            })?;
            let mask = input::parse(buttons)
                .map_err(|button| format!("unknown input button {button:?} in {buttons:?}"))?;
            let mut images = Vec::with_capacity(sample_count as usize);
            let mut frame_numbers = Vec::with_capacity(sample_count as usize);
            for _ in 0..sample_count {
                let outcome = session
                    .step(*every, mask)
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
                let rgba = {
                    let console = session.console().map_err(|error| error.to_string())?;
                    crate::session::framebuffer_rgba(
                        console.framebuffer(),
                        console.display_palette(),
                    )
                };
                let frame = RgbaImage::new(SCREEN_W as u32, SCREEN_H as u32, rgba)?;
                images.push(frame.crop(rect)?);
                frame_numbers.push(outcome.frame_count);
            }
            if frame_numbers != expected_frame_numbers {
                return Err(format!(
                    "sequence cadence drifted: expected {expected_frame_numbers:?}, got {frame_numbers:?}"
                ));
            }

            // Produce every requested artifact in memory before writing any of
            // them, so dimension/encoder errors cannot leave a partial stage.
            let gif_output = gif
                .as_ref()
                .map(|_| visual::animated_gif(&images, *zoom, *every))
                .transpose()?;
            let strip_output = strip
                .as_ref()
                .map(|_| visual::contact_strip(&images, *zoom))
                .transpose()?;
            let board_output = board
                .as_ref()
                .map(|_| {
                    visual::review_board(&visual::BoardSpec {
                        stage: &stage_label,
                        frames: &images,
                        frame_numbers: &frame_numbers,
                        crop: rect,
                        zoom: *zoom,
                        columns: *columns,
                        reference: reference_image.as_ref(),
                    })
                })
                .transpose()?;

            if let (Some(path), Some((bytes, _, _, _))) = (gif, &gif_output) {
                report
                    .artifacts
                    .push(write_artifact(root, path, "sequence_gif", bytes)?);
            }
            if let (Some(path), Some(image)) = (strip, &strip_output) {
                report
                    .artifacts
                    .push(write_artifact(root, path, "sequence_strip", &image.png())?);
            }
            if let (Some(path), Some((image, _))) = (board, &board_output) {
                report
                    .artifacts
                    .push(write_artifact(root, path, "review_board", &image.png())?);
            }

            let gif_meta = gif_output.as_ref().map(|(_, width, height, delay)| {
                json!({
                    "width": width,
                    "height": height,
                    "delay_centiseconds": delay,
                    "loop": "infinite"
                })
            });
            let reference_meta = reference_image.as_ref().map(|image| {
                let panel = board_output
                    .as_ref()
                    .and_then(|(_, layout)| layout.reference_panel)
                    .expect("validated reference has a review board panel");
                json!({
                    "path": reference,
                    "width": image.width,
                    "height": image.height,
                    "scale": "native",
                    "pixel_aligned": false,
                    "panel": {"x": panel.x, "y": panel.y, "w": panel.w, "h": panel.h}
                })
            });
            let runtime_panels = board_output.as_ref().map(|(_, layout)| {
                layout
                    .runtime_panels
                    .iter()
                    .map(|panel| {
                        json!({
                            "x": panel.x, "y": panel.y, "w": panel.w, "h": panel.h
                        })
                    })
                    .collect::<Vec<_>>()
            });
            report.actual = Some(json!({
                "frames": frames,
                "buttons": buttons,
                "mask": mask,
                "every": every,
                "samples": sample_count,
                "sampled_frames": frame_numbers,
                "crop": {"x": crop.x, "y": crop.y, "w": crop.w, "h": crop.h},
                "zoom": zoom,
                "scaling": "nearest_neighbor",
                "columns": columns,
                "gif": gif_meta,
                "runtime_panels": runtime_panels,
                "reference": reference_meta
            }));
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
            draw_trace,
            layers,
            map,
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
            if let Some(name) = draw_trace {
                let trace = session
                    .draw_events(*from_frame, None)
                    .map_err(|error| error.to_string())?;
                let mut bytes = serde_json::to_vec(&trace)
                    .map_err(|error| format!("serializing draw trace: {error}"))?;
                bytes.push(b'\n');
                report
                    .artifacts
                    .push(write_artifact(root, name, "draw_trace", &bytes)?);
            }
            if !layers.is_empty() {
                // Resolve and encode the complete requested set before any
                // layer is written. A misspelled tag cannot leave a partial,
                // misleading diagnostic set behind.
                let captures = session
                    .layer_screenshots_png_zoomed(*zoom)
                    .map_err(|error| error.to_string())?;
                if captures.dropped != 0 {
                    return Err(format!(
                        "layer capture exceeded its {}-tag capacity and dropped {} draw operations",
                        captures.capacity, captures.dropped
                    ));
                }
                let mut outputs = Vec::with_capacity(layers.len());
                for (requested, path) in layers {
                    let tag = (requested != "__untagged__").then_some(requested.as_str());
                    let capture = captures
                        .layers
                        .iter()
                        .find(|capture| capture.tag.as_deref() == tag)
                        .ok_or_else(|| {
                            let available = captures
                                .layers
                                .iter()
                                .map(|capture| capture.tag.as_deref().unwrap_or("__untagged__"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!(
                                "requested layer {requested:?} was not drawn in the current frame; available layers: [{available}]"
                            )
                        })?;
                    outputs.push((path, &capture.png));
                }
                for (path, png) in outputs {
                    report
                        .artifacts
                        .push(write_artifact(root, path, "layer_png", png)?);
                }
            }
            if let Some(map_capture) = map {
                let console = session.console().map_err(|error| error.to_string())?;
                let live_tiles;
                let tiles = match map_capture.source {
                    MapSource::Authored => console.cart().map(),
                    MapSource::Live => {
                        live_tiles = console.live_map();
                        &live_tiles
                    }
                };
                let region = map::parse_region(map_capture.region.as_deref(), tiles)?;
                if let Some(name) = &map_capture.png {
                    let image = map::view::render_tiles(
                        console.cart(),
                        tiles,
                        region,
                        &map::view::MapRenderOpts {
                            zoom: map_capture.zoom,
                            grid: map_capture.grid,
                            ids: map_capture.ids,
                        },
                    )?;
                    report
                        .artifacts
                        .push(write_artifact(root, name, "map_png", &image.png)?);
                }
                if let Some(name) = &map_capture.dump {
                    let bytes = map::view::dump_tiles(tiles, region)?;
                    report.artifacts.push(write_artifact(
                        root,
                        name,
                        "map_dump",
                        bytes.as_bytes(),
                    )?);
                }
                if let Some(name) = &map_capture.lint {
                    let mut bytes =
                        serde_json::to_vec_pretty(&map::view::lint_tiles(console.cart(), tiles))
                            .map_err(|error| format!("serializing map lint: {error}"))?;
                    bytes.push(b'\n');
                    report
                        .artifacts
                        .push(write_artifact(root, name, "map_lint", &bytes)?);
                }
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

fn load_reference(scenario_dir: &Path, reference: &str) -> Result<RgbaImage, String> {
    if reference.trim().is_empty() {
        return Err("sequence reference path is empty".to_string());
    }
    let reference_path = Path::new(reference);
    let path = if reference_path.is_absolute() {
        reference_path.to_path_buf()
    } else {
        scenario_dir.join(reference_path)
    };
    let bytes = fs::read(&path)
        .map_err(|error| format!("reading sequence reference {}: {error}", path.display()))?;
    let decoded = crate::palette::decode_png_rgba(&bytes)
        .map_err(|error| format!("decoding sequence reference {}: {error}", path.display()))?;
    RgbaImage::new(decoded.width, decoded.height, decoded.rgba)
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

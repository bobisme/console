//! Declarative, deterministic playtest scenarios for agent-authored carts.
//!
//! A scenario is strict, versioned JSON containing ordered stages. It is a
//! thin orchestration layer over [`Session`]: no second stepping engine, no
//! hidden wall clock, and no browser-only semantics.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};

use console_core::{
    COLOR_MASK, Cart, LayerCaptureFrame, SCREEN_H, SCREEN_W, SaveValue, TileMap,
    canonical_save_document, input,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::artifact;
use crate::ecs_watch::{self, WatchDefinition};
use crate::hooks::{dev_value_to_json, json_to_dev_value, validate_name};
use crate::map;
use crate::session::{MAX_SCREEN_TEXT_REGION_PIXELS, ScreenTextRegion, Session};
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
const MAX_DIAGNOSTIC_ZOOM: u32 = 4;
const MAX_DIAGNOSTIC_COLUMNS: u32 = 8;
const MAX_MOTION_SAMPLES: u32 = 8;
const MAX_DIAGNOSTIC_SOURCES: usize = 24;

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
  {\"version\":1,\"seed\":0,\"initial_save\":{\"version\":1,\"data\":{\"unlocked\":true}},\"stages\":[
    {\"op\":\"hook\",\"hook\":\"start\"},
    {\"op\":\"hook\",\"hook\":\"status\",\"expect\":{\"op\":\"equals\",\"field\":\"scene\",\"value\":\"play\"}},
    {\"op\":\"input\",\"frames\":1,\"buttons\":\"A\"},
    {\"op\":\"eval\",\"code\":\"dev_warp(48,449)\"},
    {\"op\":\"assert\",\"code\":\"return dev_status().embers\",\"equals\":1},
    {\"op\":\"save_assert\",\"version\":2,\"equals\":{\"unlocked\":true}},
    {\"op\":\"ecs_watch\",\"watch\":\"enemies\",\"define\":{\"world\":\"arena\",\"with\":[\"enemy\"],\"limit\":64}},
    {\"op\":\"ecs_watch\",\"watch\":\"enemies\"},
    {\"op\":\"sequence\",\"name\":\"hop\",\"frames\":12,\"buttons\":\"R\",\"every\":3,
      \"crop\":{\"x\":16,\"y\":24,\"w\":96,\"h\":80},\"zoom\":2,
      \"gif\":\"hop.gif\",\"strip\":\"hop-strip.png\",\"board\":\"hop-board.png\"},
    {\"op\":\"capture\",\"screenshot\":\"scene.png\",\"zoom\":4,
      \"screen_text_summary\":\"screen.json\",\"screen_text_region\":{\"x\":0,\"y\":0,\"width\":192,\"height\":48},
      \"draw_trace\":\"draw-trace.json\",
      \"layers\":{\"background\":\"layers/background.png\",\"terrain\":\"layers/terrain.png\"},
      \"map\":{\"source\":\"live\",\"png\":\"map.png\",\"dump\":\"map.txt\"}},
    {\"op\":\"review\",\"board\":\"visual-board.png\",\"report\":\"visual-report.json\",
      \"stages\":[\"hop\"],\"reference\":\"reference.png\",
      \"temporal_checks\":[{\"kind\":\"consecutive\",\"name\":\"static-shimmer\",
        \"stage\":\"hop\",\"max_changed_fraction\":0.2,\"heatmap\":\"hop-diff.png\"}]}
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
    #[serde(default)]
    pub initial_save: Option<InitialSave>,
    pub stages: Vec<Stage>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialSave {
    pub version: u32,
    pub data: Json,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Stage {
    Eval {
        #[serde(default)]
        name: Option<String>,
        code: String,
    },
    Hook {
        #[serde(default)]
        name: Option<String>,
        hook: String,
        #[serde(default)]
        args: Json,
        #[serde(default)]
        expect: Option<HookExpectation>,
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
    SaveAssert {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        version: Option<u32>,
        equals: Json,
    },
    /// Define (when `define` is present) and sample one named bounded ECS
    /// watch. Later stages omit `define` and refer only to `watch`.
    EcsWatch {
        #[serde(default)]
        name: Option<String>,
        watch: String,
        #[serde(default)]
        define: Option<Json>,
        #[serde(default)]
        artifact: Option<String>,
    },
    Review {
        #[serde(default)]
        name: Option<String>,
        board: String,
        #[serde(default)]
        report: Option<String>,
        stages: Vec<String>,
        #[serde(default = "visual::default_diagnostic_views")]
        views: Vec<visual::DiagnosticView>,
        #[serde(default = "default_zoom")]
        zoom: u32,
        #[serde(default = "default_diagnostic_columns")]
        columns: u32,
        #[serde(default = "default_motion_samples")]
        motion_samples: u32,
        #[serde(default)]
        reference: Option<String>,
        #[serde(default)]
        layers: Option<Box<ReviewLayers>>,
        #[serde(default)]
        map: Option<Box<ReviewMap>>,
        #[serde(default)]
        temporal_checks: Vec<TemporalVisualCheck>,
        #[serde(default)]
        lint: Option<Box<ReviewVisualLint>>,
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
        screen_text_region: Option<ScreenTextRegion>,
        #[serde(default)]
        screen_text_summary: Option<String>,
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
        #[serde(default)]
        save: Option<String>,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum HookExpectation {
    Equals {
        #[serde(default)]
        field: Option<String>,
        value: Json,
    },
    NotEquals {
        #[serde(default)]
        field: Option<String>,
        value: Json,
    },
    AtLeast {
        #[serde(default)]
        field: Option<String>,
        value: f64,
    },
    GreaterThan {
        #[serde(default)]
        field: Option<String>,
        value: f64,
    },
}

impl HookExpectation {
    fn field(&self) -> Option<&str> {
        match self {
            Self::Equals { field, .. }
            | Self::NotEquals { field, .. }
            | Self::AtLeast { field, .. }
            | Self::GreaterThan { field, .. } => field.as_deref(),
        }
    }
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewLayers {
    stage: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    include_untagged: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewMap {
    stage: String,
    #[serde(default)]
    source: MapSource,
    #[serde(default)]
    region: Option<String>,
    #[serde(default = "default_map_zoom")]
    zoom: u32,
    #[serde(default)]
    grid: bool,
    #[serde(default)]
    ids: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemporalVisualCheck {
    Boundary {
        name: String,
        from: String,
        to: String,
        max_changed_fraction: f64,
        #[serde(default)]
        allowed_regions: Vec<Crop>,
        #[serde(default)]
        heatmap: Option<String>,
    },
    Consecutive {
        name: String,
        stage: String,
        max_changed_fraction: f64,
        #[serde(default)]
        allowed_regions: Vec<Crop>,
        #[serde(default)]
        heatmap: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewVisualLint {
    #[serde(default)]
    reserved_collision_colors: Option<ReservedCollisionColorLint>,
    #[serde(default)]
    bright_background_horizontals: Option<BrightHorizontalLint>,
    #[serde(default)]
    actor_background_luma: Option<ActorBackgroundLumaLint>,
    #[serde(default)]
    traversal_corridor_edges: Option<TraversalCorridorEdgeLint>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservedCollisionColorLint {
    source_tag: String,
    indices: Vec<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrightHorizontalLint {
    background_tag: String,
    min_luma: u8,
    max_run: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorBackgroundLumaLint {
    actor_tag: String,
    background_tag: String,
    min_gap: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraversalCorridorEdgeLint {
    background_tag: String,
    region: Crop,
    min_luma_delta: u8,
    max_edge_fraction: f64,
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

fn default_diagnostic_columns() -> u32 {
    5
}

fn default_motion_samples() -> u32 {
    3
}

impl Stage {
    fn op(&self) -> &'static str {
        match self {
            Stage::Eval { .. } => "eval",
            Stage::Hook { .. } => "hook",
            Stage::Input { .. } => "input",
            Stage::Sequence { .. } => "sequence",
            Stage::Assert { .. } => "assert",
            Stage::SaveAssert { .. } => "save_assert",
            Stage::EcsWatch { .. } => "ecs_watch",
            Stage::Review { .. } => "review",
            Stage::Capture { .. } => "capture",
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Stage::Eval { name, .. }
            | Stage::Hook { name, .. }
            | Stage::Input { name, .. }
            | Stage::Sequence { name, .. }
            | Stage::Assert { name, .. }
            | Stage::SaveAssert { name, .. }
            | Stage::EcsWatch { name, .. }
            | Stage::Review { name, .. }
            | Stage::Capture { name, .. } => name.as_deref(),
        }
    }
}

fn playtest_watch_definition(
    watch: &str,
    define: &Json,
    context: &str,
) -> Result<WatchDefinition, String> {
    let mut params = define
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{context} define must be an object"))?;
    if params.contains_key("name") {
        return Err(format!(
            "{context} define must not contain \"name\"; use the stage's \"watch\" field"
        ));
    }
    params.insert("name".to_string(), Json::String(watch.to_string()));
    ecs_watch::parse_definition(&Json::Object(params), "name", context)
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

#[derive(Debug, Serialize)]
struct TemporalVisualResult {
    name: String,
    kind: &'static str,
    from: String,
    to: String,
    compared_pixels: u64,
    changed_pixels: u64,
    changed_fraction: f64,
    max_changed_fraction: f64,
    passed: bool,
}

#[derive(Debug, Serialize)]
struct SemanticVisualWarning {
    kind: &'static str,
    source: String,
    message: String,
    actual: Json,
    limit: Json,
}

struct TemporalComparison {
    compared_pixels: u64,
    changed_pixels: u64,
    changed_fraction: f64,
    heatmap: visual::RgbaImage,
}

#[derive(Debug, Clone)]
struct ReviewLayerPlan {
    stage: String,
}

#[derive(Debug, Clone)]
struct ReviewMapPlan {
    stage: String,
    source: MapSource,
}

struct CapturedLayers {
    frame: LayerCaptureFrame,
    display_palette: [u8; 64],
}

struct ReviewCollector {
    selected_stages: BTreeSet<String>,
    motion_samples: usize,
    sources: Vec<visual::DiagnosticSource>,
    reference: Option<visual::RgbaImage>,
    layer_plan: Option<ReviewLayerPlan>,
    captured_layers: Option<CapturedLayers>,
    map_plan: Option<ReviewMapPlan>,
    captured_map: Option<Box<TileMap>>,
}

impl ReviewCollector {
    fn from_stage(stage: Option<&Stage>, reference: Option<visual::RgbaImage>) -> Self {
        let Some(Stage::Review {
            stages,
            motion_samples,
            layers,
            map,
            ..
        }) = stage
        else {
            return ReviewCollector {
                selected_stages: BTreeSet::new(),
                motion_samples: 0,
                sources: Vec::new(),
                reference,
                layer_plan: None,
                captured_layers: None,
                map_plan: None,
                captured_map: None,
            };
        };
        ReviewCollector {
            selected_stages: stages.iter().cloned().collect(),
            motion_samples: *motion_samples as usize,
            sources: Vec::new(),
            reference,
            layer_plan: layers.as_deref().map(|layers| ReviewLayerPlan {
                stage: layers.stage.clone(),
            }),
            captured_layers: None,
            map_plan: map.as_deref().map(|map| ReviewMapPlan {
                stage: map.stage.clone(),
                source: map.source,
            }),
            captured_map: None,
        }
    }

    fn wants_stage(&self, name: Option<&str>) -> bool {
        name.is_some_and(|name| self.selected_stages.contains(name))
    }

    fn capture_after_stage(&mut self, stage: &Stage, session: &Session) -> Result<(), String> {
        let Some(name) = stage.name() else {
            return Ok(());
        };
        let console = session.console().map_err(|error| error.to_string())?;
        if self.selected_stages.contains(name) && !matches!(stage, Stage::Sequence { .. }) {
            self.sources.push(runtime_source(
                format!("{name} F{}", console.frame_count()),
                Some(visual::DiagnosticSelector::Stage(name.to_string())),
                console.framebuffer(),
                console.display_palette(),
                Rect {
                    x: 0,
                    y: 0,
                    w: SCREEN_W as u32,
                    h: SCREEN_H as u32,
                },
            )?);
        }
        if self
            .layer_plan
            .as_ref()
            .is_some_and(|plan| plan.stage == name)
        {
            self.captured_layers = Some(CapturedLayers {
                frame: console.layer_capture_frame(),
                display_palette: *console.display_palette(),
            });
        }
        if self
            .map_plan
            .as_ref()
            .is_some_and(|plan| plan.stage == name)
        {
            let plan = self.map_plan.as_ref().expect("checked above");
            let tiles = match plan.source {
                MapSource::Authored => *console.cart().map(),
                MapSource::Live => console.live_map(),
            };
            self.captured_map = Some(Box::new(tiles));
        }
        Ok(())
    }

    fn capture_sequence(
        &mut self,
        name: Option<&str>,
        images: &[visual::RgbaImage],
        indices: &[Vec<u8>],
        frame_numbers: &[u64],
    ) -> Result<(), String> {
        let Some(name) = name.filter(|name| self.selected_stages.contains(*name)) else {
            return Ok(());
        };
        if images.len() != indices.len() || images.len() != frame_numbers.len() {
            return Err("internal review sequence evidence is misaligned".to_string());
        }
        let count = images.len().min(self.motion_samples);
        if count == 0 {
            return Err(format!("reviewed sequence stage {name:?} has no samples"));
        }
        let selected = if count == 1 {
            vec![images.len() - 1]
        } else {
            (0..count)
                .map(|index| index * (images.len() - 1) / (count - 1))
                .collect()
        };
        for index in selected {
            self.sources.push(visual::DiagnosticSource {
                label: format!("{name} F{}", frame_numbers[index]),
                selector: Some(visual::DiagnosticSelector::Stage(name.to_string())),
                image: images[index].clone(),
                palette_indices: Some(indices[index].clone()),
            });
        }
        Ok(())
    }

    fn finish_sources(
        &mut self,
        stage: &Stage,
        session: &Session,
    ) -> Result<Vec<visual::DiagnosticSource>, String> {
        let Stage::Review { layers, map, .. } = stage else {
            return Err("internal visual review stage mismatch".to_string());
        };
        let mut sources = std::mem::take(&mut self.sources);
        if let Some(layer_request) = layers.as_deref() {
            let captured = self.captured_layers.as_ref().ok_or_else(|| {
                format!(
                    "review layer stage {:?} was not captured",
                    layer_request.stage
                )
            })?;
            if captured.frame.dropped != 0 {
                return Err(format!(
                    "review layer capture exceeded its {}-tag capacity and dropped {} draw operations",
                    captured.frame.capacity, captured.frame.dropped
                ));
            }
            let mut requested = layer_request
                .tags
                .iter()
                .map(|tag| Some(tag.as_str()))
                .collect::<Vec<_>>();
            if layer_request.include_untagged {
                requested.push(None);
            }
            for tag in requested {
                let layer = captured
                    .frame
                    .layers
                    .iter()
                    .find(|layer| layer.tag.as_deref() == tag)
                    .ok_or_else(|| {
                        format!(
                            "review requested layer {:?} at stage {:?}, but it was not drawn",
                            tag.unwrap_or("<untagged>"),
                            layer_request.stage
                        )
                    })?;
                let rgba = crate::session::layer_framebuffer_rgba(
                    &layer.framebuffer,
                    &captured.display_palette,
                );
                let indices = layer
                    .framebuffer
                    .iter()
                    .map(|&index| {
                        if index == console_core::LAYER_TRANSPARENT {
                            255
                        } else {
                            captured.display_palette[(index & COLOR_MASK) as usize] & COLOR_MASK
                        }
                    })
                    .collect();
                sources.push(visual::DiagnosticSource {
                    label: format!(
                        "LAYER {} @ {}",
                        tag.unwrap_or("UNTAGGED"),
                        layer_request.stage
                    ),
                    selector: tag.map(|tag| visual::DiagnosticSelector::Layer(tag.to_string())),
                    image: visual::RgbaImage::new(SCREEN_W as u32, SCREEN_H as u32, rgba)?,
                    palette_indices: Some(indices),
                });
            }
        }
        if let Some(map_request) = map.as_deref() {
            let tiles = self.captured_map.as_deref().ok_or_else(|| {
                format!("review map stage {:?} was not captured", map_request.stage)
            })?;
            let console = session.console().map_err(|error| error.to_string())?;
            let region = map::parse_region(map_request.region.as_deref(), tiles)?;
            let image = map::view::render_tiles(
                console.cart(),
                tiles,
                region,
                &map::view::MapRenderOpts {
                    zoom: map_request.zoom,
                    grid: map_request.grid,
                    ids: map_request.ids,
                },
            )?;
            sources.push(visual::DiagnosticSource {
                label: format!(
                    "MAP {} @ {}",
                    map_source_name(map_request.source),
                    map_request.stage
                ),
                selector: None,
                image: visual::RgbaImage::new(image.width, image.height, image.rgba)?,
                palette_indices: None,
            });
        }
        if let Some(reference) = &self.reference {
            sources.push(visual::DiagnosticSource {
                label: "REFERENCE".to_string(),
                selector: None,
                image: reference.clone(),
                palette_indices: None,
            });
        }
        if sources.len() > 24 {
            return Err(format!(
                "visual review collected {} sources; at most 24 are allowed",
                sources.len()
            ));
        }
        Ok(sources)
    }
}

fn map_source_name(source: MapSource) -> &'static str {
    match source {
        MapSource::Authored => "AUTHORED",
        MapSource::Live => "LIVE",
    }
}

fn presented_indices(framebuffer: &[u8], display_palette: &[u8; 64]) -> Vec<u8> {
    framebuffer
        .iter()
        .map(|&index| display_palette[(index & COLOR_MASK) as usize] & COLOR_MASK)
        .collect()
}

fn crop_indices(indices: &[u8], crop: Rect) -> Result<Vec<u8>, String> {
    let right = crop
        .x
        .checked_add(crop.w)
        .ok_or("review crop x+w overflows")?;
    let bottom = crop
        .y
        .checked_add(crop.h)
        .ok_or("review crop y+h overflows")?;
    if right > SCREEN_W as u32 || bottom > SCREEN_H as u32 {
        return Err("review index crop exceeds the framebuffer".to_string());
    }
    let mut cropped = Vec::with_capacity((crop.w * crop.h) as usize);
    for y in crop.y..bottom {
        let start = y as usize * SCREEN_W + crop.x as usize;
        cropped.extend_from_slice(&indices[start..start + crop.w as usize]);
    }
    Ok(cropped)
}

fn runtime_source(
    label: String,
    selector: Option<visual::DiagnosticSelector>,
    framebuffer: &[u8; console_core::FB_LEN],
    display_palette: &[u8; 64],
    crop: Rect,
) -> Result<visual::DiagnosticSource, String> {
    let rgba = crate::session::framebuffer_rgba(framebuffer, display_palette);
    let image = visual::RgbaImage::new(SCREEN_W as u32, SCREEN_H as u32, rgba)?.crop(crop)?;
    let indices = crop_indices(&presented_indices(framebuffer, display_palette), crop)?;
    Ok(visual::DiagnosticSource {
        label,
        selector,
        image,
        palette_indices: Some(indices),
    })
}

fn stage_review_sources<'a>(
    sources: &'a [visual::DiagnosticSource],
    stage: &str,
) -> Vec<&'a visual::DiagnosticSource> {
    sources
        .iter()
        .filter(|source| {
            matches!(
                source.selector.as_ref(),
                Some(visual::DiagnosticSelector::Stage(name)) if name == stage
            )
        })
        .collect()
}

fn layer_review_source<'a>(
    sources: &'a [visual::DiagnosticSource],
    tag: &str,
) -> Result<&'a visual::DiagnosticSource, String> {
    let matches = sources
        .iter()
        .filter(|source| {
            matches!(
                source.selector.as_ref(),
                Some(visual::DiagnosticSelector::Layer(name)) if name == tag
            )
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [source] => Ok(*source),
        [] => Err(format!("visual lint layer tag {tag:?} was not captured")),
        _ => Err(format!("visual lint layer tag {tag:?} is ambiguous")),
    }
}

fn region_contains(region: &Crop, x: u32, y: u32) -> bool {
    x >= region.x
        && x < region.x.saturating_add(region.w)
        && y >= region.y
        && y < region.y.saturating_add(region.h)
}

fn compare_visual_sources(
    from: &visual::DiagnosticSource,
    to: &visual::DiagnosticSource,
    allowed_regions: &[Crop],
) -> Result<TemporalComparison, String> {
    if (from.image.width, from.image.height) != (to.image.width, to.image.height) {
        return Err(format!(
            "temporal comparison dimensions differ: {:?} is {}x{}, {:?} is {}x{}",
            from.label,
            from.image.width,
            from.image.height,
            to.label,
            to.image.width,
            to.image.height
        ));
    }
    for region in allowed_regions {
        let right = region
            .x
            .checked_add(region.w)
            .ok_or("temporal allowed region x+w overflows")?;
        let bottom = region
            .y
            .checked_add(region.h)
            .ok_or("temporal allowed region y+h overflows")?;
        if region.w == 0 || region.h == 0 || right > from.image.width || bottom > from.image.height
        {
            return Err(format!(
                "temporal allowed region {},{},{},{} exceeds {}x{} comparison",
                region.x, region.y, region.w, region.h, from.image.width, from.image.height
            ));
        }
    }

    let pixels = (from.image.width as usize) * (from.image.height as usize);
    let exact = from
        .palette_indices
        .as_ref()
        .zip(to.palette_indices.as_ref())
        .filter(|(a, b)| a.len() == pixels && b.len() == pixels);
    let mut rgba = vec![0u8; pixels * 4];
    let mut compared_pixels = 0u64;
    let mut changed_pixels = 0u64;
    for index in 0..pixels {
        let x = index as u32 % from.image.width;
        let y = index as u32 / from.image.width;
        let output = &mut rgba[index * 4..index * 4 + 4];
        if allowed_regions
            .iter()
            .any(|region| region_contains(region, x, y))
        {
            output.copy_from_slice(&[48, 48, 48, 255]);
            continue;
        }
        compared_pixels += 1;
        let changed = exact.map_or_else(
            || from.image.rgba[index * 4..index * 4 + 4] != to.image.rgba[index * 4..index * 4 + 4],
            |(a, b)| a[index] != b[index],
        );
        if changed {
            changed_pixels += 1;
            output.copy_from_slice(&[255, 0, 96, 255]);
        } else {
            output.copy_from_slice(&[8, 12, 20, 255]);
        }
    }
    if compared_pixels == 0 {
        return Err("temporal allowed regions exclude every comparison pixel".to_string());
    }
    Ok(TemporalComparison {
        compared_pixels,
        changed_pixels,
        changed_fraction: changed_pixels as f64 / compared_pixels as f64,
        heatmap: visual::RgbaImage::new(from.image.width, from.image.height, rgba)?,
    })
}

fn pixel_luma(pixel: &[u8]) -> u8 {
    ((u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29 + 128) >> 8)
        as u8
}

fn visual_lint_warnings(
    lint: Option<&ReviewVisualLint>,
    sources: &[visual::DiagnosticSource],
) -> Result<Vec<SemanticVisualWarning>, String> {
    let Some(lint) = lint else {
        return Ok(Vec::new());
    };
    let mut warnings = Vec::new();

    if let Some(check) = &lint.reserved_collision_colors {
        let source = layer_review_source(sources, &check.source_tag)?;
        let indices = source.palette_indices.as_ref().ok_or_else(|| {
            format!(
                "visual lint source {:?} has no exact palette indices",
                source.label
            )
        })?;
        let hits = indices
            .iter()
            .filter(|index| check.indices.contains(index))
            .count() as u64;
        if hits != 0 {
            warnings.push(SemanticVisualWarning {
                kind: "reserved_collision_colors",
                source: source.label.clone(),
                message: format!(
                    "{} pixels use game-authored collision-contact colors {:?}",
                    hits, check.indices
                ),
                actual: json!({"pixels": hits, "indices": check.indices}),
                limit: json!({"pixels": 0}),
            });
        }
    }

    if let Some(check) = &lint.bright_background_horizontals {
        let source = layer_review_source(sources, &check.background_tag)?;
        let mut longest = 0u32;
        for row in source
            .image
            .rgba
            .chunks_exact((source.image.width * 4) as usize)
        {
            let mut run = 0u32;
            for pixel in row.chunks_exact(4) {
                if pixel[3] != 0 && pixel_luma(pixel) >= check.min_luma {
                    run += 1;
                    longest = longest.max(run);
                } else {
                    run = 0;
                }
            }
        }
        if longest > check.max_run {
            warnings.push(SemanticVisualWarning {
                kind: "bright_background_horizontals",
                source: source.label.clone(),
                message: format!(
                    "bright background horizontal runs for {longest}px, above the authored {}px limit",
                    check.max_run
                ),
                actual: json!({"longest_run": longest, "min_luma": check.min_luma}),
                limit: json!({"max_run": check.max_run}),
            });
        }
    }

    if let Some(check) = &lint.actor_background_luma {
        let actor = layer_review_source(sources, &check.actor_tag)?;
        let background = layer_review_source(sources, &check.background_tag)?;
        if (actor.image.width, actor.image.height)
            != (background.image.width, background.image.height)
        {
            return Err("actor/background lint layers have different dimensions".to_string());
        }
        let mut actor_sum = 0u64;
        let mut background_sum = 0u64;
        let mut pixels = 0u64;
        for (actor_pixel, background_pixel) in actor
            .image
            .rgba
            .chunks_exact(4)
            .zip(background.image.rgba.chunks_exact(4))
        {
            if actor_pixel[3] == 0 {
                continue;
            }
            actor_sum += u64::from(pixel_luma(actor_pixel));
            background_sum += u64::from(pixel_luma(background_pixel));
            pixels += 1;
        }
        if pixels == 0 {
            return Err(format!(
                "visual lint actor layer {:?} is empty",
                actor.label
            ));
        }
        let actor_mean = actor_sum as f64 / pixels as f64;
        let background_mean = background_sum as f64 / pixels as f64;
        let gap = (actor_mean - background_mean).abs();
        if gap < f64::from(check.min_gap) {
            warnings.push(SemanticVisualWarning {
                kind: "actor_background_luma",
                source: format!("{} vs {}", actor.label, background.label),
                message: format!(
                    "actor/background mean luma gap is {gap:.2}, below the authored {} limit",
                    check.min_gap
                ),
                actual: json!({"gap": gap, "actor_mean": actor_mean, "background_mean": background_mean}),
                limit: json!({"min_gap": check.min_gap}),
            });
        }
    }

    if let Some(check) = &lint.traversal_corridor_edges {
        let source = layer_review_source(sources, &check.background_tag)?;
        let region = &check.region;
        let right = region
            .x
            .checked_add(region.w)
            .ok_or("traversal corridor x+w overflows")?;
        let bottom = region
            .y
            .checked_add(region.h)
            .ok_or("traversal corridor y+h overflows")?;
        if region.w == 0
            || region.h == 0
            || right > source.image.width
            || bottom > source.image.height
        {
            return Err(format!(
                "traversal corridor {},{},{},{} exceeds lint source {}x{}",
                region.x, region.y, region.w, region.h, source.image.width, source.image.height
            ));
        }
        let mut edge_pixels = 0u64;
        let mut pixels = 0u64;
        for y in region.y..bottom {
            for x in region.x..right {
                let index = (y * source.image.width + x) as usize;
                let current = pixel_luma(&source.image.rgba[index * 4..index * 4 + 4]);
                let right_luma = if x + 1 < source.image.width {
                    pixel_luma(&source.image.rgba[(index + 1) * 4..(index + 1) * 4 + 4])
                } else {
                    current
                };
                let down_luma = if y + 1 < source.image.height {
                    let next = index + source.image.width as usize;
                    pixel_luma(&source.image.rgba[next * 4..next * 4 + 4])
                } else {
                    current
                };
                edge_pixels += u64::from(
                    current
                        .abs_diff(right_luma)
                        .max(current.abs_diff(down_luma))
                        >= check.min_luma_delta,
                );
                pixels += 1;
            }
        }
        let fraction = edge_pixels as f64 / pixels as f64;
        if fraction > check.max_edge_fraction {
            warnings.push(SemanticVisualWarning {
                kind: "traversal_corridor_edges",
                source: source.label.clone(),
                message: format!(
                    "background edge density in the traversal corridor is {fraction:.4}, above the authored {:.4} limit",
                    check.max_edge_fraction
                ),
                actual: json!({"edge_pixels": edge_pixels, "pixels": pixels, "fraction": fraction}),
                limit: json!({
                    "min_luma_delta": check.min_luma_delta,
                    "max_edge_fraction": check.max_edge_fraction
                }),
            });
        }
    }

    Ok(warnings)
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
    let review_stage = scenario
        .stages
        .iter()
        .find(|stage| matches!(stage, Stage::Review { .. }));
    let review_reference = match review_stage {
        Some(Stage::Review {
            reference: Some(reference),
            ..
        }) => Some(load_reference(scenario_dir, reference)?),
        _ => None,
    };
    preflight_review(&scenario, review_reference.as_ref())?;
    let mut review = ReviewCollector::from_stage(review_stage, review_reference);

    let cart_text = crate::project::load_cart_text(cart_path).map_err(|error| error.to_string())?;
    let parsed_cart = Cart::parse(&cart_text).map_err(|error| error.to_string())?;
    let initial_save = scenario
        .initial_save
        .as_ref()
        .map(|initial| {
            let config = parsed_cart
                .save_config()
                .ok_or("scenario initial_save requires cart __meta__ save_id and save_version")?;
            let data =
                serde_json::from_value::<SaveValue>(initial.data.clone()).map_err(|error| {
                    format!("scenario initial_save data is not save-compatible: {error}")
                })?;
            canonical_save_document(config, initial.version, data)
                .map_err(|error| format!("scenario initial_save is invalid: {error}"))
        })
        .transpose()?;
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
    }) || review.layer_plan.is_some()
    {
        session.set_layer_capture(true);
    }
    session
        .load_cart_with_save(&cart_text, seed, initial_save.as_deref())
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
            &mut review,
        ) {
            report.status = "failed";
            report.error = Some(error);
            passed = false;
        }
        if passed && !matches!(stage, Stage::Review { .. }) {
            if let Err(error) = review.capture_after_stage(stage, &session) {
                report.status = "failed";
                report.error = Some(error);
                passed = false;
            }
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
    if let Some(initial) = &scenario.initial_save {
        if initial.version == 0 {
            return Err("scenario initial_save version must be a positive u32 integer".into());
        }
        serde_json::from_value::<SaveValue>(initial.data.clone()).map_err(|error| {
            format!("scenario initial_save data is not save-compatible: {error}")
        })?;
    }
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut ecs_watches = BTreeSet::new();
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
            Stage::EcsWatch {
                watch,
                define,
                artifact,
                ..
            } => {
                ecs_watch::name(watch, &format!("stage {index} ECS watch"))?;
                if let Some(definition) = define {
                    playtest_watch_definition(
                        watch,
                        definition,
                        &format!("stage {index} ECS watch"),
                    )?;
                    if !ecs_watches.insert(watch.as_str()) {
                        return Err(format!("stage {index} redefines ECS watch {watch:?}"));
                    }
                } else if !ecs_watches.contains(watch.as_str()) {
                    return Err(format!(
                        "stage {index} samples undefined ECS watch {watch:?}"
                    ));
                }
                if let Some(output) = artifact {
                    if artifacts.is_none() {
                        return Err(format!(
                            "stage {index} captures a file, so --artifacts <DIR> is required"
                        ));
                    }
                    let normalized = normalize_relative_path(output).map_err(|error| {
                        format!("stage {index} ECS watch path {output:?}: {error}")
                    })?;
                    if !paths.insert(duplicate_path_key(&normalized)) {
                        return Err(format!(
                            "stage {index} ECS watch path {output:?} aliases an earlier artifact; every normalized path must be unique"
                        ));
                    }
                }
            }
            Stage::Hook {
                hook, args, expect, ..
            } => {
                validate_name(hook).map_err(|error| format!("stage {index}: {error}"))?;
                json_to_dev_value(args).map_err(|error| format!("stage {index}: {error}"))?;
                if let Some(expect) = expect {
                    if let Some(field) = expect.field()
                        && (field.is_empty() || field.len() > console_core::DEV_HOOK_MAX_KEY_BYTES)
                    {
                        return Err(format!(
                            "stage {index} hook expectation field must be 1-{} bytes",
                            console_core::DEV_HOOK_MAX_KEY_BYTES
                        ));
                    }
                    match expect {
                        HookExpectation::AtLeast { value, .. }
                        | HookExpectation::GreaterThan { value, .. }
                            if !value.is_finite() =>
                        {
                            return Err(format!(
                                "stage {index} hook expectation value must be finite"
                            ));
                        }
                        _ => {}
                    }
                }
            }
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
            Stage::Review {
                board,
                report,
                stages,
                views,
                zoom,
                columns,
                motion_samples,
                reference,
                layers,
                map,
                temporal_checks,
                lint,
                ..
            } => {
                if index + 1 != scenario.stages.len() {
                    return Err(format!("stage {index} review must be the final stage"));
                }
                if artifacts.is_none() {
                    return Err(format!(
                        "stage {index} captures files, so --artifacts <DIR> is required"
                    ));
                }
                if stages.is_empty() {
                    return Err(format!("stage {index} review stages must not be empty"));
                }
                if views.is_empty() {
                    return Err(format!("stage {index} review views must not be empty"));
                }
                if !(1..=MAX_DIAGNOSTIC_ZOOM).contains(zoom) {
                    return Err(format!(
                        "stage {index} review zoom must be 1..={MAX_DIAGNOSTIC_ZOOM}, got {zoom}"
                    ));
                }
                if !(1..=MAX_DIAGNOSTIC_COLUMNS).contains(columns) {
                    return Err(format!(
                        "stage {index} review columns must be 1..={MAX_DIAGNOSTIC_COLUMNS}, got {columns}"
                    ));
                }
                if !(1..=MAX_MOTION_SAMPLES).contains(motion_samples) {
                    return Err(format!(
                        "stage {index} review motion_samples must be 1..={MAX_MOTION_SAMPLES}, got {motion_samples}"
                    ));
                }
                let unique_views = views.iter().copied().collect::<BTreeSet<_>>();
                if unique_views.len() != views.len() {
                    return Err(format!("stage {index} review repeats a diagnostic view"));
                }
                let prior = scenario.stages[..index]
                    .iter()
                    .filter_map(Stage::name)
                    .collect::<BTreeSet<_>>();
                let mut selected = BTreeSet::new();
                for stage_name in stages {
                    if stage_name.len() > 64 {
                        return Err(format!(
                            "stage {index} review stage name {stage_name:?} exceeds 64 UTF-8 bytes"
                        ));
                    }
                    if !selected.insert(stage_name.as_str()) {
                        return Err(format!("stage {index} review repeats stage {stage_name:?}"));
                    }
                    if !prior.contains(stage_name.as_str()) {
                        return Err(format!(
                            "stage {index} review references non-prior stage {stage_name:?}"
                        ));
                    }
                }
                if temporal_checks.len() > 16 {
                    return Err(format!(
                        "stage {index} review requests {} temporal checks; at most 16 are allowed",
                        temporal_checks.len()
                    ));
                }
                let mut check_names = BTreeSet::new();
                for check in temporal_checks {
                    let (name, referenced, limit, regions) = match check {
                        TemporalVisualCheck::Boundary {
                            name,
                            from,
                            to,
                            max_changed_fraction,
                            allowed_regions,
                            ..
                        } => (
                            name,
                            vec![from.as_str(), to.as_str()],
                            *max_changed_fraction,
                            allowed_regions,
                        ),
                        TemporalVisualCheck::Consecutive {
                            name,
                            stage,
                            max_changed_fraction,
                            allowed_regions,
                            ..
                        } => (
                            name,
                            vec![stage.as_str()],
                            *max_changed_fraction,
                            allowed_regions,
                        ),
                    };
                    if name.is_empty() || name.len() > 64 {
                        return Err(format!(
                            "stage {index} temporal check name {name:?} must be 1..=64 UTF-8 bytes"
                        ));
                    }
                    if !check_names.insert(name) {
                        return Err(format!(
                            "stage {index} repeats temporal check name {name:?}"
                        ));
                    }
                    if !limit.is_finite() || !(0.0..=1.0).contains(&limit) {
                        return Err(format!(
                            "stage {index} temporal check {name:?} max_changed_fraction must be in 0..=1"
                        ));
                    }
                    for stage_name in &referenced {
                        if !selected.contains(stage_name) {
                            return Err(format!(
                                "stage {index} temporal check {name:?} references {stage_name:?}, which is not selected by review stages"
                            ));
                        }
                    }

                    let dimensions = |stage_name: &str| -> Result<(u32, u32, bool, usize), String> {
                        let matches = scenario.stages[..index]
                            .iter()
                            .filter(|stage| stage.name() == Some(stage_name))
                            .collect::<Vec<_>>();
                        let [stage] = matches.as_slice() else {
                            return Err(format!(
                                "stage {index} temporal check {name:?} needs exactly one prior stage {stage_name:?}, found {}",
                                matches.len()
                            ));
                        };
                        match stage {
                            Stage::Sequence {
                                crop,
                                frames,
                                every,
                                ..
                            } => Ok((
                                crop.w,
                                crop.h,
                                true,
                                ((*frames / *every) as usize).min(*motion_samples as usize),
                            )),
                            _ => Ok((SCREEN_W as u32, SCREEN_H as u32, false, 1)),
                        }
                    };
                    let mut common_dimensions = None;
                    for stage_name in &referenced {
                        let (width, height, _, _) = dimensions(stage_name)?;
                        if let Some(expected) = common_dimensions {
                            if expected != (width, height) {
                                return Err(format!(
                                    "stage {index} temporal check {name:?} compares different dimensions"
                                ));
                            }
                        } else {
                            common_dimensions = Some((width, height));
                        }
                    }
                    let (width, height) = common_dimensions.expect("one referenced stage");
                    for region in regions {
                        let right = region.x.checked_add(region.w).ok_or_else(|| {
                            format!("stage {index} temporal allowed region x+w overflows")
                        })?;
                        let bottom = region.y.checked_add(region.h).ok_or_else(|| {
                            format!("stage {index} temporal allowed region y+h overflows")
                        })?;
                        if region.w == 0 || region.h == 0 || right > width || bottom > height {
                            return Err(format!(
                                "stage {index} temporal check {name:?} allowed region {},{},{},{} exceeds {width}x{height}",
                                region.x, region.y, region.w, region.h
                            ));
                        }
                    }
                    match check {
                        TemporalVisualCheck::Boundary { from, to, .. } => {
                            if dimensions(from)?.2 || dimensions(to)?.2 {
                                return Err(format!(
                                    "stage {index} boundary check {name:?} requires single-frame stages, not sequences"
                                ));
                            }
                        }
                        TemporalVisualCheck::Consecutive { stage, .. } => {
                            let (_, _, is_sequence, samples) = dimensions(stage)?;
                            if !is_sequence || samples < 2 {
                                return Err(format!(
                                    "stage {index} consecutive check {name:?} requires a reviewed sequence with at least two motion_samples"
                                ));
                            }
                        }
                    }
                }
                let mut source_count = scenario.stages[..index]
                    .iter()
                    .filter(|stage| stage.name().is_some_and(|name| selected.contains(name)))
                    .map(|stage| match stage {
                        Stage::Sequence { frames, every, .. } => {
                            ((*frames / *every) as usize).min(*motion_samples as usize)
                        }
                        _ => 1,
                    })
                    .sum::<usize>();
                if let Some(layers) = layers {
                    if layers.stage.len() > 64 {
                        return Err(format!(
                            "stage {index} review layer stage name exceeds 64 UTF-8 bytes"
                        ));
                    }
                    if !prior.contains(layers.stage.as_str()) {
                        return Err(format!(
                            "stage {index} review layers reference non-prior stage {:?}",
                            layers.stage
                        ));
                    }
                    if layers.tags.is_empty() && !layers.include_untagged {
                        return Err(format!(
                            "stage {index} review layers request no tags or untagged draws"
                        ));
                    }
                    let mut tags = BTreeSet::new();
                    for tag in &layers.tags {
                        if tag.len() > 64 {
                            return Err(format!(
                                "stage {index} review layer tag {tag:?} exceeds 64 UTF-8 bytes"
                            ));
                        }
                        if !tags.insert(tag) {
                            return Err(format!("stage {index} review repeats layer tag {tag:?}"));
                        }
                    }
                    source_count += layers.tags.len() + usize::from(layers.include_untagged);
                }
                if let Some(lint) = lint {
                    let Some(layers) = layers.as_deref() else {
                        return Err(format!("stage {index} visual lint requires review layers"));
                    };
                    let configured = usize::from(lint.reserved_collision_colors.is_some())
                        + usize::from(lint.bright_background_horizontals.is_some())
                        + usize::from(lint.actor_background_luma.is_some())
                        + usize::from(lint.traversal_corridor_edges.is_some());
                    if configured == 0 {
                        return Err(format!("stage {index} visual lint configures no checks"));
                    }
                    let require_tag = |tag: &str, kind: &str| -> Result<(), String> {
                        if !layers.tags.iter().any(|requested| requested == tag) {
                            return Err(format!(
                                "stage {index} {kind} lint tag {tag:?} is not requested by review layers"
                            ));
                        }
                        Ok(())
                    };
                    if let Some(check) = &lint.reserved_collision_colors {
                        require_tag(&check.source_tag, "reserved collision color")?;
                        if check.indices.is_empty()
                            || check
                                .indices
                                .iter()
                                .any(|&palette_index| palette_index > 63)
                        {
                            return Err(format!(
                                "stage {index} reserved collision color lint needs Apollo64 indices in 0..=63"
                            ));
                        }
                        let unique = check.indices.iter().copied().collect::<BTreeSet<_>>();
                        if unique.len() != check.indices.len() {
                            return Err(format!(
                                "stage {index} reserved collision color lint repeats an index"
                            ));
                        }
                    }
                    if let Some(check) = &lint.bright_background_horizontals {
                        require_tag(&check.background_tag, "bright horizontal")?;
                        if check.max_run == 0 || check.max_run > SCREEN_W as u32 {
                            return Err(format!(
                                "stage {index} bright horizontal max_run must be 1..={SCREEN_W}"
                            ));
                        }
                    }
                    if let Some(check) = &lint.actor_background_luma {
                        require_tag(&check.actor_tag, "actor luma")?;
                        require_tag(&check.background_tag, "background luma")?;
                    }
                    if let Some(check) = &lint.traversal_corridor_edges {
                        require_tag(&check.background_tag, "corridor edge")?;
                        if !check.max_edge_fraction.is_finite()
                            || !(0.0..=1.0).contains(&check.max_edge_fraction)
                        {
                            return Err(format!(
                                "stage {index} traversal corridor max_edge_fraction must be in 0..=1"
                            ));
                        }
                        let right =
                            check.region.x.checked_add(check.region.w).ok_or_else(|| {
                                format!("stage {index} traversal corridor x+w overflows")
                            })?;
                        let bottom =
                            check.region.y.checked_add(check.region.h).ok_or_else(|| {
                                format!("stage {index} traversal corridor y+h overflows")
                            })?;
                        if check.region.w == 0
                            || check.region.h == 0
                            || right > SCREEN_W as u32
                            || bottom > SCREEN_H as u32
                        {
                            return Err(format!(
                                "stage {index} traversal corridor exceeds the {SCREEN_W}x{SCREEN_H} framebuffer"
                            ));
                        }
                    }
                }
                if let Some(map) = map {
                    if map.stage.len() > 64 {
                        return Err(format!(
                            "stage {index} review map stage name exceeds 64 UTF-8 bytes"
                        ));
                    }
                    if !prior.contains(map.stage.as_str()) {
                        return Err(format!(
                            "stage {index} review map references non-prior stage {:?}",
                            map.stage
                        ));
                    }
                    if !(1..=MAX_MAP_ZOOM).contains(&map.zoom) {
                        return Err(format!(
                            "stage {index} review map zoom must be 1..={MAX_MAP_ZOOM}, got {}",
                            map.zoom
                        ));
                    }
                    // A live map's used extent is unknown until the selected
                    // stage runs. Preflight against a fully occupied map so
                    // no runtime topology can exceed the visual memory bound.
                    let full_tiles = Box::new([1 as console_core::TileId; console_core::MAP_LEN]);
                    let (_, _, width_cells, height_cells) =
                        map::parse_region(map.region.as_deref(), &full_tiles).map_err(|error| {
                            format!("stage {index} review map region is invalid: {error}")
                        })?;
                    let (width, height) = review_map_dimensions(
                        index,
                        map.zoom,
                        map.grid,
                        width_cells,
                        height_cells,
                    )?;
                    let bytes = (width as usize)
                        .checked_mul(height as usize)
                        .and_then(|pixels| pixels.checked_mul(4))
                        .ok_or_else(|| format!("stage {index} review map RGBA size overflows"))?;
                    if bytes > visual::MAX_VISUAL_RGBA_BYTES {
                        return Err(format!(
                            "stage {index} review map can need {bytes} RGBA bytes, exceeding the {} byte limit",
                            visual::MAX_VISUAL_RGBA_BYTES
                        ));
                    }
                    source_count += 1;
                }
                source_count += usize::from(reference.is_some());
                if source_count > MAX_DIAGNOSTIC_SOURCES {
                    return Err(format!(
                        "stage {index} review requests {source_count} sources; at most {MAX_DIAGNOSTIC_SOURCES} are allowed"
                    ));
                }
                let panel_count = source_count
                    .checked_mul(views.len())
                    .ok_or_else(|| format!("stage {index} review panel count overflows"))?;
                if panel_count > 120 {
                    return Err(format!(
                        "stage {index} review requests {panel_count} panels; at most 120 are allowed"
                    ));
                }
                let heatmap_paths = temporal_checks.iter().filter_map(|check| match check {
                    TemporalVisualCheck::Boundary { heatmap, .. }
                    | TemporalVisualCheck::Consecutive { heatmap, .. } => heatmap.as_deref(),
                });
                for output in std::iter::once(board.as_str())
                    .chain(report.as_deref())
                    .chain(heatmap_paths)
                {
                    let normalized = normalize_relative_path(output).map_err(|error| {
                        format!("stage {index} review path {output:?}: {error}")
                    })?;
                    if !paths.insert(duplicate_path_key(&normalized)) {
                        return Err(format!(
                            "stage {index} review path {output:?} aliases an earlier artifact; every normalized path must be unique"
                        ));
                    }
                }
            }
            Stage::Capture {
                screenshot,
                zoom,
                screen_text,
                screen_text_region,
                screen_text_summary,
                wav,
                spectrogram,
                audio_events,
                audio_stats,
                text_events,
                draw_trace,
                save,
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
                    screen_text_summary.as_deref(),
                    wav.as_deref(),
                    spectrogram.as_deref(),
                    audio_events.as_deref(),
                    audio_stats.as_deref(),
                    text_events.as_deref(),
                    draw_trace.as_deref(),
                    save.as_deref(),
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
                if screen_text_region.is_some()
                    && screen_text.is_none()
                    && screen_text_summary.is_none()
                {
                    return Err(format!(
                        "stage {index} screen_text_region requires screen_text or screen_text_summary output"
                    ));
                }
                if let Some(region) = screen_text_region {
                    let region = region.validate().map_err(|error| {
                        format!("stage {index} screen_text_region is invalid: {error}")
                    })?;
                    if screen_text.is_some()
                        && !region.is_full()
                        && region.pixel_count() > MAX_SCREEN_TEXT_REGION_PIXELS
                    {
                        return Err(format!(
                            "stage {index} screen_text_region contains {} pixels; cropped line output is limited to {MAX_SCREEN_TEXT_REGION_PIXELS} pixels (capture only screen_text_summary or select a smaller region)",
                            region.pixel_count()
                        ));
                    }
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
            Stage::SaveAssert {
                version, equals, ..
            } => {
                if *version == Some(0) {
                    return Err(format!(
                        "stage {index} save_assert version must be a positive u32 integer"
                    ));
                }
                serde_json::from_value::<SaveValue>(equals.clone()).map_err(|error| {
                    format!("stage {index} save_assert value is not save-compatible: {error}")
                })?;
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

fn review_map_dimensions(
    index: usize,
    zoom: u32,
    grid: bool,
    width_cells: u32,
    height_cells: u32,
) -> Result<(u32, u32), String> {
    let extra = u32::from(grid);
    let width = width_cells
        .checked_mul(8)
        .and_then(|width| width.checked_mul(zoom))
        .and_then(|width| width.checked_add(extra))
        .ok_or_else(|| format!("stage {index} review map width overflows"))?;
    let height = height_cells
        .checked_mul(8)
        .and_then(|height| height.checked_mul(zoom))
        .and_then(|height| height.checked_add(extra))
        .ok_or_else(|| format!("stage {index} review map height overflows"))?;
    Ok((width, height))
}

fn preflight_review(
    scenario: &Scenario,
    reference_image: Option<&visual::RgbaImage>,
) -> Result<(), String> {
    let Some((review_index, review_stage)) = scenario
        .stages
        .iter()
        .enumerate()
        .find(|(_, stage)| matches!(stage, Stage::Review { .. }))
    else {
        return Ok(());
    };
    let Stage::Review {
        stages,
        views,
        zoom,
        columns,
        motion_samples,
        reference,
        layers,
        map,
        ..
    } = review_stage
    else {
        unreachable!();
    };

    let selected = stages.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut frame_count = 0u64;
    let mut sources = Vec::<(String, u32, u32)>::new();
    for stage in &scenario.stages[..review_index] {
        let frame_start = frame_count;
        match stage {
            Stage::Input { frames, .. } | Stage::Sequence { frames, .. } => {
                frame_count = frame_count
                    .checked_add(*frames)
                    .ok_or("review preflight frame count overflows")?;
            }
            _ => {}
        }
        let Some(name) = stage.name().filter(|name| selected.contains(*name)) else {
            continue;
        };
        if let Stage::Sequence {
            frames,
            every,
            crop,
            ..
        } = stage
        {
            let sample_count = (*frames / *every) as usize;
            let count = sample_count.min(*motion_samples as usize);
            let selected_indices = if count == 1 {
                vec![sample_count - 1]
            } else {
                (0..count)
                    .map(|sample| sample * (sample_count - 1) / (count - 1))
                    .collect()
            };
            for sample in selected_indices {
                let frame_offset = (sample as u64 + 1)
                    .checked_mul(*every)
                    .ok_or("review preflight sequence frame offset overflows")?;
                let frame = frame_start
                    .checked_add(frame_offset)
                    .ok_or("review preflight sequence frame number overflows")?;
                sources.push((format!("{name} F{frame}"), crop.w, crop.h));
            }
        } else {
            sources.push((
                format!("{name} F{frame_count}"),
                SCREEN_W as u32,
                SCREEN_H as u32,
            ));
        }
    }

    if let Some(layers) = layers {
        for tag in &layers.tags {
            sources.push((
                format!("LAYER {tag} @ {}", layers.stage),
                SCREEN_W as u32,
                SCREEN_H as u32,
            ));
        }
        if layers.include_untagged {
            sources.push((
                format!("LAYER UNTAGGED @ {}", layers.stage),
                SCREEN_W as u32,
                SCREEN_H as u32,
            ));
        }
    }
    if let Some(map) = map {
        let full_tiles = Box::new([1 as console_core::TileId; console_core::MAP_LEN]);
        let (_, _, width_cells, height_cells) =
            map::parse_region(map.region.as_deref(), &full_tiles).map_err(|error| {
                format!("stage {review_index} review map region is invalid: {error}")
            })?;
        let (width, height) =
            review_map_dimensions(review_index, map.zoom, map.grid, width_cells, height_cells)?;
        sources.push((
            format!("MAP {} @ {}", map_source_name(map.source), map.stage),
            width,
            height,
        ));
    }
    if reference.is_some() {
        let image = reference_image.ok_or("review reference was not decoded for preflight")?;
        sources.push(("REFERENCE".to_string(), image.width, image.height));
    }

    let source_preflight = sources
        .iter()
        .map(|(label, width, height)| visual::DiagnosticSourcePreflight {
            label,
            width: *width,
            height: *height,
        })
        .collect::<Vec<_>>();
    visual::preflight_diagnostic_board(&visual::DiagnosticBoardPreflight {
        sources: &source_preflight,
        views,
        zoom: *zoom,
        columns: *columns,
    })
    .map_err(|error| format!("stage {review_index} review dimensions are invalid: {error}"))
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
    review: &mut ReviewCollector,
) -> Result<(), String> {
    match stage {
        Stage::Eval { code, .. } => {
            let value = session.eval(code).map_err(|error| error.to_string())?;
            report.actual = Some(lua_to_json(&value));
        }
        Stage::Hook {
            hook, args, expect, ..
        } => {
            let info = session
                .dev_hooks()
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|info| info.name == *hook)
                .ok_or_else(|| format!("unknown development hook {hook:?}"))?;
            let args = json_to_dev_value(args)?;
            let invocation = session
                .invoke_dev_hook(hook, info.phase, args)
                .map_err(|error| error.to_string())?;
            let result = dev_value_to_json(&invocation.result);
            if let Some(expect) = expect {
                let actual = select_hook_value(&result, expect.field())?;
                let (expected, passed, description) = match expect {
                    HookExpectation::Equals { value, .. } => {
                        (value.clone(), actual == value, format!("equal {value}"))
                    }
                    HookExpectation::NotEquals { value, .. } => {
                        (value.clone(), actual != value, format!("not equal {value}"))
                    }
                    HookExpectation::AtLeast { value, .. } => (
                        json!(value),
                        actual.as_f64().is_some_and(|actual| actual >= *value),
                        format!("be at least {value}"),
                    ),
                    HookExpectation::GreaterThan { value, .. } => (
                        json!(value),
                        actual.as_f64().is_some_and(|actual| actual > *value),
                        format!("be greater than {value}"),
                    ),
                };
                report.expected = Some(expected);
                report.actual = Some(actual.clone());
                if !passed {
                    return Err(format!(
                        "hook expectation failed: expected result{} to {description}, got {actual}",
                        expect
                            .field()
                            .map(|field| format!(" field {field:?}"))
                            .unwrap_or_default()
                    ));
                }
            } else {
                report.actual = Some(json!({
                    "name": invocation.name,
                    "phase": invocation.phase,
                    "frame_count": invocation.frame_count,
                    "result": result,
                }));
            }
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
            let collect_review = review.wants_stage(name.as_deref());
            let mut review_indices = Vec::with_capacity(if collect_review {
                sample_count as usize
            } else {
                0
            });
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
                let (rgba, indices) = {
                    let console = session.console().map_err(|error| error.to_string())?;
                    (
                        crate::session::framebuffer_rgba(
                            console.framebuffer(),
                            console.display_palette(),
                        ),
                        collect_review.then(|| {
                            presented_indices(console.framebuffer(), console.display_palette())
                        }),
                    )
                };
                let frame = RgbaImage::new(SCREEN_W as u32, SCREEN_H as u32, rgba)?;
                images.push(frame.crop(rect)?);
                if let Some(indices) = indices {
                    review_indices.push(crop_indices(&indices, rect)?);
                }
                frame_numbers.push(outcome.frame_count);
            }
            if frame_numbers != expected_frame_numbers {
                return Err(format!(
                    "sequence cadence drifted: expected {expected_frame_numbers:?}, got {frame_numbers:?}"
                ));
            }
            if collect_review {
                review.capture_sequence(
                    name.as_deref(),
                    &images,
                    &review_indices,
                    &frame_numbers,
                )?;
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
        Stage::EcsWatch {
            watch,
            define,
            artifact,
            ..
        } => {
            if let Some(definition) = define {
                let definition =
                    playtest_watch_definition(watch, definition, "playtest ECS watch")?;
                session
                    .define_ecs_watch(definition)
                    .map_err(|error| error.to_string())?;
            }
            let sample = session
                .sample_ecs_watch(watch)
                .map_err(|error| error.to_string())?;
            let actual = serde_json::to_value(&sample)
                .map_err(|error| format!("serializing ECS watch sample: {error}"))?;
            if let Some(path) = artifact {
                let root = artifact_root.expect("ECS watch artifact validation requires a root");
                let mut bytes = serde_json::to_vec_pretty(&actual)
                    .map_err(|error| format!("serializing ECS watch artifact: {error}"))?;
                bytes.push(b'\n');
                report
                    .artifacts
                    .push(write_artifact(root, path, "ecs_watch", &bytes)?);
            }
            report.actual = Some(actual);
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
        Stage::SaveAssert {
            version, equals, ..
        } => {
            let actual = current_save_json(session)?;
            let actual_data = actual
                .as_ref()
                .and_then(|value| value.get("data"))
                .cloned()
                .unwrap_or(Json::Null);
            let actual_version = actual
                .as_ref()
                .and_then(|value| value.get("version"))
                .and_then(Json::as_u64);
            report.expected = Some(json!({"data": equals, "version": version}));
            report.actual = Some(actual.clone().unwrap_or(Json::Null));
            let version_matches =
                version.is_none_or(|expected| actual_version == Some(u64::from(expected)));
            if actual_data != *equals || !version_matches {
                return Err(format!(
                    "save assertion failed: expected data {}{}; got {}",
                    equals,
                    version
                        .map(|value| format!(" at version {value}"))
                        .unwrap_or_default(),
                    actual.map_or_else(|| "no save".into(), |value| value.to_string())
                ));
            }
        }
        Stage::Review {
            board,
            report: report_path,
            views,
            zoom,
            columns,
            temporal_checks,
            lint,
            ..
        } => {
            let root = artifact_root.expect("review validation requires an artifact root");
            let sources = review.finish_sources(stage, session)?;
            let mut temporal_results = Vec::with_capacity(temporal_checks.len());
            let mut heatmaps = Vec::new();
            let mut failures = Vec::new();
            for check in temporal_checks {
                let (name, kind, from, to, limit, comparison, heatmap_path) = match check {
                    TemporalVisualCheck::Boundary {
                        name,
                        from,
                        to,
                        max_changed_fraction,
                        allowed_regions,
                        heatmap,
                    } => {
                        let from_sources = stage_review_sources(&sources, from);
                        let to_sources = stage_review_sources(&sources, to);
                        let [from_source] = from_sources.as_slice() else {
                            return Err(format!(
                                "boundary check {name:?} needs exactly one source for stage {from:?}, found {}",
                                from_sources.len()
                            ));
                        };
                        let [to_source] = to_sources.as_slice() else {
                            return Err(format!(
                                "boundary check {name:?} needs exactly one source for stage {to:?}, found {}",
                                to_sources.len()
                            ));
                        };
                        (
                            name,
                            "boundary",
                            from_source.label.clone(),
                            to_source.label.clone(),
                            *max_changed_fraction,
                            compare_visual_sources(from_source, to_source, allowed_regions)?,
                            heatmap.as_deref(),
                        )
                    }
                    TemporalVisualCheck::Consecutive {
                        name,
                        stage,
                        max_changed_fraction,
                        allowed_regions,
                        heatmap,
                    } => {
                        let stage_sources = stage_review_sources(&sources, stage);
                        if stage_sources.len() < 2 {
                            return Err(format!(
                                "consecutive check {name:?} needs at least two sources for stage {stage:?}, found {}",
                                stage_sources.len()
                            ));
                        }
                        let mut worst = None;
                        for pair in stage_sources.windows(2) {
                            let comparison =
                                compare_visual_sources(pair[0], pair[1], allowed_regions)?;
                            if worst.as_ref().is_none_or(
                                |(_, _, current): &(String, String, TemporalComparison)| {
                                    comparison.changed_fraction > current.changed_fraction
                                },
                            ) {
                                worst = Some((
                                    pair[0].label.clone(),
                                    pair[1].label.clone(),
                                    comparison,
                                ));
                            }
                        }
                        let (from, to, comparison) = worst.expect("two sources have one pair");
                        (
                            name,
                            "consecutive",
                            from,
                            to,
                            *max_changed_fraction,
                            comparison,
                            heatmap.as_deref(),
                        )
                    }
                };
                let passed = comparison.changed_fraction <= limit;
                if !passed {
                    failures.push(format!(
                        "{name} changed {:.6} > {:.6}",
                        comparison.changed_fraction, limit
                    ));
                }
                if let Some(path) = heatmap_path {
                    heatmaps.push((path.to_string(), comparison.heatmap.png()));
                }
                temporal_results.push(TemporalVisualResult {
                    name: name.clone(),
                    kind,
                    from,
                    to,
                    compared_pixels: comparison.compared_pixels,
                    changed_pixels: comparison.changed_pixels,
                    changed_fraction: comparison.changed_fraction,
                    max_changed_fraction: limit,
                    passed,
                });
            }
            let warnings = visual_lint_warnings(lint.as_deref(), &sources)?;
            let (image, layout, metrics) =
                visual::diagnostic_board(&visual::DiagnosticBoardSpec {
                    sources: &sources,
                    views,
                    zoom: *zoom,
                    columns: *columns,
                })?;
            let payload = json!({
                "kind": "visual_diagnostics",
                "interpretation": "evidence_only_no_aesthetic_score",
                "views": views,
                "layout": layout,
                "sources": metrics,
                "temporal_checks": temporal_results,
                "warnings": warnings,
            });
            let mut report_bytes = serde_json::to_vec_pretty(&payload)
                .map_err(|error| format!("serializing visual diagnostic report: {error}"))?;
            report_bytes.push(b'\n');
            let board_png = image.png();
            report.artifacts.push(write_artifact(
                root,
                board,
                "visual_review_board",
                &board_png,
            )?);
            if let Some(path) = report_path {
                report.artifacts.push(write_artifact(
                    root,
                    path,
                    "visual_review_report",
                    &report_bytes,
                )?);
            }
            for (path, bytes) in heatmaps {
                report
                    .artifacts
                    .push(write_artifact(root, &path, "visual_diff_heatmap", &bytes)?);
            }
            report.actual = Some(payload);
            if !failures.is_empty() {
                return Err(format!(
                    "temporal visual assertion failed: {}",
                    failures.join("; ")
                ));
            }
        }
        Stage::Capture {
            screenshot,
            zoom,
            screen_text,
            screen_text_region,
            screen_text_summary,
            wav,
            spectrogram,
            audio_events,
            audio_stats,
            text_events,
            draw_trace,
            save,
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
                let region = screen_text_region
                    .as_ref()
                    .copied()
                    .unwrap_or_else(ScreenTextRegion::full);
                let lines = session
                    .screen_text_report(region, true)
                    .map_err(|error| error.to_string())?
                    .lines
                    .expect("playtest raw screen text includes lines");
                let mut text = lines.join("\n");
                text.push('\n');
                report
                    .artifacts
                    .push(write_artifact(root, name, "screen_text", text.as_bytes())?);
            }
            if let Some(name) = screen_text_summary {
                let region = screen_text_region
                    .as_ref()
                    .copied()
                    .unwrap_or_else(ScreenTextRegion::full);
                let summary = session
                    .screen_text_report(region, false)
                    .map_err(|error| error.to_string())?;
                let mut bytes = serde_json::to_vec_pretty(&summary)
                    .map_err(|error| format!("serializing screen text summary: {error}"))?;
                bytes.push(b'\n');
                report
                    .artifacts
                    .push(write_artifact(root, name, "screen_text_summary", &bytes)?);
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
            if let Some(name) = save {
                let value = current_save_json(session)?.unwrap_or(Json::Null);
                let mut bytes = serde_json::to_vec_pretty(&value)
                    .map_err(|error| format!("serializing save artifact: {error}"))?;
                bytes.push(b'\n');
                report
                    .artifacts
                    .push(write_artifact(root, name, "save", &bytes)?);
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

fn current_save_json(session: &Session) -> Result<Option<Json>, String> {
    session
        .save_document()
        .map_err(|error| error.to_string())?
        .map(|document| {
            serde_json::from_str(&document)
                .map_err(|error| format!("core returned an invalid save document: {error}"))
        })
        .transpose()
}

fn select_hook_value<'a>(result: &'a Json, field: Option<&str>) -> Result<&'a Json, String> {
    let Some(field) = field else {
        return Ok(result);
    };
    result
        .as_object()
        .and_then(|object| object.get(field))
        .ok_or_else(|| format!("development hook result has no top-level field {field:?}"))
}

fn frame_count(session: &Session) -> u64 {
    session
        .console()
        .map(|console| console.frame_count())
        .unwrap_or(0)
}

fn load_reference(scenario_dir: &Path, reference: &str) -> Result<RgbaImage, String> {
    if reference.trim().is_empty() {
        return Err("visual reference path is empty".to_string());
    }
    let reference_path = Path::new(reference);
    let path = if reference_path.is_absolute() {
        reference_path.to_path_buf()
    } else {
        scenario_dir.join(reference_path)
    };
    let bytes = fs::read(&path)
        .map_err(|error| format!("reading visual reference {}: {error}", path.display()))?;
    let decoded = crate::palette::decode_png_rgba(&bytes)
        .map_err(|error| format!("decoding visual reference {}: {error}", path.display()))?;
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

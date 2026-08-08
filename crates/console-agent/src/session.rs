//! Session state shared by the oneshot CLI and the JSON-RPC `serve` loop.
//!
//! A [`Session`] owns at most one running [`Console`], the cart text and
//! seed it was built from, the full per-frame input log since the last
//! reset/load (needed for save/load state and for replay-based resets), a
//! parallel per-frame audio sample log and sequencer event log (see
//! [`crate::audio`]), and a table of named save states.

use std::collections::{BTreeMap, VecDeque};

use console_core::{
    CHANNEL_COUNT, COLOR_COUNT, COLOR_MASK, ChannelInfo, Console, DevHookInfo, DevHookPhase,
    DevValue, DrawEvent, DrawTraceFrame, Error, FB_LEN, LAYER_TRANSPARENT, PALETTE, PlatformEvent,
    PlatformEventFrame, SAMPLES_PER_FRAME, SCREEN_H, SCREEN_W, TextDraw, color_char, input,
};
use serde::Serialize;

use crate::audio::{self, AudioEvent, AudioState, Spectrogram, StatsWindow};
use crate::ecs_watch::{QueryDefinition, WatchDefinition, WatchMetadata, WatchSample, WatchStore};
use crate::value::lua_to_json;

/// A named save state: enough to recreate the exact console state by
/// rebuilding the cart/registrations, then replaying steps and hooks in order.
#[derive(Clone)]
pub struct SavedState {
    pub seed: u64,
    /// Host document supplied before the original cart `_init`. Replays must
    /// not substitute whatever ambient backend happens to contain later.
    pub initial_save: Option<String>,
    pub input_log: Vec<u8>,
    replay_log: Vec<ReplayEvent>,
}

#[derive(Clone)]
enum ReplayEvent {
    Step(u8),
    DevHook {
        name: String,
        phase: DevHookPhase,
        args: DevValue,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DevHookInvocation {
    pub name: String,
    pub phase: DevHookPhase,
    pub frame_count: u64,
    pub result: DevValue,
}

/// The result of a `step` (or the replay inside `load_state`).
pub struct StepOutcome {
    pub frame_count: u64,
    pub halted: bool,
    pub message: Option<String>,
}

/// One Lua `print` call placed on a particular completed frame. Positions and
/// bounds are screen-space after camera subtraction; the original world-space
/// anchor is retained separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextEvent {
    pub frame: u64,
    pub text: String,
    pub align: String,
    pub anchor_x: i32,
    pub anchor_y: i32,
    pub screen_anchor_x: i32,
    pub screen_anchor_y: i32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub color: u8,
    pub visible: bool,
    pub clipped: bool,
}

/// Maximum draw calls retained across frames by an agent session. The core
/// independently caps each frame; this second cap keeps long traced runs
/// bounded while retaining the most recent evidence.
pub const MAX_SESSION_DRAW_EVENTS: usize = 65_536;

/// One core draw call stamped with its completed runtime frame and stable
/// within-frame order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DrawEventRecord {
    pub frame: u64,
    pub index: u64,
    #[serde(flatten)]
    pub event: DrawEvent,
}

/// Serializable snapshot returned by RPC, one-shot runs, and playtests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DrawTraceReport {
    pub enabled: bool,
    pub capacity: usize,
    pub dropped: u64,
    pub events: Vec<DrawEventRecord>,
}

/// Maximum cart-to-platform events retained by a native agent session.
pub const MAX_SESSION_PLATFORM_EVENTS: usize = 65_536;

/// One core platform event stamped with the agent session's completed frame
/// and stable within-frame order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformEventRecord {
    pub frame: u64,
    pub index: u64,
    #[serde(flatten)]
    pub event: PlatformEvent,
}

/// Serializable host-neutral platform diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformEventReport {
    pub capacity: usize,
    pub dropped: u64,
    pub max_submitted_score: Option<u64>,
    pub events: Vec<PlatformEventRecord>,
}

/// A transparent PNG for one isolated `draw_tag()` layer.
pub struct LayerScreenshot {
    pub tag: Option<String>,
    pub png: Vec<u8>,
}

pub struct LayerScreenshotSet {
    pub capacity: usize,
    pub dropped: u32,
    pub layers: Vec<LayerScreenshot>,
}

/// Maximum number of characters emitted by a cropped `screen_text` request.
/// The exact full framebuffer remains available as an explicit legacy dump;
/// arbitrary crops are bounded so a misplaced rectangle cannot flood an
/// agent's context.
pub const MAX_SCREEN_TEXT_REGION_PIXELS: usize = 16_384;

/// A strict native-framebuffer rectangle. Coordinates in reports are always
/// absolute screen coordinates, never relative to the selected region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenTextRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ScreenTextRegion {
    pub const fn full() -> Self {
        Self {
            x: 0,
            y: 0,
            width: SCREEN_W as u32,
            height: SCREEN_H as u32,
        }
    }

    pub fn validate(self) -> Result<Self, SessionError> {
        if self.width == 0 || self.height == 0 {
            return Err(SessionError::BadParams(
                "screen_text region width and height must be at least 1".to_string(),
            ));
        }
        let right = self.x.checked_add(self.width).ok_or_else(|| {
            SessionError::BadParams("screen_text region x + width overflows".to_string())
        })?;
        let bottom = self.y.checked_add(self.height).ok_or_else(|| {
            SessionError::BadParams("screen_text region y + height overflows".to_string())
        })?;
        if right > SCREEN_W as u32 || bottom > SCREEN_H as u32 {
            return Err(SessionError::BadParams(format!(
                "screen_text region must fit inside the {SCREEN_W}x{SCREEN_H} framebuffer; got x={}, y={}, width={}, height={}",
                self.x, self.y, self.width, self.height
            )));
        }
        Ok(self)
    }

    pub fn pixel_count(self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn is_full(self) -> bool {
        self == Self::full()
    }
}

/// What a bounded framebuffer diagnostic omitted. `cropped_pixels` counts
/// framebuffer pixels outside `region`; `line_characters_omitted` counts
/// selected pixels whose glyphs were intentionally suppressed by summary
/// mode. They are independent so consumers do not have to infer either kind
/// of truncation from dimensions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScreenTextTruncation {
    pub truncated: bool,
    pub cropped_pixels: usize,
    pub crop_left: u32,
    pub crop_top: u32,
    pub crop_right: u32,
    pub crop_bottom: u32,
    pub lines_omitted: bool,
    pub line_count_omitted: u32,
    pub line_characters_omitted: usize,
}

/// Machine-readable framebuffer text diagnostics shared by RPC, one-shot
/// summary output, and playtest JSON artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScreenTextReport {
    pub framebuffer_width: u32,
    pub framebuffer_height: u32,
    pub region: ScreenTextRegion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<String>>,
    pub palette_counts: BTreeMap<u8, usize>,
    pub glyph_counts: BTreeMap<char, usize>,
    pub non_background_bounds: Option<ScreenTextRegion>,
    pub truncation: ScreenTextTruncation,
}

fn record_text_draws(events: &mut Vec<TextEvent>, frame: u64, draws: Vec<TextDraw>) {
    events.extend(draws.into_iter().map(|draw| TextEvent {
        frame,
        text: draw.text,
        align: draw.align.as_str().to_string(),
        anchor_x: draw.anchor_x,
        anchor_y: draw.anchor_y,
        screen_anchor_x: draw.layout.anchor_x,
        screen_anchor_y: draw.layout.anchor_y,
        x: draw.layout.x,
        y: draw.layout.y,
        width: draw.layout.width,
        height: draw.layout.height,
        color: draw.color,
        visible: draw.layout.visible,
        clipped: draw.layout.clipped,
    }));
}

fn record_draw_events(
    events: &mut VecDeque<DrawEventRecord>,
    dropped: &mut u64,
    index_frame: &mut Option<u64>,
    next_index: &mut u64,
    frame: u64,
    trace: DrawTraceFrame,
) {
    if *index_frame != Some(frame) {
        *index_frame = Some(frame);
        *next_index = 0;
    }
    *dropped = dropped.saturating_add(u64::from(trace.dropped));
    for event in trace.events {
        if events.len() == MAX_SESSION_DRAW_EVENTS {
            events.pop_front();
            *dropped = dropped.saturating_add(1);
        }
        events.push_back(DrawEventRecord {
            frame,
            index: *next_index,
            event,
        });
        *next_index = next_index.saturating_add(1);
    }
    *next_index = next_index.saturating_add(u64::from(trace.dropped));
}

fn record_platform_events(
    events: &mut VecDeque<PlatformEventRecord>,
    dropped: &mut u64,
    index_frame: &mut Option<u64>,
    next_index: &mut u64,
    max_submitted_score: &mut Option<u64>,
    frame: u64,
    platform: PlatformEventFrame,
) {
    if *index_frame != Some(frame) {
        *index_frame = Some(frame);
        *next_index = 0;
    }
    *dropped = dropped.saturating_add(u64::from(platform.dropped));
    for event in platform.events {
        if let PlatformEvent::ScoreSubmit { score } = event {
            *max_submitted_score = Some(max_submitted_score.map_or(score, |best| best.max(score)));
        }
        if events.len() == MAX_SESSION_PLATFORM_EVENTS {
            events.pop_front();
            *dropped = dropped.saturating_add(1);
        }
        events.push_back(PlatformEventRecord {
            frame,
            index: *next_index,
            event,
        });
        *next_index = next_index.saturating_add(1);
    }
    *next_index = next_index.saturating_add(u64::from(platform.dropped));
}

/// Errors from session operations that aren't Lua/cart errors (those are
/// carried as `console_core::Error` and mapped to `-32000` by the RPC
/// layer). These map to other JSON-RPC codes.
#[derive(Debug)]
pub enum SessionError {
    /// No cart has been loaded yet (`-32002`).
    NoCart,
    /// Bad/missing parameters (`-32602`).
    BadParams(String),
    /// The console halted on a previous step and this call requires it not
    /// to have (`-32000`); carries the halt message.
    AlreadyHalted(String),
    /// A cart/Lua error surfaced while running (`-32000`).
    Cart(Error),
    /// Filesystem error (reading a cart path, writing a screenshot).
    Io(String),
}

impl From<Error> for SessionError {
    fn from(e: Error) -> Self {
        SessionError::Cart(e)
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NoCart => write!(f, "no cart loaded"),
            SessionError::BadParams(m) => write!(f, "{m}"),
            SessionError::AlreadyHalted(m) => write!(f, "console halted: {m}"),
            SessionError::Cart(e) => write!(f, "{e}"),
            SessionError::Io(m) => write!(f, "{m}"),
        }
    }
}

/// A channel with no sfx/music claim: the baseline the very first frame's
/// audio events are diffed against, so a channel that's *already* busy on
/// frame 1 (e.g. `music()` started from `_init`, before any `step()`) still
/// emits a `note_on`.
fn idle_channels() -> [ChannelInfo; CHANNEL_COUNT] {
    [ChannelInfo {
        sfx: None,
        row: 0,
        wave: 0,
        vol: 0,
        from_music: false,
        busy: false,
    }; CHANNEL_COUNT]
}

pub struct Session {
    cart_text: Option<String>,
    console: Option<Console>,
    seed: u64,
    initial_save: Option<String>,
    input_log: Vec<u8>,
    replay_log: Vec<ReplayEvent>,
    saved_states: BTreeMap<String, SavedState>,
    /// Every sample rendered by every stepped frame since the last
    /// reset/load, in order. `audio_log.len() / SAMPLES_PER_FRAME` is the
    /// number of frames stepped.
    audio_log: Vec<f32>,
    /// Sequencer events (note_on/row_change/note_off/music_pattern_change)
    /// derived by diffing `audio_channels()` frame over frame.
    audio_events: Vec<AudioEvent>,
    /// The channel snapshot the *next* stepped frame diffs against.
    prev_channels: [ChannelInfo; CHANNEL_COUNT],
    prev_pattern: Option<u8>,
    /// Every text draw since load/reset, including frame-zero `_init` calls.
    text_events: Vec<TextEvent>,
    /// Bounded cart-to-host platform diagnostics. The submitted maximum is
    /// host-owned and deliberately absent from cart state and save states.
    platform_events: VecDeque<PlatformEventRecord>,
    platform_events_dropped: u64,
    platform_event_index_frame: Option<u64>,
    platform_event_next_index: u64,
    max_submitted_score: Option<u64>,
    /// Opt-in bounded draw-call diagnostics. This host-side switch survives
    /// load/reset/load_state even though each operation replaces the core.
    draw_tracing: bool,
    draw_events: VecDeque<DrawEventRecord>,
    draw_events_dropped: u64,
    draw_event_index_frame: Option<u64>,
    draw_event_next_index: u64,
    /// Host-side opt-in that survives load/reset/load_state just like draw
    /// tracing. Layer buffers themselves remain current-frame core state.
    layer_capture: bool,
    /// Named, bounded ECS diagnostics. Definitions survive a rewind, while
    /// their baselines are cleared so deltas never cross reset boundaries.
    ecs_watches: WatchStore,
}

impl Default for Session {
    fn default() -> Session {
        Session {
            cart_text: None,
            console: None,
            seed: 0,
            initial_save: None,
            input_log: Vec::new(),
            replay_log: Vec::new(),
            saved_states: BTreeMap::new(),
            audio_log: Vec::new(),
            audio_events: Vec::new(),
            prev_channels: idle_channels(),
            prev_pattern: None,
            text_events: Vec::new(),
            platform_events: VecDeque::new(),
            platform_events_dropped: 0,
            platform_event_index_frame: None,
            platform_event_next_index: 0,
            max_submitted_score: None,
            draw_tracing: false,
            draw_events: VecDeque::new(),
            draw_events_dropped: 0,
            draw_event_index_frame: None,
            draw_event_next_index: 0,
            layer_capture: false,
            ecs_watches: WatchStore::default(),
        }
    }
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    fn clear_audio_log(&mut self) {
        self.audio_log.clear();
        self.audio_events.clear();
        self.prev_channels = idle_channels();
        self.prev_pattern = None;
    }

    fn clear_draw_log(&mut self) {
        self.draw_events.clear();
        self.draw_events_dropped = 0;
        self.draw_event_index_frame = None;
        self.draw_event_next_index = 0;
    }

    fn clear_platform_log(&mut self) {
        self.platform_events.clear();
        self.platform_events_dropped = 0;
        self.platform_event_index_frame = None;
        self.platform_event_next_index = 0;
    }

    /// Load a new cart from source text and (re)build the console. Clears
    /// the input log, audio log, event log and any save states from a
    /// previous cart, since a save state's replay only makes sense against
    /// the cart it was recorded on.
    pub fn load_cart(&mut self, text: &str, seed: u64) -> Result<(), SessionError> {
        self.load_cart_with_save(text, seed, None)
    }

    /// Load a cart with an explicit persistence document injected before
    /// top-level Lua and `_init`. Agent sessions never consult ambient host
    /// storage; callers opt into this deterministic input explicitly.
    pub fn load_cart_with_save(
        &mut self,
        text: &str,
        seed: u64,
        initial_save: Option<&str>,
    ) -> Result<(), SessionError> {
        let mut console = Console::new_with_save(text, seed, initial_save)?;
        console.set_draw_tracing(self.draw_tracing);
        console.set_layer_capture(self.layer_capture);
        let init_draws = console.take_text_draws();
        let init_platform = console.take_platform_events();
        self.cart_text = Some(text.to_string());
        self.seed = seed;
        self.initial_save = initial_save.map(str::to_string);
        self.console = Some(console);
        self.input_log.clear();
        self.replay_log.clear();
        self.saved_states.clear();
        self.clear_audio_log();
        self.text_events.clear();
        self.clear_platform_log();
        self.max_submitted_score = None;
        self.clear_draw_log();
        self.ecs_watches.clear();
        record_text_draws(&mut self.text_events, 0, init_draws);
        record_platform_events(
            &mut self.platform_events,
            &mut self.platform_events_dropped,
            &mut self.platform_event_index_frame,
            &mut self.platform_event_next_index,
            &mut self.max_submitted_score,
            0,
            init_platform,
        );
        Ok(())
    }

    /// Recreate the console from the stored cart text, optionally with a
    /// new seed, and clear the input/audio/event logs. The save-state table
    /// survives (states are self-contained: cart text + their own seed +
    /// log).
    pub fn reset(&mut self, seed: Option<u64>) -> Result<(), SessionError> {
        let text = self.cart_text.clone().ok_or(SessionError::NoCart)?;
        let seed = seed.unwrap_or(self.seed);
        // Reset behaves like restarting a physical console: committed cart
        // persistence survives, while all volatile runtime state is rebuilt.
        let initial_save = self.console()?.save_document();
        let mut console = Console::new_with_save(&text, seed, initial_save.as_deref())?;
        console.set_draw_tracing(self.draw_tracing);
        console.set_layer_capture(self.layer_capture);
        let init_draws = console.take_text_draws();
        let init_platform = console.take_platform_events();
        self.seed = seed;
        self.initial_save = initial_save;
        self.console = Some(console);
        self.input_log.clear();
        self.replay_log.clear();
        self.clear_audio_log();
        self.text_events.clear();
        self.clear_platform_log();
        self.clear_draw_log();
        self.ecs_watches.reset_baselines();
        record_text_draws(&mut self.text_events, 0, init_draws);
        record_platform_events(
            &mut self.platform_events,
            &mut self.platform_events_dropped,
            &mut self.platform_event_index_frame,
            &mut self.platform_event_next_index,
            &mut self.max_submitted_score,
            0,
            init_platform,
        );
        Ok(())
    }

    fn console_mut(&mut self) -> Result<&mut Console, SessionError> {
        self.console.as_mut().ok_or(SessionError::NoCart)
    }

    pub fn console(&self) -> Result<&Console, SessionError> {
        self.console.as_ref().ok_or(SessionError::NoCart)
    }

    /// Canonical current host document, copied for a backend or assertion.
    pub fn save_document(&self) -> Result<Option<String>, SessionError> {
        Ok(self.console()?.save_document())
    }

    pub fn save_revision(&self) -> Result<u32, SessionError> {
        Ok(self.console()?.save_revision())
    }

    pub fn save_diagnostic(&self) -> Result<Option<String>, SessionError> {
        Ok(self.console()?.save_diagnostic())
    }

    /// Step `frames` times with the same input `mask` applied each frame.
    ///
    /// If the console is *already* halted before this call, that's an
    /// error (nothing to do) and the session stays alive untouched. If it
    /// halts *during* this call, that's reported in the returned
    /// [`StepOutcome`] instead, since the call did make progress.
    pub fn step(&mut self, frames: u64, mask: u8) -> Result<StepOutcome, SessionError> {
        // Access fields directly (rather than via a `&mut self` helper
        // method) so the borrow checker can see `self.console` and the
        // logs as disjoint and let the loop below mutate all of them.
        let console = self.console.as_mut().ok_or(SessionError::NoCart)?;
        if let Some(err) = console.halted() {
            return Err(SessionError::AlreadyHalted(err.message().to_string()));
        }

        let mask = mask & input::MASK;
        let mut halt_message = None;
        for _ in 0..frames {
            self.input_log.push(mask);
            self.replay_log.push(ReplayEvent::Step(mask));
            let result = console.step(mask);
            let event_frame = if result.is_ok() {
                console.frame_count()
            } else {
                console.frame_count().saturating_add(1)
            };
            record_text_draws(
                &mut self.text_events,
                event_frame,
                console.take_text_draws(),
            );
            record_draw_events(
                &mut self.draw_events,
                &mut self.draw_events_dropped,
                &mut self.draw_event_index_frame,
                &mut self.draw_event_next_index,
                event_frame,
                console.take_draw_events(),
            );
            record_platform_events(
                &mut self.platform_events,
                &mut self.platform_events_dropped,
                &mut self.platform_event_index_frame,
                &mut self.platform_event_next_index,
                &mut self.max_submitted_score,
                event_frame,
                console.take_platform_events(),
            );
            match result {
                Ok(()) => {
                    self.audio_log.extend_from_slice(console.audio_frame());
                    let frame = console.frame_count();
                    let channels = console.audio_channels();
                    let pattern = console.music_pattern();
                    audio::record_events(
                        &mut self.audio_events,
                        frame,
                        &self.prev_channels,
                        &channels,
                        self.prev_pattern,
                        pattern,
                        console.cart(),
                    );
                    self.prev_channels = channels;
                    self.prev_pattern = pattern;
                }
                Err(e) => {
                    halt_message = Some(e.message().to_string());
                    break;
                }
            }
        }

        Ok(StepOutcome {
            frame_count: console.frame_count(),
            halted: halt_message.is_some(),
            message: halt_message,
        })
    }

    pub fn screenshot_png(&self) -> Result<Vec<u8>, SessionError> {
        let console = self.console()?;
        // Screenshots are what the *player* sees, so the display palette
        // (`pal(c0, c1, 1)`) applies here. `screen_text` deliberately does not
        // apply it: that stays raw draw-space indices.
        Ok(encode_png(console.framebuffer(), console.display_palette()))
    }

    /// Like [`Session::screenshot_png`] but nearest-neighbor upscaled by an
    /// integer `zoom` factor (1 = unchanged, matches `screenshot_png`
    /// exactly). SPEC.md's 192x320 logical framebuffer is unreadably small
    /// at 1:1 for human/agent review, so callers can ask for it blown up.
    pub fn screenshot_png_zoomed(&self, zoom: u32) -> Result<Vec<u8>, SessionError> {
        let console = self.console()?;
        Ok(encode_png_zoomed(
            console.framebuffer(),
            console.display_palette(),
            zoom,
        ))
    }

    pub fn screen_text(&self) -> Result<Vec<String>, SessionError> {
        Ok(self
            .screen_text_report(ScreenTextRegion::full(), true)?
            .lines
            .expect("full screen_text requests include lines"))
    }

    /// Inspect a strict screen region in raw draw-space palette indices.
    ///
    /// Full-frame line output is intentionally exempt from the crop budget:
    /// callers request that historical, exact diagnostic explicitly. Any
    /// smaller region that includes lines is capped; summaries have constant
    /// output size and may cover any valid rectangle.
    pub fn screen_text_report(
        &self,
        region: ScreenTextRegion,
        include_lines: bool,
    ) -> Result<ScreenTextReport, SessionError> {
        let region = region.validate()?;
        if include_lines
            && !region.is_full()
            && region.pixel_count() > MAX_SCREEN_TEXT_REGION_PIXELS
        {
            return Err(SessionError::BadParams(format!(
                "screen_text region contains {} pixels; cropped line output is limited to {MAX_SCREEN_TEXT_REGION_PIXELS} pixels (use summary mode or a smaller region)",
                region.pixel_count()
            )));
        }

        let console = self.console()?;
        let framebuffer = console.framebuffer();
        let mut lines = include_lines.then(|| Vec::with_capacity(region.height as usize));
        let mut palette_counts = BTreeMap::new();
        let mut glyph_counts = BTreeMap::new();
        let mut min_x = SCREEN_W as u32;
        let mut min_y = SCREEN_H as u32;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        let mut found_non_background = false;

        for y in region.y..region.y + region.height {
            let mut line = include_lines.then(|| String::with_capacity(region.width as usize));
            for x in region.x..region.x + region.width {
                let px = framebuffer[y as usize * SCREEN_W + x as usize] & COLOR_MASK;
                let glyph = color_char(px);
                *palette_counts.entry(px).or_insert(0) += 1;
                *glyph_counts.entry(glyph).or_insert(0) += 1;
                if let Some(line) = &mut line {
                    line.push(glyph);
                }
                if px != 0 {
                    found_non_background = true;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
            if let (Some(lines), Some(line)) = (&mut lines, line) {
                lines.push(line);
            }
        }

        let non_background_bounds = if found_non_background {
            Some(ScreenTextRegion {
                x: min_x,
                y: min_y,
                width: max_x - min_x + 1,
                height: max_y - min_y + 1,
            })
        } else {
            None
        };
        let crop_right = SCREEN_W as u32 - (region.x + region.width);
        let crop_bottom = SCREEN_H as u32 - (region.y + region.height);
        let cropped_pixels = FB_LEN - region.pixel_count();
        let lines_omitted = !include_lines;
        let line_count_omitted = if lines_omitted { region.height } else { 0 };
        let line_characters_omitted = if lines_omitted {
            region.pixel_count()
        } else {
            0
        };

        Ok(ScreenTextReport {
            framebuffer_width: SCREEN_W as u32,
            framebuffer_height: SCREEN_H as u32,
            region,
            lines,
            palette_counts,
            glyph_counts,
            non_background_bounds,
            truncation: ScreenTextTruncation {
                truncated: cropped_pixels != 0 || lines_omitted,
                cropped_pixels,
                crop_left: region.x,
                crop_top: region.y,
                crop_right,
                crop_bottom,
                lines_omitted,
                line_count_omitted,
                line_characters_omitted,
            },
        })
    }

    pub fn eval(&mut self, code: &str) -> Result<console_core::mlua::Value, SessionError> {
        let console = self.console.as_mut().ok_or(SessionError::NoCart)?;
        let result = console.eval(code);
        let frame = console.frame_count();
        let draws = console.take_text_draws();
        record_text_draws(&mut self.text_events, frame, draws);
        record_draw_events(
            &mut self.draw_events,
            &mut self.draw_events_dropped,
            &mut self.draw_event_index_frame,
            &mut self.draw_event_next_index,
            frame,
            console.take_draw_events(),
        );
        record_platform_events(
            &mut self.platform_events,
            &mut self.platform_events_dropped,
            &mut self.platform_event_index_frame,
            &mut self.platform_event_next_index,
            &mut self.max_submitted_score,
            frame,
            console.take_platform_events(),
        );
        Ok(result?)
    }

    pub fn get_global(&self, name: &str) -> Result<console_core::mlua::Value, SessionError> {
        let console = self.console()?;
        Ok(console.get_global(name)?)
    }

    /// Inspect one named console ECS world without exposing mutable storage.
    pub fn ecs_query(
        &self,
        world: &str,
        required: &[String],
        select: &BTreeMap<String, Vec<String>>,
        limit: usize,
        after: u64,
    ) -> Result<console_core::mlua::Value, SessionError> {
        Ok(self
            .console()?
            .ecs_query(world, required, select, limit, after)?)
    }

    pub fn dev_hooks(&self) -> Result<Vec<DevHookInfo>, SessionError> {
        Ok(self.console()?.dev_hooks()?)
    }

    pub fn invoke_dev_hook(
        &mut self,
        name: &str,
        expected_phase: DevHookPhase,
        args: DevValue,
    ) -> Result<DevHookInvocation, SessionError> {
        let console = self.console.as_mut().ok_or(SessionError::NoCart)?;
        if let Some(error) = console.halted() {
            return Err(SessionError::AlreadyHalted(error.message().to_string()));
        }
        let info = console
            .dev_hooks()?
            .into_iter()
            .find(|hook| hook.name == name)
            .ok_or_else(|| SessionError::BadParams(format!("unknown development hook {name:?}")))?;
        if info.phase != expected_phase {
            return Err(SessionError::BadParams(format!(
                "development hook {name:?} has phase {}, not {}",
                info.phase.as_str(),
                expected_phase.as_str()
            )));
        }
        if expected_phase == DevHookPhase::PreFrame && console.frame_count() != 0 {
            return Err(SessionError::BadParams(format!(
                "pre_frame development hook {name:?} cannot run after frame 0"
            )));
        }
        self.replay_log.push(ReplayEvent::DevHook {
            name: name.to_string(),
            phase: expected_phase,
            args: args.clone(),
        });
        let result = console.invoke_dev_hook(name, expected_phase, &args);
        let frame_count = console.frame_count();
        record_text_draws(
            &mut self.text_events,
            frame_count,
            console.take_text_draws(),
        );
        record_draw_events(
            &mut self.draw_events,
            &mut self.draw_events_dropped,
            &mut self.draw_event_index_frame,
            &mut self.draw_event_next_index,
            frame_count,
            console.take_draw_events(),
        );
        record_platform_events(
            &mut self.platform_events,
            &mut self.platform_events_dropped,
            &mut self.platform_event_index_frame,
            &mut self.platform_event_next_index,
            &mut self.max_submitted_score,
            frame_count,
            console.take_platform_events(),
        );
        Ok(DevHookInvocation {
            name: name.to_string(),
            phase: expected_phase,
            frame_count,
            result: result?,
        })
    }

    /// Save a bounded ECS query under a host-side name. Querying once here
    /// validates that the world exists, but does not establish a baseline;
    /// only explicit samples participate in deltas.
    pub fn define_ecs_watch(
        &mut self,
        definition: WatchDefinition,
    ) -> Result<WatchMetadata, SessionError> {
        let query = &definition.query;
        self.ecs_query(&query.world, &query.required, &query.select, query.limit, 0)?;
        self.ecs_watches
            .define(definition)
            .map_err(SessionError::BadParams)
    }

    pub fn sample_ecs_watch(&mut self, name: &str) -> Result<WatchSample, SessionError> {
        let definition = self
            .ecs_watches
            .definition(name)
            .map_err(SessionError::BadParams)?;
        let QueryDefinition {
            world,
            required,
            select,
            limit,
        } = definition.query;
        let frame_count = self.console()?.frame_count();
        let snapshot = self.ecs_query(&world, &required, &select, limit, 0)?;
        self.ecs_watches
            .record(name, frame_count, lua_to_json(&snapshot))
            .map_err(SessionError::BadParams)
    }

    pub fn ecs_watch_list(&self) -> Vec<WatchMetadata> {
        self.ecs_watches.list()
    }

    pub fn has_ecs_watch(&self, name: &str) -> bool {
        self.ecs_watches.definition(name).is_ok()
    }

    pub fn remove_ecs_watch(&mut self, name: &str) -> bool {
        self.ecs_watches.remove(name)
    }

    pub fn logs(&mut self) -> Result<Vec<String>, SessionError> {
        let console = self.console_mut()?;
        Ok(console.take_logs())
    }

    pub fn save_state(&mut self, name: &str) -> Result<(), SessionError> {
        // Ensure there is a console at all (so `save_state` on an unloaded
        // session errors the same way everything else does).
        self.console()?;
        self.saved_states.insert(
            name.to_string(),
            SavedState {
                seed: self.seed,
                initial_save: self.initial_save.clone(),
                input_log: self.input_log.clone(),
                replay_log: self.replay_log.clone(),
            },
        );
        Ok(())
    }

    /// Recreate the console from the cart text and saved seed, then replay
    /// hook calls and input frames in their original order. Audio and host
    /// event logs are rebuilt identically to a continuous run. Returns the
    /// number of stepped frames replayed.
    pub fn load_state(&mut self, name: &str) -> Result<StepOutcome, SessionError> {
        let text = self.cart_text.clone().ok_or(SessionError::NoCart)?;
        let saved = self
            .saved_states
            .get(name)
            .cloned()
            .ok_or_else(|| SessionError::BadParams(format!("no saved state named {name:?}")))?;

        let mut console = Console::new_with_save(&text, saved.seed, saved.initial_save.as_deref())?;
        console.set_draw_tracing(self.draw_tracing);
        console.set_layer_capture(self.layer_capture);
        let mut audio_log = Vec::new();
        let mut audio_events = Vec::new();
        let mut text_events = Vec::new();
        let mut platform_events = VecDeque::new();
        let mut platform_events_dropped = 0;
        let mut platform_event_index_frame = None;
        let mut platform_event_next_index = 0;
        let mut max_submitted_score = self.max_submitted_score;
        let mut draw_events = VecDeque::new();
        let mut draw_events_dropped = 0;
        let mut draw_event_index_frame = None;
        let mut draw_event_next_index = 0;
        record_text_draws(&mut text_events, 0, console.take_text_draws());
        record_platform_events(
            &mut platform_events,
            &mut platform_events_dropped,
            &mut platform_event_index_frame,
            &mut platform_event_next_index,
            &mut max_submitted_score,
            0,
            console.take_platform_events(),
        );
        let mut prev_channels = idle_channels();
        let mut prev_pattern = None;
        let mut halt_message = None;
        let mut replayed = 0u64;
        for event in &saved.replay_log {
            if let ReplayEvent::DevHook { name, phase, args } = event {
                if let Err(error) = console.invoke_dev_hook(name, *phase, args) {
                    halt_message = Some(error.message().to_string());
                    break;
                }
                let event_frame = console.frame_count();
                record_text_draws(&mut text_events, event_frame, console.take_text_draws());
                record_draw_events(
                    &mut draw_events,
                    &mut draw_events_dropped,
                    &mut draw_event_index_frame,
                    &mut draw_event_next_index,
                    event_frame,
                    console.take_draw_events(),
                );
                record_platform_events(
                    &mut platform_events,
                    &mut platform_events_dropped,
                    &mut platform_event_index_frame,
                    &mut platform_event_next_index,
                    &mut max_submitted_score,
                    event_frame,
                    console.take_platform_events(),
                );
                continue;
            }
            let ReplayEvent::Step(mask) = event else {
                unreachable!()
            };
            let result = console.step(*mask);
            let event_frame = if result.is_ok() {
                console.frame_count()
            } else {
                console.frame_count().saturating_add(1)
            };
            record_text_draws(&mut text_events, event_frame, console.take_text_draws());
            record_draw_events(
                &mut draw_events,
                &mut draw_events_dropped,
                &mut draw_event_index_frame,
                &mut draw_event_next_index,
                event_frame,
                console.take_draw_events(),
            );
            record_platform_events(
                &mut platform_events,
                &mut platform_events_dropped,
                &mut platform_event_index_frame,
                &mut platform_event_next_index,
                &mut max_submitted_score,
                event_frame,
                console.take_platform_events(),
            );
            match result {
                Ok(()) => {
                    replayed += 1;
                    audio_log.extend_from_slice(console.audio_frame());
                    let frame = console.frame_count();
                    let channels = console.audio_channels();
                    let pattern = console.music_pattern();
                    audio::record_events(
                        &mut audio_events,
                        frame,
                        &prev_channels,
                        &channels,
                        prev_pattern,
                        pattern,
                        console.cart(),
                    );
                    prev_channels = channels;
                    prev_pattern = pattern;
                }
                Err(e) => {
                    halt_message = Some(e.message().to_string());
                    break;
                }
            }
        }

        self.seed = saved.seed;
        self.initial_save = saved.initial_save;
        self.input_log = saved.input_log;
        self.replay_log = saved.replay_log;
        self.console = Some(console);
        self.audio_log = audio_log;
        self.audio_events = audio_events;
        self.prev_channels = prev_channels;
        self.prev_pattern = prev_pattern;
        self.text_events = text_events;
        self.platform_events = platform_events;
        self.platform_events_dropped = platform_events_dropped;
        self.platform_event_index_frame = platform_event_index_frame;
        self.platform_event_next_index = platform_event_next_index;
        self.max_submitted_score = max_submitted_score;
        self.draw_events = draw_events;
        self.draw_events_dropped = draw_events_dropped;
        self.draw_event_index_frame = draw_event_index_frame;
        self.draw_event_next_index = draw_event_next_index;
        self.ecs_watches.reset_baselines();

        Ok(StepOutcome {
            frame_count: replayed,
            halted: halt_message.is_some(),
            message: halt_message,
        })
    }

    pub fn info(&self) -> Result<Info, SessionError> {
        let console = self.console()?;
        Ok(Info {
            frame_count: console.frame_count(),
            seed: console.seed(),
            halted: console.halted().map(|e| e.message().to_string()),
            title: console.cart().title().to_string(),
            meta: console.cart().meta().clone(),
            input_log_len: self.input_log.len(),
            saved_states: self.saved_states.keys().cloned().collect(),
            max_submitted_score: self.max_submitted_score,
        })
    }

    pub fn has_cart(&self) -> bool {
        self.console.is_some()
    }

    pub fn input_log(&self) -> &[u8] {
        &self.input_log
    }

    /// Every sample rendered since the last load/reset/load_state, in
    /// order.
    pub fn audio_log(&self) -> &[f32] {
        &self.audio_log
    }

    /// Number of frames represented in the audio log.
    pub fn audio_frame_count(&self) -> u64 {
        (self.audio_log.len() / SAMPLES_PER_FRAME) as u64
    }

    /// Resolve `{from_frame, to_frame}` against the audio log, clamping to
    /// valid bounds. `to_frame` is exclusive; both default to the full log.
    fn audio_range(&self, from_frame: Option<u64>, to_frame: Option<u64>) -> (u64, u64) {
        let total = self.audio_frame_count();
        let from = from_frame.unwrap_or(0).min(total);
        let to = to_frame.unwrap_or(total).clamp(from, total);
        (from, to)
    }

    /// The audio log's samples for `[from_frame, to_frame)`, plus the
    /// clamped `(from, to)` frame indices actually used.
    pub fn audio_slice(
        &self,
        from_frame: Option<u64>,
        to_frame: Option<u64>,
    ) -> Result<(&[f32], u64, u64), SessionError> {
        self.console()?;
        let (from, to) = self.audio_range(from_frame, to_frame);
        let start = from as usize * SAMPLES_PER_FRAME;
        let end = to as usize * SAMPLES_PER_FRAME;
        Ok((&self.audio_log[start..end], from, to))
    }

    /// Encode `[from_frame, to_frame)` of the audio log as a WAV file.
    /// Returns `(bytes, frames_covered, samples_covered)`.
    pub fn wav_bytes(
        &self,
        from_frame: Option<u64>,
        to_frame: Option<u64>,
    ) -> Result<(Vec<u8>, u64, usize), SessionError> {
        let (samples, from, to) = self.audio_slice(from_frame, to_frame)?;
        Ok((audio::encode_wav(samples), to - from, samples.len()))
    }

    /// Current per-channel state (with resolved note names) + music pattern
    /// + frame count.
    pub fn audio_state(&self) -> Result<AudioState, SessionError> {
        Ok(audio::audio_state(self.console()?))
    }

    /// The event log, optionally filtered to events at or after
    /// `from_frame`.
    pub fn audio_events(&self, from_frame: Option<u64>) -> Result<Vec<AudioEvent>, SessionError> {
        self.console()?;
        Ok(self
            .audio_events
            .iter()
            .filter(|e| from_frame.is_none_or(|f| e.frame >= f))
            .cloned()
            .collect())
    }

    /// Text draws, optionally filtered to events at or after `from_frame`.
    pub fn text_events(&self, from_frame: Option<u64>) -> Result<Vec<TextEvent>, SessionError> {
        self.console()?;
        Ok(self
            .text_events
            .iter()
            .filter(|event| from_frame.is_none_or(|frame| event.frame >= frame))
            .cloned()
            .collect())
    }

    /// Ordered score/leaderboard requests, optionally filtered by completed
    /// frame. The host-only submitted maximum is never readable from Lua.
    pub fn platform_events(
        &self,
        from_frame: Option<u64>,
    ) -> Result<PlatformEventReport, SessionError> {
        self.console()?;
        Ok(PlatformEventReport {
            capacity: MAX_SESSION_PLATFORM_EVENTS,
            dropped: self.platform_events_dropped,
            max_submitted_score: self.max_submitted_score,
            events: self
                .platform_events
                .iter()
                .filter(|event| from_frame.is_none_or(|frame| event.frame >= frame))
                .cloned()
                .collect(),
        })
    }

    /// Enable or disable trace collection for subsequent draw calls. Changing
    /// the mode starts a fresh bounded log so reports never mix policies.
    pub fn set_draw_tracing(&mut self, enabled: bool) {
        if self.draw_tracing == enabled {
            return;
        }
        self.draw_tracing = enabled;
        self.clear_draw_log();
        if let Some(console) = &mut self.console {
            console.set_draw_tracing(enabled);
        }
    }

    pub fn draw_tracing(&self) -> bool {
        self.draw_tracing
    }

    /// Enable isolated `draw_tag()` framebuffers for subsequent drawing.
    /// Changing the mode clears any previously captured layer frame.
    pub fn set_layer_capture(&mut self, enabled: bool) {
        if self.layer_capture == enabled {
            return;
        }
        self.layer_capture = enabled;
        if let Some(console) = &mut self.console {
            console.set_layer_capture(enabled);
        }
    }

    pub fn layer_capture_enabled(&self) -> bool {
        self.layer_capture
    }

    /// Encode every non-empty current-frame layer with untouched pixels as
    /// alpha zero. Real colour 0 remains opaque.
    pub fn layer_screenshots_png_zoomed(
        &self,
        zoom: u32,
    ) -> Result<LayerScreenshotSet, SessionError> {
        let console = self.console()?;
        let palette = console.display_palette();
        let frame = console.layer_capture_frame();
        Ok(LayerScreenshotSet {
            capacity: frame.capacity,
            dropped: frame.dropped,
            layers: frame
                .layers
                .into_iter()
                .map(|layer| LayerScreenshot {
                    tag: layer.tag,
                    png: encode_layer_png_zoomed(&layer.framebuffer, palette, zoom),
                })
                .collect(),
        })
    }

    pub fn clear_draw_events(&mut self) {
        self.clear_draw_log();
        if let Some(console) = &mut self.console {
            let _ = console.take_draw_events();
        }
    }

    /// Snapshot bounded draw events, optionally filtering by completed frame
    /// and semantic `draw_tag`. Filtering does not mutate the retained log.
    pub fn draw_events(
        &self,
        from_frame: Option<u64>,
        tag: Option<&str>,
    ) -> Result<DrawTraceReport, SessionError> {
        self.console()?;
        Ok(DrawTraceReport {
            enabled: self.draw_tracing,
            capacity: MAX_SESSION_DRAW_EVENTS,
            dropped: self.draw_events_dropped,
            events: self
                .draw_events
                .iter()
                .filter(|event| from_frame.is_none_or(|frame| event.frame >= frame))
                .filter(|event| tag.is_none_or(|tag| event.event.tag.as_deref() == Some(tag)))
                .cloned()
                .collect(),
        })
    }

    /// Per-window RMS/peak/clip-count stats over the whole audio log.
    pub fn audio_stats(&self, window_frames: u64) -> Result<Vec<StatsWindow>, SessionError> {
        self.console()?;
        Ok(audio::compute_stats(&self.audio_log, window_frames))
    }

    /// Render a semitone-grid spectrogram PNG of `[from_frame, to_frame)`.
    pub fn spectrogram_png(
        &self,
        from_frame: Option<u64>,
        to_frame: Option<u64>,
        cell: u32,
    ) -> Result<Spectrogram, SessionError> {
        let (samples, _from, _to) = self.audio_slice(from_frame, to_frame)?;
        Ok(audio::render_spectrogram(samples, cell))
    }
}

pub struct Info {
    pub frame_count: u64,
    pub seed: u64,
    pub halted: Option<String>,
    pub title: String,
    pub meta: BTreeMap<String, String>,
    pub input_log_len: usize,
    pub saved_states: Vec<String>,
    pub max_submitted_score: Option<u64>,
}

/// Encode a framebuffer as an RGBA PNG at 1:1 scale using the fixed palette.
///
/// `dpal` is the console's display palette (`Console::display_palette`): a
/// 64-entry index -> index map applied at scanout, identity unless the cart
/// called `pal(c0, c1, 1)`. Pass [`console_core::IDENTITY_PAL`] for raw output.
pub fn encode_png(fb: &[u8; FB_LEN], dpal: &[u8; COLOR_COUNT]) -> Vec<u8> {
    encode_png_zoomed(fb, dpal, 1)
}

/// Encode a framebuffer as an RGBA PNG, nearest-neighbor upscaled by an
/// integer `zoom` factor (each logical pixel becomes a `zoom`x`zoom` block).
/// `zoom <= 1` behaves exactly like [`encode_png`].
pub fn encode_png_zoomed(fb: &[u8; FB_LEN], dpal: &[u8; COLOR_COUNT], zoom: u32) -> Vec<u8> {
    let zoom = zoom.max(1);
    let rgba = framebuffer_rgba(fb, dpal);

    let (rgba, width, height) = if zoom == 1 {
        (rgba, SCREEN_W as u32, SCREEN_H as u32)
    } else {
        let scaled = nearest_neighbor_scale(&rgba, SCREEN_W as u32, SCREEN_H as u32, zoom);
        (scaled, SCREEN_W as u32 * zoom, SCREEN_H as u32 * zoom)
    };

    crate::palette::encode_png_rgba(&rgba, width, height)
}

fn encode_layer_png_zoomed(fb: &[u8; FB_LEN], dpal: &[u8; COLOR_COUNT], zoom: u32) -> Vec<u8> {
    let zoom = zoom.max(1);
    let rgba = layer_framebuffer_rgba(fb, dpal);
    let (rgba, width, height) = if zoom == 1 {
        (rgba, SCREEN_W as u32, SCREEN_H as u32)
    } else {
        (
            nearest_neighbor_scale(&rgba, SCREEN_W as u32, SCREEN_H as u32, zoom),
            SCREEN_W as u32 * zoom,
            SCREEN_H as u32 * zoom,
        )
    };
    crate::palette::encode_png_rgba(&rgba, width, height)
}

pub(crate) fn layer_framebuffer_rgba(fb: &[u8; FB_LEN], dpal: &[u8; COLOR_COUNT]) -> Vec<u8> {
    let mut rgba = framebuffer_rgba(fb, dpal);
    for (pixel, &index) in rgba.chunks_exact_mut(4).zip(fb) {
        if index == LAYER_TRANSPARENT {
            pixel.copy_from_slice(&[0, 0, 0, 0]);
        }
    }
    rgba
}

/// Convert raw framebuffer indices to opaque RGBA using the current display
/// palette. Kept separate from PNG encoding so deterministic sequence capture
/// can crop native pixels before any nearest-neighbor enlargement.
pub(crate) fn framebuffer_rgba(fb: &[u8; FB_LEN], dpal: &[u8; COLOR_COUNT]) -> Vec<u8> {
    // Fold the display map into an RGB lookup once, not per pixel.
    let lut: [[u8; 3]; COLOR_COUNT] =
        std::array::from_fn(|i| PALETTE[(dpal[i] & COLOR_MASK) as usize]);
    let mut rgba = Vec::with_capacity(FB_LEN * 4);
    for &idx in fb.iter() {
        let [r, g, b] = lut[(idx & COLOR_MASK) as usize];
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    rgba
}

/// Nearest-neighbor integer upscale of an RGBA buffer: each source pixel
/// becomes a `zoom`x`zoom` block of identical pixels. `sprite/view.rs` has
/// its own zoomed-canvas renderer, but it's built around sprite-specific
/// concerns (checkerboard transparency, palette-index glyphs, RGB triples)
/// that don't apply to a screen framebuffer, so this is a small
/// general-purpose helper instead of a shared one.
fn nearest_neighbor_scale(rgba: &[u8], src_w: u32, src_h: u32, zoom: u32) -> Vec<u8> {
    let dst_w = src_w * zoom;
    let dst_h = src_h * zoom;
    let mut out = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    for sy in 0..src_h {
        for sx in 0..src_w {
            let si = ((sy * src_w + sx) * 4) as usize;
            let px = &rgba[si..si + 4];
            for dy in 0..zoom {
                let oy = sy * zoom + dy;
                for dx in 0..zoom {
                    let ox = sx * zoom + dx;
                    let di = ((oy * dst_w + ox) * 4) as usize;
                    out[di..di + 4].copy_from_slice(px);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cart that draws, then fades the whole screen through the display
    /// palette without redrawing a pixel.
    const FADE_CART: &str = "\
__lua__
function _draw() cls(1) rectfill(0, 0, 9, 9, 7) end
function _update() if t() * 60 >= 2 then for i = 0, 63 do pal(i, 0, 1) end end end
";

    #[test]
    fn screenshots_apply_the_display_palette_but_screen_text_does_not() {
        let mut session = Session::default();
        session.load_cart(FADE_CART, 0).unwrap();
        session.step(1, 0).unwrap();

        let before_png = session.screenshot_png().unwrap();
        let before_text = session.screen_text().unwrap();

        session.step(5, 0).unwrap();
        let after_png = session.screenshot_png().unwrap();
        let after_text = session.screen_text().unwrap();

        assert_eq!(
            before_text, after_text,
            "screen_text stays in raw draw space"
        );
        assert_ne!(
            before_png, after_png,
            "the screenshot must show the display-palette fade"
        );
    }

    #[test]
    fn screen_text_uses_the_full_palette_alphabet() {
        let mut session = Session::default();
        session
            .load_cart(
                "__lua__\nfunction _draw() cls(0) pset(0,0,36) pset(1,0,63) end\n",
                0,
            )
            .unwrap();
        session.step(1, 0).unwrap();
        let text = session.screen_text().unwrap();
        assert!(text[0].starts_with("A_"));
    }

    #[test]
    fn screen_text_report_has_absolute_bounds_counts_and_omission_metadata() {
        let mut session = Session::default();
        session
            .load_cart(
                "__lua__\nfunction _draw() cls(0) rectfill(2,3,4,4,5) pset(10,10,63) end\n",
                0,
            )
            .unwrap();
        session.step(1, 0).unwrap();

        let report = session
            .screen_text_report(
                ScreenTextRegion {
                    x: 1,
                    y: 2,
                    width: 5,
                    height: 4,
                },
                true,
            )
            .unwrap();
        assert_eq!(
            report.lines.unwrap(),
            vec!["00000", "05550", "05550", "00000"]
        );
        assert_eq!(report.palette_counts.get(&0), Some(&14));
        assert_eq!(report.palette_counts.get(&5), Some(&6));
        assert_eq!(report.glyph_counts.get(&'5'), Some(&6));
        assert_eq!(
            report.non_background_bounds,
            Some(ScreenTextRegion {
                x: 2,
                y: 3,
                width: 3,
                height: 2,
            })
        );
        assert!(report.truncation.truncated);
        assert_eq!(report.truncation.crop_left, 1);
        assert_eq!(report.truncation.crop_top, 2);
        assert_eq!(report.truncation.crop_right, 186);
        assert_eq!(report.truncation.crop_bottom, 314);
        assert!(!report.truncation.lines_omitted);

        let summary = session
            .screen_text_report(ScreenTextRegion::full(), false)
            .unwrap();
        assert!(summary.lines.is_none());
        assert_eq!(summary.palette_counts.get(&5), Some(&6));
        assert_eq!(summary.palette_counts.get(&63), Some(&1));
        assert_eq!(summary.glyph_counts.get(&'_'), Some(&1));
        assert_eq!(
            summary.non_background_bounds,
            Some(ScreenTextRegion {
                x: 2,
                y: 3,
                width: 9,
                height: 8,
            })
        );
        assert_eq!(summary.truncation.cropped_pixels, 0);
        assert!(summary.truncation.lines_omitted);
        assert_eq!(summary.truncation.line_count_omitted, SCREEN_H as u32);
        assert_eq!(summary.truncation.line_characters_omitted, FB_LEN);
    }

    #[test]
    fn cropped_screen_text_line_output_has_a_hard_budget() {
        let mut session = Session::default();
        session.load_cart("__lua__\n", 0).unwrap();
        let error = session
            .screen_text_report(
                ScreenTextRegion {
                    x: 0,
                    y: 0,
                    width: SCREEN_W as u32,
                    height: 100,
                },
                true,
            )
            .unwrap_err();
        assert!(error.to_string().contains("limited to"));
        assert!(
            session
                .screen_text_report(
                    ScreenTextRegion {
                        x: 0,
                        y: 0,
                        width: SCREEN_W as u32,
                        height: 100,
                    },
                    false,
                )
                .is_ok()
        );
        assert_eq!(session.screen_text().unwrap().len(), SCREEN_H);
    }

    #[test]
    fn summary_size_stays_bounded_with_every_palette_glyph_present() {
        let mut session = Session::default();
        session
            .load_cart(
                "__lua__\nfunction _draw() cls(0) for i=0,63 do pset(i,0,i) end end\n",
                0,
            )
            .unwrap();
        session.step(1, 0).unwrap();
        let report = session
            .screen_text_report(ScreenTextRegion::full(), false)
            .unwrap();
        assert_eq!(report.palette_counts.len(), 64);
        assert_eq!(report.glyph_counts.len(), 64);
        let encoded = serde_json::to_vec(&report).unwrap();
        assert!(
            encoded.len() < 4_096,
            "all-color summary must remain agent-safe, got {} bytes",
            encoded.len()
        );
    }

    #[test]
    fn encode_png_with_the_identity_map_is_the_plain_render() {
        let mut fb = [0u8; FB_LEN];
        fb[0] = 7;
        fb[1] = 11;
        let mut faded = console_core::IDENTITY_PAL;
        faded[7] = 0;
        assert_eq!(
            encode_png(&fb, &console_core::IDENTITY_PAL),
            encode_png(&fb, &console_core::IDENTITY_PAL)
        );
        assert_ne!(
            encode_png(&fb, &console_core::IDENTITY_PAL),
            encode_png(&fb, &faded)
        );
    }

    #[test]
    fn zoomed_screenshots_apply_the_display_palette_too() {
        let mut fb = [0u8; FB_LEN];
        fb[0] = 7;
        let mut faded = console_core::IDENTITY_PAL;
        faded[7] = 0;
        assert_ne!(
            encode_png_zoomed(&fb, &console_core::IDENTITY_PAL, 3),
            encode_png_zoomed(&fb, &faded, 3)
        );
    }

    #[test]
    fn layer_capture_survives_reset_and_replay_load_state() {
        let cart = "__lua__\n\
            function _draw()\n\
              draw_tag('actor') pset(t()*60, 1, 7)\n\
            end\n";
        let mut session = Session::new();
        session.set_layer_capture(true);
        session.load_cart(cart, 11).unwrap();
        session.step(2, 0).unwrap();
        session.save_state("two").unwrap();
        let expected = session
            .layer_screenshots_png_zoomed(1)
            .unwrap()
            .layers
            .remove(0)
            .png;

        session.step(1, 0).unwrap();
        session.load_state("two").unwrap();
        let replayed = session
            .layer_screenshots_png_zoomed(1)
            .unwrap()
            .layers
            .remove(0)
            .png;
        assert_eq!(replayed, expected);

        session.reset(None).unwrap();
        assert!(session.layer_capture_enabled());
        session.step(1, 0).unwrap();
        assert_eq!(
            session.layer_screenshots_png_zoomed(1).unwrap().layers[0]
                .tag
                .as_deref(),
            Some("actor")
        );
    }
}

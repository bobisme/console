//! Session state shared by the oneshot CLI and the JSON-RPC `serve` loop.
//!
//! A [`Session`] owns at most one running [`Console`], the cart text and
//! seed it was built from, the full per-frame input log since the last
//! reset/load (needed for save/load state and for replay-based resets), a
//! parallel per-frame audio sample log and sequencer event log (see
//! [`crate::audio`]), and a table of named save states.

use std::collections::BTreeMap;

use console_core::{
    CHANNEL_COUNT, ChannelInfo, Console, Error, FB_LEN, PALETTE, SAMPLES_PER_FRAME, SCREEN_H,
    SCREEN_W, input,
};

use crate::audio::{self, AudioEvent, AudioState, Spectrogram, StatsWindow};

/// A named save state: enough to recreate the exact console state via
/// replay (`Console::new(cart_text, seed)` + step every mask in order).
#[derive(Clone)]
pub struct SavedState {
    pub seed: u64,
    pub input_log: Vec<u8>,
}

/// The result of a `step` (or the replay inside `load_state`).
pub struct StepOutcome {
    pub frame_count: u64,
    pub halted: bool,
    pub message: Option<String>,
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
    input_log: Vec<u8>,
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
}

impl Default for Session {
    fn default() -> Session {
        Session {
            cart_text: None,
            console: None,
            seed: 0,
            input_log: Vec::new(),
            saved_states: BTreeMap::new(),
            audio_log: Vec::new(),
            audio_events: Vec::new(),
            prev_channels: idle_channels(),
            prev_pattern: None,
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

    /// Load a new cart from source text and (re)build the console. Clears
    /// the input log, audio log, event log and any save states from a
    /// previous cart, since a save state's replay only makes sense against
    /// the cart it was recorded on.
    pub fn load_cart(&mut self, text: &str, seed: u64) -> Result<(), SessionError> {
        let console = Console::new(text, seed)?;
        self.cart_text = Some(text.to_string());
        self.seed = seed;
        self.console = Some(console);
        self.input_log.clear();
        self.saved_states.clear();
        self.clear_audio_log();
        Ok(())
    }

    /// Recreate the console from the stored cart text, optionally with a
    /// new seed, and clear the input/audio/event logs. The save-state table
    /// survives (states are self-contained: cart text + their own seed +
    /// log).
    pub fn reset(&mut self, seed: Option<u64>) -> Result<(), SessionError> {
        let text = self.cart_text.clone().ok_or(SessionError::NoCart)?;
        let seed = seed.unwrap_or(self.seed);
        let console = Console::new(&text, seed)?;
        self.seed = seed;
        self.console = Some(console);
        self.input_log.clear();
        self.clear_audio_log();
        Ok(())
    }

    fn console_mut(&mut self) -> Result<&mut Console, SessionError> {
        self.console.as_mut().ok_or(SessionError::NoCart)
    }

    pub fn console(&self) -> Result<&Console, SessionError> {
        self.console.as_ref().ok_or(SessionError::NoCart)
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
            match console.step(mask) {
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
        Ok(encode_png(console.framebuffer()))
    }

    /// Like [`Session::screenshot_png`] but nearest-neighbor upscaled by an
    /// integer `zoom` factor (1 = unchanged, matches `screenshot_png`
    /// exactly). SPEC.md's 144x256 logical framebuffer is unreadably small
    /// at 1:1 for human/agent review, so callers can ask for it blown up.
    pub fn screenshot_png_zoomed(&self, zoom: u32) -> Result<Vec<u8>, SessionError> {
        let console = self.console()?;
        Ok(encode_png_zoomed(console.framebuffer(), zoom))
    }

    pub fn screen_text(&self) -> Result<Vec<String>, SessionError> {
        let console = self.console()?;
        let fb = console.framebuffer();
        let mut lines = Vec::with_capacity(SCREEN_H);
        for row in fb.chunks_exact(SCREEN_W) {
            let mut line = String::with_capacity(SCREEN_W);
            for &px in row {
                line.push(std::char::from_digit((px & 0x0f) as u32, 16).unwrap());
            }
            lines.push(line);
        }
        Ok(lines)
    }

    pub fn eval(&mut self, code: &str) -> Result<console_core::mlua::Value, SessionError> {
        let console = self.console_mut()?;
        Ok(console.eval(code)?)
    }

    pub fn get_global(&self, name: &str) -> Result<console_core::mlua::Value, SessionError> {
        let console = self.console()?;
        Ok(console.get_global(name)?)
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
                input_log: self.input_log.clone(),
            },
        );
        Ok(())
    }

    /// Recreate the console from the cart text and the saved state's seed,
    /// then replay its input log frame-by-frame, rebuilding the audio log
    /// and event log identically to a continuous run (the synth is
    /// deterministic, so replay reproduces both byte-for-byte). Returns the
    /// number of frames replayed.
    pub fn load_state(&mut self, name: &str) -> Result<StepOutcome, SessionError> {
        let text = self.cart_text.clone().ok_or(SessionError::NoCart)?;
        let saved = self
            .saved_states
            .get(name)
            .cloned()
            .ok_or_else(|| SessionError::BadParams(format!("no saved state named {name:?}")))?;

        let mut console = Console::new(&text, saved.seed)?;
        let mut audio_log = Vec::new();
        let mut audio_events = Vec::new();
        let mut prev_channels = idle_channels();
        let mut prev_pattern = None;
        let mut halt_message = None;
        let mut replayed = 0u64;
        for &mask in &saved.input_log {
            match console.step(mask) {
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
        self.input_log = saved.input_log;
        self.console = Some(console);
        self.audio_log = audio_log;
        self.audio_events = audio_events;
        self.prev_channels = prev_channels;
        self.prev_pattern = prev_pattern;

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
}

/// Encode a framebuffer as an RGBA PNG at 1:1 scale using the fixed
/// Sweetie-16 palette.
pub fn encode_png(fb: &[u8; FB_LEN]) -> Vec<u8> {
    encode_png_zoomed(fb, 1)
}

/// Encode a framebuffer as an RGBA PNG, nearest-neighbor upscaled by an
/// integer `zoom` factor (each logical pixel becomes a `zoom`x`zoom` block).
/// `zoom <= 1` behaves exactly like [`encode_png`].
pub fn encode_png_zoomed(fb: &[u8; FB_LEN], zoom: u32) -> Vec<u8> {
    let zoom = zoom.max(1);
    let mut rgba = Vec::with_capacity(FB_LEN * 4);
    for &idx in fb.iter() {
        let [r, g, b] = PALETTE[(idx & 0x0f) as usize];
        rgba.extend_from_slice(&[r, g, b, 255]);
    }

    let (rgba, width, height) = if zoom == 1 {
        (rgba, SCREEN_W as u32, SCREEN_H as u32)
    } else {
        let scaled = nearest_neighbor_scale(&rgba, SCREEN_W as u32, SCREEN_H as u32, zoom);
        (scaled, SCREEN_W as u32 * zoom, SCREEN_H as u32 * zoom)
    };

    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .expect("PNG header write cannot fail on an in-memory buffer");
        writer
            .write_image_data(&rgba)
            .expect("PNG data write cannot fail on an in-memory buffer");
    }
    bytes
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

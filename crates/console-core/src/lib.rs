//! Deterministic fantasy-console core.
//!
//! A [`Console`] owns a sandboxed Lua 5.4 VM, a 192x320 palette-indexed
//! software framebuffer and a seeded PRNG. It has no windowing, audio,
//! filesystem, threading or wall-clock dependency, so the same crate builds
//! for native hosts and for `wasm32-unknown-emscripten`, and the same
//! `(cart, seed, input log)` always produces the same pixels.
//!
//! ```
//! use console_core::{Console, input};
//!
//! let cart = "__lua__\nfunction _draw() cls(3) print('hi', 4, 4) end\n";
//! let mut con = Console::new(cart, 0)?;
//! con.step(input::RIGHT | input::A)?;
//!
//! let pixels: &[u8] = con.framebuffer();
//! assert_eq!(pixels.len(), console_core::FB_LEN);
//! assert_eq!(con.frame_count(), 1);
//! # Ok::<(), console_core::Error>(())
//! ```

mod api;
mod audio;
mod cart;
mod draw_trace;
mod error;
mod font;
mod gfx;
mod gfx_meta;
mod rng;
mod state;

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Function, Lua, StdLib, Value};

pub use crate::api::BUTTON_COUNT;
pub use crate::audio::{
    ARP_FRAMES_PER_STEP, AudioBank, AudioFrame, CENTS_TO_RATIO, CHANNEL_COUNT, ChannelInfo,
    DUCK_ATTACK_SAMPLES, Duck, ECHO_LINE_LEN, ECHO_LP_CUTOFF_HZ, Echo, Env, FM_DECAY_HALF_LIFE, Fm,
    Fx, Instrument, LFO_STEPS, MASTER_REF_LEVEL, MAX_DRIVE, MAX_DUCK_DEPTH, MAX_ECHO_DELAY,
    MAX_ECHO_FEEDBACK, MAX_ECHO_LEVEL, MAX_ECHO_SEND, MAX_FM_DECAY, MAX_FM_INDEX,
    MAX_FM_RATIO_HALF, MAX_FX_SEMIS, MAX_HISS, MAX_ID, MAX_SFX_ROWS, MAX_TONE, MAX_TREM_DEPTH,
    MAX_TREM_RATE, MAX_VIB_CENTS, MAX_VIB_RATE, MAX_VOL, MIN_FM_RATIO_HALF, Master, NIBBLE_LEVEL,
    NOTE_FREQ, PERIODIC_PERIOD, PERIODIC_TRANSPOSE_SEMIS, Pattern, PatternEnd, PreviewSynth,
    RAMP_SAMPLES, RowMod, SAMPLE_RATE, SAMPLES_PER_FRAME, SINE_QUARTER, Sfx, SfxRow, Sweep,
    TONE_CUTOFF_HZ, Tempo, Trem, Vib, WAVE_COUNT, WAVE_FM, WAVE_PERIODIC, WAVE_TABLE_BASE,
    WAVETABLE_LEN, WAVETABLE_SLOTS, Wavetable, freq_at, parse_note,
};
pub use crate::cart::{Cart, PreviewPalette};
pub use crate::draw_trace::{
    Bounds as DrawBounds, DrawDetails, DrawEvent, DrawTraceFrame, MAX_DRAW_EVENTS_PER_FRAME,
    PaletteRemap as DrawPaletteRemap,
};
pub use crate::error::Error;
pub use crate::gfx::{
    COLOR_ALPHABET, COLOR_COUNT, COLOR_MASK, DrawState, FB_LEN, FILLP_SIZE, Framebuffer,
    IDENTITY_PAL, MAP_H, MAP_LEN, MAP_W, MAX_MOSAIC, PALETTE, SCREEN_H, SCREEN_W, SHEET_LEN,
    SHEET_W, SPRITE_SIZE, ShiftTable, SpriteSheet, TextAlign, TextDraw, TextLayout, TileMap,
    color_char, parse_color_char, text_size,
};
pub use crate::gfx_meta::{AnimDef, FrameSpec, GfxMeta, SpriteDef};
pub use crate::rng::Pcg32;
pub use crate::state::{LAYER_TRANSPARENT, MAX_CAPTURED_LAYERS};
/// Re-exported so hosts (`console`, `console-web`) can talk to the VM
/// without pinning their own, possibly different, mlua version.
pub use mlua;

/// Button bit positions within the input mask.
pub mod input {
    pub const LEFT: u8 = 1 << 0;
    pub const RIGHT: u8 = 1 << 1;
    pub const UP: u8 = 1 << 2;
    pub const DOWN: u8 = 1 << 3;
    pub const A: u8 = 1 << 4;
    pub const B: u8 = 1 << 5;
    /// Game-facing menu/start button (the triangle above the device-menu
    /// pill on the web shell). Distinct from the shell's own pause menu.
    pub const MENU: u8 = 1 << 6;
    /// All defined buttons.
    pub const MASK: u8 = 0b0111_1111;

    /// Parse a letter input spec such as `"RA"` (right + A). Unknown letters
    /// are reported to the caller. Case-insensitive; `-`, `.` and spaces are
    /// ignored so `"--"` can mean "no buttons".
    pub fn parse(spec: &str) -> Result<u8, char> {
        let mut mask = 0u8;
        for ch in spec.chars() {
            mask |= match ch.to_ascii_uppercase() {
                'L' => LEFT,
                'R' => RIGHT,
                'U' => UP,
                'D' => DOWN,
                'A' => A,
                'B' => B,
                'M' => MENU,
                ' ' | '-' | '.' | '_' | ',' => 0,
                other => return Err(other),
            };
        }
        Ok(mask)
    }
}

/// A loaded cart plus its running Lua VM.
pub struct Console {
    lua: Lua,
    state: Rc<RefCell<state::State>>,
    /// Snapshot of the framebuffer, refreshed after every step/eval so
    /// [`Console::framebuffer`] can hand out a plain reference.
    fb: Box<Framebuffer>,
    /// Snapshot of the display palette, refreshed alongside `fb`.
    dpal: [u8; COLOR_COUNT],
    /// Samples rendered by the most recent successful [`Console::step`].
    /// All zeros before the first step and after a halt.
    audio: Box<AudioFrame>,
    cart: Cart,
    seed: u64,
    halted: Option<Error>,
}

/// One isolated diagnostic layer from the most recently rendered frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedLayer {
    pub tag: Option<String>,
    pub framebuffer: Box<Framebuffer>,
}

/// Bounded snapshot of all `draw_tag()` layers in the current frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerCaptureFrame {
    pub enabled: bool,
    pub capacity: usize,
    pub dropped: u32,
    pub layers: Vec<CapturedLayer>,
}

impl Console {
    /// Parse `cart_text`, build the sandboxed VM, run the cart's top-level
    /// chunk and then `_init()` if it exists.
    pub fn new(cart_text: &str, seed: u64) -> Result<Console, Error> {
        let cart = Cart::parse(cart_text)?;
        Console::from_cart(cart, seed)
    }

    /// Same as [`Console::new`] with an already-parsed cart.
    pub fn from_cart(cart: Cart, seed: u64) -> Result<Console, Error> {
        // Only the pure standard libraries; `io`/`os`/`debug`/`package` are
        // never loaded in the first place, and `sandbox` clears the base
        // library's loaders on top of that.
        let lua = Lua::new_with(
            StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::COROUTINE,
            mlua::LuaOptions::default(),
        )?;

        let st = Rc::new(RefCell::new(state::State::new(
            Box::new(*cart.sprites()),
            Box::new(*cart.map()),
            Rc::new(cart.gfx_meta().clone()),
            seed,
            cart.audio().clone(),
        )));

        api::sandbox(&lua)?;
        api::register(&lua, &st)?;

        let mut console = Console {
            lua,
            state: st,
            fb: Box::new([0u8; FB_LEN]),
            dpal: IDENTITY_PAL,
            audio: Box::new([0.0; SAMPLES_PER_FRAME]),
            cart,
            seed,
            halted: None,
        };

        let source = console.cart.lua().to_string();
        console
            .lua
            .load(&source)
            .set_name("@cart.lua")
            .exec()
            .map_err(Error::from)?;

        if let Some(init) = console.func("_init")? {
            init.call::<()>(()).map_err(Error::from)?;
        }
        console.sync_framebuffer();
        Ok(console)
    }

    /// Advance exactly one frame: latch input, run `_update()` then `_draw()`,
    /// then render exactly [`SAMPLES_PER_FRAME`] audio samples from the
    /// post-update channel state.
    ///
    /// On a Lua error the console halts: the error is remembered and every
    /// later call returns the same `Err` without running any cart code. A
    /// halted console renders silence.
    pub fn step(&mut self, input: u8) -> Result<(), Error> {
        if let Some(err) = &self.halted {
            self.audio.fill(0.0);
            return Err(err.clone());
        }
        {
            let mut s = self.state.borrow_mut();
            s.text_draws.clear();
            s.clear_draw_events();
            s.clear_layer_framebuffers();
            s.prev_input = s.input;
            s.input = input & input::MASK;
        }

        let result = self.run_frame();
        self.sync_framebuffer();
        match result {
            Ok(()) => {
                let mut s = self.state.borrow_mut();
                s.frame += 1;
                // Render this frame's samples from the state `_update`/`_draw`
                // left behind, then advance the sequencer for the next frame.
                s.audio.render();
                self.audio.copy_from_slice(s.audio.frame());
                s.audio.advance();
                drop(s);
                Ok(())
            }
            Err(e) => {
                self.state.borrow_mut().audio.silence();
                self.audio.fill(0.0);
                self.halted = Some(e.clone());
                Err(e)
            }
        }
    }

    fn run_frame(&mut self) -> Result<(), Error> {
        if let Some(update) = self.func("_update")? {
            update.call::<()>(())?;
        }
        if let Some(draw) = self.func("_draw")? {
            draw.call::<()>(())?;
        }
        Ok(())
    }

    fn func(&self, name: &str) -> Result<Option<Function>, Error> {
        match self.lua.globals().get::<Value>(name)? {
            Value::Function(f) => Ok(Some(f)),
            _ => Ok(None),
        }
    }

    /// The current frame's pixels: one palette index (0..=63) per pixel,
    /// row-major, 192 wide by 320 tall.
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.fb
    }

    /// The display palette: a 64-entry index -> index map that the **host**
    /// applies when it turns framebuffer indices into colours (`pal(c0, c1, 1)`
    /// in Lua). Identity by default.
    ///
    /// This never touches [`Console::framebuffer`], which always holds raw
    /// draw-space indices — that is what keeps goldens and `screen_text` stable
    /// while a cart fades the whole screen.
    pub fn display_palette(&self) -> &[u8; COLOR_COUNT] {
        &self.dpal
    }

    /// The full persistent draw state: camera, clip rectangle, both palette
    /// maps and sprite transparency.
    pub fn draw_state(&self) -> DrawState {
        self.state.borrow().draw.clone()
    }

    /// The mono 44100 Hz samples rendered by the most recent [`Console::step`].
    ///
    /// All zeros before the first step and for every step of a halted console.
    pub fn audio_frame(&self) -> &AudioFrame {
        &self.audio
    }

    /// Read-only snapshot of the four synth channels.
    pub fn audio_channels(&self) -> [ChannelInfo; CHANNEL_COUNT] {
        self.state.borrow().audio.channel_info()
    }

    /// The music pattern currently playing, if any.
    pub fn music_pattern(&self) -> Option<u8> {
        self.state.borrow().audio.music_pattern()
    }

    /// The master bus setting in force: the cart's `__instruments__` `master`
    /// line unless Lua's `master()` has overridden it. All-zero means the
    /// output stage is bypassed entirely.
    pub fn master(&self) -> Master {
        self.state.borrow().audio.master()
    }

    /// The echo bus setting in force: the cart's `__instruments__` `echo` line
    /// unless Lua's `echo()` has overridden it. All-zero means the delay line
    /// is not running at all.
    pub fn echo(&self) -> Echo {
        self.state.borrow().audio.echo()
    }

    /// True when the echo delay line holds nothing (silent, or never used).
    pub fn echo_is_silent(&self) -> bool {
        self.state.borrow().audio.echo_is_silent()
    }

    /// Sidechain state: how much the non-trigger channels are attenuated right
    /// now (0 = not ducking) and which channel is exempt.
    pub fn duck_state(&self) -> (f32, Option<u8>) {
        self.state.borrow().audio.duck_state()
    }

    /// Number of completed frames. `t()` in Lua is this divided by 60.
    pub fn frame_count(&self) -> u64 {
        self.state.borrow().frame
    }

    /// The seed this console was created with.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The parsed cart (meta, Lua source, sprites).
    pub fn cart(&self) -> &Cart {
        &self.cart
    }

    /// A read-only snapshot of the live 128x64 tile map.
    ///
    /// This starts as [`Cart::map`] and then reflects every deterministic
    /// `mset` performed by cart code. Returning a snapshot keeps the host
    /// inspection surface read-only and avoids exposing the runtime's
    /// interior `RefCell` borrow across API boundaries.
    pub fn live_map(&self) -> TileMap {
        *self.state.borrow().map
    }

    /// Enable or disable draw-event tracing. Tracing is off by default and
    /// does not participate in rendering; toggling it clears buffered events.
    pub fn set_draw_tracing(&mut self, enabled: bool) {
        let mut state = self.state.borrow_mut();
        state.draw_trace_enabled = enabled;
        state.clear_draw_events();
    }

    /// Whether this console currently records draw events.
    pub fn draw_tracing(&self) -> bool {
        self.state.borrow().draw_trace_enabled
    }

    /// Drain the current frame's bounded trace buffer.
    pub fn take_draw_events(&mut self) -> DrawTraceFrame {
        let mut state = self.state.borrow_mut();
        DrawTraceFrame {
            events: std::mem::take(&mut state.draw_events),
            dropped: std::mem::take(&mut state.draw_events_dropped),
        }
    }

    /// Enable or disable isolated `draw_tag()` framebuffers. Capture is off by
    /// default and has no effect on the presented framebuffer.
    pub fn set_layer_capture(&mut self, enabled: bool) {
        self.state.borrow_mut().set_layer_capture(enabled);
    }

    pub fn layer_capture_enabled(&self) -> bool {
        self.state.borrow().layer_capture_enabled
    }

    /// Clone the current frame's non-empty diagnostic layers and apply the
    /// same end-of-frame mosaic/raster effects as the player-visible image.
    pub fn layer_capture_frame(&self) -> LayerCaptureFrame {
        let state = self.state.borrow();
        let mosaic = state.draw.mosaic();
        let rshift = state
            .draw
            .rshift_active()
            .then(|| *state.draw.rshift_table());
        let mut layers = Vec::new();
        for (tag, framebuffer) in &state.layer_framebuffers {
            if framebuffer
                .iter()
                .all(|&pixel| pixel == state::LAYER_TRANSPARENT)
            {
                continue;
            }
            let mut framebuffer = framebuffer.clone();
            gfx::apply_mosaic(&mut framebuffer, mosaic);
            if let Some(shifts) = &rshift {
                gfx::apply_rshift(&mut framebuffer, shifts);
            }
            layers.push(CapturedLayer {
                tag: tag.clone(),
                framebuffer,
            });
        }
        LayerCaptureFrame {
            enabled: state.layer_capture_enabled,
            capacity: state::MAX_CAPTURED_LAYERS,
            dropped: state.layer_capture_dropped,
            layers,
        }
    }

    /// Drain buffered `printh` output.
    pub fn take_logs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.state.borrow_mut().logs)
    }

    /// Drain `print` calls captured since the current frame began. The core
    /// also clears this buffer automatically at the start of every step, so
    /// hosts that do not inspect text never accumulate an unbounded log.
    pub fn take_text_draws(&mut self) -> Vec<TextDraw> {
        std::mem::take(&mut self.state.borrow_mut().text_draws)
    }

    /// The error that halted the console, if any.
    pub fn halted(&self) -> Option<&Error> {
        self.halted.as_ref()
    }

    /// True once a Lua error has halted the cart.
    pub fn is_halted(&self) -> bool {
        self.halted.is_some()
    }

    /// The button mask latched for the most recent [`Console::step`].
    pub fn input(&self) -> u8 {
        self.state.borrow().input
    }

    /// Run arbitrary Lua in the cart's environment (used by the agent harness'
    /// `eval`). Does not clear a halt and does not advance the frame counter,
    /// but does refresh the framebuffer snapshot in case the code drew.
    pub fn eval(&mut self, code: &str) -> Result<Value, Error> {
        let result = self
            .lua
            .load(code)
            .set_name("@eval")
            .eval::<Value>()
            .map_err(Error::from);
        self.sync_framebuffer();
        result
    }

    /// Read a global out of the cart's environment.
    pub fn get_global(&self, name: &str) -> Result<Value, Error> {
        Ok(self.lua.globals().get::<Value>(name)?)
    }

    /// Direct access to the VM for hosts that need it.
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Refresh the presented frame from the draw framebuffer.
    ///
    /// This is where the end-of-frame effects happen, in order: the cart draws
    /// at full resolution, the finished frame is pixelated by `mosaic(f)` and
    /// then displaced scanline by scanline by `rshift(y, dx)`. Both are part of
    /// the deterministic framebuffer (screenshots, `screen_text`, goldens) but
    /// neither feeds back into the next frame's drawing or into `pget`.
    ///
    /// Mosaic first, rshift second: the raster shift moves finished pixels, so
    /// it can never split a mosaic block, and water-over-pixelation looks the
    /// way a real HDMA scanline effect on a mosaic-ed layer looked.
    fn sync_framebuffer(&mut self) {
        let s = self.state.borrow();
        self.fb.copy_from_slice(&s.fb[..]);
        self.dpal = *s.draw.display_palette();
        let mosaic = s.draw.mosaic();
        // Copied out (320 bytes) only when a cart actually uses the effect, so
        // the default path stays the plain framebuffer memcpy above.
        let rshift = s.draw.rshift_active().then(|| *s.draw.rshift_table());
        drop(s);
        gfx::apply_mosaic(&mut self.fb, mosaic);
        if let Some(shifts) = rshift {
            gfx::apply_rshift(&mut self.fb, &shifts);
        }
    }
}

impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Console")
            .field("title", &self.cart.title())
            .field("seed", &self.seed)
            .field("frame", &self.frame_count())
            .field("halted", &self.halted)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_spec_parsing() {
        assert_eq!(input::parse("RA"), Ok(input::RIGHT | input::A));
        assert_eq!(input::parse(""), Ok(0));
        assert_eq!(input::parse("--"), Ok(0));
        assert_eq!(input::parse("ludrabm"), Ok(input::MASK));
        assert_eq!(input::parse("q"), Err('Q'));
    }

    #[test]
    fn palette_is_apollo64() {
        assert_eq!(PALETTE.len(), 64);
        assert_eq!(PALETTE[0], [0x17, 0x20, 0x38]);
        assert_eq!(PALETTE[31], [0xe8, 0xc1, 0x70]);
        assert_eq!(PALETTE[63], [0xeb, 0xed, 0xe9]);
    }
}

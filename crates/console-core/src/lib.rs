//! Deterministic fantasy-console core.
//!
//! A [`Console`] owns a sandboxed Lua 5.4 VM, a 144x256 palette-indexed
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
    ARP_FRAMES_PER_STEP, AudioBank, AudioFrame, CENTS_TO_RATIO, CHANNEL_COUNT, ChannelInfo, Env,
    Fx, Instrument, LFO_STEPS, MAX_FX_SEMIS, MAX_ID, MAX_SFX_ROWS, MAX_VIB_CENTS, MAX_VIB_RATE,
    MAX_VOL, NOTE_FREQ, Pattern, PatternEnd, RAMP_SAMPLES, RowMod, SAMPLE_RATE, SAMPLES_PER_FRAME,
    Sfx, SfxRow, Sweep, Tempo, Vib, WAVE_COUNT, freq_at, parse_note,
};
pub use crate::cart::Cart;
pub use crate::error::Error;
pub use crate::gfx::{
    FB_LEN, Framebuffer, PALETTE, SCREEN_H, SCREEN_W, SHEET_LEN, SHEET_W, SPRITE_SIZE, SpriteSheet,
};
pub use crate::gfx_meta::{AnimDef, GfxMeta, SpriteDef};
pub use crate::rng::Pcg32;
/// Re-exported so hosts (`console-agent`, `console-web`) can talk to the VM
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
    /// Samples rendered by the most recent successful [`Console::step`].
    /// All zeros before the first step and after a halt.
    audio: Box<AudioFrame>,
    cart: Cart,
    seed: u64,
    halted: Option<Error>,
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
            seed,
            cart.audio().clone(),
        )));

        api::sandbox(&lua)?;
        api::register(&lua, &st)?;

        let mut console = Console {
            lua,
            state: st,
            fb: Box::new([0u8; FB_LEN]),
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

    /// The current frame's pixels: one palette index (0..=15) per pixel,
    /// row-major, 144 wide by 256 tall.
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.fb
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

    /// Drain buffered `printh` output.
    pub fn take_logs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.state.borrow_mut().logs)
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

    fn sync_framebuffer(&mut self) {
        self.fb.copy_from_slice(&self.state.borrow().fb[..]);
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
    fn palette_is_sweetie16() {
        assert_eq!(PALETTE.len(), 16);
        assert_eq!(PALETTE[0], [0x1a, 0x1c, 0x2c]);
        assert_eq!(PALETTE[12], [0xf4, 0xf4, 0xf4]);
        assert_eq!(PALETTE[15], [0x33, 0x3c, 0x57]);
    }
}

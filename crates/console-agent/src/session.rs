//! Session state shared by the oneshot CLI and the JSON-RPC `serve` loop.
//!
//! A [`Session`] owns at most one running [`Console`], the cart text and
//! seed it was built from, the full per-frame input log since the last
//! reset/load (needed for save/load state and for replay-based resets), and
//! a table of named save states.

use std::collections::BTreeMap;

use console_core::{Console, Error, FB_LEN, PALETTE, SCREEN_H, SCREEN_W, input};

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

#[derive(Default)]
pub struct Session {
    cart_text: Option<String>,
    console: Option<Console>,
    seed: u64,
    input_log: Vec<u8>,
    saved_states: BTreeMap<String, SavedState>,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    /// Load a new cart from source text and (re)build the console. Clears
    /// the input log and any save states from a previous cart, since a
    /// save state's replay only makes sense against the cart it was
    /// recorded on.
    pub fn load_cart(&mut self, text: &str, seed: u64) -> Result<(), SessionError> {
        let console = Console::new(text, seed)?;
        self.cart_text = Some(text.to_string());
        self.seed = seed;
        self.console = Some(console);
        self.input_log.clear();
        self.saved_states.clear();
        Ok(())
    }

    /// Recreate the console from the stored cart text, optionally with a
    /// new seed, and clear the input log. The save-state table survives
    /// (states are self-contained: cart text + their own seed + log).
    pub fn reset(&mut self, seed: Option<u64>) -> Result<(), SessionError> {
        let text = self.cart_text.clone().ok_or(SessionError::NoCart)?;
        let seed = seed.unwrap_or(self.seed);
        let console = Console::new(&text, seed)?;
        self.seed = seed;
        self.console = Some(console);
        self.input_log.clear();
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
        // Access the field directly (rather than via a `&mut self` helper
        // method) so the borrow checker can see `self.console` and
        // `self.input_log` as disjoint and let the loop below mutate both.
        let console = self.console.as_mut().ok_or(SessionError::NoCart)?;
        if let Some(err) = console.halted() {
            return Err(SessionError::AlreadyHalted(err.message().to_string()));
        }

        let mask = mask & input::MASK;
        let mut halt_message = None;
        for _ in 0..frames {
            self.input_log.push(mask);
            if let Err(e) = console.step(mask) {
                halt_message = Some(e.message().to_string());
                break;
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
    /// then replay its input log frame-by-frame. Returns the number of
    /// frames replayed.
    pub fn load_state(&mut self, name: &str) -> Result<StepOutcome, SessionError> {
        let text = self.cart_text.clone().ok_or(SessionError::NoCart)?;
        let saved = self
            .saved_states
            .get(name)
            .cloned()
            .ok_or_else(|| SessionError::BadParams(format!("no saved state named {name:?}")))?;

        let mut console = Console::new(&text, saved.seed)?;
        let mut halt_message = None;
        let mut replayed = 0u64;
        for &mask in &saved.input_log {
            match console.step(mask) {
                Ok(()) => replayed += 1,
                Err(e) => {
                    halt_message = Some(e.message().to_string());
                    break;
                }
            }
        }

        self.seed = saved.seed;
        self.input_log = saved.input_log;
        self.console = Some(console);

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
    let mut rgba = Vec::with_capacity(FB_LEN * 4);
    for &idx in fb.iter() {
        let [r, g, b] = PALETTE[(idx & 0x0f) as usize];
        rgba.extend_from_slice(&[r, g, b, 255]);
    }

    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, SCREEN_W as u32, SCREEN_H as u32);
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

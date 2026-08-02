//! Mutable console state shared between Rust and the Lua closures.

use crate::audio::{Audio, AudioBank};
use crate::gfx::{FB_LEN, Framebuffer, SHEET_LEN, SpriteSheet};
use crate::rng::Pcg32;

/// Everything the Lua API can touch. Owned by the [`Console`](crate::Console)
/// through an `Rc<RefCell<_>>` that every registered closure captures.
#[derive(Debug)]
pub struct State {
    pub fb: Box<Framebuffer>,
    pub sheet: Box<SpriteSheet>,
    /// Button mask for the frame being processed.
    pub input: u8,
    /// Button mask from the previous frame (drives `btnp`).
    pub prev_input: u8,
    /// Completed frames. `t()` is `frame / 60`.
    pub frame: u64,
    pub rng: Pcg32,
    /// Synth + sequencer. Never reads the PRNG, so audio can never perturb
    /// framebuffer determinism.
    pub audio: Audio,
    /// `printh` output, drained by the host.
    pub logs: Vec<String>,
}

impl State {
    pub fn new(sheet: Box<SpriteSheet>, seed: u64, bank: AudioBank) -> State {
        State {
            fb: Box::new([0u8; FB_LEN]),
            sheet,
            input: 0,
            prev_input: 0,
            frame: 0,
            rng: Pcg32::new(seed),
            audio: Audio::new(bank),
            logs: Vec::new(),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        State::new(Box::new([0u8; SHEET_LEN]), 0, AudioBank::default())
    }
}

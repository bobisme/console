//! Mutable console state shared between Rust and the Lua closures.

use std::rc::Rc;

use crate::audio::{Audio, AudioBank};
use crate::gfx::{DrawState, FB_LEN, Framebuffer, MAP_LEN, SHEET_LEN, SpriteSheet, TileMap};
use crate::gfx_meta::GfxMeta;
use crate::rng::Pcg32;

/// Everything the Lua API can touch. Owned by the [`Console`](crate::Console)
/// through an `Rc<RefCell<_>>` that every registered closure captures.
#[derive(Debug)]
pub struct State {
    pub fb: Box<Framebuffer>,
    /// Camera / clip / palette / transparency. Persists across frames; only a
    /// cart call (or a fresh console) changes it.
    pub draw: DrawState,
    pub sheet: Box<SpriteSheet>,
    /// The live 128x64 tile map: the cart's `__map__` at load, then whatever
    /// `mset` has made of it. Mutations persist across frames and are part of
    /// console state, so a replay of the same inputs reproduces them exactly.
    pub map: Box<TileMap>,
    /// The cart's `__gfx_meta__`, shared with the cart itself: read-only
    /// indexing over the sheet that `aspr`/`anim_len`/`anim_done` resolve
    /// names against. Nothing mutates it, so it is never part of the replay
    /// state.
    pub gfx_meta: Rc<GfxMeta>,
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
    pub fn new(
        sheet: Box<SpriteSheet>,
        map: Box<TileMap>,
        gfx_meta: Rc<GfxMeta>,
        seed: u64,
        bank: AudioBank,
    ) -> State {
        State {
            fb: Box::new([0u8; FB_LEN]),
            draw: DrawState::new(),
            sheet,
            map,
            gfx_meta,
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
        State::new(
            Box::new([0u8; SHEET_LEN]),
            Box::new([0u8; MAP_LEN]),
            Rc::new(GfxMeta::default()),
            0,
            AudioBank::default(),
        )
    }
}

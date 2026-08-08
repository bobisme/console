//! Mutable console state shared between Rust and the Lua closures.

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::audio::{Audio, AudioBank};
use crate::draw_trace::{
    Bounds, DrawEvent, DrawSpec, MAX_DRAW_EVENTS_PER_FRAME, PaletteRemap, ScreenBounds,
};
use crate::gfx::{
    DrawState, FB_LEN, Framebuffer, GfxFlags, MAP_LEN, SHEET_LEN, SpriteSheet, TILE_COUNT,
    TextDraw, TileMap,
};
use crate::gfx_meta::GfxMeta;
use crate::rng::Pcg32;
use crate::save::SaveState;

/// Pixel value used only inside diagnostic layer buffers to mean "this tag
/// did not draw here". Runtime pixels are always masked to 0..=63, so the
/// sentinel cannot collide with a real cart colour (including colour 0).
pub const LAYER_TRANSPARENT: u8 = u8::MAX;

/// A diagnostic capture is deliberately bounded. A typo that generates a new
/// tag every frame must not grow the runtime without limit.
pub const MAX_CAPTURED_LAYERS: usize = 32;

/// Everything the Lua API can touch. Owned by the [`Console`](crate::Console)
/// through an `Rc<RefCell<_>>` that every registered closure captures.
#[derive(Debug)]
pub struct State {
    pub fb: Box<Framebuffer>,
    /// Camera / clip / palette / transparency. Persists across frames; only a
    /// cart call (or a fresh console) changes it.
    pub draw: DrawState,
    pub sheet: Box<SpriteSheet>,
    /// Mutable per-tile flag bytes. Authored values come from `__gfx_flags__`;
    /// `fset` changes only this running copy.
    pub gfx_flags: Box<GfxFlags>,
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
    /// Deterministic cart-visible save document. Hosts supply its initial
    /// value before `_init` and observe committed revisions after successful
    /// boundaries; this state itself performs no I/O.
    pub save: SaveState,
    /// `printh` output, drained by the host.
    pub logs: Vec<String>,
    /// Calls to `print` since the current frame began. Core clears this at the
    /// next step so web hosts stay bounded; agent hosts drain it into a log.
    pub text_draws: Vec<TextDraw>,
    pub draw_trace_enabled: bool,
    pub draw_tag: Option<String>,
    pub draw_events: Vec<DrawEvent>,
    pub draw_events_dropped: u32,
    /// Opt-in diagnostic framebuffers keyed by `draw_tag()`. `None` records
    /// draws made without a tag. These never participate in normal rendering.
    pub layer_capture_enabled: bool,
    pub layer_framebuffers: BTreeMap<Option<String>, Box<Framebuffer>>,
    pub layer_capture_dropped: u32,
}

impl State {
    pub fn new(
        sheet: Box<SpriteSheet>,
        gfx_flags: Box<GfxFlags>,
        map: Box<TileMap>,
        gfx_meta: Rc<GfxMeta>,
        seed: u64,
        bank: AudioBank,
        save: SaveState,
    ) -> State {
        State {
            fb: Box::new([0u8; FB_LEN]),
            draw: DrawState::new(),
            sheet,
            gfx_flags,
            map,
            gfx_meta,
            input: 0,
            prev_input: 0,
            frame: 0,
            rng: Pcg32::new(seed),
            audio: Audio::new(bank),
            save,
            logs: Vec::new(),
            text_draws: Vec::new(),
            draw_trace_enabled: false,
            draw_tag: None,
            draw_events: Vec::new(),
            draw_events_dropped: 0,
            layer_capture_enabled: false,
            layer_framebuffers: BTreeMap::new(),
            layer_capture_dropped: 0,
        }
    }

    /// Draw to the real framebuffer and, when diagnostics are enabled, repeat
    /// the same operation against the active tag's transparent framebuffer.
    /// The first return value is authoritative; the diagnostic result is
    /// intentionally discarded (notably for `print`).
    pub fn draw_with_layer<R>(
        &mut self,
        mut operation: impl FnMut(&mut Framebuffer, &DrawState, &SpriteSheet, &TileMap) -> R,
    ) -> R {
        let State {
            fb,
            draw,
            sheet,
            map,
            draw_tag,
            layer_capture_enabled,
            layer_framebuffers,
            layer_capture_dropped,
            ..
        } = self;
        let result = operation(fb, draw, sheet, map);
        if !*layer_capture_enabled {
            return result;
        }

        let tag = draw_tag.clone();
        if !layer_framebuffers.contains_key(&tag) && layer_framebuffers.len() >= MAX_CAPTURED_LAYERS
        {
            *layer_capture_dropped = layer_capture_dropped.saturating_add(1);
            return result;
        }
        let layer = layer_framebuffers
            .entry(tag)
            .or_insert_with(|| Box::new([LAYER_TRANSPARENT; FB_LEN]));
        let _ = operation(layer, draw, sheet, map);
        result
    }

    pub fn set_layer_capture(&mut self, enabled: bool) {
        self.layer_capture_enabled = enabled;
        self.layer_framebuffers.clear();
        self.layer_capture_dropped = 0;
    }

    /// Start a fresh set of current-frame layer evidence. Capacity is a
    /// per-frame limit: historical tags must never crowd out a tag that is
    /// active now.
    pub fn clear_layer_framebuffers(&mut self) {
        if !self.layer_capture_enabled {
            return;
        }
        self.layer_framebuffers.clear();
        self.layer_capture_dropped = 0;
    }

    pub fn clear_draw_events(&mut self) {
        self.draw_events.clear();
        self.draw_events_dropped = 0;
    }

    pub fn record_draw(&mut self, spec: DrawSpec) {
        if !self.draw_trace_enabled {
            return;
        }
        if self.draw_events.len() >= MAX_DRAW_EVENTS_PER_FRAME {
            self.draw_events_dropped = self.draw_events_dropped.saturating_add(1);
            return;
        }
        let camera = self.draw.camera();
        let screen_bounds = match spec.screen {
            ScreenBounds::ScreenSpace => spec.bounds,
            ScreenBounds::Origin { x, y } => Bounds::wide_xywh(
                i64::from(x.saturating_sub(camera.0)),
                i64::from(y.saturating_sub(camera.1)),
                spec.bounds.w,
                spec.bounds.h,
            ),
            ScreenBounds::Corners { x0, y0, x1, y1 } => Bounds::corners(
                x0.saturating_sub(camera.0),
                y0.saturating_sub(camera.1),
                x1.saturating_sub(camera.0),
                y1.saturating_sub(camera.1),
            ),
            ScreenBounds::Circle { x, y, radius } => {
                if radius < 0 {
                    Bounds::wide_xywh(
                        i64::from(x.saturating_sub(camera.0)),
                        i64::from(y.saturating_sub(camera.1)),
                        0,
                        0,
                    )
                } else {
                    let radius = i64::from(radius);
                    Bounds::wide_xywh(
                        i64::from(x.saturating_sub(camera.0)) - radius,
                        i64::from(y.saturating_sub(camera.1)) - radius,
                        radius * 2 + 1,
                        radius * 2 + 1,
                    )
                }
            }
            ScreenBounds::Explicit(bounds) => bounds,
        };
        let (clip_x0, clip_y0, clip_x1, clip_y1) = self.draw.clip();
        let clip = if clip_x0 > clip_x1 || clip_y0 > clip_y1 {
            Bounds::xywh(0, 0, 0, 0)
        } else {
            Bounds::corners(clip_x0, clip_y0, clip_x1, clip_y1)
        };
        let visible_bounds = screen_bounds.intersection(clip);
        let clipped = visible_bounds != Some(screen_bounds);
        let transparent_colors = (0..64)
            .filter(|color| self.draw.palt_mask() & (1u64 << color) != 0)
            .map(|color| color as u8)
            .collect();
        let palette_remaps = |palette: &[u8; 64]| {
            palette
                .iter()
                .enumerate()
                .filter_map(|(from, &to)| {
                    (usize::from(to) != from).then_some(PaletteRemap {
                        from: from as u8,
                        to,
                    })
                })
                .collect()
        };
        self.draw_events.push(DrawEvent {
            op: spec.op,
            tag: self.draw_tag.clone(),
            world_bounds: spec.bounds,
            screen_bounds,
            visible_bounds,
            clipped,
            camera: [camera.0, camera.1],
            clip,
            draw_palette: palette_remaps(self.draw.draw_palette()),
            display_palette: palette_remaps(self.draw.display_palette()),
            transparent_colors,
            fill_pattern: self.draw.fillp(),
            fill_secondary: self.draw.fill_secondary(),
            details: spec.details,
        });
    }
}

impl Default for State {
    fn default() -> Self {
        State::new(
            Box::new([0u8; SHEET_LEN]),
            Box::new([0u8; TILE_COUNT]),
            Box::new([0; MAP_LEN]),
            Rc::new(GfxMeta::default()),
            0,
            AudioBank::default(),
            SaveState::new(None, None),
        )
    }
}

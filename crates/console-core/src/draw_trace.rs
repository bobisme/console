//! Optional, bounded draw-call diagnostics.

use serde::Serialize;

/// Maximum calls retained from one frame. Excess calls are counted, not
/// allocated, so hostile or accidental particle loops cannot grow memory.
pub const MAX_DRAW_EVENTS_PER_FRAME: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Bounds {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Bounds {
    pub fn xywh(x: i32, y: i32, w: i32, h: i32) -> Bounds {
        Bounds::wide_xywh(i64::from(x), i64::from(y), i64::from(w), i64::from(h))
    }

    pub(crate) fn wide_xywh(x: i64, y: i64, w: i64, h: i64) -> Bounds {
        Bounds {
            x,
            y,
            w: w.max(0),
            h: h.max(0),
        }
    }

    pub fn corners(x0: i32, y0: i32, x1: i32, y1: i32) -> Bounds {
        let lx = i64::from(x0.min(x1));
        let ty = i64::from(y0.min(y1));
        let rx = i64::from(x0.max(x1));
        let by = i64::from(y0.max(y1));
        Bounds {
            x: lx,
            y: ty,
            w: rx - lx + 1,
            h: by - ty + 1,
        }
    }

    pub(crate) fn intersection(self, other: Bounds) -> Option<Bounds> {
        if self.w <= 0 || self.h <= 0 || other.w <= 0 || other.h <= 0 {
            return None;
        }
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        (x0 < x1 && y0 < y1).then(|| Bounds {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

pub(crate) fn clamp_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DrawDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sprite_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animation_frame: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet_bounds: Option<Bounds>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_bounds: Option<Bounds>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flip_x: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flip_y: Option<bool>,
}

/// One non-identity palette entry. Omitted source indices map to themselves,
/// which keeps the common/default trace compact without losing state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PaletteRemap {
    pub from: u8,
    pub to: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DrawEvent {
    pub op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub world_bounds: Bounds,
    pub screen_bounds: Bounds,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_bounds: Option<Bounds>,
    pub clipped: bool,
    pub camera: [i32; 2],
    pub clip: Bounds,
    pub draw_palette: Vec<PaletteRemap>,
    pub display_palette: Vec<PaletteRemap>,
    pub transparent_colors: Vec<u8>,
    pub fill_pattern: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_secondary: Option<u8>,
    #[serde(flatten)]
    pub details: DrawDetails,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DrawTraceFrame {
    pub events: Vec<DrawEvent>,
    pub dropped: u32,
}

pub(crate) struct DrawSpec {
    pub op: &'static str,
    pub bounds: Bounds,
    pub screen: ScreenBounds,
    pub details: DrawDetails,
}

pub(crate) enum ScreenBounds {
    ScreenSpace,
    Origin { x: i32, y: i32 },
    Corners { x0: i32, y0: i32, x1: i32, y1: i32 },
    Circle { x: i32, y: i32, radius: i32 },
    Explicit(Bounds),
}

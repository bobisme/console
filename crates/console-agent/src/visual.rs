//! Deterministic runtime-sequence images and labeled visual review boards.

use console_core::PALETTE;
use serde::{Deserialize, Serialize};

pub const MAX_VISUAL_RGBA_BYTES: usize = 64 * 1024 * 1024;
const GUTTER: u32 = 8;
const PAD: u32 = 8;
const FONT_SCALE: u32 = 2;
const FONT_H: u32 = 5 * FONT_SCALE;
const LABEL_H: u32 = FONT_H + 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PanelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardLayout {
    pub runtime_panels: Vec<PanelRect>,
    pub reference_panel: Option<PanelRect>,
}

fn rgba_len(width: u32, height: u32) -> Result<usize, String> {
    let len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("visual RGBA dimensions overflow")?;
    if len > MAX_VISUAL_RGBA_BYTES {
        return Err(format!(
            "visual RGBA needs {len} bytes, exceeding the {MAX_VISUAL_RGBA_BYTES} byte limit"
        ));
    }
    Ok(len)
}

fn aggregate_rgba_len(
    width: u32,
    height: u32,
    frames: usize,
    label: &str,
) -> Result<usize, String> {
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| bytes.checked_mul(frames))
        .ok_or_else(|| format!("{label} aggregate RGBA work overflows"))?;
    if bytes > MAX_VISUAL_RGBA_BYTES {
        return Err(format!(
            "{label} aggregate RGBA work needs {bytes} bytes, exceeding the {MAX_VISUAL_RGBA_BYTES} byte limit"
        ));
    }
    Ok(bytes)
}

fn checked_add(a: u32, b: u32, label: &str) -> Result<u32, String> {
    a.checked_add(b)
        .ok_or_else(|| format!("{label} dimension overflow"))
}

fn checked_mul(a: u32, b: u32, label: &str) -> Result<u32, String> {
    a.checked_mul(b)
        .ok_or_else(|| format!("{label} dimension overflow"))
}

impl RgbaImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<RgbaImage, String> {
        let expected = rgba_len(width, height)?;
        if rgba.len() != expected {
            return Err(format!(
                "RGBA length {} does not match {width}x{height} ({expected} bytes)",
                rgba.len()
            ));
        }
        Ok(RgbaImage {
            width,
            height,
            rgba,
        })
    }

    pub fn crop(&self, rect: Rect) -> Result<RgbaImage, String> {
        let right = rect.x.checked_add(rect.w).ok_or("crop x+w overflows")?;
        let bottom = rect.y.checked_add(rect.h).ok_or("crop y+h overflows")?;
        if rect.w == 0 || rect.h == 0 {
            return Err("crop width and height must be >= 1".to_string());
        }
        if right > self.width || bottom > self.height {
            return Err(format!(
                "crop {},{},{},{} exceeds {}x{} image",
                rect.x, rect.y, rect.w, rect.h, self.width, self.height
            ));
        }
        let mut rgba = Vec::with_capacity(rgba_len(rect.w, rect.h)?);
        for y in rect.y..bottom {
            let start = ((y * self.width + rect.x) * 4) as usize;
            let end = start + rect.w as usize * 4;
            rgba.extend_from_slice(&self.rgba[start..end]);
        }
        RgbaImage::new(rect.w, rect.h, rgba)
    }

    pub fn zoom(&self, zoom: u32) -> Result<RgbaImage, String> {
        if zoom == 0 {
            return Err("visual zoom must be >= 1".to_string());
        }
        if zoom == 1 {
            return Ok(self.clone());
        }
        let width = checked_mul(self.width, zoom, "visual width")?;
        let height = checked_mul(self.height, zoom, "visual height")?;
        let mut rgba = vec![0; rgba_len(width, height)?];
        for sy in 0..self.height {
            for sx in 0..self.width {
                let source = ((sy * self.width + sx) * 4) as usize;
                for dy in 0..zoom {
                    let y = sy * zoom + dy;
                    for dx in 0..zoom {
                        let x = sx * zoom + dx;
                        let dest = ((y * width + x) * 4) as usize;
                        rgba[dest..dest + 4].copy_from_slice(&self.rgba[source..source + 4]);
                    }
                }
            }
        }
        RgbaImage::new(width, height, rgba)
    }

    pub fn png(&self) -> Vec<u8> {
        crate::palette::encode_png_rgba(&self.rgba, self.width, self.height)
    }
}

pub(crate) fn blank(width: u32, height: u32) -> Result<RgbaImage, String> {
    let [r, g, b] = PALETTE[48];
    let mut rgba = vec![0; rgba_len(width, height)?];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[r, g, b, 255]);
    }
    RgbaImage::new(width, height, rgba)
}

pub(crate) fn blit(dst: &mut RgbaImage, src: &RgbaImage, x: u32, y: u32) -> Result<(), String> {
    let right = x.checked_add(src.width).ok_or("blit x overflow")?;
    let bottom = y.checked_add(src.height).ok_or("blit y overflow")?;
    if right > dst.width || bottom > dst.height {
        return Err("internal visual blit exceeds canvas".to_string());
    }
    for sy in 0..src.height {
        let source = (sy * src.width * 4) as usize;
        let dest = (((y + sy) * dst.width + x) * 4) as usize;
        let bytes = src.width as usize * 4;
        dst.rgba[dest..dest + bytes].copy_from_slice(&src.rgba[source..source + bytes]);
    }
    Ok(())
}

/// Deterministic straight-alpha compositing for diagnostic sources such as
/// isolated draw-tag layers. Opaque pixels take the fast exact-copy path.
fn blit_over(dst: &mut RgbaImage, src: &RgbaImage, x: u32, y: u32) -> Result<(), String> {
    let right = x.checked_add(src.width).ok_or("blit x overflow")?;
    let bottom = y.checked_add(src.height).ok_or("blit y overflow")?;
    if right > dst.width || bottom > dst.height {
        return Err("internal visual blit exceeds canvas".to_string());
    }
    for sy in 0..src.height {
        for sx in 0..src.width {
            let source = ((sy * src.width + sx) * 4) as usize;
            let dest = ((((y + sy) * dst.width) + x + sx) * 4) as usize;
            let alpha = u32::from(src.rgba[source + 3]);
            match alpha {
                0 => {}
                255 => dst.rgba[dest..dest + 4].copy_from_slice(&src.rgba[source..source + 4]),
                _ => {
                    let inverse = 255 - alpha;
                    for channel in 0..3 {
                        let over = u32::from(src.rgba[source + channel]) * alpha;
                        let under = u32::from(dst.rgba[dest + channel]) * inverse;
                        dst.rgba[dest + channel] = ((over + under + 127) / 255) as u8;
                    }
                    dst.rgba[dest + 3] = 255;
                }
            }
        }
    }
    Ok(())
}

pub fn contact_strip(frames: &[RgbaImage], zoom: u32) -> Result<RgbaImage, String> {
    let first = frames.first().ok_or("sequence has no sampled frames")?;
    if frames
        .iter()
        .any(|frame| frame.width != first.width || frame.height != first.height)
    {
        return Err("sequence frames have inconsistent dimensions".to_string());
    }
    let panel_w = checked_mul(first.width, zoom, "strip frame width")?;
    let panel_h = checked_mul(first.height, zoom, "strip frame height")?;
    let count = u32::try_from(frames.len()).map_err(|_| "too many strip frames")?;
    let width = checked_add(
        checked_mul(panel_w, count, "strip width")?,
        checked_mul(GUTTER, count.saturating_sub(1), "strip gutters")?,
        "strip width",
    )?;
    let mut out = blank(width, panel_h)?;
    let mut x = 0;
    for frame in frames {
        let image = frame.zoom(zoom)?;
        blit(&mut out, &image, x, 0)?;
        x = checked_add(
            x,
            checked_add(panel_w, GUTTER, "strip offset")?,
            "strip offset",
        )?;
    }
    Ok(out)
}

pub fn gif_delay_cs(cadence_frames: u64) -> Result<u16, String> {
    let fifths = cadence_frames
        .checked_mul(5)
        .ok_or("sequence GIF cadence overflows")?;
    let rounded = fifths
        .checked_add(1)
        .ok_or("sequence GIF delay overflows")?
        / 3;
    u16::try_from(rounded.max(2)).map_err(|_| {
        format!("sequence cadence {cadence_frames} frames is too long for a GIF delay")
    })
}

pub fn animated_gif(
    frames: &[RgbaImage],
    zoom: u32,
    cadence_frames: u64,
) -> Result<(Vec<u8>, u32, u32, u16), String> {
    let first = frames.first().ok_or("sequence has no sampled frames")?;
    if frames
        .iter()
        .any(|frame| frame.width != first.width || frame.height != first.height)
    {
        return Err("sequence frames have inconsistent dimensions".to_string());
    }
    let width = checked_mul(first.width, zoom, "GIF width")?;
    let height = checked_mul(first.height, zoom, "GIF height")?;
    aggregate_rgba_len(width, height, frames.len(), "sequence GIF")?;
    let width16 = u16::try_from(width).map_err(|_| format!("GIF width {width} exceeds 65535"))?;
    let height16 =
        u16::try_from(height).map_err(|_| format!("GIF height {height} exceeds 65535"))?;
    let delay = gif_delay_cs(cadence_frames)?;
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, width16, height16, &[])
            .map_err(|error| format!("encoding sequence GIF: {error}"))?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .map_err(|error| format!("encoding sequence GIF repeat: {error}"))?;
        for (index, frame) in frames.iter().enumerate() {
            let mut rgba = frame.zoom(zoom)?.rgba;
            let mut gif_frame = gif::Frame::from_rgba_speed(width16, height16, &mut rgba, 10);
            gif_frame.delay = delay;
            encoder
                .write_frame(&gif_frame)
                .map_err(|error| format!("encoding sequence GIF frame {index}: {error}"))?;
        }
        encoder
            .into_inner()
            .map_err(|error| format!("finishing sequence GIF: {error}"))?;
    }
    Ok((bytes, width, height, delay))
}

fn glyph(ch: char) -> [u8; 5] {
    match ch.to_ascii_uppercase() {
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b111, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b110, 0b001, 0b010, 0b100, 0b111],
        '3' => [0b110, 0b001, 0b010, 0b001, 0b110],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b110, 0b001, 0b110],
        '6' => [0b011, 0b100, 0b110, 0b101, 0b010],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b010, 0b101, 0b010, 0b101, 0b010],
        '9' => [0b010, 0b101, 0b011, 0b001, 0b110],
        ':' => [0, 0b010, 0, 0b010, 0],
        ',' => [0, 0, 0, 0b010, 0b100],
        '.' => [0, 0, 0, 0, 0b010],
        '-' => [0, 0, 0b111, 0, 0],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '(' => [0b001, 0b010, 0b010, 0b010, 0b001],
        ')' => [0b100, 0b010, 0b010, 0b010, 0b100],
        ' ' => [0; 5],
        _ => [0b111, 0b001, 0b010, 0, 0b010],
    }
}

pub(crate) fn text_width(text: &str) -> u32 {
    text.chars().count() as u32 * 4 * FONT_SCALE
}

pub(crate) fn draw_text(image: &mut RgbaImage, x: u32, y: u32, text: &str) {
    let [r, g, b] = PALETTE[63];
    for (char_index, ch) in text.chars().enumerate() {
        let rows = glyph(ch);
        let char_x = x + char_index as u32 * 4 * FONT_SCALE;
        for (row, bits) in rows.into_iter().enumerate() {
            for col in 0..3 {
                if bits & (1 << (2 - col)) == 0 {
                    continue;
                }
                for dy in 0..FONT_SCALE {
                    for dx in 0..FONT_SCALE {
                        let px = char_x + col * FONT_SCALE + dx;
                        let py = y + row as u32 * FONT_SCALE + dy;
                        if px < image.width && py < image.height {
                            let offset = ((py * image.width + px) * 4) as usize;
                            image.rgba[offset..offset + 4].copy_from_slice(&[r, g, b, 255]);
                        }
                    }
                }
            }
        }
    }
}

pub struct BoardSpec<'a> {
    pub stage: &'a str,
    pub frames: &'a [RgbaImage],
    pub frame_numbers: &'a [u64],
    pub crop: Rect,
    pub zoom: u32,
    pub columns: u32,
    pub reference: Option<&'a RgbaImage>,
}

struct BoardGeometry {
    width: u32,
    height: u32,
    panel_w: u32,
    panel_h: u32,
    columns: u32,
    runtime_y: u32,
    runtime_h: u32,
    header: String,
    frame_labels: Vec<String>,
    reference_label: Option<String>,
}

fn board_geometry(
    stage: &str,
    frame_numbers: &[u64],
    crop: Rect,
    zoom: u32,
    columns: u32,
    reference_dimensions: Option<(u32, u32)>,
) -> Result<BoardGeometry, String> {
    if frame_numbers.is_empty() {
        return Err("sequence has no sampled frames".to_string());
    }
    if columns == 0 {
        return Err("review board columns must be >= 1".to_string());
    }
    let image_w = checked_mul(crop.w, zoom, "review image width")?;
    let image_h = checked_mul(crop.h, zoom, "review image height")?;
    let frame_labels: Vec<String> = frame_numbers
        .iter()
        .map(|frame| format!("FRAME:{frame}"))
        .collect();
    let panel_w = frame_labels
        .iter()
        .map(|label| text_width(label))
        .fold(image_w, u32::max);
    let panel_h = checked_add(LABEL_H, image_h, "review panel height")?;
    let count = u32::try_from(frame_numbers.len()).map_err(|_| "too many review frames")?;
    let columns = columns.min(count);
    let rows = count.div_ceil(columns);
    let runtime_w = checked_add(
        checked_mul(panel_w, columns, "review runtime width")?,
        checked_mul(GUTTER, columns.saturating_sub(1), "review runtime gutters")?,
        "review runtime width",
    )?;
    let runtime_h = checked_add(
        checked_mul(panel_h, rows, "review runtime height")?,
        checked_mul(GUTTER, rows.saturating_sub(1), "review runtime gutters")?,
        "review runtime height",
    )?;
    let header = format!(
        "STAGE:{} CROP:{},{},{}X{} RUNTIME:NN {}X",
        stage, crop.x, crop.y, crop.w, crop.h, zoom
    );
    let reference_label = reference_dimensions
        .map(|(width, height)| format!("REFERENCE:NATIVE {width}X{height} NOT PIXEL-ALIGNED"));
    let content_w = runtime_w
        .max(text_width(&header))
        .max(reference_label.as_deref().map_or(0, text_width))
        .max(reference_dimensions.map_or(0, |(width, _)| width));
    let runtime_y = checked_add(PAD, LABEL_H, "review runtime y")?;
    let mut height = checked_add(runtime_y, runtime_h, "review height")?;
    if let Some((_, reference_height)) = reference_dimensions {
        height = checked_add(height, GUTTER, "review height")?;
        height = checked_add(height, LABEL_H, "review height")?;
        height = checked_add(height, reference_height, "review height")?;
    }
    height = checked_add(height, PAD, "review height")?;
    let width = checked_add(
        checked_add(PAD, content_w, "review width")?,
        PAD,
        "review width",
    )?;
    rgba_len(width, height)?;
    Ok(BoardGeometry {
        width,
        height,
        panel_w,
        panel_h,
        columns,
        runtime_y,
        runtime_h,
        header,
        frame_labels,
        reference_label,
    })
}

pub struct SequencePreflight<'a> {
    pub stage: &'a str,
    pub frame_numbers: &'a [u64],
    pub crop: Rect,
    pub zoom: u32,
    pub columns: u32,
    pub reference_dimensions: Option<(u32, u32)>,
    pub cadence_frames: u64,
    pub gif: bool,
    pub strip: bool,
    pub board: bool,
}

/// Validate all requested sequence output dimensions before the runtime is
/// stepped. This mirrors the encoders' exact layout math but allocates no
/// output canvas.
pub fn preflight_sequence(spec: &SequencePreflight<'_>) -> Result<(), String> {
    if spec.frame_numbers.is_empty() {
        return Err("sequence has no sampled frames".to_string());
    }
    let frame_w = checked_mul(spec.crop.w, spec.zoom, "sequence frame width")?;
    let frame_h = checked_mul(spec.crop.h, spec.zoom, "sequence frame height")?;
    rgba_len(frame_w, frame_h)?;
    if spec.gif && (frame_w > u16::MAX as u32 || frame_h > u16::MAX as u32) {
        return Err(format!(
            "sequence GIF dimensions {frame_w}x{frame_h} exceed 65535"
        ));
    }
    if spec.gif {
        aggregate_rgba_len(frame_w, frame_h, spec.frame_numbers.len(), "sequence GIF")?;
        // Validate the delay conversion too, including the GIF u16 limit.
        gif_delay_cs(spec.cadence_frames)?;
    }
    if spec.strip {
        let count = u32::try_from(spec.frame_numbers.len()).map_err(|_| "too many strip frames")?;
        let width = checked_add(
            checked_mul(frame_w, count, "strip width")?,
            checked_mul(GUTTER, count.saturating_sub(1), "strip gutters")?,
            "strip width",
        )?;
        rgba_len(width, frame_h)?;
    }
    if spec.board {
        board_geometry(
            spec.stage,
            spec.frame_numbers,
            spec.crop,
            spec.zoom,
            spec.columns,
            spec.reference_dimensions,
        )?;
    }
    Ok(())
}

pub fn review_board(spec: &BoardSpec<'_>) -> Result<(RgbaImage, BoardLayout), String> {
    let first = spec
        .frames
        .first()
        .ok_or("sequence has no sampled frames")?;
    if spec.frames.len() != spec.frame_numbers.len() {
        return Err("sequence frame-number count does not match images".to_string());
    }
    if first.width != spec.crop.w
        || first.height != spec.crop.h
        || spec
            .frames
            .iter()
            .any(|frame| frame.width != first.width || frame.height != first.height)
    {
        return Err("sequence frames do not match the declared crop dimensions".to_string());
    }
    let geometry = board_geometry(
        spec.stage,
        spec.frame_numbers,
        spec.crop,
        spec.zoom,
        spec.columns,
        spec.reference
            .map(|reference| (reference.width, reference.height)),
    )?;
    let mut board = blank(geometry.width, geometry.height)?;
    draw_text(&mut board, PAD, PAD, &geometry.header);

    let mut panels = Vec::with_capacity(spec.frames.len());
    for (index, frame) in spec.frames.iter().enumerate() {
        let col = index as u32 % geometry.columns;
        let row = index as u32 / geometry.columns;
        let cell_x = PAD + col * (geometry.panel_w + GUTTER);
        let cell_y = geometry.runtime_y + row * (geometry.panel_h + GUTTER);
        draw_text(&mut board, cell_x, cell_y, &geometry.frame_labels[index]);
        let image = frame.zoom(spec.zoom)?;
        let image_x = cell_x + (geometry.panel_w - image.width) / 2;
        let image_y = cell_y + LABEL_H;
        blit(&mut board, &image, image_x, image_y)?;
        panels.push(PanelRect {
            x: image_x,
            y: image_y,
            w: image.width,
            h: image.height,
        });
    }

    let reference_panel =
        if let (Some(reference), Some(label)) = (spec.reference, geometry.reference_label) {
            let label_y = geometry.runtime_y + geometry.runtime_h + GUTTER;
            draw_text(&mut board, PAD, label_y, &label);
            let image_y = label_y + LABEL_H;
            blit(&mut board, reference, PAD, image_y)?;
            Some(PanelRect {
                x: PAD,
                y: image_y,
                w: reference.width,
                h: reference.height,
            })
        } else {
            None
        };

    Ok((
        board,
        BoardLayout {
            runtime_panels: panels,
            reference_panel,
        },
    ))
}

// ---------------------------------------------------------------------------
// Scenario-level visual diagnostics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticView {
    Color,
    Grayscale,
    LumaBands,
    Edges,
    PaletteIndex,
}

impl DiagnosticView {
    pub fn label(self) -> &'static str {
        match self {
            DiagnosticView::Color => "COLOR",
            DiagnosticView::Grayscale => "GRAYSCALE",
            DiagnosticView::LumaBands => "LUMA BANDS",
            DiagnosticView::Edges => "EDGES",
            DiagnosticView::PaletteIndex => "PALETTE INDEX",
        }
    }
}

pub fn default_diagnostic_views() -> Vec<DiagnosticView> {
    vec![
        DiagnosticView::Color,
        DiagnosticView::Grayscale,
        DiagnosticView::LumaBands,
        DiagnosticView::Edges,
        DiagnosticView::PaletteIndex,
    ]
}

#[derive(Debug, Clone)]
pub struct DiagnosticSource {
    pub label: String,
    pub image: RgbaImage,
    /// Presented Apollo64 indices, one per pixel. 255 means transparent.
    /// `None` makes diagnostics use a deterministic nearest-palette mapping.
    pub palette_indices: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct DiagnosticBoardSpec<'a> {
    pub sources: &'a [DiagnosticSource],
    pub views: &'a [DiagnosticView],
    pub zoom: u32,
    pub columns: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct DiagnosticSourcePreflight<'a> {
    pub label: &'a str,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct DiagnosticBoardPreflight<'a> {
    pub sources: &'a [DiagnosticSourcePreflight<'a>],
    pub views: &'a [DiagnosticView],
    pub zoom: u32,
    pub columns: u32,
}

#[derive(Debug, Clone, Copy)]
struct DiagnosticGeometry {
    panel_count: usize,
    columns: u32,
    panel_w: u32,
    panel_h: u32,
    grid_y: u32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticPanel {
    pub source: String,
    pub view: DiagnosticView,
    pub rect: PanelRect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaletteCount {
    pub index: u8,
    pub pixels: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetailGridMetrics {
    pub cell_size: u32,
    pub cells: u64,
    pub empty_cells: u64,
    pub sparse_cells: u64,
    pub dense_cells: u64,
    pub max_edges_in_cell: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticMetrics {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub palette_basis: &'static str,
    pub opaque_pixels: u64,
    pub transparent_pixels: u64,
    pub bright_pixels: u64,
    pub luma_bands: [u64; 4],
    pub edge_pixels: u64,
    pub detail_grid: DetailGridMetrics,
    pub palette_histogram: Vec<PaletteCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticBoardLayout {
    pub width: u32,
    pub height: u32,
    pub zoom: u32,
    pub columns: u32,
    pub panels: Vec<DiagnosticPanel>,
}

fn luma(pixel: &[u8]) -> u8 {
    ((u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29 + 128) >> 8)
        as u8
}

fn source_lumas(image: &RgbaImage) -> Vec<u8> {
    image
        .rgba
        .chunks_exact(4)
        .map(|pixel| if pixel[3] == 0 { 0 } else { luma(pixel) })
        .collect()
}

fn nearest_palette_index(pixel: &[u8]) -> u8 {
    PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, color)| {
            color
                .iter()
                .zip(pixel)
                .map(|(&a, &b)| {
                    let delta = i32::from(a) - i32::from(b);
                    (delta * delta) as u32
                })
                .sum::<u32>()
        })
        .map_or(0, |(index, _)| index as u8)
}

fn source_palette_indices(source: &DiagnosticSource) -> Result<Vec<u8>, String> {
    let pixels = (source.image.width as usize) * (source.image.height as usize);
    if let Some(indices) = &source.palette_indices {
        if indices.len() != pixels {
            return Err(format!(
                "diagnostic source {:?} has {} palette indices for {pixels} pixels",
                source.label,
                indices.len()
            ));
        }
        if let Some(index) = indices.iter().find(|&&index| index > 63 && index != 255) {
            return Err(format!(
                "diagnostic source {:?} has invalid palette index {index}",
                source.label
            ));
        }
        return Ok(indices.clone());
    }
    Ok(source
        .image
        .rgba
        .chunks_exact(4)
        .map(|pixel| {
            if pixel[3] == 0 {
                255
            } else {
                nearest_palette_index(pixel)
            }
        })
        .collect())
}

fn edge_mask(image: &RgbaImage, lumas: &[u8]) -> Vec<bool> {
    let mut edges = vec![false; lumas.len()];
    for y in 0..image.height as usize {
        for x in 0..image.width as usize {
            let index = y * image.width as usize + x;
            if image.rgba[index * 4 + 3] == 0 {
                continue;
            }
            let current = lumas[index];
            let right = if x + 1 < image.width as usize {
                lumas[index + 1]
            } else {
                current
            };
            let left = if x > 0 { lumas[index - 1] } else { current };
            let down = if y + 1 < image.height as usize {
                lumas[index + image.width as usize]
            } else {
                current
            };
            let up = if y > 0 {
                lumas[index - image.width as usize]
            } else {
                current
            };
            edges[index] = [left, right, up, down]
                .into_iter()
                .map(|neighbor| current.abs_diff(neighbor))
                .max()
                .unwrap_or(0)
                >= 24;
        }
    }
    edges
}

fn diagnostic_image(
    source: &DiagnosticSource,
    view: DiagnosticView,
    lumas: &[u8],
    edges: &[bool],
    indices: &[u8],
) -> Result<RgbaImage, String> {
    if view == DiagnosticView::Color {
        return Ok(source.image.clone());
    }
    let mut rgba = vec![0; rgba_len(source.image.width, source.image.height)?];
    for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        let alpha = source.image.rgba[index * 4 + 3];
        if alpha == 0 {
            continue;
        }
        let rgb = match view {
            DiagnosticView::Color => unreachable!(),
            DiagnosticView::Grayscale => [lumas[index]; 3],
            DiagnosticView::LumaBands => {
                let value = match lumas[index] {
                    0..=63 => 16,
                    64..=127 => 88,
                    128..=191 => 168,
                    _ => 240,
                };
                [value; 3]
            }
            DiagnosticView::Edges => {
                if edges[index] {
                    [255; 3]
                } else {
                    [0; 3]
                }
            }
            DiagnosticView::PaletteIndex => {
                let palette_index = indices[index];
                if palette_index == 255 {
                    continue;
                }
                // A stable permutation makes adjacent source indices visibly
                // distinct instead of merely reproducing the color view.
                PALETTE[((usize::from(palette_index) * 37 + 11) % 63) + 1]
            }
        };
        pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
    }
    RgbaImage::new(source.image.width, source.image.height, rgba)
}

fn analyze_source(
    source: &DiagnosticSource,
    lumas: &[u8],
    edges: &[bool],
    indices: &[u8],
) -> DiagnosticMetrics {
    let mut opaque_pixels = 0u64;
    let mut transparent_pixels = 0u64;
    let mut bright_pixels = 0u64;
    let mut luma_bands = [0u64; 4];
    let mut palette = [0u64; 64];
    for (index, pixel) in source.image.rgba.chunks_exact(4).enumerate() {
        if pixel[3] == 0 {
            transparent_pixels += 1;
            continue;
        }
        opaque_pixels += 1;
        bright_pixels += u64::from(lumas[index] >= 192);
        luma_bands[usize::from(lumas[index] / 64)] += 1;
        if indices[index] != 255 {
            palette[usize::from(indices[index])] += 1;
        }
    }

    const CELL: usize = 8;
    let cell_columns = (source.image.width as usize).div_ceil(CELL);
    let cell_rows = (source.image.height as usize).div_ceil(CELL);
    let mut empty_cells = 0u64;
    let mut sparse_cells = 0u64;
    let mut dense_cells = 0u64;
    let mut max_edges_in_cell = 0u32;
    for cy in 0..cell_rows {
        for cx in 0..cell_columns {
            let mut opaque = 0u32;
            let mut edge_count = 0u32;
            for y in cy * CELL..((cy + 1) * CELL).min(source.image.height as usize) {
                for x in cx * CELL..((cx + 1) * CELL).min(source.image.width as usize) {
                    let index = y * source.image.width as usize + x;
                    opaque += u32::from(source.image.rgba[index * 4 + 3] != 0);
                    edge_count += u32::from(edges[index]);
                }
            }
            empty_cells += u64::from(opaque == 0);
            sparse_cells += u64::from(opaque != 0 && edge_count <= 1);
            dense_cells += u64::from(edge_count >= 16);
            max_edges_in_cell = max_edges_in_cell.max(edge_count);
        }
    }

    DiagnosticMetrics {
        source: source.label.clone(),
        width: source.image.width,
        height: source.image.height,
        palette_basis: if source.palette_indices.is_some() {
            "exact"
        } else {
            "nearest_apollo64"
        },
        opaque_pixels,
        transparent_pixels,
        bright_pixels,
        luma_bands,
        edge_pixels: edges.iter().filter(|&&edge| edge).count() as u64,
        detail_grid: DetailGridMetrics {
            cell_size: CELL as u32,
            cells: (cell_columns * cell_rows) as u64,
            empty_cells,
            sparse_cells,
            dense_cells,
            max_edges_in_cell,
        },
        palette_histogram: palette
            .into_iter()
            .enumerate()
            .filter_map(|(index, pixels)| {
                (pixels != 0).then_some(PaletteCount {
                    index: index as u8,
                    pixels,
                })
            })
            .collect(),
    }
}

fn diagnostic_geometry(spec: &DiagnosticBoardPreflight<'_>) -> Result<DiagnosticGeometry, String> {
    if spec.sources.is_empty() {
        return Err("visual diagnostics have no sources".to_string());
    }
    if spec.views.is_empty() {
        return Err("visual diagnostics have no views".to_string());
    }
    if spec.zoom == 0 || spec.columns == 0 {
        return Err("visual diagnostic zoom and columns must be >= 1".to_string());
    }
    let panel_count = spec
        .sources
        .len()
        .checked_mul(spec.views.len())
        .ok_or("visual diagnostic panel count overflows")?;
    if panel_count > 120 {
        return Err(format!(
            "visual diagnostics request {panel_count} panels; at most 120 are allowed"
        ));
    }

    let mut aggregate_bytes = 0usize;
    let mut panel_w = 1u32;
    let mut image_h = 1u32;
    for source in spec.sources {
        if source.width == 0 || source.height == 0 {
            return Err(format!(
                "visual diagnostic source {:?} has zero width or height",
                source.label
            ));
        }
        let width = checked_mul(source.width, spec.zoom, "diagnostic panel width")?;
        let height = checked_mul(source.height, spec.zoom, "diagnostic panel height")?;
        let panel_bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or("visual diagnostic panel RGBA size overflows")?;
        image_h = image_h.max(height);
        for view in spec.views {
            aggregate_bytes = aggregate_bytes
                .checked_add(panel_bytes)
                .ok_or("visual diagnostic aggregate work overflows")?;
            if aggregate_bytes > MAX_VISUAL_RGBA_BYTES {
                return Err(format!(
                    "visual diagnostic panels need {aggregate_bytes} RGBA bytes, exceeding the {MAX_VISUAL_RGBA_BYTES} byte limit"
                ));
            }
            panel_w =
                panel_w
                    .max(width)
                    .max(text_width(&format!("{} / {}", source.label, view.label())));
        }
    }

    let columns = spec.columns.min(panel_count as u32);
    let rows = (panel_count as u32).div_ceil(columns);
    let panel_h = checked_add(LABEL_H, image_h, "diagnostic panel height")?;
    let content_w = checked_add(
        checked_mul(panel_w, columns, "diagnostic board width")?,
        checked_mul(GUTTER, columns.saturating_sub(1), "diagnostic gutters")?,
        "diagnostic board width",
    )?;
    let header = "VISUAL DIAGNOSTICS / EVIDENCE ONLY / NO AESTHETIC SCORE";
    let width = checked_add(
        PAD,
        checked_add(content_w.max(text_width(header)), PAD, "diagnostic width")?,
        "diagnostic width",
    )?;
    let grid_y = checked_add(PAD, LABEL_H, "diagnostic grid y")?;
    let grid_h = checked_add(
        checked_mul(panel_h, rows, "diagnostic board height")?,
        checked_mul(GUTTER, rows.saturating_sub(1), "diagnostic gutters")?,
        "diagnostic board height",
    )?;
    let height = checked_add(
        grid_y,
        checked_add(grid_h, PAD, "diagnostic height")?,
        "diagnostic height",
    )?;
    rgba_len(width, height)
        .map_err(|error| format!("visual diagnostic board is invalid: {error}"))?;
    Ok(DiagnosticGeometry {
        panel_count,
        columns,
        panel_w,
        panel_h,
        grid_y,
        width,
        height,
    })
}

pub fn preflight_diagnostic_board(spec: &DiagnosticBoardPreflight<'_>) -> Result<(), String> {
    diagnostic_geometry(spec).map(|_| ())
}

pub fn diagnostic_board(
    spec: &DiagnosticBoardSpec<'_>,
) -> Result<(RgbaImage, DiagnosticBoardLayout, Vec<DiagnosticMetrics>), String> {
    let source_preflight = spec
        .sources
        .iter()
        .map(|source| DiagnosticSourcePreflight {
            label: &source.label,
            width: source.image.width,
            height: source.image.height,
        })
        .collect::<Vec<_>>();
    let geometry = diagnostic_geometry(&DiagnosticBoardPreflight {
        sources: &source_preflight,
        views: spec.views,
        zoom: spec.zoom,
        columns: spec.columns,
    })?;

    let mut rendered = Vec::with_capacity(geometry.panel_count);
    let mut metrics = Vec::with_capacity(spec.sources.len());
    let mut aggregate_bytes = 0usize;
    for source in spec.sources {
        let lumas = source_lumas(&source.image);
        let edges = edge_mask(&source.image, &lumas);
        let indices = source_palette_indices(source)?;
        metrics.push(analyze_source(source, &lumas, &edges, &indices));
        for &view in spec.views {
            let image =
                diagnostic_image(source, view, &lumas, &edges, &indices)?.zoom(spec.zoom)?;
            aggregate_bytes = aggregate_bytes
                .checked_add(image.rgba.len())
                .ok_or("visual diagnostic aggregate work overflows")?;
            if aggregate_bytes > MAX_VISUAL_RGBA_BYTES {
                return Err(format!(
                    "visual diagnostic panels need {aggregate_bytes} RGBA bytes, exceeding the {MAX_VISUAL_RGBA_BYTES} byte limit"
                ));
            }
            rendered.push((source.label.clone(), view, image));
        }
    }

    let header = "VISUAL DIAGNOSTICS / EVIDENCE ONLY / NO AESTHETIC SCORE";
    let mut board = blank(geometry.width, geometry.height)?;
    draw_text(&mut board, PAD, PAD, header);
    let mut panels = Vec::with_capacity(geometry.panel_count);
    for (index, (source, view, image)) in rendered.iter().enumerate() {
        let col = index as u32 % geometry.columns;
        let row = index as u32 / geometry.columns;
        let cell_x = PAD + col * (geometry.panel_w + GUTTER);
        let cell_y = geometry.grid_y + row * (geometry.panel_h + GUTTER);
        draw_text(
            &mut board,
            cell_x,
            cell_y,
            &format!("{source} / {}", view.label()),
        );
        let image_x = cell_x + (geometry.panel_w - image.width) / 2;
        let image_y = cell_y + LABEL_H;
        blit_over(&mut board, image, image_x, image_y)?;
        panels.push(DiagnosticPanel {
            source: source.clone(),
            view: *view,
            rect: PanelRect {
                x: image_x,
                y: image_y,
                w: image.width,
                h: image.height,
            },
        });
    }
    Ok((
        board,
        DiagnosticBoardLayout {
            width: geometry.width,
            height: geometry.height,
            zoom: spec.zoom,
            columns: geometry.columns,
            panels,
        },
        metrics,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_then_zoom_is_exact_nearest_neighbor() {
        let source = RgbaImage::new(
            3,
            2,
            vec![
                1, 2, 3, 4, 10, 20, 30, 40, 50, 60, 70, 80, 5, 6, 7, 8, 90, 100, 110, 120, 130,
                140, 150, 160,
            ],
        )
        .unwrap();
        let zoomed = source
            .crop(Rect {
                x: 1,
                y: 0,
                w: 1,
                h: 2,
            })
            .unwrap()
            .zoom(2)
            .unwrap();
        assert_eq!((zoomed.width, zoomed.height), (2, 4));
        assert_eq!(
            zoomed.rgba,
            vec![
                10, 20, 30, 40, 10, 20, 30, 40, 10, 20, 30, 40, 10, 20, 30, 40, 90, 100, 110, 120,
                90, 100, 110, 120, 90, 100, 110, 120, 90, 100, 110, 120,
            ]
        );
    }

    #[test]
    fn gif_delay_rounds_sixty_hz_samples_to_centiseconds() {
        assert_eq!(gif_delay_cs(1).unwrap(), 2);
        assert_eq!(gif_delay_cs(2).unwrap(), 3);
        assert_eq!(gif_delay_cs(3).unwrap(), 5);
        assert_eq!(gif_delay_cs(6).unwrap(), 10);
    }

    #[test]
    fn preflight_rejects_oversized_strip_without_allocating_it() {
        let frame_numbers = (1..=240).collect::<Vec<_>>();
        let error = preflight_sequence(&SequencePreflight {
            stage: "huge",
            frame_numbers: &frame_numbers,
            crop: Rect {
                x: 0,
                y: 0,
                w: 192,
                h: 320,
            },
            zoom: 2,
            columns: 4,
            reference_dimensions: None,
            cadence_frames: 1,
            gif: false,
            strip: true,
            board: false,
        })
        .unwrap_err();
        assert!(error.contains("exceeding the 67108864 byte limit"));
    }

    #[test]
    fn preflight_and_encoder_both_bound_aggregate_gif_work() {
        let frame_numbers = (1..=240).collect::<Vec<_>>();
        let error = preflight_sequence(&SequencePreflight {
            stage: "huge",
            frame_numbers: &frame_numbers,
            crop: Rect {
                x: 0,
                y: 0,
                w: 192,
                h: 320,
            },
            zoom: 16,
            columns: 4,
            reference_dimensions: None,
            cadence_frames: 1,
            gif: true,
            strip: false,
            board: false,
        })
        .unwrap_err();
        assert!(error.contains("sequence GIF aggregate RGBA work"));
        assert!(error.contains("exceeding the 67108864 byte limit"));

        let native = RgbaImage::new(192, 320, vec![0; 192 * 320 * 4]).unwrap();
        let error = animated_gif(&[native.clone(), native], 16, 1).unwrap_err();
        assert!(error.contains("sequence GIF aggregate RGBA work"));
    }

    #[test]
    fn diagnostic_board_is_deterministic_and_reports_evidence_not_a_score() {
        let source = DiagnosticSource {
            label: "LANDING F42".to_string(),
            image: RgbaImage::new(
                2,
                2,
                vec![
                    0, 0, 0, 255, 255, 255, 255, 255, 40, 50, 60, 255, 0, 0, 0, 0,
                ],
            )
            .unwrap(),
            palette_indices: Some(vec![0, 63, 7, 255]),
        };
        let views = default_diagnostic_views();
        let build = || {
            diagnostic_board(&DiagnosticBoardSpec {
                sources: std::slice::from_ref(&source),
                views: &views,
                zoom: 2,
                columns: 5,
            })
            .unwrap()
        };
        let (first, layout, metrics) = build();
        let (second, _, _) = build();
        assert_eq!(first.png(), second.png());
        assert_eq!(layout.panels.len(), 5);
        assert_eq!(metrics[0].opaque_pixels, 3);
        assert_eq!(metrics[0].transparent_pixels, 1);
        assert_eq!(metrics[0].bright_pixels, 1);
        assert_eq!(metrics[0].palette_histogram.len(), 3);

        let report = serde_json::to_value(&metrics).unwrap();
        assert!(report[0].get("edge_pixels").is_some());
        assert!(report[0].get("score").is_none());
    }

    #[test]
    fn diagnostic_board_bounds_panel_count_before_rendering() {
        let source = DiagnosticSource {
            label: "FRAME".to_string(),
            image: RgbaImage::new(1, 1, vec![0, 0, 0, 255]).unwrap(),
            palette_indices: Some(vec![0]),
        };
        let sources = vec![source; 25];
        let error = diagnostic_board(&DiagnosticBoardSpec {
            sources: &sources,
            views: &default_diagnostic_views(),
            zoom: 1,
            columns: 5,
        })
        .unwrap_err();
        assert!(error.contains("125 panels; at most 120"));
    }

    #[test]
    fn diagnostic_preflight_bounds_derived_panels_and_final_board() {
        let map = [DiagnosticSourcePreflight {
            label: "MAP LIVE @ scene",
            width: 1024,
            height: 512,
        }];
        let views = default_diagnostic_views();
        let error = preflight_diagnostic_board(&DiagnosticBoardPreflight {
            sources: &map,
            views: &views,
            zoom: 4,
            columns: 5,
        })
        .unwrap_err();
        assert!(error.contains("visual diagnostic panels need"));
        assert!(error.contains("exceeding the 67108864 byte limit"));

        let labels = (0..24)
            .map(|index| format!("{index:02}-{}", "X".repeat(61)))
            .collect::<Vec<_>>();
        let ordinary = labels
            .iter()
            .map(|label| DiagnosticSourcePreflight {
                label,
                width: 192,
                height: 320,
            })
            .collect::<Vec<_>>();
        let error = preflight_diagnostic_board(&DiagnosticBoardPreflight {
            sources: &ordinary,
            views: &views,
            zoom: 1,
            columns: 8,
        })
        .unwrap_err();
        assert!(error.contains("visual diagnostic board is invalid"));
        assert!(error.contains("exceeding the 67108864 byte limit"));
    }
}

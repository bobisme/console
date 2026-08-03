//! `music piano-roll` — the vision layer for music, at *score* level.
//!
//! The existing `spectrogram` is a signal-level picture: it shows what came
//! out of the mixer, harmonics and all, and it cannot tell you which channel
//! played what. A piano roll is the complement — it draws the cart's own
//! `__sfx__` rows, so every note is exactly where the author put it, colored
//! by the channel that plays it. Reading the two together is how you find out
//! that the note you wrote is not the note you heard.
//!
//! Layout: **x = time in frames** (not rows: slots may run at different
//! speeds, and a note's *length* is worth seeing), **y = semitone**, cropped
//! to the notes the song actually uses so a two-octave bassline does not draw
//! eight octaves of empty space. Channel colors come from the console's own
//! Sweetie-16 palette, note brightness is velocity, C boundaries are
//! horizontal gridlines with the octave number in the gutter, pattern
//! boundaries are vertical gridlines, and the loop point is a bright vertical
//! bar — so the song's form reads off the picture exactly as it reads off
//! `music score`'s chain line.

use console_core::{CHANNEL_COUNT, Cart, PALETTE, SfxRow};

use crate::sprite::view::{self, Canvas, Image, Rect};

use super::{cart_arg, parse_flags, parse_pattern_list, plan_song};

/// Default device pixels per frame on the x axis.
pub const DEFAULT_CELL: u32 = 2;
/// Default device pixels per semitone on the y axis.
pub const DEFAULT_ROW_H: u32 = 5;
/// Gutter reserved on the left for octave-number labels.
pub const GUTTER: u32 = 6;
/// Semitones of padding above and below the used note range.
const PAD_SEMIS: i32 = 2;
const MAX_PIXELS: u32 = 16_000;

/// Palette index per channel, chosen so no two adjacent channels share a hue
/// and none of them is the background or a gridline.
pub const CHANNEL_COLORS: [usize; CHANNEL_COUNT] = [3, 5, 10, 4, 2, 11];
const BACKGROUND: usize = 0;
const GRID: usize = 15;
const OCTAVE_GRID: usize = 14;
const PATTERN_LINE: usize = 13;
const LOOP_LINE: usize = 12;

#[derive(Debug, Clone, Copy)]
pub struct RollOpts {
    /// Device pixels per frame.
    pub cell: u32,
    /// Device pixels per semitone.
    pub row_h: u32,
}

impl Default for RollOpts {
    fn default() -> RollOpts {
        RollOpts {
            cell: DEFAULT_CELL,
            row_h: DEFAULT_ROW_H,
        }
    }
}

/// One drawn note.
struct NoteBox {
    /// Frame the note starts on, measured from the start of the drawing.
    start: u32,
    /// Frames the note lasts (its row's `speed`, truncated at the pattern's
    /// end).
    len: u32,
    note: u8,
    vol: u8,
    channel: usize,
}

/// Draw `order`'s patterns end to end. `loop_at` is an index into `order`:
/// the pattern the song jumps back to, marked with a bright vertical bar.
pub fn piano_roll(
    cart: &Cart,
    order: &[u8],
    loop_at: Option<usize>,
    opts: &RollOpts,
) -> Result<Image, String> {
    if order.is_empty() {
        return Err("piano-roll needs at least one pattern".to_string());
    }
    let cell = opts.cell.max(1);
    let row_h = opts.row_h.max(1);

    // Lay the patterns out on one frame timeline and collect their notes.
    let mut notes: Vec<NoteBox> = Vec::new();
    let mut boundaries: Vec<u32> = Vec::new();
    let mut loop_frame: Option<u32> = None;
    let mut at = 0u32;
    for (i, &id) in order.iter().enumerate() {
        if loop_at == Some(i) {
            loop_frame = Some(at);
        }
        if i > 0 {
            boundaries.push(at);
        }
        let frames = super::pattern_frames(cart, id);
        let pat = cart
            .pattern(id)
            .ok_or_else(|| format!("cart has no pattern {id}"))?;
        for ch in 0..CHANNEL_COUNT {
            let Some(sid) = pat.slots[ch] else { continue };
            let Some(sfx) = cart.sfx(sid) else { continue };
            let speed = u32::from(sfx.speed);
            for (row, entry) in sfx.rows.iter().enumerate() {
                let start = row as u32 * speed;
                if start >= frames {
                    break; // the sequencer truncates a slot longer than its pattern
                }
                let SfxRow::Note { note, vol, .. } = entry else {
                    continue;
                };
                notes.push(NoteBox {
                    start: at + start,
                    len: speed.min(frames - start),
                    note: *note,
                    vol: *vol,
                    channel: ch,
                });
            }
        }
        at += frames;
    }
    let total_frames = at.max(1);

    if notes.is_empty() {
        return Err(format!(
            "pattern(s) {} contain no notes to draw (every row is a rest)",
            order
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }

    // Crop the semitone axis to the used range, padded, snapped outward to
    // whole semitones inside C0-B7.
    let lo_note = notes.iter().map(|n| i32::from(n.note)).min().unwrap_or(0);
    let hi_note = notes.iter().map(|n| i32::from(n.note)).max().unwrap_or(95);
    let lo = (lo_note - PAD_SEMIS).max(0);
    let hi = (hi_note + PAD_SEMIS).min(95);
    let span = (hi - lo + 1) as u32;

    let width = GUTTER + total_frames * cell;
    let height = span * row_h;
    if width > MAX_PIXELS || height > MAX_PIXELS {
        return Err(format!(
            "piano-roll would be {width}x{height} px (limit {MAX_PIXELS}); lower --cell/--row-h \
             or draw fewer patterns with --patterns"
        ));
    }

    let mut canvas = Canvas::new(width, height, 1);
    canvas.fill(
        Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        },
        PALETTE[BACKGROUND],
    );

    // Semitone gridlines: every C is a bright line, every other semitone a
    // faint one, so a note's pitch is countable off the picture.
    for n in lo..=hi {
        let y = y_of(n, hi, row_h);
        let color = if n % 12 == 0 { OCTAVE_GRID } else { GRID };
        canvas.fill(
            Rect {
                x: GUTTER,
                y,
                w: width - GUTTER,
                h: 1,
            },
            PALETTE[color],
        );
        if n % 12 == 0 && row_h >= 5 {
            // The octave number, in the gutter, using the sprite tools' 3x5
            // hex-digit glyphs (octaves are 0-7, so a digit is enough).
            draw_digit(&mut canvas, 1, y, (n / 12) as u8, PALETTE[OCTAVE_GRID]);
        }
    }

    // Pattern boundaries, then the loop bar on top of whichever one it lands
    // on.
    for b in &boundaries {
        let x = GUTTER + b * cell;
        canvas.fill(
            Rect {
                x,
                y: 0,
                w: 1,
                h: height,
            },
            PALETTE[PATTERN_LINE],
        );
    }
    if let Some(f) = loop_frame {
        let x = GUTTER + f * cell;
        canvas.fill(
            Rect {
                x,
                y: 0,
                w: cell.max(2),
                h: height,
            },
            PALETTE[LOOP_LINE],
        );
    }

    // Notes last, so nothing overdraws them.
    for n in &notes {
        if i32::from(n.note) < lo || i32::from(n.note) > hi {
            continue;
        }
        let y = y_of(i32::from(n.note), hi, row_h);
        let x = GUTTER + n.start * cell;
        // A one-pixel gap on both axes keeps a run of repeated notes at the
        // same pitch (a held chord, a hi-hat line) readable as separate
        // strikes instead of one long bar.
        let w = (n.len * cell).saturating_sub(1).max(1);
        let h = row_h.saturating_sub(1).max(1);
        canvas.fill(
            Rect { x, y, w, h },
            velocity_color(CHANNEL_COLORS[n.channel], n.vol),
        );
    }

    Ok(canvas.finish(order.len()))
}

/// Row `n`'s top edge. High notes at the top, matching the spectrogram and
/// every piano roll ever drawn.
fn y_of(n: i32, hi: i32, row_h: u32) -> u32 {
    (hi - n).max(0) as u32 * row_h
}

/// A channel's color at volume `vol`: full brightness at 7, a third of it at
/// 0, so a `fade`d tail or a quiet pad is visibly quiet without vanishing.
pub fn velocity_color(palette_index: usize, vol: u8) -> [u8; 3] {
    let t = 0.34 + 0.66 * f32::from(vol.min(7)) / 7.0;
    let c = PALETTE[palette_index];
    [
        (f32::from(c[0]) * t).round() as u8,
        (f32::from(c[1]) * t).round() as u8,
        (f32::from(c[2]) * t).round() as u8,
    ]
}

/// One 3x5 glyph at 1:1, from the sprite tools' hex font.
fn draw_digit(canvas: &mut Canvas, x: u32, y: u32, digit: u8, ink: [u8; 3]) {
    for (row, bits) in view::GLYPHS[usize::from(digit & 0x0f)].iter().enumerate() {
        for bit in 0..3u32 {
            if bits & (1 << (2 - bit)) != 0 {
                canvas.fill(
                    Rect {
                        x: x + bit,
                        y: y + row as u32,
                        w: 1,
                        h: 1,
                    },
                    ink,
                );
            }
        }
    }
}

/// Resolve the `--song` / `--patterns` pair into a pattern order plus the
/// loop index to mark. `--patterns` draws exactly what it lists, in order,
/// with no loop marker; `--song` follows the chain.
pub fn resolve_order(
    cart: &Cart,
    song: Option<u8>,
    patterns: Option<&str>,
) -> Result<(Vec<u8>, Option<usize>), String> {
    if let Some(list) = patterns {
        if song.is_some() {
            return Err("--song and --patterns are mutually exclusive".to_string());
        }
        return Ok((parse_pattern_list(list, cart)?, None));
    }
    let start = match song {
        Some(id) => id,
        None => super::default_song(cart)?,
    };
    let plan = plan_song(cart, start)?;
    Ok((plan.pattern_ids(), plan.loop_index))
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub fn cli_piano_roll(args: &[String]) -> Result<i32, String> {
    let flags = parse_flags(args)?;
    let path = cart_arg(&flags, "piano-roll")?;
    let out = flags
        .out
        .as_deref()
        .ok_or("music piano-roll requires -o <out.png>")?;
    let (_text, cart) = super::read_cart(&path)?;
    let (order, loop_at) = resolve_order(&cart, flags.song, flags.patterns.as_deref())?;
    let opts = RollOpts {
        cell: flags.cell.unwrap_or(DEFAULT_CELL),
        row_h: flags.row_h.unwrap_or(DEFAULT_ROW_H),
    };
    let image = piano_roll(&cart, &order, loop_at, &opts)?;
    crate::artifact::write(out, &image.png)?;
    println!(
        "wrote {out} ({}x{} px, pattern(s) {}{})",
        image.width,
        image.height,
        order
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(","),
        match loop_at {
            Some(i) => format!(", loop from pat {}", order[i]),
            None => String::new(),
        },
    );
    Ok(0)
}

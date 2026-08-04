//! Apollo64 palette interchange and deterministic PNG quantization.

use std::collections::BTreeSet;
use std::io::IsTerminal;

use console_core::PALETTE;
use serde_json::{Value, json};

pub const COMMANDS: &[&str] = &["show", "quantize"];
const MAX_DECODED_PNG_BYTES: usize = 64 * 1024 * 1024;

pub const PALETTE_USAGE: &str = "\
usage:
  console palette show [-o|--out out.png] [--cell N]
  console palette quantize <input.png> -o|--out <output.png> [--colors 1-63] [--alpha-threshold 0-255] [--dither none] [--format text|pretty|json]
  (quantization never resizes; opaque pixels use palette indices 1-63 and
   transparent pixels remain transparent/source color 0)";

#[derive(Debug, Clone)]
pub struct DecodedPng {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct QuantizedPng {
    pub rgba: Vec<u8>,
    pub indices: Vec<u8>,
    pub selected_indices: Vec<u8>,
    pub source_colors: usize,
    pub output_colors: usize,
    pub transparent_pixels: usize,
    pub mean_squared_error: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportFormat {
    Text,
    Pretty,
    Json,
}

pub fn cli_palette(args: &[String]) -> i32 {
    if super::help_requested(args) {
        println!("{PALETTE_USAGE}");
        return 0;
    }
    match args.first().map(String::as_str) {
        Some("show") => cli_show(&args[1..]),
        Some("quantize") => cli_quantize(&args[1..]),
        _ => {
            eprintln!("{PALETTE_USAGE}");
            2
        }
    }
}

fn cli_show(args: &[String]) -> i32 {
    let mut cell = 16u32;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cell" => match value(args, &mut i, "--cell").and_then(parse_u32) {
                Ok(v) if (1..=128).contains(&v) => cell = v,
                Ok(v) => return cli_error(format!("--cell must be 1-128, got {v}")),
                Err(e) => return cli_error(e),
            },
            "-o" | "--out" => match value(args, &mut i, "--out") {
                Ok(v) => out = Some(v.to_string()),
                Err(e) => return cli_error(e),
            },
            other => return cli_error(format!("unknown palette show argument {other:?}")),
        }
        i += 1;
    }
    let out = out.unwrap_or_else(|| "apollo64.png".to_string());
    let (rgba, width, height) = palette_grid(cell);
    match crate::artifact::write(&out, &encode_png_rgba(&rgba, width, height)) {
        Ok(()) => {
            println!("wrote {out} ({width}x{height}, 64 colors)");
            0
        }
        Err(e) => cli_error(e),
    }
}

fn cli_quantize(args: &[String]) -> i32 {
    let mut input = None;
    let mut out = None;
    let mut colors = 16usize;
    let mut alpha_threshold = 128u8;
    let mut format_flag = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => match value(args, &mut i, "--out") {
                Ok(v) => out = Some(v.to_string()),
                Err(e) => return cli_error(e),
            },
            "--colors" => match value(args, &mut i, "--colors").and_then(parse_usize) {
                Ok(v) if (1..=63).contains(&v) => colors = v,
                Ok(v) => return cli_error(format!("--colors must be 1-63, got {v}")),
                Err(e) => return cli_error(e),
            },
            "--alpha-threshold" => {
                match value(args, &mut i, "--alpha-threshold").and_then(parse_u8) {
                    Ok(v) => alpha_threshold = v,
                    Err(e) => return cli_error(e),
                }
            }
            "--dither" => match value(args, &mut i, "--dither") {
                Ok("none") => {}
                Ok(other) => {
                    return cli_error(format!(
                        "unsupported --dither {other:?}; only explicit 'none' is deterministic and supported"
                    ));
                }
                Err(e) => return cli_error(e),
            },
            "--format" => match value(args, &mut i, "--format") {
                Ok(v) => match parse_report_format(v, "--format") {
                    Ok(format) => format_flag = Some(format),
                    Err(e) => return cli_error(e),
                },
                Err(e) => return cli_error(e),
            },
            "--json" => format_flag = Some(ReportFormat::Json),
            other if other.starts_with('-') => {
                return cli_error(format!("unknown palette quantize flag {other:?}"));
            }
            other if input.is_none() => input = Some(other.to_string()),
            other => return cli_error(format!("unexpected argument {other:?}")),
        }
        i += 1;
    }
    let Some(input) = input else {
        return cli_error("palette quantize requires <input.png>".into());
    };
    let Some(out) = out else {
        return cli_error("palette quantize requires -o <output.png>".into());
    };
    let format = match resolve_report_format(format_flag) {
        Ok(format) => format,
        Err(e) => return cli_error(e),
    };
    let bytes = match std::fs::read(&input) {
        Ok(bytes) => bytes,
        Err(e) => return cli_error(format!("cannot read {input:?}: {e}")),
    };
    let decoded = match decode_png_rgba(&bytes) {
        Ok(decoded) => decoded,
        Err(e) => return cli_error(format!("cannot decode {input:?}: {e}")),
    };
    let result = match quantize_rgba(&decoded.rgba, colors, alpha_threshold) {
        Ok(result) => result,
        Err(e) => return cli_error(e),
    };
    if let Err(e) = crate::artifact::write(
        &out,
        &encode_png_rgba(&result.rgba, decoded.width, decoded.height),
    ) {
        return cli_error(e);
    }
    let report = json!({
        "command": "palette quantize",
        "input": input,
        "output": out,
        "width": decoded.width,
        "height": decoded.height,
        "color_budget": colors,
        "source_colors": result.source_colors,
        "output_colors": result.output_colors,
        "selected_indices": result.selected_indices,
        "transparent_pixels": result.transparent_pixels,
        "mean_squared_error": round3(result.mean_squared_error),
        "dither": "none",
        "resized": false,
    });
    print_report(format, &report);
    0
}

pub fn decode_png_rgba(bytes: &[u8]) -> Result<DecodedPng, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let rgba_size = checked_rgba_len(reader.info().width, reader.info().height)?;
    if rgba_size > MAX_DECODED_PNG_BYTES {
        return Err(format!(
            "decoded RGBA PNG needs {rgba_size} bytes, exceeding the {} byte safety limit",
            MAX_DECODED_PNG_BYTES
        ));
    }
    let size = reader
        .output_buffer_size()
        .ok_or("decoded PNG dimensions exceed addressable memory")?;
    if size > MAX_DECODED_PNG_BYTES {
        return Err(format!(
            "decoded PNG needs {size} bytes, exceeding the {} byte safety limit",
            MAX_DECODED_PNG_BYTES
        ));
    }
    let mut buffer = vec![0; size];
    let info = reader.next_frame(&mut buffer).map_err(|e| e.to_string())?;
    let source = &buffer[..info.buffer_size()];
    let rgba_size = checked_rgba_len(info.width, info.height)?;
    let mut rgba = Vec::with_capacity(rgba_size);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(source),
        png::ColorType::Rgb => {
            for rgb in source.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for ga in source.chunks_exact(2) {
                rgba.extend_from_slice(&[ga[0], ga[0], ga[0], ga[1]]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in source {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::Indexed => {
            return Err("indexed PNG was not expanded by the decoder".into());
        }
    }
    if rgba.len() != rgba_size {
        return Err("decoded PNG buffer length does not match its dimensions".into());
    }
    Ok(DecodedPng {
        width: info.width,
        height: info.height,
        rgba,
    })
}

fn checked_rgba_len(width: u32, height: u32) -> Result<usize, String> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "decoded RGBA PNG dimensions overflow".into())
}

pub fn encode_png_rgba(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("in-memory PNG header");
        writer.write_image_data(rgba).expect("in-memory PNG data");
    }
    bytes
}

pub fn quantize_rgba(
    rgba: &[u8],
    color_budget: usize,
    alpha_threshold: u8,
) -> Result<QuantizedPng, String> {
    if rgba.len() % 4 != 0 {
        return Err("RGBA buffer length must be divisible by four".into());
    }
    if !(1..=63).contains(&color_budget) {
        return Err(format!("color budget must be 1-63, got {color_budget}"));
    }
    // One bit per possible 24-bit RGB keeps exact source-color counting bounded
    // to 2 MiB even when every input pixel has a distinct color.
    let mut seen_rgb = vec![0u8; 1 << 21];
    let mut source_colors = 0usize;
    let mut apollo_histogram = [0u64; 64];
    let mut transparent_pixels = 0usize;
    for px in rgba.chunks_exact(4) {
        if px[3] < alpha_threshold {
            transparent_pixels += 1;
        } else {
            let rgb = [px[0], px[1], px[2]];
            let key =
                (usize::from(rgb[0]) << 16) | (usize::from(rgb[1]) << 8) | usize::from(rgb[2]);
            let byte = key >> 3;
            let bit = 1u8 << (key & 7);
            if seen_rgb[byte] & bit == 0 {
                seen_rgb[byte] |= bit;
                source_colors += 1;
            }
            let index = nearest_opaque_index(rgb);
            apollo_histogram[index as usize] += 1;
        }
    }

    // First collapse arbitrary source RGBs onto the fixed Apollo64 candidates.
    // The subsequent budgeted selection then operates on at most 63 weighted
    // bins instead of rescanning every unique source color for every tentative
    // palette. This keeps generated and photographic inputs interactive while
    // retaining deterministic weighted-error selection.
    let occupied: Vec<u8> = (1u8..64)
        .filter(|index| apollo_histogram[*index as usize] != 0)
        .collect();
    let wanted = color_budget.min(occupied.len());
    let mut selected = Vec::<u8>::with_capacity(wanted);
    if occupied.len() <= color_budget {
        selected.extend_from_slice(&occupied);
    } else {
        let mut current_distances = [u32::MAX; 64];
        for _ in 0..wanted {
            let mut best = None::<(u128, u8)>;
            for candidate in 1u8..64 {
                if selected.contains(&candidate) {
                    continue;
                }
                let error = occupied.iter().fold(0u128, |sum, source| {
                    let distance = current_distances[*source as usize].min(color_distance(
                        PALETTE[*source as usize],
                        PALETTE[candidate as usize],
                    ));
                    sum + u128::from(distance) * u128::from(apollo_histogram[*source as usize])
                });
                if best.is_none_or(|prior| (error, candidate) < prior) {
                    best = Some((error, candidate));
                }
            }
            if let Some((_, index)) = best {
                selected.push(index);
                for source in &occupied {
                    current_distances[*source as usize] = current_distances[*source as usize].min(
                        color_distance(PALETTE[*source as usize], PALETTE[index as usize]),
                    );
                }
            }
        }
    }

    let mut output = Vec::with_capacity(rgba.len());
    let mut indices = Vec::with_capacity(rgba.len() / 4);
    let mut used = BTreeSet::new();
    let mut total_error = 0u128;
    let mut opaque = 0u128;
    for px in rgba.chunks_exact(4) {
        if px[3] < alpha_threshold {
            output.extend_from_slice(&[0, 0, 0, 0]);
            indices.push(0);
            continue;
        }
        let rgb = [px[0], px[1], px[2]];
        let index = nearest_index(rgb, &selected).ok_or("no opaque palette color selected")?;
        total_error += u128::from(color_distance(rgb, PALETTE[index as usize]));
        opaque += 1;
        used.insert(index);
        indices.push(index);
        output.extend_from_slice(&[
            PALETTE[index as usize][0],
            PALETTE[index as usize][1],
            PALETTE[index as usize][2],
            255,
        ]);
    }
    let mse = if opaque == 0 {
        0.0
    } else {
        total_error as f64 / opaque as f64 / 3.0
    };
    Ok(QuantizedPng {
        rgba: output,
        indices,
        selected_indices: selected,
        source_colors,
        output_colors: used.len(),
        transparent_pixels,
        mean_squared_error: mse,
    })
}

pub fn nearest_opaque_index(rgb: [u8; 3]) -> u8 {
    (1u8..64)
        .min_by_key(|index| (color_distance(rgb, PALETTE[*index as usize]), *index))
        .expect("Apollo64 has opaque colors")
}

fn nearest_index(rgb: [u8; 3], allowed: &[u8]) -> Option<u8> {
    allowed
        .iter()
        .copied()
        .min_by_key(|index| (color_distance(rgb, PALETTE[*index as usize]), *index))
}

fn color_distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.into_iter().zip(b).fold(0u32, |sum, (a, b)| {
        let d = i32::from(a) - i32::from(b);
        sum + (d * d) as u32
    })
}

fn palette_grid(cell: u32) -> (Vec<u8>, u32, u32) {
    let width = cell * 8;
    let height = cell * 8;
    let mut rgba = vec![255; (width * height * 4) as usize];
    for index in 0..64u32 {
        let sx = (index % 8) * cell;
        let sy = (index / 8) * cell;
        let rgb = PALETTE[index as usize];
        for y in sy..sy + cell {
            for x in sx..sx + cell {
                let at = ((y * width + x) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
    }
    (rgba, width, height)
}

fn value<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_u32(value: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("invalid integer {value:?}"))
}

fn parse_usize(value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("invalid integer {value:?}"))
}

fn parse_u8(value: &str) -> Result<u8, String> {
    value.parse().map_err(|_| format!("invalid byte {value:?}"))
}

fn cli_error(error: String) -> i32 {
    eprintln!("error: {error}");
    eprintln!("{PALETTE_USAGE}");
    2
}

pub(crate) fn parse_report_format(raw: &str, label: &str) -> Result<ReportFormat, String> {
    match raw {
        "text" => Ok(ReportFormat::Text),
        "pretty" => Ok(ReportFormat::Pretty),
        "json" => Ok(ReportFormat::Json),
        _ => Err(format!(
            "{label} must be text, pretty, or json, got {raw:?}"
        )),
    }
}

pub(crate) fn resolve_report_format(flag: Option<ReportFormat>) -> Result<ReportFormat, String> {
    if let Some(format) = flag {
        return Ok(format);
    }
    if let Ok(raw) = std::env::var("FORMAT") {
        return parse_report_format(&raw, "FORMAT");
    }
    Ok(if std::io::stdout().is_terminal() {
        ReportFormat::Pretty
    } else {
        ReportFormat::Text
    })
}

pub(crate) fn print_report(format: ReportFormat, report: &Value) {
    match format {
        ReportFormat::Json => println!("{}", serde_json::to_string(report).expect("JSON report")),
        ReportFormat::Pretty => println!(
            "{}",
            serde_json::to_string_pretty(report).expect("JSON report")
        ),
        ReportFormat::Text => {
            let command = report["command"].as_str().unwrap_or("palette");
            let width = report["width"].as_u64().unwrap_or(0);
            let height = report["height"].as_u64().unwrap_or(0);
            println!("{command}: {width}x{height}");
            for key in [
                "color_budget",
                "source_colors",
                "output_colors",
                "selected_indices",
                "palette_indices",
                "changed_pixels",
                "changed_rows",
                "partial_alpha_pixels",
                "transparent_pixels",
                "alpha_threshold",
                "mean_squared_error",
                "dry_run",
                "written",
            ] {
                if !report[key].is_null() {
                    println!("{key}={}", report[key]);
                }
            }
        }
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_is_deterministic_and_obeys_budget() {
        let rgba = [
            10, 20, 30, 255, 200, 220, 80, 255, 250, 60, 40, 255, 1, 2, 3, 0,
        ];
        let a = quantize_rgba(&rgba, 2, 128).unwrap();
        let b = quantize_rgba(&rgba, 2, 128).unwrap();
        assert_eq!(a.indices, b.indices);
        assert!(a.output_colors <= 2);
        assert_eq!(a.indices[3], 0);
        assert_eq!(&a.rgba[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn palette_grid_contains_every_exact_palette_color() {
        let (rgba, width, height) = palette_grid(1);
        assert_eq!((width, height), (8, 8));
        for (index, rgb) in PALETTE.iter().enumerate() {
            let at = index * 4;
            assert_eq!(&rgba[at..at + 3], rgb);
        }
    }

    #[test]
    fn rgba_safety_limit_applies_after_channel_expansion() {
        assert_eq!(checked_rgba_len(4096, 4096).unwrap(), 64 * 1024 * 1024);
        assert!(checked_rgba_len(4097, 4096).unwrap() > MAX_DECODED_PNG_BYTES);

        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 4097, 4096);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![0; 4097 * 4096]).unwrap();
        }
        let error = decode_png_rgba(&png).unwrap_err();
        assert!(error.contains("decoded RGBA PNG needs"), "{error}");
    }

    #[test]
    fn many_unique_colors_quantize_without_source_color_rescans() {
        let mut rgba = Vec::with_capacity(256 * 256 * 4);
        for y in 0..256u32 {
            for x in 0..256u32 {
                rgba.extend_from_slice(&[
                    x as u8,
                    y as u8,
                    (x.wrapping_mul(17) ^ y.wrapping_mul(29)) as u8,
                    255,
                ]);
            }
        }
        let result = quantize_rgba(&rgba, 16, 128).unwrap();
        assert_eq!(result.indices.len(), 256 * 256);
        assert!(result.output_colors <= 16);
    }
}

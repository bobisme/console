//! Exact-size PNG import/export for resolved sprite targets.

use std::collections::BTreeSet;

use console_core::{Cart, PALETTE, SHEET_W};
use serde_json::json;

use super::resolve_rect;
use super::transform::{EditResult, replace_region_values};
use crate::palette::{
    ReportFormat, decode_png_rgba, encode_png_rgba, nearest_opaque_index, parse_report_format,
    print_report, resolve_report_format,
};

const EXPORT_USAGE: &str =
    "usage:\n  console sprite export <cart> <target> [--frame N] [--palette source] -o out.png";
const IMPORT_USAGE: &str = "usage:\n  console sprite import <cart> <target> [--frame N] --input in.png [--mapping exact|nearest] [--alpha-threshold 0-255] [--max-colors 1-63] [--dry-run] [--format text|pretty|json]";

pub fn cli_export(args: &[String]) -> i32 {
    match run_export_cli(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("{EXPORT_USAGE}");
            2
        }
    }
}

fn run_export_cli(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut frame = 0u8;
    let mut out = None;
    let mut palette = "source";
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frame" => frame = parse_u8(value(args, &mut i, "--frame")?, "--frame")?,
            "-o" | "--out" => out = Some(value(args, &mut i, "--out")?.to_string()),
            "--palette" => palette = value(args, &mut i, "--palette")?,
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}")),
            other => positional.push(other.to_string()),
        }
        i += 1;
    }
    if positional.len() != 2 {
        return Err("sprite export requires <cart> and <target>".into());
    }
    if palette != "source" {
        return Err(format!(
            "--palette must be source, got {palette:?}; import/export round-trips source indices"
        ));
    }
    let out = out.ok_or("sprite export requires -o <out.png>")?;
    let text = std::fs::read_to_string(&positional[0])
        .map_err(|e| format!("cannot read {:?}: {e}", positional[0]))?;
    let cart = Cart::parse(&text).map_err(|e| e.to_string())?;
    let rect = resolve_rect(&cart, &positional[1], frame)?;
    let (rgba, width, height) = export_rgba(&cart, rect);
    crate::artifact::write(&out, &encode_png_rgba(&rgba, width, height))?;
    println!("wrote {out} ({width}x{height}, source palette)");
    Ok(())
}

pub fn cli_import(args: &[String]) -> i32 {
    match run_import_cli(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("{IMPORT_USAGE}");
            2
        }
    }
}

fn run_import_cli(args: &[String]) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut frame = 0u8;
    let mut input = None;
    let mut mapping = "exact";
    let mut alpha_threshold = 128u8;
    let mut max_colors = None;
    let mut dry_run = false;
    let mut format_flag = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--frame" => frame = parse_u8(value(args, &mut i, "--frame")?, "--frame")?,
            "--input" => input = Some(value(args, &mut i, "--input")?.to_string()),
            "--mapping" => mapping = value(args, &mut i, "--mapping")?,
            "--alpha-threshold" => {
                alpha_threshold = parse_u8(
                    value(args, &mut i, "--alpha-threshold")?,
                    "--alpha-threshold",
                )?
            }
            "--max-colors" => {
                let parsed = parse_u8(value(args, &mut i, "--max-colors")?, "--max-colors")?;
                if parsed == 0 || parsed > 63 {
                    return Err(format!("--max-colors must be 1-63, got {parsed}"));
                }
                max_colors = Some(parsed as usize);
            }
            "--dry-run" => dry_run = true,
            "--format" => {
                format_flag = Some(parse_report_format(
                    value(args, &mut i, "--format")?,
                    "--format",
                )?);
            }
            "--json" => format_flag = Some(ReportFormat::Json),
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}")),
            other => positional.push(other.to_string()),
        }
        i += 1;
    }
    if positional.len() != 2 {
        return Err("sprite import requires <cart> and <target>".into());
    }
    if !matches!(mapping, "exact" | "nearest") {
        return Err(format!(
            "--mapping must be exact or nearest, got {mapping:?}"
        ));
    }
    let input = input.ok_or("sprite import requires --input <in.png>")?;
    let format = resolve_report_format(format_flag)?;
    let text = std::fs::read_to_string(&positional[0])
        .map_err(|e| format!("cannot read {:?}: {e}", positional[0]))?;
    let cart = Cart::parse(&text).map_err(|e| e.to_string())?;
    let rect = resolve_rect(&cart, &positional[1], frame)?;
    let bytes = std::fs::read(&input).map_err(|e| format!("cannot read {input:?}: {e}"))?;
    let decoded = decode_png_rgba(&bytes).map_err(|e| format!("cannot decode {input:?}: {e}"))?;
    if (decoded.width, decoded.height) != (rect.2, rect.3) {
        return Err(format!(
            "PNG is {}x{}, but target {:?} frame {frame} is {}x{}; resize explicitly before import",
            decoded.width, decoded.height, positional[1], rect.2, rect.3
        ));
    }
    let (values, source_colors, partial_alpha) =
        map_import_pixels(&decoded.rgba, mapping, alpha_threshold)?;
    let colors: BTreeSet<u8> = values.iter().copied().filter(|value| *value != 0).collect();
    if let Some(limit) = max_colors {
        if colors.len() > limit {
            return Err(format!(
                "import uses {} nontransparent palette colors, exceeding --max-colors {limit}; quantize explicitly first",
                colors.len()
            ));
        }
    }
    let changed_pixels = changed_pixels(&cart, rect, &values);
    let edit = replace_region_values(&text, &cart, rect, &values)?;
    let (new_text, changed_rows) = match edit {
        EditResult::Unchanged => (None, Vec::new()),
        EditResult::Changed { new_text, report } => {
            let rows = report.iter().map(|(line, _)| *line).collect();
            (Some(new_text), rows)
        }
    };
    if !dry_run {
        if let Some(new_text) = &new_text {
            std::fs::write(&positional[0], new_text)
                .map_err(|e| format!("cannot write {:?}: {e}", positional[0]))?;
        }
    }
    let report = json!({
        "command": "sprite import",
        "cart": positional[0],
        "target": positional[1],
        "frame": frame,
        "input": input,
        "width": rect.2,
        "height": rect.3,
        "mapping": mapping,
        "alpha_threshold": alpha_threshold,
        "source_colors": source_colors,
        "output_colors": colors.len(),
        "palette_indices": colors,
        "partial_alpha_pixels": partial_alpha,
        "changed_pixels": changed_pixels,
        "changed_rows": changed_rows,
        "dry_run": dry_run,
        "written": !dry_run && new_text.is_some(),
        "resized": false,
    });
    print_report(format, &report);
    Ok(())
}

fn export_rgba(cart: &Cart, rect: (u32, u32, u32, u32)) -> (Vec<u8>, u32, u32) {
    let (x0, y0, width, height) = rect;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let index = cart.sprites()[(y0 + y) as usize * SHEET_W + (x0 + x) as usize];
            let rgb = PALETTE[index as usize];
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], if index == 0 { 0 } else { 255 }]);
        }
    }
    (rgba, width, height)
}

fn map_import_pixels(
    rgba: &[u8],
    mapping: &str,
    alpha_threshold: u8,
) -> Result<(Vec<u8>, usize, usize), String> {
    let mut values = Vec::with_capacity(rgba.len() / 4);
    let mut source_colors = BTreeSet::new();
    let mut partial_alpha = 0usize;
    for (offset, px) in rgba.chunks_exact(4).enumerate() {
        if px[3] < alpha_threshold {
            values.push(0);
            continue;
        }
        if px[3] != 255 {
            partial_alpha += 1;
        }
        let rgb = [px[0], px[1], px[2]];
        source_colors.insert(rgb);
        let index = if mapping == "exact" {
            PALETTE
                .iter()
                .position(|candidate| *candidate == rgb)
                .ok_or_else(|| {
                    format!(
                        "pixel {offset} has non-Apollo RGB #{:02x}{:02x}{:02x}; run console palette quantize or use --mapping nearest explicitly",
                        rgb[0], rgb[1], rgb[2]
                    )
                })? as u8
        } else {
            nearest_opaque_index(rgb)
        };
        if index == 0 {
            return Err(format!(
                "pixel {offset} is opaque Apollo index 0, which cannot be distinguished from sprite transparency; use transparent alpha or another Apollo color"
            ));
        }
        values.push(index);
    }
    Ok((values, source_colors.len(), partial_alpha))
}

fn changed_pixels(cart: &Cart, rect: (u32, u32, u32, u32), values: &[u8]) -> usize {
    let (x0, y0, width, height) = rect;
    let mut changed = 0;
    for y in 0..height {
        for x in 0..width {
            let old = cart.sprites()[(y0 + y) as usize * SHEET_W + (x0 + x) as usize];
            if old != values[(y * width + x) as usize] {
                changed += 1;
            }
        }
    }
    changed
}

fn value<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_u8(value: &str, flag: &str) -> Result<u8, String> {
    value
        .parse()
        .map_err(|_| format!("invalid {flag} value {value:?} (want 0-255)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_mapping_round_trips_transparency_and_apollo_colors() {
        let rgba = [
            0,
            0,
            0,
            0,
            PALETTE[14][0],
            PALETTE[14][1],
            PALETTE[14][2],
            255,
        ];
        let (values, colors, partial) = map_import_pixels(&rgba, "exact", 128).unwrap();
        assert_eq!(values, [0, 14]);
        assert_eq!(colors, 1);
        assert_eq!(partial, 0);
    }

    #[test]
    fn exact_mapping_rejects_arbitrary_rgb() {
        let error = map_import_pixels(&[1, 2, 3, 255], "exact", 128).unwrap_err();
        assert!(error.contains("non-Apollo"));
    }
}

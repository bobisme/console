//! Deterministic assembly of explicitly placed PNG assets into a sprite sheet.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use console_core::{SHEET_LEN, SHEET_W, color_char};

use super::{SpriteAssetConfig, SpriteAssetReport, SpriteMapping, resolve_input};
use crate::palette::{decode_png_rgba, quantize_rgba};
use crate::sprite::png_io::map_import_pixels;

const TILE_SIZE: u32 = 8;
const TILE_GRID: u32 = 16;
const DEFAULT_QUANTIZE_COLORS: usize = 16;

pub(super) struct AssembledSprites {
    pub sheet: String,
    pub gfx_meta: String,
    pub inputs: Vec<PathBuf>,
    pub assets: Vec<SpriteAssetReport>,
}

pub(super) fn assemble(
    project_root: &Path,
    configs: &[SpriteAssetConfig],
) -> Result<AssembledSprites, String> {
    let mut names = BTreeSet::new();
    for config in configs {
        validate_name(&config.name)?;
        if !names.insert(config.name.as_str()) {
            return Err(format!("duplicate [[sprites]] name {:?}", config.name));
        }
        if let Some(limit) = config.max_colors
            && !(1..=63).contains(&limit)
        {
            return Err(format!(
                "sprite {:?} max_colors must be 1-63, got {limit}",
                config.name
            ));
        }
    }

    let mut ordered = configs.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|config| &config.name);
    let mut sheet = [0u8; SHEET_LEN];
    let mut owners = vec![None::<String>; (TILE_GRID * TILE_GRID) as usize];
    let mut inputs = Vec::new();
    let mut reports = Vec::new();
    let mut metadata = Vec::new();

    for config in ordered {
        let source = resolve_input(
            project_root,
            &config.source,
            &format!("sprite {:?} source", config.name),
        )?;
        let bytes = std::fs::read(&source)
            .map_err(|error| format!("cannot read sprite PNG {}: {error}", source.display()))?;
        let decoded = decode_png_rgba(&bytes).map_err(|error| {
            format!(
                "cannot decode sprite {:?} PNG {}: {error}",
                config.name,
                source.display()
            )
        })?;
        validate_dimensions(config, decoded.width, decoded.height)?;
        let size_tiles = [decoded.width / TILE_SIZE, decoded.height / TILE_SIZE];
        claim_tiles(config, size_tiles, &mut owners)?;

        let partial_alpha_pixels = decoded
            .rgba
            .chunks_exact(4)
            .filter(|pixel| pixel[3] >= config.alpha_threshold && pixel[3] != 255)
            .count();
        let (indices, source_colors, mean_squared_error, color_budget) = match config.mapping {
            SpriteMapping::Exact | SpriteMapping::Nearest => {
                let (indices, source_colors, _) = map_import_pixels(
                    &decoded.rgba,
                    config.mapping.as_str(),
                    config.alpha_threshold,
                )
                .map_err(|error| format!("sprite {:?}: {error}", config.name))?;
                (indices, source_colors, None, config.max_colors)
            }
            SpriteMapping::Quantize => {
                let budget = config.max_colors.unwrap_or(DEFAULT_QUANTIZE_COLORS);
                let result = quantize_rgba(&decoded.rgba, budget, config.alpha_threshold)
                    .map_err(|error| format!("sprite {:?}: {error}", config.name))?;
                (
                    result.indices,
                    result.source_colors,
                    Some(result.mean_squared_error),
                    Some(budget),
                )
            }
        };
        let palette_indices = indices
            .iter()
            .copied()
            .filter(|index| *index != 0)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if let Some(limit) = config.max_colors
            && palette_indices.len() > limit
        {
            return Err(format!(
                "sprite {:?} uses {} nontransparent palette colors, exceeding max_colors {limit}; use mapping = \"quantize\" or raise the explicit budget",
                config.name,
                palette_indices.len()
            ));
        }
        place_pixels(config, decoded.width, decoded.height, &indices, &mut sheet);

        let anchor = config.anchor.unwrap_or([
            i32::try_from(decoded.width / 2).expect("sprite width fits i32"),
            i32::try_from(decoded.height - 1).expect("sprite height fits i32"),
        ]);
        metadata.push(format!(
            "sprite {} rect={},{} size={}x{} anchor={},{}",
            config.name,
            config.tile[0],
            config.tile[1],
            size_tiles[0],
            size_tiles[1],
            anchor[0],
            anchor[1]
        ));
        reports.push(SpriteAssetReport {
            name: config.name.clone(),
            source: source.clone(),
            tile: config.tile,
            size_tiles,
            size_pixels: [decoded.width, decoded.height],
            anchor,
            mapping: config.mapping.as_str().into(),
            alpha_threshold: config.alpha_threshold,
            color_budget,
            source_colors,
            output_colors: palette_indices.len(),
            palette_indices,
            transparent_pixels: indices.iter().filter(|index| **index == 0).count(),
            partial_alpha_pixels,
            mean_squared_error,
        });
        inputs.push(source);
    }

    inputs.sort();
    inputs.dedup();
    Ok(AssembledSprites {
        sheet: render_sheet(&sheet),
        gfx_meta: metadata.join("\n"),
        inputs,
        assets: reports,
    })
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(format!("sprite name {name:?} must match [a-z0-9_]+"));
    }
    Ok(())
}

fn validate_dimensions(config: &SpriteAssetConfig, width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width % TILE_SIZE != 0 || height % TILE_SIZE != 0 {
        return Err(format!(
            "sprite {:?} PNG is {width}x{height}; both dimensions must be nonzero multiples of 8 and are never resized",
            config.name
        ));
    }
    let width_tiles = width / TILE_SIZE;
    let height_tiles = height / TILE_SIZE;
    if config.tile[0] >= TILE_GRID
        || config.tile[1] >= TILE_GRID
        || config.tile[0] + width_tiles > TILE_GRID
        || config.tile[1] + height_tiles > TILE_GRID
    {
        return Err(format!(
            "sprite {:?} at tile {},{} with size {}x{} tiles falls outside the 16x16 sprite sheet",
            config.name, config.tile[0], config.tile[1], width_tiles, height_tiles
        ));
    }
    Ok(())
}

fn claim_tiles(
    config: &SpriteAssetConfig,
    size_tiles: [u32; 2],
    owners: &mut [Option<String>],
) -> Result<(), String> {
    for y in config.tile[1]..config.tile[1] + size_tiles[1] {
        for x in config.tile[0]..config.tile[0] + size_tiles[0] {
            let cell = &mut owners[(y * TILE_GRID + x) as usize];
            if let Some(owner) = cell {
                return Err(format!(
                    "sprite {:?} overlaps sprite {owner:?} at tile {x},{y}",
                    config.name
                ));
            }
            *cell = Some(config.name.clone());
        }
    }
    Ok(())
}

fn place_pixels(
    config: &SpriteAssetConfig,
    width: u32,
    height: u32,
    indices: &[u8],
    sheet: &mut [u8; SHEET_LEN],
) {
    let x0 = config.tile[0] * TILE_SIZE;
    let y0 = config.tile[1] * TILE_SIZE;
    for y in 0..height {
        let source = (y * width) as usize;
        let destination = ((y0 + y) * SHEET_W as u32 + x0) as usize;
        sheet[destination..destination + width as usize]
            .copy_from_slice(&indices[source..source + width as usize]);
    }
}

fn render_sheet(sheet: &[u8; SHEET_LEN]) -> String {
    let mut output = String::with_capacity(SHEET_LEN + SHEET_W - 1);
    for y in 0..SHEET_W {
        if y != 0 {
            output.push('\n');
        }
        for x in 0..SHEET_W {
            output.push(color_char(sheet[y * SHEET_W + x]));
        }
    }
    output
}

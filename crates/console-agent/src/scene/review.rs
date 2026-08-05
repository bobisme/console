use std::collections::BTreeMap;

use console_core::{MAP_W, PALETTE};

use super::GeneratedFile;
use super::compiler::{PackedTile, ReviewInput};
use crate::visual::{self, RgbaImage};

const PAD: u32 = 8;
const HEADER: u32 = 22;

pub(super) fn render(input: &ReviewInput<'_>) -> Result<Vec<GeneratedFile>, String> {
    let by_id = input
        .packed
        .iter()
        .map(|tile| (tile.id, tile))
        .collect::<BTreeMap<_, _>>();
    let mut files = vec![
        png("review/atlas.png", atlas_review(input)?),
        png("review/live-shape.png", live_shape(input, &by_id)?),
        png("review/repeat-3x3.png", repeat_review(input)?),
        png("review/used-adjacency.png", adjacency_review(input)?),
        png("review/collision.png", collision_review(input, &by_id)?),
        png("review/native-map.png", native_map_review(input, &by_id)?),
    ];
    if let Some(heatmap) = lossy_heatmap(input)? {
        files.push(png("review/lossy-heatmap.png", heatmap));
    }
    Ok(files)
}

fn png(relative: &'static str, image: RgbaImage) -> GeneratedFile {
    GeneratedFile {
        relative,
        bytes: image.png(),
    }
}

fn atlas_review(input: &ReviewInput<'_>) -> Result<RgbaImage, String> {
    let width = input.atlas_size[0] * 8;
    let height = input.atlas_size[1] * 8;
    let native = indexed_image(input.atlas_indices, width, height, true)?;
    let zoom = 4;
    let enlarged = native.zoom(zoom)?;
    let label = format!(
        "ATLAS ORIGIN:{},{} SIZE:{}X{} USED:{}",
        input.atlas_origin[0],
        input.atlas_origin[1],
        input.atlas_size[0],
        input.atlas_size[1],
        input.packed.len()
    );
    let mut canvas = visual::blank(
        enlarged.width.max(visual::text_width(&label)) + PAD * 2,
        enlarged.height + HEADER + PAD,
    )?;
    visual::draw_text(&mut canvas, PAD, PAD, &label);
    visual::blit(&mut canvas, &enlarged, PAD, HEADER)?;
    let [gr, gg, gb] = PALETTE[54];
    for x in 0..=input.atlas_size[0] {
        let px = PAD + x * 8 * zoom;
        fill_rect(
            &mut canvas,
            px,
            HEADER,
            1,
            enlarged.height,
            [gr, gg, gb, 255],
        );
    }
    for y in 0..=input.atlas_size[1] {
        let py = HEADER + y * 8 * zoom;
        fill_rect(&mut canvas, PAD, py, enlarged.width, 1, [gr, gg, gb, 255]);
    }
    for tile in input.packed {
        let absolute_x = u32::from(tile.id) % 16;
        let absolute_y = u32::from(tile.id) / 16;
        let local_x = absolute_x - input.atlas_origin[0];
        let local_y = absolute_y - input.atlas_origin[1];
        visual::draw_text(
            &mut canvas,
            PAD + local_x * 8 * zoom + 2,
            HEADER + local_y * 8 * zoom + 2,
            &format!("{:02X}", tile.id),
        );
    }
    Ok(canvas)
}

fn live_shape(
    input: &ReviewInput<'_>,
    by_id: &BTreeMap<u8, &PackedTile>,
) -> Result<RgbaImage, String> {
    let cell = 8;
    let label = format!(
        "LIVE SHAPE NATIVE CELLS:{}X{}",
        input.used_width, input.used_height
    );
    let mut image = visual::blank(
        (input.used_width * cell).max(visual::text_width(&label)) + PAD * 2,
        input.used_height * cell + HEADER + PAD,
    )?;
    visual::draw_text(&mut image, PAD, PAD, &label);
    for y in 0..input.used_height {
        for x in 0..input.used_width {
            let id = input.map[y as usize * MAP_W + x as usize];
            let color = class_color(id, by_id, input);
            fill_rect(
                &mut image,
                PAD + x * cell,
                HEADER + y * cell,
                cell,
                cell,
                color,
            );
        }
    }
    for object in input.objects {
        let x = PAD + (object.at[0].max(0) as u32 / 8) * cell;
        let y = HEADER + (object.at[1].max(0) as u32 / 8) * cell;
        cross(&mut image, x, y, [255, 255, 255, 255]);
    }
    Ok(image)
}

fn repeat_review(input: &ReviewInput<'_>) -> Result<RgbaImage, String> {
    let columns = 8u32;
    let panel_w = 32;
    let panel_h = 42;
    let count = input.packed.len().max(1) as u32;
    let rows = count.div_ceil(columns);
    let mut image = visual::blank(columns * panel_w + PAD * 2, rows * panel_h + HEADER + PAD)?;
    visual::draw_text(&mut image, PAD, PAD, "3X3 REPEAT AND SEAM PROOF");
    for (index, tile) in input.packed.iter().enumerate() {
        let col = index as u32 % columns;
        let row = index as u32 / columns;
        let x0 = PAD + col * panel_w;
        let y0 = HEADER + row * panel_h;
        visual::draw_text(&mut image, x0, y0, &format!("{:02X}", tile.id));
        for repeat_y in 0..3 {
            for repeat_x in 0..3 {
                draw_tile(&mut image, tile, x0 + repeat_x * 8, y0 + 14 + repeat_y * 8);
            }
        }
    }
    Ok(image)
}

fn adjacency_review(input: &ReviewInput<'_>) -> Result<RgbaImage, String> {
    let mut ids = vec![0u8];
    ids.extend(input.packed.iter().map(|tile| tile.id));
    ids.sort_unstable();
    ids.dedup();
    let positions = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index as u32))
        .collect::<BTreeMap<_, _>>();
    let scale = if ids.len() <= 32 { 8 } else { 4 };
    let matrix = ids.len() as u32 * scale;
    let label = "USED ADJACENCY PACKED-ORDER X:A Y:B GREEN:EAST BLUE:SOUTH RED:ILLEGAL";
    let mut image = visual::blank(
        matrix.max(visual::text_width(label)) + PAD * 2,
        matrix + HEADER + PAD,
    )?;
    visual::draw_text(&mut image, PAD, PAD, label);
    for adjacency in input.adjacencies {
        let color = if !adjacency.legal {
            [255, 48, 96, 255]
        } else if adjacency.direction == "east" {
            [64, 224, 120, 255]
        } else {
            [72, 152, 255, 255]
        };
        fill_rect(
            &mut image,
            PAD + positions[&adjacency.a] * scale,
            HEADER + positions[&adjacency.b] * scale,
            scale,
            scale,
            color,
        );
    }
    Ok(image)
}

fn native_map_review(
    input: &ReviewInput<'_>,
    by_id: &BTreeMap<u8, &PackedTile>,
) -> Result<RgbaImage, String> {
    let map = render_native_map(input, by_id)?;
    with_header(
        &format!(
            "NATIVE MAP 1X {}X{} CELLS NO FILTERING",
            input.used_width, input.used_height
        ),
        &map,
    )
}

fn collision_review(
    input: &ReviewInput<'_>,
    by_id: &BTreeMap<u8, &PackedTile>,
) -> Result<RgbaImage, String> {
    let mut map = render_native_map(input, by_id)?;
    for y in 0..input.used_height {
        for x in 0..input.used_width {
            let id = input.map[y as usize * MAP_W + x as usize];
            let Some(tile) = by_id.get(&id) else {
                continue;
            };
            let Some(class) = input.classes.get(&tile.key.class) else {
                continue;
            };
            let overlay = if class.hazard {
                Some([255, 32, 160])
            } else if class.solid {
                Some([255, 176, 32])
            } else {
                None
            };
            if let Some(overlay) = overlay {
                tint_rect(&mut map, x * 8, y * 8, 8, 8, overlay);
            }
        }
    }
    for object in input.objects {
        cross(
            &mut map,
            object.at[0].max(0) as u32,
            object.at[1].max(0) as u32,
            [255, 255, 255, 255],
        );
    }
    with_header("COLLISION ORANGE:SOLID MAGENTA:HAZARD WHITE:OBJECT", &map)
}

fn lossy_heatmap(input: &ReviewInput<'_>) -> Result<Option<RgbaImage>, String> {
    let layers = input
        .heat_layers
        .iter()
        .filter_map(|layer| layer.heat_rgba.as_ref().map(|heat| (layer, heat)))
        .collect::<Vec<_>>();
    if layers.is_empty() {
        return Ok(None);
    }
    let width = layers
        .iter()
        .map(|(layer, _)| layer.width * 8)
        .max()
        .unwrap_or(1)
        .max(visual::text_width(
            "LOSSY PALETTE ERROR HEATMAP BLACK:EXACT MAGENTA:ERROR",
        ));
    let height = layers.iter().try_fold(HEADER + PAD, |height, (layer, _)| {
        height
            .checked_add(HEADER + layer.height * 8)
            .ok_or("lossy heatmap height overflow")
    })?;
    let mut image = visual::blank(width + PAD * 2, height)?;
    visual::draw_text(
        &mut image,
        PAD,
        PAD,
        "LOSSY PALETTE ERROR HEATMAP BLACK:EXACT MAGENTA:ERROR",
    );
    let mut y = HEADER;
    for (layer, heat) in layers {
        visual::draw_text(&mut image, PAD, y, &format!("LAYER:{}", layer.name));
        y += HEADER;
        let source = RgbaImage::new(layer.width * 8, layer.height * 8, heat.clone())?;
        visual::blit(&mut image, &source, PAD, y)?;
        y += source.height;
    }
    Ok(Some(image))
}

fn render_native_map(
    input: &ReviewInput<'_>,
    by_id: &BTreeMap<u8, &PackedTile>,
) -> Result<RgbaImage, String> {
    let mut image = visual::blank(input.used_width * 8, input.used_height * 8)?;
    for y in 0..input.used_height {
        for x in 0..input.used_width {
            let id = input.map[y as usize * MAP_W + x as usize];
            if let Some(tile) = by_id.get(&id) {
                draw_tile(&mut image, tile, x * 8, y * 8);
            }
        }
    }
    Ok(image)
}

fn indexed_image(
    indices: &[u8],
    width: u32,
    height: u32,
    transparent: bool,
) -> Result<RgbaImage, String> {
    if indices.len() != (width * height) as usize {
        return Err("indexed review dimensions do not match pixels".to_string());
    }
    let mut rgba = Vec::with_capacity(indices.len() * 4);
    for index in indices {
        if *index == 0 && transparent {
            let [r, g, b] = PALETTE[48];
            rgba.extend_from_slice(&[r, g, b, 255]);
        } else {
            let [r, g, b] = PALETTE[*index as usize];
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    RgbaImage::new(width, height, rgba)
}

fn draw_tile(image: &mut RgbaImage, tile: &PackedTile, x: u32, y: u32) {
    for ty in 0..8 {
        for tx in 0..8 {
            let index = tile.key.pixels[(ty * 8 + tx) as usize];
            if index == 0 {
                continue;
            }
            let [r, g, b] = PALETTE[index as usize];
            set_pixel(image, x + tx, y + ty, [r, g, b, 255]);
        }
    }
}

fn class_color(id: u8, by_id: &BTreeMap<u8, &PackedTile>, input: &ReviewInput<'_>) -> [u8; 4] {
    let Some(tile) = by_id.get(&id) else {
        let [r, g, b] = PALETTE[48];
        return [r, g, b, 255];
    };
    let class = &input.classes[&tile.key.class];
    if class.hazard {
        [255, 32, 160, 255]
    } else if class.solid {
        [255, 176, 32, 255]
    } else {
        [72, 152, 255, 255]
    }
}

fn with_header(label: &str, source: &RgbaImage) -> Result<RgbaImage, String> {
    let width = source.width.max(visual::text_width(label)) + PAD * 2;
    let mut image = visual::blank(width, source.height + HEADER + PAD)?;
    visual::draw_text(&mut image, PAD, PAD, label);
    visual::blit(&mut image, source, PAD, HEADER)?;
    Ok(image)
}

fn tint_rect(image: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, tint: [u8; 3]) {
    for py in y..(y + h).min(image.height) {
        for px in x..(x + w).min(image.width) {
            let offset = ((py * image.width + px) * 4) as usize;
            image.rgba[offset] = ((u16::from(image.rgba[offset]) + u16::from(tint[0])) / 2) as u8;
            image.rgba[offset + 1] =
                ((u16::from(image.rgba[offset + 1]) + u16::from(tint[1])) / 2) as u8;
            image.rgba[offset + 2] =
                ((u16::from(image.rgba[offset + 2]) + u16::from(tint[2])) / 2) as u8;
        }
    }
}

fn fill_rect(image: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: [u8; 4]) {
    for py in y..y.saturating_add(h).min(image.height) {
        for px in x..x.saturating_add(w).min(image.width) {
            set_pixel(image, px, py, color);
        }
    }
}

fn cross(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 4]) {
    for delta in 0..5 {
        if let Some(px) = x.checked_add(delta).and_then(|value| value.checked_sub(2)) {
            set_pixel(image, px, y, color);
        }
        if let Some(py) = y.checked_add(delta).and_then(|value| value.checked_sub(2)) {
            set_pixel(image, x, py, color);
        }
    }
}

fn set_pixel(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 4]) {
    if x >= image.width || y >= image.height {
        return;
    }
    let offset = ((y * image.width + x) * 4) as usize;
    image.rgba[offset..offset + 4].copy_from_slice(&color);
}

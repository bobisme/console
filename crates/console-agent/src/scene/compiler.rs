use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use console_core::{
    MAP_FORMAT_MARKER, MAP_H, MAP_LEN, MAP_W, PALETTE, SHEET_TILES, TileId, TileMap,
};
use serde::Serialize;

use super::{
    AdjacencyReport, AtlasReport, ClassConfig, GeneratedFile, LayerReport, LayerRole, LintReport,
    MapReport, ObjectReport, PackedTileReport, PaletteMapping, SceneManifest, SceneReport,
    SemanticSplit,
};
use crate::palette::{decode_png_rgba, encode_png_rgba, quantize_rgba};
use crate::sprite::png_io::map_import_pixels;

const TILE: u32 = 8;
/// Four complete native map layers. This bounds retained semantic grids,
/// candidate-source bookkeeping, and optional lossy heatmaps as one request.
const MAX_SCENE_LAYER_CELLS: u64 = (MAP_LEN as u64) * 4;

pub(super) struct SceneBuild {
    pub report: SceneReport,
    pub files: Vec<GeneratedFile>,
}

#[derive(Debug, Clone)]
struct Grid {
    width: u32,
    height: u32,
    cells: Vec<String>,
}

impl Grid {
    fn get(&self, x: u32, y: u32) -> &str {
        &self.cells[(y * self.width + x) as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct TileKey {
    pub class: String,
    pub pixels: [u8; 64],
}

#[derive(Debug)]
struct Candidate {
    names: BTreeSet<String>,
    sources: BTreeSet<String>,
    edges: Option<[String; 4]>,
}

#[derive(Debug)]
pub(super) struct LayerData {
    pub name: String,
    role: LayerRole,
    offset: [u32; 2],
    pub width: u32,
    pub height: u32,
    semantics: Grid,
    keys: Vec<Option<TileKey>>,
    pub heat_rgba: Option<Vec<u8>>,
    report: LayerReport,
}

#[derive(Debug, Clone)]
pub(super) struct PackedTile {
    pub id: TileId,
    pub key: TileKey,
    names: Vec<String>,
    sources: Vec<String>,
    edges: [String; 4],
}

#[derive(Debug)]
struct Metatile {
    width: u32,
    height: u32,
    cells: Vec<Option<TileId>>,
}

struct PackResult {
    packed: Vec<PackedTile>,
    key_ids: BTreeMap<TileKey, TileId>,
    atlas_indices: Vec<u8>,
    capacity: usize,
}

pub(super) struct ReviewInput<'a> {
    pub atlas_origin: [u32; 2],
    pub atlas_size: [u32; 2],
    pub atlas_indices: &'a [u8],
    pub map: &'a TileMap,
    pub used_width: u32,
    pub used_height: u32,
    pub packed: &'a [PackedTile],
    pub classes: &'a BTreeMap<String, ClassConfig>,
    pub objects: &'a [ObjectReport],
    pub adjacencies: &'a [AdjacencyReport],
    pub heat_layers: &'a [LayerData],
}

pub(super) fn compile(manifest_path: &Path) -> Result<SceneBuild, String> {
    let manifest_path = std::fs::canonicalize(manifest_path).map_err(|error| {
        format!(
            "cannot open scene manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    if !manifest_path.is_file() {
        return Err(format!(
            "scene manifest {} is not a file",
            manifest_path.display()
        ));
    }
    let root = manifest_path
        .parent()
        .ok_or_else(|| format!("scene manifest {} has no parent", manifest_path.display()))?
        .to_path_buf();
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let manifest: SceneManifest = toml::from_str(&text)
        .map_err(|error| format!("invalid {}: {error}", manifest_path.display()))?;
    validate_manifest(&manifest)?;

    let classes = collect_classes(&manifest.classes)?;
    let mut inputs = BTreeSet::from([manifest_path.clone()]);
    let mut candidates = BTreeMap::<TileKey, Candidate>::new();
    let mut layers = load_layers(&root, &manifest, &classes, &mut candidates, &mut inputs)?;
    let named_keys = collect_named_tiles(&manifest, &layers, &classes, &mut candidates)?;
    let PackResult {
        packed,
        key_ids,
        atlas_indices,
        capacity,
    } = pack_tiles(&manifest, candidates, &named_keys)?;
    let named_ids = named_keys
        .iter()
        .map(|(name, key)| (name.clone(), key_ids[key]))
        .collect::<BTreeMap<_, _>>();
    let metatiles = collect_metatiles(&manifest, &named_ids)?;

    let mut map = [0 as TileId; MAP_LEN];
    place_base_layers(&layers, &key_ids, &mut map)?;
    let placed_keys = layers
        .iter()
        .filter(|layer| layer.role != LayerRole::Library)
        .flat_map(|layer| layer.keys.iter().flatten())
        .collect::<BTreeSet<_>>();
    let mut used_named = named_keys
        .iter()
        .filter(|(_, key)| placed_keys.contains(key))
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    let (autotile_cells, variant_cells, variant_grid) = apply_play_grid(
        &root,
        &manifest,
        &named_ids,
        &packed,
        &mut map,
        &mut used_named,
        &mut inputs,
    )?;
    apply_stamps(&manifest, &metatiles, &mut map, &mut used_named, &named_ids)?;
    apply_overrides(&manifest, &named_ids, &mut map, &mut used_named)?;
    let objects = validate_objects(&manifest)?;
    let (used_width, used_height) = used_extent(&map);
    let mut lint = lint_scene(
        &manifest,
        &map,
        used_width,
        used_height,
        &packed,
        &named_ids,
        &used_named,
        variant_grid.as_ref(),
    );

    let atlas_png = render_atlas_png(
        &atlas_indices,
        manifest.atlas.size[0] * TILE,
        manifest.atlas.size[1] * TILE,
    );
    let map_text = render_map(&map, used_width, used_height);
    let tile_classes = render_classes_lua(&classes, &packed);
    let decorative_layers = render_layers_lua(&layers, &key_ids)?;
    let objects_lua = render_objects_lua(&objects);
    let palette_indices = packed
        .iter()
        .flat_map(|tile| tile.key.pixels)
        .filter(|index| *index != 0)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let packed_reports = packed.iter().map(packed_report).collect::<Vec<_>>();
    let nonzero_cells = map.iter().filter(|id| **id != 0).count();
    for layer in &mut layers {
        // Reports are assembled after all validation so their ordering exactly
        // follows the manifest's deterministic layer order.
        layer.report.nonempty_cells = layer.keys.iter().filter(|key| key.is_some()).count();
    }
    let mut report = SceneReport {
        command: "console scene compile",
        manifest: manifest_path.display().to_string(),
        output: String::new(),
        status: "compiled",
        scene: manifest.name.clone(),
        seed: manifest.seed,
        mapping: manifest.atlas.mapping.as_str(),
        alpha_threshold: manifest.atlas.alpha_threshold,
        max_colors: manifest.atlas.max_colors,
        atlas: AtlasReport {
            origin: manifest.atlas.origin,
            size: manifest.atlas.size,
            capacity,
            used: packed.len(),
            available: capacity - packed.len(),
            palette_indices,
            tiles: packed_reports,
        },
        map: MapReport {
            used_width,
            used_height,
            nonzero_cells,
            autotile_cells,
            variant_cells,
            stamps: manifest.stamps.len(),
            overrides: manifest.overrides.len(),
        },
        layers: layers.iter().map(|layer| layer.report.clone()).collect(),
        classes: classes.values().cloned().collect(),
        objects,
        lint: std::mem::take(&mut lint),
        inputs: inputs
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        artifacts: Vec::new(),
    };

    let mut files = vec![
        GeneratedFile {
            relative: "atlas.png",
            bytes: atlas_png,
        },
        GeneratedFile {
            relative: "map.txt",
            bytes: map_text.into_bytes(),
        },
        GeneratedFile {
            relative: "tile_classes.lua",
            bytes: tile_classes.into_bytes(),
        },
        GeneratedFile {
            relative: "decorative_layers.lua",
            bytes: decorative_layers.into_bytes(),
        },
        GeneratedFile {
            relative: "objects.lua",
            bytes: objects_lua.into_bytes(),
        },
    ];
    files.extend(super::review::render(&ReviewInput {
        atlas_origin: manifest.atlas.origin,
        atlas_size: manifest.atlas.size,
        atlas_indices: &atlas_indices,
        map: &map,
        used_width,
        used_height,
        packed: &packed,
        classes: &classes,
        objects: &report.objects,
        adjacencies: &report.lint.used_adjacencies,
        heat_layers: &layers,
    })?);
    let provenance = Provenance {
        scene: &report.scene,
        scene_version: manifest.scene_version,
        seed: manifest.seed,
        mapping: manifest.atlas.mapping.as_str(),
        alpha_threshold: manifest.atlas.alpha_threshold,
        max_colors: manifest.atlas.max_colors,
        inputs: &report.inputs,
        atlas: &report.atlas,
        map: &report.map,
        layers: &report.layers,
        objects: &report.objects,
        lint: &report.lint,
        generated: files
            .iter()
            .map(|file| file.relative)
            .chain(std::iter::once("provenance.json"))
            .collect(),
    };
    let mut provenance_bytes = serde_json::to_vec_pretty(&provenance)
        .map_err(|error| format!("serializing scene provenance: {error}"))?;
    provenance_bytes.push(b'\n');
    files.push(GeneratedFile {
        relative: "provenance.json",
        bytes: provenance_bytes,
    });
    report.artifacts.clear();
    Ok(SceneBuild { report, files })
}

#[derive(Serialize)]
struct Provenance<'a> {
    scene: &'a str,
    scene_version: u32,
    seed: u64,
    mapping: &'a str,
    alpha_threshold: u8,
    max_colors: Option<usize>,
    inputs: &'a [String],
    atlas: &'a AtlasReport,
    map: &'a MapReport,
    layers: &'a [LayerReport],
    objects: &'a [ObjectReport],
    lint: &'a LintReport,
    generated: Vec<&'static str>,
}

fn validate_manifest(manifest: &SceneManifest) -> Result<(), String> {
    if manifest.scene_version != 1 {
        return Err(format!(
            "unsupported scene_version {}; expected 1",
            manifest.scene_version
        ));
    }
    validate_name(&manifest.name, "scene name")?;
    let [x, y] = manifest.atlas.origin;
    let [w, h] = manifest.atlas.size;
    let right = x.checked_add(w);
    let bottom = y.checked_add(h);
    if w == 0
        || h == 0
        || x >= SHEET_TILES as u32
        || y >= SHEET_TILES as u32
        || right.is_none_or(|right| right > SHEET_TILES as u32)
        || bottom.is_none_or(|bottom| bottom > SHEET_TILES as u32)
    {
        return Err(format!(
            "atlas reservation origin={x},{y} size={w}x{h} falls outside the {SHEET_TILES}x{SHEET_TILES} sheet"
        ));
    }
    if manifest.atlas.mapping == PaletteMapping::Quantize {
        let colors = manifest
            .atlas
            .max_colors
            .ok_or("atlas.mapping=\"quantize\" requires explicit atlas.max_colors (1-63)")?;
        if !(1..=63).contains(&colors) {
            return Err(format!("atlas.max_colors must be 1-63, got {colors}"));
        }
    } else if manifest.atlas.max_colors.is_some() {
        return Err("atlas.max_colors is only valid with mapping=\"quantize\"".to_string());
    }
    if manifest.layers.is_empty() {
        return Err("scene has no layers".to_string());
    }
    if manifest
        .layers
        .iter()
        .filter(|layer| layer.role == LayerRole::Play)
        .count()
        > 1
    {
        return Err("scene may have at most one role=\"play\" PNG layer".to_string());
    }
    Ok(())
}

fn validate_name(name: &str, label: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(format!("{label} {name:?} must match [a-z0-9_]+"));
    }
    Ok(())
}

fn collect_classes(classes: &[ClassConfig]) -> Result<BTreeMap<String, ClassConfig>, String> {
    if classes.is_empty() {
        return Err("scene has no semantic classes".to_string());
    }
    let mut out = BTreeMap::new();
    for class in classes {
        validate_name(&class.name, "class name")?;
        for tag in &class.tags {
            validate_name(tag, &format!("class {:?} tag", class.name))?;
        }
        if out.insert(class.name.clone(), class.clone()).is_some() {
            return Err(format!("duplicate class {:?}", class.name));
        }
    }
    Ok(out)
}

fn resolve_input(root: &Path, relative: &Path, label: &str) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(format!("{label} must be a confined relative path"));
    }
    let joined = root.join(relative);
    let resolved = std::fs::canonicalize(&joined)
        .map_err(|error| format!("cannot open {label} {}: {error}", joined.display()))?;
    if !resolved.starts_with(root) || !resolved.is_file() {
        return Err(format!(
            "{label} {} escapes the scene root or is not a file",
            joined.display()
        ));
    }
    Ok(resolved)
}

fn parse_grid(path: &Path, label: &str) -> Result<Grid, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {label} {}: {error}", path.display()))?;
    let mut rows = Vec::<Vec<String>>::new();
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let row = line
            .split(|character: char| character == ',' || character.is_ascii_whitespace())
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if row.is_empty() {
            continue;
        }
        if let Some(width) = rows.first().map(Vec::len)
            && row.len() != width
        {
            return Err(format!(
                "{label} {}:{} has {} cells, expected {width}",
                path.display(),
                line_index + 1,
                row.len()
            ));
        }
        rows.push(row);
    }
    let width = rows.first().map_or(0, Vec::len);
    if width == 0 || rows.is_empty() {
        return Err(format!("{label} {} has no grid cells", path.display()));
    }
    let height = rows.len();
    Ok(Grid {
        width: width as u32,
        height: height as u32,
        cells: rows.into_iter().flatten().collect(),
    })
}

fn load_layers(
    root: &Path,
    manifest: &SceneManifest,
    classes: &BTreeMap<String, ClassConfig>,
    candidates: &mut BTreeMap<TileKey, Candidate>,
    inputs: &mut BTreeSet<PathBuf>,
) -> Result<Vec<LayerData>, String> {
    let mut names = BTreeSet::new();
    let mut layers = Vec::new();
    let mut total_cells = 0u64;
    for config in &manifest.layers {
        validate_name(&config.name, "layer name")?;
        if !names.insert(config.name.clone()) {
            return Err(format!("duplicate layer {:?}", config.name));
        }
        let source = resolve_input(
            root,
            &config.source,
            &format!("layer {:?} source", config.name),
        )?;
        let semantics_path = resolve_input(
            root,
            &config.semantics,
            &format!("layer {:?} semantics", config.name),
        )?;
        let bytes = std::fs::read(&source)
            .map_err(|error| format!("cannot read layer PNG {}: {error}", source.display()))?;
        let decoded = decode_png_rgba(&bytes)
            .map_err(|error| format!("cannot decode layer PNG {}: {error}", source.display()))?;
        if decoded.width == 0
            || decoded.height == 0
            || decoded.width % TILE != 0
            || decoded.height % TILE != 0
        {
            return Err(format!(
                "layer {:?} PNG is {}x{}; dimensions must be nonzero multiples of 8 and are never resized",
                config.name, decoded.width, decoded.height
            ));
        }
        let semantics = parse_grid(
            &semantics_path,
            &format!("layer {:?} semantics", config.name),
        )?;
        let width = decoded.width / TILE;
        let height = decoded.height / TILE;
        let layer_cells = u64::from(width) * u64::from(height);
        total_cells = total_cells.checked_add(layer_cells).ok_or_else(|| {
            format!(
                "aggregate scene layer cell count overflows while adding layer {:?}",
                config.name
            )
        })?;
        if total_cells > MAX_SCENE_LAYER_CELLS {
            return Err(format!(
                "aggregate scene layers retain {total_cells} cells, exceeding the {MAX_SCENE_LAYER_CELLS}-cell safety limit (four full native map layers)"
            ));
        }
        if config.role != LayerRole::Library {
            validate_map_rect(
                config.offset,
                width,
                height,
                &format!("layer {:?}", config.name),
            )?;
        }
        if semantics.width != width || semantics.height != height {
            return Err(format!(
                "layer {:?} PNG is {width}x{height} cells but its semantic grid is {}x{}",
                config.name, semantics.width, semantics.height
            ));
        }
        let (indices, source_colors, partial_alpha_pixels, mse, heat_rgba) =
            map_layer_pixels(&decoded.rgba, manifest)?;
        let palette_indices = indices
            .iter()
            .copied()
            .filter(|index| *index != 0)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut keys = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let class = semantics.get(x, y);
                let pixels = extract_cell(&indices, decoded.width, x, y);
                let blank = pixels.iter().all(|index| *index == 0);
                if class == "." {
                    if !blank {
                        return Err(format!(
                            "layer {:?} cell {x},{y} has visible pixels but semantic class `.`",
                            config.name
                        ));
                    }
                    keys.push(None);
                    continue;
                }
                if !classes.contains_key(class) {
                    return Err(format!(
                        "layer {:?} cell {x},{y} uses unknown class {class:?}",
                        config.name
                    ));
                }
                let key = TileKey {
                    class: class.to_string(),
                    pixels,
                };
                candidates
                    .entry(key.clone())
                    .or_insert_with(|| Candidate {
                        names: BTreeSet::new(),
                        sources: BTreeSet::new(),
                        edges: None,
                    })
                    .sources
                    .insert(format!("{}:{x},{y}", config.name));
                keys.push(Some(key));
            }
        }
        inputs.insert(source.clone());
        inputs.insert(semantics_path.clone());
        layers.push(LayerData {
            name: config.name.clone(),
            role: config.role,
            offset: config.offset,
            width,
            height,
            semantics,
            keys,
            heat_rgba,
            report: LayerReport {
                name: config.name.clone(),
                role: config.role,
                source: source.display().to_string(),
                semantics: semantics_path.display().to_string(),
                size_cells: [width, height],
                offset: config.offset,
                nonempty_cells: 0,
                source_colors,
                palette_indices,
                partial_alpha_pixels,
                mean_squared_error: round6(mse),
            },
        });
    }
    if let Some(max_colors) = manifest.atlas.max_colors {
        let atlas_palette = layers
            .iter()
            .flat_map(|layer| layer.report.palette_indices.iter().copied())
            .collect::<BTreeSet<_>>();
        if atlas_palette.len() > max_colors {
            return Err(format!(
                "quantized layer union uses {} Apollo64 indices {:?}, exceeding atlas.max_colors={max_colors}",
                atlas_palette.len(),
                atlas_palette
            ));
        }
    }
    Ok(layers)
}

type MappedLayer = (Vec<u8>, usize, usize, f64, Option<Vec<u8>>);

fn map_layer_pixels(rgba: &[u8], manifest: &SceneManifest) -> Result<MappedLayer, String> {
    let mapping = manifest.atlas.mapping;
    let (indices, source_colors, partial_alpha) = match mapping {
        PaletteMapping::Exact | PaletteMapping::Nearest => {
            map_import_pixels(rgba, mapping.as_str(), manifest.atlas.alpha_threshold)?
        }
        PaletteMapping::Quantize => {
            let result = quantize_rgba(
                rgba,
                manifest.atlas.max_colors.expect("quantize validated"),
                manifest.atlas.alpha_threshold,
            )?;
            let partial = rgba
                .chunks_exact(4)
                .filter(|pixel| pixel[3] >= manifest.atlas.alpha_threshold && pixel[3] != 255)
                .count();
            (result.indices, result.source_colors, partial)
        }
    };
    if mapping == PaletteMapping::Exact {
        return Ok((indices, source_colors, partial_alpha, 0.0, None));
    }
    let mut heat = Vec::with_capacity(rgba.len());
    let mut squared = 0u64;
    let mut opaque = 0u64;
    for (source, index) in rgba.chunks_exact(4).zip(indices.iter().copied()) {
        if index == 0 {
            heat.extend_from_slice(&[0, 0, 0, 255]);
            continue;
        }
        let [r, g, b] = PALETTE[index as usize];
        let error = [r, g, b]
            .into_iter()
            .zip(source[..3].iter().copied())
            .map(|(mapped, source)| {
                let delta = i32::from(mapped) - i32::from(source);
                (delta * delta) as u64
            })
            .sum::<u64>();
        squared += error;
        opaque += 1;
        let magnitude = ((error / 3).min(65_025) as f64).sqrt().min(255.0) as u8;
        heat.extend_from_slice(&[magnitude, 0, magnitude / 2, 255]);
    }
    let mse = if opaque == 0 {
        0.0
    } else {
        squared as f64 / (opaque * 3) as f64
    };
    Ok((indices, source_colors, partial_alpha, mse, Some(heat)))
}

fn extract_cell(indices: &[u8], width_pixels: u32, cell_x: u32, cell_y: u32) -> [u8; 64] {
    let mut pixels = [0u8; 64];
    for y in 0..TILE {
        let source = ((cell_y * TILE + y) * width_pixels + cell_x * TILE) as usize;
        let destination = (y * TILE) as usize;
        pixels[destination..destination + TILE as usize]
            .copy_from_slice(&indices[source..source + TILE as usize]);
    }
    pixels
}

fn collect_named_tiles(
    manifest: &SceneManifest,
    layers: &[LayerData],
    classes: &BTreeMap<String, ClassConfig>,
    candidates: &mut BTreeMap<TileKey, Candidate>,
) -> Result<BTreeMap<String, TileKey>, String> {
    let layer_by_name = layers
        .iter()
        .map(|layer| (layer.name.as_str(), layer))
        .collect::<BTreeMap<_, _>>();
    let mut named = BTreeMap::new();
    for config in &manifest.tiles {
        validate_name(&config.name, "tile name")?;
        if named.contains_key(&config.name) {
            return Err(format!("duplicate tile {:?}", config.name));
        }
        if !classes.contains_key(&config.class) {
            return Err(format!(
                "tile {:?} uses unknown class {:?}",
                config.name, config.class
            ));
        }
        let layer = layer_by_name.get(config.layer.as_str()).ok_or_else(|| {
            format!(
                "tile {:?} refers to unknown layer {:?}",
                config.name, config.layer
            )
        })?;
        let [x, y, w, h] = config.rect;
        if w != TILE || h != TILE || x % TILE != 0 || y % TILE != 0 {
            return Err(format!(
                "tile {:?} rect must be one aligned 8x8 source rectangle, got {x},{y},{w},{h}",
                config.name
            ));
        }
        let cx = x / TILE;
        let cy = y / TILE;
        if cx >= layer.width || cy >= layer.height {
            return Err(format!(
                "tile {:?} rect starts outside layer {:?}",
                config.name, config.layer
            ));
        }
        let semantic = layer.semantics.get(cx, cy);
        if semantic != config.class {
            return Err(format!(
                "tile {:?} declares class {:?}, but layer {:?} semantic cell {cx},{cy} is {semantic:?}",
                config.name, config.class, config.layer
            ));
        }
        let key = layer.keys[(cy * layer.width + cx) as usize]
            .clone()
            .ok_or_else(|| format!("tile {:?} selects an empty semantic cell", config.name))?;
        let candidate = candidates.get_mut(&key).expect("layer cell registered");
        if let Some(edges) = &config.edges {
            for edge in edges {
                validate_edge(edge, &config.name)?;
            }
            if let Some(existing) = &candidate.edges
                && existing != edges
            {
                return Err(format!(
                    "tile {:?} gives a pixel/class-equivalent allocation conflicting edge semantics",
                    config.name
                ));
            }
            candidate.edges = Some(edges.clone());
        }
        candidate.names.insert(config.name.clone());
        named.insert(config.name.clone(), key);
    }
    Ok(named)
}

fn validate_edge(edge: &str, tile: &str) -> Result<(), String> {
    if edge == "*" {
        return Ok(());
    }
    validate_name(edge, &format!("tile {tile:?} edge"))
}

fn reservation_ids(manifest: &SceneManifest) -> Vec<TileId> {
    let mut ids = Vec::new();
    for y in manifest.atlas.origin[1]..manifest.atlas.origin[1] + manifest.atlas.size[1] {
        for x in manifest.atlas.origin[0]..manifest.atlas.origin[0] + manifest.atlas.size[0] {
            let id = (y * SHEET_TILES as u32 + x) as TileId;
            if id != 0 {
                ids.push(id);
            }
        }
    }
    ids
}

fn pack_tiles(
    manifest: &SceneManifest,
    candidates: BTreeMap<TileKey, Candidate>,
    named: &BTreeMap<String, TileKey>,
) -> Result<PackResult, String> {
    let ids = reservation_ids(manifest);
    let capacity = ids.len();
    if candidates.len() > capacity {
        let mut reuse = candidates
            .iter()
            .map(|(key, candidate)| {
                (
                    candidate.sources.len() + candidate.names.len(),
                    key.class.clone(),
                    candidate.sources.iter().cloned().collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        reuse.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let opportunities = reuse
            .into_iter()
            .take(5)
            .map(|(uses, class, sources)| format!("class={class} uses={uses} sources={sources:?}"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "exact semantic packing needs {} tiles but the declared atlas reservation has {capacity} usable slots (tile 0 is reserved); largest existing reuse groups: {opportunities}",
            candidates.len()
        ));
    }
    let named_by_key = named.iter().fold(
        BTreeMap::<TileKey, Vec<String>>::new(),
        |mut by_key, (name, key)| {
            by_key.entry(key.clone()).or_default().push(name.clone());
            by_key
        },
    );
    let mut atlas =
        vec![0u8; (manifest.atlas.size[0] * TILE * manifest.atlas.size[1] * TILE) as usize];
    let atlas_w = manifest.atlas.size[0] * TILE;
    let mut packed = Vec::with_capacity(candidates.len());
    let mut key_ids = BTreeMap::new();
    for ((key, candidate), id) in candidates.into_iter().zip(ids) {
        let absolute_x = u32::from(id) % SHEET_TILES as u32;
        let absolute_y = u32::from(id) / SHEET_TILES as u32;
        let local_x = (absolute_x - manifest.atlas.origin[0]) * TILE;
        let local_y = (absolute_y - manifest.atlas.origin[1]) * TILE;
        for y in 0..TILE {
            let source = (y * TILE) as usize;
            let destination = ((local_y + y) * atlas_w + local_x) as usize;
            atlas[destination..destination + TILE as usize]
                .copy_from_slice(&key.pixels[source..source + TILE as usize]);
        }
        let edges = candidate.edges.unwrap_or_else(|| {
            [
                key.class.clone(),
                key.class.clone(),
                key.class.clone(),
                key.class.clone(),
            ]
        });
        let mut names = named_by_key.get(&key).cloned().unwrap_or_default();
        names.sort();
        packed.push(PackedTile {
            id,
            key: key.clone(),
            names,
            sources: candidate.sources.into_iter().collect(),
            edges,
        });
        key_ids.insert(key, id);
    }
    Ok(PackResult {
        packed,
        key_ids,
        atlas_indices: atlas,
        capacity,
    })
}

fn collect_metatiles(
    manifest: &SceneManifest,
    named: &BTreeMap<String, TileId>,
) -> Result<BTreeMap<String, Metatile>, String> {
    let mut out = BTreeMap::new();
    for config in &manifest.metatiles {
        validate_name(&config.name, "metatile name")?;
        if out.contains_key(&config.name) {
            return Err(format!("duplicate metatile {:?}", config.name));
        }
        let grid = parse_inline_rows(&config.rows, &format!("metatile {:?}", config.name))?;
        let mut cells = Vec::with_capacity(grid.cells.len());
        for token in &grid.cells {
            cells.push(if token == "." {
                None
            } else {
                Some(*named.get(token).ok_or_else(|| {
                    format!("metatile {:?} uses unknown tile {token:?}", config.name)
                })?)
            });
        }
        out.insert(
            config.name.clone(),
            Metatile {
                width: grid.width,
                height: grid.height,
                cells,
            },
        );
    }
    Ok(out)
}

fn parse_inline_rows(rows: &[String], label: &str) -> Result<Grid, String> {
    if rows.is_empty() {
        return Err(format!("{label} has no rows"));
    }
    let mut cells = Vec::new();
    let mut width = None;
    for (index, row) in rows.iter().enumerate() {
        let tokens = row
            .split(|character: char| character == ',' || character.is_ascii_whitespace())
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if tokens.is_empty() {
            return Err(format!("{label} row {} is empty", index + 1));
        }
        if let Some(expected) = width
            && tokens.len() != expected
        {
            return Err(format!(
                "{label} row {} has {} cells, expected {expected}",
                index + 1,
                tokens.len()
            ));
        }
        width = Some(tokens.len());
        cells.extend(tokens);
    }
    Ok(Grid {
        width: width.unwrap_or(0) as u32,
        height: rows.len() as u32,
        cells,
    })
}

fn place_base_layers(
    layers: &[LayerData],
    ids: &BTreeMap<TileKey, TileId>,
    map: &mut TileMap,
) -> Result<(), String> {
    for layer in layers.iter().filter(|layer| layer.role == LayerRole::Play) {
        validate_map_rect(
            layer.offset,
            layer.width,
            layer.height,
            &format!("layer {:?}", layer.name),
        )?;
        for y in 0..layer.height {
            for x in 0..layer.width {
                if let Some(key) = &layer.keys[(y * layer.width + x) as usize] {
                    let mx = layer.offset[0] + x;
                    let my = layer.offset[1] + y;
                    map[my as usize * MAP_W + mx as usize] = ids[key];
                }
            }
        }
    }
    Ok(())
}

fn validate_map_rect(origin: [u32; 2], width: u32, height: u32, label: &str) -> Result<(), String> {
    if origin[0] >= MAP_W as u32
        || origin[1] >= MAP_H as u32
        || origin[0]
            .checked_add(width)
            .is_none_or(|right| right > MAP_W as u32)
        || origin[1]
            .checked_add(height)
            .is_none_or(|bottom| bottom > MAP_H as u32)
    {
        return Err(format!(
            "{label} origin={},{} size={width}x{height} exceeds the {}x{} map",
            origin[0], origin[1], MAP_W, MAP_H
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_play_grid(
    root: &Path,
    manifest: &SceneManifest,
    named: &BTreeMap<String, TileId>,
    packed: &[PackedTile],
    map: &mut TileMap,
    used_named: &mut BTreeSet<String>,
    inputs: &mut BTreeSet<PathBuf>,
) -> Result<(usize, usize, Option<Grid>), String> {
    validate_families(manifest, named, packed)?;
    let Some(relative) = &manifest.play.grid else {
        return Ok((0, 0, None));
    };
    let path = resolve_input(root, relative, "play.grid")?;
    let grid = parse_grid(&path, "play.grid")?;
    inputs.insert(path);
    validate_map_rect(manifest.play.origin, grid.width, grid.height, "play.grid")?;
    let autotiles = manifest
        .autotiles
        .iter()
        .map(|family| (family.name.as_str(), family))
        .collect::<BTreeMap<_, _>>();
    let variants = manifest
        .variants
        .iter()
        .map(|family| (family.name.as_str(), family))
        .collect::<BTreeMap<_, _>>();
    let mut autotile_cells = 0;
    let mut variant_cells = 0;
    let mut selected_variants = grid.clone();
    selected_variants.cells.fill(".".to_string());
    for y in 0..grid.height {
        for x in 0..grid.width {
            let token = grid.get(x, y);
            let (id, selected) = if token == "." {
                continue;
            } else if let Some(name) = token.strip_prefix("auto:") {
                let family = autotiles.get(name).ok_or_else(|| {
                    format!("play.grid cell {x},{y} uses unknown autotile {name:?}")
                })?;
                let mask = four_neighbor_mask(&grid, x, y, token);
                let key = mask.to_string();
                let tile = family.lookup.get(&key).ok_or_else(|| {
                    format!(
                        "autotile {:?} has no lookup for used four-neighbor mask {mask} at play.grid {x},{y}",
                        family.name
                    )
                })?;
                autotile_cells += 1;
                (*named.get(tile).expect("families validated"), tile.clone())
            } else if let Some(name) = token.strip_prefix("variant:") {
                let family = variants.get(name).ok_or_else(|| {
                    format!("play.grid cell {x},{y} uses unknown variant {name:?}")
                })?;
                let tile = choose_variant(manifest.seed, x, y, family)?;
                variant_cells += 1;
                (
                    *named.get(tile).expect("families validated"),
                    tile.to_string(),
                )
            } else {
                let id = *named
                    .get(token)
                    .ok_or_else(|| format!("play.grid cell {x},{y} uses unknown tile {token:?}"))?;
                (id, token.to_string())
            };
            let mx = manifest.play.origin[0] + x;
            let my = manifest.play.origin[1] + y;
            map[my as usize * MAP_W + mx as usize] = id;
            if token.starts_with("variant:") {
                selected_variants.cells[(y * grid.width + x) as usize] = selected.clone();
            }
            used_named.insert(selected);
        }
    }
    Ok((autotile_cells, variant_cells, Some(selected_variants)))
}

fn validate_families(
    manifest: &SceneManifest,
    named: &BTreeMap<String, TileId>,
    packed: &[PackedTile],
) -> Result<(), String> {
    let class_by_id = packed
        .iter()
        .map(|tile| (tile.id, tile.key.class.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut family_names = BTreeSet::new();
    for family in &manifest.autotiles {
        validate_name(&family.name, "autotile name")?;
        if !family_names.insert(family.name.clone()) {
            return Err(format!("duplicate tile family name {:?}", family.name));
        }
        if family.lookup.is_empty() {
            return Err(format!("autotile {:?} has no lookup entries", family.name));
        }
        for (mask, tile) in &family.lookup {
            let parsed = mask
                .parse::<u8>()
                .map_err(|_| format!("autotile {:?} mask {mask:?} must be 0-15", family.name))?;
            if parsed > 15 || parsed.to_string() != *mask {
                return Err(format!(
                    "autotile {:?} mask {mask:?} must be canonical decimal 0-15",
                    family.name
                ));
            }
            validate_family_tile(&family.name, &family.class, tile, named, &class_by_id)?;
        }
    }
    for family in &manifest.variants {
        validate_name(&family.name, "variant name")?;
        if !family_names.insert(family.name.clone()) {
            return Err(format!("duplicate tile family name {:?}", family.name));
        }
        if family.choices.is_empty() {
            return Err(format!("variant {:?} has no choices", family.name));
        }
        let mut total = 0u64;
        for choice in &family.choices {
            if choice.weight == 0 {
                return Err(format!(
                    "variant {:?} tile {:?} has zero weight",
                    family.name, choice.tile
                ));
            }
            total = total
                .checked_add(u64::from(choice.weight))
                .ok_or_else(|| format!("variant {:?} weights overflow", family.name))?;
            validate_family_tile(
                &family.name,
                &family.class,
                &choice.tile,
                named,
                &class_by_id,
            )?;
        }
    }
    Ok(())
}

fn validate_family_tile(
    family: &str,
    class: &str,
    tile: &str,
    named: &BTreeMap<String, TileId>,
    class_by_id: &BTreeMap<TileId, &str>,
) -> Result<(), String> {
    let id = named
        .get(tile)
        .ok_or_else(|| format!("tile family {family:?} uses unknown tile {tile:?}"))?;
    let actual = class_by_id[id];
    if actual != class {
        return Err(format!(
            "tile family {family:?} declares class {class:?}, but tile {tile:?} has class {actual:?}"
        ));
    }
    Ok(())
}

fn four_neighbor_mask(grid: &Grid, x: u32, y: u32, token: &str) -> u8 {
    let same = |x: i32, y: i32| {
        x >= 0
            && y >= 0
            && x < grid.width as i32
            && y < grid.height as i32
            && grid.get(x as u32, y as u32) == token
    };
    u8::from(same(x as i32, y as i32 - 1))
        | (u8::from(same(x as i32 + 1, y as i32)) << 1)
        | (u8::from(same(x as i32, y as i32 + 1)) << 2)
        | (u8::from(same(x as i32 - 1, y as i32)) << 3)
}

fn choose_variant(
    seed: u64,
    x: u32,
    y: u32,
    family: &super::VariantConfig,
) -> Result<&str, String> {
    let total = family
        .choices
        .iter()
        .try_fold(0u64, |total, choice| {
            total.checked_add(u64::from(choice.weight))
        })
        .ok_or_else(|| format!("variant {:?} weights overflow", family.name))?;
    let mut value = stable_hash(seed, x, y, &family.name) % total;
    for choice in &family.choices {
        if value < u64::from(choice.weight) {
            return Ok(&choice.tile);
        }
        value -= u64::from(choice.weight);
    }
    unreachable!("weighted choice total is exact")
}

fn stable_hash(seed: u64, x: u32, y: u32, name: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64 ^ seed;
    for byte in x
        .to_le_bytes()
        .into_iter()
        .chain(y.to_le_bytes())
        .chain(name.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn apply_stamps(
    manifest: &SceneManifest,
    metatiles: &BTreeMap<String, Metatile>,
    map: &mut TileMap,
    used_named: &mut BTreeSet<String>,
    named: &BTreeMap<String, TileId>,
) -> Result<(), String> {
    let names_by_id = named
        .iter()
        .map(|(name, id)| (*id, name.as_str()))
        .collect::<BTreeMap<_, _>>();
    for stamp in &manifest.stamps {
        let metatile = metatiles
            .get(&stamp.metatile)
            .ok_or_else(|| format!("stamp refers to unknown metatile {:?}", stamp.metatile))?;
        validate_map_rect(stamp.at, metatile.width, metatile.height, "metatile stamp")?;
        for y in 0..metatile.height {
            for x in 0..metatile.width {
                if let Some(id) = metatile.cells[(y * metatile.width + x) as usize] {
                    let mx = stamp.at[0] + x;
                    let my = stamp.at[1] + y;
                    map[my as usize * MAP_W + mx as usize] = id;
                    if let Some(name) = names_by_id.get(&id) {
                        used_named.insert((*name).to_string());
                    }
                }
            }
        }
    }
    Ok(())
}

fn apply_overrides(
    manifest: &SceneManifest,
    named: &BTreeMap<String, TileId>,
    map: &mut TileMap,
    used_named: &mut BTreeSet<String>,
) -> Result<(), String> {
    let mut cells = BTreeSet::new();
    for override_ in &manifest.overrides {
        if override_.at[0] >= MAP_W as u32 || override_.at[1] >= MAP_H as u32 {
            return Err(format!(
                "override at {},{} exceeds the {}x{} map",
                override_.at[0], override_.at[1], MAP_W, MAP_H
            ));
        }
        if !cells.insert(override_.at) {
            return Err(format!(
                "duplicate override at {},{}",
                override_.at[0], override_.at[1]
            ));
        }
        let id = *named
            .get(&override_.tile)
            .ok_or_else(|| format!("override uses unknown tile {:?}", override_.tile))?;
        map[override_.at[1] as usize * MAP_W + override_.at[0] as usize] = id;
        used_named.insert(override_.tile.clone());
    }
    Ok(())
}

fn validate_objects(manifest: &SceneManifest) -> Result<Vec<ObjectReport>, String> {
    let map_width = (MAP_W * TILE as usize) as i32;
    let map_height = (MAP_H * TILE as usize) as i32;
    let mut names = BTreeSet::new();
    let mut bounds = Vec::<([i32; 4], String)>::new();
    let mut reports = Vec::new();
    for object in &manifest.objects {
        validate_name(&object.name, "object name")?;
        validate_name(&object.kind, &format!("object {:?} kind", object.name))?;
        if !names.insert(object.name.clone()) {
            return Err(format!("duplicate object {:?}", object.name));
        }
        if object.size[0] == 0 || object.size[1] == 0 {
            return Err(format!("object {:?} size must be nonzero", object.name));
        }
        let left = object.at[0]
            .checked_sub(object.anchor[0])
            .ok_or_else(|| format!("object {:?} x bound overflows", object.name))?;
        let top = object.at[1]
            .checked_sub(object.anchor[1])
            .ok_or_else(|| format!("object {:?} y bound overflows", object.name))?;
        let width = i32::try_from(object.size[0])
            .map_err(|_| format!("object {:?} width exceeds i32", object.name))?;
        let height = i32::try_from(object.size[1])
            .map_err(|_| format!("object {:?} height exceeds i32", object.name))?;
        let right = left
            .checked_add(width)
            .ok_or_else(|| format!("object {:?} width overflows", object.name))?;
        let bottom = top
            .checked_add(height)
            .ok_or_else(|| format!("object {:?} height overflows", object.name))?;
        if left < 0 || top < 0 || right > map_width || bottom > map_height {
            return Err(format!(
                "object {:?} bounds {left},{top},{right},{bottom} exceed the {map_width}x{map_height} world",
                object.name
            ));
        }
        for (other, other_name) in &bounds {
            if left < other[2] && right > other[0] && top < other[3] && bottom > other[1] {
                return Err(format!(
                    "object {:?} overlaps object {other_name:?} at bounds {left},{top},{right},{bottom}",
                    object.name
                ));
            }
        }
        let object_bounds = [left, top, right, bottom];
        bounds.push((object_bounds, object.name.clone()));
        reports.push(ObjectReport {
            name: object.name.clone(),
            kind: object.kind.clone(),
            at: object.at,
            anchor: object.anchor,
            bounds: object_bounds,
        });
    }
    Ok(reports)
}

#[allow(clippy::too_many_arguments)]
fn lint_scene(
    manifest: &SceneManifest,
    map: &TileMap,
    used_width: u32,
    used_height: u32,
    packed: &[PackedTile],
    named: &BTreeMap<String, TileId>,
    used_named: &BTreeSet<String>,
    variant_grid: Option<&Grid>,
) -> LintReport {
    let mut report = LintReport::default();
    let by_id = packed
        .iter()
        .map(|tile| (tile.id, tile))
        .collect::<BTreeMap<_, _>>();
    for tile in packed {
        if tile.key.pixels.iter().all(|index| *index == 0) {
            report.warnings.push(format!(
                "blank allocation tile {} class {:?} sources {:?}",
                tile.id, tile.key.class, tile.sources
            ));
        }
    }
    let mut pixel_classes = BTreeMap::<[u8; 64], BTreeMap<String, TileId>>::new();
    for tile in packed {
        pixel_classes
            .entry(tile.key.pixels)
            .or_default()
            .insert(tile.key.class.clone(), tile.id);
    }
    for classes in pixel_classes.values().filter(|classes| classes.len() > 1) {
        let split = SemanticSplit {
            classes: classes.keys().cloned().collect(),
            tile_ids: classes.values().copied().collect(),
        };
        report.warnings.push(format!(
            "collision-class drift: identical pixels intentionally occupy tile IDs {:?} for classes {:?}",
            split.tile_ids, split.classes
        ));
        report.semantic_pixel_splits.push(split);
    }
    report.unused_named_tiles = named
        .keys()
        .filter(|name| !used_named.contains(*name))
        .cloned()
        .collect();
    for name in &report.unused_named_tiles {
        if name.contains("corner") || name.contains("endcap") {
            report.warnings.push(format!(
                "orphan corner/endcap tile {name:?} is never placed"
            ));
        }
    }

    let mut adjacency_set = BTreeSet::new();
    for y in 0..used_height {
        for x in 0..used_width {
            let a = map[y as usize * MAP_W + x as usize];
            if x + 1 < used_width {
                let b = map[y as usize * MAP_W + x as usize + 1];
                if a != 0 || b != 0 {
                    adjacency_set.insert((a, b, "east"));
                }
            }
            if y + 1 < used_height {
                let b = map[(y as usize + 1) * MAP_W + x as usize];
                if a != 0 || b != 0 {
                    adjacency_set.insert((a, b, "south"));
                }
            }
        }
    }
    for (a, b, direction) in adjacency_set {
        let legal = adjacency_legal(by_id.get(&a).copied(), by_id.get(&b).copied(), direction);
        if !legal {
            report.warnings.push(format!(
                "illegal {direction} edge pair tile {a} -> tile {b}"
            ));
        }
        report.used_adjacencies.push(AdjacencyReport {
            a,
            b,
            direction,
            legal,
        });
    }
    report.max_variant_run = variant_grid.map_or(0, max_variant_run);
    if report.max_variant_run > 4 {
        report.warnings.push(format!(
            "periodic variant cadence has a repeated run of {} cells",
            report.max_variant_run
        ));
    }
    let _ = manifest;
    report
}

fn adjacency_legal(a: Option<&PackedTile>, b: Option<&PackedTile>, direction: &str) -> bool {
    let empty = "empty";
    let (a_edge, b_edge) = match direction {
        "east" => (
            a.map_or(empty, |tile| tile.edges[1].as_str()),
            b.map_or(empty, |tile| tile.edges[3].as_str()),
        ),
        "south" => (
            a.map_or(empty, |tile| tile.edges[2].as_str()),
            b.map_or(empty, |tile| tile.edges[0].as_str()),
        ),
        _ => unreachable!(),
    };
    a_edge == "*" || b_edge == "*" || a_edge == b_edge
}

fn max_variant_run(grid: &Grid) -> u32 {
    let mut max_run = 0;
    for y in 0..grid.height {
        let mut previous = "";
        let mut run = 0;
        for x in 0..grid.width {
            let current = grid.get(x, y);
            if current == previous && current != "." {
                run += 1;
            } else {
                previous = current;
                run = u32::from(current != ".");
            }
            max_run = max_run.max(run);
        }
    }
    for x in 0..grid.width {
        let mut previous = "";
        let mut run = 0;
        for y in 0..grid.height {
            let current = grid.get(x, y);
            if current == previous && current != "." {
                run += 1;
            } else {
                previous = current;
                run = u32::from(current != ".");
            }
            max_run = max_run.max(run);
        }
    }
    max_run
}

fn used_extent(map: &TileMap) -> (u32, u32) {
    let mut width = 1;
    let mut height = 1;
    for y in 0..MAP_H {
        for x in 0..MAP_W {
            if map[y * MAP_W + x] != 0 {
                width = width.max(x as u32 + 1);
                height = height.max(y as u32 + 1);
            }
        }
    }
    (width, height)
}

fn render_atlas_png(indices: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(indices.len() * 4);
    for index in indices {
        if *index == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let [r, g, b] = PALETTE[*index as usize];
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    encode_png_rgba(&rgba, width, height)
}

fn render_map(map: &TileMap, width: u32, height: u32) -> String {
    let mut text = format!("{MAP_FORMAT_MARKER}\n");
    for y in 0..height {
        for x in 0..width {
            text.push_str(&format!("{:03x}", map[y as usize * MAP_W + x as usize]));
        }
        text.push('\n');
    }
    text
}

fn render_classes_lua(classes: &BTreeMap<String, ClassConfig>, packed: &[PackedTile]) -> String {
    let mut text = String::from(
        "-- generated by console scene compile; do not edit\nlocal M = {}\nM.classes = {\n",
    );
    for class in classes.values() {
        text.push_str(&format!(
            "  [{:?}] = {{solid={}, hazard={}, tags={{{}}}}},\n",
            class.name,
            class.solid,
            class.hazard,
            class
                .tags
                .iter()
                .map(|tag| format!("[{tag:?}]=true"))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    text.push_str("}\nM.tile_class = {\n");
    for tile in packed {
        text.push_str(&format!("  [{}] = {:?},\n", tile.id, tile.key.class));
    }
    text.push_str(
        "}\nfunction M.class(id) return M.classes[M.tile_class[id]] end\nfunction M.is_solid(id) local c=M.class(id) return c ~= nil and c.solid end\nfunction M.is_hazard(id) local c=M.class(id) return c ~= nil and c.hazard end\nreturn M\n",
    );
    text
}

fn render_layers_lua(
    layers: &[LayerData],
    ids: &BTreeMap<TileKey, TileId>,
) -> Result<String, String> {
    let mut text =
        String::from("-- generated by console scene compile; do not edit\nlocal M = {layers={}}\n");
    for layer in layers.iter().filter(|layer| {
        matches!(
            layer.role,
            LayerRole::Far | LayerRole::Mid | LayerRole::Foreground
        )
    }) {
        validate_map_rect(
            layer.offset,
            layer.width,
            layer.height,
            &format!("layer {:?}", layer.name),
        )?;
        text.push_str(&format!("M.layers[{:?}] = {{\n", layer.name));
        for y in 0..layer.height {
            for x in 0..layer.width {
                if let Some(key) = &layer.keys[(y * layer.width + x) as usize] {
                    text.push_str(&format!(
                        "  {{tile={},x={},y={}}},\n",
                        ids[key],
                        layer.offset[0] + x,
                        layer.offset[1] + y
                    ));
                }
            }
        }
        text.push_str("}\n");
    }
    text.push_str(
        "function M.draw_visible(name,cx,cy,cw,ch)\n  local layer=M.layers[name] or {}\n  for i=1,#layer do local c=layer[i] if c.x>=cx and c.y>=cy and c.x<cx+cw and c.y<cy+ch then spr(c.tile,c.x*8,c.y*8) end end\nend\nreturn M\n",
    );
    Ok(text)
}

fn render_objects_lua(objects: &[ObjectReport]) -> String {
    let mut text = String::from("-- generated by console scene compile; do not edit\nreturn {\n");
    for object in objects {
        text.push_str(&format!(
            "  {{name={:?},kind={:?},x={},y={},anchor_x={},anchor_y={},bounds={{{},{},{},{}}}}},\n",
            object.name,
            object.kind,
            object.at[0],
            object.at[1],
            object.anchor[0],
            object.anchor[1],
            object.bounds[0],
            object.bounds[1],
            object.bounds[2],
            object.bounds[3]
        ));
    }
    text.push_str("}\n");
    text
}

fn packed_report(tile: &PackedTile) -> PackedTileReport {
    PackedTileReport {
        id: tile.id,
        atlas_cell: [
            u32::from(tile.id) % SHEET_TILES as u32,
            u32::from(tile.id) / SHEET_TILES as u32,
        ],
        class: tile.key.class.clone(),
        names: tile.names.clone(),
        sources: tile.sources.clone(),
        edges: tile.edges.clone(),
        blank: tile.key.pixels.iter().all(|index| *index == 0),
    }
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

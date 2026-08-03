//! Map authoring tools (SPEC.md "Tile map authoring", `console_core::Cart::map()`):
//! inspection renders + numeric lint in [`view`], in-place cart transforms
//! (`poke`, `edit`) in [`transform`], shared region resolution here — the
//! same three-layer split `sprite/mod.rs` uses, applied to the `__map__`
//! tile grid instead of the sprite sheet. Where the pixel path itself is
//! identical (rendering one 8x8 tile), `view.rs` reuses `sprite::view`'s
//! primitives directly rather than duplicating them.

pub mod transform;
pub mod view;

use console_core::{MAP_H, MAP_W, TileMap};

/// Parse a `[cx,cy,cw,ch]` region argument, in map cell coordinates.
/// `None` (the argument was omitted) resolves to [`used_extent`] — the
/// bounding box of non-zero cells — or a single cell at the origin if the
/// map has none at all, so `render`/`dump`/`poke` always have a sensible
/// region to work with even before a cart has any terrain.
pub fn parse_region(s: Option<&str>, tiles: &TileMap) -> Result<(u32, u32, u32, u32), String> {
    match s {
        Some(s) => {
            let parts: Vec<&str> = s.split(',').collect();
            if parts.len() != 4 {
                return Err(format!("region must be cx,cy,cw,ch: {s:?}"));
            }
            let nums: Result<Vec<u32>, _> = parts.iter().map(|p| p.trim().parse::<u32>()).collect();
            let nums = nums.map_err(|e| format!("bad region {s:?}: {e}"))?;
            let (cx, cy, cw, ch) = (nums[0], nums[1], nums[2], nums[3]);
            validate_region(cx, cy, cw, ch)?;
            Ok((cx, cy, cw, ch))
        }
        None => Ok(default_region(tiles)),
    }
}

/// Same bound checks `parse_region` applies to an explicit region, factored
/// out so `map edit`'s ops (which require an explicit region — see
/// `transform.rs`) validate identically.
pub fn validate_region(cx: u32, cy: u32, cw: u32, ch: u32) -> Result<(), String> {
    let (map_w, map_h) = (MAP_W as u32, MAP_H as u32);
    if cw == 0 || ch == 0 {
        return Err(format!("region must be at least 1x1 cells, got {cw}x{ch}"));
    }
    if cx >= map_w || cy >= map_h || cx + cw > map_w || cy + ch > map_h {
        return Err(format!(
            "region cx={cx} cy={cy} cw={cw} ch={ch} falls outside the {map_w}x{map_h} map"
        ));
    }
    Ok(())
}

/// The bounding box of non-zero cells, as `(cx, cy, cw, ch)`. `None` when
/// every cell in `tiles` is empty (tile 0).
pub fn used_extent(tiles: &TileMap) -> Option<(u32, u32, u32, u32)> {
    let mut bbox: Option<(u32, u32, u32, u32)> = None;
    for y in 0..MAP_H {
        for x in 0..MAP_W {
            if tiles[y * MAP_W + x] == 0 {
                continue;
            }
            let (x, y) = (x as u32, y as u32);
            bbox = Some(match bbox {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }
    bbox.map(|(x0, y0, x1, y1)| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// The default region when a command omits `[cx,cy,cw,ch]`.
fn default_region(tiles: &TileMap) -> (u32, u32, u32, u32) {
    used_extent(tiles).unwrap_or((0, 0, 1, 1))
}

/// CLI dispatch for `console-agent map <cmd> ...`. Returns the process exit
/// code. `render`/`dump`/`lint` are wired by `view.rs`, `edit`/`poke` by
/// `transform.rs`.
pub fn cli_map(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("render") | Some("dump") | Some("lint") => view::cli_view(args),
        Some("edit") => transform::cli_edit(&args[1..]),
        Some("poke") => transform::cli_poke(&args[1..]),
        _ => {
            eprintln!("{MAP_USAGE}");
            2
        }
    }
}

pub const MAP_USAGE: &str = "\
usage:
  console-agent map render <cart> [cx,cy,cw,ch] [--zoom Z] [--grid] [--ids] -o out.png
  console-agent map dump   <cart> [cx,cy,cw,ch]
  console-agent map poke   <cart> [cx,cy,cw,ch] (--rows <hex,hex,...> | --stdin) [--dry-run]
  console-agent map lint   <cart>
  console-agent map edit   <cart> <copy|shift|fill|clear> ... [--dry-run]
  (region cx,cy,cw,ch defaults to the used extent -- the bounding box of
   non-zero cells -- when omitted from render/dump/poke; map edit requires
   it explicitly. Rows are 2 hex chars per cell, matching __map__ itself.)";

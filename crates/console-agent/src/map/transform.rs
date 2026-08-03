//! In-place `__map__` cart transforms: `poke` (row-based writes, mirroring
//! `sprite poke`) and `edit` (`copy`/`shift`/`fill`/`clear` region
//! transforms, mirroring `sprite edit`'s verbs and arg style) — this bone's
//! write half of the map-agent-tooling pair, `view.rs` being the read half.
//!
//! Cell math runs against a [`Cart`] parsed the normal way (for region
//! validation), but the file itself is rewritten by locating the
//! `__map__` section's raw text lines *positionally* — mirroring
//! `console_core::cart`'s section/row scanning exactly (including its `#`
//! comment lines, which `__sprites__` doesn't have) — and overwriting only
//! the specific hex rows whose cells actually changed. Every other byte of
//! the cart file survives verbatim.
//!
//! Changed rows are always rewritten at the full 128-cell (256 hex char)
//! width, matching `sprite::transform::encode_row`'s convention: a touched
//! row may grow past whatever short form it had, but the rewrite still only
//! touches the specific file lines whose cells actually changed.
//!
//! If the cart has no `__map__` section at all, `poke`/`edit` create one,
//! positioned right after the cart's last `__sprites__` section, or — if it
//! has none — right before its first `__gfx_meta__` section, or at EOF if it
//! has neither. That is `__map__`'s slot in SPEC.md's cart anatomy (meta,
//! lua, sprites, map, gfx_meta, instruments, sfx, music).
//!
//! As a last line of defense, the rewritten text is re-parsed with
//! `Cart::parse` before anything is written to disk; a parse failure aborts
//! with no write.

use console_core::{Cart, MAP_H, MAP_W, TileMap};

use super::{parse_region, validate_region};

// ---------------------------------------------------------------------
// poke: row-based cell writes
// ---------------------------------------------------------------------

pub const POKE_USAGE: &str = "\
usage:
  console-agent map poke <cart> [cx,cy,cw,ch] --rows <hex,hex,...> [--dry-run]
  console-agent map poke <cart> [cx,cy,cw,ch] --stdin [--dry-run]
  (rows run top to bottom, one per region row, each exactly 2*cw hex digits;
   with --stdin, lines starting with '#' are skipped, so `map dump`'s own
   output pipes straight into `--stdin`; region defaults to the used extent
   when omitted)";

/// CLI entry for `map poke`. `args[0]` is the cart path, an optional
/// `[cx,cy,cw,ch]` may follow; `--rows`/`--stdin` and `--dry-run` may appear
/// anywhere.
pub fn cli_poke(args: &[String]) -> i32 {
    let mut args = args.to_vec();
    let dry_run = take_flag(&mut args, "--dry-run");
    let use_stdin = take_flag(&mut args, "--stdin");
    let rows_csv = take_value(&mut args, "--rows");

    if args.is_empty() {
        eprintln!("{POKE_USAGE}");
        return 2;
    }
    let cart_path = args.remove(0);
    let region_arg = if args.is_empty() { None } else { Some(args.remove(0)) };
    if !args.is_empty() {
        eprintln!("error: map poke: unexpected extra arguments: {args:?}\n{POKE_USAGE}");
        return 2;
    }

    let rows: Vec<String> = match (rows_csv, use_stdin) {
        (Some(_), true) => {
            eprintln!("error: poke: pass either --rows or --stdin, not both");
            return 2;
        }
        (Some(csv), false) => csv.split(',').map(str::to_string).collect(),
        (None, true) => match read_stdin_rows() {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        },
        (None, false) => {
            eprintln!("error: poke requires --rows <hex,hex,...> or --stdin\n{POKE_USAGE}");
            return 2;
        }
    };

    let text = match std::fs::read_to_string(&cart_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read {cart_path:?}: {e}");
            return 2;
        }
    };

    apply_edit_result(&cart_path, run_poke(&text, region_arg.as_deref(), &rows), dry_run)
}

/// Read poke rows from stdin: one row per line, skipping any line starting
/// with `#` so `map dump`'s header passes through harmlessly.
fn read_stdin_rows() -> Result<Vec<String>, String> {
    use std::io::BufRead;
    std::io::stdin()
        .lock()
        .lines()
        .filter(|line| !matches!(line, Ok(l) if l.trim_start().starts_with('#')))
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| format!("reading stdin: {e}"))
}

/// Parse `text` as a cart, resolve `region_arg` (or default to the used
/// extent), validate `rows` against it exactly, overwrite those cells, and
/// compute the resulting cart text. Pure — no file I/O.
fn run_poke(text: &str, region_arg: Option<&str>, rows: &[String]) -> Result<EditResult, String> {
    let cart = Cart::parse(text).map_err(|e| format!("cart: {e}"))?;
    let region = parse_region(region_arg, cart.map())?;
    let (cx, cy, cw, ch) = region;
    let (cw_us, ch_us) = (cw as usize, ch as usize);

    if rows.len() != ch_us {
        return Err(format!(
            "poke: region is {cw}x{ch} cell(s), expected {ch_us} row(s), got {}",
            rows.len()
        ));
    }
    let mut values = vec![0u8; cw_us * ch_us];
    for (j, row) in rows.iter().enumerate() {
        let bytes = row.as_bytes();
        if bytes.len() != cw_us * 2 {
            return Err(format!(
                "poke: row {j}: region is {cw} cell(s) wide, expected {} hex char(s) (2 per cell), got {} ({row:?})",
                cw_us * 2,
                bytes.len()
            ));
        }
        for i in 0..cw_us {
            let hi = (bytes[i * 2] as char).to_digit(16).ok_or_else(|| {
                format!("poke: row {j}: invalid hex digit {:?} at column {} ({row:?})", bytes[i * 2] as char, i * 2)
            })?;
            let lo = (bytes[i * 2 + 1] as char).to_digit(16).ok_or_else(|| {
                format!(
                    "poke: row {j}: invalid hex digit {:?} at column {} ({row:?})",
                    bytes[i * 2 + 1] as char,
                    i * 2 + 1
                )
            })?;
            values[j * cw_us + i] = (hi * 16 + lo) as u8;
        }
    }

    let mut tiles: Box<TileMap> = Box::new(*cart.map());
    for j in 0..ch_us {
        for i in 0..cw_us {
            tiles[(cy as usize + j) * MAP_W + (cx as usize + i)] = values[j * cw_us + i];
        }
    }

    finish_edit(text, cart.map(), &tiles, "poke")
}

// ---------------------------------------------------------------------
// edit: copy / shift / fill / clear region transforms
// ---------------------------------------------------------------------

pub const EDIT_USAGE: &str = "\
usage:
  console-agent map edit <cart> copy  <cx,cy,cw,ch> <dest_cx,dest_cy> [--dry-run]
  console-agent map edit <cart> shift <cx,cy,cw,ch> [--dx <n>] [--dy <n>] [--dry-run]
  console-agent map edit <cart> fill  <cx,cy,cw,ch> <tile-hex> [--dry-run]
  console-agent map edit <cart> clear <cx,cy,cw,ch> [--dry-run]
  (region is required and explicit -- unlike render/dump/poke it never
   defaults to the used extent; tile ids are 1-2 hex digits, 00-ff, the
   __map__ alphabet; shift drops cells that fall off the region and fills
   vacated cells with tile 0)";

/// CLI entry for `map edit`. `args[0]` is the cart path, `args[1]` the op
/// name (`copy|shift|fill|clear`); the rest are op-specific arguments.
/// `--dry-run` may appear anywhere and is stripped first.
pub fn cli_edit(args: &[String]) -> i32 {
    let mut args = args.to_vec();
    let dry_run = take_flag(&mut args, "--dry-run");

    if args.len() < 2 {
        eprintln!("{EDIT_USAGE}");
        return 2;
    }
    let cart_path = args.remove(0);
    let op = args.remove(0);

    let text = match std::fs::read_to_string(&cart_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read {cart_path:?}: {e}");
            return 2;
        }
    };

    apply_edit_result(&cart_path, run_edit(&text, &op, &args), dry_run)
}

fn run_edit(text: &str, op: &str, op_args: &[String]) -> Result<EditResult, String> {
    let cart = Cart::parse(text).map_err(|e| format!("cart: {e}"))?;
    let mut tiles: Box<TileMap> = Box::new(*cart.map());

    match op {
        "copy" => run_copy(&mut tiles, op_args)?,
        "shift" => run_shift(&mut tiles, op_args)?,
        "fill" => run_fill(&mut tiles, op_args)?,
        "clear" => run_clear(&mut tiles, op_args)?,
        other => return Err(format!("unknown edit op {other:?}; expected copy|shift|fill|clear")),
    }

    finish_edit(text, cart.map(), &tiles, "edit")
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        args.remove(pos);
        true
    } else {
        false
    }
}

fn take_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.remove(pos);
    if pos < args.len() { Some(args.remove(pos)) } else { None }
}

/// Parse and bound-check an explicit `cx,cy,cw,ch` region argument for `map
/// edit` (which, unlike `render`/`dump`/`poke`, never defaults it to the
/// used extent — a destructive region transform should always name its
/// region explicitly).
fn required_region(s: &str) -> Result<(u32, u32, u32, u32), String> {
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

fn parse_tile_hex(s: &str) -> Result<u8, String> {
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u8::from_str_radix(hex, 16).map_err(|e| format!("bad tile id {s:?} (want 1-2 hex digits, 00-ff): {e}"))
}

fn parse_point(s: &str) -> Result<(u32, u32), String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return Err(format!("destination must be cx,cy: {s:?}"));
    }
    let x = parts[0].trim().parse::<u32>().map_err(|e| format!("bad destination {s:?}: {e}"))?;
    let y = parts[1].trim().parse::<u32>().map_err(|e| format!("bad destination {s:?}: {e}"))?;
    Ok((x, y))
}

fn one_arg<'a>(args: &'a [String], op: &str) -> Result<&'a str, String> {
    match args {
        [only] => Ok(only.as_str()),
        [] => Err(format!("{op} requires a region cx,cy,cw,ch")),
        _ => Err(format!("{op}: unexpected extra arguments: {args:?}")),
    }
}

fn run_shift(tiles: &mut TileMap, args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    let dx = match take_value(&mut args, "--dx") {
        Some(v) => v.parse::<i64>().map_err(|e| format!("bad --dx: {e}"))?,
        None => 0,
    };
    let dy = match take_value(&mut args, "--dy") {
        Some(v) => v.parse::<i64>().map_err(|e| format!("bad --dy: {e}"))?,
        None => 0,
    };
    let region = required_region(one_arg(&args, "shift")?)?;
    op_shift(tiles, region, dx, dy);
    Ok(())
}

fn run_fill(tiles: &mut TileMap, args: &[String]) -> Result<(), String> {
    let [region_str, tile_str] = args else {
        return Err("fill requires a region and a tile id: fill <cx,cy,cw,ch> <tile-hex>".into());
    };
    let region = required_region(region_str)?;
    let tile = parse_tile_hex(tile_str)?;
    op_fill(tiles, region, tile);
    Ok(())
}

fn run_clear(tiles: &mut TileMap, args: &[String]) -> Result<(), String> {
    let region = required_region(one_arg(args, "clear")?)?;
    op_fill(tiles, region, 0);
    Ok(())
}

fn run_copy(tiles: &mut TileMap, args: &[String]) -> Result<(), String> {
    let [region_str, dest_str] = args else {
        return Err("copy requires a region and a destination: copy <cx,cy,cw,ch> <dest_cx,dest_cy>".into());
    };
    let region = required_region(region_str)?;
    let dest = parse_point(dest_str)?;
    let (_, _, cw, ch) = region;
    let (dcx, dcy) = dest;
    if dcx + cw > MAP_W as u32 || dcy + ch > MAP_H as u32 {
        return Err(format!(
            "copy destination {dcx},{dcy} plus region size {cw}x{ch} falls outside the {}x{} map",
            MAP_W, MAP_H
        ));
    }
    op_copy(tiles, region, dest);
    Ok(())
}

fn idx(cx: u32, cy: u32) -> usize {
    cy as usize * MAP_W + cx as usize
}

/// Shift `region`'s content by `(dx, dy)`. Cells whose source would fall
/// outside the region are dropped (no wrap); a destination cell with no
/// valid source becomes tile 0. Mirrors `sprite::transform::op_shift`
/// without its `--wrap` option (map shift never wraps).
fn op_shift(tiles: &mut TileMap, region: (u32, u32, u32, u32), dx: i64, dy: i64) {
    let (cx0, cy0, cw, ch) = region;
    let (wi, hi) = (cw as i64, ch as i64);
    let mut src = vec![0u8; (cw * ch) as usize];
    for j in 0..ch {
        for i in 0..cw {
            src[(j * cw + i) as usize] = tiles[idx(cx0 + i, cy0 + j)];
        }
    }
    for j in 0..hi {
        for i in 0..wi {
            let (si, sj) = (i - dx, j - dy);
            let v = if (0..wi).contains(&si) && (0..hi).contains(&sj) {
                src[(sj * wi + si) as usize]
            } else {
                0
            };
            tiles[idx(cx0 + i as u32, cy0 + j as u32)] = v;
        }
    }
}

fn op_fill(tiles: &mut TileMap, region: (u32, u32, u32, u32), tile: u8) {
    let (cx0, cy0, cw, ch) = region;
    for j in 0..ch {
        for i in 0..cw {
            tiles[idx(cx0 + i, cy0 + j)] = tile;
        }
    }
}

fn op_copy(tiles: &mut TileMap, region: (u32, u32, u32, u32), dest: (u32, u32)) {
    let (cx0, cy0, cw, ch) = region;
    let (dcx, dcy) = dest;
    let mut buf = vec![0u8; (cw * ch) as usize];
    for j in 0..ch {
        for i in 0..cw {
            buf[(j * cw + i) as usize] = tiles[idx(cx0 + i, cy0 + j)];
        }
    }
    for j in 0..ch {
        for i in 0..cw {
            tiles[idx(dcx + i, dcy + j)] = buf[(j * cw + i) as usize];
        }
    }
}

// ---------------------------------------------------------------------
// Shared: compute + apply the result
// ---------------------------------------------------------------------

/// The outcome of computing an edit against cart text, before any I/O.
enum EditResult {
    /// No cell in the map actually changed; nothing to write.
    Unchanged,
    /// `new_text` is the full rewritten cart text (only `__map__` lines
    /// touched, or a freshly-inserted `__map__` section); `report` is the
    /// dry-run listing of `(1-based line number, new line content)` for
    /// each row that changed, in row order.
    Changed { new_text: String, report: Vec<(usize, String)> },
}

/// Compute the rewrite (if any), re-parsing to confirm validity before
/// handing back a `Changed` result. Shared tail of `run_poke` and
/// `run_edit`; `what` only flavors the "would produce an invalid cart"
/// error message.
fn finish_edit(text: &str, old_tiles: &TileMap, new_tiles: &TileMap, what: &str) -> Result<EditResult, String> {
    match rewrite_map(text, old_tiles, new_tiles) {
        None => Ok(EditResult::Unchanged),
        Some((new_text, report)) => {
            Cart::parse(&new_text)
                .map_err(|e| format!("{what} would produce an invalid cart (not written): {e}"))?;
            Ok(EditResult::Changed { new_text, report })
        }
    }
}

/// Shared tail of `cli_edit` and `cli_poke`: either print the dry-run report
/// or write the new text to `cart_path`, translating the result to a
/// process exit code.
fn apply_edit_result(cart_path: &str, result: Result<EditResult, String>, dry_run: bool) -> i32 {
    match result {
        Ok(EditResult::Unchanged) => 0,
        Ok(EditResult::Changed { new_text, report }) => {
            if dry_run {
                for (lineno, content) in &report {
                    println!("{lineno}: {content}");
                }
                0
            } else {
                match std::fs::write(cart_path, &new_text) {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("error: cannot write {cart_path:?}: {e}");
                        1
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

// ---------------------------------------------------------------------
// Text-level rewrite: locate __map__ rows positionally and touch only the
// lines that changed; create the section if it is missing.
// ---------------------------------------------------------------------

/// `__name__` on a line of its own (surrounding whitespace tolerated).
/// Mirrors `console_core::cart::section_header` exactly (private there, and
/// this module may not edit `cart.rs`) — correct rewrite depends on finding
/// section boundaries the same way the parser does.
fn section_header(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix("__")?.strip_suffix("__")?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(inner.to_ascii_lowercase())
}

fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// Where `__map__`'s rows live in the raw file text: `row_lines[y]` is the
/// file line index of logical map row `y` (shorter than [`MAP_H`] if the
/// section has fewer non-blank, non-comment lines than that — missing rows
/// are implicitly all tile 0, per `console_core::cart::parse_map`).
/// `insert_at` is where to append more rows once `row_lines` runs out: the
/// end of the *last* `__map__` section's body (repeated sections
/// concatenate, mirroring the parser); `None` if the cart has no `__map__`
/// section at all.
fn locate_map_layout(lines: &[&str]) -> (Vec<usize>, Option<usize>) {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut current: Option<String> = None;
    let mut body_start = 0usize;

    for (i, raw) in lines.iter().enumerate() {
        if let Some(name) = section_header(strip_cr(raw)) {
            if let Some(prev) = current.take() {
                if prev == "map" {
                    ranges.push((body_start, i));
                }
            }
            current = Some(name);
            body_start = i + 1;
        }
    }
    if let Some(prev) = current {
        if prev == "map" {
            ranges.push((body_start, lines.len()));
        }
    }

    let mut row_lines = Vec::new();
    'ranges: for &(start, end) in &ranges {
        for (i, raw) in lines.iter().enumerate().take(end).skip(start) {
            if row_lines.len() >= MAP_H {
                break 'ranges;
            }
            let t = strip_cr(raw).trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            row_lines.push(i);
        }
    }

    let insert_at = ranges.last().map(|&(_, end)| end);
    (row_lines, insert_at)
}

/// Where to splice a brand-new `__map__` section when the cart has none:
/// right after the last `__sprites__` section's body ends, or — if there is
/// no `__sprites__` section either — right before the first `__gfx_meta__`
/// section's header, or at the end of the file if the cart has neither.
fn new_section_anchor(lines: &[&str]) -> usize {
    let mut sprites_end: Option<usize> = None;
    let mut gfx_meta_start: Option<usize> = None;
    let mut current: Option<String> = None;

    for (i, raw) in lines.iter().enumerate() {
        if let Some(name) = section_header(strip_cr(raw)) {
            if let Some(prev) = current.take() {
                if prev == "sprites" {
                    sprites_end = Some(i);
                }
            }
            if name == "gfx_meta" && gfx_meta_start.is_none() {
                gfx_meta_start = Some(i);
            }
            current = Some(name);
        }
    }
    if let Some(prev) = current {
        if prev == "sprites" {
            sprites_end = Some(lines.len());
        }
    }

    sprites_end.or(gfx_meta_start).unwrap_or(lines.len())
}

/// Insert a `header` line plus `rows` at `pos` in `owned`, adding blank-line
/// padding around the new block only where the surrounding lines aren't
/// already blank (so it reads like a hand-authored cart, not a text dump).
/// When `pos` is exactly EOF, trailing blank lines are trimmed first,
/// mirroring `sprite::transform`'s fresh-section convention.
fn splice_new_section(owned: &mut Vec<String>, pos: usize, header: &str, rows: Vec<String>) {
    let mut pos = pos;
    if pos == owned.len() {
        while owned.last().map(String::as_str) == Some("") {
            owned.pop();
        }
        pos = owned.len();
    }

    let mut block = Vec::new();
    if pos > 0 && !owned[pos - 1].trim().is_empty() {
        block.push(String::new());
    }
    block.push(header.to_string());
    block.extend(rows);
    if pos < owned.len() {
        if !owned[pos].trim().is_empty() {
            block.push(String::new());
        }
    } else {
        block.push(String::new());
    }
    owned.splice(pos..pos, block);
}

fn encode_row(tiles: &TileMap, y: usize) -> String {
    let mut s = String::with_capacity(MAP_W * 2);
    for x in 0..MAP_W {
        s.push_str(&format!("{:02x}", tiles[y * MAP_W + x]));
    }
    s
}

fn row_slice(tiles: &TileMap, y: usize) -> &[u8] {
    &tiles[y * MAP_W..(y + 1) * MAP_W]
}

/// Line-ending hint for freshly-inserted lines: mirror whatever the line
/// right before the insertion point uses (CRLF vs LF).
fn eol_hint(lines: &[&str], insert_at: Option<usize>) -> &'static str {
    match insert_at {
        Some(pos) if pos > 0 && lines[pos - 1].ends_with('\r') => "\r",
        _ => "",
    }
}

/// Compute the rewritten cart text for a `tiles` change against the
/// original `text`, touching only `__map__` rows that actually changed (and
/// creating the section per [`new_section_anchor`] if it is missing).
/// Returns `None` if `old_tiles == new_tiles` (nothing to do).
fn rewrite_map(
    text: &str,
    old_tiles: &TileMap,
    new_tiles: &TileMap,
) -> Option<(String, Vec<(usize, String)>)> {
    let mut changed_rows: Vec<usize> =
        (0..MAP_H).filter(|&y| row_slice(old_tiles, y) != row_slice(new_tiles, y)).collect();
    if changed_rows.is_empty() {
        return None;
    }
    changed_rows.sort_unstable();
    let max_row = *changed_rows.last().unwrap();

    let lines: Vec<&str> = text.split('\n').collect();
    let (row_lines, insert_at) = locate_map_layout(&lines);
    let mut owned: Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();

    for &y in &changed_rows {
        if y < row_lines.len() {
            let file_line = row_lines[y];
            let eol = if lines[file_line].ends_with('\r') { "\r" } else { "" };
            owned[file_line] = format!("{}{eol}", encode_row(new_tiles, y));
        }
    }

    if max_row >= row_lines.len() {
        let append_from = row_lines.len();
        let zero_row = "0".repeat(MAP_W * 2);
        let gap_rows: Vec<String> = (append_from..=max_row)
            .map(|y| if changed_rows.binary_search(&y).is_ok() { encode_row(new_tiles, y) } else { zero_row.clone() })
            .collect();

        match insert_at {
            Some(pos) => {
                let eol = eol_hint(&lines, Some(pos));
                for (offset, row) in gap_rows.into_iter().enumerate() {
                    owned.insert(pos + offset, format!("{row}{eol}"));
                }
            }
            None => {
                let anchor = new_section_anchor(&lines);
                splice_new_section(&mut owned, anchor, "__map__", gap_rows);
            }
        }
    }

    let new_text = owned.join("\n");

    // Recompute against the *new* text so reported line numbers reflect the
    // file as it will actually be written.
    let new_lines: Vec<&str> = new_text.split('\n').collect();
    let (new_row_lines, _) = locate_map_layout(&new_lines);
    let report = changed_rows
        .iter()
        .map(|&y| {
            let file_line = new_row_lines[y];
            (file_line + 1, new_lines[file_line].to_string())
        })
        .collect();

    Some((new_text, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_header_matches_cart_rs() {
        assert_eq!(section_header("__map__"), Some("map".into()));
        assert_eq!(section_header("  __MAP__  "), Some("map".into()));
        assert_eq!(section_header("not a header"), None);
        assert_eq!(section_header("____"), None);
    }

    #[test]
    fn rewrite_appends_new_section_when_absent() {
        let text = "__lua__\nfunction _init() end\n";
        let old = [0u8; MAP_W * MAP_H];
        let mut new = [0u8; MAP_W * MAP_H];
        new[0] = 5;

        let (new_text, report) = rewrite_map(text, &old, &new).expect("row 0 changed");
        assert_eq!(report.len(), 1);

        let cart = Cart::parse(&new_text).expect("still a valid cart");
        assert_eq!(cart.map()[0], 5);
        assert!(new_text.contains("__map__"));
    }

    #[test]
    fn rewrite_inserts_between_sprites_and_gfx_meta() {
        let text = "__lua__\nfunction _init() end\n\n__sprites__\na0000000\n\n__gfx_meta__\nsprite p rect=0,0 size=1x1\n";
        let old = [0u8; MAP_W * MAP_H];
        let mut new = old;
        new[0] = 0x1f;

        let (new_text, _report) = rewrite_map(text, &old, &new).expect("row 0 changed");
        let cart = Cart::parse(&new_text).expect("still a valid cart");
        assert_eq!(cart.map()[0], 0x1f);

        let map_pos = new_text.find("__map__").expect("map section present");
        let sprites_pos = new_text.find("__sprites__").unwrap();
        let gfx_pos = new_text.find("__gfx_meta__").unwrap();
        assert!(sprites_pos < map_pos && map_pos < gfx_pos, "{new_text}");
    }
}

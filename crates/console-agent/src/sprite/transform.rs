//! In-place cart transforms (shift/flip/rotate/copy/clear) — SPEC.md
//! "Sprite & animation authoring (PoC v1)" > "Transforms".
//!
//! Pixel math runs against a [`Cart`] parsed the normal way (for target
//! resolution and validation), but the file itself is rewritten by locating
//! the `__sprites__` section's raw text lines *positionally* — mirroring
//! `console_core::cart`'s section/row scanning exactly — and overwriting
//! only the specific palette-text rows whose pixels actually changed. Every other
//! byte of the cart file (other sections, comments, blank lines, ordering,
//! line endings, trailing whitespace) survives verbatim, because we only
//! ever touch the lines we've identified as changed; everything else is
//! copied through untouched.
//!
//! As a last line of defense against corrupting a cart, the rewritten text
//! is re-parsed with `Cart::parse` before anything is written to disk; a
//! parse failure aborts with no write.

use console_core::{Cart, SHEET_W, SpriteSheet, color_char, parse_color_char};

use super::{frame_pixel_rect, parse_target, resolve_rect};

/// CLI entry for `sprite edit`. `args[0]` is the cart path, `args[1]` the op
/// name (`shift|flip|rotate|copy|clear`); the rest are op-specific
/// arguments. `--dry-run` may appear anywhere and is stripped first.
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

pub const EDIT_USAGE: &str = "\
usage:
  console-agent sprite edit <cart> shift  <target> [--frame N] [--dx <n>] [--dy <n>] [--wrap] [--dry-run]
  console-agent sprite edit <cart> flip   <target> [--frame N] --horizontal|--vertical [--dry-run]
  console-agent sprite edit <cart> rotate <target> [--frame N] --cw|--ccw [--dry-run]
  console-agent sprite edit <cart> copy   <src> <dst> [--dry-run]
  console-agent sprite edit <cart> clear  <target> [--frame N] [--dry-run]
  (targets: sprite name, anim name, or tile rect tx,ty,w,h; copy endpoints:
   sprite[:frame] or tx,ty,w,h)";

/// Shared tail of `cli_edit` and `cli_poke`: either print the dry-run report
/// or write the new text to `cart_path`, translating the result to a process
/// exit code.
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
// poke: row-based pixel writes (SPEC.md "Sprite & animation authoring" —
// the write half of the dump/poke pair; `dump` lives in view.rs).
// ---------------------------------------------------------------------

pub const POKE_USAGE: &str = "\
usage:
  console-agent sprite poke <cart> <target> [--frame N] --rows <pixels,pixels,...> [--dry-run]
  console-agent sprite poke <cart> <target> [--frame N] --stdin [--dry-run]
  (rows run top to bottom, one per source row, each exactly as many palette
   characters as the region is wide; with --stdin, lines starting with '#' are
   skipped, so `sprite dump`'s own output pipes straight into `--stdin`;
   targets: sprite name, anim name, or tile rect tx,ty,w,h)";

/// CLI entry for `sprite poke`. `args[0]` is the cart path, the remaining
/// positional is the `<target>`; `--frame`, `--rows`/`--stdin` and
/// `--dry-run` may appear anywhere.
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

    let frame = match take_frame(&mut args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let target_str = match take_target(&args, "poke") {
        Ok(t) => t.to_string(),
        Err(e) => {
            eprintln!("error: {e}\n{POKE_USAGE}");
            return 2;
        }
    };

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

    apply_edit_result(
        &cart_path,
        run_poke(&text, &target_str, frame, &rows),
        dry_run,
    )
}

/// Read poke rows from stdin: one row per line, in order, skipping any line
/// starting with `#` (a comment, matching the cart format's own convention)
/// so `sprite dump`'s header line passes through harmlessly when piped
/// straight into `poke --stdin`.
fn read_stdin_rows() -> Result<Vec<String>, String> {
    use std::io::BufRead;
    std::io::stdin()
        .lock()
        .lines()
        .filter(|line| !matches!(line, Ok(l) if l.trim_start().starts_with('#')))
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| format!("reading stdin: {e}"))
}

/// Parse `text` as a cart, validate `rows` against `target`'s resolved
/// region (exact row count, exact row width, valid palette characters), overwrite
/// those pixels, and compute the resulting cart text via the same rewrite
/// path `sprite edit` uses. Pure — no file I/O — so it is directly testable.
fn run_poke(
    text: &str,
    target_str: &str,
    frame: u8,
    rows: &[String],
) -> Result<EditResult, String> {
    let cart = Cart::parse(text).map_err(|e| format!("cart: {e}"))?;
    let (x0, y0, w, h) = resolve_rect(&cart, target_str, frame)?;
    let (w, h) = (w as usize, h as usize);

    if rows.len() != h {
        return Err(format!(
            "poke: region is {w}x{h}, expected {h} row(s), got {}",
            rows.len()
        ));
    }
    let mut values = vec![0u8; w * h];
    for (j, row) in rows.iter().enumerate() {
        let chars: Vec<char> = row.chars().collect();
        if chars.len() != w {
            return Err(format!(
                "poke: row {j}: region is {w} pixel(s) wide, expected {w} palette character(s), got {} ({row:?})",
                chars.len()
            ));
        }
        for (i, ch) in chars.into_iter().enumerate() {
            let v = parse_color_char(ch).ok_or_else(|| {
                format!("poke: row {j}: invalid palette character {ch:?} at column {i} ({row:?})")
            })?;
            values[j * w + i] = v;
        }
    }

    let mut sheet: SpriteSheet = *cart.sprites();
    for j in 0..h {
        for i in 0..w {
            sheet[idx(x0 + i as u32, y0 + j as u32)] = values[j * w + i];
        }
    }

    match rewrite_sprites(text, cart.sprites(), &sheet) {
        None => Ok(EditResult::Unchanged),
        Some((new_text, report)) => {
            Cart::parse(&new_text)
                .map_err(|e| format!("poke would produce an invalid cart (not written): {e}"))?;
            Ok(EditResult::Changed { new_text, report })
        }
    }
}

/// The outcome of computing an edit against cart text, before any I/O.
enum EditResult {
    /// No pixel in the sheet actually changed; nothing to write.
    Unchanged,
    /// `new_text` is the full rewritten cart text (only `__sprites__` lines
    /// touched); `report` is the dry-run listing of `(1-based line number,
    /// new line content)` for each row that changed, in row order.
    Changed {
        new_text: String,
        report: Vec<(usize, String)>,
    },
}

/// Parse `text` as a cart, apply `op` with `op_args`, and compute the
/// resulting cart text (if anything changed). Pure — does no file I/O, so
/// it's directly testable. Never returns a `Changed` result unless the new
/// text has also been confirmed to re-parse successfully.
fn run_edit(text: &str, op: &str, op_args: &[String]) -> Result<EditResult, String> {
    let cart = Cart::parse(text).map_err(|e| format!("cart: {e}"))?;

    let mut sheet: SpriteSheet = *cart.sprites();
    match op {
        "shift" => run_shift(&cart, &mut sheet, op_args)?,
        "flip" => run_flip(&cart, &mut sheet, op_args)?,
        "rotate" => run_rotate(&cart, &mut sheet, op_args)?,
        "copy" => run_copy(&cart, &mut sheet, op_args)?,
        "clear" => run_clear(&cart, &mut sheet, op_args)?,
        other => {
            return Err(format!(
                "unknown edit op {other:?}; expected shift|flip|rotate|copy|clear"
            ));
        }
    }

    match rewrite_sprites(text, cart.sprites(), &sheet) {
        None => Ok(EditResult::Unchanged),
        Some((new_text, report)) => {
            // Belt and braces: never hand back text we haven't confirmed is
            // still a valid cart.
            Cart::parse(&new_text)
                .map_err(|e| format!("edit would produce an invalid cart (not written): {e}"))?;
            Ok(EditResult::Changed { new_text, report })
        }
    }
}

// ---------------------------------------------------------------------
// Op argument parsing + pixel math
// ---------------------------------------------------------------------

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
    if pos < args.len() {
        Some(args.remove(pos))
    } else {
        None
    }
}

fn take_frame(args: &mut Vec<String>) -> Result<u8, String> {
    match take_value(args, "--frame") {
        Some(v) => v
            .parse::<u8>()
            .map_err(|e| format!("bad --frame {v:?}: {e}")),
        None => Ok(0),
    }
}

fn take_target<'a>(args: &'a [String], op: &str) -> Result<&'a str, String> {
    match args {
        [only] => Ok(only.as_str()),
        [] => Err(format!("{op} requires a <target>")),
        _ => Err(format!("{op}: unexpected extra arguments: {args:?}")),
    }
}

fn run_shift(cart: &Cart, sheet: &mut SpriteSheet, args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    let wrap = take_flag(&mut args, "--wrap");
    let frame = take_frame(&mut args)?;
    let dx = match take_value(&mut args, "--dx") {
        Some(v) => v.parse::<i64>().map_err(|e| format!("bad --dx: {e}"))?,
        None => 0,
    };
    let dy = match take_value(&mut args, "--dy") {
        Some(v) => v.parse::<i64>().map_err(|e| format!("bad --dy: {e}"))?,
        None => 0,
    };
    let target_str = take_target(&args, "shift")?;
    let rect = resolve_rect(cart, target_str, frame)?;
    op_shift(sheet, rect, dx, dy, wrap);
    Ok(())
}

fn run_flip(cart: &Cart, sheet: &mut SpriteSheet, args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    let horizontal = take_flag(&mut args, "--horizontal");
    let vertical = take_flag(&mut args, "--vertical");
    if horizontal == vertical {
        return Err("flip requires exactly one of --horizontal or --vertical".into());
    }
    let frame = take_frame(&mut args)?;
    let target_str = take_target(&args, "flip")?;
    let rect = resolve_rect(cart, target_str, frame)?;
    op_flip(sheet, rect, horizontal);
    Ok(())
}

fn run_rotate(cart: &Cart, sheet: &mut SpriteSheet, args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    let cw = take_flag(&mut args, "--cw");
    let ccw = take_flag(&mut args, "--ccw");
    if cw == ccw {
        return Err("rotate requires exactly one of --cw or --ccw".into());
    }
    let frame = take_frame(&mut args)?;
    let target_str = take_target(&args, "rotate")?;
    let rect = resolve_rect(cart, target_str, frame)?;
    op_rotate(sheet, rect, cw)
}

fn run_clear(cart: &Cart, sheet: &mut SpriteSheet, args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    let frame = take_frame(&mut args)?;
    let target_str = take_target(&args, "clear")?;
    let rect = resolve_rect(cart, target_str, frame)?;
    op_clear(sheet, rect);
    Ok(())
}

fn run_copy(cart: &Cart, sheet: &mut SpriteSheet, args: &[String]) -> Result<(), String> {
    let [src, dst] = args else {
        return Err("copy requires exactly two targets: <src> <dst>".into());
    };
    let src_rect = parse_copy_endpoint(cart, src)?;
    let dst_rect = parse_copy_endpoint(cart, dst)?;
    op_copy(sheet, src_rect, dst_rect)
}

/// A copy endpoint: `sprite[:frame]` (frame defaults 0) or a raw
/// `tx,ty,w,h` tile rect. Distinct from `<target>` elsewhere because of the
/// `:frame` suffix syntax (vs. a separate `--frame` flag).
fn parse_copy_endpoint(cart: &Cart, s: &str) -> Result<(u32, u32, u32, u32), String> {
    if s.contains(',') {
        let target = parse_target(s, cart.gfx_meta())?;
        return frame_pixel_rect(cart, &target, 0);
    }
    let (name, frame) = match s.split_once(':') {
        Some((n, f)) => (
            n,
            f.parse::<u8>()
                .map_err(|e| format!("bad frame in {s:?}: {e}"))?,
        ),
        None => (s, 0u8),
    };
    let target = parse_target(name, cart.gfx_meta())?;
    frame_pixel_rect(cart, &target, frame)
}

fn idx(x: u32, y: u32) -> usize {
    y as usize * SHEET_W + x as usize
}

fn op_shift(sheet: &mut SpriteSheet, rect: (u32, u32, u32, u32), dx: i64, dy: i64, wrap: bool) {
    let (x0, y0, w, h) = rect;
    let (wi, hi) = (w as i64, h as i64);
    let mut src = vec![0u8; (w * h) as usize];
    for j in 0..h {
        for i in 0..w {
            src[(j * w + i) as usize] = sheet[idx(x0 + i, y0 + j)];
        }
    }
    for j in 0..hi {
        for i in 0..wi {
            let v = if wrap {
                let si = (i - dx).rem_euclid(wi);
                let sj = (j - dy).rem_euclid(hi);
                src[(sj * wi + si) as usize]
            } else {
                let si = i - dx;
                let sj = j - dy;
                if (0..wi).contains(&si) && (0..hi).contains(&sj) {
                    src[(sj * wi + si) as usize]
                } else {
                    0
                }
            };
            sheet[idx(x0 + i as u32, y0 + j as u32)] = v;
        }
    }
}

fn op_flip(sheet: &mut SpriteSheet, rect: (u32, u32, u32, u32), horizontal: bool) {
    let (x0, y0, w, h) = rect;
    if horizontal {
        for j in 0..h {
            for i in 0..w / 2 {
                let a = idx(x0 + i, y0 + j);
                let b = idx(x0 + w - 1 - i, y0 + j);
                sheet.swap(a, b);
            }
        }
    } else {
        for j in 0..h / 2 {
            for i in 0..w {
                let a = idx(x0 + i, y0 + j);
                let b = idx(x0 + i, y0 + h - 1 - j);
                sheet.swap(a, b);
            }
        }
    }
}

fn op_rotate(sheet: &mut SpriteSheet, rect: (u32, u32, u32, u32), cw: bool) -> Result<(), String> {
    let (x0, y0, w, h) = rect;
    if w != h {
        return Err(format!(
            "rotate requires a square region, got {w}x{h} pixels"
        ));
    }
    let n = w;
    let mut src = vec![0u8; (n * n) as usize];
    for j in 0..n {
        for i in 0..n {
            src[(j * n + i) as usize] = sheet[idx(x0 + i, y0 + j)];
        }
    }
    for j in 0..n {
        for i in 0..n {
            // cw:  dst(i,j) = src(j, n-1-i)
            // ccw: dst(i,j) = src(n-1-j, i)
            let (si, sj) = if cw { (j, n - 1 - i) } else { (n - 1 - j, i) };
            sheet[idx(x0 + i, y0 + j)] = src[(sj * n + si) as usize];
        }
    }
    Ok(())
}

fn op_copy(
    sheet: &mut SpriteSheet,
    src: (u32, u32, u32, u32),
    dst: (u32, u32, u32, u32),
) -> Result<(), String> {
    let (sx, sy, sw, sh) = src;
    let (dx, dy, dw, dh) = dst;
    if sw != dw || sh != dh {
        return Err(format!(
            "copy size mismatch: src {sw}x{sh} vs dst {dw}x{dh} (pixels)"
        ));
    }
    let mut buf = vec![0u8; (sw * sh) as usize];
    for j in 0..sh {
        for i in 0..sw {
            buf[(j * sw + i) as usize] = sheet[idx(sx + i, sy + j)];
        }
    }
    for j in 0..dh {
        for i in 0..dw {
            sheet[idx(dx + i, dy + j)] = buf[(j * dw + i) as usize];
        }
    }
    Ok(())
}

fn op_clear(sheet: &mut SpriteSheet, rect: (u32, u32, u32, u32)) {
    let (x0, y0, w, h) = rect;
    for j in 0..h {
        for i in 0..w {
            sheet[idx(x0 + i, y0 + j)] = 0;
        }
    }
}

// ---------------------------------------------------------------------
// Text-level rewrite: locate __sprites__ rows positionally and touch only
// the lines that changed.
// ---------------------------------------------------------------------

/// `__name__` on a line of its own (surrounding whitespace tolerated).
/// Mirrors `console_core::cart::section_header` exactly (that function is
/// private, and this module may not edit `cart.rs`), since correct rewrite
/// depends on finding section boundaries the same way the parser does.
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

/// Where the `__sprites__` section's rows live in the raw file text.
struct SpritesLayout {
    /// File line index (0-based, into the line-split of the whole file) for
    /// each currently-present sheet row, in row order. Shorter than 128 if
    /// the section has fewer non-blank lines than that (missing rows are
    /// implicitly zero, per `console_core::cart::parse_sprites`).
    row_lines: Vec<usize>,
    /// Line index at which to insert new rows once `row_lines` runs out:
    /// the end of the last `__sprites__` section's body (repeated sections
    /// concatenate, mirroring the parser). `None` if the cart has no
    /// `__sprites__` section at all.
    insert_at: Option<usize>,
}

fn locate_sprites_layout(lines: &[&str]) -> SpritesLayout {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut current: Option<String> = None;
    let mut body_start = 0usize;

    for (i, raw) in lines.iter().enumerate() {
        if let Some(name) = section_header(strip_cr(raw)) {
            if let Some(prev) = current.take() {
                if prev == "sprites" {
                    ranges.push((body_start, i));
                }
            }
            current = Some(name);
            body_start = i + 1;
        }
    }
    if let Some(prev) = current {
        if prev == "sprites" {
            ranges.push((body_start, lines.len()));
        }
    }

    let mut row_lines = Vec::new();
    'ranges: for &(start, end) in &ranges {
        for (i, raw) in lines.iter().enumerate().take(end).skip(start) {
            if row_lines.len() >= SHEET_W {
                break 'ranges;
            }
            if strip_cr(raw).trim().is_empty() {
                continue;
            }
            row_lines.push(i);
        }
    }

    let insert_at = ranges.last().map(|&(_, end)| end);
    SpritesLayout {
        row_lines,
        insert_at,
    }
}

fn encode_row(sheet: &SpriteSheet, y: usize) -> String {
    (0..SHEET_W)
        .map(|x| color_char(sheet[y * SHEET_W + x]))
        .collect()
}

fn row_slice(sheet: &SpriteSheet, y: usize) -> &[u8] {
    &sheet[y * SHEET_W..(y + 1) * SHEET_W]
}

/// Line-ending hint for freshly-inserted lines: mirror whatever the line
/// right before the insertion point uses (CRLF vs LF), so a file that's
/// consistently CRLF in its `__sprites__` section stays that way.
fn eol_hint(lines: &[&str], insert_at: Option<usize>) -> &'static str {
    match insert_at {
        Some(pos) if pos > 0 && lines[pos - 1].ends_with('\r') => "\r",
        _ => "",
    }
}

/// Compute the rewritten cart text for a `sheet` change against the
/// original `text`, touching only `__sprites__` rows that actually
/// changed. Returns `None` if `old_sheet == new_sheet` (nothing to do).
fn rewrite_sprites(
    text: &str,
    old_sheet: &SpriteSheet,
    new_sheet: &SpriteSheet,
) -> Option<(String, Vec<(usize, String)>)> {
    let mut changed_rows: Vec<usize> = (0..SHEET_W)
        .filter(|&y| row_slice(old_sheet, y) != row_slice(new_sheet, y))
        .collect();
    if changed_rows.is_empty() {
        return None;
    }
    changed_rows.sort_unstable();
    let max_row = *changed_rows.last().unwrap();

    let lines: Vec<&str> = text.split('\n').collect();
    let layout = locate_sprites_layout(&lines);

    let mut owned: Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();

    for &y in &changed_rows {
        if y < layout.row_lines.len() {
            let file_line = layout.row_lines[y];
            let eol = if lines[file_line].ends_with('\r') {
                "\r"
            } else {
                ""
            };
            owned[file_line] = format!("{}{eol}", encode_row(new_sheet, y));
        }
    }

    if max_row >= layout.row_lines.len() {
        let append_from = layout.row_lines.len();
        let eol = eol_hint(&lines, layout.insert_at);
        let new_rows: Vec<String> = (append_from..=max_row)
            .map(|y| {
                let content = if changed_rows.binary_search(&y).is_ok() {
                    encode_row(new_sheet, y)
                } else {
                    "0".repeat(SHEET_W)
                };
                format!("{content}{eol}")
            })
            .collect();

        match layout.insert_at {
            Some(pos) => {
                for (offset, row) in new_rows.into_iter().enumerate() {
                    owned.insert(pos + offset, row);
                }
            }
            None => {
                // No __sprites__ section anywhere in the file: append a
                // fresh one at EOF.
                while owned.last().map(String::as_str) == Some("") {
                    owned.pop();
                }
                owned.push("__sprites__".to_string());
                owned.extend(new_rows);
                owned.push(String::new());
            }
        }
    }

    let new_text = owned.join("\n");

    // Recompute against the *new* text so reported line numbers reflect the
    // file as it will actually be written.
    let new_lines: Vec<&str> = new_text.split('\n').collect();
    let new_layout = locate_sprites_layout(&new_lines);
    let report = changed_rows
        .iter()
        .map(|&y| {
            let file_line = new_layout.row_lines[y];
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
        assert_eq!(section_header("__sprites__"), Some("sprites".into()));
        assert_eq!(section_header("  __SPRITES__  "), Some("sprites".into()));
        assert_eq!(section_header("not a header"), None);
        assert_eq!(section_header("____"), None);
    }

    #[test]
    fn rotate_formula_is_self_inverse_after_four() {
        // Sanity: applying cw four times (or cw then ccw) returns to the
        // original 2x2 arrangement.
        let rect = (0, 0, 2, 2);
        let mut sheet = Box::new([0u8; SHEET_W * SHEET_W]);
        sheet[idx(0, 0)] = 1;
        sheet[idx(1, 0)] = 2;
        sheet[idx(1, 1)] = 3;
        sheet[idx(0, 1)] = 4;
        let original = *sheet;
        op_rotate(&mut sheet, rect, true).unwrap();
        op_rotate(&mut sheet, rect, false).unwrap();
        assert_eq!(*sheet, original);
    }

    // The two tests below exercise `rewrite_sprites` white-box, for corners
    // that the real `shift|flip|rotate|copy|clear` ops can never actually
    // reach through `cli_edit` (every one of them is a permutation/copy/
    // clear of *existing* pixels, so a fully-zero sheet can never become
    // non-zero) but that the rewrite logic must still handle correctly.

    #[test]
    fn rewrite_appends_new_section_when_absent() {
        let text = "__lua__\nfunction _init() end\n";
        let old = [0u8; SHEET_W * SHEET_W];
        let mut new = [0u8; SHEET_W * SHEET_W];
        new[idx(0, 0)] = 5;

        let (new_text, report) = rewrite_sprites(text, &old, &new).expect("row 0 changed");
        assert_eq!(report.len(), 1);

        let cart = Cart::parse(&new_text).expect("still a valid cart");
        assert_eq!(cart.sprites()[idx(0, 0)], 5);
        assert!(new_text.contains("__sprites__"));
    }

    #[test]
    fn rewrite_fills_gap_rows_with_zeros_when_appending() {
        // Only row 0 physically present; touching row 3 (with rows 1-2
        // untouched-but-implicit) must materialize rows 1 and 2 as explicit
        // zero lines so row 3's line lands at the right position. A
        // trailing section confirms the new rows land *inside*
        // `__sprites__`, before it, not after.
        let text = "__lua__\nfunction _init() end\n\n__sprites__\na0000000\n__notes__\ntail\n";
        let old = *Cart::parse(text).unwrap().sprites();
        let mut new = old;
        new[idx(0, 0)] = 0;
        new[idx(0, 3)] = 0xa;

        let (new_text, report) = rewrite_sprites(text, &old, &new).expect("rows changed");
        assert_eq!(
            report.len(),
            2,
            "only rows 0 and 3 actually changed: {report:?}"
        );

        let cart = Cart::parse(&new_text).expect("still a valid cart");
        assert_eq!(cart.sprites()[idx(0, 0)], 0);
        assert_eq!(cart.sprites()[idx(0, 3)], 0xa);
        assert!(new_text.ends_with("__notes__\ntail\n"), "{new_text:?}");

        let sprites_body: Vec<&str> = new_text
            .split("__sprites__\n")
            .nth(1)
            .unwrap()
            .split("__notes__")
            .next()
            .unwrap()
            .lines()
            .collect();
        assert_eq!(
            sprites_body.len(),
            4,
            "row 0 plus 3 appended rows: {sprites_body:?}"
        );
        assert_eq!(sprites_body[0], "0".repeat(SHEET_W));
        assert_eq!(
            sprites_body[1],
            "0".repeat(SHEET_W),
            "row 1 is a zero filler"
        );
        assert_eq!(
            sprites_body[2],
            "0".repeat(SHEET_W),
            "row 2 is a zero filler"
        );
        assert_eq!(sprites_body[3], format!("a{}", "0".repeat(SHEET_W - 1)));
    }
}

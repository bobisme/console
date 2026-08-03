//! Shared `__sfx__` text-rewrite machinery: locate the section's entries
//! *positionally* in the raw cart text, then rewrite only the lines that
//! actually change.
//!
//! This is the music half of what `sprite::transform` and `map::transform`
//! do for `__sprites__`/`__map__`, and it obeys the same contract: every byte
//! of the cart file that is not a changed `__sfx__` line — other sections,
//! comments, blank lines, ordering, line endings — survives verbatim, and the
//! rewritten text is re-parsed with `Cart::parse` before anything is written
//! to disk.
//!
//! `__sfx__` differs from the hex grids in one way that shapes everything
//! here: its entries are a *header line plus a variable number of row lines*,
//! not fixed-width rows at fixed offsets. So a transform can add rows
//! ([`stretch`](super::transform) doubling), remove rows (halving), or add a
//! whole entry (`copy`, `import-abc`), and the rewrite has to be an
//! insert/replace/delete plan rather than an in-place row overwrite. Hence
//! [`Rewrite`].
//!
//! The other consequence is a nice one: because a row is *text*, transforms
//! that reorder or duplicate rows ([`shift-rows`](super::transform), `copy`)
//! move the **original line text**, and transforms that change one column
//! ([`transpose`](super::transform), `set-vol`, `set-inst`) do
//! [`replace_token`] surgery on it. An agent's own spacing, alignment and
//! effect columns survive a transpose untouched, which keeps the git diff
//! down to the notes that really moved.

use std::collections::BTreeMap;

use console_core::Cart;

/// `__name__` on a line of its own (surrounding whitespace tolerated).
/// Mirrors `console_core::cart::section_header` exactly (private there) —
/// correct rewrite depends on finding section boundaries the same way the
/// parser does.
pub fn section_header(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix("__")?.strip_suffix("__")?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(inner.to_ascii_lowercase())
}

pub fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// A content line of a section, the way `console_core::audio::clean` sees it:
/// blank lines and whole-line `#` comments are not content. (`#` only starts
/// a comment at the *start* of a line, so `C#4` is safe.)
fn content(line: &str) -> Option<&str> {
    let t = strip_cr(line).trim();
    if t.is_empty() || t.starts_with('#') {
        None
    } else {
        Some(t)
    }
}

/// One `sfx <id> speed=…` entry located in the raw file text.
#[derive(Debug, Clone)]
pub struct SfxBlock {
    pub id: u8,
    /// File line index (0-based) of the `sfx <id> …` header.
    pub header_line: usize,
    /// File line indices of this entry's row lines, in order.
    pub row_lines: Vec<usize>,
}

impl SfxBlock {
    /// Where a new row appended to this entry goes: right after its last row,
    /// or right after its header when it has none.
    pub fn append_at(&self) -> usize {
        match self.row_lines.last() {
            Some(&last) => last + 1,
            None => self.header_line + 1,
        }
    }
}

/// Where `__sfx__` lives in the raw file text.
#[derive(Debug, Clone, Default)]
pub struct SfxLayout {
    pub blocks: Vec<SfxBlock>,
    /// Exclusive end line index of the *last* `__sfx__` section's body
    /// (repeated sections concatenate, mirroring the parser); `None` when the
    /// cart has no `__sfx__` section at all.
    pub section_end: Option<usize>,
}

impl SfxLayout {
    pub fn block(&self, id: u8) -> Option<&SfxBlock> {
        self.blocks.iter().find(|b| b.id == id)
    }

    /// Where a brand-new entry for `id` should be spliced so the section stays
    /// in ascending id order: before the first entry with a higher id, else at
    /// the end of the section.
    pub fn insert_point_for(&self, id: u8) -> Option<usize> {
        match self.blocks.iter().find(|b| b.id > id) {
            Some(b) => Some(b.header_line),
            None => self.section_end,
        }
    }
}

/// Scan `lines` for `__sfx__` sections and classify their content lines into
/// entries. A content line whose first token is `sfx` opens a new entry;
/// every other content line is a row of the entry above it.
pub fn locate(lines: &[&str]) -> SfxLayout {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut current: Option<String> = None;
    let mut body_start = 0usize;

    for (i, raw) in lines.iter().enumerate() {
        if let Some(name) = section_header(strip_cr(raw)) {
            if let Some(prev) = current.take()
                && prev == "sfx"
            {
                ranges.push((body_start, i));
            }
            current = Some(name);
            body_start = i + 1;
        }
    }
    if let Some(prev) = current
        && prev == "sfx"
    {
        ranges.push((body_start, lines.len()));
    }

    let mut blocks: Vec<SfxBlock> = Vec::new();
    for &(start, end) in &ranges {
        for (i, raw) in lines.iter().enumerate().take(end).skip(start) {
            let Some(body) = content(raw) else { continue };
            let mut tokens = body.split_whitespace();
            let first = tokens.next().unwrap_or_default();
            if first.eq_ignore_ascii_case("sfx") {
                if let Some(id) = tokens.next().and_then(|t| t.parse::<u8>().ok()) {
                    blocks.push(SfxBlock {
                        id,
                        header_line: i,
                        row_lines: Vec::new(),
                    });
                }
            } else if let Some(block) = blocks.last_mut() {
                block.row_lines.push(i);
            }
        }
    }

    SfxLayout {
        blocks,
        section_end: ranges.last().map(|&(_, end)| end),
    }
}

/// Where to splice a brand-new `__sfx__` section when the cart has none.
///
/// SPEC's cart anatomy runs meta, lua, sprites, map, gfx_meta, instruments,
/// sfx, music — so the section goes right before the first `__music__`
/// header if there is one, else at the end of the last section that precedes
/// it, else at EOF (with any trailing blank run treated as "past the end").
pub fn new_section_anchor(lines: &[&str]) -> usize {
    let mut music_start: Option<usize> = None;
    // Exclusive end of the last section that `__sfx__` should follow.
    let mut predecessor_end: Option<usize> = None;
    let mut current: Option<String> = None;

    let precedes = ["instruments", "gfx_meta", "map", "sprites", "lua", "meta"];
    for (i, raw) in lines.iter().enumerate() {
        if let Some(name) = section_header(strip_cr(raw)) {
            if let Some(prev) = current.take()
                && precedes.contains(&prev.as_str())
            {
                predecessor_end = Some(i);
            }
            if name == "music" && music_start.is_none() {
                music_start = Some(i);
            }
            current = Some(name);
        }
    }
    if let Some(prev) = current
        && precedes.contains(&prev.as_str())
    {
        predecessor_end = Some(eof_anchor(lines));
    }

    music_start.or(predecessor_end).unwrap_or(eof_anchor(lines))
}

/// EOF as an insertion point: just past the last non-blank line, so a new
/// block lands before the file's trailing newline rather than after it.
fn eof_anchor(lines: &[&str]) -> usize {
    let mut i = lines.len();
    while i > 0 && lines[i - 1].trim().is_empty() {
        i -= 1;
    }
    i
}

/// An insert/replace/delete plan over the cart's raw lines.
#[derive(Debug, Default)]
pub struct Rewrite {
    /// `line -> Some(new text)` to replace, `line -> None` to delete.
    edits: BTreeMap<usize, Option<String>>,
    /// Lines to splice in *before* the given line index (`lines.len()` = EOF).
    inserts: BTreeMap<usize, Vec<String>>,
}

impl Rewrite {
    pub fn set_line(&mut self, line: usize, text: String) {
        self.edits.insert(line, Some(text));
    }

    pub fn delete_line(&mut self, line: usize) {
        self.edits.insert(line, None);
    }

    pub fn insert_before(&mut self, line: usize, text: Vec<String>) {
        self.inserts.entry(line).or_default().extend(text);
    }

    /// Splice a whole new `sfx …` entry at `at`, keeping one blank line
    /// between it and its neighbours (the way `__sfx__` is conventionally
    /// laid out) without ever doubling a blank that is already there.
    pub fn insert_entry(&mut self, lines: &[&str], at: usize, block: Vec<String>) {
        let mut block = block;
        if at > 0 && !lines[at - 1].trim().is_empty() {
            block.insert(0, String::new());
        }
        if at < lines.len() && !lines[at].trim().is_empty() {
            block.push(String::new());
        }
        self.insert_before(at, block);
    }

    /// Splice a fresh `__sfx__` section (header + `rows`) at `anchor`, padding
    /// with blank lines only where the neighbouring lines are not already
    /// blank — so it reads like a hand-authored cart, not a text dump.
    pub fn insert_new_section(&mut self, lines: &[&str], anchor: usize, rows: Vec<String>) {
        let mut block = Vec::new();
        if anchor > 0 && !lines[anchor - 1].trim().is_empty() {
            block.push(String::new());
        }
        block.push("__sfx__".to_string());
        block.extend(rows);
        // A trailing blank both separates the section from whatever follows
        // and, at EOF, supplies the file's final newline.
        if anchor >= lines.len() || !lines[anchor].trim().is_empty() {
            block.push(String::new());
        }
        self.insert_before(anchor, block);
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty() && self.inserts.is_empty()
    }
}

/// Overwrite an existing entry's header and rows: replace the rows that line
/// up, delete the surplus, insert the shortfall after the last surviving row.
/// Shared by `music edit`'s `copy --force` / `stretch` and `import-abc`'s
/// `--force`, all three of which replace an entry's whole row list.
pub fn set_block(
    rw: &mut Rewrite,
    lines: &[&str],
    block: &SfxBlock,
    header: Option<String>,
    rows: Vec<String>,
) {
    if let Some(h) = header {
        rw.set_line(block.header_line, h);
    }
    let keep = block.row_lines.len().min(rows.len());
    for (r, text) in rows.iter().take(keep).enumerate() {
        rw.set_line(block.row_lines[r], text.clone());
    }
    for &line in block.row_lines.iter().skip(keep) {
        rw.delete_line(line);
    }
    if rows.len() > keep {
        let at = if keep > 0 {
            block.row_lines[keep - 1] + 1
        } else {
            block.append_at()
        };
        // Inherit the CR convention of the line the new rows follow.
        let cr = if lines[at.saturating_sub(1)].ends_with('\r') {
            "\r"
        } else {
            ""
        };
        rw.insert_before(
            at,
            rows[keep..].iter().map(|r| format!("{r}{cr}")).collect(),
        );
    }
}

/// One line of a `--dry-run` report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportLine {
    /// A line as it will read in the rewritten file (1-based *new* number).
    Set(usize, String),
    /// A line that disappears (1-based *old* number).
    Removed(usize, String),
}

impl std::fmt::Display for ReportLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReportLine::Set(n, text) => write!(f, "{n}: {text}"),
            ReportLine::Removed(n, text) => write!(f, "{n}: (removed) {text}"),
        }
    }
}

/// The outcome of computing a rewrite against cart text, before any I/O.
#[derive(Debug)]
pub enum EditResult {
    /// Nothing actually changed; nothing to write.
    Unchanged,
    Changed {
        new_text: String,
        report: Vec<ReportLine>,
        /// The human-readable "what happened" lines every music transform
        /// prints, dry-run or not.
        summary: Vec<String>,
    },
}

/// Apply a [`Rewrite`] to `text`, producing the new file text plus a report of
/// every line that changed. Returns `None` when the plan is a no-op (every
/// replacement is byte-identical to the line it replaces).
pub fn apply(text: &str, rw: &Rewrite) -> Option<(String, Vec<ReportLine>)> {
    if rw.is_empty() {
        return None;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut report: Vec<ReportLine> = Vec::new();

    let push_inserts = |at: usize, out: &mut Vec<String>, report: &mut Vec<ReportLine>| {
        if let Some(ins) = rw.inserts.get(&at) {
            for t in ins {
                out.push(t.clone());
                report.push(ReportLine::Set(out.len(), t.clone()));
            }
        }
    };

    for (i, raw) in lines.iter().enumerate() {
        push_inserts(i, &mut out, &mut report);
        match rw.edits.get(&i) {
            Some(None) => report.push(ReportLine::Removed(i + 1, (*raw).to_string())),
            Some(Some(new)) => {
                out.push(new.clone());
                if new != raw {
                    report.push(ReportLine::Set(out.len(), new.clone()));
                }
            }
            None => out.push((*raw).to_string()),
        }
    }
    push_inserts(lines.len(), &mut out, &mut report);

    if report.is_empty() {
        return None;
    }
    Some((out.join("\n"), report))
}

/// Compute the rewrite and confirm it still parses before handing it back —
/// the last line of defense against corrupting a cart, exactly as in
/// `sprite`/`map` `transform`.
pub fn finish(
    text: &str,
    rw: &Rewrite,
    summary: Vec<String>,
    what: &str,
) -> Result<EditResult, String> {
    match apply(text, rw) {
        None => Ok(EditResult::Unchanged),
        Some((new_text, report)) => {
            Cart::parse(&new_text)
                .map_err(|e| format!("{what} would produce an invalid cart (not written): {e}"))?;
            Ok(EditResult::Changed {
                new_text,
                report,
                summary,
            })
        }
    }
}

/// Shared tail of every `music edit` / `music import-abc` CLI entry: print the
/// summary, print the diff under `--dry-run`, otherwise write the file.
pub fn apply_edit_result(
    cart_path: &str,
    result: Result<EditResult, String>,
    dry_run: bool,
) -> i32 {
    match result {
        Ok(EditResult::Unchanged) => {
            println!("no change");
            0
        }
        Ok(EditResult::Changed {
            new_text,
            report,
            summary,
        }) => {
            for line in &summary {
                println!("{line}");
            }
            if dry_run {
                for line in &report {
                    println!("{line}");
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

// ---------------------------------------------------------------------------
// Token surgery
// ---------------------------------------------------------------------------

/// Byte spans of the whitespace-separated tokens of `line`.
pub fn token_spans(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        spans.push((start, i));
    }
    spans
}

/// Replace whitespace-separated token `n` of `line`, preserving every other
/// byte — indentation, the runs of spaces between columns, a trailing `\r`.
/// This is why a `transpose` diff shows only the note names moving.
///
/// One adjustment: when the token is followed by a run of plain spaces and
/// another token, that run absorbs the width change (never shrinking below a
/// single space), so `C4  lead 5` becomes `F#4 lead 5` and a hand-aligned
/// tracker grid stays aligned instead of ragging out one row at a time.
pub fn replace_token(line: &str, n: usize, new: &str) -> Option<String> {
    let spans = token_spans(line);
    let (start, end) = *spans.get(n)?;
    let mut out = String::with_capacity(line.len() + new.len());
    out.push_str(&line[..start]);
    out.push_str(new);
    if let Some(&(next_start, _)) = spans.get(n + 1) {
        let gap = &line[end..next_start];
        if !gap.is_empty() && gap.bytes().all(|b| b == b' ') {
            let width = (gap.len() + (end - start)).saturating_sub(new.len()).max(1);
            out.push_str(&" ".repeat(width));
            out.push_str(&line[next_start..]);
            return Some(out);
        }
    }
    out.push_str(&line[end..]);
    Some(out)
}

/// Replace the `speed=<v>` token of a `sfx <id> …` header line, wherever it
/// sits among the header's tokens.
pub fn replace_speed(line: &str, speed: u8) -> Option<String> {
    let spans = token_spans(line);
    let n = spans
        .iter()
        .position(|&(s, e)| line[s..e].starts_with("speed="))?;
    replace_token(line, n, &format!("speed={speed}"))
}

/// Replace the id token of a `sfx <id> …` header line.
pub fn replace_id(line: &str, id: u8) -> Option<String> {
    replace_token(line, 1, &id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CART: &str = "\
__lua__
function _init() end

__sfx__
# a comment line
sfx 0 speed=8
C4 2 5
---
E4 2 5

sfx 3 speed=4 loop=0,1
A2 3 6

__music__
pat 0 : 0 - - -
";

    #[test]
    fn locate_finds_headers_and_rows_skipping_comments() {
        let lines: Vec<&str> = CART.split('\n').collect();
        let layout = locate(&lines);
        assert_eq!(layout.blocks.len(), 2);
        assert_eq!(layout.blocks[0].id, 0);
        assert_eq!(layout.blocks[0].row_lines.len(), 3);
        assert_eq!(layout.blocks[1].id, 3);
        assert_eq!(layout.blocks[1].row_lines.len(), 1);
        // The `#` line is not a row.
        assert_eq!(lines[layout.blocks[0].header_line], "sfx 0 speed=8");
        assert_eq!(lines[layout.blocks[0].row_lines[0]], "C4 2 5");
    }

    #[test]
    fn insert_point_keeps_ids_ascending() {
        let lines: Vec<&str> = CART.split('\n').collect();
        let layout = locate(&lines);
        // sfx 1 goes before sfx 3's header.
        assert_eq!(
            layout.insert_point_for(1),
            Some(layout.blocks[1].header_line)
        );
        // sfx 9 goes at the end of the section.
        assert_eq!(layout.insert_point_for(9), layout.section_end);
    }

    #[test]
    fn replace_token_keeps_the_following_columns_put() {
        // The gap absorbs the width change, so column 2 stays at column 7.
        assert_eq!(
            replace_token("  C4   2  5\r", 0, "D#4").unwrap(),
            "  D#4  2  5\r"
        );
        // A one-space gap never shrinks to zero.
        assert_eq!(replace_token("C4 2 5", 0, "A#4").unwrap(), "A#4 2 5");
        // The last token keeps whatever follows it verbatim, `\r` included.
        assert_eq!(
            replace_token("C4 2 5 sl+2\r", 3, "sl-2").unwrap(),
            "C4 2 5 sl-2\r"
        );
        assert_eq!(replace_token("C4 2 5", 4, "x"), None);
    }

    #[test]
    fn replace_speed_finds_the_token_anywhere() {
        assert_eq!(
            replace_speed("sfx 3 speed=4 loop=0,1", 8).unwrap(),
            "sfx 3 speed=8 loop=0,1"
        );
        assert_eq!(
            replace_speed("sfx 3 loop=0,1 speed=auto", 8).unwrap(),
            "sfx 3 loop=0,1 speed=8"
        );
    }

    #[test]
    fn new_section_anchor_lands_before_music() {
        let text = "__lua__\nx\n\n__music__\npat 0 : 0 - - -\n";
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines[new_section_anchor(&lines)], "__music__");
    }

    #[test]
    fn new_section_anchor_at_eof_skips_the_trailing_blank() {
        let text = "__lua__\nfunction _init() end\n";
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(new_section_anchor(&lines), 2);
    }
}

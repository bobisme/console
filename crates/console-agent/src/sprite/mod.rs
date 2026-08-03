//! Sprite & animation authoring tools (SPEC.md "Sprite & animation
//! authoring"): inspection renders + numeric lints in [`view`], in-place
//! cart transforms in [`transform`], shared target resolution here.
//!
//! Module boundaries are deliberate — `view.rs` and `transform.rs` are being
//! developed independently; shared plumbing lives in this file only.

pub mod transform;
pub mod view;

use console_core::{Cart, GfxMeta, SpriteDef};

/// What a `<target>` CLI argument resolved to: a named sprite (with its
/// metadata) or a raw `tx,ty,w,h` tile rect.
#[derive(Debug, Clone)]
pub enum Target {
    /// A named sprite from `__gfx_meta__` (also carries an optional anim:
    /// `player.walk` resolves to the sprite `player` plus the anim def name).
    Sprite { name: String, anim: Option<String> },
    /// Raw tile rect `tx,ty,w,h` (tile coordinates, not pixels).
    Rect { tx: u8, ty: u8, w: u8, h: u8 },
}

/// Parse a `<target>` argument: `name`, `name.anim`, or `tx,ty,w,h`.
pub fn parse_target(s: &str, meta: &GfxMeta) -> Result<Target, String> {
    if s.contains(',') {
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() != 4 {
            return Err(format!("rect target must be tx,ty,w,h: {s:?}"));
        }
        let nums: Result<Vec<u8>, _> = parts.iter().map(|p| p.trim().parse::<u8>()).collect();
        let nums = nums.map_err(|e| format!("bad rect target {s:?}: {e}"))?;
        let (tx, ty, w, h) = (nums[0], nums[1], nums[2], nums[3]);
        if tx > 15 || ty > 15 || w == 0 || h == 0 || tx + w > 16 || ty + h > 16 {
            return Err(format!("rect target out of sheet bounds: {s:?}"));
        }
        return Ok(Target::Rect { tx, ty, w, h });
    }
    if meta.anim(s).is_some() {
        let sprite = s.split('.').next().unwrap_or_default().to_string();
        return Ok(Target::Sprite { name: sprite, anim: Some(s.to_string()) });
    }
    if meta.sprite(s).is_some() {
        return Ok(Target::Sprite { name: s.to_string(), anim: None });
    }
    Err(format!(
        "unknown target {s:?}: not an anim, sprite, or tx,ty,w,h rect (cart has: {})",
        known_targets(meta)
    ))
}

fn known_targets(meta: &GfxMeta) -> String {
    let names: Vec<String> = meta
        .sprites()
        .map(|s| s.name.clone())
        .chain(meta.anims().map(|a| a.name.clone()))
        .collect();
    if names.is_empty() { "none — cart has no __gfx_meta__ section".into() } else { names.join(", ") }
}

/// Pixel-space rect (x, y, w, h) on the 128x128 sheet for a target's frame
/// `i` (frame index beyond 0 is only meaningful for named sprites; rects
/// ignore it after frame 0 validation).
pub fn frame_pixel_rect(
    cart: &Cart,
    target: &Target,
    frame: u8,
) -> Result<(u32, u32, u32, u32), String> {
    match target {
        Target::Rect { tx, ty, w, h } => {
            if frame != 0 {
                return Err("raw rect targets have no frames; omit --frame".into());
            }
            Ok((*tx as u32 * 8, *ty as u32 * 8, *w as u32 * 8, *h as u32 * 8))
        }
        Target::Sprite { name, .. } => {
            let def = cart
                .gfx_meta()
                .sprite(name)
                .ok_or_else(|| format!("sprite {name:?} not in __gfx_meta__"))?;
            def.frame_rect(frame)
                .ok_or_else(|| format!("frame {frame} of sprite {name:?} falls off the sheet"))
        }
    }
}

/// The sprite def behind a target, if it has one (rect targets do not).
pub fn target_sprite<'c>(cart: &'c Cart, target: &Target) -> Option<&'c SpriteDef> {
    match target {
        Target::Sprite { name, .. } => cart.gfx_meta().sprite(name),
        Target::Rect { .. } => None,
    }
}

/// Parse a `<target>` string and resolve it straight to a pixel-space rect
/// against `cart`, at raw sheet frame `frame` — the shared entry point for
/// every command that mutates or dumps pixels in place (`sprite edit`,
/// `sprite dump`, `sprite poke`). Note this is *not* the same as
/// `view::render`'s anim-aware `--frame` (which indexes an anim's own frame
/// list); here `frame` is always the raw sprite frame index, matching how
/// `sprite edit`'s ops have always treated it.
pub fn resolve_rect(cart: &Cart, target_str: &str, frame: u8) -> Result<(u32, u32, u32, u32), String> {
    let target = parse_target(target_str, cart.gfx_meta())?;
    frame_pixel_rect(cart, &target, frame)
}

/// CLI dispatch for `console-agent sprite <cmd> ...`. Returns process exit
/// code. `view` commands (including `dump`) are wired by view.rs, `edit` and
/// `poke` by transform.rs.
pub fn cli_sprite(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("render") | Some("strip") | Some("onion") | Some("diff") | Some("ghost")
        | Some("lint") | Some("dump") | Some("gif") => view::cli_view(args),
        Some("edit") => transform::cli_edit(&args[1..]),
        Some("poke") => transform::cli_poke(&args[1..]),
        _ => {
            eprintln!("{SPRITE_USAGE}");
            2
        }
    }
}

pub const SPRITE_USAGE: &str = "\
usage:
  console-agent sprite render <cart> <target> [--frame N] [--zoom Z] [--grid] [--indices] [--anchor] -o out.png
  console-agent sprite strip  <cart> <anim> [--zoom Z] [--anchor] -o out.png
  console-agent sprite onion  <cart> <anim> --frame N [--zoom Z] [--grid] [--anchor] -o out.png
  console-agent sprite onion  <cart> <anim> --all [--zoom Z] [--grid] [--anchor] -o out.png
  console-agent sprite diff   <cart> <anim> <frameA> <frameB> [--zoom Z] -o out.png
  console-agent sprite ghost  <cart> <anim> [--zoom Z] [--grid] [--anchor] -o out.png
  console-agent sprite gif    <cart> <anim> [--zoom Z] [--grid] [--anchor] -o out.gif
  console-agent sprite lint   <cart> [anim ...] [--max-drift PX] [--max-area-var PCT] [--max-changed PX] [--no-unique-colors] [--summary]
  console-agent sprite edit   <cart> <shift|flip|rotate|copy|clear> ... [--dry-run]
  console-agent sprite dump   <cart> <target> [--frame N]
  console-agent sprite poke   <cart> <target> [--frame N] (--rows <hex,hex,...> | --stdin) [--dry-run]
  (targets: sprite name, anim name, or tile rect tx,ty,w,h)";

//! Sprite-tool-level tests for bn-2op (`__gfx_meta__` `frames_rect=` and
//! explicit `tx:ty` frame entries) — SPEC.md "Sprite & animation authoring
//! (PoC v1)".
//!
//! The cart below deliberately puts each anim's frames in sheet regions that
//! do NOT overlap the sprite's own `rect`, so a bug that silently fell back
//! to the classic sprite-rect addressing (ignoring `frames_rect`/explicit
//! coords) would sample the wrong marker color and fail loudly, not just
//! render *something* plausible.
//!
//! Sheet layout (single-pixel markers, rest of the sheet is color 0):
//! - tile (0,0), local (0,0) = color 9 — sprite `p`'s own rect. Never
//!   expected to appear in either anim's output; if it does, `frames_rect`
//!   was ignored.
//! - tile (5,0), local (0,0) = color 3 — `frames_rect=5,0`'s frame-0 origin.
//! - tile (6,0), local (0,0) = color 4 — one step right of that origin.
//! - tile (2,3), local (0,0) = color 5 — an explicit `2:3` frame, reachable
//!   only through the explicit-coordinate path (`frames_rect` would never
//!   put a frame there).

use console_agent::sprite::view::{self, RenderOpts};
use console_core::{Cart, PALETTE};

/// Build one `__sprites__` row, `len` palette chars wide, `'0'` everywhere
/// except the given `(x, palette_index)` marks.
fn sheet_row(len: usize, marks: &[(usize, u8)]) -> String {
    let mut row = vec![b'0'; len];
    for &(x, v) in marks {
        row[x] = char::from_digit(u32::from(v), 16).unwrap() as u8;
    }
    String::from_utf8(row).unwrap()
}

fn cart() -> Cart {
    // Row 0: markers at tile (0,0) [x=0], tile (5,0) [x=5*8=40], tile (6,0)
    // [x=6*8=48]. Row 24 (tile row 3, since 24 = 3*8): marker at tile (2,3)
    // [x=2*8=16].
    let row0 = sheet_row(49, &[(0, 9), (40, 3), (48, 4)]);
    let row24 = sheet_row(17, &[(16, 5)]);
    let mut sprite_rows = vec![row0];
    sprite_rows.extend(std::iter::repeat_n("0".to_string(), 23)); // rows 1..=23
    sprite_rows.push(row24); // row 24

    let text = format!(
        "__lua__\nfunction _init() end\n\n__sprites__\n{}\n\n__gfx_meta__\n\
         sprite p rect=0,0 size=1x1\n\
         anim p.reloc frames=0,1 fps=4 frames_rect=5,0\n\
         anim p.mixed frames=0,2:3,1 fps=4 frames_rect=5,0\n",
        sprite_rows.join("\n"),
    );
    Cart::parse(&text).expect("test cart parses")
}

fn px(img: &view::Image, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
}

#[test]
fn frames_rect_relocates_the_frame_0_origin_away_from_the_sprites_own_rect() {
    let cart = cart();

    // frame 0 -> frames_rect's own tile (5,0): marker color 3, NOT the
    // sprite's own-rect marker (color 9 at tile 0,0).
    let f0 = view::render(
        &cart,
        "p.reloc",
        &RenderOpts {
            frame: Some(0),
            ..RenderOpts::default()
        },
    )
    .expect("render p.reloc frame 0");
    assert_eq!(px(&f0, 0, 0), PALETTE[3]);

    // frame 1 -> one step right of frames_rect's origin: tile (6,0), marker 4.
    let f1 = view::render(
        &cart,
        "p.reloc",
        &RenderOpts {
            frame: Some(1),
            ..RenderOpts::default()
        },
    )
    .expect("render p.reloc frame 1");
    assert_eq!(px(&f1, 0, 0), PALETTE[4]);
}

#[test]
fn mixed_index_and_explicit_tile_frames_resolve_through_one_strip() {
    let cart = cart();
    // p.mixed: frames=0 (index, via frames_rect=5,0), 2:3 (explicit tile,
    // ignores frames_rect), 1 (index, via frames_rect=5,0 again).
    let img = view::strip(&cart, "p.mixed", 4, false).expect("strip p.mixed");

    assert_eq!(img.frames, 3);
    // 3 frames * (8 sheet px * zoom 4) + 2 separators of 2px each.
    assert_eq!(img.width, 3 * 8 * 4 + 2 * 2);
    assert_eq!(img.height, 8 * 4);

    // Cell 0 (x offset 0): index frame 0 -> frames_rect tile (5,0), color 3.
    assert_eq!(px(&img, 0, 0), PALETTE[3]);
    // Cell 1 (x offset 34 = 32 + 2px separator): explicit tile (2,3), color
    // 5 — reachable only by honoring the explicit tx:ty form, never by
    // frames_rect wrap math.
    assert_eq!(px(&img, 34, 0), PALETTE[5]);
    // Cell 2 (x offset 68): index frame 1 -> frames_rect tile (6,0), color 4.
    assert_eq!(px(&img, 68, 0), PALETTE[4]);
}

#[test]
fn dump_animation_frames_honor_frames_rect_and_explicit_tiles() {
    let cart = cart();
    let index_via_frames_rect = view::dump(&cart, "p.mixed", 0).expect("dump anim frame 0");
    assert_eq!(
        index_via_frames_rect.lines().next(),
        Some("# x=40 y=0 w=8 h=8")
    );
    assert!(index_via_frames_rect.contains('3'));

    let explicit_tile = view::dump(&cart, "p.mixed", 1).expect("dump anim frame 1");
    assert_eq!(explicit_tile.lines().next(), Some("# x=16 y=24 w=8 h=8"));
    assert!(explicit_tile.contains('5'), "{explicit_tile}");
}

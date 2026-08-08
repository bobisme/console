//! `aspr` / `anim_len` / `anim_done`: playing `__gfx_meta__` anims at runtime.
//!
//! The contract these pin down:
//!   * the frame is a pure function of `frame_count - t0` (no hidden state),
//!   * `aspr` is byte-for-byte the `spr` of the frame `AnimDef::resolve_frame`
//!     picks, drawn from the sprite's declared anchor,
//!   * an unknown anim name halts the cart instead of drawing nothing.

use console_core::{Cart, Console, Error, GfxMeta, SCREEN_W, SPRITES_PER_ROW};

/// A compact authored corner of the 256x256 sheet where every pixel's colour
/// depends on both its tile and its
/// position inside that tile, so a wrong frame, a wrong anchor offset or a
/// missed flip all show up as different pixels.
fn sheet_text() -> String {
    let mut s = String::with_capacity(129 * 128);
    for y in 0..128usize {
        for x in 0..128usize {
            let (tx, ty) = (x / 8, y / 8);
            let c = 1 + (tx * 3 + ty * 5 + (x % 8) + 2 * (y % 8)) % 15;
            s.push(char::from_digit(c as u32, 16).unwrap());
        }
        s.push('\n');
    }
    s
}

fn cart(lua: &str, meta: &str) -> String {
    format!(
        "__lua__\n{lua}\n__sprites__\n{}\n__gfx_meta__\n{meta}\n",
        sheet_text()
    )
}

fn meta_of(text: &str) -> GfxMeta {
    Cart::parse(text).unwrap().gfx_meta().clone()
}

/// The `spr()` sprite id whose tile is the top-left of `anim`'s frame `pos`.
fn sprite_id(meta: &GfxMeta, name: &str, pos: usize) -> u32 {
    let anim = meta.anim(name).unwrap();
    let sprite = meta.sprite(&anim.sprite).unwrap();
    let (sx, sy, _, _) = anim.resolve_frame(sprite, pos).unwrap();
    (sy / 8) * SPRITES_PER_ROW as u32 + (sx / 8)
}

/// Run a cart for `frames` steps and hand back its framebuffer.
fn run(text: &str, frames: u64) -> Vec<u8> {
    let mut con = Console::new(text, 0).unwrap();
    for _ in 0..frames {
        con.step(0).unwrap();
    }
    con.framebuffer().to_vec()
}

/// The error a cart halts with on its first step.
fn halt_error(text: &str) -> String {
    let mut con = Console::new(text, 0).unwrap();
    match con.step(0) {
        Err(Error::Lua(e)) => e,
        other => panic!("expected a Lua error, got {other:?}"),
    }
}

const PLAYER: &str = "\
sprite p rect=1,0 size=1x1 anchor=4,7
anim p.walk frames=0,1,2,3 fps=8 loop
anim p.swing frames=0,1,2 fps=10
";

// ---------------------------------------------------------------------------
// Parity with spr(): aspr must be the same pixels through the same path
// ---------------------------------------------------------------------------

/// The headline equivalence, checked frame by frame over two full loops: at
/// console frame `f`, `aspr(name, x, y)` is exactly `spr(id, x - ax, y - ay)`
/// for the id `resolve_frame` selects.
#[test]
fn aspr_is_spr_of_the_resolved_frame_from_the_anchor() {
    let animated = cart("function _draw() cls(3) aspr('p.walk', 40, 40) end", PLAYER);
    let meta = meta_of(&animated);

    let mut con = Console::new(&animated, 0).unwrap();
    for f in 0..70i64 {
        con.step(0).unwrap(); // this step drew with frame_count == f
        let pos = meta.anim("p.walk").unwrap().frame_at(f);
        let id = sprite_id(&meta, "p.walk", pos);
        // anchor=(4, 7) => the 8x8 tile's top-left is (40 - 4, 40 - 7).
        let manual = cart(
            &format!("function _draw() cls(3) spr({id}, 36, 33) end"),
            PLAYER,
        );
        assert_eq!(
            con.framebuffer().to_vec(),
            run(&manual, 1),
            "frame {f} (anim position {pos}, sprite {id})"
        );
    }
}

/// Anchors are the whole point of `aspr`: `(x, y)` is the declared contact
/// point, not the top-left corner `spr` takes.
#[test]
fn the_anchor_positions_the_frame() {
    for (anchor, tlx, tly) in [
        ("anchor=0,0", 50, 60),  // top-left: identical to spr
        ("anchor=4,7", 46, 53),  // feet
        ("anchor=4,4", 46, 56),  // centre
        ("anchor=-3,2", 53, 58), // outside the sprite box is legal
    ] {
        let meta = format!("sprite p rect=1,0 size=1x1 {anchor}\nanim p.a frames=0 fps=1 loop\n");
        let animated = cart("function _draw() cls(3) aspr('p.a', 50, 60) end", &meta);
        let manual = cart(
            &format!("function _draw() cls(3) spr(1, {tlx}, {tly}) end"),
            &meta,
        );
        assert_eq!(run(&animated, 1), run(&manual, 1), "{anchor}");
    }
}

/// Flips mirror the pixels inside the destination rect, exactly as `spr` does.
/// The anchor itself does **not** mirror — the rect is in the same place either
/// way — which is what keeps a flipped walk cycle standing on the same spot.
#[test]
fn flips_match_spr_and_leave_the_anchor_put() {
    for (fx, fy) in [(false, false), (true, false), (false, true), (true, true)] {
        let animated = cart(
            &format!("function _draw() cls(3) aspr('p.walk', 40, 40, 0, {fx}, {fy}) end"),
            PLAYER,
        );
        let manual = cart(
            &format!("function _draw() cls(3) spr(1, 36, 33, 1, 1, {fx}, {fy}) end"),
            PLAYER,
        );
        assert_eq!(run(&animated, 1), run(&manual, 1), "flip {fx},{fy}");
    }
}

/// Megatiles, `frames_rect=` relocation and explicit `tx:ty` frames all come
/// from `AnimDef::resolve_frame`, so `aspr` inherits them for free.
#[test]
fn megatiles_frames_rect_and_explicit_tiles_all_resolve() {
    let meta = "\
sprite m rect=12,2 size=2x2 anchor=8,8
anim m.flap frames=0,1,2,3 fps=6 loop
anim m.odd frames=0,3:9,1 fps=6 loop frames_rect=4,6
";
    let animated = cart("function _draw() cls(3) aspr(NAME, 60, 60) end", meta);
    let parsed = meta_of(&animated);

    for name in ["m.flap", "m.odd"] {
        let src = animated.replace("NAME", &format!("'{name}'"));
        let mut con = Console::new(&src, 0).unwrap();
        let anim = parsed.anim(name).unwrap();
        for f in 0..40i64 {
            con.step(0).unwrap();
            let id = sprite_id(&parsed, name, anim.frame_at(f));
            let manual = cart(
                &format!("function _draw() cls(3) spr({id}, 52, 52, 2, 2) end"),
                meta,
            );
            assert_eq!(
                con.framebuffer().to_vec(),
                run(&manual, 1),
                "{name} frame {f}"
            );
        }
    }
}

/// Everything the sprite path respects, `aspr` respects — because it *is* the
/// sprite path. `fillp` stays a shape-only effect (set here to prove it does
/// not leak into either draw).
#[test]
fn camera_clip_pal_palt_and_fillp_behave_exactly_as_for_spr() {
    let state = "camera(-7, 5) clip(10, 10, 100, 100) pal(3, 11) palt(0, false) \
                 palt(5, true) fillp(0x5a5a)";
    let animated = cart(
        &format!("function _draw() cls(3) {state} aspr('p.walk', 40, 40) end"),
        PLAYER,
    );
    let manual = cart(
        &format!("function _draw() cls(3) {state} spr(1, 36, 33) end"),
        PLAYER,
    );
    assert_eq!(run(&animated, 1), run(&manual, 1));
}

// ---------------------------------------------------------------------------
// t0: the only handle on phase
// ---------------------------------------------------------------------------

/// `t0` shifts the origin and nothing else: the picture at frame `f` with
/// `t0 = k` is the picture at frame `f - k` with `t0 = 0`.
#[test]
fn t0_restarts_the_cycle() {
    let phase_locked = cart("function _draw() cls(3) aspr('p.walk', 40, 40) end", PLAYER);
    // Restart at frame 12: from frame 12 on, this must replay the frame-0 run.
    let restarted = cart(
        "function _draw() cls(3) aspr('p.walk', 40, 40, 12) end",
        PLAYER,
    );

    let mut locked = Console::new(&phase_locked, 0).unwrap();
    let mut shifted = Console::new(&restarted, 0).unwrap();
    for _ in 0..12 {
        shifted.step(0).unwrap();
    }
    for f in 0..40 {
        locked.step(0).unwrap();
        shifted.step(0).unwrap();
        assert_eq!(
            locked.framebuffer(),
            shifted.framebuffer(),
            "t0=12 at frame {} should equal t0=0 at frame {f}",
            f + 12
        );
    }
}

/// Two instances of one anim with different origins genuinely differ — the
/// point of `t0` being a per-call argument rather than console state.
#[test]
fn two_origins_in_one_frame_show_different_frames() {
    let src = cart(
        "function _draw() cls(3) aspr('p.walk', 20, 40) aspr('p.walk', 60, 40, 8) end",
        PLAYER,
    );
    let mut con = Console::new(&src, 0).unwrap();
    con.step(0).unwrap(); // frame 0: elapsed 0 and elapsed -8
    let meta = meta_of(&src);
    let a = meta.anim("p.walk").unwrap();
    assert_ne!(
        a.frame_at(0),
        a.frame_at(-8),
        "the fixture needs two different frames to be meaningful"
    );

    // The two 8x8 blocks must not be pixel-identical.
    let fb = con.framebuffer();
    let block = |x0: usize| -> Vec<u8> {
        (33..41)
            .flat_map(|y| (x0..x0 + 8).map(move |x| (x, y)))
            .map(|(x, y)| fb[y * SCREEN_W + x])
            .collect()
    };
    assert_ne!(block(16), block(56));
}

// ---------------------------------------------------------------------------
// anim_len / anim_done
// ---------------------------------------------------------------------------

#[test]
fn anim_len_reports_the_declared_frame_count() {
    let src = cart(
        "function _draw() printh(anim_len('p.walk') .. ',' .. anim_len('p.swing')) end",
        PLAYER,
    );
    let mut con = Console::new(&src, 0).unwrap();
    con.step(0).unwrap();
    assert_eq!(con.take_logs(), vec!["4,3".to_string()]);
}

/// The one-shot state machine this exists for: play `p.swing` from the frame
/// the button went down, and switch back the moment `anim_done` says the last
/// frame has had its full time on screen.
#[test]
fn anim_done_ends_a_one_shot_and_never_a_loop() {
    let src = cart(
        "\
function _draw()
  cls(3)
  local f = flr(t() * 60)
  printh((anim_done('p.swing', 5) and 'done' or 'busy') ..
         ',' .. (anim_done('p.walk', 5) and 'done' or 'busy'))
end",
        PLAYER,
    );
    let mut con = Console::new(&src, 0).unwrap();
    // p.swing: 3 frames at 10 fps = 6 console frames each, so playback runs
    // past the end 18 frames after its origin at frame 5, i.e. at frame 23.
    for _ in 0..30 {
        con.step(0).unwrap();
    }
    let logs = con.take_logs();
    for (f, line) in logs.iter().enumerate() {
        let want = if f >= 23 { "done,busy" } else { "busy,busy" };
        assert_eq!(line, want, "frame {f}");
    }
}

/// A one-shot's last frame stays on screen for good once it is done.
#[test]
fn a_finished_one_shot_holds_its_last_frame() {
    let src = cart(
        "function _draw() cls(3) aspr('p.swing', 40, 40) end",
        PLAYER,
    );
    let mut con = Console::new(&src, 0).unwrap();
    for _ in 0..18 {
        con.step(0).unwrap();
    }
    let settled = con.framebuffer().to_vec();
    for _ in 0..60 {
        con.step(0).unwrap();
        assert_eq!(con.framebuffer(), &settled[..]);
    }
    let id = sprite_id(&meta_of(&src), "p.swing", 2);
    let manual = cart(
        &format!("function _draw() cls(3) spr({id}, 36, 33) end"),
        PLAYER,
    );
    assert_eq!(settled, run(&manual, 1));
}

// ---------------------------------------------------------------------------
// Errors: a typo halts, it does not silently draw nothing
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_anim_name_halts_the_cart() {
    for call in [
        "aspr('p.wlak', 10, 10)",
        "anim_len('p.wlak')",
        "anim_done('p.wlak', 0)",
    ] {
        let src = cart(&format!("function _draw() {call} end"), PLAYER);
        let err = halt_error(&src);
        assert!(err.contains("p.wlak"), "{err}");
        assert!(err.contains("no anim named"), "{err}");
        // The message lists what the cart actually declared.
        assert!(err.contains("p.walk"), "{err}");
    }
}

#[test]
fn a_sprite_name_is_not_an_anim_name() {
    let src = cart("function _draw() aspr('p', 10, 10) end", PLAYER);
    let err = halt_error(&src);
    assert!(err.contains("no anim named"), "{err}");
}

#[test]
fn a_cart_with_no_gfx_meta_says_so() {
    let src = "__lua__\nfunction _draw() aspr('p.walk', 10, 10) end\n";
    let err = halt_error(src);
    assert!(err.contains("no __gfx_meta__ section"), "{err}");
}

/// A halt is a halt: the console stays halted afterwards, like every other
/// cart error.
#[test]
fn the_halt_sticks() {
    let src = cart("function _draw() aspr('nope.nope', 1, 1) end", PLAYER);
    let mut con = Console::new(&src, 0).unwrap();
    assert!(con.step(0).is_err());
    assert!(con.is_halted());
    assert!(con.step(0).is_err());
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// No hidden animation state anywhere: two fresh consoles playing the same
/// anims agree frame for frame, and a cart that draws 50 instances with
/// scattered origins is as reproducible as one that draws none.
#[test]
fn animated_carts_replay_identically() {
    let src = cart(
        "\
function _draw()
  cls(3)
  for i = 0, 49 do
    aspr('p.walk', 4 + (i % 10) * 14, 20 + flr(i / 10) * 20, i * 3, i % 2 == 0, false)
  end
  aspr('p.swing', 70, 200, 30)
end",
        PLAYER,
    );
    let mut a = Console::new(&src, 0).unwrap();
    let mut b = Console::new(&src, 0).unwrap();
    for f in 0..90 {
        a.step(0).unwrap();
        b.step(0).unwrap();
        assert_eq!(a.framebuffer(), b.framebuffer(), "divergence at frame {f}");
    }
}

/// Frames drawn by `aspr` are ordinary pixels: they land in the framebuffer,
/// they animate, and nothing about them is deferred to the host.
#[test]
fn aspr_actually_draws_and_actually_animates() {
    let src = cart("function _draw() cls(0) aspr('p.walk', 40, 40) end", PLAYER);
    let mut con = Console::new(&src, 0).unwrap();
    con.step(0).unwrap();
    let first = con.framebuffer().to_vec();
    assert!(first.iter().any(|&c| c != 0), "aspr drew nothing");
    // 8 fps: the second frame is showing by console frame 8.
    for _ in 0..8 {
        con.step(0).unwrap();
    }
    assert_ne!(con.framebuffer(), &first[..], "the anim never advanced");
}

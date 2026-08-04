use console_core::{Console, DrawBounds as Bounds, MAX_DRAW_EVENTS_PER_FRAME};

const META: &str = "\
sprite bug rect=1,0 size=1x1 anchor=4,7
anim bug.fly frames=0 fps=8 loop
";

fn cart(draw: &str) -> String {
    format!("__lua__\nfunction _draw()\n{draw}\nend\n__gfx_meta__\n{META}\n")
}

#[test]
fn tracing_is_opt_in_and_does_not_change_pixels() {
    let text = cart(
        "cls(2)\n\
         draw_tag('actors')\n\
         camera(10,20)\n\
         clip(0,0,40,40)\n\
         pal(3,7)\n\
         palt(5,true)\n\
         fillp(0x1234,9)\n\
         rectfill(12,22,14,24,3)\n\
         line(12,22,14,24,3)\n\
         rect(12,22,14,24,3)\n\
         circ(20,30,2,3)\n\
         circfill(20,30,2,3)\n\
         spr(1,18,28,1,1,true,false)\n\
         sspr(8,0,8,8,18,28,8,8,false,true)\n\
         aspr('bug.fly',30,60)\n\
         map(0,0,10,20,2,1)\n\
         print('hi',11,21,6,'left')\n\
         draw_tag()\n\
         pset(11,21,4)",
    );
    let mut plain = Console::new(&text, 7).unwrap();
    let mut traced = Console::new(&text, 7).unwrap();
    traced.set_draw_tracing(true);

    plain.step(0).unwrap();
    traced.step(0).unwrap();
    assert_eq!(plain.framebuffer(), traced.framebuffer());
    assert!(plain.take_draw_events().events.is_empty());

    let trace = traced.take_draw_events();
    assert_eq!(trace.dropped, 0);
    assert_eq!(
        trace
            .events
            .iter()
            .map(|event| event.op)
            .collect::<Vec<_>>(),
        [
            "cls", "rectfill", "line", "rect", "circ", "circfill", "spr", "sspr", "aspr", "map",
            "print", "pset"
        ]
    );

    let cls = &trace.events[0];
    assert_eq!(cls.tag, None);
    assert_eq!(cls.camera, [0, 0]);
    assert_eq!(cls.screen_bounds, Bounds::xywh(0, 0, 192, 320));

    let shape = &trace.events[1];
    assert_eq!(shape.tag.as_deref(), Some("actors"));
    assert_eq!(shape.world_bounds, Bounds::xywh(12, 22, 3, 3));
    assert_eq!(shape.screen_bounds, Bounds::xywh(2, 2, 3, 3));
    assert_eq!(shape.visible_bounds, Some(Bounds::xywh(2, 2, 3, 3)));
    assert!(!shape.clipped);
    assert_eq!(shape.camera, [10, 20]);
    assert_eq!(shape.clip, Bounds::xywh(0, 0, 40, 40));
    assert_eq!(shape.draw_palette.len(), 1);
    assert_eq!(shape.draw_palette[0].from, 3);
    assert_eq!(shape.draw_palette[0].to, 7);
    assert!(shape.display_palette.is_empty());
    assert_eq!(shape.transparent_colors, [0, 5]);
    assert_eq!(shape.fill_pattern, 0x1234);
    assert_eq!(shape.fill_secondary, Some(9));
    assert_eq!(shape.details.color, Some(3));

    let sprite = &trace.events[6];
    assert_eq!(sprite.details.sprite_id, Some(1));
    assert_eq!(sprite.details.sheet_bounds, Some(Bounds::xywh(8, 0, 8, 8)));
    assert_eq!(sprite.details.flip_x, Some(true));
    assert_eq!(sprite.details.flip_y, Some(false));

    let sampled = &trace.events[7];
    assert_eq!(sampled.details.sheet_bounds, Some(Bounds::xywh(8, 0, 8, 8)));
    assert_eq!(sampled.details.flip_y, Some(true));

    let animation = &trace.events[8];
    assert_eq!(animation.details.animation.as_deref(), Some("bug.fly"));
    assert_eq!(animation.details.animation_frame, Some(0));
    assert_eq!(animation.world_bounds, Bounds::xywh(26, 53, 8, 8));
    assert!(animation.clipped, "bottom row lies outside the 40px clip");

    assert_eq!(
        trace.events[9].details.map_bounds,
        Some(Bounds::xywh(0, 0, 2, 1))
    );
    assert_eq!(trace.events[10].details.text.as_deref(), Some("hi"));
    assert_eq!(trace.events[10].details.align.as_deref(), Some("left"));
    assert_eq!(trace.events[11].tag, None);
}

#[test]
fn per_frame_buffer_is_bounded_and_reports_drops() {
    let mut console = Console::new(
        "__lua__\nfunction _draw() for i=1,5000 do pset(i,1,1) end end\n",
        0,
    )
    .unwrap();
    console.set_draw_tracing(true);
    console.step(0).unwrap();
    let trace = console.take_draw_events();
    assert_eq!(trace.events.len(), MAX_DRAW_EVENTS_PER_FRAME);
    assert_eq!(trace.dropped as usize, 5000 - MAX_DRAW_EVENTS_PER_FRAME);
}

#[test]
fn camera_extremes_are_saturating_and_tags_are_bounded() {
    let text = cart("camera(-2147483648,-2147483648) draw_tag('fx') pset(0,0,1)");
    let mut console = Console::new(&text, 0).unwrap();
    console.set_draw_tracing(true);
    console.step(0).unwrap();
    let event = console.take_draw_events().events.remove(0);
    assert_eq!(event.screen_bounds.x, i64::from(i32::MAX));
    assert_eq!(event.screen_bounds.y, i64::from(i32::MAX));
    assert_eq!(event.tag.as_deref(), Some("fx"));

    let too_long = "x".repeat(65);
    let error = console
        .eval(&format!("draw_tag('{too_long}')"))
        .expect_err("tags longer than 64 bytes must fail")
        .to_string();
    assert!(error.contains("at most 64 UTF-8 bytes"), "{error}");
}

#[test]
fn extreme_camera_and_corner_spans_match_visible_renderer_bounds() {
    let mut console = Console::new(
        &cart(
            "camera(-2147483648,-2147483648)\n\
             pset(-2147483648,-2147483648,1)\n\
             camera()\n\
             rectfill(-2147483648,1,2147483647,1,2)\n\
             camera(2147483647,0)\n\
             spr(1,2147483647,2)",
        ),
        0,
    )
    .unwrap();
    console.set_draw_tracing(true);
    console.step(0).unwrap();
    let events = console.take_draw_events().events;

    assert_eq!(events[0].screen_bounds, Bounds::xywh(0, 0, 1, 1));
    assert_eq!(events[0].visible_bounds, Some(Bounds::xywh(0, 0, 1, 1)));
    assert_eq!(
        events[1].world_bounds,
        Bounds {
            x: i64::from(i32::MIN),
            y: 1,
            w: u32::MAX as i64 + 1,
            h: 1,
        }
    );
    assert_eq!(events[1].visible_bounds, Some(Bounds::xywh(0, 1, 192, 1)));
    assert_eq!(events[2].screen_bounds, Bounds::xywh(0, 2, 8, 8));
}

#[test]
fn extreme_map_cell_ranges_keep_visible_offset_tiles_in_bounds() {
    let mut sprites = String::new();
    for _ in 0..8 {
        sprites.push_str("0000000011111111\n");
    }
    let text = format!(
        "__lua__\nfunction _draw() map(-268435456,0,-2147483648,0,268435457,1) end\n\
         __sprites__\n{sprites}__map__\n01\n"
    );
    let mut console = Console::new(&text, 0).unwrap();
    console.set_draw_tracing(true);
    console.step(0).unwrap();
    assert_eq!(console.framebuffer()[0], 1, "offset map tile is visible");

    let event = console.take_draw_events().events.remove(0);
    assert_eq!(event.world_bounds.x, i64::from(i32::MIN));
    assert_eq!(event.world_bounds.w, 2_147_483_656);
    assert_eq!(event.visible_bounds, Some(Bounds::xywh(0, 0, 8, 8)));
}

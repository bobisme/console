use console_core::{Console, LAYER_TRANSPARENT, MAX_CAPTURED_LAYERS, SCREEN_W};

fn cart(draw: &str) -> String {
    format!("__lua__\nfunction _draw()\n{draw}\nend\n")
}

fn layer<'a>(frame: &'a console_core::LayerCaptureFrame, tag: Option<&str>) -> &'a [u8] {
    &frame
        .layers
        .iter()
        .find(|layer| layer.tag.as_deref() == tag)
        .unwrap_or_else(|| panic!("missing captured layer {tag:?}"))
        .framebuffer[..]
}

#[test]
fn capture_is_opt_in_and_never_changes_the_presented_frame() {
    let text = cart(
        "draw_tag('background') cls(0)\n\
         draw_tag('terrain') camera(10,20) clip(0,0,5,5) pal(3,7) rectfill(12,22,20,30,3)\n\
         draw_tag('actors') rectfill(12,22,14,24,9)\n\
         draw_tag() camera() clip() pal() pset(1,2,0)",
    );
    let mut plain = Console::new(&text, 7).unwrap();
    let mut captured = Console::new(&text, 7).unwrap();
    captured.set_layer_capture(true);

    plain.step(0).unwrap();
    captured.step(0).unwrap();
    assert_eq!(plain.framebuffer(), captured.framebuffer());
    assert!(!plain.layer_capture_frame().enabled);

    let frame = captured.layer_capture_frame();
    assert!(frame.enabled);
    assert_eq!(frame.dropped, 0);
    assert_eq!(frame.layers.len(), 4);

    let background = layer(&frame, Some("background"));
    assert!(background.iter().all(|&pixel| pixel == 0));

    let actors = layer(&frame, Some("actors"));
    assert_eq!(actors[2 * SCREEN_W + 2], 9);
    assert_eq!(actors[4 * SCREEN_W + 4], 9);
    assert_eq!(captured.framebuffer()[2 * SCREEN_W + 2], 9);

    let terrain = layer(&frame, Some("terrain"));
    assert_eq!(
        terrain[2 * SCREEN_W + 2],
        7,
        "camera and draw palette apply"
    );
    assert_eq!(terrain[4 * SCREEN_W + 4], 7);
    assert_eq!(
        terrain[5 * SCREEN_W + 5],
        LAYER_TRANSPARENT,
        "clip applies to the isolated layer"
    );
    assert_eq!(
        terrain[2 * SCREEN_W + 2],
        7,
        "terrain remains visible in isolation beneath an overlapping actor"
    );

    let untagged = layer(&frame, None);
    assert_eq!(untagged[2 * SCREEN_W + 1], 0, "real colour 0 is retained");
    assert_eq!(untagged[0], LAYER_TRANSPARENT);
}

#[test]
fn capture_is_current_frame_only_and_applies_scanout_effects() {
    let mut console = Console::new(
        &cart(
            "draw_tag('actor')\n\
             if t() == 0 then pset(0,0,7) else pset(8,8,3) end\n\
             mosaic(2) rshift(0,1)",
        ),
        0,
    )
    .unwrap();
    console.set_layer_capture(true);

    console.step(0).unwrap();
    let first = console.layer_capture_frame();
    let actor = layer(&first, Some("actor"));
    assert_eq!(actor[1], 7, "rshift applies after mosaic");
    assert_eq!(actor[2], 7);
    assert_eq!(actor[SCREEN_W], 7);
    assert_eq!(actor[SCREEN_W + 1], 7);

    console.step(0).unwrap();
    let second = console.layer_capture_frame();
    let actor = layer(&second, Some("actor"));
    assert_eq!(actor[1], LAYER_TRANSPARENT, "previous frame was cleared");
    assert_eq!(actor[8 * SCREEN_W + 8], 3);
}

#[test]
fn dynamically_generated_tags_are_bounded() {
    let mut console = Console::new(
        &cart("for i=1,40 do draw_tag('layer-'..i) pset(i,1,i) end"),
        0,
    )
    .unwrap();
    console.set_layer_capture(true);
    console.step(0).unwrap();

    let frame = console.layer_capture_frame();
    assert_eq!(frame.capacity, MAX_CAPTURED_LAYERS);
    assert_eq!(frame.layers.len(), MAX_CAPTURED_LAYERS);
    assert_eq!(frame.dropped, 40 - MAX_CAPTURED_LAYERS as u32);
}

#[test]
fn historical_tags_do_not_consume_the_current_frames_capacity() {
    let mut console =
        Console::new(&cart("draw_tag('frame-'..flr(t()*60)) pset(1,1,7)"), 0).unwrap();
    console.set_layer_capture(true);
    for _ in 0..40 {
        console.step(0).unwrap();
        let frame = console.layer_capture_frame();
        assert_eq!(frame.layers.len(), 1);
        assert_eq!(frame.dropped, 0);
    }
    assert_eq!(
        console.layer_capture_frame().layers[0].tag.as_deref(),
        Some("frame-39")
    );
}

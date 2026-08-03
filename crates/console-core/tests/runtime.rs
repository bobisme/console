use console_core::{Console, Error, input};

fn cart(body: &str) -> Console {
    Console::new(&format!("__lua__\n{body}\n"), 0).expect("cart should load")
}

#[test]
fn init_runs_at_load_time() {
    let con = cart("function _init() loaded = 41 + 1 end");
    assert_eq!(con.get_global("loaded").unwrap().as_i64(), Some(42));
    assert_eq!(con.frame_count(), 0);
}

#[test]
fn top_level_code_runs_before_init() {
    let con = cart(
        "order = {} order[#order+1] = 'top' function _init() order[#order+1] = 'init' end",
    );
    assert_eq!(read_strings(&con, "order"), vec!["top", "init"]);
}

#[test]
fn update_and_draw_run_once_per_frame() {
    let mut con = cart(
        "u = 0 d = 0
         function _update() u = u + 1 end
         function _draw() d = d + 1 end",
    );
    for _ in 0..5 {
        con.step(0).unwrap();
    }
    assert_eq!(con.get_global("u").unwrap().as_i64(), Some(5));
    assert_eq!(con.get_global("d").unwrap().as_i64(), Some(5));
    assert_eq!(con.frame_count(), 5);
}

#[test]
fn missing_callbacks_are_fine() {
    let mut con = cart("x = 1");
    for _ in 0..3 {
        con.step(input::A).unwrap();
    }
    assert_eq!(con.frame_count(), 3);
}

#[test]
fn t_is_frame_count_over_sixty() {
    let mut con = cart("function _update() ticks = t() end");
    con.step(0).unwrap();
    assert_eq!(con.get_global("ticks").unwrap().as_f64(), Some(0.0));
    for _ in 0..59 {
        con.step(0).unwrap();
    }
    assert_eq!(con.get_global("ticks").unwrap().as_f64(), Some(59.0 / 60.0));
    assert_eq!(con.frame_count(), 60);
}

#[test]
fn btn_is_held_and_btnp_is_edge_triggered() {
    let mut con = cart(
        "held = {} pressed = {}
         function _update()
           held[#held+1]    = btn(1) and 1 or 0
           pressed[#pressed+1] = btnp(1) and 1 or 0
         end",
    );
    // frame:      0     1     2     3     4
    // right:      no    yes   yes   no    yes
    let seq = [0u8, input::RIGHT, input::RIGHT, 0, input::RIGHT];
    for m in seq {
        con.step(m).unwrap();
    }
    let held = read_ints(&con, "held");
    let pressed = read_ints(&con, "pressed");
    assert_eq!(held, vec![0, 1, 1, 0, 1]);
    assert_eq!(pressed, vec![0, 1, 0, 0, 1]);
}

#[test]
fn btnp_fires_on_the_very_first_frame() {
    let mut con = cart("function _update() first = btnp(4) end");
    con.step(input::A).unwrap();
    assert_eq!(con.get_global("first").unwrap().as_boolean(), Some(true));
    con.step(input::A).unwrap();
    assert_eq!(con.get_global("first").unwrap().as_boolean(), Some(false));
}

#[test]
fn every_button_bit_maps_to_its_index() {
    let mut con = cart("function _update() b = {} for i=0,5 do b[i+1] = btn(i) and 1 or 0 end end");
    for (i, mask) in [
        input::LEFT,
        input::RIGHT,
        input::UP,
        input::DOWN,
        input::A,
        input::B,
    ]
    .into_iter()
    .enumerate()
    {
        con.step(mask).unwrap();
        let bits = read_ints(&con, "b");
        let expect: Vec<i64> = (0..6).map(|k| i64::from(k == i)).collect();
        assert_eq!(bits, expect, "button index {i}");
    }
}

#[test]
fn out_of_range_button_indices_are_false() {
    // BUTTON_COUNT is 7 (indices 0..=6, since the MENU button was added), so
    // the first genuinely out-of-range index is 7.
    let mut con = cart("function _update() a = btn(-1) b = btn(7) c = btnp(99) end");
    con.step(0xff).unwrap();
    for name in ["a", "b", "c"] {
        assert_eq!(
            con.get_global(name).unwrap().as_boolean(),
            Some(false),
            "{name}"
        );
    }
}

#[test]
fn printh_is_collected_and_drained() {
    let mut con = cart(
        "function _update() printh('frame ' .. flr(t()*60)) printh(7) printh(true) end",
    );
    con.step(0).unwrap();
    con.step(0).unwrap();
    let logs = con.take_logs();
    assert_eq!(
        logs,
        vec!["frame 0", "7", "true", "frame 1", "7", "true"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
    assert!(con.take_logs().is_empty(), "logs must drain");
}

#[test]
fn printh_never_draws() {
    let mut con = cart("function _draw() printh('hello') end");
    con.step(0).unwrap();
    assert!(con.framebuffer().iter().all(|&p| p == 0));
}

#[test]
fn math_helpers() {
    let mut con = cart(
        "function _update()
           a = flr(3.7) b = flr(-3.2) c = ceil(3.2) d = ceil(-3.7)
           e = abs(-5) f = min(3, 1, 2) g = max(3, 1, 2) h = mid(3, 1, 2)
           i = sin(0) j = sin(0.25) k = cos(0) l = cos(0.5)
         end",
    );
    con.step(0).unwrap();
    let f = |n: &str| read_num(&con, n);
    assert_eq!(f("a"), 3.0);
    assert_eq!(f("b"), -4.0);
    assert_eq!(f("c"), 4.0);
    assert_eq!(f("d"), -3.0);
    assert_eq!(f("e"), 5.0);
    assert_eq!(f("f"), 1.0);
    assert_eq!(f("g"), 3.0);
    assert_eq!(f("h"), 2.0);
    // sin/cos take turns and are NOT sign-inverted.
    assert!(f("i").abs() < 1e-12);
    assert!((f("j") - 1.0).abs() < 1e-12);
    assert!((f("k") - 1.0).abs() < 1e-12);
    assert!((f("l") + 1.0).abs() < 1e-12);
}

#[test]
fn flr_returns_a_lua_integer() {
    let mut con = cart("function _update() k = flr(2.9) s = tostring(k) end");
    con.step(0).unwrap();
    assert_eq!(read_str(&con, "s"), "2");
}

// ---- sandbox --------------------------------------------------------------

fn expect_runtime_error(body: &str, needle: &str) {
    let src = format!("__lua__\nfunction _update() {body} end\n");
    let mut con = Console::new(&src, 0).unwrap();
    let err = con.step(0).unwrap_err();
    assert!(matches!(err, Error::Lua(_)), "expected a Lua error: {err}");
    assert!(
        err.to_string().contains(needle),
        "error {err:?} should mention {needle:?}"
    );
    assert!(con.is_halted());
}

#[test]
fn os_and_io_are_gone() {
    expect_runtime_error("local x = os.time()", "os");
    expect_runtime_error("local x = io.open('/etc/passwd')", "io");
    expect_runtime_error("debug.getinfo(1)", "debug");
    expect_runtime_error("require('io')", "require");
    expect_runtime_error("dofile('/etc/passwd')", "dofile");
    expect_runtime_error("loadfile('/etc/passwd')", "loadfile");
    expect_runtime_error("load('return 1')", "load");
    expect_runtime_error("local p = package.path", "package");
}

#[test]
fn removed_globals_read_as_nil() {
    let con = cart("");
    for name in ["io", "os", "debug", "package", "require", "dofile", "loadfile", "load"] {
        assert!(
            con.get_global(name).unwrap().is_nil(),
            "{name} should be nil"
        );
    }
}

#[test]
fn math_random_points_at_rnd() {
    let src = "__lua__\nfunction _update() local x = math.random() end\n";
    let mut con = Console::new(src, 0).unwrap();
    let err = con.step(0).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("math.random is disabled"), "{msg}");
    assert!(msg.contains("rnd(n)"), "{msg}");

    let src = "__lua__\nfunction _update() math.randomseed(1) end\n";
    let mut con = Console::new(src, 0).unwrap();
    let msg = con.step(0).unwrap_err().to_string();
    assert!(msg.contains("math.randomseed is disabled"), "{msg}");
    assert!(msg.contains("srand(seed)"), "{msg}");
}

#[test]
fn safe_stdlib_survives() {
    let mut con = cart(
        "function _update()
           s = string.upper('ok')
           n = #({1,2,3})
           m = math.floor(2.5) + math.max(1, 2)
           tbl = table.concat({'a','b'}, '-')
           ty = type(math.pi)
         end",
    );
    con.step(0).unwrap();
    assert_eq!(read_str(&con, "s"), "OK");
    assert_eq!(con.get_global("n").unwrap().as_i64(), Some(3));
    assert_eq!(con.get_global("m").unwrap().as_i64(), Some(4));
    assert_eq!(read_str(&con, "tbl"), "a-b");
    assert_eq!(read_str(&con, "ty"), "number");
}

// ---- error handling -------------------------------------------------------

#[test]
fn parse_error_in_cart_source_fails_to_load() {
    let err = Console::new("__lua__\nthis is not lua\n", 0).unwrap_err();
    assert!(matches!(err, Error::Lua(_)), "{err:?}");
}

#[test]
fn error_in_init_fails_to_load() {
    let err = Console::new("__lua__\nfunction _init() error('boom') end\n", 0).unwrap_err();
    assert!(err.to_string().contains("boom"), "{err}");
}

#[test]
fn update_error_halts_the_console_with_a_traceback() {
    let mut con = Console::new(
        "__lua__\n\
         local function inner() error('kaboom') end\n\
         function _update() inner() end\n\
         function _draw() cls(7) end\n",
        0,
    )
    .unwrap();

    assert!(!con.is_halted());
    let err = con.step(0).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("kaboom"), "{msg}");
    assert!(msg.contains("traceback"), "expected a traceback in: {msg}");
    assert!(con.is_halted());
    assert_eq!(con.halted().unwrap(), &err);

    // _draw never ran, so the screen is untouched...
    assert!(con.framebuffer().iter().all(|&p| p == 0));
    // ...and further steps are no-ops that keep returning the same error.
    let frame_before = con.frame_count();
    let again = con.step(0).unwrap_err();
    assert_eq!(again, err);
    assert_eq!(con.frame_count(), frame_before);
}

#[test]
fn draw_error_also_halts() {
    let mut con = Console::new(
        "__lua__\nfunction _draw() nosuchfunction() end\n",
        0,
    )
    .unwrap();
    let err = con.step(0).unwrap_err();
    assert!(err.to_string().contains("nil value"), "{err}");
    assert!(con.is_halted());
}

#[test]
fn eval_reaches_the_cart_environment() {
    let mut con = cart("v = 5");
    assert_eq!(con.eval("return v * 2").unwrap().as_i64(), Some(10));
    assert!(con.eval("return nosuch.field").is_err());
    con.eval("cls(3)").unwrap();
    assert!(con.framebuffer().iter().all(|&p| p == 3));
}

fn read_ints(con: &Console, name: &str) -> Vec<i64> {
    let v = con.get_global(name).unwrap();
    let t = v.as_table().expect("table").clone();
    (1..=t.raw_len()).map(|i| t.get::<i64>(i).unwrap()).collect()
}

/// Read a numeric global, accepting either a Lua integer or a Lua float.
fn read_num(con: &Console, name: &str) -> f64 {
    let v = con.get_global(name).unwrap();
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .unwrap_or_else(|| panic!("{name} is not a number: {v:?}"))
}

fn read_str(con: &Console, name: &str) -> String {
    con.get_global(name)
        .unwrap()
        .as_string()
        .expect("string global")
        .to_string_lossy()
}

fn read_strings(con: &Console, name: &str) -> Vec<String> {
    let v = con.get_global(name).unwrap();
    let t = v.as_table().expect("table").clone();
    (1..=t.raw_len())
        .map(|i| t.get::<String>(i).unwrap())
        .collect()
}

//! Native acceptance and opt-in ceiling telemetry for Mirelight Survivors.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use console_agent::hooks::dev_value_to_json;
use console_agent::session::Session;
use console_agent::value::lua_to_json;
use console_core::{DevHookPhase, DevValue};
use serde_json::{Value, json};

struct CountingAllocator;

static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static OUTSTANDING_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_OUTSTANDING_BYTES: AtomicU64 = AtomicU64::new(0);

fn add_outstanding(bytes: u64) {
    let current = OUTSTANDING_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    let mut peak = PEAK_OUTSTANDING_BYTES.load(Ordering::Relaxed);
    while current > peak {
        match PEAK_OUTSTANDING_BYTES.compare_exchange_weak(
            peak,
            current,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => peak = actual,
        }
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            add_outstanding(layout.size() as u64);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        DEALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        OUTSTANDING_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            DEALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            if new_size >= layout.size() {
                add_outstanding((new_size - layout.size()) as u64);
            } else {
                OUTSTANDING_BYTES.fetch_sub((layout.size() - new_size) as u64, Ordering::Relaxed);
            }
        }
        replacement
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mirelight-survivors")
}

fn cart_text() -> String {
    let project = project();
    console_agent::project::compile_project(&project)
        .unwrap_or_else(|error| panic!("compiling {}: {error}", project.display()))
        .cart_text
}

fn hook(session: &mut Session, name: &str, phase: DevHookPhase, args: DevValue) -> Value {
    let invocation = session
        .invoke_dev_hook(name, phase, args)
        .unwrap_or_else(|error| panic!("invoking hook {name:?}: {error}"));
    dev_value_to_json(&invocation.result)
}

fn status(session: &mut Session) -> Value {
    hook(session, "status", DevHookPhase::PostFrame, DevValue::Null)
}

fn start_stress(session: &mut Session) {
    hook(session, "stress", DevHookPhase::PreFrame, DevValue::Null);
}

fn run_trace(session: &mut Session, segment_frames: u64) {
    for input in [
        console_core::input::RIGHT,
        console_core::input::DOWN,
        console_core::input::LEFT,
        console_core::input::UP,
    ] {
        session.step(segment_frames, input).unwrap();
    }
}

fn assert_dense_contract(value: &Value) {
    let alive = value["alive"].as_u64().unwrap();
    assert!((800..=1000).contains(&alive), "{value}");
    assert!(
        value["stress_min_alive"].as_u64().unwrap() >= 800,
        "{value}"
    );
    assert!(value["peak_alive"].as_u64().unwrap() <= 1000, "{value}");
    assert_eq!(value["enemies"], 820, "{value}");
    assert!(value["projectiles"].as_u64().unwrap() >= 64, "{value}");
    assert!(value["pickups"].as_u64().unwrap() > 0, "{value}");
    assert!(value["particles"].as_u64().unwrap() > 0, "{value}");
    assert_eq!(value["dropped_spawns"], 0, "{value}");
    assert_eq!(value["within_entity_ceiling"], true, "{value}");
    assert_eq!(value["spatial_reduction"], true, "{value}");
    assert!(
        value["collision_hits_total"].as_u64().unwrap() > 0,
        "{value}"
    );
    assert!(value["despawned_total"].as_u64().unwrap() >= 500, "{value}");
}

fn assert_dense_population(value: &Value) {
    let alive = value["alive"].as_u64().unwrap();
    assert!((800..=1000).contains(&alive), "{value}");
    assert_eq!(value["enemies"], 820, "{value}");
    assert!(value["projectiles"].as_u64().unwrap() >= 64, "{value}");
    assert!(value["pickups"].as_u64().unwrap() > 0, "{value}");
    assert!(value["particles"].as_u64().unwrap() > 0, "{value}");
    assert_eq!(value["dropped_spawns"], 0, "{value}");
}

#[test]
fn dense_trace_is_deterministic_and_stays_inside_the_800_to_1000_contract() {
    let cart = cart_text();
    let mut first = Session::new();
    let mut second = Session::new();
    first.load_cart(&cart, 37).unwrap();
    second.load_cart(&cart, 37).unwrap();
    start_stress(&mut first);
    start_stress(&mut second);

    run_trace(&mut first, 75);
    run_trace(&mut second, 75);
    let first_status = status(&mut first);
    let second_status = status(&mut second);

    assert_dense_contract(&first_status);
    assert_eq!(first_status, second_status);
    assert_eq!(
        first.console().unwrap().framebuffer(),
        second.console().unwrap().framebuffer()
    );
}

#[test]
fn title_b_release_a_release_enters_dense_mode_and_keeps_its_hud_marker() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 5).unwrap();

    session.step(1, console_core::input::B).unwrap();
    session.step(1, 0).unwrap();

    let selected = status(&mut session);
    assert_eq!(selected["phase"], "title");
    assert_eq!(selected["dense_selected"], true);

    session.step(1, console_core::input::A).unwrap();
    session.step(1, 0).unwrap();

    let value = status(&mut session);
    assert_eq!(value["mode"], "stress");
    assert_eq!(value["phase"], "play");
    assert_dense_population(&value);
    let events = session.text_events(Some(4)).unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.text == "DENSE" && event.visible && !event.clipped),
        "persistent dense marker missing from {events:?}"
    );
}

#[test]
fn large_xp_grants_queue_every_normal_upgrade_choice() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 5).unwrap();
    hook(
        &mut session,
        "start",
        DevHookPhase::PreFrame,
        DevValue::Null,
    );
    hook(
        &mut session,
        "grant_xp",
        DevHookPhase::PostFrame,
        DevValue::Number(100.0),
    );

    let queued = status(&mut session);
    assert_eq!(queued["level"], 4);
    assert_eq!(queued["pending_upgrades"], 3);
    assert_eq!(queued["phase"], "levelup");

    for remaining in [2, 1, 0] {
        session.step(1, console_core::input::A).unwrap();
        session.step(1, 0).unwrap();
        let value = status(&mut session);
        assert_eq!(value["pending_upgrades"], remaining);
        assert_eq!(
            value["phase"],
            if remaining == 0 { "play" } else { "levelup" }
        );
    }
}

#[test]
fn bounded_inspection_reports_exact_counts_without_serializing_the_swarm() {
    let mut session = Session::new();
    session.load_cart(&cart_text(), 37).unwrap();
    start_stress(&mut session);
    session.step(60, console_core::input::RIGHT).unwrap();

    let enemies = session
        .ecs_query(
            "survivors",
            &["enemy".to_string(), "pos".to_string()],
            &BTreeMap::from([
                (
                    "enemy".to_string(),
                    vec!["kind".to_string(), "hp".to_string(), "elite".to_string()],
                ),
                ("pos".to_string(), vec!["x".to_string(), "y".to_string()]),
            ]),
            128,
            0,
        )
        .map(|value| lua_to_json(&value))
        .unwrap();
    assert_eq!(enemies["matched"], 820);
    assert_eq!(enemies["returned"], 128);
    assert_eq!(enemies["truncated"], true);
    assert_eq!(enemies["budget_exhausted"], false);

    let projectiles = session
        .ecs_query(
            "survivors",
            &["projectile".to_string(), "pos".to_string()],
            &BTreeMap::from([(
                "projectile".to_string(),
                vec!["kind".to_string(), "damage".to_string(), "ttl".to_string()],
            )]),
            128,
            0,
        )
        .map(|value| lua_to_json(&value))
        .unwrap();
    assert!(
        projectiles["matched"].as_u64().unwrap() >= 64,
        "{projectiles}"
    );
    assert_eq!(projectiles["truncated"], false);
}

#[test]
fn playtests_cover_progression_loss_stress_and_watch_churn() {
    let project = project();
    let normal = project.join("playtest.json");
    let stress = project.join("stress-playtest.json");
    let watches = project.join("watch-playtest.json");
    let artifact_root =
        std::env::temp_dir().join(format!("console-mirelight-playtest-{}", std::process::id()));
    fs::create_dir_all(&artifact_root).unwrap();
    for (index, scenario) in [&normal, &stress].into_iter().enumerate() {
        let artifacts = artifact_root.join(index.to_string());
        let report =
            console_agent::playtest::run_scenario(&project, scenario, Some(&artifacts), None)
                .unwrap_or_else(|error| panic!("running {}: {error}", scenario.display()));
        let report = serde_json::to_value(report).unwrap();
        assert_eq!(report["scenario"]["status"], "passed");
    }

    let report = console_agent::playtest::run_scenario(&project, &watches, None, None)
        .unwrap_or_else(|error| panic!("running {}: {error}", watches.display()));
    let report = serde_json::to_value(report).unwrap();
    assert_eq!(report["scenario"]["status"], "passed");

    let population = &report["stages"][7]["actual"];
    assert!((800..=1000).contains(&population["alive"].as_u64().unwrap()));
    assert_eq!(population["component_counts"]["enemy"], 820);

    let enemies = &report["stages"][8]["actual"];
    assert_eq!(enemies["matched"], 820);
    assert_eq!(enemies["delta"]["entity_membership_complete"], false);

    let projectiles = &report["stages"][9]["actual"];
    assert!(projectiles["matched"].as_u64().unwrap() >= 64);
    assert_eq!(projectiles["delta"]["entity_membership_complete"], true);
    assert!(
        !projectiles["delta"]["spawned"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !projectiles["delta"]["despawned"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(&artifact_root).unwrap();
}

#[derive(Clone, Copy)]
struct AllocationSnapshot {
    alloc_calls: u64,
    dealloc_calls: u64,
    alloc_bytes: u64,
    dealloc_bytes: u64,
    outstanding_bytes: u64,
}

fn allocation_snapshot() -> AllocationSnapshot {
    AllocationSnapshot {
        alloc_calls: ALLOC_CALLS.load(Ordering::Relaxed),
        dealloc_calls: DEALLOC_CALLS.load(Ordering::Relaxed),
        alloc_bytes: ALLOC_BYTES.load(Ordering::Relaxed),
        dealloc_bytes: DEALLOC_BYTES.load(Ordering::Relaxed),
        outstanding_bytes: OUTSTANDING_BYTES.load(Ordering::Relaxed),
    }
}

fn percentile(sorted: &[u128], numerator: usize, denominator: usize) -> u128 {
    let index = ((sorted.len() - 1) * numerator).div_ceil(denominator);
    sorted[index]
}

#[test]
#[ignore = "release-mode ceiling telemetry; run explicitly with --nocapture"]
fn benchmark_stress_window_emits_timing_allocation_and_churn_json() {
    const WARMUP_FRAMES: u64 = 120;
    const MEASURED_FRAMES: usize = 600;

    let mut session = Session::new();
    session.load_cart(&cart_text(), 37).unwrap();
    start_stress(&mut session);
    session
        .step(WARMUP_FRAMES, console_core::input::RIGHT)
        .unwrap();

    let mut frame_nanos = Vec::with_capacity(MEASURED_FRAMES);
    let allocations_before = allocation_snapshot();
    PEAK_OUTSTANDING_BYTES.store(allocations_before.outstanding_bytes, Ordering::Relaxed);
    let measured_start = Instant::now();
    for frame in 0..MEASURED_FRAMES {
        let input = match frame / 150 {
            0 => console_core::input::RIGHT,
            1 => console_core::input::DOWN,
            2 => console_core::input::LEFT,
            _ => console_core::input::UP,
        };
        let start = Instant::now();
        session.step(1, input).unwrap();
        frame_nanos.push(start.elapsed().as_nanos());
    }
    let total_nanos = measured_start.elapsed().as_nanos();
    let allocations_after = allocation_snapshot();
    let peak_outstanding = PEAK_OUTSTANDING_BYTES.load(Ordering::Relaxed);
    frame_nanos.sort_unstable();

    let value = status(&mut session);
    assert_dense_contract(&value);
    let p95_ms = percentile(&frame_nanos, 95, 100) as f64 / 1_000_000.0;
    let report = json!({
        "schema": 1,
        "cart": "mirelight-survivors",
        "seed": 37,
        "target": format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "warmup_frames": WARMUP_FRAMES,
        "measured_frames": MEASURED_FRAMES,
        "timing": {
            "total_ms": total_nanos as f64 / 1_000_000.0,
            "mean_ms": total_nanos as f64 / MEASURED_FRAMES as f64 / 1_000_000.0,
            "p50_ms": percentile(&frame_nanos, 50, 100) as f64 / 1_000_000.0,
            "p95_ms": p95_ms,
            "max_ms": frame_nanos[MEASURED_FRAMES - 1] as f64 / 1_000_000.0,
            "realtime_budget_ms": 1000.0 / 60.0,
        },
        "allocations": {
            "scope": "host-process allocator deltas during Session::step only",
            "alloc_calls": allocations_after.alloc_calls - allocations_before.alloc_calls,
            "dealloc_calls": allocations_after.dealloc_calls - allocations_before.dealloc_calls,
            "allocated_bytes": allocations_after.alloc_bytes - allocations_before.alloc_bytes,
            "deallocated_bytes": allocations_after.dealloc_bytes - allocations_before.dealloc_bytes,
            "net_bytes": allocations_after.outstanding_bytes as i128 - allocations_before.outstanding_bytes as i128,
            "peak_bytes_above_baseline": peak_outstanding.saturating_sub(allocations_before.outstanding_bytes),
        },
        "game": value,
    });
    println!("MIRELIGHT_BENCHMARK {report}");

    if let Ok(limit) = std::env::var("CONSOLE_MIRELIGHT_MAX_P95_MS") {
        let limit = limit
            .parse::<f64>()
            .expect("CONSOLE_MIRELIGHT_MAX_P95_MS must be a finite number");
        assert!(p95_ms <= limit, "p95 {p95_ms:.3} ms exceeded {limit:.3} ms");
    }
}

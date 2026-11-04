//! perf-g1 (docs/RENDER_PERF_GATES.md G1): the profiler's report carries what the budget ledger needs, and a heap
//! allocation inside a profiled stage shows up in that stage's count (so "0 hot-loop allocations" is a number a test
//! can assert). This test binary installs dc-render's counting allocator and switches the profiler on for its own
//! process (other test binaries never enable it).
//!
//! 1. Synthetic, no ROM: the recorder books allocations per stage; a headless run on the ROM-free synthetic course
//!    with the profiler writes a JSON report with p50 / p95 / p99 / max per stage and counter, the ten ledger rows,
//!    the load phase and the CPU load. Skipped without a GPU adapter for the headless part.
//! 2. ROM-gated (`CA_ROM`): the `cold` stress scenario for 40 frames reports model loads, effect spawns, the runtime
//!    pipeline creates of the first effect frame and hitches with their load events.
// ORIGINAL: perf-g1 measurement tooling tests

use dc_client::prof::{self, Laps, Recorder, Stage};
use dc_client::testkit::synth_source;
use dc_client::{run_headless, HeadlessOptions};
use dc_render::{AdapterChoice, GpuContext};

#[global_allocator]
static ALLOC: dc_render::perf::CountingAlloc = dc_render::perf::CountingAlloc;

fn report_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dc-perf-tooling-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

fn read(path: &std::path::Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).expect("profile report written");
    serde_json::from_str(&text).expect("profile report is JSON")
}

const LEDGER_ROWS: [&str; 10] = [
    "system_ram",
    "gpu_static",
    "gpu_dynamic_frame",
    "staging_in_flight",
    "upload_traffic",
    "cpu_frame",
    "gpu_frame",
    "pipelines",
    "draw_bind",
    "streaming",
];

#[test]
fn profiler_reports_the_ledger_and_counts_stage_allocations() {
    let path = report_path("perf.json");
    // the warm-up is left out of the statistics: none here
    std::env::set_var("DC_PROF_WARMUP", "0");
    prof::enable(path.clone());
    assert!(prof::on());

    // --- allocations land in the stage they were made in
    let mut rec = Recorder::new();
    for _ in 0..3 {
        rec.begin(None);
        let mut laps = Laps::start();
        let v: Vec<Vec<u8>> = (0..5).map(|i| vec![0u8; 16 + i]).collect();
        std::hint::black_box(&v);
        laps.lap(Stage::Png);
        laps.lap(Stage::Encode); // nothing allocated here
        rec.end(0);
    }
    let v = serde_json::from_str::<serde_json::Value>(&rec.finish()).expect("json");
    let png = v["stages"]["png"]["allocs_max"].as_f64().unwrap_or(0.0);
    let encode = v["stages"]["encode"]["allocs_max"].as_f64().unwrap_or(-1.0);
    assert!(png >= 6.0, "the stage's allocations are counted: {png}");
    assert_eq!(encode, 0.0, "a stage without allocations books none");
    assert_eq!(v["hot_loop_allocs_per_frame"]["counted"], true);
    assert_eq!(v["frames"], 3);
    for k in ["p50", "p95", "p99", "max"] {
        assert!(v["frame_ms"][k].is_number(), "frame_ms {k}");
    }

    // --- a headless run on the synthetic course
    if let Err(e) = GpuContext::headless(AdapterChoice::Auto) {
        println!("skipped the headless part: no GPU adapter ({e})");
        return;
    }
    let mut src = synth_source();
    let opts = HeadlessOptions {
        frames: 40,
        width: 320,
        height: 180,
        out: None,
        models: false,
        ..HeadlessOptions::default()
    };
    let r = run_headless(&mut src, None, &opts).expect("headless run");
    assert_eq!(r.frames, 40);
    let v = read(&path);
    assert_eq!(v["schema"], "dc-prof/2");
    assert_eq!(v["frames"], 40);
    for row in LEDGER_ROWS {
        assert!(v["ledger"][row].is_object(), "ledger row {row}");
    }
    for k in ["p50", "p95", "p99", "max"] {
        assert!(v["stages"]["encode"][k].is_number(), "encode {k}");
        assert!(v["counters"]["upload_bytes"][k].is_number(), "upload {k}");
    }
    assert!(v["counters"]["draw_calls"]["max"].as_f64().unwrap_or(0.0) > 0.0);
    // the load phase covers the device, the pipelines and the world upload
    assert!(v["load"]["pipeline_creates"].as_u64().unwrap_or(0) > 0);
    assert!(v["load"]["upload_bytes"].as_u64().unwrap_or(0) > 0);
    assert!(v["cpu"].is_object() || !cfg!(windows), "CPU load recorded");
    assert!(v["all_frames"]["frames"].as_u64() == Some(40));
    // hitches are listed with their stage breakdown (frame 0 creates the effect pipelines, usually a hitch)
    for h in v["hitches"]["kept"].as_array().into_iter().flatten() {
        assert!(h["stages"].is_object() && h["top_stage"].is_string());
    }
    if v["gpu"]["available"] == true {
        assert!(
            v["gpu"]["frames_timed"].as_u64().unwrap_or(0) > 0,
            "timestamps collected in flight"
        );
        assert!(v["gpu"]["passes"]["world"]["p99"].is_number());
    }

    // --- ROM-gated: the cold stress scenario
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped the stress part: CA_ROM unset");
        return;
    };
    let Some(map) = dc_client::RomWorld::map_names(&rom)
        .ok()
        .and_then(|m| m.into_iter().next())
    else {
        println!("skipped the stress part: no ROM map");
        return;
    };
    let mut cfg = dc_client::stress::StressConfig::new(dc_client::stress::Scenario::Cold);
    cfg.remotes = 4;
    let mut src = dc_client::stress::StressSource::new(&rom, &map, cfg);
    let opts = HeadlessOptions {
        frames: 40,
        width: 320,
        height: 180,
        out: None,
        models: true,
        hud: dc_client::hud::HudChoice::Rom(rom.clone()),
        show_board: true,
        ..HeadlessOptions::default()
    };
    let r = run_headless(&mut src, None, &opts).expect("stress run");
    assert_eq!(r.frames, 40);
    let v = read(&path);
    assert!(
        v["counters"]["model_loads"]["total"].as_u64().unwrap_or(0) > 0,
        "first-use model loads"
    );
    assert!(
        v["counters"]["fx_spawns"]["total"].as_u64().unwrap_or(0) > 0,
        "effect groups started"
    );
    assert!(
        v["counters"]["fx_quads"]["max"].as_f64().unwrap_or(0.0) > 0.0,
        "effect quads drawn"
    );
    assert!(
        v["counters"]["bones_uploaded"]["max"]
            .as_f64()
            .unwrap_or(0.0)
            > 0.0,
        "bone palettes"
    );
    let creates = v["violations"]["runtime_creates"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !creates.is_empty(),
        "the first effect frame creates its pipelines after the load"
    );
    assert!(
        v["ledger"]["pipelines"]["runtime_creates"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert!(
        v["streaming"]["completed"].as_u64().unwrap_or(0) > 0,
        "request-to-resident latencies"
    );
}

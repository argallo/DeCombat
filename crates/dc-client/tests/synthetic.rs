//! The client loop on the made-up world of `dc_client::testkit` (no ROM needed): the scripted sequence walks
//! forward, climbs the 20-unit step, turns right, jumps and stops. Frames render offscreen on whatever
//! adapter exists (hardware, else the software fallback; skipped without any), with the overlay drawn from
//! the made-up font.
// SPEC: MOVEMENT_2009.md §9.7 (step up to about 30.5), §6.7 (jump), §13.4 (eye); RENDER_2009.md §5

use dc_client::testkit::{synth_font, synth_source, STEP_HEIGHT, STEP_Z, SYNTH_DIMS};
use dc_client::{run_headless, HeadlessOptions, WorldSource};

#[test]
fn synthetic_walk_climbs_jumps_renders_and_never_falls_through() {
    let out = dc_client::headless::default_out_dir().join("synthetic");
    let opts = HeadlessOptions {
        frames: 50,
        frame_us: 100_000, // 6 ticks per frame at 60 Hz
        width: 320,
        height: 240,
        out: Some(out),
        prefix: "synthetic_".into(),
        save_every: 10,
        font_bytes: Some(synth_font()),
        ..HeadlessOptions::default()
    };
    let mut src = synth_source();
    let r = run_headless(&mut src, None, &opts).expect("headless run");
    println!("{}", r.summary());
    assert_eq!(r.ticks, 300, "5 s at 60 Hz");
    assert_eq!(r.nan_ticks, 0);
    assert_eq!(r.nan_frames, 0);
    assert_eq!(r.tunnel_ticks, 0, "the body centre crossed a surface");
    assert!(!r.fell_out);
    assert_eq!(r.jumps, 1);
    assert!(r.landings >= 1);
    assert!(r.ended_supported);
    let standing_on_step = STEP_HEIGHT + SYNTH_DIMS.y;
    assert!(r.end.z > STEP_Z, "walked past the step: {:?}", r.end);
    assert!(
        (r.end.y - standing_on_step).abs() < 1.0,
        "ends standing on the step top: {:?}",
        r.end
    );
    assert!(r.end.x > 100.0, "moved right after the turn: {:?}", r.end);
    assert!(
        r.max_y > standing_on_step + 40.0,
        "the jump rose about 50 units: max y {}",
        r.max_y
    );
    if r.adapter.is_none() {
        println!("no adapter, rendering skipped: {:?}", r.render_skipped);
        return;
    }
    assert_eq!(r.images.len(), 6, "frames 0, 10, ..., 40 and the last");
    assert_eq!(r.font.as_deref(), Some("font bytes"));
    let img = r.last_frame.as_ref().expect("last frame");
    // last frame: facing +X on the step top; the blue side wall spans the rows just above the centre, the
    // brown step top the bottom; the overlay panel (from pixel (8, 8)) darkens what lies under it
    let row = img.height / 2 - 20;
    let wall = img.pixel(3, row);
    let wall_under_panel = img.pixel(20, row);
    let floor = img.pixel(img.width / 2, img.height - 4);
    println!("wall {wall:?} wall under panel {wall_under_panel:?} floor {floor:?}");
    assert!(
        wall[2] as i32 > wall[0] as i32 + 80,
        "side wall (blue) ahead: {wall:?}"
    );
    assert!(
        floor[0] as i32 > floor[2] as i32 + 80,
        "step top (brown) below: {floor:?}"
    );
    assert!(
        (wall_under_panel[2] as i32) < wall[2] as i32 / 2,
        "the panel is blended over the frame: {wall_under_panel:?} vs {wall:?}"
    );
}

#[test]
fn synthetic_world_source_loads_without_a_gpu() {
    let mut src = synth_source();
    let w = src.load(None).expect("load");
    assert_eq!(w.spawns.len(), 1);
    assert_eq!(w.collision.models.len(), 1);
    assert!(w.scene.is_none());
    assert_eq!(w.floor_limit, 0.0);
    assert!(w.spawn(1).is_err());
}

//! DSV-112 against the user's ROM (skipped without it): for every plain mode and every map of it, the room's `trig`
//! setting as our lobby writes it names an end trigger whose camera statements switch a world camera on and move it
//! to a camera point of the same world, and the start triggers never switch one on. Counts only are printed; no ROM
//! value is kept.
// OBSERVED: ROM world scripts and mode files read at run time (2026-10-25)

use dc_dserver::camshot::{trig_fields, trigger_steps, CamAction};
use dc_dserver::world::{load_from_rom, NameForm};

#[test]
fn rom_end_triggers_switch_a_world_camera_on_at_a_point() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let vfs = dc_rez::RezVfs::new(&rom);
    let Ok(gr) = dc_bute::GameRoomData::from_vfs(&vfs) else {
        println!("skipped: no game room data");
        return;
    };
    let unq = |s: &str| s.trim().trim_matches('"').to_string();
    let (mut rooms, mut shots, mut no_camera) = (0, 0, 0);
    for (mode, m) in gr.modes(false).iter().enumerate() {
        let Some(key) = m.get("Key").map(|k| unq(k)) else {
            continue;
        };
        for (i, map) in gr.maps().iter().enumerate() {
            if !map.contains_key(&format!("GMS_{key}")) {
                continue;
            }
            let (_md, doc) = dc_bute::gms_file_with_vfs(&vfs, &gr, i, mode, false);
            let Some(world) = doc.get_str("MapInfo", "Map").map(unq) else {
                continue;
            };
            let cfg = dc_bute::RoomConfig {
                map: i,
                mode,
                ..dc_bute::RoomConfig::default()
            };
            let line =
                dc_bute::room_line_with_vfs(&vfs, &gr, &cfg, &dc_bute::RoomOptions::default());
            let dc_dserver::pipe::Control::Room(r) = dc_dserver::pipe::parse_control(&line) else {
                continue;
            };
            let Ok(w) = load_from_rom(&rom, &world, NameForm::Listed, None) else {
                continue;
            };
            let (starts, end) = trig_fields(r.get("trig"));
            let Some(end) = end else {
                continue;
            };
            rooms += 1;
            let cams = &w.scripts.cameras;
            if cams.cameras.is_empty() {
                no_camera += 1;
                continue;
            }
            let steps = trigger_steps(&w.scripts, cams, end.as_bytes());
            let on = steps.iter().any(|s| s.action == CamAction::On);
            let moved = steps
                .iter()
                .any(|s| matches!(s.action, CamAction::MoveTo(p) if p < cams.points.len()));
            // CLI-610: the blob's second byte (the world's listener switch) is the 2009 client's letterbox kind (1):
            // every end camera draws the bars
            assert!(
                cams.cameras.iter().all(|c| c.listener),
                "mode {mode} map {i}: a camera without the second blob byte set"
            );
            assert!(
                on && moved,
                "mode {mode} map {i}: end trigger steps {steps:?}"
            );
            shots += 1;
            for t in &starts {
                let s = trigger_steps(&w.scripts, cams, t.as_bytes());
                assert!(
                    !s.iter().any(|s| s.action == CamAction::On),
                    "mode {mode} map {i}: a start trigger switches the camera on"
                );
            }
        }
    }
    println!("rooms with an end trigger: {rooms}; with the camera shot: {shots}; without a camera: {no_camera}");
    assert!(shots > 0, "no room with a camera shot");
}

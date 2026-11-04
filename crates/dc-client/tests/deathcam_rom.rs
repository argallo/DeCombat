//! ROM-gated (skipped without CA_ROM): the death camera on WARHEAD keeps its eye in the world when the killer's
//! body sits at our server's visibility park point (playtest report: "the kill cam shows the killer with no map").

use dc_client::deathcam::{in_world, DeathCam, DeathCamMode, DYING_TIME_MS};
use dc_client::look::ViewAngles;
use dc_client::world::{DeathView, RemoteBody};
use dc_client::{RomWorld, WorldSource};
use dc_sim::{CollisionWorld, V3};

fn remote(pid: u8, team: u8, pos: V3) -> RemoteBody {
    RemoteBody {
        obj: 100 + pid as u16,
        pid,
        name: format!("p{pid}"),
        team,
        pos: pos.to_array(),
        half: [16.0, 40.0, 16.0],
        yaw: None,
        look: None,
    }
}

fn bounds(w: &CollisionWorld) -> (V3, V3) {
    let mut lo = V3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut hi = V3::new(f32::MIN, f32::MIN, f32::MIN);
    for m in &w.models {
        let (l, h) = m.bounds();
        lo = V3::new(lo.x.min(l.x), lo.y.min(l.y), lo.z.min(l.z));
        hi = V3::new(hi.x.max(h.x), hi.y.max(h.y), hi.z.max(h.z));
    }
    (lo, hi)
}

/// The eye of the death camera `ms` after the death, killed by player 2.
fn eye_at(w: &CollisionWorld, body: V3, killer: &RemoteBody, ms: u32) -> V3 {
    let mut cam = DeathCam::default();
    let view = ViewAngles {
        yaw: 0.3,
        pitch: 0.1,
    };
    let mut out = None;
    let mut t = 0;
    while t <= ms {
        let d = DeathView {
            since_ms: t,
            killer: Some(2),
            own_pid: Some(1),
            own_team: Some(0),
            shot_dir: Some([0.0, 0.0, 1.0]),
            // the follow camera may take the killer (an FFA class)
            any_team: true,
            ..DeathView::default()
        };
        out = cam.frame(
            Some(&d),
            std::slice::from_ref(killer),
            body,
            body,
            &view,
            Some(w),
            0.016,
        );
        t += 16;
    }
    assert!(matches!(cam.mode(), Some(DeathCamMode::Follow(_))));
    out.expect("a death camera").0
}

#[test]
fn rom_warhead_death_camera_never_follows_a_parked_killer_out_of_the_world() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Ok(world) = RomWorld::new(&rom, "WARHEAD").load(None) else {
        println!("skipped: WARHEAD not in this ROM");
        return;
    };
    let w = &world.collision;
    let (lo, hi) = bounds(w);
    let body = world.spawn(0).expect("a spawn").pos;
    assert!(in_world(Some(w), body), "the own body stands in the world");
    // the killer in the world: the follow camera orbits him, eye in the world
    let near = world.spawn(1).expect("a spawn").pos;
    let k_in = remote(2, 1, near);
    let e_in = eye_at(w, body, &k_in, DYING_TIME_MS + 500);
    assert!(in_world(Some(w), e_in), "eye {e_in:?} in {lo:?}..{hi:?}");
    assert!((e_in - near).len() < 260.0, "the camera follows the killer");
    // the killer parked by the server's visibility culling (bottom corner of the world box): the camera stays in
    // the world (before: it orbited the park point, under the level, and drew the killer alone)
    let parked = remote(2, 1, V3::new(lo.x - 64.0, lo.y - 64.0, lo.z - 64.0));
    assert!(!in_world(Some(w), parked_pos(&parked)));
    let e = eye_at(w, body, &parked, DYING_TIME_MS + 500);
    assert!(in_world(Some(w), e), "eye {e:?} in {lo:?}..{hi:?}");
    assert!(
        w.ray(e, e - V3::new(0.0, hi.y - lo.y + 10.0, 0.0))
            .is_some(),
        "world below the eye"
    );
    let parked_low = remote(2, 1, V3::new(near.x, lo.y, near.z));
    let e2 = eye_at(w, body, &parked_low, DYING_TIME_MS + 500);
    assert!(in_world(Some(w), e2), "eye {e2:?}");
}

fn parked_pos(r: &RemoteBody) -> V3 {
    V3::new(r.pos[0], r.pos[1], r.pos[2])
}

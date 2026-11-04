//! ROM-gated (DSV-475, playtest report "the women characters ... don't have a running or walking animation,
//! they just float around"): the trackers the server streams for a female body (rows 7..9) move its legs as the male
//! body's do. The female bodies list their weight sets in another order than the male ones (the lower-body set is
//! the third, not the second), so the server must name the lower tracker's set by the body's own index. Output:
//! counts only. Skipped without `CA_ROM`.
// EVIDENCE: cshell+0x15ef0 (the player animator looks its upper / lower weight sets up by name)

use dc_dserver::chars::{CharTables, RigCache};
use dc_dserver::game::{AnimSet, Dir, Gait, Motion};
use dc_render::{ModelFiles, Tracker};

/// The number of body nodes whose pose changes between two times of the server's lower-body run tracker, posed by
/// the client with the weight set the server names.
fn moving_nodes(vfs: &dc_rez::RezVfs, t: &CharTables, cache: &mut RigCache, row: u32) -> usize {
    let body = &t.bodies[&row];
    let rig = cache.rig(vfs, &body.model).expect("rig");
    let anims = AnimSet::from_rig(&rig);
    let lower = anims.trackers(Motion::Step(Gait::Run, Dir::Forward), 0, 0)[2];
    let set = ModelFiles::load(vfs, &body.model).expect("body").anim_set();
    let at = |time_ms: f32| {
        set.pose(&[Tracker {
            clip: usize::from(lower.anim),
            time_ms,
            blend_set: lower.secondary.map(|s| usize::from(s.1)),
        }])
    };
    let (a, b) = (at(0.0), at(lower.len_ms as f32 * 0.4));
    a.iter()
        .zip(&b)
        .filter(|(x, y)| x.1.angle_between(y.1) > 0.01 || (x.0 - y.0).length() > 0.01)
        .count()
}

#[test]
fn rom_a_female_body_runs_with_its_legs_like_the_male_body() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let t = CharTables::from_vfs(&vfs).unwrap();
    let mut cache = RigCache::default();
    let male = moving_nodes(&vfs, &t, &mut cache, 0);
    assert!(male >= 8, "male run moves {male} nodes");
    for row in [7, 8, 9] {
        if !t.bodies.contains_key(&row) {
            continue;
        }
        let female = moving_nodes(&vfs, &t, &mut cache, row);
        println!("run: body 0 moves {male} nodes, body {row} {female}");
        assert!(
            female * 2 >= male,
            "body row {row}: the run moves {female} nodes (male {male})"
        );
    }
}

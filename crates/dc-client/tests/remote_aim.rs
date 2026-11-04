//! ROM-gated check of the remote upper-body aim on the default character body: the seven controlled nodes of the
//! 2009 aim (cshell+0x266c0 names them) resolve on the ROM body, and two aim notes in a row, eased as the client
//! eases them, turn the posed standing body: looking down tips the head and the right shoulder down, looking up
//! tips them up, the yaw offset twists the upper torso, the pelvis stays. Skipped without `CA_ROM`; no ROM value is
//! printed or asserted (only directions).
// EVIDENCE: cshell+0x26f60, cshell+0x256f0, cshell+0x263f0 (see dc-client src/aim.rs)

use dc_client::aim::{self, AimNote, AimState, NODES};
use dc_client::characters::ModelScene;
use dc_render::{name_key, ModelFiles, Tracker, CLIP_LOWER_STAND, CLIP_UPPER_STAND};

#[test]
fn rom_body_turns_its_upper_body_with_two_aim_notes() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let models = ModelScene::open(&rom).expect("ROM models");
    let body = models.default_look();
    assert!(!body.is_empty(), "the default character resolves");
    let vfs = dc_rez::RezVfs::new(&rom);
    let anim = ModelFiles::load(&vfs, &body[0].model)
        .expect("body model")
        .anim_set();
    let sk = &anim.skeleton;
    let found: Vec<Option<usize>> = NODES.iter().map(|n| aim::node_of(sk, n)).collect();
    for (n, f) in NODES.iter().zip(&found) {
        assert!(f.is_some(), "body node {:?} of the 2009 aim", n.names);
    }
    let [pelvis, _, upper, _, head, _, shoulder] =
        [0, 1, 2, 3, 4, 5, 6].map(|i| found[i].expect("node"));
    let clip = |k| anim.find_clip(|n| name_key(n) == k).expect("player clip");
    let pose = anim.pose(&[
        Tracker {
            clip: 0,
            time_ms: 0.0,
            blend_set: None,
        },
        Tracker {
            clip: clip(CLIP_UPPER_STAND),
            time_ms: 0.0,
            blend_set: Some(0),
        },
        Tracker {
            clip: clip(CLIP_LOWER_STAND),
            time_ms: 0.0,
            blend_set: Some(1),
        },
    ]);
    let standing = sk.globals(&pose);
    let posed = |a: [f32; 2]| {
        let mut g = standing.clone();
        aim::apply(sk, &mut g, a);
        g
    };
    let fwd = |g: &[glam::Mat4], i: usize| aim::node_forward(g, i).unwrap();
    let height = |g: &[glam::Mat4], i: usize| g[i].w_axis.y;
    // the model faces +Z: a point ahead of the head moves down when the body looks down
    let ahead = |g: &[glam::Mat4], i: usize| {
        let base = standing[i].inverse();
        // the point 20 units ahead (model +Z) of the node in its standing place, carried by the node's turn
        let p = standing[i].w_axis.truncate() + glam::Vec3::Z * 20.0;
        g[i].transform_point3(base.transform_point3(p))
    };
    let mut s = AimState::default();
    s.on_note(AimNote {
        pitch: 0.6,
        ..Default::default()
    });
    s.advance(0.25);
    let down = posed(s.drawn());
    s.on_note(AimNote {
        pitch: -0.6,
        yaw: 0.4,
        ..Default::default()
    });
    s.advance(0.25);
    let up = posed(s.drawn());
    let rest = posed([0.0, 0.0]);
    assert_eq!(rest, standing, "no aim, the stream's pose");
    assert_eq!(down[pelvis], standing[pelvis], "the pelvis keeps its place");
    for i in [head, shoulder] {
        let (d, u, r) = (ahead(&down, i), ahead(&up, i), ahead(&rest, i));
        assert!(
            d.y < r.y - 1.0,
            "node {i}: looking down lowers the point ahead"
        );
        assert!(u.y > r.y + 1.0, "node {i}: looking up raises it");
    }
    assert!(height(&down, head) < height(&rest, head), "the head bows");
    // the second note's yaw offset twists the upper torso about +Y
    let (f0, f1) = (fwd(&rest, upper), fwd(&up, upper));
    let turn = |f: glam::Vec3| f.x.atan2(f.z);
    assert!(
        (turn(f1) - turn(f0)).abs() > 0.2,
        "the upper torso twists with the yaw offset"
    );
}

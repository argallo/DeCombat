//! ROM-gated: the first-person model of the knife and the grenade stands where the 2009 rule puts it (the user's
//! 2026-10-27 report and audit M3 / M6: "the knife too high, the grenade too far off screen"). The rule: eye-space
//! origin = the weapon row's `Pos` + the playing clip's offset from the clip overrides of the model files
//! (FMT_LTB.md §11.2, engine+0x6e0d0), and the clip poses the rig as authored (its root translation included).
//! ROM values used (read from the user's ROM, 2026-10-27): idle clip offsets rifle 145 (-0.4, -3.6, -1.2) (= the
//! measured 2009 placement of CLI-330 to 0.01), knife 0 none (0, 0, 0), grenade 7 (-0.7, -8.0, -1.0); rows' `Pos`:
//! knife (0, 0, 0), grenade (0.5, -0.9, 1.5). With the rule the rig roots of rifle, pistol, knife and grenade stand at
//! eye-space height -3.85 .. -3.5; the old placement (rifle constant + rig root shift removed) put the knife's 0.6 higher
//! and 0.68 nearer and the grenade's 0.35 lower. Skipped without `CA_ROM` or a GPU adapter.
// SPEC: FMT_LTB.md §11.2; RENDER_2009.md §4.2; EVIDENCE engine+0x6e0d0, cshell+0x232c10 (DISASM)

use dc_client::arms::{WeaponClips, WeaponTable};
use dc_client::characters::ModelScene;
use dc_client::{ModelRef, ViewWeapon};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use glam::Vec3;

#[test]
fn rom_knife_and_grenade_view_models_follow_the_clip_offset_rule() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let table = WeaponTable::open(&rom).unwrap();
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut models = ModelScene::open(&rom).unwrap();
    let (w, h) = (1024u32, 768u32);
    let pv = dc_render::PLAYER_VIEW_DEFAULT.projection(w, h);
    let px = |p: Vec3| {
        let c = pv * p.extend(1.0);
        let n = c.truncate() / c.w;
        glam::Vec2::new((n.x * 0.5 + 0.5) * w as f32, (0.5 - n.y * 0.5) * h as f32)
    };
    // (weapon, idle clip offset read from the ROM model files)
    let cases = [
        (145u16, Vec3::new(-0.4, -3.6, -1.2)),
        (164, Vec3::new(-0.4, -3.6, -1.0)),
        (0, Vec3::ZERO),
        (7, Vec3::new(-0.7, -8.0, -1.0)),
    ];
    for (weapon, offset) in cases {
        let row = table.get(weapon).expect("weapon row").clone();
        let (pv_model, clips) = row
            .pv_model
            .iter()
            .chain(row.pv_models.iter())
            .find_map(|p| WeaponClips::load(&vfs, p).ok().map(|c| (p.clone(), c)))
            .expect("clips");
        let pos = row.pv_pos.unwrap_or([0.0; 3]);
        let vw = ViewWeapon {
            inspect: None,
            weapon,
            model: Some(ModelRef {
                model: pv_model,
                textures: row.pv_skins.clone(),
            }),
            clip: clips.idle.as_ref().map(|c| (c.index, 0.0)),
            pos,
        };
        for _ in 0..200 {
            models.place_view_weapon(&ctx, &renderer, Some(&vw), Vec3::ZERO, None);
            if models.stats.view_weapon {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(models.stats.view_weapon, "{weapon}: posed");
        let inst = models.view_instances()[0];
        let want = Vec3::from(pos) + offset;
        assert!(
            inst.position.distance(want) < 1e-4,
            "{weapon}: origin {:?}, the 2009 rule gives {want:?}",
            inst.position
        );
        // the rig root (the root's child with the most nodes) as posed, in eye space
        let sk = &inst.anim.skeleton;
        let root = (0..sk.parents.len())
            .find(|i| sk.parents[*i].is_none())
            .unwrap();
        let rig = (0..sk.parents.len())
            .filter(|i| sk.parents[*i] == Some(root))
            .max_by_key(|i| {
                (0..sk.parents.len())
                    .filter(|j| is_below(sk, *j, *i))
                    .count()
            })
            .unwrap();
        let rig_eye = inst.position + inst.globals[rig].w_axis.truncate();
        // the screen point of the posed nodes in front of the eye (their mean; arms, hands and the weapon)
        let front: Vec<Vec3> = inst
            .globals
            .iter()
            .map(|g| inst.position + g.w_axis.truncate())
            .filter(|p| p.z > 0.5)
            .collect();
        let mean = front.iter().copied().sum::<Vec3>() / front.len().max(1) as f32;
        let at = px(mean);
        println!(
            "{weapon}: origin {:?}, rig root eye {rig_eye:?}, {} nodes in front, their mean {mean:?} at {at:?} px ({w}x{h})",
            inst.position,
            front.len()
        );
        // the arms reach in from the lower edge: their mean point lies within the width and at most a quarter
        // height below the frame (the rifle's: 771 of 768)
        assert!(
            at.x >= 0.0 && at.x <= w as f32 && at.y >= 0.0 && at.y <= 1.25 * h as f32,
            "{weapon}: the posed arms are at the screen ({at:?})"
        );
        assert!(
            (-3.9..=-3.4).contains(&rig_eye.y),
            "{weapon}: rig root height {}",
            rig_eye.y
        );
    }
}

fn is_below(sk: &dc_render::Skeleton, mut j: usize, i: usize) -> bool {
    while let Some(p) = sk.parents[j] {
        if p == i {
            return true;
        }
        j = p;
    }
    false
}

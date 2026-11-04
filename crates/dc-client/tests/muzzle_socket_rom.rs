//! ROM-gated: the first-person muzzle flash sits at the posed weapon's muzzle socket (the user's 2026-10-27 report:
//! "muzzle flash was not at the right location on the gun"). The flash group's keys are placed on the player-view
//! muzzle socket and moved by their offset in the frame of the first-person model (unrotated in eye space), not in
//! the socket's own frame: the rifle 145's socket frame is turned a quarter round (its +Z points left), so the old
//! placement put the flash 0.5 eye units beside the barrel (about 56 px at 640 x 480). Skipped without `CA_ROM` or a
//! GPU adapter; nothing of the ROM is asserted by value except the derived positions.
// DISASM: clientfx+0x1fe7..0x20a0, clientfx+0x1f70 case 6; SPEC: RENDER_2009.md §4.2

use dc_client::arms::{RayHit, ShotFx, WeaponClips, WeaponTable};
use dc_client::characters::ModelScene;
use dc_client::fx::{FxEvent, FxStage, PARTICLE_HALF_PER_SIZE, VIEW_PARENT_ROT};
use dc_client::{ModelRef, ViewWeapon};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use glam::Vec3;

#[test]
fn rom_the_muzzle_flash_sits_on_the_posed_muzzle_socket() {
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
    let (w, h) = (640u32, 480u32);
    let pv = dc_render::PLAYER_VIEW_DEFAULT.projection(w, h);
    let px = |p: Vec3| {
        let c = pv * p.extend(1.0);
        let n = c.truncate() / c.w;
        glam::Vec2::new((n.x * 0.5 + 0.5) * w as f32, (0.5 - n.y * 0.5) * h as f32)
    };
    let camera = dc_render::Camera::looking(Vec3::ZERO, 0.0, 0.0, 1.6, 1.2);
    for weapon in [145u16, 164, 27] {
        let row = table.get(weapon).expect("weapon row").clone();
        let (pv_model, clips) = row
            .pv_model
            .iter()
            .chain(row.pv_models.iter())
            .find_map(|p| WeaponClips::load(&vfs, p).ok().map(|c| (p.clone(), c)))
            .expect("clips");
        let vw = ViewWeapon {
            weapon,
            model: Some(ModelRef {
                model: pv_model,
                textures: row.pv_skins.clone(),
            }),
            clip: clips.fire.as_ref().map(|c| (c.index, 0.0)),
            pos: row.pv_pos.unwrap_or([0.0; 3]),
        };
        for _ in 0..200 {
            models.place_view_weapon(&ctx, &renderer, Some(&vw), Vec3::ZERO, None);
            if models.stats.view_weapon {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            models.stats.view_weapon,
            "{weapon}: the first-person model is posed"
        );
        let mut stage = FxStage::open(Some(&rom));
        let sock = stage.view_socket(&models).expect("the muzzle socket");
        let (_, _, sock_pos) = sock.to_scale_rotation_translation();
        let group = stage
            .runner
            .tables
            .muzzle
            .get(&weapon)
            .and_then(|m| m.0.clone())
            .and_then(|n| stage.runner.library.get(&n))
            .expect("the first-person muzzle group");
        let key = group
            .keys
            .iter()
            .find(|k| matches!(k.kind, dc_client::fx::KeyKind::Particles(_)))
            .expect("a particle key");
        // the 2009 place: the socket position plus the key offset turned by the first-person model's rotation
        let want = sock_pos + VIEW_PARENT_ROT * Vec3::from(key.offset);
        let shot = ShotFx {
            weapon,
            ammo: row.ammo.unwrap_or(weapon),
            origin: [0.0; 3],
            muzzle: [0.0; 3],
            dir: [0.0, 0.0, 1.0],
            range: 100.0,
            hit: RayHit::Miss,
            first: true,
            time_ms: 1000,
            impact: None,
            flash_mod: None,
        };
        stage.runner.feed(1000, &[FxEvent::Shot(shot)], None);
        let d = stage.runner.draw(1000, &camera, Some(sock));
        assert!(!d.view.is_empty(), "{weapon}: eye-space quads of the flash");
        let mean = d.view.iter().map(|q| q.centre).sum::<Vec3>() / d.view.len() as f32;
        let off = px(mean) - px(want);
        println!(
            "{weapon}: socket {sock_pos:?} -> flash wanted {want:?} ({:?} px), drawn mean {mean:?} ({:?} px), {} quads",
            px(want),
            px(mean),
            d.view.len()
        );
        assert!(
            off.length() < 2.0,
            "{weapon}: the flash is {:.2} units / {:.1} px from the socket place",
            mean.distance(want),
            off.length()
        );
        // the size (arms-fixes2, CLI-591): at its first frame the particle's size is the first scale key, and the
        // engine's corners are centre +- 2 size right +- 2 size up (engine+0x1bb310 / 0x1babb0), drawn with the
        // player-view projection
        let size0 = key.scales.first().map(|s| s.1).expect("a scale key");
        let half_eye = size0 * 2.0;
        let fy = dc_render::PLAYER_VIEW_DEFAULT.fov_y_deg.to_radians();
        let want_half_px = half_eye / (fy * 0.5).tan() / want.z * h as f32 * 0.5;
        let q = d
            .view
            .iter()
            .find(|q| q.texture.is_some())
            .expect("the textured flash quad");
        let drawn_half_px = (px(q.centre + q.u) - px(q.centre)).length();
        println!(
            "{weapon}: flash half size wanted {want_half_px:.1} px (size {size0}, depth {:.2}), drawn {drawn_half_px:.1} px",
            want.z
        );
        assert!(
            (drawn_half_px - want_half_px).abs() < 2.0,
            "{weapon}: flash half size {drawn_half_px:.1} px, the engine's rule gives {want_half_px:.1} px"
        );
        assert_eq!(PARTICLE_HALF_PER_SIZE, 2.0);
    }
}

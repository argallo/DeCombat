//! ROM-gated GPU test (no server): the scene a frame is drawn from drops a character the moment it turns hidden
//! (its death: the character object goes to its hidden flags and the corpse object appears in the same group) and
//! builds it again at once when it shows again (its respawn), with inline and with threaded model loads (the
//! window's mode). Before, the scene kept the built models of a hidden character, so its last living pose stayed
//! drawn, upright and frozen, next to its falling corpse until the respawn (user retest 2026-10-26 item 7).
//! Skipped without `CA_ROM` or without a GPU adapter; nothing of the ROM is asserted by value.
// SPEC: UDP_SESSION_2009_B.md §3.5.5 (death group: the character with flags 0x0802, then the corpse create)
// DC-DEFAULT: CLI-42 (proposed), CLI-350 (proposed)

use dc_client::anims::AnimFrame;
use dc_client::characters::ModelScene;
use dc_client::{RemoteBody, RemoteLook, RemoteTracker};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};

const CHARACTER: u16 = 75;
const CORPSE: u16 = 80;

fn body(obj: u16, pid: u8, look: &[dc_client::ModelRef], visible: bool) -> RemoteBody {
    RemoteBody {
        obj,
        pid,
        name: String::new(),
        team: 0,
        pos: [0.0, 0.0, 0.0],
        half: [16.0, 40.0, 16.0],
        yaw: Some(0.0),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: look.to_vec(),
            weapon: None,
            trackers: vec![RemoteTracker {
                clip: 0,
                time_ms: 0.0,
                blend_set: None,
            }],
            visible,
            aim: None,
        }),
    }
}

#[test]
fn rom_hidden_character_leaves_the_scene_at_once() {
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
    for threaded in [false, true] {
        let mut scene = ModelScene::open(&rom).expect("ROM models");
        scene.set_async_loads(threaded);
        let look = scene.default_look();
        assert!(!look.is_empty(), "the default character");
        let frame = AnimFrame::default();
        let alive = [body(CHARACTER, 1, &look, true)];
        // the living character, until its models are built (threaded loads take a few frames)
        let mut built = false;
        for _ in 0..2000 {
            scene.update_remotes_animated(&ctx, &renderer, &alive, &frame, None);
            if !scene.instances().is_empty() {
                built = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(built, "threaded {threaded}: the character was built");
        assert_eq!(scene.drawn(), vec![CHARACTER]);
        let n_alive = scene.instances().len();

        // the death group: the character hidden, its corpse created (the same body, no face)
        let dead = [
            body(CHARACTER, 1, &look, false),
            body(CORPSE, 0xFF, &look[..1], true),
        ];
        scene.update_remotes_animated(&ctx, &renderer, &dead, &frame, None);
        let d = scene.drawn();
        assert!(
            !d.contains(&CHARACTER),
            "threaded {threaded}: the hidden character is not drawn in the corpse's frame: {d:?}"
        );
        assert!(
            d.contains(&CORPSE),
            "threaded {threaded}: the corpse is drawn (or loading): {d:?}"
        );
        // and it stays out while dead
        for _ in 0..10 {
            scene.update_remotes_animated(&ctx, &renderer, &dead, &frame, None);
            assert!(!scene.drawn().contains(&CHARACTER));
        }

        // the respawn: shown again, rebuilt from the upload cache in the same frame
        let respawned = [
            body(CHARACTER, 1, &look, true),
            body(CORPSE, 0xFF, &look[..1], true),
        ];
        scene.update_remotes_animated(&ctx, &renderer, &respawned, &frame, None);
        let d = scene.drawn();
        assert!(
            d.contains(&CHARACTER) && d.contains(&CORPSE),
            "threaded {threaded}: the respawned character is drawn at once: {d:?}"
        );
        assert!(scene.instances().len() >= n_alive);
        println!("threaded {threaded}: hidden at the corpse's frame, back at the respawn");
    }
}

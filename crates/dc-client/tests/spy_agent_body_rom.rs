//! ROM-gated (CLI-1592, playtest report "the superspy's body is purple"): the agent wears the spy body with
//! the render style `RS\EdgeAmp01.ltb`, whose second stage reads the piece's texture slot 1; the spy body's skin list
//! fills slot 0 only. The 2009 engine binds no texture there (black); the client must not draw its debug checker.
use dc_client::characters::ModelScene;
use dc_client::world::{RemoteBody, RemoteLook};
use dc_client::ModelRef;
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};

#[test]
fn rom_the_agent_body_binds_black_not_the_checker_on_its_empty_texture_slot() {
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
    let skins = vec![
        "CHARS_T_BODY\\SPY_Upper.dtx".to_string(),
        "CHARS_T_BODY\\SPY_Leg.dtx".to_string(),
    ];
    let cases = [
        ("spy", vec!["RS\\default.ltb", "RS\\GLOW.LTB", "RS\\MARKTRANSPARENT.LTB"], 0u64),
        ("agent", vec!["RS\\EdgeAmp01.ltb", "RS\\GLOW.LTB", "RS\\MARKTRANSPARENT.LTB"], 2u64),
    ];
    for (i, (label, styles, unbound)) in cases.iter().enumerate() {
        let mut models = ModelScene::open(&rom).expect("ROM models");
        let obj = 9 + i as u16;
        let remote = RemoteBody {
            obj,
            pid: 2,
            name: label.to_string(),
            team: 2,
            pos: [0.0; 3],
            half: [16.0, 50.0, 16.0],
            yaw: Some(0.0),
            look: Some(RemoteLook {
                styles: styles.iter().map(|s| s.to_string()).collect(),
                models: vec![ModelRef {
                    model: "CHARS_M_BODY\\SPY_BODY.ltb".into(),
                    textures: skins.clone(),
                }],
                weapon: None,
                trackers: Vec::new(),
                visible: true,
                aim: None,
            }),
        };
        for _ in 0..300 {
            models.update_remotes(&ctx, &renderer, std::slice::from_ref(&remote), None);
            if !models.instances().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let inst = models.instances();
        assert!(!inst.is_empty(), "{label}: the body never loaded");
        let st = &inst[0].style_stats;
        assert_eq!(st.pieces, 2, "{label}: {st:?}");
        assert_eq!(st.default_pieces, 0, "{label}: every piece has its style: {st:?}");
        // the agent style's stage 1 reads slot 1 on both pieces: two unbound slots, drawn black (not the checker)
        assert_eq!(inst[0].missing_textures, *unbound, "{label}: unbound texture slots: {st:?}");
        assert_eq!(st.stage0_ops.len(), 1, "{label}: one stage-0 op: {st:?}");
    }
}

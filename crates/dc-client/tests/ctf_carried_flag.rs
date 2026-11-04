//! ROM- and GPU-gated (skipped without `CA_ROM` or a GPU adapter): the carried capture flag (DSV-217, CLI-1072). A
//! remote character whose attachment list names a flag pickup draws the flag row's carried model at the flag row's
//! body socket (found by index on the body model, as the server sends it), standing at the body; without the list
//! nothing is drawn on him. Nothing of the ROM is asserted by value and no ROM name is printed.
// DC-DEFAULT: DSV-217 (proposed), CLI-1072 (proposed)

use std::collections::BTreeMap;

use dc_client::characters::ModelScene;
use dc_client::menu::figure::{FigureTables, LookIds};
use dc_client::{ModelRef, RemoteBody, RemoteLook};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};

#[test]
fn rom_the_carried_flag_hangs_at_its_body_socket() {
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
    let vfs = dc_rez::RezVfs::new(&rom);
    let looks = dc_dserver::rom_tables::FlagLooks::from_vfs(&vfs).expect("flag rows");
    let (carried_model, flag_socket) = looks
        .carried
        .get(&1)
        .cloned()
        .expect("the carried flag row");
    let skins = looks.rows.get(&1).map(|r| r.0.clone()).unwrap_or_default();
    let flag = ModelRef {
        model: carried_model.clone(),
        textures: skins,
    };
    // the default look's body and the socket index the server would send
    let tables = FigureTables::from_rom(&rom);
    let (look, _) = tables
        .look(&LookIds::from_equipped(&[-1; 15]))
        .expect("the default look");
    let body = look.models[0].model.clone();
    let (rez, entry) = dc_dserver::chars::archive_of(&body).expect("archive");
    let bytes = vfs.entry(&rez, &entry).unwrap().expect("body file");
    let m = dc_model::Model::parse_without_tracks(&bytes).expect("body model");
    let index = m
        .sockets
        .iter()
        .position(|s| String::from_utf8_lossy(&s.name.0).eq_ignore_ascii_case(&flag_socket))
        .expect("the body has the flag socket") as u32;
    // in the match: a remote character with the flag in its attachment list
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut models = ModelScene::open(&rom).expect("ROM models");
    let remote = RemoteBody {
        obj: 7,
        pid: 1,
        name: "Carrier".into(),
        team: 1,
        pos: [0.0; 3],
        half: [16.0, 50.0, 16.0],
        yaw: Some(0.0),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: look.models.clone(),
            weapon: None,
            trackers: Vec::new(),
            visible: true,
            aim: None,
        }),
    };
    let lists = BTreeMap::from([(7u16, vec![(flag.clone(), index)])]);
    let mut drawn = Vec::new();
    for _ in 0..300 {
        models.update_remotes(&ctx, &renderer, std::slice::from_ref(&remote), None);
        models.place_carried(&ctx, &renderer, &lists, &BTreeMap::new());
        drawn = models.body_pieces_drawn(7);
        if !drawn.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(drawn.len(), 1, "the flag is drawn on the carrier");
    assert!(drawn[0].0.eq_ignore_ascii_case(&carried_model));
    assert!(drawn[0]
        .1
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case(&flag_socket)));
    // the flag model stands at the body: its lowest point lies inside the body's box (the socket is on the body)
    let inst = models.instances();
    let (blo, bhi) = inst[0].bounds_in_world().expect("body bounds");
    let (flo, _) = inst.last().unwrap().bounds_in_world().expect("flag bounds");
    let near = |v: f32, lo: f32, hi: f32| v >= lo - 16.0 && v <= hi + 16.0;
    assert!(
        near(flo.y, blo.y, bhi.y),
        "the flag starts at the body: {flo:?} vs {blo:?}..{bhi:?}"
    );
    // dropped: nothing on him any more
    models.place_carried(&ctx, &renderer, &BTreeMap::new(), &BTreeMap::new());
    assert!(models.body_pieces_drawn(7).is_empty());
}

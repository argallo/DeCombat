//! doors-gear-r13 item 2 (found by shop-r13 item 2: the match body drew body and face only): a remote character whose
//! roster look names head gear, goggles and a backpack wears the same pieces the shop figure wears for that
//! equipment, through the same piece code (CLI-1231); a look without gear wears none of them, and the pieces stay
//! built while the look is unchanged. ROM- and GPU-gated; no ROM name is printed or asserted by value.
// DC-DEFAULT: CLI-1231 (proposed), DSV-341 (proposed)

use std::collections::BTreeMap;

use dc_client::characters::ModelScene;
use dc_client::menu::figure::{FigureTables, GearIds, LookIds};
use dc_client::{RemoteBody, RemoteLook};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};

/// A look with head gear, goggles and a backpack that the tables dress with three pieces.
fn geared(t: &FigureTables) -> Option<(LookIds, usize)> {
    let base = LookIds::from_equipped(&[-1; 15]);
    let bare = t.look(&base)?.1.len();
    for h in 1..60 {
        for g in 0..40 {
            for b in 0..40 {
                let ids = LookIds {
                    head: h,
                    goggle: Some(g),
                    backpack: Some(b),
                    ..base
                };
                let n = t.look(&ids)?.1.len();
                if n >= 3 && n > bare {
                    return Some((ids, n));
                }
            }
        }
    }
    None
}

#[test]
fn rom_a_remote_character_wears_its_look_gear() {
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
    let tables = FigureTables::from_rom(&rom);
    let (ids, n) = geared(&tables).expect("a look with three gear pieces");
    println!("gear ids head {} goggle {:?} backpack {:?}", ids.head, ids.goggle, ids.backpack);
    let (look, shop_pieces) = tables.look(&ids).expect("the look");
    // the roster look bytes the server writes for that equipment (DSV-341)
    let mut bytes = [0i8, 0, 0, 0, 0, 1, 0, 0, 0, 0, -1, -1, 0, 0, -1];
    bytes[7] = ids.head as i8;
    bytes[8] = ids.uniform as i8;
    bytes[10] = ids.backpack.unwrap() as i8;
    bytes[11] = ids.goggle.unwrap() as i8;
    let gear = GearIds::from_look(&bytes);
    assert_eq!(gear.head, ids.head);
    assert_eq!((gear.goggle, gear.backpack), (ids.goggle, ids.backpack));
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut models = ModelScene::open(&rom).expect("ROM models");
    let remote = RemoteBody {
        obj: 9,
        pid: 2,
        name: "Geared".into(),
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
    let gears = BTreeMap::from([(9u16, gear)]);
    let mut drawn = Vec::new();
    for _ in 0..300 {
        models.update_remotes(&ctx, &renderer, std::slice::from_ref(&remote), None);
        models.place_carried(&ctx, &renderer, &BTreeMap::new(), &gears);
        drawn = models.body_pieces_drawn(9);
        if drawn.len() == n {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let want: Vec<(String, Option<String>)> = shop_pieces
        .iter()
        .map(|p| (p.model.model.clone(), p.socket.clone()))
        .collect();
    assert_eq!(drawn, want, "the match body wears the shop figure's pieces");
    // the gear hangs on the body: every piece's box meets the body's box
    let inst = models.instances();
    let (blo, bhi) = inst[0].bounds_in_world().expect("body bounds");
    for p in inst.iter().skip(inst.len() - n) {
        let (lo, hi) = p.bounds_in_world().expect("piece bounds");
        assert!(
            lo.y <= bhi.y + 16.0 && hi.y >= blo.y - 16.0,
            "piece {lo:?}..{hi:?} vs body {blo:?}..{bhi:?}"
        );
    }
    // a look without gear: the head row 0 hair only, no goggles, no backpack
    let bare = BTreeMap::from([(9u16, GearIds::default())]);
    models.place_carried(&ctx, &renderer, &BTreeMap::new(), &bare);
    let base = tables.look(&LookIds::from_equipped(&[-1; 15])).unwrap().1;
    for _ in 0..300 {
        models.update_remotes(&ctx, &renderer, std::slice::from_ref(&remote), None);
        models.place_carried(&ctx, &renderer, &BTreeMap::new(), &bare);
        if models.body_pieces_drawn(9).len() == base.len() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(models.body_pieces_drawn(9).len(), base.len());
    // no look record at all: no pieces
    models.place_carried(&ctx, &renderer, &BTreeMap::new(), &BTreeMap::new());
    assert!(models.body_pieces_drawn(9).is_empty());
}

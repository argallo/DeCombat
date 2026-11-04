//! ROM-gated: the mod models of the user's round-4 loadout are drawn on the first-person weapon at their sockets
//! (the user's "the pistol makes the suppressor sound but it doesn't appear on the gun, same with its sight; G36E
//! likewise; L96 suppressor not shown"). The L96 (27) with the sniper scope 27, silencer 33, magazine 53; the
//! pistol (164) with silencer 34, sight 40, magazine 55; the G36E (25) with silencer 31, magazine 51. Without mods
//! nothing is attached. Skipped without `CA_ROM` or a GPU adapter.
// EVIDENCE: cshell+0x237983..0x237b9e, cshell+0x234380, cshell+0x23c330 (see `dc_client::attach`)

use dc_client::arms::{WeaponClips, WeaponTable};
use dc_client::characters::ModelScene;
use dc_client::{ModelRef, ViewWeapon};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use glam::Vec3;

#[test]
fn rom_the_loadouts_mod_models_sit_on_their_first_person_sockets() {
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
    let cases: [(u16, &[u16], &[(&str, &str)]); 3] = [
        (27, &[27, 33, 53], &[("pv_sr_silencer_c.ltb", "flash")]),
        (
            164,
            &[34, 40, 55],
            &[
                ("pv_hand_silencerc.ltb", "flash"),
                ("pv_hand_hds-4.ltb", "breach"),
            ],
        ),
        (25, &[31, 51], &[("pv_silencerc.ltb", "flash")]),
    ];
    for (weapon, parts, want) in cases {
        let row = table.get(weapon).expect("weapon row").clone();
        let (pv_model, clips) = row
            .pv_model
            .iter()
            .chain(row.pv_models.iter())
            .find_map(|p| WeaponClips::load(&vfs, p).ok().map(|c| (p.clone(), c)))
            .expect("clips");
        let vw = ViewWeapon {
            inspect: None,
            weapon,
            model: Some(ModelRef {
                model: pv_model.clone(),
                textures: row.pv_skins.clone(),
            }),
            clip: clips.select.as_ref().map(|c| (c.index, 0.0)),
            pos: row.pv_pos.unwrap_or([0.0; 3]),
        };
        // the parts filed by their rows' `Kinds` (muzzle, sight, magazine)
        let mut sockets = [None; 3];
        for p in parts {
            if let Some(s) = table.mod_info.get(p).and_then(|m| m.attach_socket()) {
                sockets[s] = Some(*p);
            }
        }
        let list = table.attach.view_attachments(weapon, sockets);
        for with in [true, false] {
            let list = if with { list.clone() } else { Vec::new() };
            let mut drawn = Vec::new();
            for _ in 0..300 {
                models.place_view_weapon(&ctx, &renderer, Some(&vw), Vec3::ZERO, None);
                models.place_view_attachments(&ctx, &renderer, &list);
                drawn = models.view_attachments_drawn();
                if models.stats.view_weapon && drawn.len() >= list.len() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert!(
                models.stats.view_weapon,
                "{weapon}: the first-person model is posed"
            );
            println!(
                "{weapon} {:?} with mods {with}: {drawn:?}",
                models.view_model_path()
            );
            if !with {
                assert!(drawn.is_empty(), "{weapon}: no mods, no attachment");
                assert_eq!(models.view_instances().len(), 1);
                continue;
            }
            let mut got: Vec<(String, String)> = drawn
                .iter()
                .map(|(m, s, _)| {
                    let file = m
                        .rsplit(['\\', '/'])
                        .next()
                        .unwrap_or(m)
                        .to_ascii_lowercase();
                    (file, s.to_ascii_lowercase())
                })
                .collect();
            got.sort();
            let mut want: Vec<(String, String)> = want
                .iter()
                .map(|(m, s)| (m.to_string(), s.to_string()))
                .collect();
            want.sort();
            assert_eq!(got, want, "{weapon}: the mod models at their sockets");
            assert_eq!(models.view_instances().len(), 1 + want.len());
            // each sits at its socket of the posed model moved by the row's offset in the socket's frame
            let inst = models.view_instances()[0];
            let drawn_model = models
                .view_model_path()
                .expect("the drawn model")
                .to_string();
            let files = dc_render::read_rom_entry(&vfs, &drawn_model)
                .ok()
                .and_then(|b| dc_model::Model::parse(&b).ok())
                .expect("the first-person model file");
            for (a, (_, _, pos)) in list.iter().zip(&drawn) {
                let i = files
                    .sockets
                    .iter()
                    .position(|s| s.name.as_bytes().eq_ignore_ascii_case(a.socket.as_bytes()))
                    .expect("the socket");
                let at = inst.socket_world(&files, i).expect("socket transform")
                    * glam::Mat4::from_translation(Vec3::from(a.pos_off));
                let want = at.transform_point3(Vec3::ZERO);
                assert!(
                    pos.distance(want) < 1e-3,
                    "{weapon}: {} at {pos:?}, socket place {want:?}",
                    a.model.model
                );
            }
        }
    }
}

/// CLI-635: a mod row's `AttachScale` scales its first-person model (cshell+0x237b68..0x237b91): the G36E's
/// silencer with scale 2 has twice the drawn extent of the same model with scale 1, at the same place.
#[test]
fn rom_attach_scale_two_draws_the_mod_model_at_double_size() {
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
    let scaled: Vec<_> = table
        .attach
        .mods
        .iter()
        .filter(|(_, m)| m.scale != [1.0; 3])
        .map(|(n, m)| (*n, m.scale))
        .collect();
    println!("ROM mod rows with an AttachScale other than 1: {scaled:?}");
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut models = ModelScene::open(&rom).unwrap();
    let weapon = 25;
    let row = table.get(weapon).expect("weapon row").clone();
    let (pv_model, clips) = row
        .pv_model
        .iter()
        .chain(row.pv_models.iter())
        .find_map(|p| WeaponClips::load(&vfs, p).ok().map(|c| (p.clone(), c)))
        .expect("clips");
    let vw = ViewWeapon {
        inspect: None,
        weapon,
        model: Some(ModelRef {
            model: pv_model,
            textures: row.pv_skins.clone(),
        }),
        clip: clips.select.as_ref().map(|c| (c.index, 0.0)),
        pos: row.pv_pos.unwrap_or([0.0; 3]),
    };
    let base = table
        .attach
        .view_attachments(weapon, [Some(31), None, None]);
    assert_eq!(base.len(), 1, "the G36E silencer is drawn");
    let mut extent = |scale: f32| {
        let mut list = base.clone();
        list[0].scale = [scale; 3];
        for _ in 0..300 {
            models.place_view_weapon(&ctx, &renderer, Some(&vw), Vec3::ZERO, None);
            models.place_view_attachments(&ctx, &renderer, &list);
            if models.view_instances().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let inst = models.view_instances()[1];
        let (lo, hi) = inst.bounds_in_world().expect("bounds");
        (hi - lo, inst.position)
    };
    let (one, at_one) = extent(1.0);
    let (two, at_two) = extent(2.0);
    assert!(one.length() > 0.0);
    assert!(
        (two - one * 2.0).length() < 1e-3 * one.length().max(1.0),
        "extent {two:?} vs 2 x {one:?}"
    );
    assert!(at_one.distance(at_two) < 1e-4, "same place");
}

/// CLI-645: the scope hides the first-person weapon and its mod models (cshell+0x1f3520 -> cshell+0x22d350 on a
/// zoom-in from level 0; cshell+0x1f9100 -> cshell+0x22d320 when the zoom-out ends): the L96 (27) with its sniper
/// scope (mod 27) zoomed has an empty player-view draw set while the model stays placed for the muzzle socket;
/// unzoomed the set holds the weapon and its mods.
#[test]
fn rom_a_zoomed_sniper_draws_no_first_person_weapon() {
    use dc_client::arms::{Aim, ArmsConfig, Arsenal, WeaponCommands};
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
    let table = std::sync::Arc::new(WeaponTable::open(&rom).unwrap());
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut models = ModelScene::open(&rom).unwrap();
    let mut a = Arsenal::new(ArmsConfig {
        table: Some(table.clone()),
        rom: Some(rom.clone()),
        ..ArmsConfig::default()
    });
    a.set_normal_fov_x(90f32.to_radians());
    a.on_life(0, 1);
    a.on_grant(&dc_botclient::weapon::WeaponGrant {
        weapon: 27,
        ammo_kind: 27,
        ammo: 20.0,
        clip: Some(5),
        class_pos: 0,
        slot: 0,
        equip: true,
        parts: [27, 33, 53, -1],
    });
    let aim = Aim {
        origin: [0.0, 33.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        control: 0x200,
    };
    let zoom = |z: bool| WeaponCommands {
        zoom: z,
        ..WeaponCommands::default()
    };
    let mut t = 0;
    while t < 1500 {
        a.step(t, &zoom(false), &aim, &[], None);
        t += 16;
    }
    let row = table.get(27).expect("weapon row").clone();
    let (pv_model, clips) = row
        .pv_model
        .iter()
        .chain(row.pv_models.iter())
        .find_map(|p| WeaponClips::load(&vfs, p).ok().map(|c| (p.clone(), c)))
        .expect("clips");
    let vw = ViewWeapon {
        inspect: None,
        weapon: 27,
        model: Some(ModelRef {
            model: pv_model,
            textures: row.pv_skins.clone(),
        }),
        clip: clips.select.as_ref().map(|c| (c.index, 0.0)),
        pos: row.pv_pos.unwrap_or([0.0; 3]),
    };
    let list = a.view_attachments();
    assert!(!list.is_empty(), "the L96 carries mod models");
    let frame = |a: &Arsenal, models: &mut ModelScene| {
        for _ in 0..300 {
            models.place_view_weapon(&ctx, &renderer, Some(&vw), Vec3::ZERO, None);
            models.place_view_attachments(&ctx, &renderer, &list);
            models.set_view_hidden(a.scope().weapons_hidden());
            if models.stats.view_weapon
                && (a.scope().weapons_hidden() || models.view_instances().len() > list.len())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        models.view_instances().len()
    };
    assert_eq!(
        frame(&a, &mut models),
        1 + list.len(),
        "unzoomed: weapon + mods"
    );
    a.step(t, &zoom(true), &aim, &[], None);
    t += 16;
    assert!(
        a.scope().weapons_hidden(),
        "hidden from the start of the zoom-in"
    );
    while t < 3000 {
        a.step(t, &zoom(false), &aim, &[], None);
        t += 16;
    }
    assert_eq!(a.scope().level(), 1);
    assert_eq!(frame(&a, &mut models), 0, "zoomed: no first-person weapon");
    assert!(
        models.view_placed().is_some(),
        "still placed for the muzzle socket"
    );
    assert!(models.view_attachments_drawn().is_empty());
    a.step(t, &zoom(true), &aim, &[], None);
    t += 16;
    assert!(
        a.scope().weapons_hidden(),
        "the zoom-out runs: still hidden"
    );
    while t < 5000 {
        a.step(t, &zoom(false), &aim, &[], None);
        t += 16;
    }
    assert_eq!(a.scope().level(), 0);
    assert_eq!(
        frame(&a, &mut models),
        1 + list.len(),
        "zoomed out: drawn again"
    );
}

/// Playtest report (CLI-651): "the suppressor is in the right place on my own gun but wrong on the other players". A
/// remote holding the L96 (27, third-person model `HH_L96A1`) with the silencer 33 draws `hh_sr_silencer` at the
/// weapon model's `HHModelSocket` "Flash" moved by the weapon row's `MuzzleHHPosOff` (0, 0, -50.5) in the socket's
/// frame and turned by `MuzzleHHRotOff`, not at the bare socket.
#[test]
fn rom_the_remote_l96_silencer_sits_at_its_socket_plus_the_hh_offset() {
    use dc_client::{RemoteBody, RemoteLook};
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
    let mut models = ModelScene::open(&rom).unwrap();
    let weapon = 27u16;
    let row = table.get(weapon).expect("L96 row").clone();
    let hh = row.hh_model.clone().expect("the L96's third-person model");
    let list = table.attach.hh_attachments(weapon, [27, 33, 53]);
    assert_eq!(
        list.len(),
        1,
        "only the silencer has a third-person model: {list:?}"
    );
    let sil = &list[0];
    assert_eq!(sil.socket.to_ascii_lowercase(), "flash");
    assert_eq!(sil.pos_off, [0.0, 0.0, -50.5], "the row's MuzzleHHPosOff");
    assert_eq!(sil.rot_off, [0.0, 0.0, 1.0], "the row's MuzzleHHRotOff");
    let remote = RemoteBody {
        obj: 7,
        pid: 1,
        name: "Remote".into(),
        team: 1,
        pos: [0.0, 0.0, 0.0],
        half: [16.0, 50.0, 16.0],
        yaw: Some(0.7),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: Vec::new(),
            weapon: Some((
                ModelRef {
                    model: hh,
                    textures: row.hh_skins.clone(),
                },
                2,
            )),
            trackers: Vec::new(),
            visible: true,
            aim: None,
        }),
    };
    let lists = std::collections::BTreeMap::from([(7u16, list.clone())]);
    let mut placed = false;
    for _ in 0..300 {
        models.update_remotes(&ctx, &renderer, std::slice::from_ref(&remote), None);
        models.place_remote_attachments(&ctx, &renderer, &lists);
        if models
            .remote_weapon_parts(7)
            .is_some_and(|(_, _, p)| p.len() == 1)
        {
            placed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(placed, "the remote's weapon and silencer are drawn");
    let (gun, files, parts) = models.remote_weapon_parts(7).unwrap();
    let (_, child) = parts[0];
    let i = files
        .body
        .sockets
        .iter()
        .position(|s| s.name.as_bytes().eq_ignore_ascii_case(b"flash"))
        .expect("the HH model's Flash socket");
    let socket = gun.socket_world(&files.body, i).expect("socket transform");
    let bare = socket.transform_point3(Vec3::ZERO);
    let want = (socket * glam::Mat4::from_translation(Vec3::new(0.0, 0.0, -50.5)))
        .transform_point3(Vec3::ZERO);
    println!(
        "socket {bare:?}, silencer at {:?}, weapon bounds {:?}, silencer bounds {:?}",
        child.position,
        gun.bounds_in_world(),
        child.bounds_in_world()
    );
    assert!(
        child.position.distance(want) < 1e-3,
        "silencer at {:?}, socket + HH offset {want:?}",
        child.position
    );
    assert!(
        child.position.distance(bare) > 40.0,
        "moved off the bare socket"
    );
    // the silencer mesh lies about 50 units ahead of its origin: the offset brings it back onto the muzzle
    let (lo, hi) = child.bounds_in_world().expect("silencer bounds");
    assert!(
        ((lo + hi) * 0.5).distance(bare) < 15.0,
        "the silencer mesh sits on the muzzle socket: {lo:?}..{hi:?} vs {bare:?}"
    );
    let (_, socket_rot, _) = socket.to_scale_rotation_translation();
    let turned = dc_client::attach::turned(socket_rot.normalize(), [0.0, 0.0, 1.0]);
    assert!(
        child.rotation.angle_between(turned) < 1e-3,
        "turned by the HH angle offset"
    );
}

/// ROM-gated, no GPU: the user's "the M417 with the combat scope shows two scopes; seen on other guns too such as
/// the TAR-21". Those rows name the ITECH dot sight as `DefaultScopeAttach` with `DefaultScopeVisible = 0` (their
/// models carry the sight), so the granted default sight draws no second sight model in first or third person; a
/// different sight is still drawn. Every row with the flag behaves the same way.
// DISASM: cshell+0x232f30 (see `dc_client::attach::AttachTable::sight_built_in`)
#[test]
fn rom_a_built_in_default_sight_draws_no_second_scope() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let table = WeaponTable::open(&rom).unwrap();
    let t = &table.attach;
    let mod_id = |name: &str| {
        t.mods
            .iter()
            .find(|(_, m)| m.name.eq_ignore_ascii_case(name))
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("mod row {name}"))
    };
    let doc = dc_bute::ButeDoc::try_from_vfs(
        &dc_rez::RezVfs::new(&rom),
        dc_client::arms::ATTRIBUTES_REZ,
        dc_client::arms::WEAPONS_ENTRY,
    )
    .unwrap()
    .expect("WEAPONS.TXT");
    let weapon_id = |name: &str| {
        doc.iter()
            .find(|(sec, rows)| {
                sec.starts_with("Weapon")
                    && rows
                        .get("Name")
                        .is_some_and(|n| n.trim_matches('"').eq_ignore_ascii_case(name))
            })
            .and_then(|(sec, _)| sec["Weapon".len()..].parse::<u16>().ok())
            .unwrap_or_else(|| panic!("weapon row {name}"))
    };
    let itech = mod_id("DotSight_ITECH_AR");
    let hds = mod_id("DotSight_HDS-4_AR");
    for name in ["M417", "M417_Combat", "TAR21"] {
        let w = weapon_id(name);
        assert!(t.sight_built_in(w, itech), "{name}: the ITECH sight is part of the model");
        assert!(
            t.view_attachments(w, [None, Some(itech), None]).is_empty(),
            "{name}: no second first-person scope"
        );
        assert!(
            t.hh_attachments(w, [0xff, itech as u8, 0xff]).is_empty(),
            "{name}: no second third-person scope"
        );
        let other = t.view_attachments(w, [None, Some(hds), None]);
        println!("{name} ({w}): HDS-4 draws {:?}", other.iter().map(|a| &a.model.model).collect::<Vec<_>>());
        assert!(!t.sight_built_in(w, hds), "{name}: another sight is not built in");
    }
    let flagged = t
        .weapons
        .values()
        .filter(|w| !w.default_scope_visible && !w.default_scope.is_empty())
        .count();
    println!("{flagged} weapon rows carry their default sight in the model");
    assert!(flagged >= 3);
}

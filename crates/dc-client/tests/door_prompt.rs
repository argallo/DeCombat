//! doors-gear-r13 item 1 (playtest report: "some doors have a press e to open tooltip thats missing"): the door use
//! prompt drawn from the user's ROM at 1024 x 768 and 2560 x 1440 while the own player looks at an enabled door.
//!
//! ROM-gated (skipped without `CA_ROM`). PNGs (ROM-derived, never committed) go to
//! `doors-gear-r13/prompt/` when that folder's parent exists, else to the render out dir.
// DISASM: cshell+0x82860 (the image while the target text is the door's open / close text), cshell+0x2b3176 (the
// text id = activate row field [state])

use dc_client::hud::prompt::UseEntry;
use dc_client::hud::state::HudEvent;
use dc_client::hud::{Hud, HudState};
use dc_client::overlay::text::RgbaImage;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let o = dc_render::default_out_dir().join("doors-gear-r13-prompt");
    let _ = std::fs::create_dir_all(&o);
    o
}

fn save(img: &RgbaImage, name: &str) {
    let mut rgb = Vec::with_capacity((img.width * img.height * 3) as usize);
    for px in img.rgba.as_chunks::<4>().0 {
        let a = px[3] as u32;
        for c in &px[..3] {
            rgb.push(((*c as u32 * a + 48 * (255 - a)) / 255) as u8);
        }
    }
    if let Some(b) = image::RgbImage::from_raw(img.width, img.height, rgb) {
        let _ = b.save(out_dir().join(format!("{name}.png")));
    }
}

#[test]
fn rom_door_prompt_frames() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
        let p = hud.layout.top.prompt.clone();
        let img = p.image.clone().expect("prompt image in the layout");
        let t = hud.assets.texture(&img.tex).map(|t| (t.width, t.height));
        println!("{w}: pos {:?} size {:?} tex {:?}", img.pos, img.size, t);
        let mut s = HudState::default();
        s.apply(&HudEvent::Life(1), 1000, 0.0);
        let images = |hud: &mut Hud, s: &HudState| {
            hud.items(s)
                .into_iter()
                .filter(|it| matches!(it, dc_client::hud::DrawItem::Image { tex, .. } if *tex == img.tex))
                .count()
        };
        assert_eq!(images(&mut hud, &s), 0, "no door under the cross");
        // the closed door (state off): "open"; the open door (state on): "close"; both the one image
        for state in [1u8, 0] {
            s.apply(
                &HudEvent::UseTarget(Some(UseEntry {
                    row: 0,
                    disabled: false,
                    state,
                })),
                1000,
                0.0,
            );
            assert_eq!(images(&mut hud, &s), 1, "state {state}");
            let frame = hud.render(&s);
            save(&frame, &format!("door_prompt_state{state}_{w}x{h}"));
        }
        s.apply(
            &HudEvent::UseTarget(Some(UseEntry {
                row: 0,
                disabled: true,
                state: 1,
            })),
            1000,
            0.0,
        );
        assert_eq!(images(&mut hud, &s), 0, "disabled door");
    }
}

/// match-r15 (playtest report: "there should be text when you can press E to pick up a gun"): the item image of the
/// same element while a dropped weapon lies in the use key's reach (CLI-1352).
// DISASM: cshell+0x2b31a0 (type 0x17 target -> text 0x1c6), cshell+0x82a5e (text 0x1c6 -> the item image)
#[test]
fn rom_item_prompt_frames() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
        let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
        let p = hud.layout.top.prompt.clone();
        let img = p.item.clone().expect("item image in the layout");
        let door = p.image.clone().expect("door image in the layout");
        assert_ne!(img.tex, door.tex);
        let t = hud.assets.texture(&img.tex).map(|t| (t.width, t.height));
        println!("{w}: pos {:?} size {:?} tex {:?}", img.pos, img.size, t);
        assert!(t.is_some(), "the item texture loads");
        let mut s = HudState::default();
        s.apply(&HudEvent::Life(1), 1000, 0.0);
        let images = |hud: &mut Hud, s: &HudState, tex: &str| {
            hud.items(s)
                .into_iter()
                .filter(|it| matches!(it, dc_client::hud::DrawItem::Image { tex: t, .. } if t == tex))
                .count()
        };
        assert_eq!(images(&mut hud, &s, &img.tex), 0, "no drop in reach");
        s.apply(&HudEvent::ItemTarget(true), 1000, 0.0);
        assert_eq!(images(&mut hud, &s, &img.tex), 1);
        let frame = hud.render(&s);
        save(&frame, &format!("item_prompt_{w}x{h}"));
        // a door under the cross at the same time: the item image only
        s.apply(
            &HudEvent::UseTarget(Some(UseEntry {
                row: 0,
                disabled: false,
                state: 1,
            })),
            1000,
            0.0,
        );
        assert_eq!(images(&mut hud, &s, &img.tex), 1);
        assert_eq!(images(&mut hud, &s, &door.tex), 0);
        s.apply(&HudEvent::ItemTarget(false), 1000, 0.0);
        assert_eq!(images(&mut hud, &s, &door.tex), 1, "the door again");
        // dead: nothing
        s.apply(&HudEvent::ItemTarget(true), 1000, 0.0);
        s.apply(&HudEvent::Life(2), 1000, 0.0);
        assert_eq!(images(&mut hud, &s, &img.tex), 0);
    }
}

/// CLI-1795 (live report 2026-11-02 "the garage door on Rattlesnake is supposed to give the press E popup when you're
/// able to flip the switch"): with the ROM Rattlesnake world, each garage lever (a rotating door players use, its
/// entry enabled) is the use target of a body standing 48 units in front of it, whatever the view: the prompt image
/// is drawn for it; a body near a shutter (a door only the levers move, its entry disabled) but out of the levers'
/// reach has no target; the locked lever (entry disabled after a use) has none either. Counts only.
#[test]
fn rom_rattlesnake_levers_give_the_use_prompt_in_reach() {
    use dc_botclient::objects::{kind, ClientObject, ObjectTable};
    use dc_client::hud::prompt::{UseEntries, USE_REACH};
    use dc_dserver::doors::{door_blob_of, door_infos, turn_infos, OpenBy, TurnKind};
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let entries = vfs.list_entries("WORLDS.rez").expect("world archive");
    let Some((name, _, _)) = entries.iter().find(|e| {
        let u = e.0.to_ascii_uppercase();
        u.ends_with(".DAT") && u.contains("RATTLESNAKE")
    }) else {
        println!("skipped: no Rattlesnake world");
        return;
    };
    let data = vfs.entry("WORLDS.rez", name).ok().flatten().expect("entry");
    let w = dc_world::World::parse(&data).expect("world");
    let models =
        dc_client::objects::WorldModels::of(&w, &dc_sim::from_world::WorldFilter::default(), None);
    let levers: Vec<_> = turn_infos(&w)
        .into_iter()
        .filter(|t| t.kind == TurnKind::Door && t.open_by == OpenBy::Use)
        .collect();
    let shutters: Vec<_> = door_infos(&w)
        .into_iter()
        .filter(|d| d.open_by == OpenBy::Message)
        .collect();
    assert_eq!((levers.len(), shutters.len()), (2, 2));
    // the server's creates: the levers enabled and closed (DSV-340), the shutters disabled
    let mut t = ObjectTable::default();
    let mut add = |id: u16, n: &[u8], disabled: bool| {
        let mut o = ClientObject::default();
        o.kind = kind::BRUSH;
        o.name = Some(n.to_vec());
        o.blob_type = Some(0x39);
        o.blob = Some(door_blob_of(0, disabled, 1));
        t.objects.insert(id, o);
    };
    for (k, l) in levers.iter().enumerate() {
        add(100 + k as u16, &l.name, false);
    }
    for (k, s) in shutters.iter().enumerate() {
        add(200 + k as u16, &s.name, true);
    }
    let mut e = UseEntries::default();
    e.sync(&t);
    let boxes = |n: &[u8]| models.file_box(n);
    let mid = |lo: [f32; 3], hi: [f32; 3]| [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let s_mid = mid(shutters[0].lo, shutters[0].hi);
    let ext = |k: usize| shutters[0].hi[k] - shutters[0].lo[k];
    let thin = if ext(0) < ext(2) { 0 } else { 2 };
    let mut hud = Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD");
    let img = hud.layout.top.prompt.image.clone().expect("prompt image");
    let mut s = HudState::default();
    s.apply(&HudEvent::Life(1), 1000, 0.0);
    let images = |hud: &mut Hud, s: &HudState| {
        hud.items(s)
            .into_iter()
            .filter(
                |it| matches!(it, dc_client::hud::DrawItem::Image { tex, .. } if *tex == img.tex),
            )
            .count()
    };
    for l in &levers {
        assert_eq!(
            models.file_box(&l.name),
            Some((l.lo, l.hi)),
            "the server's box"
        );
        let l_mid = mid(l.lo, l.hi);
        let mut body = l_mid;
        body[thin] += 48.0 * (l_mid[thin] - s_mid[thin]).signum();
        body[1] = l_mid[1] - 30.0;
        let target = e.reach_target(&t, body, boxes);
        assert_eq!(
            target,
            Some(UseEntry {
                row: 0,
                disabled: false,
                state: 1
            }),
            "a lever in reach"
        );
        s.apply(&HudEvent::UseTarget(target), 1000, 0.0);
        assert_eq!(images(&mut hud, &s), 1, "the prompt");
        // 200 units further out: nothing in reach
        body[thin] += 200.0 * (l_mid[thin] - s_mid[thin]).signum();
        assert_eq!(e.reach_target(&t, body, boxes), None, "out of reach");
    }
    // at the middle of a shutter out of both levers' reach: the shutter's own entry is disabled
    let mut checked = 0;
    for sh in &shutters {
        let mut body = mid(sh.lo, sh.hi);
        body[thin] += 48.0;
        let far = levers.iter().all(|l| {
            (0..3)
                .map(|k| (l.lo[k] - body[k]).max(body[k] - l.hi[k]).max(0.0).powi(2))
                .sum::<f32>()
                .sqrt()
                > USE_REACH
        });
        if far {
            assert_eq!(
                e.reach_target(&t, body, boxes),
                None,
                "a shutter has no prompt"
            );
            checked += 1;
        }
    }
    assert!(checked > 0);
    // a use locks both levers (entry disabled): no prompt for the wait
    for id in [100u16, 101] {
        e.on_note(id, dc_client::hud::prompt::NOTE_DISABLED, 1);
    }
    let l_mid = mid(levers[0].lo, levers[0].hi);
    let mut body = l_mid;
    body[thin] += 48.0 * (l_mid[thin] - s_mid[thin]).signum();
    assert_eq!(e.reach_target(&t, body, boxes), None, "locked");
    s.apply(&HudEvent::UseTarget(None), 1000, 0.0);
    assert_eq!(images(&mut hud, &s), 0);
}

/// Live report 2026-11-02 ("Overdose light switches never usable"): the server now creates the two player-activated
/// light switches with an enabled entry of the door row (DSV-561), so the use prompt shows while the own body is in
/// the use key's reach of a switch, and goes when the switch is out of reach.
#[test]
fn rom_overdose_light_switches_give_the_use_prompt_in_reach() {
    use dc_botclient::objects::{kind, ClientObject, ObjectTable};
    use dc_client::hud::prompt::UseEntries;
    use dc_dserver::doors::{door_blob_of, turn_infos, TurnDoor, TurnKind, DOOR_ACTIVATE_ROW};
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let entries = vfs.list_entries("WORLDS.rez").expect("world archive");
    let Some((name, _, _)) = entries.iter().find(|e| {
        let u = e.0.to_ascii_uppercase();
        u.rsplit('\\').next() == Some("OVERDOSE.DAT")
    }) else {
        println!("skipped: no Overdose world");
        return;
    };
    let data = vfs.entry("WORLDS.rez", name).ok().flatten().expect("entry");
    let w = dc_world::World::parse(&data).expect("world");
    let models =
        dc_client::objects::WorldModels::of(&w, &dc_sim::from_world::WorldFilter::default(), None);
    let switches: Vec<_> = turn_infos(&w)
        .into_iter()
        .filter(|t| t.kind == TurnKind::Switch)
        .collect();
    assert_eq!(switches.len(), 2);
    // the server's creates: each switch's entry as the server has it at the start
    let mut t = ObjectTable::default();
    for (k, sw) in switches.iter().enumerate() {
        let (disabled, state) = TurnDoor::new(100 + k as u16, sw.clone())
            .activate()
            .expect("a use switch has an entry");
        let mut o = ClientObject::default();
        o.kind = kind::BRUSH;
        o.name = Some(sw.name.clone());
        o.blob_type = Some(0x39);
        o.blob = Some(door_blob_of(DOOR_ACTIVATE_ROW, disabled, state));
        t.objects.insert(100 + k as u16, o);
    }
    let mut e = UseEntries::default();
    e.sync(&t);
    let boxes = |n: &[u8]| models.file_box(n);
    let mid = |lo: [f32; 3], hi: [f32; 3]| [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let mut hud = Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD");
    let img = hud.layout.top.prompt.image.clone().expect("prompt image");
    let mut s = HudState::default();
    s.apply(&HudEvent::Life(1), 1000, 0.0);
    let images = |hud: &mut Hud, s: &HudState| {
        hud.items(s)
            .into_iter()
            .filter(
                |it| matches!(it, dc_client::hud::DrawItem::Image { tex, .. } if *tex == img.tex),
            )
            .count()
    };
    for (k, sw) in switches.iter().enumerate() {
        assert_eq!(models.file_box(&sw.name), Some((sw.lo, sw.hi)), "the server's box");
        let mut body = mid(sw.lo, sw.hi);
        body[1] -= 60.0;
        let target = e.reach_target(&t, body, boxes);
        assert_eq!(
            target,
            Some(UseEntry {
                row: 0,
                disabled: false,
                state: 1
            }),
            "a switch in reach, off"
        );
        s.apply(&HudEvent::UseTarget(target), 1000, 0.0);
        assert_eq!(images(&mut hud, &s), 1, "the prompt");
        if k == 0 {
            save(&hud.render(&s), "overdose_switch_prompt_1024x768");
        }
        body[1] -= 200.0;
        assert_eq!(e.reach_target(&t, body, boxes), None, "out of reach");
    }
}

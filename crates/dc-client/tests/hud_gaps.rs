//! client-hudgaps: the HUD parts the first arena run found missing in dc-client, drawn from the user's ROM.
//!
//! - Low health: the health block red at 30 health or less, pulsing between the layout's two reds, a heartbeat cue.
//! - Kill feed: the head-shot icon of a zone-1 kill (also for a team kill: there is no team-kill mark), and the other
//!   marks of the line (payback, groin, flag, bounty, freeze; the self-kill, barrel, fire and toxic forms).
//!
//! ROM-gated (skipped without `CA_ROM`); nothing of the ROM is asserted by value: regions come from the loaded
//! layout, assertions are pixel counts and colour relations. PNGs (ROM-derived, never committed) go to
//! `reports/client-hudgaps/` when that folder's parent exists, else to the render out dir.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (0x6d), §4.3 (0x66); FMT_CRES_UI.md §8
// EVIDENCE: cshell+0x95f90 / cshell+0x96210 (low health), cshell+0x9d4a0 (feed line)

use dc_botclient::shell::{KillRecord, RosterRecord};
use dc_client::hud::state::HudEvent;
use dc_client::hud::{Hud, HudDriver, HudState};
use dc_client::overlay::text::RgbaImage;
use dc_net_engine::socket_reader::SocketReader;
use std::path::PathBuf;

const W: u32 = 1024;
const H: u32 = 768;

pub fn out_dir() -> PathBuf {
    let o = dc_render::default_out_dir().join("client-hudgaps");
    let _ = std::fs::create_dir_all(&o);
    o
}

fn rom() -> Option<PathBuf> {
    let r = dc_rom::test_rom();
    if r.is_none() {
        println!("skipped: CA_ROM unset");
    }
    r
}

/// Saves the HUD layer over a dark grey backdrop (straight alpha).
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

fn rec(pid: u8, name: &str, team: u8) -> RosterRecord {
    RosterRecord {
        name: name.into(),
        pid,
        team,
        complete: true,
        ..RosterRecord::default()
    }
}

/// Own player 0 on team 1, alive, with `hp` health and 84 armour; a mate (1) and two opponents (2, 3).
fn alive_state(hp: f32) -> HudState {
    let mut s = HudState::default();
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(1),
        },
        HudEvent::Roster(rec(0, "arena", 1)),
        HudEvent::Roster(rec(1, "bot1", 1)),
        HudEvent::Roster(rec(2, "bot2", 2)),
        HudEvent::Roster(rec(3, "bot3", 2)),
        HudEvent::Life(1),
        HudEvent::Status { kind: 2, value: hp },
        HudEvent::Status {
            kind: 3,
            value: 84.0,
        },
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.tick(1000, 60_000);
    s
}

/// Opaque pixels inside the virtual rectangle `r` whose colour passes `f`.
fn count_in(img: &RgbaImage, r: [i32; 4], f: impl Fn([u8; 4]) -> bool) -> usize {
    let (sx, sy) = (img.width as f32 / 1024.0, img.height as f32 / 768.0);
    let x0 = (r[0] as f32 * sx).floor().max(0.0) as u32;
    let y0 = (r[1] as f32 * sy).floor().max(0.0) as u32;
    let x1 = (((r[0] + r[2]) as f32) * sx).ceil().min(img.width as f32) as u32;
    let y1 = (((r[1] + r[3]) as f32) * sy).ceil().min(img.height as f32) as u32;
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = img.pixel(x, y);
            p[3] > 100 && f(p)
        })
        .count()
}

fn reddish(p: [u8; 4]) -> bool {
    p[0] > 120 && (p[1] as u32) < p[0] as u32 / 3 && (p[2] as u32) < p[0] as u32 / 3
}

fn whitish(p: [u8; 4]) -> bool {
    p[0] > 150 && p[1] > 150 && p[2] > 150
}

/// The health digits' rectangle of the layout (virtual pixels).
fn health_digits_rect(hud: &Hud) -> [i32; 4] {
    let d = hud
        .layout
        .health
        .health_digits
        .as_ref()
        .expect("health digits");
    let xs: Vec<i32> = d.pos.iter().map(|p| p[0]).collect();
    let ys: Vec<i32> = d.pos.iter().map(|p| p[1]).collect();
    let x0 = *xs.iter().min().unwrap();
    let y0 = *ys.iter().min().unwrap();
    [
        x0,
        y0,
        xs.iter().max().unwrap() + d.size[0] - x0,
        ys.iter().max().unwrap() + d.size[1] - y0,
    ]
}

#[test]
fn rom_low_health_turns_the_block_red_and_pulses() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let l = hud.layout.low_health.clone();
    println!(
        "low health: pulse {} steps/s, step {} red units every {} ms, beat every {} s, beat file {}",
        l.rate,
        l.step(),
        l.step_ms(),
        l.beat_secs,
        if l.beat_file.is_some() {
            "resolved"
        } else {
            "missing"
        }
    );
    assert!(l.pulses(), "the ROM layout gives a pulse rate and sweep");
    assert!(l.bright > l.dark, "two reds, bright above dark");
    let r = health_digits_rect(&hud);
    // 31: white digits
    let mut s = alive_state(31.0);
    dc_client::hud::vitals::tick(&mut s, &l, 1000);
    let white = hud.render(&s);
    save(&white, "low_health_31_white");
    let (w_red, w_white) = (count_in(&white, r, reddish), count_in(&white, r, whitish));
    // 19 (the arena frame's value): red at the first pulse step, brighter after the sweep
    let mut s = alive_state(19.0);
    dc_client::hud::vitals::tick(&mut s, &l, 1000);
    let first = hud.render(&s);
    save(&first, "low_health_19_first_step");
    s.now_ms = 1000 + l.step_ms() * 15;
    let peak = hud.render(&s);
    save(&peak, "low_health_19_bright");
    let (f_red, f_white) = (count_in(&first, r, reddish), count_in(&first, r, whitish));
    let p_red = count_in(&peak, r, reddish);
    let mean_red = |img: &RgbaImage| {
        let mut sum = 0u64;
        let mut n = 0u64;
        let (x0, y0) = (r[0] as u32, r[1] as u32);
        for y in y0..y0 + r[3] as u32 {
            for x in x0..x0 + r[2] as u32 {
                let p = img.pixel(x, y);
                if p[3] > 200 && reddish(p) {
                    sum += p[0] as u64;
                    n += 1;
                }
            }
        }
        sum / n.max(1)
    };
    println!(
        "health digits {r:?}: 31 -> red {w_red} white {w_white}; 19 first step -> red {f_red} white {f_white} \
         (mean red {}); 19 after the sweep -> red {p_red} (mean red {})",
        mean_red(&first),
        mean_red(&peak)
    );
    assert!(w_white > 50 && w_red == 0, "31 health is white");
    assert!(f_red > 50 && f_white == 0, "19 health is red");
    assert!(
        mean_red(&peak) > mean_red(&first) + 20,
        "the pulse brightens"
    );
    // the health arc beside the aim mark and the rest of the HUD keep their colours: only the block changes
    let mut driver = HudDriver::new(hud);
    driver.state = alive_state(19.0);
    let fs = dc_client::world::FrameState::default();
    let mut beats = 0;
    for t in (1000..4600).step_by(50) {
        driver.update(&fs, t, 0.0, false, false);
        beats += driver
            .take_sound_cues()
            .iter()
            .filter(|c| {
                matches!(c, dc_client::sound::SoundCue::Interface { what, .. } if *what == "heartbeat")
            })
            .count();
    }
    println!("heartbeats in 3.6 s of low health: {beats}");
    assert_eq!(beats, 3, "one beat each second after the first low second");
}

fn kill(victim: u8, killer: u8, zone: u8) -> KillRecord {
    KillRecord {
        victim,
        killer,
        kill_kind: 1,
        weapon: 145,
        hit_zone: zone,
        complete: true,
        ..KillRecord::default()
    }
}

#[test]
fn rom_feed_shows_the_head_shot_icon_and_the_marks() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let f = hud.layout.feed.clone();
    let m = hud.layout.feed_marks.clone();
    let lines: Vec<(&str, KillRecord)> = vec![
        ("body kill", kill(2, 0, 3)),
        ("head shot", kill(2, 0, 1)),
        (
            "team kill, head shot (the arena's mate-kill frame)",
            kill(1, 0, 1),
        ),
        ("groin", kill(3, 0, 5)),
        (
            "payback",
            KillRecord {
                revenge_done: true,
                ..kill(2, 0, 3)
            },
        ),
        (
            "flag, victim on the own team",
            KillRecord {
                relation: true,
                ..kill(1, 2, 3)
            },
        ),
        (
            "bounty",
            KillRecord {
                feed6: true,
                ..kill(2, 0, 3)
            },
        ),
        (
            "freeze",
            KillRecord {
                feed2: true,
                ..kill(2, 0, 3)
            },
        ),
        (
            "self kill",
            KillRecord {
                feed_value: 5,
                ..kill(0, 0, 3)
            },
        ),
        (
            "barrel",
            KillRecord {
                feed_value: 7,
                ..kill(2, 0xff, 3)
            },
        ),
    ];
    // at most six lines stay in the feed: three pictures, each starting with a plain body kill as the reference
    let band = |i: usize| {
        [
            f.pos[0],
            f.pos[1] + f.line_step * i as i32 - 9,
            // the line's names and icons (the top team block starts further right)
            320,
            f.line_step.max(20),
        ]
    };
    let mut counts = vec![0usize; lines.len()];
    for (part, range) in [(0, 1..5), (1, 5..9), (2, 9..lines.len())] {
        let mut s = alive_state(100.0);
        let picked: Vec<usize> = std::iter::once(0).chain(range).collect();
        for &i in &picked {
            s.apply(&HudEvent::Kill(lines[i].1), 2000, 0.0);
        }
        s.tick(2000, 60_000);
        let img = hud.render(&s);
        save(&img, &format!("feed_marks_{part}"));
        let base = count_in(&img, band(0), |_| true);
        for (row, &i) in picked.iter().enumerate().skip(1) {
            let c = count_in(&img, band(row), |_| true);
            println!(
                "feed picture {part} line {row} ({}): {c} drawn pixels (reference body kill {base})",
                lines[i].0
            );
            counts[i] = c;
        }
        counts[0] = base;
    }
    let body = counts[0];
    let head = counts[1];
    let team_head = counts[2];
    println!(
        "head icon {}, groin {}, payback {}, flag {}/{}, bounty {}, freeze {}, self {}, barrel {}",
        f.head_icon.is_some(),
        m.groin.tex.is_some(),
        m.payback.tex.is_some(),
        m.flag_victim_mate.is_some(),
        m.flag_victim_other.is_some(),
        m.bounty.tex.is_some(),
        m.freeze.tex.is_some(),
        m.self_kill.tex.is_some(),
        m.barrel.tex.is_some()
    );
    assert!(
        head > body + 40,
        "the head-shot line draws the icon ({head} vs {body})"
    );
    assert!(
        team_head > body + 40,
        "a head-shot team kill shows the same icon"
    );
    for (k, what) in [
        (3, "groin"),
        (4, "payback"),
        (5, "flag"),
        (6, "bounty"),
        (7, "freeze"),
    ] {
        assert!(
            counts[k] > body + 40,
            "{what} mark drawn ({} vs {body})",
            counts[k]
        );
    }
    // the special forms draw their image instead of the killer name and weapon icon
    assert!(counts[8] > 100 && counts[9] > 100);
}

// ---- the sky: the server's sky table places the sky eye in its view box and picks the sky objects drawn

/// The sky table our server sends for `map` and the client object table it creates for those objects.
fn server_sky(
    rom: &std::path::Path,
    map: &str,
) -> Option<([u8; 48], Vec<u16>, dc_botclient::objects::ObjectTable)> {
    use dc_dserver::worldobj::{
        brush_flags, plan_scene, sky_table, WorldObject, WorldObjects, ZoneClass,
    };
    let info = dc_dserver::world::load_from_rom(
        rom,
        &format!("Worlds\\{map}"),
        dc_dserver::world::NameForm::Listed,
        None,
    )
    .ok()?;
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(rom);
    let mut next = 1000u16..;
    let objs = plan_scene(
        &info,
        &tables,
        WorldObjects::World,
        ZoneClass::Room,
        12,
        None,
        || next.next().unwrap(),
    );
    let (block, ids) = sky_table(&objs)?;
    let mut table = dc_botclient::objects::ObjectTable::default();
    for o in &objs {
        if let WorldObject::Brush { obj, brush } = o {
            let mut o = dc_botclient::objects::ClientObject::default();
            o.kind = dc_botclient::objects::kind::BRUSH;
            o.name = Some(brush.name.clone());
            o.flags = brush_flags(brush).0;
            table.objects.insert(*obj, o);
        }
    }
    Some((block, ids, table))
}

#[test]
fn rom_sky_follows_the_server_sky_table() {
    use dc_client::offscreen::Offscreen;
    use dc_client::world::GpuTarget;
    use dc_client::{RomWorld, ShootTest, ViewSettings, WorldSource};
    use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
    const MAP: &str = "JUNK_FLEA";
    let Some(rom) = rom() else {
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
    let mut src = RomWorld::new(&rom, MAP);
    let Ok(mut world) = src.load(Some(GpuTarget {
        ctx: &ctx,
        renderer: &renderer,
    })) else {
        println!("skipped: {MAP} not in this ROM");
        return;
    };
    let (block, ids, table) = server_sky(&rom, MAP).expect("the server's sky table");
    let sky = dc_client::objects::sky_table(Some(&block), &ids, &table).expect("table");
    let scene_skies = world.scene.as_ref().unwrap().sky_model_names().len();
    println!(
        "{MAP}: {} sky models in the scene, {} named by the table ({} visible)",
        scene_skies,
        sky.models.len(),
        sky.models.iter().filter(|m| m.1).count()
    );
    assert!(scene_skies >= 1 && !sky.models.is_empty());
    let (w, h) = (800, 600);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let eye = [300.0f32, 900.0, 0.0];
    let shoot = |world: &dc_client::world::LoadedWorld, yaw_deg: f32| {
        let a = yaw_deg.to_radians();
        let to = [
            eye[0] + 1000.0 * a.sin(),
            eye[1] - 60.0,
            eye[2] + 1000.0 * a.cos(),
        ];
        let view = ShootTest::look_at(eye, to);
        let camera =
            ViewSettings::default().camera(dc_sim::V3::new(eye[0], eye[1], eye[2]), &view, w, h);
        target
            .render(
                &ctx,
                &renderer,
                world.scene.as_ref().unwrap(),
                &camera,
                None,
            )
            .unwrap()
    };
    let yaws = [0.0f32, 90.0, 180.0, 270.0];
    let before: Vec<_> = yaws.iter().map(|y| shoot(&world, *y)).collect();
    world.apply_sky(&sky);
    let after: Vec<_> = yaws.iter().map(|y| shoot(&world, *y)).collect();
    // the upper third of each view: how much changed, and how much of it is warm (the sunset) after
    let mut changed = 0usize;
    let mut warm_before = 0usize;
    let mut warm_after = 0usize;
    for (i, (a, b)) in before.iter().zip(&after).enumerate() {
        a.save_png(&out_dir().join(format!("sky_{MAP}_{}_before.png", yaws[i] as i32)))
            .unwrap();
        b.save_png(&out_dir().join(format!("sky_{MAP}_{}_server_table.png", yaws[i] as i32)))
            .unwrap();
        for y in 0..h / 3 {
            for x in 0..w {
                let (p, q) = (a.pixel(x, y), b.pixel(x, y));
                if (0..3)
                    .map(|k| (p[k] as i32 - q[k] as i32).abs())
                    .sum::<i32>()
                    > 24
                {
                    changed += 1;
                }
                let warm = |c: [u8; 4]| c[0] as i32 > c[2] as i32 + 40 && c[0] > 150;
                warm_before += warm(p) as usize;
                warm_after += warm(q) as usize;
            }
        }
    }
    println!(
        "upper thirds of 4 views: {changed} pixels changed; warm (sunset) pixels {warm_before} -> {warm_after}; PNGs in {}",
        out_dir().display()
    );
    assert!(changed > 20_000, "the sky moves to the server's view box");
    assert!(warm_after > warm_before + 2_000, "the sunset sky box shows");
}

// ---- the weapon panel after a weapon change (the spawn's select)

#[test]
fn rom_weapon_panel_shows_after_a_weapon_change_for_four_seconds() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let Some(l) = hud.layout.weapon_panel.clone() else {
        panic!("the ROM layout has the weapon panel's place and size");
    };
    println!(
        "weapon panel: place {:?} size {:?} gap {} inactive gap {}; {} rows with an active image, {} weapons with a class",
        l.pos,
        l.size,
        l.gap,
        l.idle_gap,
        l.images.values().filter(|i| i.0.is_some()).count(),
        l.classes.len()
    );
    // the arena loadout: rifle, pistol, knife, grenade; the spawn selects the rifle
    let carried = vec![145u16, 164, 0, 7];
    let mut s = alive_state(100.0);
    s.apply(
        &HudEvent::WeaponPanel {
            carried: carried.clone(),
            selected: 145,
        },
        1000,
        0.0,
    );
    let place = [l.pos[0], l.pos[1] - l.gap - 1, l.size[0], l.size[1]];
    let mut shot = |t: u32, name: &str| {
        s.tick(t, 60_000);
        let img = hud.render(&s);
        save(&img, name);
        (count_in(&img, place, |_| true), img)
    };
    let (shown, img) = shot(1000, "weapon_panel_spawn");
    let (late, _) = shot(4999, "weapon_panel_3999ms");
    let (gone, _) = shot(5000, "weapon_panel_4000ms");
    println!("panel pixels at {place:?}: 0 ms {shown}, 3999 ms {late}, 4000 ms {gone}");
    assert!(shown > 1000 && late == shown && gone < shown / 10);
    // against the reference stack's spawn frame (1024 x 768, the same ROM texture): the white weapon picture lines up
    let shot_path = std::env::var_os("DC_ARENA_SPAWN_FRAME")
        .map(PathBuf::from)
        .filter(|p| p.is_file());
    let Some(shot_path) = shot_path else {
        println!("skipped: the arena's reference spawn frame is not here");
        return;
    };
    let real = image::open(&shot_path).expect("frame").to_rgb8();
    let lum = |x: i32, y: i32| {
        if x < 0 || y < 0 || x >= real.width() as i32 || y >= real.height() as i32 {
            return 0i64;
        }
        let p = real.get_pixel(x as u32, y as u32);
        (p[0] as i64 + p[1] as i64 + p[2] as i64) / 3
    };
    let ours: Vec<(i32, i32)> = (place[1].max(0)..(place[1] + place[3]).min(H as i32))
        .flat_map(|y| (place[0]..(place[0] + place[2]).min(W as i32)).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = img.pixel(x as u32, y as u32);
            p[3] > 230 && p[0] > 230 && p[1] > 230 && p[2] > 230
        })
        .collect();
    let mut best = (i64::MIN, 0, 0);
    for dy in -6..=6 {
        for dx in -6..=6 {
            let score: i64 = ours.iter().map(|&(x, y)| lum(x + dx, y + dy)).sum();
            if score > best.0 {
                best = (score, dx, dy);
            }
        }
    }
    let mean = best.0 / ours.len().max(1) as i64;
    println!(
        "{} white picture pixels; best offset against the reference stack frame ({}, {}), mean luminance there {mean}",
        ours.len(),
        best.1,
        best.2
    );
    assert!(ours.len() > 300);
    assert!(best.1.abs() <= 2 && best.2.abs() <= 2, "the panel lines up");
    assert!(mean > 200, "the reference stack frame has the white picture there");
}

// ---- the team mark on the character (the band on the upper arm)

/// Renders the default character of team 1 (no model block: the server's team-mark slot 0) at close range for a
/// local player of team `own_team` and counts the pixels of the band's blue. Returns (band pixels, model pixels).
fn band_pixels(tag: &str, yaw_turn: f32, own_team: u8) -> Option<(usize, usize)> {
    use dc_client::characters::ModelScene;
    use dc_client::offscreen::Offscreen;
    use dc_client::world::GpuTarget;
    use dc_client::{
        LocalPlayer, RemoteBody, RemoteLook, RemoteTracker, RomWorld, ShootTest, ViewAngles,
        ViewSettings, WorldSource,
    };
    use dc_render::{
        name_key, AdapterChoice, GpuContext, ModelFiles, RenderSettings, WorldRenderer,
        CLIP_LOWER_STAND, CLIP_UPPER_STAND, OFFSCREEN_FORMAT,
    };
    use dc_sim::{drop_to_floor, Cylinder, MovementParams, V3};
    let rom = rom()?;
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return None;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&rom, "JUNK_FLEA");
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .ok()?;
    let scene = world.scene.as_ref()?;
    let spawn = world.spawn(0).ok()?;
    let params = MovementParams::default();
    let me = LocalPlayer::spawn(params, &world.collision, &spawn, world.stand_dims, 60);
    let view0 = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    let eye = me.eye_target(&view0);
    let fwd = V3::new(spawn.yaw.sin(), 0.0, spawn.yaw.cos());
    let centre = drop_to_floor(
        &world.collision,
        &Cylinder::from_dims(world.stand_dims),
        me.state.pos + fwd * 70.0,
        params.step(),
        1000.0,
    );
    let look_at = [centre.x, centre.y + 30.0, centre.z];
    let view = ShootTest::look_at(eye.to_array(), look_at);
    let (w, h) = (640u32, 480u32);
    let camera = ViewSettings::default().camera(eye, &view, w, h);
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let mut models = ModelScene::open(&rom).ok()?;
    models.own_team = Some(own_team);
    let body = models.default_look();
    let vfs = dc_rez::RezVfs::new(&rom);
    let files = ModelFiles::load(&vfs, &body[0].model).ok()?;
    let anim = files.anim_set();
    let clip = |k| anim.find_clip(|n| name_key(n) == k);
    let trackers = vec![
        RemoteTracker {
            clip: 0,
            time_ms: 0.0,
            blend_set: None,
        },
        RemoteTracker {
            clip: clip(CLIP_UPPER_STAND)?,
            time_ms: 0.0,
            blend_set: Some(0),
        },
        RemoteTracker {
            clip: clip(CLIP_LOWER_STAND)?,
            time_ms: 0.0,
            blend_set: Some(1),
        },
    ];
    let remote = RemoteBody {
        obj: 7,
        pid: 1,
        name: "Mate".into(),
        team: 1,
        pos: centre.to_array(),
        half: world.stand_dims.to_array(),
        yaw: Some(spawn.yaw + yaw_turn),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: Vec::new(),
            weapon: None,
            trackers,
            visible: true,
            aim: None,
        }),
    };
    models.update_remotes(
        &ctx,
        &renderer,
        std::slice::from_ref(&remote),
        world.model_lights.as_ref(),
    );
    let inst = models.instances();
    let plain = target
        .render_with(&ctx, &renderer, scene, &camera, None, &[], None)
        .ok()?;
    let img = target
        .render_with(&ctx, &renderer, scene, &camera, None, &inst, None)
        .ok()?;
    img.save_png(&out_dir().join(format!("team_mark_{tag}.png")))
        .ok()?;
    let mut band = 0;
    let mut model = 0;
    for y in 0..h {
        for x in 0..w {
            let (p, q) = (plain.pixel(x, y), img.pixel(x, y));
            if (0..3)
                .map(|k| (p[k] as i32 - q[k] as i32).abs())
                .sum::<i32>()
                <= 24
            {
                continue;
            }
            model += 1;
            // the band's blue: blue clearly above red and green
            if q[2] as i32 > q[0] as i32 + 50 && q[2] as i32 > q[1] as i32 + 20 && q[2] > 120 {
                band += 1;
            }
        }
    }
    println!(
        "team mark {tag}: model stats {:?}; {model} model pixels, {band} band-blue pixels",
        models.stats
    );
    Some((band, model))
}

#[test]
fn rom_team_mark_band_shows_on_team_mates_only() {
    let Some((mate, model)) = band_pixels("mate_from_behind", 0.0, 1) else {
        return;
    };
    let (enemy, _) = band_pixels("enemy_from_behind", 0.0, 2).unwrap();
    let (mate_front, _) = band_pixels("mate_from_the_front", std::f32::consts::PI, 1).unwrap();
    println!("band pixels: mate from behind {mate}, enemy from behind {enemy}, mate from the front {mate_front}");
    assert!(model > 2000, "the character is on screen");
    assert!(mate > 30, "the team-mate band shows ({mate} blue pixels)");
    assert_eq!(enemy, 0, "no band on an enemy (the transparent mark)");
}

// ---- the rank icon beside the names over the players' heads

#[test]
fn rom_rank_icon_stands_left_of_the_name() {
    use dc_client::hud::marks::{MarkState, PlayerMark};
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let ri = hud
        .layout
        .rank_icons
        .clone()
        .expect("the level table's rank icons");
    println!(
        "rank icons: {} small icons, {} rank marks",
        ri.small.len(),
        ri.rank_mark.len()
    );
    assert!(ri.texture(0, false, "bot").is_some() && ri.texture(0, true, "bot").is_some());
    // gm-client (CLI-1305): a GM-named player's over-head mark is the GM rank icon's living form
    let gm = ri.texture(0, false, "GMtest").unwrap_or_default();
    println!("GM over-head icon: {gm}");
    assert!(gm.to_ascii_lowercase().contains("rank_gm_o"), "{gm}");
    let mut s = alive_state(100.0);
    let mark = |pid: u8, name: &str, at: [f32; 2], state: MarkState| PlayerMark {
        pid,
        obj: 100 + pid as u16,
        name: name.into(),
        at,
        state,
    };
    s.marks = vec![
        mark(1, "bot1", [300.0, 300.0], MarkState::Mate),
        mark(2, "bot2", [600.0, 300.0], MarkState::Enemy),
        mark(3, "bot3", [450.0, 500.0], MarkState::Dead),
    ];
    let img = hud.render(&s);
    save(&img, "rank_icons");
    // the icon's box: 29 x 29 ending left of the caption (the caption is centred on the point)
    let left_of = |at: [f32; 2]| {
        let x0 = (at[0] - 70.0) as i32;
        [
            x0,
            (at[1] - 17.0) as i32,
            (at[0] - x0 as f32 - 12.0) as i32,
            29,
        ]
    };
    let counts: Vec<usize> = s
        .marks
        .iter()
        .map(|m| count_in(&img, left_of(m.at), |_| true))
        .collect();
    let mut none = s.clone();
    none.ranks.clear();
    hud.layout.rank_icons = None;
    let plain = hud.render(&none);
    let base: Vec<usize> = s
        .marks
        .iter()
        .map(|m| count_in(&plain, left_of(m.at), |_| true))
        .collect();
    println!("pixels left of the names with icons {counts:?}, without {base:?}");
    for (c, b) in counts.iter().zip(&base) {
        assert!(*c > b + 60, "an icon left of each name");
    }
}

// ---- the sky table over the wire: our server's table reaches the client's frame state

#[test]
fn rom_sky_table_arrives_over_loopback_and_names_the_sky_model() {
    use dc_client::{Connect, NetOptions, NetWorld, WorldSource};
    use dc_dserver::session::{Output, Server};
    use std::net::{SocketAddr, UdpSocket};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    const MAP: &str = "JUNK_FLEA";
    let Some(rom) = rom() else {
        return;
    };
    let Ok(info) = dc_dserver::world::load_from_rom(
        &rom,
        &format!("Worlds\\{MAP}"),
        dc_dserver::world::NameForm::Listed,
        None,
    ) else {
        println!("skipped: {MAP} not in this ROM");
        return;
    };
    let (expected_block, _, table) = server_sky(&rom, MAP).expect("the server's sky table");
    let sky_names: Vec<Vec<u8>> = table
        .objects
        .values()
        .filter_map(|o| o.name.clone())
        .map(|n| n.to_ascii_lowercase())
        .collect();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let args = dc_dserver::cli::parse(
        [
            "-worldfile",
            "unused.dat",
            "-tick",
            "20",
            "-roundsecs",
            "120",
            "-skytable",
        ]
        .map(String::from),
    )
    .expect("server arguments");
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();

    let port = sock.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let server_thread = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let now = || start.elapsed().as_millis() as u32;
            let mut server: Server<SocketAddr> =
                Server::with_tables(args.server.clone(), info, tables, now());
            server.set_endpoint("127.0.0.1", port);
            assert!(server.handle_line(0, "user 1 1 1 0 0 Observer"));
            let rd = SocketReader::start(&sock, 4096).unwrap();
            let mut sent = false;
            while !stop.load(Ordering::SeqCst) {
                let mut got: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
                while let Some(Ok((d, from))) = rd.next_within(Duration::from_millis(4)) {
                    got.push((from, d));
                }
                let t = now();
                for (from, d) in got {
                    server.handle_datagram(t, from, &d);
                }
                for o in server.poll(t) {
                    match o {
                        Output::Transmit { to, bytes } => {
                            let _ = sock.send_to(&bytes, to);
                        }
                        Output::Log(l) if l.contains("sky table") => sent = true,
                        _ => {}
                    }
                }
            }
            sent
        })
    };
    let mut o = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "Observer",
    );
    o.rom = Some(rom.clone());
    let mut net = NetWorld::start(o);
    let handle = net.handle();
    // the frame loop's world load (no GPU): the session goes on to the world's tables
    let loaded = net.load(None);
    let t0 = Instant::now();
    let mut seen = None;
    let mut last = (None, None, None);
    while t0.elapsed() < Duration::from_secs(30) {
        let fs = net.frame_state();
        last = (
            fs.sky.as_ref().map(|s| s.models.len()),
            fs.life_state,
            fs.hud.first().cloned(),
        );
        if let Some(s) = fs.sky.filter(|s| !s.models.is_empty()) {
            seen = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = handle.shutdown(Duration::from_secs(5));
    stop.store(true, Ordering::SeqCst);
    let sent = server_thread.join().expect("server thread");
    assert!(loaded.is_ok(), "the client loads the world");
    let sky =
        seen.unwrap_or_else(|| panic!("no sky table with objects; last frame state {last:?}"));
    println!(
        "server sent the sky table: {sent}; client: {} sky objects resolved ({} visible), view box {:?}",
        sky.models.len(),
        sky.models.iter().filter(|m| m.1).count(),
        sky.view_box()
    );
    assert!(sent);
    assert_eq!(sky.block, expected_block, "the block arrives unchanged");
    assert!(sky
        .models
        .iter()
        .all(|(n, _)| sky_names.iter().any(|s| s == n)));
    assert!(sky.models.iter().any(|m| m.1), "a visible sky object");
}

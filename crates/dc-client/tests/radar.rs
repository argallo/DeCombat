//! The radar of the in-match HUD (top right): the map layer around the own point, the turning view disc and the
//! team-mate icons, through the HUD driver (CPU compositor) and through the headless frame loop.
//!
//! - `rom_radar_on_junk_flea`: with `CA_ROM`, the 2009 layout at 1920 x 1080 with the JUNK_FLEA radar picture and
//!   mapping of the lobby's room line (the room line dc-dserver turns into the 0x45 intro block, DSV-50), the own
//!   camera at the map's first spawn; a team mate is drawn, an enemy and a dead team mate are not. PNGs (ROM-derived,
//!   local only): `<render out>/dc-client/radar_rom_*.png`.
//! - `fallback_radar_draws_mates_only`: no ROM, our own radar layout.
//! - `headless_loop_draws_the_radar`: GPU-gated, the headless frame loop calls the radar each saved frame.
//!
//! No ROM value is in this file: positions come from the loaded layout, assertions are pixel comparisons.
// EVIDENCE: cshell+0xb9410 (map window, view disc), cshell+0xb60f0 (entries), cshell+0xb5f10 (draw order)

use dc_botclient::shell::RosterRecord;
use dc_client::hud::msgs::IntroBlock;
use dc_client::hud::radar::{RadarLayout, MAP_LAYER};
use dc_client::hud::state::HudEvent;
use dc_client::hud::{HudChoice, HudDriver};
use dc_client::overlay::text::RgbaImage;
use dc_client::world::{FrameState, RemoteBody, SynthSource};
use dc_client::WorldSource;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    out
}

fn save(img: &RgbaImage, name: &str) -> PathBuf {
    let p = out_dir().join(format!("{name}.png"));
    if let Some(b) = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()) {
        let _ = b.save(&p);
    }
    p
}

/// The radar's rectangle in target pixels, cropped out of `img` over a dark background (the PNG for the eye).
fn save_crop(img: &RgbaImage, l: &RadarLayout, name: &str) {
    let (sx, sy) = (img.width as f32 / 1024.0, img.height as f32 / 768.0);
    let (x0, y0) = ((l.place[0] * sx) as u32, (l.place[1] * sy) as u32);
    let (w, h) = ((l.size[0] * sx) as u32 + 2, (l.size[1] * sy) as u32 + 2);
    let mut out = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (ix, iy) = ((x0 + x).min(img.width - 1), (y0 + y).min(img.height - 1));
            let o = ((iy * img.width + ix) * 4) as usize;
            let a = img.rgba[o + 3] as f32 / 255.0;
            let bg = 40.0;
            let px = |c: u8| (c as f32 * a + bg * (1.0 - a)) as u8;
            out.put_pixel(
                x,
                y,
                image::Rgba([
                    px(img.rgba[o]),
                    px(img.rgba[o + 1]),
                    px(img.rgba[o + 2]),
                    255,
                ]),
            );
        }
    }
    let _ = out.save(out_dir().join(format!("{name}.png")));
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

fn body(pid: u8, team: u8, pos: [f32; 3]) -> RemoteBody {
    RemoteBody {
        obj: 200 + pid as u16,
        pid,
        name: format!("P{pid}"),
        team,
        pos,
        half: [16.0, 50.0, 16.0],
        yaw: None,
        look: None,
    }
}

/// Own player 0 on team 1, a team mate 1, an enemy 2 and team mate 3 (the caller marks him dead).
fn driver(choice: &HudChoice, w: u32, h: u32) -> HudDriver {
    let (mut d, src) =
        HudDriver::open(choice, w, h, Some((dc_client::testkit::synth_font(), 0))).unwrap();
    println!("HUD: {src}");
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(1),
        },
        HudEvent::Roster(rec(0, "AB", 1)),
        HudEvent::Roster(rec(1, "BA", 1)),
        HudEvent::Roster(rec(2, "AA", 2)),
        HudEvent::Roster(rec(3, "BB", 1)),
        HudEvent::Life(1),
    ] {
        d.state.apply(&e, 1000, 0.0);
    }
    d
}

/// Pixels that differ between two images inside `r` (x, y, w, h in target pixels).
fn diff(a: &RgbaImage, b: &RgbaImage, r: [f32; 4]) -> usize {
    let mut n = 0;
    for y in (r[1].max(0.0) as u32)..((r[1] + r[3]) as u32).min(a.height) {
        for x in (r[0].max(0.0) as u32)..((r[0] + r[2]) as u32).min(a.width) {
            let o = ((y * a.width + x) * 4) as usize;
            if a.rgba[o..o + 4] != b.rgba[o..o + 4] {
                n += 1;
            }
        }
    }
    n
}

fn drawn(img: &RgbaImage, r: [f32; 4]) -> usize {
    let mut n = 0;
    for y in (r[1].max(0.0) as u32)..((r[1] + r[3]) as u32).min(img.height) {
        for x in (r[0].max(0.0) as u32)..((r[0] + r[2]) as u32).min(img.width) {
            if img.rgba[((y * img.width + x) * 4 + 3) as usize] > 20 {
                n += 1;
            }
        }
    }
    n
}

fn target_rect(l: &RadarLayout, w: u32, h: u32) -> [f32; 4] {
    let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
    [
        l.place[0] * sx,
        l.place[1] * sy,
        l.size[0] * sx,
        l.size[1] * sy,
    ]
}

/// The radar picture and mapping of a room line's `radar=` field (picture | vec | vec | real | real | int | int).
fn intro_of_room_line(line: &str) -> Option<IntroBlock> {
    let f = line.split(' ').find_map(|t| t.strip_prefix("radar="))?;
    let p: Vec<&str> = f.split('|').collect();
    let v3 = |s: &str| -> Option<[f32; 3]> {
        let v: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    };
    Some(IntroBlock {
        radar: p.first()?.to_string(),
        radar_map_scale: v3(p.get(1)?)?,
        radar_map_offset: v3(p.get(2)?)?,
        radar_zoom: p.get(3)?.parse().ok()?,
        radar_half: p.get(4)?.parse().ok()?,
        radar_w: p.get(5)?.parse().ok()?,
        radar_h: p.get(6)?.parse().ok()?,
        ..IntroBlock::default()
    })
}

/// Composes the HUD with `remotes` seen from `own` looking along `yaw`.
fn shot(d: &mut HudDriver, remotes: &[RemoteBody], own: [f32; 3], yaw: f32) -> RgbaImage {
    let cam = dc_render::Camera::looking(glam::Vec3::from(own), yaw, 0.0, 1.5, 1.2);
    d.set_radar(remotes, &cam);
    d.image(true).expect("an image")
}

#[test]
fn rom_radar_on_junk_flea() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    // the lobby's room line of map 2 (JUNK_FLEA, the default room of the side-by-side captures) in team match
    let game_room = dc_bute::GameRoomData::from_rom(&rom).expect("room table");
    let mode = game_room
        .modes(false)
        .iter()
        .position(|m| m.get("Key").is_some_and(|k| k == "TDM"))
        .expect("a TDM mode row");
    let line = dc_bute::room_line(
        &rom,
        &game_room,
        &dc_bute::RoomConfig {
            map: 2,
            mode,
            ..dc_bute::RoomConfig::default()
        },
        &dc_bute::RoomOptions::default(),
    );
    let intro = intro_of_room_line(&line).expect("the room line names a radar picture");
    // the own camera at the map's first spawn
    let mut world = dc_client::RomWorld::new(&rom, "JUNK_FLEA");
    let loaded = world.load(None).expect("JUNK_FLEA loads");
    let spawn = loaded.spawn(0).expect("a spawn point");
    // `DC_RADAR_OWN=x,y,z` places the camera elsewhere (to match a capture's screenshot)
    let own = std::env::var("DC_RADAR_OWN")
        .ok()
        .and_then(|v| {
            let p: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (p.len() == 3).then(|| [p[0], p[1], p[2]])
        })
        .unwrap_or([spawn.pos.x, spawn.pos.y + 60.0, spawn.pos.z]);

    let (w, h) = (1920, 1080);
    let mut d = driver(&HudChoice::Rom(rom.clone()), w, h);
    let l = d
        .hud
        .radar
        .as_ref()
        .map(|r| r.layout.clone())
        .expect("the ROM radar layout resolves");
    d.state.intro_block = Some(intro);
    let r = target_rect(&l, w, h);
    let yaw = std::f32::consts::PI;
    let mate = body(1, 1, [own[0] + 300.0, own[1], own[2] + 150.0]);
    let enemy = body(2, 2, [own[0] - 250.0, own[1], own[2] - 100.0]);
    let dead = body(3, 1, [own[0] - 200.0, own[1], own[2] + 250.0]);

    let alone = shot(&mut d, &[], own, yaw);
    assert!(
        drawn(&alone, r) as f32 > r[2] * r[3] * 0.5,
        "the radar disc covers its rectangle"
    );
    // the map picture shows through: the layer is not a flat colour
    let layer = d.hud.assets.texture(MAP_LAYER).expect("map layer composed");
    let mut colours: Vec<[u8; 3]> = layer
        .rgba
        .chunks(4)
        .filter(|p| p[3] > 50)
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    colours.sort();
    colours.dedup();
    assert!(
        colours.len() > 50,
        "the picture has detail: {} colours",
        colours.len()
    );
    save(&alone, "radar_rom_hud_alone");
    save_crop(&alone, &l, "radar_rom_crop_alone");

    // the enemy and the dead team mate change nothing
    d.state.roster.get_mut(&3).unwrap().dead = true;
    let others = shot(&mut d, &[enemy.clone(), dead.clone()], own, yaw);
    assert_eq!(diff(&alone, &others, r), 0, "enemy / dead mate not drawn");
    assert!(d.state.radar.as_ref().unwrap().blips.is_empty());
    // the team mate is drawn
    let with_mate = shot(&mut d, &[mate.clone(), enemy, dead], own, yaw);
    let f = d.state.radar.clone().unwrap();
    assert_eq!(f.blips.len(), 1);
    let b = &f.blips[0];
    let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
    let icon = [b.at[0] * sx, b.at[1] * sy, l.icon[0] * sx, l.icon[1] * sy];
    assert!(
        diff(&alone, &with_mate, icon) > 20,
        "the mate icon is drawn"
    );
    save(&with_mate, "radar_rom_hud_mate");
    save_crop(&with_mate, &l, "radar_rom_crop_mate");
    // the view disc turns with the view: another yaw changes the radar but not the mate icon's place
    let turned = shot(&mut d, &[mate], own, yaw - std::f32::consts::FRAC_PI_2);
    assert!(diff(&with_mate, &turned, r) > 100);
    assert_eq!(d.state.radar.as_ref().unwrap().blips[0].at, b.at);
    save_crop(&turned, &l, "radar_rom_crop_turned");
    println!("room line radar field and spawn used; blip at {:?}", b.at);
}

#[test]
fn fallback_radar_draws_mates_only() {
    let (w, h) = (1024, 768);
    let mut d = driver(&HudChoice::Fallback, w, h);
    let l = d
        .hud
        .radar
        .as_ref()
        .map(|r| r.layout.clone())
        .expect("our radar layout");
    let r = target_rect(&l, w, h);
    let own = [0.0, 50.0, 0.0];
    let alone = shot(&mut d, &[], own, 0.0);
    assert!(drawn(&alone, r) > 1000, "the generated disc is drawn");
    let enemy = body(2, 2, [200.0, 50.0, 0.0]);
    let others = shot(&mut d, std::slice::from_ref(&enemy), own, 0.0);
    assert_eq!(diff(&alone, &others, r), 0);
    let mate = body(1, 1, [0.0, 50.0, 400.0]);
    let with_mate = shot(&mut d, &[mate, enemy], own, 0.0);
    let b = d.state.radar.as_ref().unwrap().blips[0].at;
    // 400 units north: straight above the origin
    assert_eq!(b[0], l.place[0] + l.blip_origin[0]);
    assert!(b[1] < l.place[1] + l.blip_origin[1]);
    let o = (((b[1] + 2.0) as u32 * w + (b[0] + 2.0) as u32) * 4) as usize;
    assert_eq!(&with_mate.rgba[o..o + 3], &l.mate_colour[..3]);
    save(&with_mate, "radar_fallback_hud");
}

/// The synthetic world with a team mate a little ahead, roster events on the first frame.
struct MateSource {
    inner: SynthSource,
    frame: u32,
    mate: Option<RemoteBody>,
}

impl WorldSource for MateSource {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn load(
        &mut self,
        gpu: Option<dc_client::world::GpuTarget<'_>>,
    ) -> Result<dc_client::world::LoadedWorld, dc_client::ClientError> {
        let w = self.inner.load(gpu)?;
        let s = w.spawn(0)?;
        self.mate = Some(body(1, 1, [s.pos.x + 150.0, s.pos.y + 50.0, s.pos.z]));
        Ok(w)
    }
    fn frame_state(&mut self) -> FrameState {
        self.frame += 1;
        let t = self.frame * 50;
        let mut fs = FrameState {
            life_state: Some(1),
            clock_ms: Some(t),
            remotes: self.mate.iter().cloned().collect(),
            ..FrameState::default()
        };
        if self.frame == 1 {
            fs.hud_events = vec![
                (
                    t,
                    HudEvent::Own {
                        pid: Some(0),
                        mode_class: Some(1),
                    },
                ),
                (t, HudEvent::Roster(rec(0, "AB", 1))),
                (t, HudEvent::Roster(rec(1, "BA", 1))),
                (t, HudEvent::Life(1)),
            ];
        }
        fs
    }
}

#[test]
fn headless_loop_draws_the_radar() {
    let hud = match dc_rom::test_rom() {
        Some(rom) => HudChoice::Rom(rom),
        None => HudChoice::Fallback,
    };
    let opts = dc_client::HeadlessOptions {
        frames: 3,
        width: 1024,
        height: 768,
        out: Some(out_dir()),
        prefix: "radar_headless_".into(),
        save_every: 1000,
        font_bytes: Some(dc_client::testkit::synth_font()),
        hud: hud.clone(),
        debug_panel: false,
        ..dc_client::HeadlessOptions::default()
    };
    let mut src = MateSource {
        inner: dc_client::testkit::synth_source(),
        frame: 0,
        mate: None,
    };
    let r = dc_client::run_headless(&mut src, None, &opts).expect("headless run");
    if r.adapter.is_none() {
        println!("skipped: no adapter ({:?})", r.render_skipped);
        return;
    }
    let img = r.last_hud.expect("a HUD image");
    let l = match &hud {
        HudChoice::Rom(_) => {
            // the same layout the loop loaded
            let (d, _) = HudDriver::open(&hud, 1024, 768, None).unwrap();
            d.hud
                .radar
                .as_ref()
                .map(|r| r.layout.clone())
                .expect("ROM radar")
        }
        _ => RadarLayout::fallback(),
    };
    let rect = target_rect(&l, 1024, 768);
    assert!(drawn(&img, rect) > 1000, "radar drawn by the frame loop");
    save(&img, "radar_headless_hud");
    if let Some(p) = r.images.last() {
        let _ = std::fs::rename(p, out_dir().join("radar_headless_frame.png"));
    }
}

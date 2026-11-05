//! Offscreen frames of the world plus the ROM HUD and the debug panel at fixed viewpoints, for before / after
//! comparisons of the overlay path (`png_diff` compares two output folders).
//!
//! ```text
//! cargo run --release -p dc-client --example overlay_shots -- --rom <CA_ROM> --map JUNK_FLEA \
//!     --viewpoints <json> --size 2560x1440 --out <dir> [--path cpu|gpu]
//! ```
//! The viewpoint file is a JSON list of `{name, pos: [x, y, z], eye_y, yaw_deg, pitch_deg}` (our own capture
//! tooling writes it from a session's update log). Every frame gets the same made-up HUD state (roster, health,
//! ammunition, kill feed, damage arcs, a team mate's name mark ahead of the camera, the radar, the aim mark; every
//! third frame also the scoreboard). The images are derived from the ROM: local files only, never committed.
//! `--path cpu` draws the HUD as one CPU-composed image (`OverlayRenderer::update`), `--path gpu` as the batched
//! display list (`OverlayRenderer::update_items`, when the build has it).

use dc_botclient::shell::{KillRecord, RosterRecord};
use dc_client::hud::state::HudEvent;
use dc_client::hud::{AmmoView, HudChoice, HudDriver};
use dc_client::offscreen::Offscreen;
use dc_client::overlay::{load_rom_font, OverlayRenderer};
use dc_client::world::{GpuTarget, RemoteBody};
use dc_client::{RomWorld, ViewAngles, ViewSettings, WorldSource};
use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
use std::path::PathBuf;

struct Args {
    rom: PathBuf,
    map: String,
    viewpoints: PathBuf,
    size: (u32, u32),
    out: PathBuf,
    gpu: bool,
}

fn args() -> Result<Args, String> {
    let mut rom = std::env::var_os("CA_ROM").map(PathBuf::from);
    let (mut map, mut vp, mut out) = ("JUNK_FLEA".to_string(), None, None);
    let mut size = (2560, 1440);
    let mut gpu = false;
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or(format!("{k}: value"));
        match k.as_str() {
            "--rom" => rom = Some(PathBuf::from(v()?)),
            "--map" => map = v()?,
            "--viewpoints" => vp = Some(PathBuf::from(v()?)),
            "--out" => out = Some(PathBuf::from(v()?)),
            "--size" => {
                let s = v()?;
                let (w, h) = s.split_once('x').ok_or("--size WxH")?;
                size = (
                    w.parse().map_err(|_| "--size")?,
                    h.parse().map_err(|_| "--size")?,
                );
            }
            "--path" => gpu = v()? == "gpu",
            o => return Err(format!("unknown argument {o}")),
        }
    }
    Ok(Args {
        rom: rom.ok_or("--rom or CA_ROM")?,
        map,
        viewpoints: vp.ok_or("--viewpoints")?,
        size,
        out: out.ok_or("--out")?,
        gpu,
    })
}

fn events(now: u32) -> Vec<(u32, HudEvent)> {
    let rec = |pid: u8, name: &str, team: u8| RosterRecord {
        name: name.into(),
        pid,
        team,
        complete: true,
        ..RosterRecord::default()
    };
    let kill = |victim: u8, killer: u8, zone: u8| KillRecord {
        victim,
        killer,
        kill_kind: 1,
        weapon: 145,
        hit_zone: zone,
        feed_value: 2,
        complete: true,
        ..KillRecord::default()
    };
    vec![
        (
            now,
            HudEvent::Own {
                pid: Some(0),
                mode_class: Some(1),
            },
        ),
        (now, HudEvent::Roster(rec(0, "alpha", 1))),
        (now, HudEvent::Roster(rec(1, "bravo", 1))),
        (now, HudEvent::Roster(rec(2, "charlie", 2))),
        (now, HudEvent::Life(1)),
        (
            now,
            HudEvent::Status {
                kind: 0x0f,
                value: 100.0,
            },
        ),
        (
            now,
            HudEvent::Status {
                kind: 2,
                value: 73.0,
            },
        ),
        (
            now,
            HudEvent::Status {
                kind: 3,
                value: 41.0,
            },
        ),
        (now, HudEvent::RoundBegin(180)),
        (now + 100, HudEvent::Kill(kill(2, 0, 1))),
        (now + 200, HudEvent::Kill(kill(0, 2, 3))),
        (now + 300, HudEvent::Joined("delta".into())),
        (now + 400, HudEvent::Damage([-0.8, 0.0, 0.3])),
    ]
}

fn main() -> Result<(), String> {
    let a = args()?;
    let (w, h) = a.size;
    let text = std::fs::read_to_string(&a.viewpoints).map_err(|e| e.to_string())?;
    let vps: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let ctx = GpuContext::headless(AdapterChoice::Auto).map_err(|e| e.to_string())?;
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = RomWorld::new(&a.rom, &a.map);
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .map_err(|e| e.to_string())?;
    let scene = world.scene.as_ref().ok_or("no scene")?;
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, w, h);
    let mut layer = OverlayRenderer::new(&ctx, OFFSCREEN_FORMAT);
    let mut panel = OverlayRenderer::new(&ctx, OFFSCREEN_FORMAT);
    let font = load_rom_font(&a.rom).ok();
    std::fs::create_dir_all(&a.out).map_err(|e| e.to_string())?;
    for (i, vp) in vps.iter().enumerate() {
        let name = vp["name"].as_str().unwrap_or("vp").to_string();
        let f = |k: &str, j: usize| vp[k][j].as_f64().unwrap_or(0.0) as f32;
        let eye_y = vp["eye_y"].as_f64().unwrap_or(33.0) as f32;
        let eye = dc_sim::V3::new(f("pos", 0), f("pos", 1) + eye_y, f("pos", 2));
        let view = ViewAngles {
            yaw: (vp["yaw_deg"].as_f64().unwrap_or(0.0) as f32).to_radians(),
            pitch: (vp["pitch_deg"].as_f64().unwrap_or(0.0) as f32).to_radians(),
        };
        let camera = ViewSettings::default().camera(eye, &view, w, h);
        // a fresh HUD per frame: the same state for both paths
        let (mut hd, _) =
            HudDriver::open(&HudChoice::Rom(a.rom.clone()), w, h, None).ok_or("no HUD")?;
        let now = 5000;
        for (t, e) in events(now) {
            hd.state.apply(&e, t, view.yaw);
        }
        let life = hd.hud.layout.feed.lifetime_ms.max(1);
        hd.state.tick(now + 500, life);
        hd.state.ammo = Some(AmmoView {
            weapon: 145,
            clip: 27,
            reserve: 90,
            clip_size: Some(30),
            single_shot: false,
            order_class: 1,
            infinite: false,
        });
        hd.state.aim_mark = Some(dc_client::spread::AimMarkView {
            shape: dc_client::spread::mark::SHAPE_PLAIN,
            distance: 6.0,
            since_shot_s: None,
            colour: [0, 255, 0, 255],
            rest: 6.0,
        });
        hd.state.show_board = i % 3 == 2;
        // a team mate 300 units ahead of the camera, its name mark over its head
        let fwd = camera.forward();
        let mate = RemoteBody {
            obj: 7,
            pid: 1,
            name: "bravo".into(),
            team: 1,
            pos: [eye.x + fwd.x * 300.0, eye.y - 10.0, eye.z + fwd.z * 300.0],
            half: [16.0, 50.0, 16.0],
            yaw: Some(0.0),
            look: None,
        };
        let remotes = vec![mate];
        hd.set_marks(&remotes, &camera, None);
        hd.set_radar(&remotes, &camera);
        draw_hud(&ctx, &mut layer, &mut hd, a.gpu, (w, h));
        if let Some(font) = &font {
            let lines = vec![
                format!("{} viewpoint {name}", a.map),
                format!("eye {:.1} {:.1} {:.1}", eye.x, eye.y, eye.z),
                format!("yaw {:.3} pitch {:.3}", view.yaw, view.pitch),
            ];
            draw_panel(&ctx, &mut panel, &font.atlas, &lines, a.gpu);
        }
        let img = target
            .render_with_hud(
                &ctx,
                &renderer,
                scene,
                &camera,
                None,
                &[],
                &[],
                Some(&layer),
                font.as_ref().map(|_| &panel),
            )
            .map_err(|e| e.to_string())?;
        img.save_png(&a.out.join(format!("{name}.png")))
            .map_err(|e| e.to_string())?;
        println!(
            "{name}: {} HUD textures missing",
            hd.hud.assets.missing.len()
        );
    }
    Ok(())
}

fn draw_hud(
    ctx: &GpuContext,
    layer: &mut OverlayRenderer,
    hd: &mut HudDriver,
    gpu: bool,
    _size: (u32, u32),
) {
    if gpu {
        hd.draw_into(ctx, layer, true);
    } else if let Some(img) = hd.image(true) {
        layer.update(ctx, &img);
    }
}

fn draw_panel(
    ctx: &GpuContext,
    panel: &mut OverlayRenderer,
    atlas: &dc_client::overlay::GlyphAtlas,
    lines: &[String],
    gpu: bool,
) {
    if gpu {
        panel.update_panel(ctx, atlas, lines);
    } else {
        panel.update(ctx, &atlas.panel(lines));
    }
}

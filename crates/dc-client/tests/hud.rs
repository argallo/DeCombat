//! The in-match HUD drawn over a rendered frame.
//!
//! ROM-gated GPU tests (skipped without `CA_ROM` or an adapter): the HUD of the user's ROM (layout set 0, ROM
//! textures and fonts) over the made-up world; the health digits change with 0x66 kind 2, the kill feed shows a line
//! after a 0x6d, the scoreboard appears while its key is held, and the damage arc lies on the side the attacker
//! stands. Nothing of the ROM is asserted by value: regions come from the loaded layout, assertions are pixel counts.
//! The no-ROM test runs the headless loop with our fallback layout and the made-up font.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 (0x6d), §4.3 (0x66, 0x74); FMT_CRES_UI.md §8; RENDER_2009.md §12

use dc_botclient::shell::{KillRecord, RosterRecord};
use dc_client::hud::state::HudEvent;
use dc_client::hud::{Hud, HudChoice, HudState};
use dc_client::offscreen::Offscreen;
use dc_client::overlay::OverlayRenderer;
use dc_client::testkit::{synth_font, synth_source};
use dc_client::world::{FrameState, GpuTarget, LoadedWorld};
use dc_client::{
    run_headless, ClientError, HeadlessOptions, LocalPlayer, ViewAngles, ViewSettings, WorldSource,
};
use dc_render::{
    AdapterChoice, FrameImage, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT,
};

const W: u32 = 1024;
const H: u32 = 768;

struct Gpu {
    ctx: GpuContext,
    renderer: WorldRenderer,
    world: LoadedWorld,
    target: Offscreen,
    layer: OverlayRenderer,
}

fn gpu() -> Option<Gpu> {
    let ctx = match GpuContext::headless(AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return None;
        }
    };
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let mut src = synth_source();
    let world = src
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("synthetic world");
    let target = Offscreen::new(&ctx, OFFSCREEN_FORMAT, W, H);
    let layer = OverlayRenderer::new(&ctx, OFFSCREEN_FORMAT);
    Some(Gpu {
        ctx,
        renderer,
        world,
        target,
        layer,
    })
}

impl Gpu {
    fn frame(&mut self, hud: &mut Hud, state: &HudState, name: &str) -> FrameImage {
        let spawn = self.world.spawn(0).unwrap();
        let me = LocalPlayer::spawn(
            dc_sim::MovementParams::default(),
            &self.world.collision,
            &spawn,
            self.world.stand_dims,
            60,
        );
        let view = ViewAngles {
            yaw: spawn.yaw,
            pitch: 0.0,
        };
        let camera = ViewSettings::default().camera(me.eye_target(&view), &view, W, H);
        self.layer.update(&self.ctx, &hud.render(state));
        let img = self
            .target
            .render_with_hud(
                &self.ctx,
                &self.renderer,
                self.world.scene.as_ref().unwrap(),
                &camera,
                None,
                &[],
                &[],
                Some(&self.layer),
                None,
            )
            .unwrap();
        let out = dc_render::default_out_dir().join("dc-client");
        let _ = std::fs::create_dir_all(&out);
        let _ = img.save_png(&out.join(format!("hud_{name}.png")));
        img
    }
}

/// Pixels that differ between two frames inside `rect` (x, y, w, h in target pixels), and outside it.
fn diff(a: &FrameImage, b: &FrameImage, rect: [u32; 4]) -> (usize, usize) {
    let (mut inside, mut outside) = (0, 0);
    for y in 0..a.height {
        for x in 0..a.width {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            let d: i32 = (0..3).map(|k| (p[k] as i32 - q[k] as i32).abs()).sum();
            if d > 24 {
                let inr =
                    x >= rect[0] && x < rect[0] + rect[2] && y >= rect[1] && y < rect[1] + rect[3];
                if inr {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    (inside, outside)
}

fn bounds(points: &[[i32; 2]], size: [i32; 2]) -> [u32; 4] {
    let x0 = points.iter().map(|p| p[0]).min().unwrap();
    let y0 = points.iter().map(|p| p[1]).min().unwrap();
    let x1 = points.iter().map(|p| p[0] + size[0]).max().unwrap();
    let y1 = points.iter().map(|p| p[1] + size[1]).max().unwrap();
    [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]
}

fn apply(state: &mut HudState, hud: &Hud, now: u32, events: &[HudEvent]) {
    state.texts.join = hud.layout.chat.join_form.clone();
    state.texts.leave = hud.layout.chat.leave_form.clone();
    for e in events {
        state.apply(e, now, 0.0);
    }
    state.tick(now, hud.layout.feed.lifetime_ms.max(1));
}

fn alive_state(hud: &Hud) -> HudState {
    let mut s = HudState::default();
    let rec = |pid: u8, name: &str, team: u8| RosterRecord {
        name: name.into(),
        pid,
        team,
        complete: true,
        ..RosterRecord::default()
    };
    apply(
        &mut s,
        hud,
        1000,
        &[
            HudEvent::Own {
                pid: Some(0),
                mode_class: Some(1),
            },
            HudEvent::Roster(rec(0, "alpha", 1)),
            HudEvent::Roster(rec(1, "bravo", 2)),
            HudEvent::Life(1),
            HudEvent::Status {
                kind: 0x0f,
                value: 100.0,
            },
            HudEvent::Status {
                kind: 2,
                value: 100.0,
            },
            HudEvent::Status {
                kind: 3,
                value: 100.0,
            },
            HudEvent::RoundBegin(180),
        ],
    );
    s
}

fn rom_hud() -> Option<(Gpu, Hud)> {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return None;
    };
    let g = gpu()?;
    let hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    println!("HUD: {}", hud.source);
    Some((g, hud))
}

#[test]
fn rom_hud_health_digits_follow_status_kind_2() {
    let Some((mut g, mut hud)) = rom_hud() else {
        return;
    };
    assert!(hud.layout.from_rom);
    let d = hud
        .layout
        .health
        .health_digits
        .clone()
        .expect("health digits");
    let full = alive_state(&hud);
    let a = g.frame(&mut hud, &full, "health_100");
    let mut hit = full.clone();
    apply(
        &mut hit,
        &hud,
        1000,
        &[HudEvent::Status {
            kind: 2,
            value: 47.0,
        }],
    );
    let b = g.frame(&mut hud, &hit, "health_47");
    let rect = bounds(&d.pos, d.size);
    let (inside, outside) = diff(&a, &b, rect);
    // the health arc beside the aim mark changes with the value too (its own region)
    let arc = hud
        .layout
        .health
        .health_arc
        .clone()
        .map(|g| bounds(&[g.pos], g.size));
    let arc_px = arc.map(|r| diff(&a, &b, r).0).unwrap_or(0);
    println!("health digits {rect:?}: {inside} pixels changed inside, {outside} outside ({arc_px} in the arc)");
    assert!(inside > 40, "the digits changed: {inside}");
    assert!(
        outside <= arc_px + inside / 10,
        "nothing else moved: {outside} outside, {arc_px} of them in the arc"
    );
    // the HUD textures resolved
    assert!(
        hud.assets.missing.is_empty(),
        "unresolved HUD textures: {}",
        hud.assets.missing.len()
    );
}

#[test]
fn rom_hud_kill_feed_shows_a_line_after_a_kill() {
    let Some((mut g, mut hud)) = rom_hud() else {
        return;
    };
    let base = alive_state(&hud);
    let a = g.frame(&mut hud, &base, "feed_empty");
    let mut s = base.clone();
    // at the base state's time: the round clock (truncated seconds) shows the same digits in both frames
    apply(
        &mut s,
        &hud,
        1000,
        &[HudEvent::Kill(KillRecord {
            victim: 1,
            killer: 0,
            kill_kind: 1,
            weapon: 145,
            hit_zone: 1,
            feed_value: 2,
            complete: true,
            ..KillRecord::default()
        })],
    );
    assert_eq!(s.feed.len(), 1);
    assert_eq!(s.roster[&0].kills, 1, "the client counts the kill itself");
    let b = g.frame(&mut hud, &s, "feed_line");
    let f = &hud.layout.feed;
    let top = (f.pos[1] + f.weapon_lift.min(0)).max(0) as u32;
    let rect = [0, top, W / 2, (f.line_step.max(16) + 16) as u32];
    let (inside, outside) = diff(&a, &b, rect);
    println!("feed line {rect:?}: {inside} pixels changed, {outside} outside");
    assert!(inside > 150, "a feed line is drawn: {inside}");
    assert!(outside < inside / 4, "only the feed line: {outside}");
    assert!(
        hud.layout.weapon_icons.contains_key(&145),
        "weapon 145 has a kill icon"
    );
}

#[test]
fn rom_hud_scoreboard_appears_with_the_board_key() {
    let Some((mut g, mut hud)) = rom_hud() else {
        return;
    };
    let base = alive_state(&hud);
    let a = g.frame(&mut hud, &base, "board_off");
    let mut s = base.clone();
    s.show_board = true;
    let b = g.frame(&mut hud, &s, "board_on");
    let w = hud.layout.board.window.clone().expect("board window");
    println!(
        "board font {} size {:?}",
        hud.layout.board.font, hud.layout.board.font_size
    );
    let rect = bounds(&[w.pos], w.size);
    let (inside, outside) = diff(&a, &b, rect);
    let area = (rect[2] * rect[3]) as usize;
    println!("board {rect:?}: {inside} of {area} pixels changed, {outside} outside");
    assert!(
        inside * 4 > area,
        "the window covers its rectangle: {inside} of {area}"
    );
    assert!(
        outside < inside / 20,
        "nothing outside the window: {outside}"
    );
}

#[test]
fn rom_hud_damage_arc_is_on_the_attackers_side() {
    let Some((mut g, mut hud)) = rom_hud() else {
        return;
    };
    let mut base = alive_state(&hud);
    // the arcs light only inside the Reality gate (the unit tests cover the gate itself)
    base.reality = true;
    let a = g.frame(&mut hud, &base, "arc_none");
    let hit = |hud: &Hud, dir: [f32; 3]| {
        let mut s = base.clone();
        apply(&mut s, hud, 1000, &[HudEvent::Damage(dir)]);
        s
    };
    // view yaw 0 looks along +Z; an attacker at +X (the right) shoots toward -X
    let sr = hit(&hud, [-0.8, 0.0, 0.0]);
    let sl = hit(&hud, [0.8, 0.0, 0.0]);
    let right = g.frame(&mut hud, &sr, "arc_right");
    let left = g.frame(&mut hud, &sl, "arc_left");
    let halves = |img: &FrameImage| {
        let l = diff(&a, img, [0, 0, W / 2, H]);
        (l.0, l.1)
    };
    let (r_left, r_right) = halves(&right);
    let (l_left, l_right) = halves(&left);
    println!("attacker right: {r_left} left / {r_right} right; attacker left: {l_left} left / {l_right} right");
    assert!(r_right > 200 && r_right > 4 * r_left, "right arc");
    assert!(l_left > 200 && l_left > 4 * l_right, "left arc");
}

/// A synthetic world whose frame state delivers HUD inputs at chosen frames (a stand-in for the network source).
struct Scripted {
    inner: dc_client::world::SynthSource,
    frame: u32,
}

impl WorldSource for Scripted {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        self.inner.load(gpu)
    }
    fn frame_state(&mut self) -> FrameState {
        self.frame += 1;
        let mut fs = FrameState {
            life_state: Some(1),
            ..FrameState::default()
        };
        let t = self.frame * 50;
        if self.frame == 1 {
            fs.hud_events = vec![
                (t, HudEvent::Life(1)),
                (
                    t,
                    HudEvent::Status {
                        kind: 2,
                        value: 100.0,
                    },
                ),
            ];
        }
        if self.frame == 6 {
            fs.hud_events = vec![(
                t,
                HudEvent::Status {
                    kind: 2,
                    value: 7.0,
                },
            )];
        }
        fs.clock_ms = Some(t);
        fs
    }
}

#[test]
fn headless_fallback_hud_without_a_rom() {
    let run = |frames: u32| {
        let opts = HeadlessOptions {
            frames,
            width: 512,
            height: 384,
            out: None,
            font_bytes: Some(synth_font()),
            hud: HudChoice::Fallback,
            debug_panel: false,
            ..HeadlessOptions::default()
        };
        let mut src = Scripted {
            inner: synth_source(),
            frame: 0,
        };
        run_headless(&mut src, None, &opts).expect("headless run")
    };
    let early = run(3);
    if early.adapter.is_none() {
        println!("skipped: no adapter ({:?})", early.render_skipped);
        return;
    }
    let late = run(8);
    assert_eq!(early.hud.as_deref(), Some("fallback layout (no ROM)"));
    assert_eq!((early.hud_events, late.hud_events), (2, 3));
    let (a, b) = (early.last_hud.unwrap(), late.last_hud.unwrap());
    // the fallback health bar spans virtual (24, 714) + (110, 8): target (12, 357) + (55, 4) at half scale
    let changed = (357..361u32)
        .flat_map(|y| (12..67u32).map(move |x| (x, y)))
        .filter(|&(x, y)| a.pixel(x, y) != b.pixel(x, y))
        .count();
    let any_text = (0..a.width * a.height)
        .filter(|i| a.pixel(i % a.width, i / a.width)[3] > 0)
        .count();
    println!("fallback HUD: {any_text} drawn pixels, {changed} changed in the health bar");
    assert!(any_text > 200, "bars and text drawn: {any_text}");
    assert!(
        changed > 100,
        "the health bar follows 0x66 kind 2: {changed}"
    );
}

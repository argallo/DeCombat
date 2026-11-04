//! The colour of the names over the players' heads, through the headless frame loop.
//!
//! The first nametag test counted drawn pixels only, so a tag in the wrong colour passed. Here the HUD layer is read
//! pixel by pixel: a team mate's name carries the team-mate colour of the layout, the enemy under the cross the enemy
//! colour, a dead player's name the dead colour; and a player who died and was then refreshed by a record with the
//! dead flag clear (the respawn) is a mate / an enemy again, not stuck in the dead colour. GPU-gated (skipped without
//! an adapter). The fallback layout always runs, the ROM layout when `CA_ROM` is set; the expected colours come from
//! the loaded layout, no ROM value is in this file. PNGs: `<render out>/dc-client/namecolours_*.png`.
// EVIDENCE: cshell+0x8b2e0 (dead flag -> dead colour; own team -> same-team colour; else enemy colour),
// cshell+0x8a760 (a record of a known client id refreshes the marker's flag), cshell+0x8b110 (the colours, set 0)

use dc_botclient::shell::{KillRecord, RosterRecord};
use dc_client::hud::marks::mark_point;
use dc_client::hud::state::HudEvent;
use dc_client::hud::{HudChoice, HudLayout};
use dc_client::overlay::text::RgbaImage;
use dc_client::world::{FrameState, RemoteBody, SynthSource};
use dc_client::{LocalPlayer, ViewAngles, ViewSettings, WorldSource};
use std::path::PathBuf;

/// Names in the two glyphs of the made-up test font (`testkit::synth_font`: 'A' and 'B').
const MATE: &str = "ABBA";
const ENEMY: &str = "BABA";

const W: u32 = 1024;
const H: u32 = 768;

fn out_dir() -> PathBuf {
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    out
}

fn rec(pid: u8, name: &str, team: u8, dead: bool) -> RosterRecord {
    RosterRecord {
        name: name.into(),
        pid,
        team,
        kind: 2,
        dead,
        complete: true,
        ..RosterRecord::default()
    }
}

fn kill(victim: u8, killer: u8) -> KillRecord {
    KillRecord {
        victim,
        killer,
        weapon: 145,
        hit_zone: 3,
        complete: true,
        ..KillRecord::default()
    }
}

/// Where the headless loop's player stands and looks: eye and view of the synthetic world's first spawn.
fn viewpoint() -> (dc_sim::V3, ViewAngles) {
    let mut src = dc_client::testkit::synth_source();
    let world = src.load(None).expect("synthetic world");
    let spawn = world.spawn(0).unwrap();
    let me = LocalPlayer::spawn(
        dc_sim::MovementParams::default(),
        &world.collision,
        &spawn,
        world.stand_dims,
        60,
    );
    let view = ViewAngles {
        yaw: spawn.yaw,
        pitch: 0.0,
    };
    (me.eye_target(&view), view)
}

/// A remote box `ahead` units along the view and `side` units to its right, its centre 20 below the eye (the eye ray
/// passes through the box).
fn body(obj: u16, pid: u8, team: u8, name: &str, ahead: f32, side: f32) -> RemoteBody {
    let (eye, view) = viewpoint();
    let (s, c) = view.yaw.sin_cos();
    RemoteBody {
        obj,
        pid,
        name: name.into(),
        team,
        pos: [
            eye.x + s * ahead + c * side,
            eye.y - 20.0,
            eye.z + c * ahead - s * side,
        ],
        half: [16.0, 50.0, 16.0],
        yaw: Some(view.yaw + std::f32::consts::PI),
        look: None,
    }
}

/// The synthetic world with the scripted HUD events (frame number from 1) and `remotes` on every frame.
struct MarkSource {
    inner: SynthSource,
    frame: u32,
    remotes: Vec<RemoteBody>,
    script: Vec<(u32, HudEvent)>,
}

impl WorldSource for MarkSource {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn load(
        &mut self,
        gpu: Option<dc_client::world::GpuTarget<'_>>,
    ) -> Result<dc_client::world::LoadedWorld, dc_client::ClientError> {
        self.inner.load(gpu)
    }
    fn frame_state(&mut self) -> FrameState {
        self.frame += 1;
        let t = self.frame * 50;
        let mut fs = FrameState {
            life_state: Some(1),
            clock_ms: Some(t),
            remotes: self.remotes.clone(),
            ..FrameState::default()
        };
        fs.hud_events = self
            .script
            .iter()
            .filter(|(f, _)| *f == self.frame)
            .map(|(_, e)| (t, e.clone()))
            .collect();
        fs
    }
}

/// The three roster records and the own state of frame 1.
fn setup() -> Vec<(u32, HudEvent)> {
    vec![
        (
            1,
            HudEvent::Own {
                pid: Some(0),
                mode_class: Some(1),
            },
        ),
        (1, HudEvent::Roster(rec(0, "casey", 1, false))),
        (1, HudEvent::Roster(rec(1, MATE, 1, false))),
        (1, HudEvent::Roster(rec(2, ENEMY, 2, false))),
        (1, HudEvent::Life(1)),
    ]
}

fn run(
    hud: &HudChoice,
    remotes: Vec<RemoteBody>,
    script: Vec<(u32, HudEvent)>,
    name: &str,
) -> Option<RgbaImage> {
    let dir = out_dir();
    let opts = dc_client::HeadlessOptions {
        frames: 4,
        width: W,
        height: H,
        out: Some(dir.clone()),
        prefix: format!("namecolours_{name}_"),
        save_every: 1000,
        font_bytes: Some(dc_client::testkit::synth_font()),
        hud: hud.clone(),
        debug_panel: false,
        debug_boxes: true,
        ..dc_client::HeadlessOptions::default()
    };
    let mut src = MarkSource {
        inner: dc_client::testkit::synth_source(),
        frame: 0,
        remotes,
        script,
    };
    let r = dc_client::run_headless(&mut src, None, &opts).expect("headless run");
    if r.adapter.is_none() {
        println!("skipped: no adapter ({:?})", r.render_skipped);
        return None;
    }
    let hud = r.last_hud.expect("a HUD image");
    if let Some(p) = r.images.last() {
        let _ = std::fs::rename(p, dir.join(format!("namecolours_{name}.png")));
    }
    if let Some(b) = image::RgbaImage::from_raw(hud.width, hud.height, hud.rgba.clone()) {
        let _ = b.save(dir.join(format!("namecolours_hud_{name}.png")));
    }
    Some(hud)
}

/// The target-pixel box around a remote's name point (the caption is centred on it).
fn tag_box(r: &RemoteBody) -> [f32; 4] {
    let (eye, view) = viewpoint();
    let camera = ViewSettings::default().camera(eye, &view, W, H);
    let p = dc_client::hud::project(&camera, mark_point(r)).expect("in front");
    let (sx, sy) = (W as f32 / 1024.0, H as f32 / 768.0);
    [p[0] * sx - 70.0, p[1] * sy - 14.0, 140.0, 28.0]
}

/// How many fully opaque HUD pixels inside `r` have the colour `c` (within 2 per channel).
fn pixels_of(img: &RgbaImage, r: [f32; 4], c: [u8; 4]) -> usize {
    let (x0, y0) = (r[0].max(0.0) as u32, r[1].max(0.0) as u32);
    let (x1, y1) = (
        ((r[0] + r[2]) as u32).min(img.width),
        ((r[1] + r[3]) as u32).min(img.height),
    );
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * img.width + x) * 4) as usize;
            let p = &img.rgba[i..i + 4];
            if p[3] >= 250 && (0..3).all(|k| p[k].abs_diff(c[k]) <= 2) {
                n += 1;
            }
        }
    }
    n
}

fn scenario(hud: HudChoice, layout: HudLayout, tag: &str) {
    let m = layout.marks;
    assert!(
        m.mate != m.enemy && m.mate != m.dead && m.enemy != m.dead,
        "the three colours differ ({:?} {:?} {:?})",
        m.mate,
        m.enemy,
        m.dead
    );
    // the enemy under the cross, the mate to the left (both named)
    let mate = body(10, 1, 1, MATE, 320.0, -160.0);
    let foe = body(11, 2, 2, ENEMY, 260.0, 0.0);
    let remotes = vec![mate.clone(), foe.clone()];
    let (mb, fb) = (tag_box(&mate), tag_box(&foe));

    // 1. alive: the mate's name in the team-mate colour, the enemy's in the enemy colour, nothing in the others
    let Some(a) = run(&hud, remotes.clone(), setup(), &format!("{tag}_alive")) else {
        return;
    };
    let alive = (
        pixels_of(&a, mb, m.mate),
        pixels_of(&a, mb, m.enemy) + pixels_of(&a, mb, m.dead),
        pixels_of(&a, fb, m.enemy),
        pixels_of(&a, fb, m.mate) + pixels_of(&a, fb, m.dead),
    );
    println!("{tag} alive (mate in mate colour, mate in others, enemy in enemy colour, enemy in others): {alive:?}");
    assert!(
        alive.0 > 20,
        "the mate's tag is in the team-mate colour: {alive:?}"
    );
    assert_eq!(alive.1, 0, "no enemy / dead colour on the mate: {alive:?}");
    assert!(
        alive.2 > 20,
        "the enemy's tag is in the enemy colour: {alive:?}"
    );
    assert_eq!(alive.3, 0, "no mate / dead colour on the enemy: {alive:?}");

    // 2. both killed: the dead colour on both
    let mut dead = setup();
    dead.push((2, HudEvent::Kill(kill(1, 2))));
    dead.push((2, HudEvent::Kill(kill(2, 0))));
    let d = run(&hud, remotes.clone(), dead.clone(), &format!("{tag}_dead")).unwrap();
    let dn = (pixels_of(&d, mb, m.dead), pixels_of(&d, fb, m.dead));
    println!("{tag} dead (mate, enemy in the dead colour): {dn:?}");
    assert!(
        dn.0 > 20 && dn.1 > 20,
        "both names in the dead colour: {dn:?}"
    );
    assert_eq!(pixels_of(&d, mb, m.mate) + pixels_of(&d, fb, m.enemy), 0);

    // 3. both respawn (the records with the dead flag clear): back to the team-mate / enemy colour
    let mut again = dead;
    again.push((3, HudEvent::Roster(rec(1, MATE, 1, false))));
    again.push((3, HudEvent::Roster(rec(2, ENEMY, 2, false))));
    let r = run(&hud, remotes, again, &format!("{tag}_respawned")).unwrap();
    let rn = (
        pixels_of(&r, mb, m.mate),
        pixels_of(&r, fb, m.enemy),
        pixels_of(&r, mb, m.dead) + pixels_of(&r, fb, m.dead),
    );
    println!("{tag} respawned (mate colour, enemy colour, dead colour): {rn:?}");
    assert!(rn.0 > 20 && rn.1 > 20, "the colours come back: {rn:?}");
    assert_eq!(rn.2, 0, "no name stays in the dead colour: {rn:?}");
}

#[test]
fn fallback_names_take_the_mate_enemy_and_dead_colours() {
    scenario(HudChoice::Fallback, HudLayout::fallback(), "fallback");
}

#[test]
fn rom_names_take_the_mate_enemy_and_dead_colours() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let layout = dc_client::hud::Hud::from_rom(&rom, W, H, None)
        .expect("ROM HUD")
        .layout;
    scenario(HudChoice::Rom(rom), layout, "rom");
}

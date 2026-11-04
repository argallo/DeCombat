//! Names over the players' heads and the team-mate panel of the aim cross, through the headless frame loop.
//!
//! A synthetic world with a team mate straight ahead (under the aim cross) and an enemy beside him, both drawn as
//! boxes: the HUD shows exactly one name, at the team mate's head point, and the team-mate panel beside the cross;
//! with the enemy under the cross instead, the enemy's name appears too and the panel goes. GPU-gated (skipped
//! without an adapter); the no-ROM run always uses our fallback layout, the ROM run (`CA_ROM`) the 2009 layout. No
//! ROM value is in this file: regions come from the loaded layout and the projected points, assertions are pixel
//! counts. PNGs: `<render out>/dc-client/nametags_*.png` (frame) and `nametags_hud_*.png` (HUD layer).
// EVIDENCE: cshell+0x8b2e0 (mates shown, enemies only as the aim target), cshell+0x853f0 / cshell+0x826c0 (mate panel)

use dc_botclient::shell::RosterRecord;
use dc_client::hud::marks::{mark_point, MARK_BELOW_TOP};
use dc_client::hud::state::HudEvent;
use dc_client::hud::{HudChoice, HudLayout};
use dc_client::overlay::text::RgbaImage;
use dc_client::world::{FrameState, RemoteBody, SynthSource};
use dc_client::{LocalPlayer, ViewAngles, ViewSettings, WorldSource};
use std::path::PathBuf;

/// Player names in the two glyphs of the made-up test font (`testkit::synth_font`: 'A' and 'B'), so the captions
/// draw pixels with the no-ROM font too.
const MATE: &str = "ABBA";
const ENEMY: &str = "BABA";

const W: u32 = 1024;
const H: u32 = 768;

fn out_dir() -> PathBuf {
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    out
}

fn save_hud(img: &RgbaImage, name: &str) -> PathBuf {
    let p = out_dir().join(format!("nametags_hud_{name}.png"));
    if let Some(b) = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()) {
        let _ = b.save(&p);
    }
    p
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

/// The synthetic world with the roster events on the first frame and `remotes` on every frame.
struct MarkSource {
    inner: SynthSource,
    frame: u32,
    remotes: Vec<RemoteBody>,
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
        if self.frame == 1 {
            fs.hud_events = vec![
                (
                    t,
                    HudEvent::Own {
                        pid: Some(0),
                        mode_class: Some(1),
                    },
                ),
                (t, HudEvent::Roster(rec(0, "casey", 1))),
                (t, HudEvent::Roster(rec(1, MATE, 1))),
                (t, HudEvent::Roster(rec(2, ENEMY, 2))),
                (t, HudEvent::Life(1)),
            ];
        }
        fs
    }
}

struct Run {
    hud: RgbaImage,
    frame_png: Option<PathBuf>,
}

fn run(hud: &HudChoice, remotes: Vec<RemoteBody>, name: &str) -> Option<Run> {
    let dir = out_dir();
    let opts = dc_client::HeadlessOptions {
        frames: 3,
        width: W,
        height: H,
        out: Some(dir.clone()),
        prefix: format!("nametags_{name}_"),
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
    };
    let r = dc_client::run_headless(&mut src, None, &opts).expect("headless run");
    if r.adapter.is_none() {
        println!("skipped: no adapter ({:?})", r.render_skipped);
        return None;
    }
    let hud = r.last_hud.expect("a HUD image");
    // keep one stable frame name per scenario
    let frame_png = r.images.last().map(|p| {
        let to = dir.join(format!("nametags_{name}.png"));
        let _ = std::fs::rename(p, &to);
        to
    });
    save_hud(&hud, name);
    Some(Run { hud, frame_png })
}

/// Drawn (non-transparent) HUD pixels inside `r` (x, y, w, h in target pixels).
fn drawn(img: &RgbaImage, r: [f32; 4]) -> usize {
    let mut n = 0;
    let (x0, y0) = (r[0].max(0.0) as u32, r[1].max(0.0) as u32);
    let (x1, y1) = (
        ((r[0] + r[2]) as u32).min(img.width),
        ((r[1] + r[3]) as u32).min(img.height),
    );
    for y in y0..y1 {
        for x in x0..x1 {
            if img.rgba[((y * img.width + x) * 4 + 3) as usize] > 40 {
                n += 1;
            }
        }
    }
    n
}

/// The target-pixel box around a remote's name point (the caption is centred on it).
fn tag_box(r: &RemoteBody) -> [f32; 4] {
    let (eye, view) = viewpoint();
    let camera = ViewSettings::default().camera(eye, &view, W, H);
    let p = dc_client::hud::project(&camera, mark_point(r)).expect("in front");
    let (sx, sy) = (W as f32 / 1024.0, H as f32 / 768.0);
    [p[0] * sx - 70.0, p[1] * sy - 14.0, 140.0, 28.0]
}

fn panel_box(layout: &HudLayout) -> [f32; 4] {
    let mi = layout.mate_info.as_ref().expect("a mate panel layout");
    let r = match (&mi.panel, mi.fill) {
        (Some(p), _) => [p.pos[0], p.pos[1], p.size[0], p.size[1]],
        (None, Some((r, _))) => r,
        (None, None) => panic!("the panel draws something"),
    };
    let (sx, sy) = (W as f32 / 1024.0, H as f32 / 768.0);
    [
        r[0] as f32 * sx,
        r[1] as f32 * sy,
        r[2] as f32 * sx,
        r[3] as f32 * sy,
    ]
}

fn scenario(hud: HudChoice, layout: HudLayout, tag: &str) {
    // the mate straight ahead under the cross, the enemy ahead to the right (in view, not aimed at)
    let mate = body(10, 1, 1, MATE, 260.0, 0.0);
    let foe = body(11, 2, 2, ENEMY, 320.0, 160.0);
    let Some(a) = run(
        &hud,
        vec![mate.clone(), foe.clone()],
        &format!("{tag}_mate_aimed"),
    ) else {
        return;
    };
    let empty = run(&hud, Vec::new(), &format!("{tag}_nobody")).unwrap();
    // the enemy under the cross instead, the mate to the left
    let mate2 = body(10, 1, 1, MATE, 320.0, -160.0);
    let foe2 = body(11, 2, 2, ENEMY, 260.0, 0.0);
    let c = run(
        &hud,
        vec![mate2.clone(), foe2.clone()],
        &format!("{tag}_enemy_aimed"),
    )
    .unwrap();
    let (mt, ft, pb) = (tag_box(&mate), tag_box(&foe), panel_box(&layout));
    let (a_mate, a_foe, a_panel) = (drawn(&a.hud, mt), drawn(&a.hud, ft), drawn(&a.hud, pb));
    let (e_mate, e_foe, e_panel) = (
        drawn(&empty.hud, mt),
        drawn(&empty.hud, ft),
        drawn(&empty.hud, pb),
    );
    let (c_mate, c_foe, c_panel) = (
        drawn(&c.hud, tag_box(&mate2)),
        drawn(&c.hud, tag_box(&foe2)),
        drawn(&c.hud, pb),
    );
    println!(
        "{tag}: mate aimed: tag at mate {a_mate}, at enemy {a_foe}, panel {a_panel}; nobody: {e_mate} / {e_foe} / \
         {e_panel}; enemy aimed: mate {c_mate}, enemy {c_foe}, panel {c_panel}; frames {:?} {:?}",
        a.frame_png, c.frame_png
    );
    assert!(
        a_mate > e_mate + 20,
        "the team mate's name is drawn: {a_mate} vs {e_mate}"
    );
    assert!(
        a_foe <= e_foe,
        "no name at the enemy who is not under the cross: {a_foe} vs {e_foe}"
    );
    assert!(
        a_panel > e_panel + 10,
        "the mate panel beside the cross: {a_panel} vs {e_panel}"
    );
    assert!(c_foe > e_foe + 20, "the aimed enemy's name: {c_foe}");
    assert!(c_mate > e_mate + 20, "the mate's name stays: {c_mate}");
    assert!(c_panel <= e_panel, "no mate panel over an enemy: {c_panel}");
    const { assert!(MARK_BELOW_TOP > 0.0) };
}

#[test]
fn fallback_hud_names_the_mate_only_and_shows_the_mate_panel() {
    scenario(HudChoice::Fallback, HudLayout::fallback(), "fallback");
}

#[test]
fn rom_hud_names_the_mate_only_and_shows_the_mate_panel() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let layout = dc_client::hud::Hud::from_rom(&rom, W, H, None)
        .expect("ROM HUD")
        .layout;
    let m = &layout.marks;
    println!(
        "ROM marks: font {} size {:?}; mate panel image {}",
        m.style.font,
        m.style.size,
        layout.mate_info.as_ref().is_some_and(|i| i.panel.is_some())
    );
    scenario(HudChoice::Rom(rom), layout, "rom");
}

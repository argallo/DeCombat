//! client-tags2: the name over a team mate's head stands on the posed head, through the headless frame loop.
//!
//! The 2009 marker projects the world position of the character model's name socket (an attach point on the head
//! node, in the pose of the frame) and draws the caption centred across with the bottom of its line on that point
//! (`hud::marks::NAME_SOCKET`, `hud::items`). This test drives the headless loop on a ROM world with a team mate
//! posed by our game server's own clip choice (`dc_dserver::game::AnimSet::trackers_posed`) on the session clock:
//! standing, running (two phases of the run clip, seen from the side so the run lean shows across the screen) and
//! crouched. Per scenario one run with the HUD: the caption's pixels come from the HUD image, the character's from the
//! frame against a run without anybody (the HUD's own pixels left out). Checked: the caption's centre lies over the head (the mean x of
//! the silhouette's top rows) within a few pixels, and the caption's bottom stands just above the head's top (the
//! socket floats a few units over the head). The box point the names used before (CLI-115) is projected and printed
//! for comparison.
//!
//! Skipped without `CA_ROM` or a GPU adapter. No ROM value is in this file: clips are chosen by the server crate's
//! name keys, the socket name is read from the user's CShell.dll, the measurements are pixel counts. PNGs (ROM-derived,
//! local, never committed): `<render out>/dc-client/tags2_*.png`.
// EVIDENCE: cshell+0x89310 (caption point = the posed name socket), cshell+0x2ee7c0 / +0x2ee810 (flags 0x24: centred
// across, bottom on the point)
// DC-DEFAULT: CLI-370, CLI-371 (proposed)

use dc_botclient::shell::RosterRecord;
use dc_client::hud::marks::mark_point;
use dc_client::hud::state::HudEvent;
use dc_client::hud::HudChoice;
use dc_client::overlay::text::RgbaImage;
use dc_client::world::{FrameState, GpuTarget, LoadedWorld};
use dc_client::{
    ClientError, InputSource, LocalPlayer, RemoteBody, RemoteLook, RemoteTracker, RomWorld,
    ViewAngles, ViewSettings, WorldSource,
};
use dc_dserver::anims::Pose;
use dc_dserver::game::{AnimSet, Dir, Gait, Motion};
use dc_sim::{drop_to_floor, Commands, Cylinder, MovementParams, V3};
use std::path::{Path, PathBuf};

const W: u32 = 1024;
const H: u32 = 768;
/// The map of the pictures (an indoor-outdoor test map of the user's ROM, as the other ROM HUD tests use).
const MAP: &str = "JUNK_FLEA";
/// Frames of 1/60 s on the session clock.
const FRAME_MS: f32 = 1000.0 / 60.0;
/// The team mate's name (the made-up test font draws 'A' and 'B').
const MATE: &str = "ABBA";
/// How far ahead of the eye and how far to the right the team mate stands (world units).
const AHEAD: f32 = 230.0;
const SIDE: f32 = 70.0;

fn out_dir() -> PathBuf {
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    out
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

/// The viewer stands still at the spawn and keeps its facing (the default script would walk him forward).
struct Still;

impl InputSource for Still {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, _tick: u64, _hz: u32, _view: &mut ViewAngles) -> Commands {
        Commands::default()
    }
    fn label(&self) -> &'static str {
        "still"
    }
}

/// One scenario: the motion the server would stream, the facing relative to the viewer, the frames to run.
#[derive(Clone, Copy)]
struct Scene {
    tag: &'static str,
    motion: Motion,
    /// Added to the facing toward the viewer (radians).
    turn: f32,
    frames: u32,
}

/// The ROM world with the roster and the posed team mate on every frame (trackers from the session clock).
struct PosedSource {
    inner: RomWorld,
    frame: u32,
    /// The session clock and trackers of the last frame handed out.
    last: (u32, Vec<RemoteTracker>),
    mate: Option<RemoteBody>,
    set: AnimSet,
    motion: Motion,
}

impl PosedSource {
    fn trackers(&self, clock: u32) -> Vec<RemoteTracker> {
        let pose = Pose::default();
        self.set
            .trackers_posed(self.motion, &pose, clock, clock, clock, true)
            .iter()
            .map(|t| RemoteTracker {
                clip: usize::from(t.anim),
                time_ms: t.time_ms as f32,
                blend_set: t.secondary.map(|(_, w)| usize::from(w)),
            })
            .collect()
    }
}

impl WorldSource for PosedSource {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        self.inner.load(gpu)
    }
    fn rom(&self) -> Option<PathBuf> {
        self.inner.rom()
    }
    fn frame_state(&mut self) -> FrameState {
        self.frame += 1;
        let clock = (self.frame as f32 * FRAME_MS) as u32;
        let trackers = self.trackers(clock);
        self.last = (clock, trackers.clone());
        let remotes = self
            .mate
            .iter()
            .cloned()
            .map(|mut r| {
                if let Some(l) = r.look.as_mut() {
                    l.trackers = trackers.clone();
                }
                r
            })
            .collect();
        let mut fs = FrameState {
            life_state: Some(1),
            clock_ms: Some(clock),
            own_team: Some(1),
            remotes,
            ..FrameState::default()
        };
        if self.frame == 1 {
            fs.hud_events = vec![
                (
                    clock,
                    HudEvent::Own {
                        pid: Some(0),
                        mode_class: Some(1),
                    },
                ),
                (clock, HudEvent::Roster(rec(0, "BA", 1))),
                (clock, HudEvent::Roster(rec(1, MATE, 1))),
                (clock, HudEvent::Life(1)),
            ];
        }
        fs
    }
}

/// The first spawn from which the team mate's place ([`AHEAD`] along the view, [`SIDE`] to the right) can be reached
/// by a standing player's cylinder without meeting the world (so nothing stands between the viewer and him): the
/// spawn index, the headless player's eye and view there, the floor-standing box centre of the mate and the box's
/// half extents.
fn placement(rom: &Path) -> Option<(usize, V3, ViewAngles, V3, V3, V3)> {
    use dc_sim::Collider;
    let mut src = RomWorld::new(rom, MAP);
    let world = src.load(None).ok()?;
    let params = MovementParams::default();
    let cyl = Cylinder::from_dims(world.stand_dims);
    (0..world.spawns.len()).find_map(|k| {
        let spawn = world.spawn(k).ok()?;
        let me = LocalPlayer::spawn(params, &world.collision, &spawn, world.stand_dims, 60);
        let view = ViewAngles {
            yaw: spawn.yaw,
            pitch: 0.0,
        };
        let fwd = V3::new(spawn.yaw.sin(), 0.0, spawn.yaw.cos());
        let right = V3::new(spawn.yaw.cos(), 0.0, -spawn.yaw.sin());
        let to = me.state.pos + fwd * AHEAD + right * SIDE;
        if world.collision.sweep(&cyl, me.state.pos, to).is_some() {
            return None;
        }
        let centre = drop_to_floor(&world.collision, &cyl, to, params.step(), 1000.0);
        // on about the same floor (no drop into a pit or onto a lower level)
        if (centre.y - me.state.pos.y).abs() > 24.0 {
            return None;
        }
        Some((
            k,
            me.eye_target(&view),
            view,
            centre,
            world.stand_dims,
            me.state.pos,
        ))
    })
}

struct Shot {
    frame: image::RgbaImage,
    hud: Option<RgbaImage>,
    /// The session clock and trackers of the last frame.
    last: (u32, Vec<RemoteTracker>),
    /// The headless player's position at the start and at the end.
    moved: (V3, V3),
}

fn run(
    rom: &Path,
    spawn: usize,
    sc: Scene,
    mate: Option<RemoteBody>,
    hud: bool,
    name: &str,
) -> Option<Shot> {
    let dir = out_dir();
    let body = dc_client::characters::ModelScene::open(rom)
        .ok()?
        .default_look()
        .first()?
        .model
        .clone();
    let vfs = dc_rez::RezVfs::new(rom);
    let rig = dc_dserver::chars::RigCache::default()
        .rig(&vfs, &body)
        .ok()?;
    let opts = dc_client::HeadlessOptions {
        frames: sc.frames,
        frame_us: 16_667,
        width: W,
        height: H,
        out: Some(dir.clone()),
        prefix: format!("tags2_{name}_"),
        save_every: 100_000,
        font_bytes: Some(dc_client::testkit::synth_font()),
        hud: if hud {
            HudChoice::Fallback
        } else {
            HudChoice::Off
        },
        debug_panel: false,
        debug_boxes: false,
        models: true,
        spawn,
        ..dc_client::HeadlessOptions::default()
    };
    let mut src = PosedSource {
        inner: RomWorld::new(rom, MAP),
        frame: 0,
        last: (0, Vec::new()),
        mate,
        set: AnimSet::from_rig(&rig),
        motion: sc.motion,
    };
    let r = dc_client::run_headless(&mut src, Some(&mut Still), &opts).expect("headless run");
    if r.adapter.is_none() {
        println!("skipped: no adapter ({:?})", r.render_skipped);
        return None;
    }
    let png = r.images.last()?.clone();
    let to = dir.join(format!("tags2_{name}.png"));
    let _ = std::fs::rename(&png, &to);
    let frame = image::open(&to).ok()?.to_rgba8();
    if let Some(h) = &r.last_hud {
        if let Some(b) = image::RgbaImage::from_raw(h.width, h.height, h.rgba.clone()) {
            let _ = b.save(dir.join(format!("tags2_hud_{name}.png")));
        }
    }
    Some(Shot {
        frame,
        hud: r.last_hud,
        last: src.last,
        moved: (r.start, r.end),
    })
}

/// The character's pixels: where the frame differs from the frame without anybody, inside the `window`
/// (x0, y0, x1, y1), leaving out every pixel the HUD image draws on.
fn silhouette(
    a: &image::RgbaImage,
    empty: &image::RgbaImage,
    hud: &RgbaImage,
    win: [u32; 4],
) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for y in win[1]..win[3].min(a.height()) {
        for x in win[0]..win[2].min(a.width()) {
            if hud.rgba[((y * hud.width + x) * 4 + 3) as usize] > 0 {
                continue;
            }
            let (p, q) = (a.get_pixel(x, y).0, empty.get_pixel(x, y).0);
            let d: i32 = (0..3).map(|k| (p[k] as i32 - q[k] as i32).abs()).sum();
            if d > 30 {
                v.push((x, y));
            }
        }
    }
    v
}

/// The head: the silhouette's top row and the mean x of its top rows (the top 8 % of its height, at least 6 rows).
fn head(px: &[(u32, u32)]) -> Option<(f32, u32, u32)> {
    let top = px.iter().map(|p| p.1).min()?;
    let bottom = px.iter().map(|p| p.1).max()?;
    let rows = ((bottom - top) as f32 * 0.08).max(6.0) as u32;
    let near: Vec<_> = px.iter().filter(|p| p.1 < top + rows).collect();
    let x = near.iter().map(|p| p.0 as f32).sum::<f32>() / near.len() as f32;
    Some((x, top, bottom))
}

/// The caption's box in the HUD image inside the window (the aim cross about the screen centre left out):
/// (min x, max x, min y, max y).
fn caption(img: &RgbaImage, win: [u32; 4]) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    let (cx, cy) = (img.width as i32 / 2, img.height as i32 / 2);
    for y in win[1]..win[3].min(img.height) {
        for x in win[0]..win[2].min(img.width) {
            if (x as i32 - cx).abs() <= 24 && (y as i32 - cy).abs() <= 24 {
                continue;
            }
            if img.rgba[((y * img.width + x) * 4 + 3) as usize] > 40 {
                b = Some(match b {
                    None => (x, x, y, y),
                    Some((x0, x1, y0, y1)) => (x0.min(x), x1.max(x), y0.min(y), y1.max(y)),
                });
            }
        }
    }
    b
}

/// The world point of the name socket of the default body posed by `trackers`, placed as dc-client places a
/// character (yaw about +Y at the box bottom), computed here with dc-render's pose of the model file (not through the
/// model scene under test).
fn posed_socket(rom: &Path, sock: &str, r: &RemoteBody, trackers: &[RemoteTracker]) -> [f32; 3] {
    let body = dc_client::characters::ModelScene::open(rom)
        .expect("model scene")
        .default_look()[0]
        .model
        .clone();
    let files = dc_render::ModelFiles::load(&dc_rez::RezVfs::new(rom), &body).expect("body model");
    let set = files.anim_set();
    let tr: Vec<dc_render::Tracker> = trackers
        .iter()
        .map(|t| dc_render::Tracker {
            clip: t.clip,
            time_ms: t.time_ms,
            blend_set: t.blend_set,
        })
        .collect();
    let globals = set.skeleton.globals(&set.pose(&tr));
    let (node, at) =
        dc_client::characters::name_socket_of(&files.body, sock).expect("the body has the socket");
    let world = glam::Mat4::from_rotation_translation(
        glam::Quat::from_rotation_y(r.yaw.unwrap_or(0.0)),
        dc_client::characters::model_origin(r.pos, r.half),
    );
    world
        .transform_point3(globals[node].transform_point3(at))
        .to_array()
}

#[test]
fn rom_name_over_a_mate_stands_on_the_posed_head_standing_running_crouched() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(sock) = dc_client::hud::marks::name_socket_from_rom(&rom) else {
        panic!("the name socket literal of the user's CShell.dll does not match its pin");
    };
    assert!(!sock.is_empty());
    let (spawn, eye, view, centre, dims, me) = placement(&rom).expect("a spawn with a free view");
    println!("spawn {spawn}, player at {me:?}, eye {eye:?}, mate at {centre:?}");
    let face_viewer = view.yaw + std::f32::consts::PI;
    let quarter = std::f32::consts::FRAC_PI_2;
    let scenes = [
        Scene {
            tag: "stand",
            motion: Motion::Stand,
            turn: 0.0,
            frames: 20,
        },
        Scene {
            tag: "run_a",
            motion: Motion::Step(Gait::Run, Dir::Forward),
            turn: quarter,
            frames: 20,
        },
        Scene {
            tag: "run_b",
            motion: Motion::Step(Gait::Run, Dir::Forward),
            turn: quarter,
            frames: 31,
        },
        Scene {
            tag: "crouch",
            motion: Motion::Crouch,
            turn: 0.0,
            frames: 20,
        },
    ];
    let mate = |turn: f32| RemoteBody {
        obj: 40,
        pid: 1,
        name: MATE.into(),
        team: 1,
        pos: centre.to_array(),
        half: dims.to_array(),
        yaw: Some(face_viewer + turn),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: Vec::new(),
            weapon: None,
            trackers: Vec::new(),
            visible: true,
            aim: None,
        }),
    };
    let Some(empty) = run(&rom, spawn, scenes[0], None, false, "empty") else {
        return;
    };
    // the box point of CLI-115 (the rule before this change), projected with the loop's camera at the spawn
    let camera = ViewSettings::default().camera(eye, &view, W, H);
    let old = dc_client::hud::project(&camera, mark_point(&mate(0.0))).expect("in front");
    let (sx, sy) = (W as f32 / 1024.0, H as f32 / 768.0);
    let mut worst_x = 0.0f32;
    for sc in scenes {
        let tagged = run(&rom, spawn, sc, Some(mate(sc.turn)), true, sc.tag).expect("HUD run");
        let win = [
            (old[0] * sx - 220.0).max(0.0) as u32,
            (old[1] * sy - 200.0).max(0.0) as u32,
            (old[0] * sx + 220.0) as u32,
            (old[1] * sy + 420.0) as u32,
        ];
        let hud = tagged.hud.as_ref().expect("a HUD image");
        let px = silhouette(&tagged.frame, &empty.frame, hud, win);
        assert!(
            px.len() > 1500,
            "{}: the character is drawn ({} pixels)",
            sc.tag,
            px.len()
        );
        let (hx, top, bottom) = head(&px).unwrap();
        let cwin = [
            (hx - 120.0).max(0.0) as u32,
            top.saturating_sub(90),
            (hx + 120.0) as u32,
            top + 25,
        ];
        let (x0, x1, y0, y1) =
            caption(hud, cwin).unwrap_or_else(|| panic!("{}: a caption over the head", sc.tag));
        let cx = (x0 + x1 + 1) as f32 / 2.0;
        let dx = cx - hx;
        let old_dx = old[0] * sx - hx;
        println!(
            "{}: head x {hx:.1}, head top {top}, feet {bottom}; caption x {x0}..{x1} (centre {cx:.1}, off by \
             {dx:+.1}), rows {y0}..{y1} (bottom {:+} from the head top); the old box point: x off by {old_dx:+.1}, \
             y {:.1} ({:+.1} from the head top)",
            sc.tag,
            y1 as i32 - top as i32,
            old[1] * sy,
            old[1] * sy - top as f32,
        );
        worst_x = worst_x.max(dx.abs());
        // the 2009 rule exactly: centred across on the projected posed socket, the line's bottom on it
        let at = posed_socket(&rom, &sock, &mate(sc.turn), &tagged.last.1);
        // the loop's camera: the eye moved with the headless player (it settles on the floor after the spawn)
        let (from, to) = tagged.moved;
        println!("{}: headless player {from:?} -> {to:?}", sc.tag);
        let loop_eye = eye + (to - me);
        let loop_camera = ViewSettings::default().camera(loop_eye, &view, W, H);
        let p = dc_client::hud::project(&loop_camera, at).expect("socket in front");
        let (px_x, px_y) = (p[0] * sx, p[1] * sy);
        println!(
            "{}: clock {} ms; projected socket ({px_x:.1}, {px_y:.1}): caption centre off by {:+.1}, bottom off by {:+.1}",
            sc.tag,
            tagged.last.0,
            cx - px_x,
            (y1 + 1) as f32 - px_y
        );
        assert!(
            (cx - px_x).abs() <= 2.5,
            "{}: the caption is centred on the projected socket",
            sc.tag
        );
        // the line's bottom is on the point: the glyphs (no descender in the test names) end on the baseline, the
        // font's descent above it
        let below = (y1 + 1) as f32 - px_y;
        assert!(
            (-9.0..=1.0).contains(&below),
            "{}: the caption's line ends on the projected socket (glyph bottom {below:+.1})",
            sc.tag
        );
        // and that point is over the head: within the head's half width (about 15 pixels here) of the head's top
        // rows' mean x
        assert!(
            dx.abs() <= 10.0,
            "{}: the caption is over the head ({dx:+.1} px)",
            sc.tag
        );
        // the socket floats a few units above the head (about 6 on the default body): the caption's bottom is just
        // above the head's top, never on the face
        let gap = top as i32 - y1 as i32;
        assert!(
            (1..=32).contains(&gap),
            "{}: the caption stands just above the head (bottom {y1}, head top {top})",
            sc.tag
        );
    }
    println!("worst horizontal offset of the caption from the head: {worst_x:.1} px");
}

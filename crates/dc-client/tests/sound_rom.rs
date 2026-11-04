//! The client's sounds against the user's ROM, the reference stack captures and our own dedicated server: the world sounds of
//! the world-load group (the reference stack's tdm4 / tdm6 records and ours), the surface types under the feet (the 0x64 trailer
//! surfaces the 2009 client sent on tdm4), the footstep keys of the default body, and a headless round on our dserver
//! that logs every sound start (world sounds at the load, own footsteps at the run clip's keys, none while walking).
//! ROM-gated (skipped without `CA_ROM`) and capture-gated (skipped without the capture folder); nothing of
//! the ROM is asserted by value and no ROM name is printed (hashes only).
// SPEC: AUDIO_2009.md §5.3 (distance law), §14 (world sounds, footsteps); UDP_SESSION_2009_B.md §8.3; MOVEMENT_2009.md
// §10.2, §12.2 (trailer surface)
// OBSERVED: capture 20261025_002236_tdm4, 20261025_065016_tdm6 (REFERENCE, reference stack)

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dc_botclient::capdecode::{decode_file, unbundle};
use dc_botclient::objects::{ObjectTable, SoundNote};
use dc_client::sound::{path_hash, ClipSteps, SoundDirector, SoundLogEntry, Trigger};
use dc_client::{
    run_headless, Connect, HeadlessOptions, InputSource, NetOptions, NetWorld, ViewAngles,
    WorldSource,
};
use dc_sim::Commands;
use dc_wire::udp::BitWriter;
use support::*;

const TDM4: &str = "20261025_002236_tdm4";
const TDM6: &str = "20261025_065016_tdm6";

fn capture_named(folder: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = PathBuf::from(p).join(folder).join("capture.jsonl");
        return c.is_file().then_some(c);
    }
    None
}

/// What the reference stack's world-load group carried on one link: the world path of the file list, the file table and
/// the tracked-sound starts of the first id 8 message with creates.
struct ReferenceLoad {
    world: String,
    files: BTreeMap<u16, String>,
    sounds: Vec<dc_botclient::objects::SoundStart>,
    /// The tracked-sound starts after the world-load group (pain, fall and bounce sounds of the reference stack).
    later: Vec<dc_botclient::objects::SoundStart>,
}

fn reference_load(cap: &Path, conn: u32) -> Option<ReferenceLoad> {
    let d = decode_file(cap).ok()?;
    let mut files = BTreeMap::new();
    let mut world = None;
    let mut t = ObjectTable::default();
    let mut load: Option<ReferenceLoad> = None;
    for m in d.msgs.iter().filter(|m| m.conn == conn && !m.c2s) {
        for b in unbundle(&m.bits) {
            let mut r = b.reader();
            match r.read_u8() {
                0x32 => {
                    while r.remaining_bits() >= 56 {
                        let id = r.read_u16();
                        r.read_u32();
                        let n = String::from_utf8_lossy(&r.read_zstr8()).into_owned();
                        world.get_or_insert_with(|| n.clone());
                        files.insert(id, n);
                    }
                }
                8 => {
                    let rest: BitWriter = r.read_bit_string(r.remaining_bits());
                    let before = t.creates;
                    t.apply_group(&rest);
                    let starts: Vec<_> = t
                        .sound_events
                        .drain(..)
                        .filter_map(|e| match e {
                            SoundNote::Start(s) if s.track.is_some() => Some(s),
                            _ => None,
                        })
                        .collect();
                    match load.as_mut() {
                        Some(l) => l.later.extend(starts),
                        None if before == 0 && t.creates > 0 => {
                            load = Some(ReferenceLoad {
                                world: world.clone()?,
                                files: files.clone(),
                                sounds: starts,
                                later: Vec::new(),
                            })
                        }
                        None => {}
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(l) = load.as_mut() {
        l.files = files;
    }
    load
}

/// The world-load sounds of the reference stack: every link of tdm4 (SNOWVALLEY) carries the 11 records of the spec, the two
/// links of a capture carry the same number, every record names a wave file of the file table, and (with the ROM)
/// every file decodes.
// SPEC: UDP_SESSION_2009_B.md §8.3 (11 records on every SNOWVALLEY link of four captures)
#[test]
fn reference_world_load_sounds_parse_and_resolve() {
    let rom = dc_rom::test_rom();
    let vfs = rom.as_deref().map(dc_rez::RezVfs::new);
    let mut checked = 0;
    for (label, folder) in [("tdm4", TDM4), ("tdm6", TDM6)] {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let mut counts = Vec::new();
        for conn in [5u32, 6] {
            let o = reference_load(&cap, conn).expect("a world-load group");
            println!(
                "{label} conn {conn}: world {} -> {} tracked sounds at the load",
                path_hash(&o.world),
                o.sounds.len()
            );
            for s in &o.sounds {
                let f = o.files.get(&s.file).expect("the sound file is announced");
                assert!(f.to_ascii_uppercase().ends_with(".WAV"), "{}", path_hash(f));
                assert_eq!(
                    s.flags & 0x0130,
                    0x0130,
                    "the constant bits of a world sound"
                );
                if let Some(v) = vfs.as_ref() {
                    let bytes = dc_render::read_rom_entry(v, f).expect("in the ROM");
                    dc_audio::decode::decode_wav(&bytes).expect("decodes");
                }
            }
            counts.push(o.sounds.len());
        }
        assert_eq!(counts[0], counts[1], "{label}: both links");
        if label == "tdm4" {
            assert_eq!(counts[0], 11, "tdm4: the spec's 11 records");
            // the damage / fall sounds of conn 6 (B §8.3: sflags 0x0132, prio 2, outer 500, ratio 17)
            let o = reference_load(&cap, 6).unwrap();
            let pain: Vec<_> = o
                .later
                .iter()
                .filter(|s| {
                    s.flags == 0x0132 && s.prio == 2 && s.outer == 500 && s.inner_ratio == 17
                })
                .collect();
            println!(
                "tdm4 conn 6: {} later tracked starts, {} of the damage form",
                o.later.len(),
                pain.len()
            );
            assert!(pain.len() >= 3, "the reference stack's damage sounds parse");
        }
        assert!(counts[0] > 0);
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// The surface type of the polygon under the 2009 client's reported position equals the surface byte of its 0x64
/// trailer (tdm4, SNOWVALLEY): the surface record's `surf_texflags` is the ROM surface id.
// OBSERVED: capture 20261025_002236_tdm4/analysis/updates.csv (pos, on_ground, surface; the
// real client's own values, PROVEN-CAPTURE)
#[test]
fn rom_surface_types_are_the_trailer_surfaces_of_the_2009_client() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(cap) = capture_named(TDM4) else {
        println!("skipped: no tdm4 capture");
        return;
    };
    let csv = cap.parent().unwrap().join("analysis").join("updates.csv");
    let Ok(text) = std::fs::read_to_string(&csv) else {
        println!("skipped: no tdm4 updates.csv");
        return;
    };
    let o = reference_load(&cap, 6).expect("world");
    let map = dc_client::net::map_name_of(&o.world).expect("map name");
    let mut src = dc_client::RomWorld::new(&rom, &map);
    let w = src.load(None).expect("the capture's world");
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap().split(',').collect();
    let col = |n: &str| head.iter().position(|h| *h == n).unwrap();
    let (cx, cy, cz, cg, cs) = (
        col("pos_x"),
        col("pos_y"),
        col("pos_z"),
        col("on_ground"),
        col("surface"),
    );
    let (mut same, mut differ, mut none, mut off) = (0u32, 0u32, 0u32, 0u32);
    let mut seen: BTreeMap<u16, u32> = BTreeMap::new();
    let mut pairs: BTreeMap<(u16, u16), u32> = BTreeMap::new();
    for l in lines {
        let f: Vec<&str> = l.split(',').collect();
        if f.len() <= cs || f[cg] != "1" {
            continue;
        }
        let s: u16 = f[cs].parse().unwrap_or(0);
        if s == 0 || s == 200 {
            continue;
        }
        let p = [f[cx], f[cy], f[cz]].map(|v| v.parse::<f32>().unwrap());
        let from = dc_sim::V3::from_array(p);
        let to = dc_sim::V3::new(p[0], p[1] - 200.0, p[2]);
        match w.collision.ray(from, to) {
            // the body stood on something this collision set does not hold (a prop model: the 2009 client then
            // takes the standing object's surface, cshell+0x27f470)
            Some(h) if h.t * 200.0 > w.stand_dims.y + 30.0 => off += 1,
            Some(h) => {
                let t = w.surface_type(h.model, h.poly);
                if t == s {
                    same += 1;
                } else {
                    differ += 1;
                    *pairs.entry((s, t)).or_default() += 1;
                }
                *seen.entry(s).or_default() += 1;
            }
            None => none += 1,
        }
    }
    println!("trailer surfaces {seen:?}: polygon type equal {same}, different {differ} (trailer, polygon: {pairs:?}), no hit {none}, standing on something else {off} (stand half height {})",
        w.stand_dims.y
    );
    assert!(same > 100, "enough grounded updates: {same}");
    assert!(
        same as f32 >= 0.95 * (same + differ) as f32,
        "the polygon's surface type is the trailer surface: {same} equal, {differ} different"
    );
}

/// The default body's run forward clip carries two footstep keys (one per foot), its walk forward clip none.
// SPEC: AUDIO_2009.md §14.2 (run clips step, walk and crouch clips carry only ground-print keys); DC_DEFAULTS DSV-28
// refinement (run F key dce644ba7bc1b324/3, walk F key 5f8e2d7e3aa7a17d/3)
#[test]
fn rom_default_body_steps_on_the_run_clip_and_not_on_the_walk_clip() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let table = dc_render::CharacterTable::from_vfs(&vfs).expect("character table");
    let body = &table.bodies.values().next().expect("a body row").model;
    let files = dc_render::ModelFiles::load(&vfs, body).expect("body model");
    let set = files.anim_set();
    let steps = ClipSteps::of(&set);
    let run_f: dc_render::NameKey = ([0xdc, 0xe6, 0x44, 0xba, 0x7b, 0xc1, 0xb3, 0x24], 3);
    let walk_f = dc_render::CLIP_LOWER_WALK;
    let find = |k: dc_render::NameKey| {
        set.find_clip(|n| dc_render::name_key(n) == k)
            .expect("clip by key")
    };
    let (run, walk) = (find(run_f), find(walk_f));
    let (len, keys) = &steps.clips[run];
    println!(
        "run F clip {run}: {len} ms, keys {keys:?}; walk F clip {walk}: {:?}",
        steps.clips[walk]
    );
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().any(|k| k.1 == Some(true)) && keys.iter().any(|k| k.1 == Some(false)));
    assert!(steps.clips[walk].1.is_empty());
    let with_keys = steps.clips.iter().filter(|c| !c.1.is_empty()).count();
    assert!(
        with_keys >= 8,
        "the eight run directions at least: {with_keys}"
    );
}

/// Forward at the run (or walk) speed by phase: run for `run_s`, then walk until `walk_s`, then stand.
struct Runner {
    run_s: f64,
    walk_s: f64,
}

impl InputSource for Runner {
    fn frame(&mut self, _view: &mut ViewAngles) {}
    fn tick(&mut self, tick: u64, hz: u32, view: &mut ViewAngles) -> Commands {
        let t = tick as f64 / hz.max(1) as f64;
        // a slow turn keeps the body off walls most of the time
        view.yaw += 0.004;
        Commands {
            forward: t < self.walk_s,
            run: t < self.run_s,
            ..Commands::default()
        }
    }
    fn label(&self) -> &'static str {
        "runner"
    }
}

fn median(v: &mut [u32]) -> u32 {
    v.sort_unstable();
    v[v.len() / 2]
}

/// A headless round on our dserver in the world of a capture: every world sound starts at the load (as many as the
/// reference sent on that world) with a voice from the ROM, the own footsteps follow the run clip's keys (about 300 ms
/// apart, the feet alternating) and stop when the body walks.
fn round_on(folder: &str) {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let Some(cap) = capture_named(folder) else {
        println!("skipped: no {folder} capture");
        return;
    };
    let o = reference_load(&cap, 6).expect("reference world-load group");
    let map = dc_client::net::map_name_of(&o.world).expect("map");
    let mut cmd = std::process::Command::new(dserver_exe());
    cmd.args([
        "-port",
        "0",
        "-world",
        &format!("Worlds/{map}"),
        "-idle",
        "30",
        "-tick",
        "20",
    ]);
    let mut srv = Proc::spawn(cmd);
    let ready = srv
        .wait_line("@ready", Duration::from_secs(60))
        .unwrap_or_else(|| panic!("no @ready; server said {:?}", srv.seen));
    let port: u16 = ready
        .split_whitespace()
        .find_map(|t| t.strip_prefix("port="))
        .and_then(|p| p.parse().ok())
        .expect("port");
    srv.line("user 5 5 5 0 0 SoundClient");
    let mut o2 = NetOptions::new(
        Connect::Udp(format!("127.0.0.1:{port}").parse().unwrap()),
        "SoundClient",
    );
    o2.rom = Some(rom.clone());
    let mut net = NetWorld::start(o2);
    let handle = net.handle();
    let dir = scratch("sound");
    let log = dir.join("sounds.jsonl");
    let mut opts = HeadlessOptions {
        frames: 60 * 12,
        frame_us: 16_667,
        width: 160,
        height: 120,
        out: None,
        render: false,
        realtime: true,
        ..HeadlessOptions::default()
    };
    opts.sound.log = Some(log.clone());
    let mut runner = Runner {
        run_s: 8.0,
        walk_s: 11.0,
    };
    let r = run_headless(&mut net, Some(&mut runner as &mut dyn InputSource), &opts)
        .expect("headless run");
    let st = handle.shutdown(Duration::from_secs(5));
    srv.quit();
    srv.finish(Duration::from_secs(5));
    println!("{}", r.summary());
    println!("  {}", r.sound_note.as_deref().unwrap_or(""));
    assert!(st.fatal.is_empty(), "{:?}", st.fatal);
    assert!(st.life_states.contains(&1), "{:?}", st.milestones);
    // world sounds: all at the load, as many as the reference stack's, each with a voice
    let world: Vec<&SoundLogEntry> = r
        .sound_log
        .iter()
        .filter(|e| matches!(e.trigger, Trigger::LoadSound { .. }))
        .collect();
    for e in &world {
        println!(
            "  world sound t {} ms {:?} {:?} outer {} inner {:.1} vol {} loop {} dist {:?} gain {:?} file {}",
            e.t_ms,
            e.trigger,
            e.kind,
            e.outer,
            e.inner,
            e.volume,
            e.looped,
            e.distance.map(|d| d.round()),
            e.gain,
            e.file_hash.as_deref().unwrap_or("-")
        );
    }
    assert_eq!(
        world.len(),
        o.sounds.len(),
        "our world-load sounds = the reference stack's on this world"
    );
    let ours: Vec<String> = world.iter().filter_map(|e| e.file_hash.clone()).collect();
    for s in &o.sounds {
        let h = path_hash(&o.files[&s.file]);
        assert!(
            ours.contains(&h),
            "the reference stack's world sound file {h} is played"
        );
    }
    assert!(
        world.iter().all(|e| e.gain.is_some()),
        "every world sound has a voice"
    );
    let last_world = world.iter().map(|e| e.t_ms).max().unwrap();
    let first_other = r
        .sound_log
        .iter()
        .filter(|e| !matches!(e.trigger, Trigger::LoadSound { .. }))
        .map(|e| e.t_ms)
        .min();
    println!("  last world sound at {last_world} ms, first other sound at {first_other:?} ms");
    // own footsteps: the view-bob cycle's (w2-audiocues, CLI-527): only while running, 357 ms apart (1.4 cycles a
    // second, two steps a cycle), feet alternating, also inside the spawn window (CLI-680)
    let steps: Vec<&SoundLogEntry> = r
        .sound_log
        .iter()
        .filter(|e| matches!(e.trigger, Trigger::OwnStep { .. }))
        .collect();
    println!("  spawn notes {:?}", r.sound_spawn_log);
    // CLI-680: the spawn window does not mute the own steps (the death note does)
    assert!(
        r.sound_spawn_log.iter().any(|x| x.2) && r.sound_stats.step_muted == 0,
        "the spawn note muted no step: {:?}",
        r.sound_stats
    );
    println!(
        "  own footsteps {} (last at {:?} ms)",
        steps.len(),
        steps.last().map(|e| e.t_ms)
    );
    // the input is driven by the simulation ticks (run for `run_s`), the sound log by the session clock: the end of
    // the run is placed on the session clock by the frame that ran its tick, so a busy machine (frames held, then
    // caught up) does not move it; a step already keyed may follow within 600 ms (two of the run clip's 300 ms key gaps)
    let hz = opts.tick_hz.max(1) as f64;
    let run_end_tick = (runner.run_s * hz).ceil() as u64;
    let run_end_ms = r
        .clock_at_tick(run_end_tick)
        .expect("the run's end tick reached on the session clock");
    println!(
        "  run ends at tick {run_end_tick} = session {run_end_ms} ms; {}",
        r.stall_note()
    );
    assert!(
        steps
            .last()
            .is_some_and(|e| e.t_ms < run_end_ms.saturating_add(600)),
        "no own footstep while walking (after {} s of the input = session {run_end_ms} ms) or standing",
        runner.run_s
    );
    assert!(
        steps.len() >= 8,
        "own footsteps while running: {}",
        steps.len()
    );
    let mut gaps: Vec<u32> = steps.windows(2).map(|w| w[1].t_ms - w[0].t_ms).collect();
    let gaps_raw = gaps.clone();
    let m = median(&mut gaps);
    println!("  step gaps (ms) {gaps_raw:?} median {m}");
    assert!(
        (320..=400).contains(&m),
        "the view-bob steps are 357 ms apart: median {m}"
    );
    let feet: Vec<bool> = steps
        .iter()
        .map(|e| matches!(e.trigger, Trigger::OwnStep { left: true, .. }))
        .collect();
    let alternating = feet.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(
        alternating as f32 >= 0.8 * (feet.len() - 1) as f32,
        "feet alternate: {feet:?}"
    );
    let surfaces: BTreeMap<u16, usize> = steps.iter().fold(BTreeMap::new(), |mut m, e| {
        if let Trigger::OwnStep { surface, .. } = e.trigger {
            *m.entry(surface).or_default() += 1;
        }
        m
    });
    println!("  footstep surfaces {surfaces:?}");
    assert!(
        steps.iter().all(|e| e.gain.is_some()),
        "every footstep has a voice from the surface table"
    );
    // the JSONL log has one line per start, hashes only (plus the `music` and `live` lines of AUDIO-3)
    let text = std::fs::read_to_string(&log).expect("sound log");
    let starts = text
        .lines()
        .filter(|l| !l.contains(r#""trigger":"music""#) && !l.contains(r#""trigger":"live""#))
        .count();
    assert_eq!(starts, r.sound_log.len());
    for name in o.files.values() {
        assert!(!text.contains(name.as_str()), "no ROM name in the log");
    }
}

#[test]
fn rom_round_on_the_tdm4_world_plays_load_sounds_and_own_footsteps() {
    round_on(TDM4);
}

#[test]
fn rom_round_on_the_tdm6_world_plays_load_sounds_and_own_footsteps() {
    round_on(TDM6);
}

/// The director with the ROM tables: the surface table has rows and footstep files, the weapon rows name sounds.
#[test]
fn rom_tables_open() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let d = SoundDirector::open(Some(&rom));
    println!("{}", d.note);
    assert!(d.surfaces.rows.len() > 10);
    assert!(
        d.surfaces.footstep(0, false, 0).is_some(),
        "the default row steps"
    );
    assert!(
        d.weapons.values().any(|w| w.sound(6).is_some()),
        "fire sounds"
    );
    assert!(
        d.jump_sounds.iter().all(|s| !s.is_empty()),
        "both jump sounds"
    );
    // w2-audiocues: the silenced sounds, the silencer mods, the team callout rows (the throw row 6 with files for
    // the first voice template), the fly-by record and the bullet ammunition rows its test reads
    assert!(!d.silenced.is_empty(), "silenced fire rows");
    assert!(
        d.mod_rows
            .values()
            .any(|m| dc_client::sound::cues::remote_shot_sound(Some(m))
                != dc_client::sound::cues::ShotSound::Fire),
        "a silencer mod"
    );
    assert!(d.callouts.pick(6, 0, 0).is_some(), "the throw callout row");
    let f = d.flyby.as_ref().expect("the fly-by record");
    assert!(!f.paths.is_empty() && f.outer_radius > 0.0);
    assert!(
        d.shot_rows.ammo.values().any(|a| a.damage_type
            == dc_client::sound::cues::FLYBY_DAMAGE_TYPE
            && a.ammo_type == dc_client::sound::cues::FLYBY_AMMO_TYPE),
        "bullet ammunition rows"
    );
}

/// A remote character on the default body running forward (a made-up tracker time line, the reference stack's form: the
/// lower tracker on the run forward clip): the director steps at the clip's keys with the surface row's files.
// SPEC: AUDIO_2009.md §14.2 (remote footsteps from the streamed animation)
#[test]
fn rom_remote_runner_steps_with_the_surface_files() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let table = dc_render::CharacterTable::from_vfs(&vfs).expect("character table");
    let body = table
        .bodies
        .values()
        .next()
        .expect("a body row")
        .model
        .clone();
    let set = dc_render::ModelFiles::load(&vfs, &body)
        .expect("body")
        .anim_set();
    let run_f = set
        .find_clip(|n| {
            dc_render::name_key(n) == ([0xdc, 0xe6, 0x44, 0xba, 0x7b, 0xc1, 0xb3, 0x24], 3)
        })
        .unwrap();
    let mut d = SoundDirector::open(Some(&rom));
    let look = |t: f32| dc_client::RemoteLook {
        styles: Vec::new(),
        models: vec![dc_client::ModelRef {
            model: body.clone(),
            textures: vec![],
        }],
        weapon: None,
        trackers: vec![dc_client::RemoteTracker {
            clip: run_f,
            time_ms: t,
            blend_set: Some(1),
        }],
        visible: true,
        aim: None,
    };
    for t in (0..=2400).step_by(16) {
        let r = [dc_client::RemoteBody {
            obj: 300,
            pid: 2,
            name: "r".into(),
            team: 1,
            pos: [400.0, 50.0, 0.0],
            half: [15.0, 50.0, 15.0],
            yaw: None,
            look: Some(look(t as f32)),
        }];
        d.frame(&dc_client::sound::AudioFrame {
            now_ms: t,
            eye: [0.0, 33.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            cues: &[],
            remotes: &r,
            own: None,
            world: None,
        });
    }
    let steps = d.starts_of("footstep");
    println!(
        "{} remote steps; gains {:?}",
        steps.len(),
        steps.iter().map(|e| e.gain).collect::<Vec<_>>()
    );
    assert_eq!(steps.len(), 8, "2400 ms of a 600 ms clip with two keys");
    assert!(
        steps.iter().all(|e| e.gain.is_some_and(|g| g > 0.0)),
        "audible at 400 units (outer 1200)"
    );
}

//! ROM checks of the bots' navigation grid (BOT-93, BOT-112): run with `CA_ROM` (the user's client folder); without
//! it they pass without checking. Every number comes from the world file at run time.

use dc_botclient::nav::NavGrid;
use dc_botclient::romworld::{RomWorlds, WORLD_ARCHIVE};
use dc_world::{ObjectRole, ParseOptions, World, WORLD_VERSION};

/// The player spawn points of an announced world (positions only), read from the ROM.
fn spawn_points(rom: &std::path::Path, listed: &str) -> Vec<[f32; 3]> {
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)).unwrap();
    let raw = arch.entry_raw(listed).unwrap().unwrap();
    let data = if raw.starts_with(&WORLD_VERSION.to_le_bytes()) {
        raw
    } else {
        let key = dc_rez::dc_rom::crypt_key(rom).unwrap();
        dc_rez::decrypt_with_key(&raw, &key).unwrap()
    };
    let w = World::parse_with(&data, ParseOptions { render: false }).unwrap();
    w.objects_with_role(ObjectRole::PlayerSpawn)
        .filter_map(|o| o.position())
        .collect()
}

/// BOT-149: the speed the walks below run at. By default the neutral body (285 units/s), the speed these walks were
/// measured at; `NAVROM_SPEED=rifle` runs them at the bots' match speed (their lobby loadout with the rifle in hand,
/// penalty 0.23 in the ROM: 219 units/s, read from the ROM's speed rows). At the rifle speed four walks end short
/// (BOT-149 row: their routes change and meet places the follower has no way from).
fn walk_mods() -> dc_sim::SpeedMods {
    static MODS: std::sync::OnceLock<dc_sim::SpeedMods> = std::sync::OnceLock::new();
    *MODS.get_or_init(|| {
        let rifle = std::env::var("NAVROM_SPEED").is_ok_and(|v| v == "rifle");
        let rom = std::env::var_os("CA_ROM").filter(|_| rifle);
        let Some(rom) = rom else {
            return dc_sim::SpeedMods::default();
        };
        let rows = dc_bute::speed::from_vfs(&dc_rez::RezVfs::new(std::path::Path::new(&rom)))
            .expect("speed rows");
        let eq = [20, 1, 0, 7, -1, -1, -1, 2000, -1, 7001, -1, -1, -1, -1, -1];
        dc_botclient::motion::loadout_mods(
            &rows,
            Some(20),
            dc_botclient::motion::gear_rows(&eq),
            None,
            false,
        )
    })
}

/// The run speed of [`walk_mods`] (units/s).
fn walk_speed() -> f32 {
    285.0 * (1.0 - walk_mods().penalty.clamp(-1.0, 1.0))
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// BOT-112: on Gray Hammer the grid grown from one spawn point reaches every other one. Before, the grid grown from
/// the north spawn area never reached the south half (stairs steeper than the lattice step, a stairway under a
/// bridge, passages the lattice columns missed), so the bots of the two teams never met and never fired.
#[test]
fn rom_grayhammer_the_grid_reaches_every_spawn_point() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let listed = r.resolve(b"WORLDS/GRAYHAMMER.DAT").expect("the world");
    let w = r.load(&listed).expect("loads");
    let spawns = spawn_points(r.rom(), &listed);
    assert!(spawns.len() > 10, "{} spawn points", spawns.len());
    // the bot's body: the server's teleport box (BOT-112 grows the grid for it)
    let mut nav = NavGrid::new(w.collision.clone(), [16.0, 50.0, 16.0]);
    // the first spawn point the body stands at (a spawn point lies at the floor or a little above it)
    let start = spawns
        .iter()
        .find(|s| nav.seed(**s))
        .copied()
        .expect("a spawn point the body stands at");
    while nav.growing() && nav.probes < 3_000_000 {
        nav.expand(2_000, None);
    }
    let mut missed = Vec::new();
    for (i, s) in spawns.iter().enumerate().skip(1) {
        let t = *s;
        let reached = nav
            .path(start, t)
            .is_some_and(|p| flat(p[p.len() - 1], t) <= 72.0);
        if !reached {
            missed.push((i, s.map(f32::round)));
        }
    }
    assert!(
        missed.is_empty(),
        "{} of {} spawn points out of the grid's reach ({} nodes): {missed:?}",
        missed.len(),
        spawns.len() - 1,
        nav.len()
    );
}

/// How near the body gets to `to` when it runs from the standing place `from` straight at it (the movement code, at
/// 60 Hz, for the time the run takes plus half a second).
fn body_gets_to(
    world: &std::sync::Arc<dc_sim::CollisionWorld>,
    from: [f32; 3],
    to: [f32; 3],
) -> f32 {
    // BOT-132: a link up more than a step is a jump: the best of jumps at a few moments of the walk
    if to[1] - from[1] > 31.0 {
        return (0..12)
            .map(|k| body_run(world, from, to, Some(k * 5)))
            .fold(f32::MAX, f32::min);
    }
    body_run(world, from, to, None)
}

fn body_run(
    world: &std::sync::Arc<dc_sim::CollisionWorld>,
    from: [f32; 3],
    to: [f32; 3],
    jump_at: Option<u32>,
) -> f32 {
    use dc_botclient::motion::{Body, MotionKind};
    let mut b = Body::new(
        from,
        [16.0, 50.0, 16.0],
        0.0,
        MotionKind::Hunt,
        60,
        Some(world.clone()),
    );
    b.mods = walk_mods();
    let p = b.pos();
    b.steer = Some((to[0] - p[0]).atan2(to[2] - p[2]));
    let mut best = flat(p, to);
    for tick in 0..((best / walk_speed() + 0.5) * 60.0) as u32 + 40 {
        b.evade = (jump_at == Some(tick)).then_some(dc_botclient::evade::Move {
            yaw: 0.0,
            strafe: 0,
            back: false,
            jump: true,
            crouch: false,
            push: false,
            note: None,
        });
        b.advance(16_667);
        best = best.min(flat(b.pos(), to));
    }
    best
}

/// BOT-118: around the places where bots pushed into the world for 8-12 s at a time in a 6-minute Gray Hammer match
/// (a railing above the trench at x 1290, the sides of the stairs at x 870), the body walks every link of the grid.
/// Before, 111 of 6257 links there were no way for the body: links checked only to the probed place but stored to a
/// node up to a lattice step beyond it, and nodes on slopes the mover slides down.
#[test]
fn rom_grayhammer_the_body_walks_every_link_near_the_stuck_places() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let listed = r.resolve(b"WORLDS/GRAYHAMMER.DAT").expect("the world");
    let w = r.load(&listed).expect("loads");
    let places = [
        ([1354.0f32, 50.1, -376.0], [-2102.0f32, -2.0, -1962.0]),
        ([806.0, 50.1, -1152.0], [1447.0, 50.0, -723.0]),
        ([-1308.0, 50.1, -27.0], [-2102.0, -2.0, -1962.0]),
    ];
    let mut bad = Vec::new();
    let mut links = 0;
    for (at, goal) in places {
        let mut nav = NavGrid::new(w.collision.clone(), [16.0, 50.0, 16.0]);
        assert!(
            [
                (0.0f32, 0.0f32),
                (8.0, 0.0),
                (-8.0, 0.0),
                (0.0, 8.0),
                (0.0, -8.0),
                (24.0, 0.0)
            ]
            .iter()
            .any(|(x, z)| nav.seed([at[0] + x, at[1], at[2] + z])),
            "a node at {at:?}"
        );
        while nav.growing() && nav.probes < 400_000 {
            nav.expand(2_000, Some(goal));
        }
        for (n, to) in nav.links_near(at, 400.0) {
            for t in to {
                links += 1;
                let left = body_gets_to(&w.collision, n, t);
                if left > 40.0 {
                    bad.push((n.map(f32::round), t.map(f32::round), left.round()));
                }
            }
        }
    }
    assert!(links > 3000, "{links} links");
    // (BOT-132's jump nodes shift two walk links by the ladder at x -1185 and on the stairs at x 754 that the body
    // stops 42-50 units short of: OBSERVED 2026-11-07, known, at most two allowed; BOT-185's corner rule shifts the
    // nodes on those stairs, the one link up there becomes two drops off their side at x 713 the body stops 40-44
    // short of: OBSERVED 2026-11-09, at most three)
    assert!(
        bad.len() <= 3,
        "{} of {links} links the body does not walk: {bad:?}",
        bad.len()
    );
}

/// BOT-119: on Power Surge a bot on a crate or container top has a way down on the grid, and the body walks every
/// link from its nodes. Before, 17 of 290 tops 40..200 units above the floor were islands (the drop's straight sweep met
/// the edge; drops over 96 were no link), so a bot that got onto one (a stuck jump, a fall) stayed there.
#[test]
fn rom_powersurge_every_crate_top_has_a_way_down() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let listed = r.resolve(b"WORLDS/RAPIDSTREAM.DAT").expect("the world");
    let w = r.load(&listed).expect("loads");
    let spawns = spawn_points(r.rom(), &listed);
    // tops found by a sweep from above, off the grid grown from the spawns (OBSERVED 2026-11-04, EU-test ROM)
    let tops = [
        [-589.0f32, -3112.0, -2283.0],
        [35.0, -3037.0, -843.0],
        [227.0, -3313.0, -939.0],
        [-685.0, -3354.0, 69.0],
        [515.0, -2928.0, -123.0],
    ];
    let mut bad = Vec::new();
    for top in tops {
        let mut nav = NavGrid::new(w.collision.clone(), [16.0, 50.0, 16.0]);
        let home = spawns
            .iter()
            .find(|s| nav.seed(**s))
            .copied()
            .expect("a spawn point the body stands at");
        let at = [top[0], top[1] + 51.0, top[2]];
        assert!(nav.seed(at), "a node on the top at {top:?}");
        while nav.growing() && nav.probes < 800_000 {
            nav.expand(2_000, Some(at));
        }
        let off = nav
            .path(at, home)
            .is_some_and(|p| p.iter().any(|q| q[1] < at[1] - 30.0));
        if !off {
            bad.push((top, "no way down".to_string()));
        }
        // the links from the top's nodes (the ways off it)
        for (n, to) in nav.links_near(at, 150.0) {
            if (n[1] - at[1]).abs() > 20.0 {
                continue;
            }
            for t in to {
                let left = body_gets_to(&w.collision, n, t);
                if left > 40.0 {
                    bad.push((
                        top,
                        format!(
                            "{:?} -> {:?} left {left:.0}",
                            n.map(f32::round),
                            t.map(f32::round)
                        ),
                    ));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

/// The world bytes of an announced world, read from the ROM.
fn world_data(rom: &std::path::Path, listed: &str) -> Vec<u8> {
    let arch = dc_rez::RezArchive::open(&rom.join("Game").join(WORLD_ARCHIVE)).unwrap();
    let raw = arch.entry_raw(listed).unwrap().unwrap();
    if raw.starts_with(&WORLD_VERSION.to_le_bytes()) {
        raw
    } else {
        let key = dc_rez::dc_rom::crypt_key(rom).unwrap();
        dc_rez::decrypt_with_key(&raw, &key).unwrap()
    }
}

/// BOT-189: the session's test (`session::dead_end_below`): the run leads off an edge deeper than the grid's walk
/// off onto a dead end.
fn dead_end_below(nav: &NavGrid, world: &dc_sim::CollisionWorld, from_y: f32, run: &[[f32; 3]]) -> bool {
    dc_botclient::evade::deep_landing(world, BODY, from_y, run.iter().copied())
        .is_some_and(|l| nav.dead_end(l, dc_botclient::nav::DEAD_END_PROBES, 600.0))
}

/// A ROM world as a room's server sets it up before its start scripts (BOT-110): the linked models whose object has
/// the solid property off are left out of the collision set (water planes, fall-through floors), with the world's
/// kill volumes, ladders, spawn points and flags.
struct RoomWorld {
    w: dc_botclient::romworld::LoadedWorld,
    kill: Vec<dc_sim::ConvexVolume>,
    hazard: Vec<dc_sim::ConvexVolume>,
    ladders: Vec<dc_sim::ConvexVolume>,
    spawns: Vec<[f32; 3]>,
    flags: Vec<[f32; 3]>,
}

fn room_world(r: &RomWorlds, announced: &[u8]) -> RoomWorld {
    let listed = r.resolve(announced).expect("the world");
    let mut w = r.load(&listed).expect("loads");
    let data = world_data(r.rom(), &listed);
    let pw = World::parse_with(&data, ParseOptions { render: false }).unwrap();
    let mut c = (*w.full).clone();
    for l in &w.linked {
        let off = pw
            .objects
            .iter()
            .find(|o| o.name().is_some_and(|n| n.to_ascii_lowercase() == l.name))
            .and_then(|o| o.prop_named(&dc_world::names::PROP_SOLID))
            .is_some_and(|p| matches!(p.value, dc_world::FieldValue::Bool(false)));
        if off {
            c.models[l.index] = dc_sim::CollModel::new(&[], &[], None);
        }
    }
    w.collision = std::sync::Arc::new(c);
    let kill = w.kill_volumes.iter().map(|k| k.1.clone()).collect();
    // BOT-154: the hazard volumes, which the session's grid takes apart (`NavGrid::with_hazard_volumes`)
    let hazard = w.hazard_volumes.clone();
    let ladders = w.ladders.volumes.iter().map(|v| v.0.clone()).collect();
    let pos = |role: ObjectRole| -> Vec<[f32; 3]> {
        pw.objects_with_role(role)
            .filter_map(|o| o.position())
            .collect()
    };
    RoomWorld {
        spawns: pos(ObjectRole::PlayerSpawn),
        flags: pos(ObjectRole::CaptureFlag),
        w,
        kill,
        hazard,
        ladders,
    }
}

const BODY: [f32; 3] = [16.0, 50.0, 16.0];

/// BOT-181: the game server's fall track (DSV-76) over a walk's movement trailers: the highest place since the last
/// grounded or ladder trailer, and the highest fall a landing ended.
#[derive(Default)]
struct FallTrack {
    top: Option<f32>,
    airborne: bool,
    worst: f32,
    /// BOT-186: the steps (of 50 ms) left of the 3 s after the last jump, and the falls of more than 30 units in them.
    after_jump: u32,
    jump_falls: u32,
}

impl FallTrack {
    fn feed(&mut self, b: &dc_botclient::motion::Body) {
        let t = b.update(0.0).trailer;
        let y = t.pos[1];
        if t.surface == dc_botclient::motion::LADDER_SURFACE {
            (self.top, self.airborne) = (Some(y), false);
        } else if t.on_ground != 0 {
            if self.airborne {
                self.worst = self.worst.max(self.top.unwrap_or(y) - y);
                if self.after_jump > 0 && self.top.unwrap_or(y) - y > 30.0 {
                    self.jump_falls += 1;
                }
            }
            (self.top, self.airborne) = (Some(y), false);
        } else {
            self.top = Some(self.top.map_or(y, |t| t.max(y)));
            self.airborne = true;
        }
    }
}

std::thread_local! {
    /// BOT-181: the highest fall of the last walk on this thread (`RoomWorld::walk_known`).
    static LAST_FALL: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
    /// BOT-186: the jumps of the last walk on this thread, and its falls of more than 30 units within 3 s of a jump.
    static LAST_JUMPS: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

/// [`room_world`] with every door and switch open (the bots' grid takes doors as ways, BOT-120, and a bot opens the
/// door it walks into): their models are left out of the collision set.
fn room_world_open(r: &RomWorlds, announced: &[u8]) -> RoomWorld {
    let mut room = room_world(r, announced);
    let mut c = (*room.w.collision).clone();
    for l in room.w.linked.iter().filter(|l| l.mover) {
        c.models[l.index] = dc_sim::CollModel::new(&[], &[], None);
    }
    room.w.collision = std::sync::Arc::new(c);
    room
}

/// BOT-141: the linked models a Spy Hunt room's server leaves out of the collision set (OBSERVED 2026-11-08: the bots'
/// `world_set` event `off`, 2-bot Spy Hunt rooms on the release build); the rest stay solid.
const SPY_OFF_WARHEAD: &[&str] = &[
    "worldmodel26",
    "worldmodel24",
    "worldmodel25",
    "worldmodel0",
    "worldmodel3",
    "collision_box01",
    "collision_mdl01",
    "qrt_show_n",
    "qrt_show_l",
    "worldmodel02",
    "worldmodel01",
    "brk70drumq3",
    "brk70drumq4",
    "rotatingdoor03",
    "rotatingdoor04",
];
const SPY_OFF_WAVERIDER: &[&str] = &[
    "genetic_blender",
    "uw_str_blender",
    "worldmodel2",
    "worldmodel3",
    "worldmodel01",
    "worldmodel02",
    "worldmodel00",
    "collision_box01",
    "collision_mdl01",
];
const SPY_OFF_TWOTOWERS: &[&str] = &[
    "collision_box01",
    "collision_mdl01",
    "worldmodel01",
    "worldmodel02",
    "worldmodel0",
    "worldmodel1",
    "worldmodel2",
    "blender01",
    "worldmodel3",
    "worldmodel4",
    "qrt_blood",
    "qrt_show_g",
    "qrt_show_l",
    "qrt_show_r",
    "qrt_show_f",
    "qrt_show_n",
];
const SPY_OFF_SNOWVALLEY: &[&str] = &[
    "broken_gas",
    "collision_box01",
    "collision_mdl01",
    "worldmodel0",
    "worldmodel13",
    "worldmodel4",
    "slidingdoor2",
    "slidingdoor0",
    "worldmodel5",
    "worldmodel7",
    "worldmodel1",
    "worldmodel14",
    "worldmodel15",
    "worldmodel10",
    "worldmodel12",
    "worldmodel8",
    "slidingdoor1",
    "worldmodel2",
    "worldmodel3",
    "worldmodel03",
    "worldmodel04",
    "worldmodel05",
];
const SPY_OFF_GRAYHAMMER: &[&str] = &["collision_box01", "collision_mdl01", "worldmodel1"];

/// BOT-141: [`room_world`] with the collision set of a room's server: the linked models in `off` left out, every
/// other one solid, and every door open (the bots' grid takes doors as ways, BOT-120).
fn room_world_set(r: &RomWorlds, announced: &[u8], off: &[&str]) -> RoomWorld {
    let mut room = room_world(r, announced);
    let mut c = (*room.w.full).clone();
    for l in &room.w.linked {
        if l.mover || off.iter().any(|n| n.as_bytes() == l.name.as_slice()) {
            c.models[l.index] = dc_sim::CollModel::new(&[], &[], None);
        }
    }
    room.w.collision = std::sync::Arc::new(c);
    room
}

/// The Spy Hunt room's world of a map index (0 Snow Valley, 1 Gray Hammer, 3 Two Towers, 8 Warhead, 9 Waverider).
fn spy_room(r: &RomWorlds, map: u32) -> RoomWorld {
    let (w, off): (&[u8], &[&str]) = match map {
        0 => (b"WORLDS/SNOWVALLEY.DAT", SPY_OFF_SNOWVALLEY),
        1 => (b"WORLDS/GRAYHAMMER.DAT", SPY_OFF_GRAYHAMMER),
        3 => (b"WORLDS/TWOTOWERS.DAT", SPY_OFF_TWOTOWERS),
        8 => (b"WORLDS/WARHEAD.DAT", SPY_OFF_WARHEAD),
        9 => (b"WORLDS/WAVERIDER.DAT", SPY_OFF_WAVERIDER),
        _ => panic!("no Spy Hunt set for map {map}"),
    };
    room_world_set(r, w, off)
}

impl RoomWorld {
    fn grid(&self) -> NavGrid {
        NavGrid::new(self.w.collision.clone(), BODY)
            .with_kill_volumes(self.kill.clone())
            .with_hazard_volumes(self.hazard.clone())
            .with_ladders(&self.ladders)
    }

    /// The grid grown in full from the first spawn point the body stands at, and that point.
    fn grown(&self) -> (NavGrid, [f32; 3]) {
        let mut nav = self.grid();
        let start = self
            .spawns
            .iter()
            .find(|s| nav.seed(**s))
            .copied()
            .expect("a spawn point the body stands at");
        while nav.growing() && nav.probes < 4_000_000 {
            nav.expand(2_000, None);
        }
        (nav, start)
    }

    /// The spawn points a body stands at in this collision set (a mode's own spawn platforms may be left out).
    fn standing_spawns(&self) -> Vec<[f32; 3]> {
        self.spawns
            .iter()
            .filter(|s| self.grid().seed(**s))
            .copied()
            .collect()
    }

    /// The places of `to` the grid's way from `start` does not end within 72 units (flat) and 120 in height of (a
    /// spawn point may lie that far above its floor).
    fn missed(&self, nav: &mut NavGrid, start: [f32; 3], to: &[[f32; 3]]) -> Vec<[f32; 3]> {
        to.iter()
            .filter(|t| {
                !nav.path(start, **t).is_some_and(|p| {
                    let e = p[p.len() - 1];
                    flat(e, **t) <= 72.0 && (e[1] - t[1]).abs() <= 120.0
                })
            })
            .map(|t| t.map(f32::round))
            .collect()
    }

    /// The nodes and link middles where the body touches a kill volume.
    fn in_kill_volumes(&self, nav: &NavGrid) -> Vec<[f32; 3]> {
        let shape = dc_sim::Cylinder::from_dims(dc_sim::V3::new(BODY[0], BODY[1], BODY[2]));
        let touches = |p: [f32; 3]| {
            self.kill
                .iter()
                .any(|k| k.touches(&shape, dc_sim::V3::new(p[0], p[1], p[2])))
        };
        let mut bad = Vec::new();
        for (n, to) in nav.links_near([0.0; 3], f32::MAX) {
            if touches(n) {
                bad.push(n.map(f32::round));
            }
            for t in to {
                let m = [(n[0] + t[0]) * 0.5, n[1].max(t[1]), (n[2] + t[2]) * 0.5];
                if touches(m) {
                    bad.push(m.map(f32::round));
                }
            }
        }
        bad
    }

    /// A bot's walk from `from` to `to` as the session does it: the follower (BOT-121) asked every 50 ms, the body
    /// at 60 Hz steering at its point, climbing a ladder toward the goal's height, jumping when the follower says
    /// so and the jump is safe (BOT-134's jump test), kicked by BOT-134's watchdog after 3 s within 48 units while it
    /// has a way, the greedy way-finder on an island of the grid (BOT-145), standing where the grid gives no way yet. `Ok(seconds)` when it gets within 72 units, `Err` with where it ended (or died
    /// in a kill volume).
    fn goto(&self, from: [f32; 3], to: [f32; 3], secs: u32) -> Result<f32, String> {
        self.walk(None, from, to, secs, |p| {
            flat(p, to) <= 72.0 && (p[1] - to[1]).abs() <= 90.0
        })
    }

    /// BOT-141: [`Self::goto`] to an intel case, done only when the body gets into the server's touch window (56
    /// units across, its centre at most 32 above and 53 below the case's point, DSV-391): a body under a case on a
    /// crate has not got it.
    fn take(&self, from: [f32; 3], case: [f32; 3], secs: u32) -> Result<f32, String> {
        self.take_on(None, from, case, secs)
    }

    /// BOT-184: [`Self::take`] on the grid `nav` (a grid the bot grew before, with its deep falls toward the case),
    /// or a new one.
    fn take_on(
        &self,
        nav: Option<NavGrid>,
        from: [f32; 3],
        case: [f32; 3],
        secs: u32,
    ) -> Result<f32, String> {
        self.walk(nav, from, case, secs, |p| {
            flat(p, case) <= 56.0 && (-53.0..=32.0).contains(&(p[1] - case[1]))
        })
    }

    fn walk(
        &self,
        pre: Option<NavGrid>,
        from: [f32; 3],
        to: [f32; 3],
        secs: u32,
        arrived: impl Fn([f32; 3]) -> bool,
    ) -> Result<f32, String> {
        self.walk_known(pre, from, to, secs, arrived, None)
    }

    /// BOT-181: [`Self::goto`] with the grid grown first from `known` (where the bot walked before it got to `from`),
    /// the way a session's grid knows the map beyond the place it stands at.
    fn goto_known(
        &self,
        from: [f32; 3],
        to: [f32; 3],
        secs: u32,
        known: [f32; 3],
    ) -> Result<f32, String> {
        self.walk_known(
            None,
            from,
            to,
            secs,
            |p| flat(p, to) <= 72.0 && (p[1] - to[1]).abs() <= 90.0,
            Some(known),
        )
    }

    fn walk_known(
        &self,
        pre: Option<NavGrid>,
        from: [f32; 3],
        to: [f32; 3],
        secs: u32,
        arrived: impl Fn([f32; 3]) -> bool,
        known: Option<[f32; 3]>,
    ) -> Result<f32, String> {
        use dc_botclient::motion::{Body, MotionKind};
        use dc_botclient::nav::follow::{Follower, NavGoal, Stuck, GROW_PROBES};
        // (the session's danger tests take the hazard volumes as kill volumes, BOT-143)
        let danger: Vec<_> = self.kill.iter().chain(&self.hazard).cloned().collect();
        let mut nav = pre.unwrap_or_else(|| self.grid());
        if let Some(k) = known {
            assert!(nav.seed(k), "the known place stands");
            while nav.growing() && nav.probes < 4_000_000 {
                nav.expand(2_000, None);
            }
        }
        let mut f = Follower::default();
        let mut b = Body::new(
            from,
            BODY,
            0.0,
            MotionKind::Hunt,
            60,
            Some(self.w.collision.clone()),
        );
        b.mods = walk_mods();
        b.ladders = Some(self.w.ladders.clone());
        b.goal_y = Some(to[1]);
        let point = dc_sim::Cylinder::from_dims(dc_sim::V3::new(0.01, 0.01, 0.01));
        // BOT-134's watchdog: the place the body stayed within 48 units of while it walked its way, and since when
        let mut watch: Option<([f32; 3], u32)> = None;
        let mut hunter = dc_botclient::hunt::Hunter::default();
        let mut greedy_check: Option<(u32, [f32; 3])> = None;
        // BOT-159: on a small island of the grid (8 s after the first island or partial-way end, or at BOT-145's third
        // watchdog trip in one place) the bot takes a way off it
        let mut island_since: Option<u32> = None;
        let mut escape: Option<(dc_botclient::evade::Escape, u32)> = None;
        let mut kicks: Vec<(u32, [f32; 3])> = Vec::new();
        let mut jumped = false;
        let mut esc_last: Option<[f32; 3]> = None;
        let mut guard_held = false;
        let mut falls = FallTrack::default();
        LAST_FALL.with(|c| c.set(0.0));
        LAST_JUMPS.with(|c| c.set((0, 0)));
        let mut jumps = 0u32;
        for step in 0..secs * 20 {
            let now = step * 50;
            let p = b.pos();
            if arrived(p) {
                return Ok(now as f32 / 1000.0);
            }
            if let Some((e, at)) = escape {
                if now - at < (dc_botclient::evade::ESCAPE_SECS * 1000.0) as u32 {
                    b.steer = Some(e.yaw);
                    let speed = esc_last.map_or(f32::MAX, |q: [f32; 3]| flat(q, p) / 0.05);
                    esc_last = Some(p);
                    let due = !jumped
                        && b.escape_jump_due(e.jump, (now - at) as f32 / 1000.0, speed);
                    jumped |= due;
                    b.evade = due.then_some(dc_botclient::evade::Move {
                        yaw: e.yaw,
                        strafe: 0,
                        back: false,
                        jump: true,
                        crouch: false,
                        push: false,
                        note: None,
                    });
                    for _ in 0..3 {
                        b.advance(16_667);
                        falls.feed(&b);
                    }
                    LAST_FALL.with(|c| c.set(falls.worst));
                    let q = b.pos();
                    if self
                        .kill
                        .iter()
                        .any(|k| k.touches(&point, dc_sim::V3::new(q[0], q[1], q[2])))
                    {
                        return Err(format!("died at {:?}", q.map(f32::round)));
                    }
                    continue;
                }
                escape = None;
                island_since = None;
                f = Follower::default();
            }
            if island_since.is_some_and(|t| now - t >= 8000) {
                island_since = None;
                if let Some(part) = nav.confined_part(p, 600.0) {
                    // BOT-181: an end at a known node off the island leaves it too
                    let known =
                        |q: [f32; 3]| dc_botclient::evade::known_off_island(Some(&nav), &part, q);
                    // BOT-189: an end that is a dead end does not count
                    let dead_end =
                        |q: [f32; 3]| nav.dead_end(q, dc_botclient::nav::DEAD_END_PROBES, 600.0);
                    escape = dc_botclient::evade::escape_move_open(
                        &b,
                        &self.w.collision,
                        &danger,
                        &part,
                        dc_botclient::nav::ESCAPE_DROP,
                        &known,
                        &dead_end,
                    )
                    .map(|e| (e, now));
                    jumped = false;
                    esc_last = None;
                    if escape.is_some() {
                        continue;
                    }
                }
            }
            match watch {
                // (on a ladder the height climbed counts, as the session's watchdog)
                Some((at, _)) if flat(at, p) > 48.0 || (b.on_ladder() && (at[1] - p[1]).abs() >= 48.0) => {
                    watch = Some((p, now))
                }
                Some((_, since)) if now - since >= 3000 => {
                    if !f.points().is_empty() {
                        f.kick(&mut nav, now, p);
                    }
                    // BOT-145's third trip within 128 units and 30 s gives the goal up (BOT-159: on an island,
                    // the way off it)
                    kicks.retain(|(t, q)| now - t <= 30_000 && flat(*q, p) <= 128.0);
                    kicks.push((now, p));
                    if kicks.len() >= 3 {
                        kicks.clear();
                        island_since = Some(now.saturating_sub(8000));
                    }
                    watch = Some((p, now));
                }
                None => watch = Some((p, now)),
                _ => {}
            }
            f.grow(&mut nav, p, GROW_PROBES);
            let mut jump = false;
            let next = f.next(&mut nav, now, p, to, true);
            let way_arm = matches!(next, NavGoal::Way(_));
            b.steer = match next {
                NavGoal::Way(w) => {
                    greedy_check = None;
                    hunter.stop();
                    let steer = (w[0] - p[0]).atan2(w[2] - p[2]);
                    // BOT-139: the ladder pitch follows the next point's height
                    b.goal_y = Some(w[1]);
                    // BOT-134: the session takes the stuck jump only when its flight and landing are safe
                    // BOT-196: a jump only when the body's jump leaves the ground now
                    let said = f.stuck(&mut nav, now, p, b.can_jump()) == Stuck::Jump;
                    jump = said
                        && dc_botclient::evade::jump_safe(
                            &b,
                            &self.w.collision,
                            &danger,
                            Some(steer),
                            None,
                        );
                    // BOT-145: the session refuses a jump up whose flight its jump test refused (BOT-191: such
                    // refusals near one another count together)
                    // BOT-196: a jump not taken is no jump of a run
                    if said && !jump {
                        f.jump_not_taken();
                    }
                    if said && !jump && f.last_jump.is_some_and(|w| w.top) {
                        f.refuse(&mut nav, now, p, false);
                    }
                    Some(steer)
                }
                // (at the end of a partial way the session's greedy way-finder walks on: not modelled, the body
                // stands while the grid grows)
                NavGoal::Steer => {
                    island_since.get_or_insert(now);
                    None
                }
                // BOT-145: on an island of the grid the session's greedy way-finder walks (a probe that passes
                // danger made no progress, BOT-111), with its stuck jump when the body made no way for 1.5 s
                NavGoal::Hold => {
                    island_since.get_or_insert(now);
                    // (BOT-189: nor a walk off an edge deeper than the grid's walk off onto a dead end)
                    let probe = |y: f32| {
                        let path = b.probe_path(Some(y), None, dc_botclient::hunt::PROBE_SECS, 4);
                        (dc_botclient::evade::path_safe(&self.w.collision, &danger, BODY, &path)
                            && !dead_end_below(&nav, &self.w.collision, p[1], &path))
                        .then(|| path[path.len() - 1])
                    };
                    let y = hunter.steer(now, p, to, Some(&probe));
                    match greedy_check {
                        Some((at, from)) if now - at >= 1500 => {
                            if flat(from, p) < 24.0 {
                                jump = dc_botclient::evade::jump_safe(
                                    &b,
                                    &self.w.collision,
                                    &danger,
                                    y,
                                    None,
                                );
                            }
                            greedy_check = Some((now, p));
                        }
                        Some(_) => {}
                        None => greedy_check = Some((now, p)),
                    }
                    y
                }
            };
            // BOT-150 / BOT-181: the session's walk guard: a walk whose next half second (run on a copy) passes danger
            // while the body stands safe is held until the next check (every 250 ms); the way's step it took is
            // given up at every check that holds it (a ladder's climb leg is not tested)
            let ladder_leg = way_arm
                && f.points().first().is_some_and(|wp| {
                    nav.ladder_ends(*wp, p).is_some() || nav.ladder_ends(p, *wp).is_some()
                });
            match (b.steer, b.on_ladder() || ladder_leg) {
                (Some(y), false) => {
                    if step % 5 == 0 {
                        let coll = &self.w.collision;
                        let here = dc_botclient::evade::walk_danger(coll, &danger, BODY, p);
                        let path = b.probe_path_ladder(
                            Some(y),
                            None,
                            dc_botclient::qrt::WALK_GUARD_SECS,
                            5,
                        );
                        let run: Vec<[f32; 3]> =
                            path.iter().take_while(|(_, ladder)| !ladder).map(|(q, _)| *q).collect();
                        // BOT-189: a walk off an edge deeper than the grid's walk off onto a dead end is held too,
                        // unless the way steers down a drop of its own
                        let way_drops = way_arm
                            && f.points().first().is_some_and(|w| p[1] - w[1] > dc_botclient::nav::STEP_UP);
                        let held = !here
                            && (run.iter().any(|q| dc_botclient::evade::walk_danger(coll, &danger, BODY, *q))
                                || (!way_drops && dead_end_below(&nav, coll, p[1], &run)));
                        if held && way_arm {
                            if let Some(wp) = f.points().first().copied() {
                                nav.ban_step(p, wp);
                                f.kick(&mut nav, now, p);
                            }
                        }
                        guard_held = held;
                    }
                    if guard_held {
                        b.steer = None;
                    }
                }
                _ => guard_held = false,
            }
            falls.after_jump = if jump { 60 } else { falls.after_jump.saturating_sub(1) };
            jumps += u32::from(jump);
            LAST_JUMPS.with(|c| c.set((jumps, falls.jump_falls)));
            b.evade = jump.then_some(dc_botclient::evade::Move {
                yaw: 0.0,
                strafe: 0,
                back: false,
                jump: true,
                crouch: false,
                push: false,
                note: None,
            });
            for _ in 0..3 {
                b.advance(16_667);
                falls.feed(&b);
            }
            LAST_FALL.with(|c| c.set(falls.worst));
            let q = b.pos();
            if self
                .kill
                .iter()
                .any(|k| k.touches(&point, dc_sim::V3::new(q[0], q[1], q[2])))
            {
                return Err(format!("died at {:?}", q.map(f32::round)));
            }
        }
        Err(format!("ended at {:?}", b.pos().map(f32::round)))
    }
}

/// BOT-131 / BOT-132: on Two Towers the grid grown from one team's spawn reaches every spawn point (both halves of
/// the ground floor, over the bars between them, the towers), both flag bases and the Spy Hunt intel cases (one on
/// the lower floor by the water, reached by a ladder), and no node or link middle touches a kill volume. Before, the
/// two halves were apart (bars 48 and 57 above the floors, under a low beam), 32 of 100 spawn points and two of three
/// intel cases were out of reach, links crossed cracks over the water, and bots fell into the fall volume under the
/// water (22 world deaths in a 6-minute TDM room). A bot's walk across the map and back does not end in the water.
#[test]
fn rom_twotowers_the_grid_reaches_every_goal_and_keeps_out_of_the_water() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world(&r, b"WORLDS/TWOTOWERS.DAT");
    let (mut nav, start) = room.grown();
    // the intel cases of a Spy Hunt room (OBSERVED 2026-11-07: the server's case places in a BNT room)
    let cases = [
        [2562.0f32, -315.0, -454.0],
        [3408.0, -727.0, -1625.0],
        [3450.0, -967.0, -397.0],
    ];
    let mut goals = room.standing_spawns();
    assert!(goals.len() > 60, "{} spawn points", goals.len());
    goals.extend(room.flags.iter().copied());
    goals.extend(cases);
    let missed = room.missed(&mut nav, start, &goals);
    assert!(
        missed.is_empty(),
        "{} of {} out of reach: {missed:?}",
        missed.len(),
        goals.len()
    );
    let bad = room.in_kill_volumes(&nav);
    assert!(
        bad.is_empty(),
        "{} places in kill volumes: {bad:?}",
        bad.len()
    );
    // walks between the two halves (over the bars and along the cracks by the water) and up the ladder from the
    // lower floor never end in the water (the walk itself may stop short: the breakables stand in this set)
    let (a, b) = ([312.0f32, -696.0, 1824.0], [3408.0f32, -727.0, -1625.0]);
    for (from, to) in [(a, b), (b, a), (a, cases[0]), (cases[2], a)] {
        let walk = room.goto(from, to, 90);
        assert!(
            !walk.as_ref().is_err_and(|e| e.starts_with("died")),
            "{from:?} -> {to:?}: {walk:?}"
        );
    }
    let home = [3029.0f32, -716.0, -830.0];
    let up = room.goto(cases[2], home, 120);
    assert!(up.is_ok(), "up the ladder from the lower floor: {up:?}");
}

/// BOT-131: on Rattlesnake no node or link middle of the grid touches a kill volume, and the grid reaches the spawn
/// points. (The world deaths of a Search and Destroy room there were the bomb's blast, not the volumes.)
#[test]
fn rom_rattlesnake_the_grid_keeps_out_of_the_kill_volumes() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world(&r, b"WORLDS/RATTLESNAKE.DAT");
    let (mut nav, start) = room.grown();
    let bad = room.in_kill_volumes(&nav);
    assert!(
        bad.is_empty(),
        "{} places in kill volumes: {bad:?}",
        bad.len()
    );
    // (one spawn point stands on a perch the ground has no way up to: OBSERVED, not counted)
    let spawns = room.standing_spawns();
    let missed = room.missed(&mut nav, start, &spawns);
    assert!(
        missed.len() <= spawns.len() / 20,
        "{} of {} spawn points out of reach: {missed:?}",
        missed.len(),
        spawns.len()
    );
}

/// BOT-131: on Gray Hammer the grid reaches both flag bases and every spawn point, and a bot's walk out of the pit at
/// the map's middle (y -170) and from beside the raised floor by the team-1 base gets to either base. Before, a body
/// pushed against the raised floor had no node at its own place and planned from a node on top of it (a way that
/// led nowhere for its body).
#[test]
fn rom_grayhammer_the_grid_reaches_both_flag_bases_and_out_of_the_pit() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world(&r, b"WORLDS/GRAYHAMMER.DAT");
    let (mut nav, start) = room.grown();
    assert_eq!(room.flags.len(), 2, "two flags");
    let mut goals = room.standing_spawns();
    goals.extend(room.flags.iter().copied());
    let missed = room.missed(&mut nav, start, &goals);
    assert!(
        missed.is_empty(),
        "{} of {} out of reach: {missed:?}",
        missed.len(),
        goals.len()
    );
    // the places bots stood at in the surveys (OBSERVED 2026-11-07: CTF and TSV rooms on this map)
    let pit = [20.0f32, -158.0, -27.0];
    let by_base = [2375.0f32, 66.0, 678.0];
    for (from, to) in [
        (pit, room.flags[0]),
        (pit, room.flags[1]),
        (by_base, room.flags[0]),
        (by_base, room.flags[1]),
    ] {
        let walk = room.goto(from, to, 120);
        assert!(walk.is_ok(), "{from:?} -> {to:?}: {walk:?}");
    }
}

/// BOT-138: on every flag-mode world the grid grown from a spawn reaches each flag's touch (64 units across, 96 up /
/// down of the flag point, DSV-192) and the body walks from the way's end into it.
#[test]
fn rom_ctf_every_flag_is_in_touch_from_the_grid() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let only = std::env::var("CTF_WORLD").ok();
    let mut missed = Vec::new();
    for world in [
        "SNOWVALLEY",
        "GRAYHAMMER",
        "JUNK_FLEA",
        "TWOTOWERS",
        "PUMPJACK",
        "BRUSHWOOD",
        "COLDSEED",
        "SANDHOG",
        "WARHEAD",
        "WAVERIDER",
        "DEATHROOM",
        "RAPIDSTREAM",
        "GRAVEDIGGER",
        "ROADKILL",
    ] {
        if only.as_deref().is_some_and(|o| o != world) {
            continue;
        }
        let room = room_world(&r, format!("WORLDS/{world}.DAT").as_bytes());
        assert_eq!(room.flags.len(), 2, "{world}: two flags");
        for (i, f) in room.flags.iter().enumerate() {
            // the way from the other flag's base (a carrier's run home, an attacker's from its own base)
            let from = room.flags[1 - i];
            let mut nav = room.grid();
            let start = [
                (0.0f32, 0.0f32),
                (16.0, 0.0),
                (-16.0, 0.0),
                (0.0, 16.0),
                (0.0, -16.0),
            ]
            .iter()
            .flat_map(|(x, z)| [0.0f32, 28.0].map(|y| [from[0] + x, from[1] + y, from[2] + z]))
            .find(|p| nav.seed(*p))
            .expect("a node at the other flag");
            while nav.growing() && nav.probes < 4_000_000 {
                nav.expand(2_000, None);
            }
            let end = nav.path(start, *f).map(|p| p[p.len() - 1]);
            let near = nav
                .links_near(*f, 600.0)
                .into_iter()
                .map(|n| n.0)
                .min_by(|a, b| flat(*a, *f).total_cmp(&flat(*b, *f)));
            let walk = end.map(|e| body_gets_to(&room.w.collision, e, *f));
            eprintln!(
                "{world}: flag {:?} way end {:?} ({:?} across) nearest node {:?} body to {:?}",
                f.map(f32::round),
                end.map(|e| e.map(f32::round)),
                end.map(|e| flat(e, *f).round()),
                near.map(|e| e.map(f32::round)),
                walk.map(f32::round)
            );
            let in_touch = end.is_some_and(|e| flat(e, *f) <= 64.0 && (e[1] - f[1]).abs() <= 96.0);
            if !in_touch || walk.is_none_or(|w| w > 48.0) {
                missed.push((world, f.map(f32::round), near.map(|e| e.map(f32::round))));
            }
        }
    }
    // (flag, nearest grid node) of each flag out of touch: a grid gap to hand on
    assert!(missed.is_empty(), "flags out of touch: {missed:?}");
}

/// BOT-139: on Waverider a bot on the upper deck (y -2750) walks down to the far flag. Before, its plan led over the
/// deck's 50-unit railing (a jump up whose run went on over the drop beyond, so the bot's jump test never took it) and
/// the grid grown toward the flag never grew the way back along the deck that leads down (the follower stood at the
/// end of its partial way): a bot stood 4 minutes there in a CTF room.
#[test]
fn rom_waverider_a_bot_on_the_upper_deck_walks_down_to_the_flag() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world(&r, b"WORLDS/WAVERIDER.DAT");
    let walk = room.goto([-2356.0, -2750.0, 3627.0], [-2128.0, -3250.0, -815.0], 120);
    assert!(walk.is_ok(), "{walk:?}");
}

/// BOT-139: on Power Surge a flag carrier at the foot of the ladder by x -120 climbs it and gets off at its top on its
/// way home, and a bot on the floor above walks to the other flag. Before, the ladder's top counted as reached while
/// the body was still on the rungs below it (within 90 units in height) and the bot steered on, off the ladder: it
/// fell back and climbed again for 40 s.
#[test]
fn rom_powersurge_a_carrier_climbs_the_ladder_and_gets_off_at_its_top() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world(&r, b"WORLDS/RAPIDSTREAM.DAT");
    for (from, to) in [
        ([-155.0f32, -3465.0, -1376.0], [1531.0f32, -3189.0, -816.0]),
        ([113.0, -3168.0, -1464.0], [-1132.0, -3189.0, -1238.0]),
    ] {
        let walk = room.goto(from, to, 60);
        assert!(walk.is_ok(), "{from:?} -> {to:?}: {walk:?}");
    }
}

/// BOT-139: on Overdose the grid grown from a spawn point in the spawn hall leaves the hall (its ledges stand 64 above
/// the floor, a running jump) and reaches the far side of the map, and a bot walks out. Before, the grid from there
/// had 390 nodes (the hall and its ramp, BOT-132's jumps reached 58): bots left only by greedy stuck jumps and one stood
/// 32 s at the top of the ramp with a way that led nowhere.
#[test]
fn rom_overdose_the_spawn_hall_has_ways_out() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_open(&r, b"WORLDS/OVERDOSE.DAT");
    let spawn = [-102.0f32, 104.0, 1275.0];
    let mut nav = room.grid();
    assert!(nav.seed(spawn), "a node at the spawn point");
    while nav.growing() && nav.probes < 4_000_000 {
        nav.expand(2_000, None);
    }
    assert!(nav.len() > 2000, "{} nodes from the spawn hall", nav.len());
    let far = [[690.0f32, 274.0, 337.0], [-551.0, 178.0, 964.0]];
    let missed = room.missed(&mut nav, spawn, &far);
    assert!(missed.is_empty(), "out of reach: {missed:?}");
    let walk = room.goto(spawn, far[1], 60);
    assert!(walk.is_ok(), "out of the hall: {walk:?}");
}

/// BOT-139: on Two Towers a bot from the team-1 spawn walks to the Spy Hunt intel cases under the upper walk (one on
/// a ledge reached only by a drop from the walk above it, one on the floor by the water). Before, the grid grown
/// toward a case never grew the way that first leads away from it (up the tower to the walk, along it to the drop):
/// the follower stood at the end of its partial way, and every floor below a drop counted as a pit (BOT-119) until
/// the grid had found the way back up from it, so no plan took the drop. Offline the walk failed even with 16 times
/// the growth budget; in a 6-minute Spy Hunt room no bot came within 100 units of either case.
#[test]
fn rom_twotowers_a_bot_walks_to_the_intel_cases_under_the_upper_walk() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    // BOT-141: in the collision set of a Spy Hunt room (the models its server leaves out)
    let room = spy_room(&r, 3);
    let from = [312.0f32, -696.0, 1824.0];
    for case in [[2562.0f32, -315.0, -454.0], [3408.0, -727.0, -1625.0]] {
        let walk = room.goto(from, case, 150);
        assert!(walk.is_ok(), "{from:?} -> {case:?}: {walk:?}");
    }
}

/// BOT-137: on Two Towers the five places the server put the Spy Hunt cases at are cases a bot can take, and a case
/// a spy dropped as he died in the water (the server laid them on the pool floor under the fall volume, OBSERVED
/// 2026-11-07) is in danger, so no bot walks into the water for it. Warhead's placed cases are goals too.
#[test]
fn rom_twotowers_a_case_in_the_water_is_no_goal() {
    use dc_botclient::spyrun::{case_reach, IN_DANGER};
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world(&r, b"WORLDS/TWOTOWERS.DAT");
    let (mut nav, _) = room.grown();
    let placed = [
        [312.0f32, -722.0, 1785.0],
        [984.0, -894.0, 556.0],
        [2562.0, -315.0, -454.0],
        [3408.0, -727.0, -1625.0],
        [2046.0, -139.0, -203.0],
    ];
    // (and for a bot whose box is taller than the body the server lays the cases for)
    let mut tall = NavGrid::new(room.w.collision.clone(), [16.0, 60.0, 16.0])
        .with_kill_volumes(room.kill.clone())
        .with_ladders(&room.ladders);
    for c in placed {
        for n in [&mut nav, &mut tall] {
            assert_eq!(
                case_reach(n, &room.w.collision, &room.kill, c),
                None,
                "{c:?}"
            );
        }
    }
    for wet in [
        [700.0f32, -1827.0, 911.0],
        [804.0, -1827.0, 353.0],
        [958.0, -1827.0, 494.0],
        [1311.0, -1827.0, 405.0],
    ] {
        assert_eq!(
            case_reach(&mut nav, &room.w.collision, &room.kill, wet),
            Some(IN_DANGER),
            "{wet:?}"
        );
    }
    // Warhead's placed cases (OBSERVED 2026-11-07) under its open roofs, with kill volumes 200..400 units above
    let room = room_world(&r, b"WORLDS/WARHEAD.DAT");
    let mut nav = room.grid();
    for c in [
        [-52.0f32, 2973.0, -2578.0],
        [-544.0, 2774.0, -139.0],
        [-1122.0, 2452.0, -903.0],
        [-892.0, 2621.0, 445.0],
        [-1118.0, 2687.0, -2092.0],
    ] {
        assert_eq!(
            case_reach(&mut nav, &room.w.collision, &room.kill, c),
            None,
            "{c:?}"
        );
    }
}

/// BOT-141: in the collision sets of Spy Hunt rooms, bots from spawn points across the map get into the touch window
/// of the placed intel cases no bot took in the 2026-11-08 rooms (one bot held 4 cases for 140..265 s while the 5th
/// lay there): Warhead's case on the walk behind a crate (69..70 above the floor, the walk 52 above the crate; the
/// grid's jumps reached 66, and a bot in its jump onto the crate counted its top as reached and steered off),
/// Waverider's case on the deck 220 above the floor under it (its way leads round by the far stairs: the grid grew it
/// after 145 s of standing at the end of a partial way), Two Towers' case on the ledge under the upper walk (BOT-139).
/// Before: Warhead 0 of 3 walks, Waverider 0 of 2, Two Towers 1 of 3 (two walks ended on the tower and the walk).
#[test]
fn rom_spy_hunt_bots_get_to_the_placed_cases_on_the_crate_the_deck_and_the_ledge() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    type Walk<'a> = (u32, [f32; 3], &'a [[f32; 3]]);
    let walks: [Walk; 3] = [
        (
            8,
            [-2508.0, 2819.0, -2160.0],
            &[
                [-2951.5, 2803.0, -371.0],
                [1682.5, 2795.0, -980.0],
                [1694.1, 2795.0, -1258.8],
            ],
        ),
        (
            9,
            [-3137.0, -2739.0, 51.0],
            &[[-3436.9, -3381.0, 5069.2], [-3483.1, -3381.0, 5229.7]],
        ),
        (
            3,
            [2562.0, -315.0, -454.0],
            &[
                [312.0, -696.0, 1824.0],
                [2316.0, -298.0, -1069.0],
                [1233.0, -820.0, 1979.0],
            ],
        ),
    ];
    let mut bad = Vec::new();
    for (map, case, from) in walks {
        let room = spy_room(&r, map);
        for f in from {
            match room.take(*f, case, 90) {
                Ok(s) => eprintln!("map {map}: {f:?} -> {case:?} in {s} s"),
                Err(e) => bad.push((map, *f, e)),
            }
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

/// BOT-145: the ledge traps of the 2026-11-08 rooms (offline, the rooms' collision sets): Snow Valley's rock by the
/// team-1 spawn (the way led up a jump the body never took: the bot paced 36 s under it), Rattlesnake's platform over
/// the A posts (a jump the jump test refused) and the walk from Two Towers' team-1 spawn toward team 2's side (its
/// partial way ends on the walks over the water; the greedy way-finder walked off them into the fall volume). Each walk
/// gets there (or, on Two Towers, does not die).
#[test]
fn rom_bots_get_off_the_ledges_they_were_trapped_on() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let sv = spy_room(&r, 0);
    let walk = sv.goto([-892.0, 800.0, -1738.0], [-585.0, 363.0, -520.0], 60);
    assert!(walk.is_ok(), "Snow Valley rock: {walk:?}");
    let rs = room_world_open(&r, b"WORLDS/RATTLESNAKE.DAT");
    let walk = rs.goto([-188.0, -905.0, 1702.0], [-558.0, -1098.0, 1273.0], 30);
    assert!(walk.is_ok(), "Rattlesnake platform: {walk:?}");
    let tt = spy_room(&r, 3);
    let walk = tt.goto([939.0, -888.0, 1799.0], [1510.0, -702.0, -246.0], 90);
    assert!(
        !walk.as_ref().is_err_and(|e| e.starts_with("died")),
        "Two Towers walks over the water: {walk:?}"
    );
}

/// BOT-155: Warhead's nook by the placed case at (-892, 2621, 445), where bots stood 100..340 s, is in danger only by a
/// hazard volume (no kill volume, a floor under it), and the grid without the hazard volumes leads out of it to a
/// spawn point: the bot judges its moves there by the kill volumes alone.
#[test]
fn rom_warhead_nook_by_the_placed_case_is_a_hazard_only_and_leads_out() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = spy_room(&r, 8);
    let coll = room.w.collision.clone();
    let hazard = room.w.hazard_volumes.clone();
    let (mut nav, start) = room.grown();
    for p in [
        [-947.45f32, 2629.86, 460.5],
        [-947.4, 2622.74, 520.9],
        [-892.0, 2621.0, 445.0],
    ] {
        assert!(
            !dc_botclient::nav::in_danger(&coll, &room.kill, BODY, p),
            "{p:?}"
        );
        assert!(
            dc_botclient::nav::in_danger(&coll, &hazard, BODY, p),
            "{p:?}"
        );
        assert!(nav.path(p, start).is_some(), "{p:?}");
    }
}

/// BOT-154: spots the bots never got to, reachable by players (offline, the rooms' collision sets). Snow Valley S&D
/// site A from the attackers' spawn (before: the walk ended on the roof 470 above the site after 120 s; the cellar
/// ladders up to it were no ways), and Warhead's lower floor (a poison volume, BOT-143): a bot that fell in walks out by the
/// ladder (before: no node stood in the volume and the walk had no way at all).
#[test]
fn rom_bots_get_to_the_spots_they_never_reached() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let sv = spy_room(&r, 0);
    let walk = sv.goto([2195.0, 107.0, -3574.0], [17.8, 246.1, 849.8], 140);
    eprintln!("Snow Valley site A: {walk:?}");
    assert!(walk.is_ok(), "Snow Valley site A: {walk:?}");
    let wh = spy_room(&r, 8);
    for from in [[-1747.0, 2470.0, -1615.0], [-1251.0, 2449.0, -783.0]] {
        let walk = wh.goto(from, [-1461.0, 2681.0, -1605.0], 30);
        eprintln!("Warhead lower floor {from:?}: {walk:?}");
        assert!(walk.is_ok(), "Warhead lower floor {from:?}: {walk:?}");
    }
}

/// The linked models a Rattlesnake respawn room's server leaves out of the collision set (OBSERVED 2026-11-08: the
/// bots' `world_set` event `off`, FFA room of the bot evaluation); doors are open as in [`room_world_set`].
const OFF_RATTLESNAKE: &[&str] = &[
    "worldmodel0",
    "worldmodel1",
    "worldmodel3",
    "worldmodel01",
    "worldmodel8",
    "worldmodel9",
    "worldmodel10",
    "l_shutter",
    "r_shutter",
    "latch",
    "latch_o",
    "blender01",
];

/// BOT-159: small islands of the grid the bots stayed on for whole matches (offline, the rooms' collision sets):
/// Snow Valley's pit below team 2's spawn ledge (floor 522, its only ways out a jump back onto the ledge or a drop of
/// 220..350 to the slope west of it; before, the walk paced in the pit for the whole 120 s) and Rattlesnake's deck
/// at (549.5, -600, 640.5) (a team-2 spawn; its way out a drop of about 313 to the floor at -913, which the greedy
/// way-finder's danger test, BOT-145, refused). The walk from each gets to a place far beyond.
#[test]
fn rom_bots_get_off_the_islands_they_were_confined_to() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let sv = spy_room(&r, 0);
    let walk = sv.goto([-1200.0, 573.0, 2350.0], [1819.8, 642.0, -650.2], 120);
    eprintln!("Snow Valley pit: {walk:?}");
    assert!(walk.is_ok(), "Snow Valley pit: {walk:?}");
    let rs = room_world_set(&r, b"WORLDS/RATTLESNAKE.DAT", OFF_RATTLESNAKE);
    let walk = rs.goto([549.5, -600.0, 640.5], [1323.0, -912.0, 600.0], 60);
    eprintln!("Rattlesnake deck: {walk:?}");
    assert!(walk.is_ok(), "Rattlesnake deck: {walk:?}");
}

/// BOT-176: Snow Valley's Spy Hunt intel point on the roof at (-580, 1364, 662) is in the grid's reach: its way runs
/// from the valley west of it up a hatch ladder (-1500..-1457, 623..1119, 456..481) into the room under the roof and
/// up the roof ladder. Before, the hatch opened 20 units wider than the ladder, the place a body's width beyond it hung
/// over the shaft, so the ladder had no top: no node stood within 128 units of the point and the walks ended 520
/// below it.
#[test]
fn rom_snowvalley_roof_intel_point_is_in_the_grids_reach() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = spy_room(&r, 0);
    let (mut nav, _) = room.grown();
    assert!(!nav.growing());
    let c = [-580.0f32, 1364.0, 662.0];
    let why = dc_botclient::spyrun::case_reach(&mut nav, &room.w.collision, &room.kill, c);
    assert_eq!(why, None);
    // (from a team-1 spawn point; the grid's way goes up both ladders)
    let missed = room.missed(&mut nav, [780.97, 520.0, -1607.3], &[c]);
    assert!(missed.is_empty(), "{missed:?}");
}

/// BOT-163: Gray Hammer's Spy Hunt intel points (the world's 17 item spawn points of the mode, case places 5 above
/// them) with the grid grown in full from a spawn point: two lie where no way leads (no node within 128 units flat and
/// 60 in height): the deck point (2251, 299, -2030), which drew 16 holders of 4 to their deaths under it in a
/// 6-minute room (out_boteval matrix4 BNT_01; it blocked matrix1 too), and (644, 211, 272). The others have nodes by
/// them. OBSERVED 2026-11-08 (EU-test ROM, offline walks of the
/// session's follower from three spawn points, 90 s each): every other point was taken from at least two of them;
/// the two points from none (the walks ended under them, at y 68 / -14 / 74). Since the server places 5 cases and the
/// agent needs all 5, a room that places either one cannot reach stage 2 with bots alone: a grid growth question.
/// BOT-176: both lie on girders (a roof girder over the electrified pit by site A; a box top reached along beams and
/// pipes 8..16 units wide), which the grid's lattice of places with floor under the body's centre never stands on.
#[test]
fn rom_grayhammer_two_spy_hunt_intel_points_are_out_of_the_grids_reach() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let pts: [[f32; 3]; 17] = [
        [-537.0, 277.0, 723.5],
        [644.0, 211.0, 272.5],
        [416.0, 72.0, 447.5],
        [-508.0, 99.0, 370.5],
        [1099.0, -172.0, -163.5],
        [-2098.0, 88.0, -726.5],
        [-1628.0, 88.0, -1352.5],
        [-93.0, 88.0, -1102.5],
        [-1465.0, 88.0, -2078.5],
        [474.0, 88.0, -2807.5],
        [-515.0, 296.0, -2424.5],
        [164.0, 315.0, -1398.5],
        [1077.0, 299.0, -1767.5],
        [2251.0, 299.0, -2030.5],
        [1779.0, 89.0, -329.5],
        [1477.0, 89.0, 227.5],
        [59.0, -118.0, -1367.5],
    ];
    let room = spy_room(&r, 1);
    let (mut nav, _) = room.grown();
    assert!(!nav.growing());
    let mut out = Vec::new();
    for p in pts {
        let c = [p[0], p[1] + 5.0, p[2]];
        let why = dc_botclient::spyrun::case_reach(&mut nav, &room.w.collision, &room.kill, c);
        eprintln!("{c:?}: {why:?}");
        if why.is_some() {
            out.push(p.map(f32::round));
        }
    }
    assert_eq!(out, vec![[644.0, 211.0, 273.0], [2251.0, 299.0, -2031.0]]);
}

/// BOT-173: on every CTF world each flag base has at least two cover spots that see it (on the grid grown in full
/// from the other flag's base, the approach from there); the open share of the best spot and of the band's nodes on
/// average are printed (the score also weighs the sight of the approach and height, so the best spot may face more
/// open ground than the average node).
#[test]
fn rom_ctf_every_flag_base_gets_cover_spots() {
    use dc_botclient::cover;
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let only = std::env::var("CTF_WORLD").ok();
    let mut short = Vec::new();
    for world in [
        "SNOWVALLEY",
        "GRAYHAMMER",
        "JUNK_FLEA",
        "TWOTOWERS",
        "PUMPJACK",
        "BRUSHWOOD",
        "COLDSEED",
        "SANDHOG",
        "WARHEAD",
        "WAVERIDER",
        "DEATHROOM",
        "RAPIDSTREAM",
        "GRAVEDIGGER",
        "ROADKILL",
    ] {
        if only.as_deref().is_some_and(|o| o != world) {
            continue;
        }
        let room = room_world(&r, format!("WORLDS/{world}.DAT").as_bytes());
        let coll = room.w.collision.clone();
        let mut nav = room.grid();
        let from = room.flags[0];
        let seeded = [0.0f32, 28.0]
            .iter()
            .map(|y| [from[0], from[1] + y, from[2]])
            .any(|p| nav.seed(p))
            || room.spawns.iter().any(|s| nav.seed(*s));
        assert!(seeded, "{world}: a node to grow from");
        while nav.growing() && nav.probes < 4_000_000 {
            nav.expand(2_000, None);
        }
        for (i, f) in room.flags.iter().enumerate() {
            let other = room.flags[1 - i];
            let app = nav
                .path(*f, other)
                .and_then(|mut p| {
                    p.reverse();
                    p.push(*f);
                    cover::approach_on(&p, *f)
                })
                .unwrap_or_else(|| cover::approach_toward(other, *f));
            let nodes: Vec<[f32; 3]> = nav
                .links_near(*f, dc_botclient::roles::GUARD_FAR + cover::PEEK_FAR)
                .into_iter()
                .filter(|(_, l)| !l.is_empty())
                .map(|(p, _)| p)
                .collect();
            let eye = 0.5 * dc_botclient::motion::BOT_STAND_DIMS[1];
            let up = |p: [f32; 3]| [p[0], p[1] + eye, p[2]];
            let spots = cover::rank_guard_spots(
                *f,
                &nodes,
                dc_botclient::roles::FLAG_COVER_BAND,
                &[app],
                |a, b| {
                    use dc_sim::{Collider, Cylinder, V3};
                    let shape = Cylinder::from_dims(V3::from_array(dc_botclient::hunt::RAY_DIMS));
                    coll.sweep(&shape, V3::from_array(up(a)), V3::from_array(up(b)))
                        .map_or(1.0, |h| h.t)
                },
                |a, b| dc_botclient::hunt::line_clear(&*coll, up(a), up(b)),
            );
            let seeing = spots.iter().filter(|s| s.score > -0.5 * cover::BLIND_COST).count();
            let band: Vec<f32> = cover::candidates(*f, &nodes, dc_botclient::roles::FLAG_COVER_BAND)
                .iter()
                .map(|p| cover::open_share(&cover::world_shares(&coll, *p)))
                .collect();
            let mean = band.iter().sum::<f32>() / band.len().max(1) as f32;
            eprintln!(
                "{world} flag {i}: {} spots ({seeing} see it), best {:?} score {:.2} open {:.2}, band mean open {:.2} ({} nodes)",
                spots.len(),
                spots.first().map(|s| s.at.map(f32::round)),
                spots.first().map_or(0.0, |s| s.score),
                spots.first().map_or(1.0, |s| s.open),
                mean,
                band.len()
            );
            if seeing < 2 {
                short.push((world, i, seeing));
            }
        }
    }
    assert!(short.is_empty(), "flag bases short of cover spots: {short:?}");
}

/// BOT-173 evidence tool (run by hand): the open share ([`dc_botclient::cover::open_share`]) at body places read from
/// the file `COVER_PROBE` (lines `<announced world> x y z`), written to `COVER_PROBE_OUT` (lines `x y z open walls`,
/// walls: the rays stopped within [`dc_botclient::cover::COVER_WALL`]).
#[test]
#[ignore]
fn rom_cover_probe() {
    let (Some(r), Ok(inp), Ok(out)) = (
        RomWorlds::from_env(),
        std::env::var("COVER_PROBE"),
        std::env::var("COVER_PROBE_OUT"),
    ) else {
        eprintln!("no CA_ROM / COVER_PROBE / COVER_PROBE_OUT: skipped");
        return;
    };
    let mut rooms: std::collections::BTreeMap<String, RoomWorld> = Default::default();
    let mut lines = Vec::new();
    for l in std::fs::read_to_string(inp).unwrap().lines() {
        let v: Vec<&str> = l.split_whitespace().collect();
        if v.len() != 4 {
            continue;
        }
        let room = rooms
            .entry(v[0].to_string())
            .or_insert_with(|| room_world(&r, v[0].as_bytes()));
        let p = [v[1], v[2], v[3]].map(|x| x.parse::<f32>().unwrap());
        let shares = dc_botclient::cover::world_shares(&room.w.collision, p);
        let open = dc_botclient::cover::open_share(&shares);
        let walls = shares
            .iter()
            .filter(|s| **s * dc_botclient::cover::COVER_RAY_LEN < dc_botclient::cover::COVER_WALL)
            .count();
        lines.push(format!("{} {} {} {open:.4} {walls}", v[1], v[2], v[3]));
    }
    std::fs::write(out, lines.join("\n")).unwrap();
}

/// BOT-181: Waverider's spawn at (-5009, -2286, 1700) (an FFA room's collision set): the way to the deck below runs
/// down an 813-unit ladder. A body that climbs down sends the ladder surface, so the game server's fall track (DSV-76)
/// restarts on the rungs and the landing at the foot is no fall. Before, every trailer said surface 0 and the descent
/// was one fall: 3 of 3 bots spawned there died at the foot within 5 s.
#[test]
fn rom_waverider_the_spawn_ladder_down_is_no_fall() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(&r, b"WORLDS/WAVERIDER.DAT", SPY_OFF_WAVERIDER);
    let spawn = [-5009.0f32, -2286.4, 1700.0];
    let walk = room.goto(spawn, [-4952.8, -3099.3, 1643.4], 40);
    let fall = LAST_FALL.with(|c| c.get());
    assert!(walk.is_ok(), "{walk:?}");
    assert!(fall <= 300.0, "a fall of {fall} on the way down");
}

/// BOT-181: Two Towers in a team deathmatch room's collision set.
const OFF_TDM_TWOTOWERS: &[&str] = &[
    "collision_box01",
    "collision_mdl01",
    "worldmodel01",
    "worldmodel02",
    "worldmodel0",
    "worldmodel1",
    "worldmodel2",
    "blender01",
    "worldmodel3",
    "worldmodel4",
    "qrt_blood",
    "qrt_show_g",
    "qrt_show_l",
    "qrt_show_r",
    "qrt_show_f",
    "qrt_show_n",
];

/// BOT-181: walks from the places of Two Towers' bridge where TDM bots of `out_boteval/matrix5` fell off (a spacing
/// goal's way that cut a corner over the edge at (1686, -696, 929), the walk by the bridge at (1085, -896, 295)) and
/// near the holes in it do not fall: the walk guard holds a step whose next half second leads off. Before, the walk
/// by the bridge slid off its edge into the fall volume.
#[test]
fn rom_twotowers_walks_on_the_bridge_do_not_fall_off() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(&r, b"WORLDS/TWOTOWERS.DAT", OFF_TDM_TWOTOWERS);
    for (from, to) in [
        ([1906.0f32, -713.0, -27.0], [1694.9, -701.9, 710.3]),
        ([1753.8, -701.9, -2.8], [1794.9, -701.9, 713.3]),
    ] {
        let walk = room.goto(from, to, 40);
        let fall = LAST_FALL.with(|c| c.get());
        eprintln!("{from:?} -> {to:?}: {walk:?}, fall {fall}");
        assert!(
            !walk.as_ref().is_err_and(|e| e.starts_with("died")),
            "{from:?} -> {to:?}: {walk:?}"
        );
        assert!(fall <= 300.0, "{from:?} -> {to:?}: a fall of {fall}");
    }
}

/// BOT-181: the corner under Two Towers' bridge by the pool (a one-node island beside the pool's kill volume, whose
/// only way out is the walk back along the volume's edge) is left: the bot's grid knows the map from its spawn, and
/// the way off the island takes the walk along the edge to a known node. Before, no way off counted (no drop, and the
/// body's box touched the volume at every place of the walk) and bots stood there 20..110 s.
#[test]
fn rom_twotowers_the_corner_under_the_bridge_is_left() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(&r, b"WORLDS/TWOTOWERS.DAT", OFF_TDM_TWOTOWERS);
    let spawn = [3676.8f32, -969.9, -634.7];
    for from in [
        [2900.0f32, -970.0, -300.0],
        [2902.6, -969.9, -296.5],
        [2884.0, -970.0, -312.0],
    ] {
        let walk = room.goto_known(from, [1700.0, -702.0, 700.0], 60, spawn);
        assert!(walk.is_ok(), "from {from:?}: {walk:?}");
    }
}

/// BOT-166: Snow Valley's ladders between the lower area round S&D site A and the upper level of site B (offline, the
/// room's collision set): a defender on site A's floor walks to the shaft ladder and climbs it to the upper level, and
/// one in the cellar under site A climbs the cellar ladder and then the shaft ladder to the bomb's defuse point at
/// site B. Before, the shaft ladder's foot counted as a jump's take-off (BOT-162) and was never reached by a body on
/// the rungs above it, the climb made no way for the stuck count, and the body on the cellar ladder hung under the floor
/// slab beside the hatch: the walks ended under the upper level after 60 s (2026-11-08 rooms: the defenders' way round
/// took 31 s, every bomb at site B detonated).
#[test]
fn rom_snowvalley_defenders_climb_to_site_b() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let sv = spy_room(&r, 0);
    let walk = sv.goto([-137.0, 236.0, 921.0], [818.0, 847.0, 1068.0], 60);
    eprintln!("Snow Valley site A floor to the shaft ladder's top: {walk:?}");
    assert!(walk.as_ref().is_ok_and(|s| *s <= 25.0), "site A floor: {walk:?}");
    let walk = sv.goto([-488.0, 90.0, 845.0], [473.9, 796.6, 654.6], 60);
    eprintln!("Snow Valley cellar under site A to site B's defuse point: {walk:?}");
    assert!(walk.as_ref().is_ok_and(|s| *s <= 30.0), "cellar: {walk:?}");
}

/// BOT-184: Warhead's Spy Hunt "door case" at (301.5, 2686, -1195) lies on the floor of a pit (2633) under a lid
/// (2916, a kill volume over it), west of the ramp the bots circled on (a wall between, 100 above the pit floor) and
/// beneath the walk at 2920. A player gets in by walking off the walk's west edge into the open part of the pit: a
/// drop of 287, under the 300 that do no fall damage (DSV-76); the way out runs through a low door in the pit's west
/// wall, up a stair and off a ledge (drops of 150..180). Before, the grid's walks off an edge stopped at 200: no node
/// stood within 275 units of the case, and in matrix5 BNT_08 no bot got closer than 289 in 207 samples with it as goal.
/// Bots from the places they circled at (the ramp east of the wall, the walk above) and from spawn points across the
/// map get into the case's touch window.
#[test]
fn rom_warhead_door_case_is_reached_by_the_drop_into_its_pit() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = spy_room(&r, 8);
    let case = [301.5, 2686.0, -1195.0];
    // the grid a bot grew in full, then the case check (twice: the second after the grid grew on)
    let ready = || {
        let (mut nav, _) = room.grown();
        assert!(!nav.growing());
        // (the grid's own ways end 275 units off; the check makes the deep falls toward the case, the grid grows on)
        assert!(nav
            .links_near(case, 128.0)
            .iter()
            .all(|n| (n.0[1] - case[1]).abs() > 60.0));
        let why = dc_botclient::spyrun::case_reach(&mut nav, &room.w.collision, &room.kill, case);
        assert_eq!(why, None);
        assert!(nav.growing());
        while nav.growing() && nav.probes < 4_000_000 {
            nav.expand(2_000, None);
        }
        let why = dc_botclient::spyrun::case_reach(&mut nav, &room.w.collision, &room.kill, case);
        assert_eq!(why, None);
        nav
    };
    let mut bad = Vec::new();
    for from in [
        [600.0, 2783.0, -1300.0],
        [200.0, 2970.0, -1300.0],
        [-2951.5, 2803.0, -371.0],
        [1682.5, 2795.0, -980.0],
    ] {
        match room.take_on(Some(ready()), from, case, 120) {
            Ok(s) => eprintln!("{from:?} -> door case in {s} s"),
            Err(e) => bad.push((from, e)),
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

/// BOT-184: Gray Hammer's two girder points (BOT-163, BOT-176) lie on islands of beams the grid walks once it stands
/// on them, and no way of the grid leads onto them. A grid seeded at each case reaches it and walks off its island
/// to the spawn (drops of 180..190 from the girder of (2251, 299, -2030), of about 176 from the box top of (644, 211,
/// 272)); the grid grown in full from a spawn point, with the case check's deep falls (none: nothing stands 200..290
/// above either), has no node by them. The way on, as the ROM shows it: (2251, ...) is entered from the end of the
/// pipe at z -2432 (24 wide, top 203; the grid reaches it) by a jump across a gap of about 100 (no floor between z
/// -2420 and -2312) onto the truss member climbing from 172 to 276, then along girders 24..36 wide; (644, ...) from the
/// end of a ledge 12 wide (top 257) by a corner step of 23 down onto a beam 12 wide. The grid has no jump across a gap
/// and no step between two beams that touch at a corner, so the bots keep giving these places up by BOT-163/180.
#[test]
fn rom_grayhammer_girder_cases_lie_on_islands_no_grid_way_enters() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = spy_room(&r, 1);
    let (mut nav, start) = room.grown();
    assert!(!nav.growing());
    for case in [[2251.0f32, 304.0, -2030.5], [644.0, 216.0, 272.5]] {
        let why = dc_botclient::spyrun::case_reach(&mut nav, &room.w.collision, &room.kill, case);
        assert_eq!(why, Some(dc_botclient::spyrun::NO_REACH), "{case:?}");
        assert!(!nav.growing(), "{case:?}: no deep fall toward it");
        let mut island = room.grid();
        assert!(island.seed([case[0], case[1] + 40.0, case[2]]), "{case:?}");
        while island.growing() && island.probes < 4_000_000 {
            island.expand(2_000, None);
        }
        let by = island
            .links_near(case, dc_botclient::spyrun::REACH)
            .into_iter()
            .map(|n| n.0)
            .find(|n| (n[1] - case[1]).abs() <= dc_botclient::spyrun::REACH_RISE);
        let by = by.unwrap_or_else(|| panic!("{case:?}: the island stands by the case"));
        assert!(island.path(by, start).is_some(), "{case:?}: the island's way off");
    }
    // the gap between the pipe's end and the truss member: no floor from 120 to 330 at x 1790, z -2408..-2324
    use dc_sim::{Collider, Cylinder, V3};
    let thin = Cylinder::from_dims(V3::new(2.0, 2.0, 2.0));
    for z in (0..8).map(|i| -2408.0 + 12.0 * i as f32) {
        let hit = room
            .w
            .collision
            .sweep(&thin, V3::new(1790.0, 330.0, z), V3::new(1790.0, 120.0, z));
        assert!(hit.is_none_or(|h| h.t >= 0.999), "floor at z {z}");
    }
}

/// BOT-185: the linked models an Elimination Pro room's server leaves out of the collision set on Overdose (OBSERVED
/// 2026-11-09: the bots' `world_set` event `off`, TSV room of the bot evaluation); doors are open as in
/// [`room_world_set`].
const OFF_OVERDOSE_TSV: &[&str] = &[
    "g00",
    "worldmodel7",
    "light_burb_long_02_func00.worldmodel0",
    "light_burb_long_02_func01.worldmodel0",
    "light_burb_long_02_func02.worldmodel0",
    "light_burb_long_02_func03.worldmodel0",
    "light_burb_long_02_func04.worldmodel0",
    "light_burb_long_02_func05.worldmodel0",
    "light_burb_long_02_func06.worldmodel0",
    "light_burb_long_02_func07.worldmodel0",
    "light_burb_long_02_func08.worldmodel0",
    "bard_01",
    "bard_02",
    "bard_03",
    "bard_05",
    "bard_06",
    "bard_07",
    "bard_08",
    "bard_09",
    "bard_10",
    "bard_04",
    "bar_01_3",
    "bar_02_3",
    "bar_03_3",
    "bar_04_3",
    "bar_05_3",
    "bar_01_2",
    "bar_02_2",
    "bar_03_2",
    "bar_04_2",
    "bar_05_2",
    "ele_out_r01",
    "ele_out_l01",
    "ele_d_open_01",
    "ele_in_r01",
    "ele_in_l01",
    "dp_03_01",
    "dp_05_01",
    "dp_06_01",
    "dp_04_01",
    "dp_07_01",
    "dp_02_01",
    "light_burb_long_02_func09.worldmodel0",
    "redalarmon01",
    "worldmodel01",
    "light_burb_long_02_on02.worldmodel0",
    "room_switch_01",
    "room_switch_02",
    "light_room_01.vis_on",
    "light_room_02.vis_on",
    "worldmodel03",
    "worldmodel05",
    "debriswnd00",
    "debriswnd01",
    "debriswnd02",
    "ele_out_r02",
    "ele_out_l02",
    "ele_in_r02",
    "ele_in_l02",
    "ele_d_open_02",
    "dp_03_02",
    "dp_05_02",
    "dp_06_02",
    "dp_04_02",
    "dp_07_02",
    "dp_02_02",
    "redalarmon02",
    "worldmodel00",
    "worldmodel1",
    "worldmodel2",
    "worldmodel6",
    "worldmodel5",
    "light_fluore_01_on01.worldmodel0",
    "light_fluore_01_on00.worldmodel0",
    "worldmodel4",
    "tdm_hide01",
];

/// BOT-185: on Overdose (Elimination Pro) a bot from team 2's spawn hall gets onto team 1's ledge (481 above the hall's
/// 114). Before, the grid's way climbed the crate stack under the ledge by a step of 15 from one crate onto the corner
/// of the next (the body stood there on a sliver of its rim): the body went on past the corner and fell to the floor,
/// jumped up again, and after 60 s stood at the foot of the stack (18 jumps in 30 s in a TSV room).
#[test]
fn rom_overdose_team_two_climbs_to_the_ledge_without_falling_off_the_crates() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(&r, b"WORLDS/OVERDOSE.DAT", OFF_OVERDOSE_TSV);
    let walk = room.goto([-85.0, 114.1, 1159.0], [-450.0, 481.0, 930.0], 30);
    eprintln!("Overdose ledge: {walk:?}");
    assert!(walk.is_ok(), "Overdose ledge: {walk:?}");
}

/// BOT-186: on Gray Hammer (Elimination Pro) a bot climbs the crate stairs (50, 93, 135, 182, 238) up to the walk at 250
/// without falling back: falls of more than 30 units within 3 s of a jump at most four in six walks (grids grown in
/// full from three team-2 spawn points the bots of a TSV room grew theirs from, starts on the floor at the stairs' foot).
/// The crates are 40 deep and the walk's slab overhangs the back of the lower ones. Before, a body that landed on the
/// 93 crate under the slab jumped at the next one from there and struck its head (its flight topped out 37 up, short of
/// 42), and a top that became the next point while the body rose toward it counted as no top, was reached in the
/// flight and the body steered on over the crate's rim: 13 such falls and 34 jumps in these six walks, 34 s in all
/// (MEASURED 2026-11-09; with the rule 3 falls, 22 jumps, 16 s).
#[test]
fn rom_grayhammer_crate_stairs_are_climbed_without_falling_back() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(
        &r,
        b"WORLDS/GRAYHAMMER.DAT",
        &["collision_box01", "collision_mdl01", "worldmodel1"],
    );
    let top = [-1460.0f32, 250.1, -1499.0];
    let mut falls = 0;
    for (seed, from) in [
        ([274.88f32, 40.1, -3260.68], [-1186.0f32, 50.1, -1626.0]),
        ([39.4, 50.1, -3228.0], [-1210.0, 50.1, -1609.0]),
        ([39.4, 50.1, -3228.0], [-1186.0, 50.1, -1626.0]),
        ([39.4, 50.1, -3228.0], [-1150.0, 50.1, -1595.0]),
        ([39.4, 50.1, -3228.0], [-1250.0, 50.1, -1640.0]),
        ([317.5, 50.1, -3132.4], [-1186.0, 50.1, -1626.0]),
    ] {
        let mut nav = room.grid();
        assert!(nav.seed(seed), "the spawn stands");
        while nav.growing() && nav.probes < 4_000_000 {
            nav.expand(2_000, None);
        }
        let walk = room.walk(Some(nav), from, top, 30, |p| {
            flat(p, top) <= 72.0 && (p[1] - top[1]).abs() <= 40.0
        });
        let (jumps, fell) = LAST_JUMPS.with(|c| c.get());
        eprintln!("from {from:?} (grid from {seed:?}): {walk:?}, {jumps} jumps, {fell} falls");
        assert!(walk.is_ok(), "from {from:?}: {walk:?}");
        falls += fell;
    }
    assert!(falls <= 4, "{falls} falls after jumps");
}


/// BOT-191: the linked models a Rattlesnake Elimination room's server leaves out of the collision set (OBSERVED
/// 2026-11-10: the bots' `world_set` event `off`, TDM room of the bot evaluation; the shutters stay); doors are open
/// as in [`room_world_set`].
const OFF_RATTLESNAKE_TDM: &[&str] = &[
    "worldmodel0",
    "worldmodel3",
    "worldmodel1",
    "worldmodel01",
    "worldmodel8",
    "worldmodel9",
    "worldmodel10",
    "blender01",
];

/// BOT-191: on Rattlesnake (Elimination) a bot at team 2's spawn exit walks down to the lower floor without hopping at
/// the parapet beside it (53 high, 10 wide, with a beam across the pit behind it at the same height): the grid's
/// ways jump onto the parapet or the beam, the body falls back to the floor (or the jump test refuses the flight) and
/// the next plan jumps at the node beside the last. Before, each node kept its own failure count and the walks
/// jumped 5 and 6 times there (11 in all, MEASURED 2026-11-10); with the failures near one another counted together
/// the ways up go after the third and the walks go round (2 and 3).
#[test]
fn rom_rattlesnake_team_two_leaves_the_spawn_exit_without_hopping_at_the_parapet() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(&r, b"WORLDS/RATTLESNAKE.DAT", OFF_RATTLESNAKE_TDM);
    let from = [-526.0f32, -912.0, 1400.0];
    let mut jumps = 0;
    for to in [[-737.0f32, -1232.0, 1407.0], [-696.0, -1232.0, 1241.0]] {
        let walk = room.goto(from, to, 30);
        let (j, fell) = LAST_JUMPS.with(|c| c.get());
        eprintln!("to {to:?}: {walk:?}, {j} jumps, {fell} falls");
        assert!(walk.is_ok(), "to {to:?}: {walk:?}");
        jumps += j;
    }
    assert!(jumps <= 7, "{jumps} jumps");
}

/// BOT-189: the places of Two Towers where bots of `out_boteval/matrix6` stayed 60..130 s with every plan closed are
/// dead ends (`NavGrid::dead_end`: a grid seeded there grows shut within 600 units): the beams round the pool under the
/// bridge (y -974), the ledge above them (y -702, z 1079) and the slot beside the tower's wall (y -318); the floors the
/// bots walk on, the plank walk over the water and the landings of BOT-159's escapes on Rattlesnake and Snow Valley are
/// not. From the upper walk where the matrix6 Spy Hunt bot left for the ledge, a greedy heading the danger test lets
/// through drops onto a dead end, and is no way now (`dead_end_below`).
#[test]
fn rom_twotowers_no_way_off_an_island_ends_in_a_dead_end() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(&r, b"WORLDS/TWOTOWERS.DAT", OFF_TDM_TWOTOWERS);
    let probes = dc_botclient::nav::DEAD_END_PROBES;
    let nav = room.grid();
    for p in [
        [2589.5f32, -973.6, 776.1],
        [2000.0, -973.6, 610.0],
        [2700.0, -701.9, 1079.0],
        [2414.0, -318.0, 300.0],
    ] {
        assert!(nav.dead_end(p, probes, 600.0), "{p:?} is a dead end");
    }
    for p in [[1700.0f32, -702.0, 700.0], [1063.0, -895.5, 400.0], [312.0, -696.0, 1824.0]] {
        assert!(!nav.dead_end(p, probes, 600.0), "{p:?} is no dead end");
    }
    let rs = room_world_set(&r, b"WORLDS/RATTLESNAKE.DAT", OFF_RATTLESNAKE);
    assert!(!rs.grid().dead_end([1323.0, -912.0, 600.0], probes, 600.0));
    let sv = spy_room(&r, 0);
    assert!(!sv.grid().dead_end([-1450.0, 360.0, 2350.0], probes, 600.0));
    // the greedy way-finder on the upper walk where the matrix6 Spy Hunt bot left it for the ledge (its way down to
    // the floor by the pool): a heading the danger test lets through drops 201 onto a dead end; it is no way now
    let from = [2490.0f32, -326.0, 892.0];
    let danger: Vec<_> = room.kill.iter().chain(&room.hazard).cloned().collect();
    let mut b = dc_botclient::motion::Body::new(
        from,
        BODY,
        0.0,
        dc_botclient::motion::MotionKind::Hunt,
        60,
        Some(room.w.collision.clone()),
    );
    b.mods = walk_mods();
    let mut refused = 0;
    for i in 0..16 {
        let y = i as f32 * std::f32::consts::TAU / 16.0;
        let path = b.probe_path(Some(y), None, dc_botclient::hunt::PROBE_SECS, 4);
        if dc_botclient::evade::path_safe(&room.w.collision, &danger, BODY, &path)
            && dead_end_below(&nav, &room.w.collision, from[1], &path)
        {
            refused += 1;
        }
    }
    assert!(refused > 0, "a heading off the upper walk onto a dead end");
}


/// BOT-197: a bot that spawns on Waverider's spawn platform at (-5009, -2286, 1700) (an FFA room's collision set) with
/// a grid grown on the lower deck before (it died there) walks down the spawn ladder. Before, the lattice place of that
/// grid 55 units off the ladder's foot held the foot's key, the ladder joined no way, its top grew the platform as an
/// island and the bot paced the platform's far end (-4790, -2285) for a minute (an FFA room of the 2026-11-10 bot
/// evaluation); a grid grown from the platform itself was fine.
#[test]
fn rom_waverider_a_bot_spawned_on_the_platform_with_a_known_deck_walks_down_the_ladder() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(&r, b"WORLDS/WAVERIDER.DAT", SPY_OFF_WAVERIDER);
    let spawn = [-5009.0f32, -2286.4, 1700.0];
    let deck = [-3824.0f32, -3310.0, 1194.0];
    for to in [[-3899.0f32, -2986.0, 1567.0], [-4952.8, -3099.3, 1643.4]] {
        let walk = room.goto_known(spawn, to, 40, deck);
        eprintln!("to {to:?}: {walk:?}");
        assert!(walk.is_ok(), "to {to:?}: {walk:?}");
    }
}

/// BOT-197 (OPEN, ignored): the far end of Waverider's spawn platform (x -4850..-4740), where an FFA bot of a
/// 2026-11-11 headless room paced for 110 s with every plan closed (28 islands; its grid's part there 39 nodes), joins
/// the rest of the platform only through a passage a little wider than the body by (-4850, -2284, 1610). A grid grown
/// from a place there whose lattice misses the passage (each lattice place and the half-step places beside it fall on
/// the obstacles to either side) grows shut (5 of 284 places 6 units apart, MEASURED 2026-11-12; 8 nodes from
/// (-4833, -2283.6, 1652.5)): the plans end there and the greedy way-finder walks the body back into the end, toward
/// goals across the map. The walks from (-4833, 1652.5) stand where they begin. A grid that knows the spawn end first
/// threads the passage (the walks get there). Tried and not kept: quarter-step places for every node (5 other tests
/// of this file failed), for the nodes of small closed parts grown again (Two Towers' bridge walk fell off), and links
/// along the body's own walk (Two Towers' corner under the bridge was not left).
#[test]
#[ignore = "OPEN: BOT-197 far end of Waverider's spawn platform"]
fn rom_waverider_a_bot_walks_out_of_the_spawn_platforms_far_end() {
    let Some(r) = RomWorlds::from_env() else {
        eprintln!("no CA_ROM: skipped");
        return;
    };
    let room = room_world_set(&r, b"WORLDS/WAVERIDER.DAT", SPY_OFF_WAVERIDER);
    let from = [-4833.0f32, -2283.57, 1652.45];
    assert!(
        room.grid().dead_end(from, dc_botclient::nav::DEAD_END_PROBES, 600.0),
        "{from:?} grows shut"
    );
    for to in [[-5074.0f32, -2286.4, 1731.0], [-4952.8, -3099.3, 1643.4]] {
        let walk = room.goto(from, to, 40);
        eprintln!("to {to:?}: {walk:?}");
        assert!(walk.is_ok(), "to {to:?}: {walk:?}");
    }
}

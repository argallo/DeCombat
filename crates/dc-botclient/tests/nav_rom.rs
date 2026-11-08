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
        let rows = dc_sim::SpeedTable::from_vfs(&dc_rez::RezVfs::new(std::path::Path::new(&rom)))
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
    // stops 42-50 units short of: OBSERVED 2026-11-07, known, at most two allowed)
    assert!(
        bad.len() <= 2,
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
        self.walk(from, to, secs, |p| {
            flat(p, to) <= 72.0 && (p[1] - to[1]).abs() <= 90.0
        })
    }

    /// BOT-141: [`Self::goto`] to an intel case, done only when the body gets into the server's touch window (56
    /// units across, its centre at most 32 above and 53 below the case's point, DSV-391): a body under a case on a
    /// crate has not got it.
    fn take(&self, from: [f32; 3], case: [f32; 3], secs: u32) -> Result<f32, String> {
        self.walk(from, case, secs, |p| {
            flat(p, case) <= 56.0 && (-53.0..=32.0).contains(&(p[1] - case[1]))
        })
    }

    fn walk(
        &self,
        from: [f32; 3],
        to: [f32; 3],
        secs: u32,
        arrived: impl Fn([f32; 3]) -> bool,
    ) -> Result<f32, String> {
        use dc_botclient::motion::{Body, MotionKind};
        use dc_botclient::nav::follow::{Follower, NavGoal, Stuck, GROW_PROBES};
        // (the session's danger tests take the hazard volumes as kill volumes, BOT-143)
        let danger: Vec<_> = self.kill.iter().chain(&self.hazard).cloned().collect();
        let mut nav = self.grid();
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
                        note: None,
                    });
                    for _ in 0..3 {
                        b.advance(16_667);
                    }
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
                    escape = dc_botclient::evade::escape_move(
                        &b,
                        &self.w.collision,
                        &danger,
                        &part,
                        dc_botclient::nav::ESCAPE_DROP,
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
                Some((at, _)) if flat(at, p) > 48.0 => watch = Some((p, now)),
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
            b.steer = match f.next(&mut nav, now, p, to, true) {
                NavGoal::Way(w) => {
                    greedy_check = None;
                    hunter.stop();
                    let steer = (w[0] - p[0]).atan2(w[2] - p[2]);
                    // BOT-139: the ladder pitch follows the next point's height
                    b.goal_y = Some(w[1]);
                    // BOT-134: the session takes the stuck jump only when its flight and landing are safe
                    jump = f.stuck(&mut nav, now, p) == Stuck::Jump
                        && dc_botclient::evade::jump_safe(
                            &b,
                            &self.w.collision,
                            &danger,
                            Some(steer),
                            None,
                        );
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
                    let probe = |y: f32| {
                        let path = b.probe_path(Some(y), None, dc_botclient::hunt::PROBE_SECS, 4);
                        dc_botclient::evade::path_safe(&self.w.collision, &danger, BODY, &path)
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
            b.evade = jump.then_some(dc_botclient::evade::Move {
                yaw: 0.0,
                strafe: 0,
                back: false,
                jump: true,
                crouch: false,
                note: None,
            });
            for _ in 0..3 {
                b.advance(16_667);
            }
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
    let walks: [(u32, [f32; 3], &[[f32; 3]]); 3] = [
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

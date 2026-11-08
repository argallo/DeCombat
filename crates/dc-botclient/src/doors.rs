//! BOT-120: bots open doors the way a player does. The grid treats a door's place as open floor (a door is a way
//! that opens, not a wall); a bot whose way runs through a closed door stops at it, faces it and sends the use key,
//! `UP_USE` with no object, as the DeCombat client does when no dropped weapon is in reach; the server toggles the
//! nearest player-activated door in its reach (DSV-270 / DSV-460). A door that does not open (locked, opened only by
//! a lever, or one no player near it moves) is given up for a while: the grid takes it as a wall again and the bot
//! goes round. A DeCombat bot behaviour, not a 2009 rule.

use std::sync::Arc;

use dc_net_engine::Millis;
use dc_sim::{CollModel, CollisionWorld};
use dc_wire::udp::BitWriter;

use crate::objects::{kind, ObjectTable};
use crate::romworld::LoadedWorld;

/// `UP_USE` (0x59) `[target:objref16][phase:u8]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §7.1 (0x59 layout)
// DC-DEFAULT: CLI-1133 (proposed) the use key with no drop in reach names no object; the server finds the door
pub const UP_USE: u8 = 0x59;
/// The `UP_USE` target of a press with no object.
// DC-DEFAULT: CLI-1133 (proposed)
pub const USE_NO_OBJECT: u16 = 0xffff;
/// Shell object type of a door's create blob `[row:u8][disabled:bool1][state:u8]` (the activate entry).
// DC-DEFAULT: DSV-340 (proposed) every door's create blob carries its activate entry; DSV-46 (blob type 0x39)
pub const DOOR_BLOB_TYPE: u8 = 0x39;

/// A bot takes up a closed door on its way within this distance of the door's closed box (the server's use reach is
/// 128 units from the box the door sweeps, DSV-270).
// DC-DEFAULT: BOT-120 (proposed) 96 units
pub const DOOR_NEAR: f32 = 96.0;
/// A door taken up is let go when the bot is farther than this from its box (it walked off, was pushed away).
// DC-DEFAULT: BOT-120 (proposed) 160 units
pub const DOOR_LEAVE: f32 = 160.0;
/// The part of the way checked for a door: as far as this from the bot.
// DC-DEFAULT: BOT-120 (proposed) 160 units
pub const PATH_LOOK: f32 = 160.0;
/// A leg of the way crosses a door when it passes within this of the door's box (x, y, z): the body's half width,
/// and the standing body's half height (the way runs at the body's centre).
// DC-DEFAULT: BOT-120 (proposed)
pub const PATH_PAD: [f32; 3] = [20.0, 48.0, 20.0];
/// Time between two presses at the same door (the use key toggles: a second press while it opens would close it).
// DC-DEFAULT: BOT-120 (proposed) 1500 ms
pub const PRESS_GAP_MS: u32 = 1500;
/// Presses without the door moving before it is given up.
// DC-DEFAULT: BOT-120 (proposed) 3
pub const MAX_PRESSES: u32 = 3;
/// How long a bot waits at a door that has no use entry (a door a near player opens moves by itself).
// DC-DEFAULT: BOT-120 (proposed) 4000 ms
pub const WAIT_MS: u32 = 4000;
/// How long a given-up door counts as a wall on the grid.
// DC-DEFAULT: BOT-120 (proposed) 30 s
pub const GIVE_UP_MS: u32 = 30_000;
/// The longest first press delay, picked per bot (two bots at one door would press within a tick: open, then shut).
// DC-DEFAULT: BOT-120 (proposed) 0..600 ms
pub const JITTER_MAX_MS: u32 = 600;
/// "Near a closed door" for the statistic (every mode, also with doors off).
pub const STAT_NEAR: f32 = 48.0;

/// The `UP_USE` body of a press with no object, phase 0 (pressed).
// SPEC: GAME_MESSAGES_2009_C2S.md §7.1
pub fn use_body() -> BitWriter {
    let mut b = BitWriter::new();
    b.write_objref16(Some(USE_NO_OBJECT)).write_u8(0);
    b
}

/// Whether a door's activate entry is enabled (the file marks it player-activated and it was not locked): `None`
/// when the blob is no door blob.
// DC-DEFAULT: DSV-340 (proposed) `[row:u8][disabled:bool1][state:u8]` after the type byte
pub fn use_enabled(blob: &BitWriter) -> Option<bool> {
    let mut r = blob.reader();
    if r.remaining_bits() < 8 + 8 + 1 || r.read_u8() != DOOR_BLOB_TYPE {
        return None;
    }
    r.skip_bits(8);
    Some(!r.read_bit())
}

/// A door or switch of the loaded world: its name (lower case), its place in [`LoadedWorld::linked`] and its box as
/// the file places it (closed).
#[derive(Debug, Clone, PartialEq)]
pub struct DoorBox {
    pub name: Vec<u8>,
    pub linked: usize,
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

impl DoorBox {
    pub fn distance(&self, p: [f32; 3]) -> f32 {
        (0..3)
            .map(|k| (self.lo[k] - p[k]).max(p[k] - self.hi[k]).max(0.0).powi(2))
            .sum::<f32>()
            .sqrt()
    }

    pub fn centre(&self) -> [f32; 3] {
        [0, 1, 2].map(|k| 0.5 * (self.lo[k] + self.hi[k]))
    }

    /// Whether the leg `a` -> `b` passes through the box grown by [`PATH_PAD`] (slab test).
    pub fn crossed_by(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for k in 0..3 {
            let (lo, hi) = (self.lo[k] - PATH_PAD[k], self.hi[k] + PATH_PAD[k]);
            let d = b[k] - a[k];
            if d.abs() < 1e-6 {
                if a[k] < lo || a[k] > hi {
                    return false;
                }
                continue;
            }
            let (mut u, mut v) = ((lo - a[k]) / d, (hi - a[k]) / d);
            if u > v {
                std::mem::swap(&mut u, &mut v);
            }
            t0 = t0.max(u);
            t1 = t1.min(v);
            if t0 > t1 {
                return false;
            }
        }
        true
    }
}

/// The doors and switches of a loaded world (the movers among its linked models).
pub fn door_boxes(w: &LoadedWorld) -> Vec<DoorBox> {
    w.linked
        .iter()
        .enumerate()
        .filter(|(_, l)| l.mover)
        .filter_map(|(i, l)| {
            let m = w.full.models.get(l.index)?;
            let (lo, hi) = m.bounds();
            (lo.x <= hi.x).then(|| DoorBox {
                name: l.name.clone(),
                linked: i,
                lo: lo.to_array(),
                hi: hi.to_array(),
            })
        })
        .collect()
}

/// What the bot sees of one door this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DoorView {
    /// It stands where the server created it (BOT-110: solid).
    pub closed: bool,
    /// Its activate entry is enabled: the use key turns it.
    pub usable: bool,
}

/// The views of `boxes` from the server's objects (the door's named brush) and the world's solid set.
pub fn door_views(w: &LoadedWorld, table: &ObjectTable, boxes: &[DoorBox]) -> Vec<DoorView> {
    boxes
        .iter()
        .map(|d| {
            let closed = w.solid.as_ref().is_none_or(|s| s[d.linked]);
            let usable = table
                .objects
                .values()
                .filter(|o| o.kind == kind::BRUSH)
                .find(|o| {
                    o.name
                        .as_ref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(&d.name))
                })
                .and_then(|o| o.blob.as_ref().and_then(use_enabled))
                .unwrap_or(false);
            DoorView { closed, usable }
        })
        .collect()
}

/// Which linked models the grid takes as solid: the world's solid set, with every door taken as open floor unless
/// the bot gave it up (then it is as solid as it stands). With doors off: the solid set unchanged.
pub fn nav_key(w: &LoadedWorld, boxes: &[DoorBox], given_up: &[bool], on: bool) -> Vec<bool> {
    let mut key: Vec<bool> = match &w.solid {
        Some(s) => s.clone(),
        None => vec![true; w.linked.len()],
    };
    if on {
        for (i, d) in boxes.iter().enumerate() {
            if !given_up.get(i).copied().unwrap_or(false) {
                key[d.linked] = false;
            }
        }
    }
    key
}

/// The grid's collision set for `key` (from [`nav_key`]).
pub fn nav_collision(w: &LoadedWorld, key: &[bool]) -> Arc<CollisionWorld> {
    let solid_now = |i: usize| w.solid.as_ref().is_none_or(|s| s[i]);
    if (0..w.linked.len()).all(|i| key[i] == solid_now(i)) {
        return w.collision.clone();
    }
    let mut c = (*w.full).clone();
    for (i, l) in w.linked.iter().enumerate() {
        if !key[i] && l.index < c.models.len() {
            c.models[l.index] = CollModel::new(&[], &[], None);
        }
    }
    Arc::new(c)
}

/// What the door logic asks of the bot this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    /// The door taken up (index into the boxes).
    pub door: usize,
    /// Face the door (yaw, as the walk's heading).
    pub yaw: f32,
    /// Send the use key now.
    pub press: bool,
}

/// What happened at a door (for the bot's log).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DoorNote {
    Press { door: usize, n: u32 },
    Opened { door: usize, waited_ms: u32 },
    GaveUp { door: usize, presses: u32 },
    Back { door: usize },
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct DoorState {
    presses: u32,
    last_press: Option<Millis>,
    given_up_at: Option<Millis>,
}

/// Counters for the bot's report.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DoorStats {
    pub presses: u32,
    pub opened: u32,
    pub given_up: u32,
    /// Time standing at a door waiting for it.
    pub wait_ms: u64,
    /// Time alive with a way to walk within [`STAT_NEAR`] of a closed door (every mode).
    pub near_closed_ms: u64,
}

/// One bot's door logic.
#[derive(Debug, Clone, Default)]
pub struct DoorMind {
    pub boxes: Vec<DoorBox>,
    state: Vec<DoorState>,
    cur: Option<(usize, Millis)>,
    jitter_ms: u32,
    last: Option<Millis>,
    pub stats: DoorStats,
}

fn since(now: Millis, t: Millis) -> u32 {
    now.wrapping_sub(t)
}

impl DoorMind {
    pub fn new(boxes: Vec<DoorBox>, seed: u64) -> DoorMind {
        DoorMind {
            state: vec![DoorState::default(); boxes.len()],
            boxes,
            jitter_ms: (seed % u64::from(JITTER_MAX_MS)) as u32,
            ..DoorMind::default()
        }
    }

    /// Which doors are given up now (the grid takes them as walls).
    pub fn given_up(&self) -> Vec<bool> {
        self.state.iter().map(|s| s.given_up_at.is_some()).collect()
    }

    /// The closed door a way from `pos` along `path` meets first within [`PATH_LOOK`] (within [`DOOR_NEAR`] of the
    /// bot).
    fn on_way(&self, pos: [f32; 3], path: &[[f32; 3]], views: &[DoorView]) -> Option<usize> {
        let mut from = pos;
        let mut left = PATH_LOOK;
        for &p in path.iter().take(4) {
            let d = (0..3).map(|k| (p[k] - from[k]).powi(2)).sum::<f32>().sqrt();
            let to = if d > left {
                let f = left / d.max(1e-3);
                [0, 1, 2].map(|k| from[k] + (p[k] - from[k]) * f)
            } else {
                p
            };
            let hit = self
                .boxes
                .iter()
                .enumerate()
                .filter(|(i, b)| {
                    views.get(*i).is_some_and(|v| v.closed)
                        && self.state[*i].given_up_at.is_none()
                        && b.distance(pos) <= DOOR_NEAR
                        && b.crossed_by(from, to)
                })
                .min_by(|a, b| a.1.distance(pos).total_cmp(&b.1.distance(pos)))
                .map(|(i, _)| i);
            if hit.is_some() {
                return hit;
            }
            left -= d;
            if left <= 0.0 {
                break;
            }
            from = p;
        }
        None
    }

    /// One tick. `alive`: the bot plays and lives; `path`: the way it walks (its waypoints, else its goal; empty
    /// when it does not walk); `views`: [`door_views`]; `on`: false = only the statistic (doors as walls, as before).
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        now: Millis,
        alive: bool,
        pos: [f32; 3],
        path: &[[f32; 3]],
        views: &[DoorView],
        on: bool,
        notes: &mut Vec<DoorNote>,
    ) -> Option<Hold> {
        let dt = self.last.map_or(0, |t| since(now, t).min(250)) as u64;
        self.last = Some(now);
        // a given-up door comes back after a while
        for (i, s) in self.state.iter_mut().enumerate() {
            if s.given_up_at.is_some_and(|t| since(now, t) >= GIVE_UP_MS) {
                *s = DoorState::default();
                notes.push(DoorNote::Back { door: i });
            }
        }
        if !alive {
            self.cur = None;
            return None;
        }
        if !path.is_empty()
            && self
                .boxes
                .iter()
                .zip(views)
                .any(|(b, v)| v.closed && b.distance(pos) <= STAT_NEAR)
        {
            self.stats.near_closed_ms += dt;
        }
        if !on || self.boxes.is_empty() {
            return None;
        }
        // the door taken up: opened (walk on), let go (walked off), or still to wait for
        if let Some((i, since_at)) = self.cur {
            let open = !views.get(i).is_some_and(|v| v.closed);
            if open {
                self.cur = None;
                self.state[i].presses = 0;
                self.stats.opened += 1;
                notes.push(DoorNote::Opened {
                    door: i,
                    waited_ms: since(now, since_at),
                });
                return None;
            }
            if self.boxes[i].distance(pos) > DOOR_LEAVE || path.is_empty() {
                self.cur = None;
            }
        }
        let i = match self.cur {
            Some((i, _)) => i,
            None => {
                let i = self.on_way(pos, path, views)?;
                self.cur = Some((i, now));
                i
            }
        };
        let since_at = self.cur.map_or(now, |c| c.1);
        self.stats.wait_ms += dt;
        let usable = views[i].usable;
        // the use key turns the nearest use door in reach: press only when that one is closed (never shut an open
        // one, maybe a team mate's)
        let nearest_usable_closed = self
            .boxes
            .iter()
            .zip(views)
            .filter(|(_, v)| v.usable)
            .min_by(|a, b| a.0.distance(pos).total_cmp(&b.0.distance(pos)))
            .is_some_and(|(_, v)| v.closed);
        let st = &mut self.state[i];
        let mut press = false;
        let give_up = if usable && nearest_usable_closed {
            let due = match st.last_press {
                Some(t) => since(now, t) >= PRESS_GAP_MS,
                None => since(now, since_at) >= self.jitter_ms,
            };
            if due && st.presses >= MAX_PRESSES {
                true
            } else {
                if due {
                    st.presses += 1;
                    st.last_press = Some(now);
                    press = true;
                    self.stats.presses += 1;
                    notes.push(DoorNote::Press {
                        door: i,
                        n: st.presses,
                    });
                }
                false
            }
        } else {
            since(now, since_at) >= WAIT_MS
        };
        if give_up {
            let presses = st.presses;
            st.given_up_at = Some(now.max(1));
            self.cur = None;
            self.stats.given_up += 1;
            notes.push(DoorNote::GaveUp { door: i, presses });
            return None;
        }
        let c = self.boxes[i].centre();
        Some(Hold {
            door: i,
            yaw: (c[0] - pos[0]).atan2(c[2] - pos[2]),
            press,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hunt::tests::solid_box;
    use crate::motion::{Body, MotionKind, BOT_STAND_DIMS};
    use crate::nav::follow::{Follower, NavGoal, GROW_PROBES};
    use crate::nav::{NavGrid, BUDGET};
    use crate::objects::{ClientObject, ExtentBox};
    use crate::romworld::LinkedModel;

    /// Two rooms (z < 0 and z > 0) split by a wall at z -8..8 over x -600..600 with a doorway at x -40..40 that a
    /// door model fills; the rooms are closed all round (no way round the wall).
    const DOOR_LO: [f32; 3] = [-40.0, 0.0, -4.0];
    const DOOR_HI: [f32; 3] = [40.0, 120.0, 4.0];

    fn two_rooms() -> LoadedWorld {
        let models = vec![
            solid_box([-700.0, -64.0, -700.0], [700.0, 0.0, 700.0]),
            solid_box([-600.0, 0.0, -8.0], [-40.0, 200.0, 8.0]),
            solid_box([40.0, 0.0, -8.0], [600.0, 200.0, 8.0]),
            solid_box([-700.0, 0.0, -700.0], [-600.0, 200.0, 700.0]),
            solid_box([600.0, 0.0, -700.0], [700.0, 200.0, 700.0]),
            solid_box([-700.0, 0.0, -700.0], [700.0, 200.0, -600.0]),
            solid_box([-700.0, 0.0, 600.0], [700.0, 200.0, 700.0]),
            solid_box(DOOR_LO, DOOR_HI),
        ];
        let full = Arc::new(CollisionWorld::from_parts(models, &[]));
        LoadedWorld {
            listed: "two rooms".into(),
            size: 0,
            sha16: String::new(),
            extents: ExtentBox {
                min: [-700.0, -64.0, -700.0],
                max: [700.0, 200.0, 700.0],
            },
            collision: full.clone(),
            ladders: Default::default(),
            player_spawns: 0,
            full,
            linked: vec![LinkedModel {
                name: b"door".to_vec(),
                index: 7,
                mover: true,
            }],
            solid: Some(vec![true]),
            kill_volumes: Vec::new(),
            hazard_volumes: Vec::new(),
        }
    }

    fn door_brush(enabled: bool) -> ObjectTable {
        let mut o = ClientObject::default();
        o.kind = kind::BRUSH;
        o.name = Some(b"DOOR".to_vec());
        let mut b = BitWriter::new();
        b.write_u8(DOOR_BLOB_TYPE)
            .write_u8(0)
            .write_bit(!enabled)
            .write_u8(1);
        o.blob = Some(b);
        let mut t = ObjectTable::default();
        t.objects.insert(100, o);
        t
    }

    /// The bot walks from one room to the other; the door opens `open_after` ms after a press (`None`: never, a
    /// locked door). Returns the end position, the presses and the notes.
    fn walk(open_after: Option<u32>, usable: bool) -> ([f32; 3], u32, Vec<DoorNote>) {
        let mut w = two_rooms();
        let table = door_brush(usable);
        let boxes = door_boxes(&w);
        assert_eq!(boxes.len(), 1);
        let mut mind = DoorMind::new(boxes.clone(), 7);
        let start = [0.0, BOT_STAND_DIMS[1], -300.0];
        let goal = [0.0, BOT_STAND_DIMS[1], 300.0];
        let mut body = Body::new(
            start,
            BOT_STAND_DIMS,
            0.0,
            MotionKind::Hunt,
            60,
            Some(w.collision.clone()),
        );
        let mut key = nav_key(&w, &mind.boxes, &mind.given_up(), true);
        let mut nav = NavGrid::new(nav_collision(&w, &key), BOT_STAND_DIMS);
        let mut route = Follower::default();
        let mut notes = Vec::new();
        let mut pressed_at = None;
        let mut presses = 0;
        let mut now: Millis = 1000;
        for _ in 0..(40_000 / 50) {
            now += 50;
            let pos = body.pos();
            route.grow(&mut nav, pos, GROW_PROBES);
            let wp = match route.next(&mut nav, now, pos, goal, true) {
                NavGoal::Way(w) => Some(w),
                NavGoal::Hold | NavGoal::Steer => None,
            };
            let path: Vec<[f32; 3]> = if route.points().is_empty() {
                vec![goal]
            } else {
                route.points().to_vec()
            };
            let views = door_views(&w, &table, &mind.boxes);
            let n0 = notes.len();
            let hold = mind.tick(now, true, pos, &path, &views, true, &mut notes);
            if hold.is_some_and(|h| h.press) {
                presses += 1;
                assert!(
                    mind.boxes[0].distance(pos) <= 128.0,
                    "in the server's reach"
                );
                pressed_at.get_or_insert(now);
            }
            // the server: the door moves away some time after the press (not solid any more)
            if let (Some(t), Some(after)) = (pressed_at, open_after) {
                if since(now, t) >= after && w.solid == Some(vec![true]) {
                    w.solid = Some(vec![false]);
                    let mut c = (*w.full).clone();
                    c.models[7] = CollModel::new(&[], &[], None);
                    w.collision = Arc::new(c);
                    body.set_world(w.collision.clone(), &[7]);
                }
            }
            if notes[n0..]
                .iter()
                .any(|n| matches!(n, DoorNote::GaveUp { .. } | DoorNote::Back { .. }))
            {
                let k = nav_key(&w, &mind.boxes, &mind.given_up(), true);
                if k != key {
                    key = k;
                    nav = NavGrid::new(nav_collision(&w, &key), BOT_STAND_DIMS);
                    route.clear();
                }
            }
            body.steer = match hold {
                Some(_) => None,
                None => wp.map(|p| (p[0] - pos[0]).atan2(p[2] - pos[2])),
            };
            body.advance(50_000);
            if body.pos()[2] > 250.0 {
                break;
            }
        }
        (body.pos(), presses, notes)
    }

    /// The bot walks up to the closed door, stops, presses the use key once, waits, and walks through.
    #[test]
    fn a_bot_opens_a_closed_door_on_its_way_and_walks_through() {
        let (end, presses, notes) = walk(Some(600), true);
        assert!(end[2] > 250.0, "through the door: {end:?} {notes:?}");
        assert_eq!(presses, 1, "{notes:?}");
        assert!(
            notes.iter().any(|n| matches!(n, DoorNote::Opened { .. })),
            "{notes:?}"
        );
    }

    /// A door that never opens is given up after the presses: the bot is not held there for ever (the grid takes it
    /// as a wall, and the closed rooms have no other way, so it stays on its side).
    #[test]
    fn a_locked_door_is_given_up_after_its_presses() {
        let (end, presses, notes) = walk(None, true);
        assert!(presses >= MAX_PRESSES, "{notes:?}");
        // no fourth press before the give-up (after GIVE_UP_MS the door is tried again)
        assert!(
            !notes
                .iter()
                .any(|n| matches!(n, DoorNote::Press { n, .. } if *n > MAX_PRESSES)),
            "{notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|n| matches!(n, DoorNote::GaveUp { presses: 3, .. })),
            "{notes:?}"
        );
        assert!(end[2] < 0.0, "{end:?}");
    }

    /// A door without a use entry gets no press; the bot waits for it, then gives it up.
    #[test]
    fn a_door_without_a_use_entry_is_waited_for_not_pressed() {
        let (_, presses, notes) = walk(None, false);
        assert_eq!(presses, 0);
        assert!(
            notes
                .iter()
                .any(|n| matches!(n, DoorNote::GaveUp { presses: 0, .. })),
            "{notes:?}"
        );
    }

    #[test]
    fn the_grid_takes_doors_as_floor_unless_given_up_and_doors_off_keeps_the_walls() {
        let w = two_rooms();
        let boxes = door_boxes(&w);
        assert_eq!(nav_key(&w, &boxes, &[false], true), vec![false]);
        assert_eq!(nav_key(&w, &boxes, &[true], true), vec![true]);
        assert_eq!(nav_key(&w, &boxes, &[false], false), vec![true]);
        // BOT-119 pits: the room behind the closed door is no pit (the door is floor to the grid), from either side
        let (near, far) = (
            [0.0, BOT_STAND_DIMS[1], -300.0],
            [0.0, BOT_STAND_DIMS[1], 300.0],
        );
        for (from, to) in [(near, far), (far, near)] {
            let mut nav = NavGrid::new(nav_collision(&w, &[false]), BOT_STAND_DIMS);
            nav.seed(from);
            for _ in 0..400 {
                nav.expand(BUDGET, Some(to));
            }
            let p = nav.path(from, to).unwrap_or_default();
            assert!(
                p.last().is_some_and(|q| (q[2] > 200.0) == (to[2] > 0.0)),
                "{from:?} -> {to:?}: {p:?}"
            );
            // and a seeded grid plans back the other way (the far side leads back to the seed)
            let back = nav.path(to, from).unwrap_or_default();
            assert!(
                back.last().is_some_and(|q| (q[2] > 200.0) == (from[2] > 0.0)),
                "{to:?} -> {from:?}: {back:?}"
            );
        }
        // doors as walls (doors off, or given up): no way to the other room
        let mut nav = NavGrid::new(nav_collision(&w, &[true]), BOT_STAND_DIMS);
        nav.seed(near);
        for _ in 0..400 {
            nav.expand(BUDGET, Some(far));
        }
        assert!(nav
            .path(near, far)
            .unwrap_or_default()
            .iter()
            .all(|q| q[2] < 0.0));
    }

    #[test]
    fn the_use_body_and_the_activate_entry() {
        let b = use_body();
        let mut r = b.reader();
        assert_eq!(r.read_u16(), USE_NO_OBJECT);
        assert_eq!(r.read_u8(), 0);
        let t = door_brush(true);
        assert_eq!(
            t.objects[&100].blob.as_ref().and_then(use_enabled),
            Some(true)
        );
        let t = door_brush(false);
        assert_eq!(
            t.objects[&100].blob.as_ref().and_then(use_enabled),
            Some(false)
        );
        assert!(DoorBox {
            name: Vec::new(),
            linked: 0,
            lo: DOOR_LO,
            hi: DOOR_HI
        }
        .crossed_by([0.0, 40.0, -50.0], [0.0, 40.0, 50.0]));
    }
}

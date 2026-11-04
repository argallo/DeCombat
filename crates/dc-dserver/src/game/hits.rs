//! Hit claim bookkeeping: the verdict of every `UP_FIRE` that reports an object hit (accepted, or the reason it was
//! refused), the counters behind the `hits:` log line, and the victim position track the bounded rewind tests a
//! claim against.
// SPEC: GAME_MESSAGES_2009_C2S.md §4.1 server answer (validation is server policy); MOVEMENT_2009.md §12.2 (the
// victim's positions are its own `UP_PLAYER_UPDATE` trailers, 15 per second by default, UDP_SESSION_2009_B.md §10.3)
// DC-DEFAULT: DSV-42 (proposed) rewind and verdict counters

use std::collections::VecDeque;

use dc_net_engine::time::since;
use dc_net_engine::Millis;

/// Why a reported object hit did not become damage (or that it did).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HitVerdict {
    /// Damage was applied with the victim's latest position.
    Accepted,
    /// Damage was applied with a position of the victim's recent track (DSV-42).
    AcceptedRewound,
    /// The round is not running (before the start or after the result).
    NotRunning,
    /// The message ended early (parse short).
    Incomplete,
    /// The shooter is not alive on the server (it died before its claim arrived, for example in a trade).
    ShooterDead,
    /// The clip mirror is empty (DC-C2S-4).
    ClipEmpty,
    /// The pellet released a throw.
    Throw,
    /// The hit object is no other player.
    UnknownVictim,
    /// The victim is not alive.
    VictimDead,
    /// Same team without team damage.
    Friendly,
    /// Not finite or no direction.
    BadGeometry,
    /// The shot origin is too far from the shooter's position (origin tolerance).
    OriginFar,
    /// The hit point lies outside the victim's box plus the tolerance at every tested position.
    OutsideBox,
    /// The traced segment meets a wall before the hit point (DSV-34).
    Wall,
    /// The damage rule gave nothing (hook or zero damage).
    NoDamage,
    /// The pellet's direction is not the 2009 cone's (DSV-102, `cone_check=refuse` only).
    ConeOff,
    /// The victim is inside its spawn window (DSV-156).
    SpawnShield,
    /// A melee hit farther from the eye than the weapon row's `Range` (DSV-161).
    OutOfRange,
    /// The fired weapon is one the room's weapon limit keeps out of the match and the server never granted
    /// (DSV-476).
    WeaponLimit,
}

impl HitVerdict {
    /// Every verdict, in log order.
    pub const ALL: [HitVerdict; 19] = [
        HitVerdict::Accepted,
        HitVerdict::AcceptedRewound,
        HitVerdict::NotRunning,
        HitVerdict::Incomplete,
        HitVerdict::ShooterDead,
        HitVerdict::ClipEmpty,
        HitVerdict::Throw,
        HitVerdict::UnknownVictim,
        HitVerdict::VictimDead,
        HitVerdict::Friendly,
        HitVerdict::BadGeometry,
        HitVerdict::OriginFar,
        HitVerdict::OutsideBox,
        HitVerdict::Wall,
        HitVerdict::NoDamage,
        HitVerdict::ConeOff,
        HitVerdict::SpawnShield,
        HitVerdict::OutOfRange,
        HitVerdict::WeaponLimit,
    ];

    /// Short name for the log.
    pub fn name(self) -> &'static str {
        match self {
            HitVerdict::Accepted => "accepted",
            HitVerdict::AcceptedRewound => "accepted_rewound",
            HitVerdict::NotRunning => "not_running",
            HitVerdict::Incomplete => "incomplete",
            HitVerdict::ShooterDead => "shooter_dead",
            HitVerdict::ClipEmpty => "clip_empty",
            HitVerdict::Throw => "throw",
            HitVerdict::UnknownVictim => "unknown_victim",
            HitVerdict::VictimDead => "victim_dead",
            HitVerdict::Friendly => "friendly",
            HitVerdict::BadGeometry => "bad_geometry",
            HitVerdict::OriginFar => "origin_far",
            HitVerdict::OutsideBox => "outside_box",
            HitVerdict::Wall => "wall",
            HitVerdict::NoDamage => "no_damage",
            HitVerdict::ConeOff => "cone_off",
            HitVerdict::SpawnShield => "spawn_shield",
            HitVerdict::OutOfRange => "out_of_range",
            HitVerdict::WeaponLimit => "weapon_limit",
        }
    }

    pub fn accepted(self) -> bool {
        matches!(self, HitVerdict::Accepted | HitVerdict::AcceptedRewound)
    }

    fn index(self) -> usize {
        HitVerdict::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }
}

/// Counters of the hit claims (`UP_FIRE` with `hit_obj`) of a session, by verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HitCounters {
    by: [u64; HitVerdict::ALL.len()],
}

impl HitCounters {
    pub fn add(&mut self, v: HitVerdict) {
        self.by[v.index()] += 1;
    }

    pub fn get(&self, v: HitVerdict) -> u64 {
        self.by[v.index()]
    }

    /// All claims counted.
    pub fn claims(&self) -> u64 {
        self.by.iter().sum()
    }

    pub fn accepted(&self) -> u64 {
        self.get(HitVerdict::Accepted) + self.get(HitVerdict::AcceptedRewound)
    }

    /// `hits: 12 claims, 10 accepted (2 rewound); refused shooter_dead 1, wall 1`.
    pub fn summary(&self) -> String {
        let refused: Vec<String> = HitVerdict::ALL
            .iter()
            .filter(|v| !v.accepted() && self.get(**v) > 0)
            .map(|v| format!("{} {}", v.name(), self.get(*v)))
            .collect();
        format!(
            "hits: {} claims, {} accepted ({} rewound); refused {}",
            self.claims(),
            self.accepted(),
            self.get(HitVerdict::AcceptedRewound),
            if refused.is_empty() {
                "none".to_string()
            } else {
                refused.join(", ")
            }
        )
    }
}

/// How long a player's position track is kept (the longest rewind the config may ask for).
// DC-DEFAULT: DSV-42 (proposed)
pub const TRACK_KEEP_MS: u32 = 1_000;

/// A player's recent positions (accepted `UP_PLAYER_UPDATE` trailers and spawns), oldest first. A spawn or teleport
/// starts a new track, so a rewind never moves the box along a path the player did not walk.
#[derive(Debug, Clone, Default)]
pub struct Track {
    points: VecDeque<(Millis, [f32; 3])>,
}

impl Track {
    /// Starts a new track at `pos` (spawn, teleport).
    pub fn restart(&mut self, now: Millis, pos: [f32; 3]) {
        self.points.clear();
        self.points.push_back((now, pos));
    }

    /// Adds an accepted position and drops what is older than [`TRACK_KEEP_MS`] (one older point is kept so that
    /// the position at the window start can be interpolated).
    pub fn push(&mut self, now: Millis, pos: [f32; 3]) {
        self.points.push_back((now, pos));
        while self.points.len() > 2 && since(now, self.points[1].0) > TRACK_KEEP_MS {
            self.points.pop_front();
        }
    }

    /// The path of the last `window_ms` before `now`, as a polyline: the (interpolated) position at the window start,
    /// the track points inside the window, and `current` (the position the server holds now).
    pub fn path(&self, now: Millis, window_ms: u32, current: [f32; 3]) -> Vec<[f32; 3]> {
        let mut out = Vec::new();
        if window_ms > 0 {
            let start = now.wrapping_sub(window_ms);
            let mut before: Option<(Millis, [f32; 3])> = None;
            for &(t, p) in &self.points {
                if since(now, t) > window_ms {
                    before = Some((t, p));
                    continue;
                }
                if let Some((tb, pb)) = before.take() {
                    // the window starts between `tb` and `t`
                    let span = since(t, tb).max(1) as f32;
                    let f = (since(start, tb) as f32 / span).clamp(0.0, 1.0);
                    out.push(lerp(pb, p, f));
                }
                out.push(p);
            }
        }
        if out.last() != Some(&current) {
            out.push(current);
        }
        out
    }
}

fn lerp(a: [f32; 3], b: [f32; 3], f: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
}

/// The victim centre on the segment `a -> b` at which the point `hit` lies within `ext` (half extents per axis) of
/// it, the middle of that part of the segment; `None` when there is none. (The segment against the box of half
/// extents `ext` around `hit`: the slab test.)
pub fn centre_on_segment(
    a: [f32; 3],
    b: [f32; 3],
    hit: [f32; 3],
    ext: [f32; 3],
) -> Option<[f32; 3]> {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for i in 0..3 {
        let d = b[i] - a[i];
        let (min, max) = (hit[i] - ext[i], hit[i] + ext[i]);
        if d.abs() < 1e-6 {
            if a[i] < min || a[i] > max {
                return None;
            }
            continue;
        }
        let (t0, t1) = ((min - a[i]) / d, (max - a[i]) / d);
        let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
        lo = lo.max(t0);
        hi = hi.min(t1);
        if lo > hi {
            return None;
        }
    }
    Some(lerp(a, b, (lo + hi) * 0.5))
}

/// The first victim centre of `path` (newest segments first) whose box grown by `ext` holds `hit`.
pub fn match_path(path: &[[f32; 3]], hit: [f32; 3], ext: [f32; 3]) -> Option<[f32; 3]> {
    let inside = |c: [f32; 3]| (0..3).all(|i| (hit[i] - c[i]).abs() <= ext[i]);
    let last = *path.last()?;
    if inside(last) {
        return Some(last);
    }
    for w in path.windows(2).rev() {
        if let Some(c) = centre_on_segment(w[0], w[1], hit, ext) {
            return Some(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_box_test() {
        let ext = [10.0, 10.0, 10.0];
        // the segment passes the point: the middle of the overlap
        let c = centre_on_segment([0.0; 3], [100.0, 0.0, 0.0], [50.0, 5.0, 0.0], ext).unwrap();
        assert!((c[0] - 50.0).abs() < 1e-3, "{c:?}");
        // too far sideways
        assert!(centre_on_segment([0.0; 3], [100.0, 0.0, 0.0], [50.0, 11.0, 0.0], ext).is_none());
        // beyond the end
        assert!(centre_on_segment([0.0; 3], [100.0, 0.0, 0.0], [111.0, 0.0, 0.0], ext).is_none());
        // a point segment
        assert!(centre_on_segment([5.0; 3], [5.0; 3], [0.0; 3], ext).is_some());
    }

    #[test]
    fn track_path_covers_the_window_only() {
        let mut t = Track::default();
        t.restart(0, [0.0; 3]);
        for i in 1..=20u32 {
            t.push(i * 100, [i as f32 * 10.0, 0.0, 0.0]);
        }
        // window 250 ms at 2000: from x = 175 (interpolated at 1750) to the current 200
        let p = t.path(2_000, 250, [200.0, 0.0, 0.0]);
        assert_eq!(p.first().map(|x| x[0]), Some(175.0));
        assert_eq!(p.last().map(|x| x[0]), Some(200.0));
        let ext = [5.0; 3];
        assert!(match_path(&p, [180.0, 0.0, 0.0], ext).is_some());
        assert!(match_path(&p, [160.0, 0.0, 0.0], ext).is_none());
        // window 0: the current position only
        assert_eq!(t.path(2_000, 0, [200.0, 0.0, 0.0]), vec![[200.0, 0.0, 0.0]]);
        // older than the keep time is dropped (one point before it stays)
        assert!(t.points.len() <= 12, "{}", t.points.len());
        // a restart forgets the path
        t.restart(2_100, [0.0, 0.0, 500.0]);
        let p = t.path(2_100, 250, [0.0, 0.0, 500.0]);
        assert!(match_path(&p, [180.0, 0.0, 0.0], ext).is_none());
    }

    #[test]
    fn counters_summary() {
        let mut c = HitCounters::default();
        c.add(HitVerdict::Accepted);
        c.add(HitVerdict::AcceptedRewound);
        c.add(HitVerdict::ShooterDead);
        assert_eq!(c.claims(), 3);
        assert_eq!(
            c.summary(),
            "hits: 3 claims, 2 accepted (1 rewound); refused shooter_dead 1"
        );
    }
}

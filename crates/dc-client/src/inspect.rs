//! The weapon inspect: a key that turns the first-person weapon in the hands to show it off, from the side and from
//! above, while its idle clip plays on. It is drawn on the own screen only: nothing goes to the server, no other player
//! sees or hears it. Firing, reloading, a weapon change, sprinting or the zoom end it at once, and they work as if it
//! had not been playing. The poses are data: a pack gives a weapon row the keys below through `content.weapons`.
//!
//! Row keys (all optional but `InspectLen` and at least one pose; a row without them cannot be inspected):
//! - `InspectLen`: the length in ms;
//! - `InspectPivot`: `x y z`, the eye-space point the weapon turns about (right, up, forward);
//! - `InspectPose<n>`, n = 1 ..= [`MAX_POSES`]: `ms pitch yaw roll x y z`: degrees (nose up, nose right, top to the
//!   right) and an eye-space offset, at that time;
//! - `InspectSound<n>`, n = 1 ..= [`MAX_SOUNDS`]: `ms k`: the row's own weapon sound `k` (1 ..= 14) at that time.
// DC-DEFAULT: CLI-1845 (proposed; USER REQUEST 2026-11-03 "a hotkey that does a weapon inspect animations", "It's only
// visual on your screen", "uses existing sounds"); not 2009 behaviour, no clip of the ROM's weapons does this

use glam::{EulerRot, Quat, Vec3};

pub const KEY_LEN: &str = "InspectLen";
pub const KEY_PIVOT: &str = "InspectPivot";
pub const KEY_POSE: &str = "InspectPose";
pub const KEY_SOUND: &str = "InspectSound";
/// The most pose keys a row can have.
pub const MAX_POSES: usize = 24;
/// The most sound keys a row can have.
pub const MAX_SOUNDS: usize = 12;
/// The longest inspect (ms).
const MAX_LEN_MS: u32 = 15_000;
/// How far a pose can turn (degrees) and move (eye-space units).
const MAX_TURN: f32 = 180.0;
const MAX_MOVE: f32 = 40.0;
/// How long a cancelled inspect takes to come back (ms): at once for the eye, and the weapon works meanwhile.
pub const CANCEL_MS: u32 = 110;
/// How long the hands' sway takes to fade in and out (ms).
const SWAY_FADE_MS: f32 = 500.0;

/// A turn and an offset.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pose {
    /// Pitch, yaw, roll in degrees.
    pub turn: [f32; 3],
    pub offset: [f32; 3],
}

impl Pose {
    fn mix(self, other: Pose, k: f32) -> Pose {
        let lerp = |a: [f32; 3], b: [f32; 3]| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * k);
        Pose {
            turn: lerp(self.turn, other.turn),
            offset: lerp(self.offset, other.offset),
        }
    }
}

/// What a row's keys say.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InspectAnim {
    pub len_ms: u32,
    pub pivot: [f32; 3],
    /// By time, starting and ending at rest.
    pub keys: Vec<(u32, Pose)>,
    /// By time.
    pub sounds: Vec<(u32, u8)>,
}

fn numbers(text: &str) -> Vec<f32> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .collect()
}

impl InspectAnim {
    /// Reads a row through `get` (the key's text). `None`: the row cannot be inspected.
    pub fn from_row(get: &dyn Fn(&str) -> Option<String>) -> Option<InspectAnim> {
        let len = *numbers(&get(KEY_LEN)?).first()?;
        if !(len >= 1.0) {
            return None;
        }
        let len_ms = (len as u32).min(MAX_LEN_MS);
        let mut pivot = [0.0; 3];
        if let Some(p) = get(KEY_PIVOT).map(|t| numbers(&t)).filter(|p| p.len() == 3) {
            pivot = [p[0], p[1], p[2]].map(|v| v.clamp(-MAX_MOVE, MAX_MOVE));
        }
        let mut keys: Vec<(u32, Pose)> = Vec::new();
        for n in 1..=MAX_POSES {
            let Some(v) = get(&format!("{KEY_POSE}{n}")).map(|t| numbers(&t)) else {
                continue;
            };
            if v.len() != 7 || v[0] < 0.0 || v[0] > len {
                continue;
            }
            keys.push((
                v[0] as u32,
                Pose {
                    turn: [v[1], v[2], v[3]].map(|a| a.clamp(-MAX_TURN, MAX_TURN)),
                    offset: [v[4], v[5], v[6]].map(|a| a.clamp(-MAX_MOVE, MAX_MOVE)),
                },
            ));
        }
        if keys.is_empty() {
            return None;
        }
        keys.sort_by_key(|k| k.0);
        // it leaves from rest and comes back to it, whatever the row says
        if keys[0].0 > 0 {
            keys.insert(0, (0, Pose::default()));
        }
        if keys[keys.len() - 1].0 < len_ms {
            keys.push((len_ms, Pose::default()));
        }
        let mut sounds = Vec::new();
        for n in 1..=MAX_SOUNDS {
            let Some(v) = get(&format!("{KEY_SOUND}{n}")).map(|t| numbers(&t)) else {
                continue;
            };
            if v.len() == 2 && v[0] >= 0.0 && v[0] <= len && (1.0..=14.0).contains(&v[1]) {
                sounds.push((v[0] as u32, v[1] as u8));
            }
        }
        sounds.sort_by_key(|s| s.0);
        Some(InspectAnim {
            len_ms,
            pivot,
            keys,
            sounds,
        })
    }

    /// The pose `t` ms in: a curve through the keys that keeps its speed through each one (it starts and ends still),
    /// with the small unsteady sway of a gun held in the hands.
    pub fn pose_at(&self, t: u32) -> Pose {
        let held = self.curve_at(t);
        // the sway fades in and out, so that the inspect still starts and ends exactly at rest
        let edge = (t.min(self.len_ms.saturating_sub(t)) as f32 / SWAY_FADE_MS).min(1.0);
        let fade = edge * edge * (3.0 - 2.0 * edge);
        let s = t as f32 / 1000.0;
        let sway = |freq: f32, phase: f32, amount: f32| {
            (s * freq * std::f32::consts::TAU + phase).sin() * amount * fade
        };
        Pose {
            turn: [
                held.turn[0] + sway(0.43, 0.0, 0.7),
                held.turn[1] + sway(0.31, 1.3, 0.9),
                held.turn[2] + sway(0.37, 2.1, 0.9),
            ],
            offset: [
                held.offset[0] + sway(0.29, 0.7, 0.03),
                held.offset[1] + sway(0.41, 2.6, 0.04),
                held.offset[2],
            ],
        }
    }

    fn curve_at(&self, t: u32) -> Pose {
        let Some(i) = self.keys.iter().position(|k| k.0 > t) else {
            return self.keys.last().map_or(Pose::default(), |k| k.1);
        };
        if i == 0 {
            return self.keys[0].1;
        }
        let (t0, a) = self.keys[i - 1];
        let (t1, b) = self.keys[i];
        let span = (t1 - t0).max(1) as f32;
        // the speed through a key: towards its neighbours, none at the two ends
        let slope = |k: usize, c: usize, pick: &dyn Fn(&Pose) -> [f32; 3]| -> f32 {
            if k == 0 || k + 1 >= self.keys.len() {
                return 0.0;
            }
            let (before, after) = (self.keys[k - 1], self.keys[k + 1]);
            (pick(&after.1)[c] - pick(&before.1)[c]) / (after.0 - before.0).max(1) as f32
        };
        let x = (t - t0) as f32 / span;
        let (x2, x3) = (x * x, x * x * x);
        let channel = |pick: &dyn Fn(&Pose) -> [f32; 3]| -> [f32; 3] {
            [0, 1, 2].map(|c| {
                (2.0 * x3 - 3.0 * x2 + 1.0) * pick(&a)[c]
                    + (x3 - 2.0 * x2 + x) * span * slope(i - 1, c, pick)
                    + (-2.0 * x3 + 3.0 * x2) * pick(&b)[c]
                    + (x3 - x2) * span * slope(i, c, pick)
            })
        };
        Pose {
            turn: channel(&|p| p.turn),
            offset: channel(&|p| p.offset),
        }
    }
}

/// How the first-person model is placed this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewPose {
    pub rotation: Quat,
    /// The eye-space point it turns about.
    pub pivot: Vec3,
    pub offset: Vec3,
}

impl ViewPose {
    fn of(pose: Pose, pivot: [f32; 3]) -> ViewPose {
        let [pitch, yaw, roll] = pose.turn.map(f32::to_radians);
        ViewPose {
            rotation: Quat::from_euler(EulerRot::YXZ, yaw, -pitch, -roll),
            pivot: Vec3::from(pivot),
            offset: Vec3::from(pose.offset),
        }
    }

    /// The model's position when `origin` is its position at rest.
    pub fn place(&self, origin: Vec3) -> Vec3 {
        self.pivot + self.rotation * (origin - self.pivot) + self.offset
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Playing {
    weapon: u16,
    since: u32,
    next_sound: usize,
}

/// What this frame asks of the inspect.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct InspectFrame {
    /// The key went down.
    pub press: bool,
    /// The weapon in the hands, if the player is alive.
    pub held: Option<u16>,
    /// Anything that ends an inspect or keeps it from starting: fire, reload, a weapon change, sprint, zoom, a clip other
    /// than idle.
    pub busy: bool,
}

/// The inspect state of the own weapon.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Inspector {
    playing: Option<Playing>,
    /// A cancelled inspect coming back: since when, from which pose, about which point.
    back: Option<(u32, Pose, [f32; 3])>,
    sounds: Vec<(u8, u16)>,
}

impl Inspector {
    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// One frame. `anim`: what the held weapon's row says.
    pub fn step(&mut self, now: u32, f: InspectFrame, anim: Option<&InspectAnim>) {
        if self
            .back
            .is_some_and(|(t0, _, _)| now.saturating_sub(t0) >= CANCEL_MS)
        {
            self.back = None;
        }
        let Some(p) = self.playing else {
            if let (true, false, Some(weapon), Some(_)) = (f.press, f.busy, f.held, anim) {
                self.back = None;
                self.playing = Some(Playing {
                    weapon,
                    since: now,
                    next_sound: 0,
                });
            }
            return;
        };
        let t = now.saturating_sub(p.since);
        let Some(a) = anim.filter(|_| f.held == Some(p.weapon)) else {
            // the weapon went away: nothing to come back with
            self.playing = None;
            return;
        };
        if f.busy || f.press {
            self.back = Some((now, a.pose_at(t), a.pivot));
            self.playing = None;
            return;
        }
        if t >= a.len_ms {
            self.playing = None;
            return;
        }
        let mut next = p.next_sound;
        while let Some((at, k)) = a.sounds.get(next) {
            if *at > t {
                break;
            }
            self.sounds.push((*k, p.weapon));
            next += 1;
        }
        self.playing = Some(Playing {
            next_sound: next,
            ..p
        });
    }

    /// The first-person model's pose now (`None`: at rest).
    pub fn pose(&self, now: u32, anim: Option<&InspectAnim>) -> Option<ViewPose> {
        if let (Some(p), Some(a)) = (self.playing, anim) {
            return Some(ViewPose::of(
                a.pose_at(now.saturating_sub(p.since)),
                a.pivot,
            ));
        }
        let (t0, from, pivot) = self.back?;
        let k = now.saturating_sub(t0) as f32 / CANCEL_MS as f32;
        (k < 1.0).then(|| ViewPose::of(from.mix(Pose::default(), k), pivot))
    }

    /// The sounds that came due: (weapon sound number, weapon). They play here only.
    pub fn take_sounds(&mut self) -> Vec<(u8, u16)> {
        std::mem::take(&mut self.sounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn anim() -> InspectAnim {
        let row: BTreeMap<&str, &str> = [
            (KEY_LEN, "1000"),
            (KEY_PIVOT, "1 2 3"),
            ("InspectPose1", "500 0 90 0 0 0 0"),
            ("InspectSound1", "100 10"),
            ("InspectSound2", "400 11"),
        ]
        .into_iter()
        .collect();
        InspectAnim::from_row(&|k| row.get(k).map(|v| v.to_string())).unwrap()
    }

    fn frame(press: bool, busy: bool) -> InspectFrame {
        InspectFrame {
            press,
            busy,
            held: Some(25),
        }
    }

    #[test]
    fn a_row_needs_a_length_and_a_pose_and_rests_at_both_ends() {
        let none: BTreeMap<&str, &str> = [(KEY_LEN, "1000")].into_iter().collect();
        assert!(InspectAnim::from_row(&|k| none.get(k).map(|v| v.to_string())).is_none());
        let a = anim();
        assert_eq!(a.keys.len(), 3);
        assert_eq!(a.pose_at(0), Pose::default());
        assert_eq!(a.curve_at(500).turn, [0.0, 90.0, 0.0]);
        assert!(
            (a.pose_at(500).turn[1] - 90.0).abs() < 1.0,
            "a key, give or take the sway"
        );
        assert_eq!(a.pose_at(1000), Pose::default());
        assert!(a.pose_at(250).turn[1] > 0.0 && a.pose_at(250).turn[1] < 90.0);
    }

    #[test]
    fn the_motion_keeps_its_speed_through_a_key() {
        let row: BTreeMap<&str, &str> = [
            (KEY_LEN, "2000"),
            ("InspectPose1", "500 0 10 0 0 0 0"),
            ("InspectPose2", "1000 0 20 0 0 0 0"),
            ("InspectPose3", "1500 0 30 0 0 0 0"),
        ]
        .into_iter()
        .collect();
        let a = InspectAnim::from_row(&|k| row.get(k).map(|v| v.to_string())).unwrap();
        let yaw = |t: u32| a.curve_at(t).turn[1];
        let (before, after) = (yaw(1000) - yaw(990), yaw(1010) - yaw(1000));
        assert!(before > 0.1, "still moving at a key: {before}");
        assert!((before - after).abs() < 0.02, "{before} then {after}");
        // and it is still at the two ends
        assert!(yaw(10) < 0.2);
    }

    #[test]
    fn nonsense_in_a_row_is_left_out_or_held_in_bounds() {
        let row: BTreeMap<&str, &str> = [
            (KEY_LEN, "999999"),
            ("InspectPose1", "100 500 0 0 0 0 999"),
            ("InspectPose2", "not numbers"),
            ("InspectSound1", "100 99"),
        ]
        .into_iter()
        .collect();
        let a = InspectAnim::from_row(&|k| row.get(k).map(|v| v.to_string())).unwrap();
        assert_eq!(a.len_ms, MAX_LEN_MS);
        assert_eq!(a.keys[1].1.turn[0], MAX_TURN);
        assert_eq!(a.keys[1].1.offset[2], MAX_MOVE);
        assert!(a.sounds.is_empty());
    }

    #[test]
    fn the_key_starts_it_its_sounds_come_once_and_it_ends_by_itself() {
        let a = anim();
        let mut i = Inspector::default();
        i.step(0, frame(false, false), Some(&a));
        assert!(!i.is_playing(), "not without the key");
        i.step(10, frame(true, false), Some(&a));
        assert!(i.is_playing());
        i.step(200, frame(false, false), Some(&a));
        i.step(201, frame(false, false), Some(&a));
        assert_eq!(i.take_sounds(), vec![(10, 25)]);
        assert!(i.pose(200, Some(&a)).is_some());
        i.step(1100, frame(false, false), Some(&a));
        assert!(!i.is_playing());
        assert!(i.pose(1100, Some(&a)).is_none());
    }

    #[test]
    fn firing_reloading_or_the_key_cancels_it_and_it_comes_back_quickly() {
        let a = anim();
        let mut i = Inspector::default();
        i.step(0, frame(true, false), Some(&a));
        i.step(500, frame(false, true), Some(&a));
        assert!(!i.is_playing(), "busy cancels");
        let early = i.pose(510, Some(&a)).expect("coming back");
        assert!(early.rotation.w < 0.99, "still turned");
        assert!(i.pose(500 + CANCEL_MS, Some(&a)).is_none(), "and home");
        // the key again cancels too
        let mut i = Inspector::default();
        i.step(0, frame(true, false), Some(&a));
        i.step(100, frame(true, false), Some(&a));
        assert!(!i.is_playing());
    }

    #[test]
    fn it_cannot_start_when_busy_or_without_a_row() {
        let a = anim();
        let mut i = Inspector::default();
        i.step(0, frame(true, true), Some(&a));
        assert!(!i.is_playing());
        i.step(10, frame(true, false), None);
        assert!(!i.is_playing());
    }

    #[test]
    fn another_weapon_ends_it_with_no_way_back() {
        let a = anim();
        let mut i = Inspector::default();
        i.step(0, frame(true, false), Some(&a));
        i.step(
            100,
            InspectFrame {
                held: Some(27),
                ..frame(false, false)
            },
            Some(&a),
        );
        assert!(!i.is_playing());
        assert!(i.pose(100, Some(&a)).is_none());
    }

    #[test]
    fn a_turn_keeps_the_pivot_where_it_is() {
        let p = ViewPose::of(
            Pose {
                turn: [0.0, 90.0, 0.0],
                offset: [0.0; 3],
            },
            [0.0, 0.0, 10.0],
        );
        assert!(
            p.place(Vec3::new(0.0, 0.0, 10.0))
                .distance(Vec3::new(0.0, 0.0, 10.0))
                < 1e-4
        );
        // nose right: what lay behind the pivot goes to the left of it
        assert!(p.place(Vec3::ZERO).x < -9.0);
    }
}

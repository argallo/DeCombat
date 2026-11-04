//! The camera while the local player is dead: after a 0x67 state 2 the 2009 client leaves the own view for an
//! observer camera and returns to the own view at the next 0x67 state 1 (or when it asks for its respawn).
//!
//! The 2009 rule (CLI-170, read from CShell's observer state machine cshell+0x60ac0 and the camera update
//! cshell+0x278830):
//! - With a known killer (the 0x6d that named the local player kept `roster`+0x10 >= 0) the **killer camera** runs
//!   first (observer mode 4, camera mode 7): it starts 20 units above the own body, looks back along the last
//!   damage direction (minus the 0x74 `from_dir`: toward the shooter) and moves that way 5 units a frame while
//!   every placement pulls it 10 % back to its anchor, so it settles 45 units from the body toward the killer; a
//!   wall on the way stops it.
//! - After the dying time (3500 ms after the death) the client switches to the **follow camera** (observer mode 2,
//!   camera mode 4) on the killer (cshell+0x60c69); without a killer, at once on a living team mate (cshell+0x60010),
//!   else on the own body. The follow camera orbits its target at the offset 165 back / 90 up (radius 187.95,
//!   no zoom), turned by the dead player's own view angles, around the target's position raised 20 units, pulled
//!   10 % back toward that anchor and kept out of walls, and looks at the anchor.
//! - Neither camera smooths its turn: the view is set every frame.
//! - The follow camera's target search (cshell+0x60010 over the roster search cshell+0x29090) takes the next living
//!   player of the own team; only game classes 6 and 9 search every team (the "enemy dead camera" profile option
//!   would allow the enemy team too; this client has no such option). So in a team mode the follow camera that the
//!   2009 client put on the killer after the dying time takes a team mate here (playtest report: follow team mates).
//! - The mouse wheel moves the follow camera along its offset: 40 units a notch, from half the radius closer up to
//!   400 units farther away; a new target starts at no zoom (cshell+0x276620, reset at cshell+0x60010).
// SPEC: GAME_MESSAGES_2009_C2S.md §3.3 (cshell+0x60ac0: killer camera mode 4 / follow camera mode 2), §3.4 (camera
// reset at the request), §3.5 (observer modes, the followed record); GAME_MESSAGES_2009_S2C.md §3.3.1 effect 3,
// §4.1, §4.3 row 0x74 (`from_dir` at controller +0x1ec)
// DISASM: cshell+0x60ac0..0x60d6b (observer modes), cshell+0x27930e..0x2795e1 (camera mode 7), cshell+0x2795e4..
// 0x279c18 (camera mode 4), cshell+0x276a60 (placement: anchor ray, 10 % pull), cshell+0x278292..0x278385 (defaults)

use crate::look::ViewAngles;
use crate::world::{DeathView, RemoteBody};
use dc_sim::{CollisionWorld, V3};

/// Killer camera: the start point is the own body's position raised this far (the client's registered default
/// of its console variable at VA 0x375acef0).
// EVIDENCE: cshell+0x278373 (default 20.0 registered for the variable at VA 0x375acef0), read at cshell+0x2793b3
pub const KILLCAM_LIFT: f32 = 20.0;
/// Killer camera: the step along the look direction per frame (console variable VA 0x375acee4).
// EVIDENCE: cshell+0x27835c (default 5.0 for VA 0x375acee4), read at cshell+0x2794d3 and added at cshell+0x279512
pub const KILLCAM_STEP: f32 = 5.0;
/// Killer camera: the travel limit from the start (console variable VA 0x375aced8; the 10 % pull keeps the camera
/// at 45 units, far inside it, so it only matters if the pull is changed).
// EVIDENCE: cshell+0x278345 (default 220.0 for VA 0x375aced8), compared at cshell+0x2795b7..0x2795ca
pub const KILLCAM_REACH: f32 = 220.0;
/// Every camera placement moves the camera this fraction of the way back toward its anchor.
// EVIDENCE: cshell+0x276a60 (tail: position := anchor + (position - anchor) x (1 - 0.1))
pub const CAM_PULL: f32 = 0.1;
/// Height of the anchor above an object's position in every placement.
// EVIDENCE: cshell+0x276a60 (`+ 20.0` on the anchor ray), cshell+0x27990f (the follow target raised by 20.0)
pub const ANCHOR_LIFT: f32 = 20.0;
/// The killer camera lasts this long after the death, then the follow camera takes the killer (console variable
/// VA 0x37560e00).
// EVIDENCE: cshell+0x5e72b (default 3500.0 for VA 0x37560e00), compared with now - death time at cshell+0x60c0f..
// 0x60c5b (mode 4 while shorter, else mode 2 on the killer, cshell+0x60c69..0x60caa)
pub const DYING_TIME_MS: u32 = 3500;
/// Follow camera: the offset back from and up above its anchor (console variables VA 0x375aceb4, 0x375acec0).
// EVIDENCE: cshell+0x278300 (default 165.0, VA 0x375aceb4), cshell+0x278317 (default 90.0, VA 0x375acec0); used at
// cshell+0x2798e2..0x2798f6 and cshell+0x277530
pub const FOLLOW_BACK: f32 = 165.0;
pub const FOLLOW_UP: f32 = 90.0;
/// The per-frame step and pull of the 2009 client are read at this frame rate, so the camera moves the same at
/// every frame rate (the 2009 client's own result depends on its frame rate; at 60 fps it settles in about 0.5 s).
// DC-DEFAULT: CLI-171 (proposed) 60 frames per second
pub const CAM_FRAME_RATE: f32 = 60.0;
/// The camera stops this far before a wall its ray hits.
pub const WALL_MARGIN: f32 = 4.0;
/// Follow camera: one wheel notch moves the camera this far along its offset (positive: farther).
// EVIDENCE: cshell+0x276620 (zoom := zoom + notches x 40.0, kept at camera +0xdc), called from the wheel handler
// cshell+0x1b67b0 in the observer camera state
pub const ZOOM_STEP: f32 = 40.0;
/// Follow camera: the farthest zoom beyond the offset's length.
// EVIDENCE: cshell+0x276620 (`400.0 < zoom` -> 400.0)
pub const ZOOM_OUT_MAX: f32 = 400.0;

/// The follow camera's zoom range: half the radius closer up, [`ZOOM_OUT_MAX`] farther away.
// EVIDENCE: cshell+0x276620 (lower limit -sqrt(back^2 + up^2) x 0.5, from the two offset variables)
pub fn zoom_range() -> (f32, f32) {
    (-follow_radius() * 0.5, ZOOM_OUT_MAX)
}

/// Whether the follow camera may take a player of any team: game classes 6 and 9 search without the team filter
/// (cshell+0x60010); a player with no team (0 or 0xff) searches with team 0, every player (cshell+0x29090).
// EVIDENCE: cshell+0x60010 (team argument 0 when `client_shell`+0x16938 is 6 or 9), cshell+0x29090 (team 0 = any)
pub fn follows_any_team(mode_class: Option<u8>, own_team: Option<u8>) -> bool {
    matches!(mode_class, Some(6 | 9)) || !matches!(own_team, Some(1 | 2))
}

/// Which observer camera runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathCamMode {
    /// Mode 4 (camera mode 7): from above the own body back toward the player who killed the local player.
    Killer(u8),
    /// Mode 2 (camera mode 4): orbiting a player (the killer after the killer camera, else a living team mate), or
    /// the own body when there is none.
    Follow(Option<u8>),
}

/// The player the death panel shows while the follow camera follows another player: his roster id and object.
// EVIDENCE: cshell+0x60010 -> cshell+0xc9e20 (the follow camera's new target bound to the death panel)
pub fn observed(mode: Option<DeathCamMode>, remotes: &[RemoteBody]) -> Option<(u8, u16)> {
    let Some(DeathCamMode::Follow(Some(pid))) = mode else {
        return None;
    };
    remotes.iter().find(|r| r.pid == pid).map(|r| (pid, r.obj))
}

/// The death camera of a frame loop: the death point is taken at the first dead frame.
#[derive(Debug, Clone, Default)]
pub struct DeathCam {
    active: Option<Active>,
    /// Deaths the camera ran for (tests and logs).
    pub deaths: u32,
    /// Target steps asked for by the mouse buttons since the last frame (+1 next, -1 previous).
    step: i32,
}

#[derive(Debug, Clone)]
struct Active {
    since: u32,
    /// The killer camera's start (own body + [`KILLCAM_LIFT`]).
    anchor: V3,
    mode: DeathCamMode,
    /// The killer camera's travel from the anchor; it stops growing at a wall.
    travel: f32,
    blocked: bool,
    view: ViewAngles,
    /// The follow camera's wheel zoom (units along its offset, within [`zoom_range`]).
    zoom: f32,
}

/// The view angles that look from `from` to `to` (yaw 0 along +Z, positive toward +X; positive pitch down).
pub fn look_at(from: V3, to: V3) -> Option<ViewAngles> {
    let d = to - from;
    let flat = (d.x * d.x + d.z * d.z).sqrt();
    if !(flat.is_finite() && d.y.is_finite()) || flat + d.y.abs() < 1e-3 {
        return None;
    }
    Some(ViewAngles {
        yaw: d.x.atan2(d.z),
        pitch: (-d.y).atan2(flat),
    })
}

/// Where the killer camera settles: the step pulled back every frame, `q = (q + step) (1 - pull)`, has the fixed
/// point `step (1 - pull) / pull` (45 units with the defaults).
pub fn killcam_rest() -> f32 {
    (KILLCAM_STEP * (1.0 - CAM_PULL) / CAM_PULL).min(KILLCAM_REACH)
}

/// The follow camera's radius: the length of the offset (165, 90) without zoom.
pub fn follow_radius() -> f32 {
    (FOLLOW_BACK * FOLLOW_BACK + FOLLOW_UP * FOLLOW_UP).sqrt()
}

/// `from` moved toward `to`, stopped [`WALL_MARGIN`] before the first wall on the way; the flag says a wall stopped it.
fn clamp_by_world(world: Option<&CollisionWorld>, from: V3, to: V3) -> (V3, bool) {
    let Some(w) = world else {
        return (to, false);
    };
    let d = to - from;
    let len = d.len();
    if !(len.is_finite()) || len < 1e-3 {
        return (to, false);
    }
    match w.ray(from, to) {
        Some(h) => {
            let free = (h.t * len - WALL_MARGIN).max(0.0);
            (from + d * (free / len), true)
        }
        None => (to, false),
    }
}

/// The unit vector back along a damage direction (toward the shooter), `None` for a zero or broken direction.
fn back_along(shot: Option<[f32; 3]>) -> Option<V3> {
    let s = shot?;
    let v = V3::new(-s[0], -s[1], -s[2]);
    let n = v.len();
    (n.is_finite() && n > 1e-6).then(|| v * (1.0 / n))
}

fn body_pos(r: &RemoteBody) -> Option<V3> {
    r.pos
        .iter()
        .all(|v| v.is_finite())
        .then(|| V3::new(r.pos[0], r.pos[1], r.pos[2]))
}

/// Whether a body at `p` stands in the world: inside the bounds of the world's collision models and above their
/// lowest point. A character our server's visibility culling has parked for this client (the park point at the
/// bottom of the world box, VIS rules of dc-dserver) is not, so no camera follows it into the void.
// DC-DEFAULT: CLI-671 (proposed) a death camera neither follows nor aims at a body outside the world
pub fn in_world(world: Option<&CollisionWorld>, p: V3) -> bool {
    let Some(w) = world else {
        return true;
    };
    let mut it = w.models.iter().map(|m| m.bounds());
    let Some(first) = it.next() else {
        return true;
    };
    let (lo, hi) = it.fold(first, |(lo, hi), (l, h)| {
        (
            V3::new(lo.x.min(l.x), lo.y.min(l.y), lo.z.min(l.z)),
            V3::new(hi.x.max(h.x), hi.y.max(h.y), hi.z.max(h.z)),
        )
    });
    (lo.x..=hi.x).contains(&p.x)
        && (lo.z..=hi.z).contains(&p.z)
        && p.y > lo.y + IN_WORLD_FLOOR
        && p.y <= hi.y
}

/// A body counts as in the world only this far above the world's lowest point (units).
// DC-DEFAULT: CLI-671 (proposed)
pub const IN_WORLD_FLOOR: f32 = 1.0;

impl DeathCam {
    /// The mode in use (`None`: the own view).
    pub fn mode(&self) -> Option<DeathCamMode> {
        self.active.as_ref().map(|a| a.mode)
    }

    /// The follow camera's zoom (0 while the own view is used).
    pub fn zoom(&self) -> f32 {
        self.active.as_ref().map_or(0.0, |a| a.zoom)
    }

    /// A mouse button while the death camera runs: the next (`+1`, left button) or previous (`-1`, right button)
    /// player to follow, taken at the next frame; it ends the killer camera. `false`: no death camera, the button is
    /// not taken.
    // DC-DEFAULT: CLI-1120 (proposed) left button next, right button previous team mate
    pub fn cycle(&mut self, dir: i32) -> bool {
        if self.active.is_none() {
            return false;
        }
        self.step += dir.signum();
        true
    }

    /// Wheel notches while the death camera runs (positive: away from the user): the follow camera zooms by
    /// [`ZOOM_STEP`] a notch within [`zoom_range`]. `false`: no death camera.
    // EVIDENCE: cshell+0x276620 (zoom + notches x 40, clamped)
    // DC-DEFAULT: CLI-1121 (proposed) wheel up (away from the user) moves the camera farther out
    pub fn wheel(&mut self, notches: f32) -> bool {
        let Some(a) = self.active.as_mut() else {
            return false;
        };
        if notches.is_finite() {
            let (lo, hi) = zoom_range();
            a.zoom = (a.zoom + notches * ZOOM_STEP).clamp(lo, hi);
        }
        true
    }

    /// The camera of this frame: `None` while the own view is used; else the eye and view angles to draw from.
    /// `eye` is the own eye this frame, `body` the own body centre, `view` the own view angles (the follow camera
    /// orbits with them), `dt` the frame time in seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        death: Option<&DeathView>,
        remotes: &[RemoteBody],
        eye: V3,
        body: V3,
        view: &ViewAngles,
        world: Option<&CollisionWorld>,
        dt: f32,
    ) -> Option<(V3, ViewAngles)> {
        // the 2009 cameras start from the body, not the eye (the parameter stays for the frame loops)
        let _ = eye;
        let Some(d) = death else {
            self.active = None;
            return None;
        };
        // a new death (or the first frame of this one): the start point and the first mode
        let fresh = self.active.as_ref().is_none_or(|a| d.since_ms < a.since);
        if fresh {
            let mode = match d.killer {
                Some(k) if Some(k) != d.own_pid => DeathCamMode::Killer(k),
                _ => DeathCamMode::Follow(None),
            };
            self.active = Some(Active {
                since: d.since_ms,
                anchor: body + V3::new(0.0, KILLCAM_LIFT, 0.0),
                mode,
                travel: 0.0,
                blocked: false,
                view: *view,
                zoom: 0.0,
            });
            self.deaths += 1;
            self.step = 0;
        }
        let step = std::mem::take(&mut self.step);
        let a = self.active.as_mut()?;
        a.since = d.since_ms;
        // the killer camera ends after the dying time (or at a button): the follow camera takes the killer when
        // the search may take him (classes 6 and 9), else a team mate
        // DC-DEFAULT: CLI-1122 (proposed) in a team class the follow camera never takes the enemy killer
        // DC-DEFAULT: CLI-1193 (proposed, playtest report "no delay of the death before switching to a teammate pov"):
        // a button during the dying time is dropped; the killer camera always runs its 3500 ms (the 2009 observer
        // state machine leaves mode 4 only by the time, cshell+0x60c0f..0x60c5b)
        let step = if matches!(a.mode, DeathCamMode::Killer(_)) && d.since_ms < DYING_TIME_MS {
            0
        } else {
            step
        };
        if let DeathCamMode::Killer(k) = a.mode {
            if d.since_ms >= DYING_TIME_MS {
                a.mode = DeathCamMode::Follow(d.any_team.then_some(k));
            }
        }
        let drawn = |pid: u8| {
            remotes
                .iter()
                .find(|r| r.pid == pid)
                .and_then(body_pos)
                .filter(|p| in_world(world, *p))
        };
        match a.mode {
            DeathCamMode::Killer(k) => {
                let dir = back_along(d.shot_dir).or_else(|| {
                    // no damage direction (none received): toward the killer's body when it is drawn
                    // DC-DEFAULT: CLI-171 (proposed) the fallback direction
                    drawn(k).and_then(|p| {
                        let v = p + V3::new(0.0, ANCHOR_LIFT, 0.0) - a.anchor;
                        let n = v.len();
                        (n > 1e-3).then(|| v * (1.0 / n))
                    })
                });
                let Some(dir) = dir else {
                    return Some((a.anchor, a.view));
                };
                if !a.blocked {
                    // q -> rest - (rest - q) x 0.9^(frames), frames = dt x 60 (the per-frame law in time)
                    let rest = killcam_rest();
                    let frames = (dt.max(0.0) * CAM_FRAME_RATE).min(1e4);
                    let keep = (1.0 - CAM_PULL).powf(frames);
                    a.travel = rest - (rest - a.travel) * keep;
                }
                let (cam, hit) = clamp_by_world(world, a.anchor, a.anchor + dir * a.travel);
                if hit {
                    a.blocked = true;
                    a.travel = (cam - a.anchor).len();
                }
                if let Some(v) = look_at(cam, cam + dir) {
                    a.view = v;
                }
                Some((cam, a.view))
            }
            DeathCamMode::Follow(prev) => {
                // the players the search may take: living, drawn in the world, of the own team (any team in
                // classes 6 and 9), in roster id order
                let list = follow_candidates(d, remotes, world);
                let kept = prev.filter(|&p| drawn(p).is_some() && list.contains(&p));
                let who = match (step, kept) {
                    (0, Some(p)) => Some(p),
                    (0, None) => list.first().copied(),
                    (s, _) => step_target(&list, prev, s),
                };
                if who != prev {
                    // a new target starts at no zoom (cshell+0x60010)
                    a.zoom = 0.0;
                }
                a.mode = DeathCamMode::Follow(who);
                let target = who.and_then(drawn).unwrap_or(body);
                let anchor = target + V3::new(0.0, ANCHOR_LIFT, 0.0);
                // the offset (back, up) turned by the own view: yaw around the anchor, pitch raising it
                // DC-DEFAULT: CLI-171 (proposed) the phase: the camera stands behind the own view direction
                let r = follow_radius() + a.zoom;
                let e = (FOLLOW_UP.atan2(FOLLOW_BACK) + view.pitch).clamp(-1.45, 1.45);
                let (s, c) = view.yaw.sin_cos();
                let want = anchor + V3::new(-s * r * e.cos(), r * e.sin(), -c * r * e.cos());
                let (cam, _) = clamp_by_world(world, anchor, want);
                let cam = anchor + (cam - anchor) * (1.0 - CAM_PULL);
                if let Some(v) = look_at(cam, anchor) {
                    a.view = v;
                }
                Some((cam, a.view))
            }
        }
    }
}

/// The players the follow camera may take, in roster id order: not the own player, not seen dead, drawn in the
/// world, of the own team unless [`DeathView::any_team`].
// EVIDENCE: cshell+0x29090 (roster search mode 1: not dead, team matches or 0, team not 0xff, has an object)
fn follow_candidates(
    d: &DeathView,
    remotes: &[RemoteBody],
    world: Option<&CollisionWorld>,
) -> Vec<u8> {
    let mut v: Vec<u8> = remotes
        .iter()
        .filter(|r| Some(r.pid) != d.own_pid && !d.dead.contains(r.pid))
        .filter(|r| d.any_team || (d.own_team == Some(r.team) && r.team != 0xff))
        .filter(|r| body_pos(r).is_some_and(|p| in_world(world, p)))
        .map(|r| r.pid)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The target `steps` places after (positive) or before (negative) `cur` in `list`, wrapping around; from no
/// target (or one no longer listed) the next or previous one by roster id.
fn step_target(list: &[u8], cur: Option<u8>, steps: i32) -> Option<u8> {
    if list.is_empty() {
        return None;
    }
    let n = list.len() as i64;
    let at = match cur {
        Some(c) => {
            // not listed: stand between its neighbours
            let below = list.iter().filter(|&&p| p < c).count() as i64;
            match (list.contains(&c), steps > 0) {
                (true, _) => below,
                (false, true) => below - 1,
                (false, false) => below,
            }
        }
        None if steps > 0 => -1,
        None => n,
    };
    Some(list[(at + steps as i64).rem_euclid(n) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(pid: u8, team: u8, pos: [f32; 3]) -> RemoteBody {
        RemoteBody {
            obj: 100 + pid as u16,
            pid,
            name: format!("p{pid}"),
            team,
            pos,
            half: [16.0, 40.0, 16.0],
            yaw: None,
            look: None,
        }
    }

    /// The death panel's target is the followed player (CLI-128); the own body or the killer camera bind none.
    #[test]
    fn the_followed_player_is_the_panels_target() {
        let r = [remote(3, 1, [0.0; 3]), remote(7, 1, [9.0; 3])];
        assert_eq!(observed(Some(DeathCamMode::Follow(Some(7))), &r), Some((7, 107)));
        assert_eq!(observed(Some(DeathCamMode::Follow(None)), &r), None);
        assert_eq!(observed(Some(DeathCamMode::Killer(3)), &r), None);
        assert_eq!(observed(Some(DeathCamMode::Follow(Some(9))), &r), None, "gone from the stream");
        assert_eq!(observed(None, &r), None);
    }

    fn death(killer: Option<u8>, since: u32, shot: Option<[f32; 3]>) -> DeathView {
        DeathView {
            since_ms: since,
            killer,
            own_pid: Some(0),
            own_team: Some(1),
            shot_dir: shot,
            ..DeathView::default()
        }
    }

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn look_at_follows_the_camera_convention() {
        let o = V3::ZERO;
        let v = look_at(o, V3::new(0.0, 0.0, 10.0)).unwrap();
        assert!(v.yaw.abs() < 1e-6 && v.pitch.abs() < 1e-6);
        let v = look_at(o, V3::new(10.0, 0.0, 0.0)).unwrap();
        assert!((v.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let v = look_at(o, V3::new(0.0, -10.0, 10.0)).unwrap();
        assert!(
            (v.pitch - std::f32::consts::FRAC_PI_4).abs() < 1e-6,
            "down is positive"
        );
        assert!(look_at(o, o).is_none());
    }

    #[test]
    fn the_2009_numbers() {
        assert!((killcam_rest() - 45.0).abs() < 1e-3, "{}", killcam_rest());
        assert!((follow_radius() - 187.949).abs() < 1e-2);
    }

    #[test]
    fn killer_camera_moves_back_along_the_shot_and_settles_at_45_units() {
        // an FFA class: the follow camera may take the killer
        let death = |k, t, s| DeathView {
            any_team: true,
            ..death(k, t, s)
        };
        let mut cam = DeathCam::default();
        let body = V3::new(0.0, 40.0, 0.0);
        // the killer stands at +X 300 and shot along -X
        let killer = remote(1, 2, [300.0, 40.0, 0.0]);
        let shot = Some([-0.27, 0.0, 0.0]);
        let ks = std::slice::from_ref(&killer);
        let v0 = ViewAngles::default();
        let (p0, v) = cam
            .frame(
                Some(&death(Some(1), 0, shot)),
                ks,
                body,
                body,
                &v0,
                None,
                DT,
            )
            .expect("dead: the death camera");
        assert_eq!(cam.mode(), Some(DeathCamMode::Killer(1)));
        assert!((p0.y - (40.0 + KILLCAM_LIFT)).abs() < 1e-4, "{p0:?}");
        assert!(p0.x > 0.0 && p0.x < 6.0, "one frame's step: {p0:?}");
        assert!(
            (v.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4 && v.pitch.abs() < 1e-4,
            "facing the shooter (+X): {v:?}"
        );
        // one second later (any frame rate): close to the rest point 45 units toward the killer
        let mut p = p0;
        for i in 1..=60 {
            p = cam
                .frame(
                    Some(&death(Some(1), i * 16, shot)),
                    ks,
                    body,
                    body,
                    &v0,
                    None,
                    DT,
                )
                .unwrap()
                .0;
        }
        assert!((p.x - 45.0).abs() < 0.2, "settled: {p:?}");
        let mut slow = DeathCam::default();
        slow.frame(
            Some(&death(Some(1), 0, shot)),
            ks,
            body,
            body,
            &v0,
            None,
            0.0,
        );
        let q = slow
            .frame(
                Some(&death(Some(1), 1000, shot)),
                ks,
                body,
                body,
                &v0,
                None,
                1.0,
            )
            .unwrap()
            .0;
        assert!(
            (q.x - p.x).abs() < 0.5,
            "frame-rate independent: {q:?} vs {p:?}"
        );
        // the dying time: the follow camera takes the killer
        let (f, fv) = cam
            .frame(
                Some(&death(Some(1), DYING_TIME_MS, shot)),
                ks,
                body,
                body,
                &v0,
                None,
                DT,
            )
            .unwrap();
        assert_eq!(cam.mode(), Some(DeathCamMode::Follow(Some(1))));
        let anchor = V3::new(300.0, 40.0 + ANCHOR_LIFT, 0.0);
        let dist = (f - anchor).len();
        assert!(
            (dist - follow_radius() * (1.0 - CAM_PULL)).abs() < 0.1,
            "169 units from the killer: {dist}"
        );
        assert!(
            f.z < -100.0 && f.y > anchor.y + 60.0,
            "behind and above: {f:?}"
        );
        let look = look_at(f, anchor).unwrap();
        assert!((fv.yaw - look.yaw).abs() < 1e-5, "looking at the killer");
        // alive again: the own view
        assert!(cam.frame(None, &[], body, body, &v0, None, DT).is_none());
        assert_eq!(cam.mode(), None);
        assert_eq!(cam.deaths, 1);
    }

    #[test]
    fn follow_camera_takes_a_team_mate_else_the_own_body_and_orbits_with_the_view() {
        let mut cam = DeathCam::default();
        let body = V3::new(0.0, 40.0, 0.0);
        let enemy = remote(1, 2, [300.0, 40.0, 0.0]);
        let v0 = ViewAngles::default();
        let (p, _) = cam
            .frame(
                Some(&death(None, 0, None)),
                std::slice::from_ref(&enemy),
                body,
                body,
                &v0,
                None,
                DT,
            )
            .unwrap();
        assert_eq!(
            cam.mode(),
            Some(DeathCamMode::Follow(None)),
            "no killer, no mate: the own body"
        );
        let k = 1.0 - CAM_PULL;
        assert!(
            (p.z + FOLLOW_BACK * k).abs() < 1e-2 && (p.y - (60.0 + FOLLOW_UP * k)).abs() < 1e-2,
            "{p:?}"
        );
        let mate = remote(2, 1, [0.0, 40.0, 500.0]);
        let turned = ViewAngles {
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
        };
        let (q, _) = cam
            .frame(
                Some(&death(None, 20, None)),
                &[enemy, mate],
                body,
                body,
                &turned,
                None,
                DT,
            )
            .unwrap();
        assert_eq!(cam.mode(), Some(DeathCamMode::Follow(Some(2))));
        assert!(
            (q.x + FOLLOW_BACK * k).abs() < 1e-2 && (q.z - 500.0).abs() < 1e-2,
            "behind the mate along the turned view: {q:?}"
        );
    }

    #[test]
    fn a_followed_killer_who_is_gone_gives_way_to_a_team_mate() {
        let mut cam = DeathCam::default();
        let body = V3::ZERO;
        let killer = remote(1, 2, [300.0, 0.0, 0.0]);
        let mate = remote(3, 1, [0.0, 0.0, -400.0]);
        let v0 = ViewAngles::default();
        let shot = Some([-1.0, 0.0, 0.0]);
        cam.frame(
            Some(&death(Some(1), 0, shot)),
            &[killer, mate.clone()],
            body,
            body,
            &v0,
            None,
            DT,
        );
        cam.frame(
            Some(&death(Some(1), 3600, shot)),
            std::slice::from_ref(&mate),
            body,
            body,
            &v0,
            None,
            DT,
        );
        assert_eq!(cam.mode(), Some(DeathCamMode::Follow(Some(3))));
    }

    #[test]
    fn a_second_death_restarts_the_camera() {
        let mut cam = DeathCam::default();
        let k = remote(1, 2, [300.0, 0.0, 0.0]);
        let a = V3::new(0.0, 33.0, 0.0);
        let v0 = ViewAngles::default();
        let shot = Some([-1.0, 0.0, 0.0]);
        cam.frame(
            Some(&death(Some(1), 0, shot)),
            std::slice::from_ref(&k),
            a,
            a,
            &v0,
            None,
            DT,
        );
        cam.frame(
            Some(&death(Some(1), 5000, shot)),
            std::slice::from_ref(&k),
            a,
            a,
            &v0,
            None,
            DT,
        );
        // the death clock went back: a new death at another place, no killer: the own body
        let b = V3::new(900.0, 0.0, 0.0);
        let (p, _) = cam
            .frame(Some(&death(None, 10, None)), &[], b, b, &v0, None, DT)
            .unwrap();
        assert_eq!(cam.deaths, 2);
        assert!((p.x - 900.0).abs() < 1e-3, "{p:?}");
    }

    fn follow_dist(cam: &DeathCam, p: V3, target: V3) -> f32 {
        let _ = cam;
        (p - (target + V3::new(0.0, ANCHOR_LIFT, 0.0))).len()
    }

    #[test]
    fn in_a_team_mode_the_follow_camera_takes_team_mates_never_the_enemy_killer() {
        let mut cam = DeathCam::default();
        let body = V3::ZERO;
        let killer = remote(1, 2, [300.0, 0.0, 0.0]);
        let shot = Some([-1.0, 0.0, 0.0]);
        let v0 = ViewAngles::default();
        cam.frame(
            Some(&death(Some(1), 0, shot)),
            std::slice::from_ref(&killer),
            body,
            body,
            &v0,
            None,
            DT,
        );
        assert_eq!(cam.mode(), Some(DeathCamMode::Killer(1)));
        // after the dying time with no team mate: the own body, not the enemy killer
        cam.frame(
            Some(&death(Some(1), DYING_TIME_MS, shot)),
            std::slice::from_ref(&killer),
            body,
            body,
            &v0,
            None,
            DT,
        );
        assert_eq!(cam.mode(), Some(DeathCamMode::Follow(None)));
        // a team mate shows up: the camera takes him; a dead mate is skipped
        let mate = remote(4, 1, [0.0, 0.0, 500.0]);
        let fallen = remote(2, 1, [0.0, 0.0, -500.0]);
        let mut d = death(Some(1), DYING_TIME_MS + 20, shot);
        d.dead.insert(2);
        cam.frame(
            Some(&d),
            &[killer.clone(), fallen, mate],
            body,
            body,
            &v0,
            None,
            DT,
        );
        assert_eq!(cam.mode(), Some(DeathCamMode::Follow(Some(4))));
        // the FFA classes 6 and 9 may take anybody
        assert!(follows_any_team(Some(6), Some(1)) && follows_any_team(Some(9), Some(2)));
        assert!(!follows_any_team(Some(1), Some(1)) && !follows_any_team(Some(7), Some(2)));
        assert!(follows_any_team(Some(1), None) && follows_any_team(None, Some(0xff)));
    }

    #[test]
    fn left_and_right_buttons_cycle_the_team_mates_and_wrap() {
        let mut cam = DeathCam::default();
        let body = V3::ZERO;
        let v0 = ViewAngles::default();
        let all = [
            remote(3, 2, [100.0, 0.0, 0.0]),
            remote(6, 1, [0.0, 0.0, 600.0]),
            remote(2, 1, [0.0, 0.0, 200.0]),
            remote(4, 1, [0.0, 0.0, 400.0]),
            remote(5, 2, [0.0, 0.0, 900.0]),
        ];
        assert!(!cam.cycle(1), "alive: the button is the fire button");
        let t = std::cell::Cell::new(0u32);
        let step = |cam: &mut DeathCam, dir: i32| {
            if dir != 0 {
                assert!(cam.cycle(dir));
            }
            t.set(t.get() + 16);
            cam.frame(
                Some(&death(Some(3), t.get(), None)),
                &all,
                body,
                body,
                &v0,
                None,
                DT,
            );
            cam.mode()
        };
        assert_eq!(step(&mut cam, 0), Some(DeathCamMode::Killer(3)));
        // CLI-1193: a click during the dying time keeps the killer camera
        assert_eq!(step(&mut cam, 1), Some(DeathCamMode::Killer(3)));
        assert_eq!(step(&mut cam, -1), Some(DeathCamMode::Killer(3)));
        // after the dying time the follow camera takes the first team mate; then the buttons cycle
        t.set(DYING_TIME_MS);
        assert_eq!(step(&mut cam, 0), Some(DeathCamMode::Follow(Some(2))));
        assert_eq!(step(&mut cam, 1), Some(DeathCamMode::Follow(Some(4))));
        assert_eq!(step(&mut cam, 1), Some(DeathCamMode::Follow(Some(6))));
        assert_eq!(
            step(&mut cam, 1),
            Some(DeathCamMode::Follow(Some(2))),
            "wraps"
        );
        assert_eq!(
            step(&mut cam, -1),
            Some(DeathCamMode::Follow(Some(6))),
            "right: back, wraps"
        );
        assert_eq!(step(&mut cam, -1), Some(DeathCamMode::Follow(Some(4))));
        assert_eq!(
            step(&mut cam, 0),
            Some(DeathCamMode::Follow(Some(4))),
            "kept"
        );
        // never an enemy (3, 5)
        for _ in 0..7 {
            let m = step(&mut cam, 1);
            assert!(
                matches!(m, Some(DeathCamMode::Follow(Some(2 | 4 | 6)))),
                "{m:?}"
            );
        }
    }

    #[test]
    fn the_wheel_zooms_the_follow_camera_within_the_2009_limits() {
        let mut cam = DeathCam::default();
        let body = V3::ZERO;
        let v0 = ViewAngles::default();
        let mate = remote(2, 1, [0.0, 0.0, 500.0]);
        let mate_at = V3::new(0.0, 0.0, 500.0);
        assert!(!cam.wheel(1.0), "alive: the wheel is the weapon wheel");
        let frame = |cam: &mut DeathCam, t: u32| {
            cam.frame(
                Some(&death(None, t, None)),
                std::slice::from_ref(&mate),
                body,
                body,
                &v0,
                None,
                DT,
            )
            .unwrap()
            .0
        };
        let p = frame(&mut cam, 0);
        let k = 1.0 - CAM_PULL;
        assert!((follow_dist(&cam, p, mate_at) - follow_radius() * k).abs() < 0.05);
        assert!(cam.wheel(2.0));
        let p = frame(&mut cam, 16);
        assert!(
            (follow_dist(&cam, p, mate_at) - (follow_radius() + 80.0) * k).abs() < 0.05,
            "2 notches out"
        );
        cam.wheel(100.0);
        let p = frame(&mut cam, 32);
        assert!((cam.zoom() - ZOOM_OUT_MAX).abs() < 1e-4);
        assert!(
            (follow_dist(&cam, p, mate_at) - (follow_radius() + 400.0) * k).abs() < 0.05,
            "at most 400 out"
        );
        cam.wheel(-100.0);
        let p = frame(&mut cam, 48);
        assert!((cam.zoom() + follow_radius() * 0.5).abs() < 1e-3);
        assert!(
            (follow_dist(&cam, p, mate_at) - follow_radius() * 0.5 * k).abs() < 0.05,
            "half the radius in"
        );
        // a new target starts at no zoom
        let other = remote(4, 1, [0.0, 0.0, -500.0]);
        cam.cycle(1);
        cam.frame(
            Some(&death(None, 64, None)),
            &[mate.clone(), other],
            body,
            body,
            &v0,
            None,
            DT,
        );
        assert_eq!(cam.mode(), Some(DeathCamMode::Follow(Some(4))));
        assert_eq!(cam.zoom(), 0.0);
    }
}

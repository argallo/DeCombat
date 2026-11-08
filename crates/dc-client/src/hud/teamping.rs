//! Team pings, the client half (our own design, no 2009 counterpart): the Ping key marks the enemy under the
//! crosshair or the world point under it for the own team; the server's notices (`dc_wire::teamping`) show the
//! enemy's nametag (through walls: the server unculls him for the team) or a marker at the point for the times the
//! server names. Only after the server's offer at world entry; never in a free-for-all room (no offer there).
// DC-DEFAULT: CLI-1951 (proposed, project decision) the sending rule; CLI-1952 (proposed) the received pings

use dc_wire::teamping::{Notice, Request};

/// The shortest time between two own pings (the server keeps the same limit, DSV-651).
// DC-DEFAULT: CLI-1951 (proposed)
pub const COOLDOWN_MS: u32 = 1000;
/// How far the crosshair ray looks for a place (inside the server's 30000-unit reach from the body).
// DC-DEFAULT: CLI-1951 (proposed)
pub const PLACE_RANGE: f32 = 29_000.0;

/// A place marker of the team.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacePing {
    /// The pinging player's object.
    pub from: u16,
    pub pos: [f32; 3],
    /// The source clock (ms) at which it goes.
    pub until: u32,
}

/// The pings of this match.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamPings {
    offered: bool,
    /// Pinged enemies: object -> until (source clock, ms).
    enemies: Vec<(u16, u32)>,
    places: Vec<PlacePing>,
    last_sent: Option<u32>,
}

/// What lies under the crosshair at a Ping press.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AimHit {
    /// A remote character of another team.
    Enemy(u16),
    /// A world point (or a team mate's body: the point on it).
    Point([f32; 3]),
    Nothing,
}

/// What the HUD draws this frame: only pings whose time is not up.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PingView {
    pub enemies: Vec<u16>,
    pub places: Vec<PlacePing>,
}

/// `until` lies after `now` on the wrapping clock.
fn running(until: u32, now: u32) -> bool {
    (until.wrapping_sub(now) as i32) > 0
}

impl TeamPings {
    /// Whether the server offered pings in this match.
    pub fn offered(&self) -> bool {
        self.offered
    }

    /// A notice of the server at source time `now`. An offer starts the match's state afresh (a new world).
    // DC-DEFAULT: CLI-1952 (proposed) one tag per enemy (a new ping renews it), one marker per pinger (his new ping
    // moves it)
    pub fn on_notice(&mut self, n: Notice, now: u32) {
        match n {
            Notice::Offer { .. } => {
                *self = TeamPings {
                    offered: true,
                    ..TeamPings::default()
                }
            }
            Notice::Enemy { obj, ms, .. } => {
                let until = now.wrapping_add(u32::from(ms));
                self.enemies.retain(|(o, _)| *o != obj);
                self.enemies.push((obj, until));
            }
            Notice::Place { from, pos, ms } => {
                self.places.retain(|p| p.from != from);
                self.places.push(PlacePing {
                    from,
                    pos,
                    until: now.wrapping_add(u32::from(ms)),
                });
            }
        }
    }

    /// The world is left: no offer and no pings until the next offer.
    pub fn reset(&mut self) {
        *self = TeamPings::default();
    }

    /// The pings still shown at `now`; the expired ones are dropped.
    pub fn view(&mut self, now: u32) -> PingView {
        self.enemies.retain(|(_, u)| running(*u, now));
        self.places.retain(|p| running(p.until, now));
        PingView {
            enemies: self.enemies.iter().map(|(o, _)| *o).collect(),
            places: self.places.clone(),
        }
    }

    /// The request a Ping press sends at `now` (`None`: not offered, not alive, too soon, or nothing under the
    /// crosshair).
    // DC-DEFAULT: CLI-1951 (proposed)
    pub fn press(&mut self, alive: bool, now: u32, hit: AimHit) -> Option<Request> {
        if !self.offered || !alive {
            return None;
        }
        if self
            .last_sent
            .is_some_and(|t| now.wrapping_sub(t) < COOLDOWN_MS)
        {
            return None;
        }
        let q = match hit {
            AimHit::Enemy(obj) => Request::Enemy { obj },
            AimHit::Point(pos) => Request::Place { pos },
            AimHit::Nothing => return None,
        };
        self.last_sent = Some(now);
        Some(q)
    }
}

/// What the crosshair ray from `origin` along the unit `dir` meets: the nearest shown remote box before the world
/// (an enemy when `team_of` gives it a team other than `own_team`; a mate's box gives the point on it), else the
/// world point within [`PLACE_RANGE`].
// DC-DEFAULT: CLI-1951 (proposed) only a character of another known team is pinged as an enemy (the server checks
// again); without a known own team every character is a point
pub fn aim_hit(
    remotes: &[crate::world::RemoteBody],
    own_team: Option<u8>,
    team_of: &dyn Fn(&crate::world::RemoteBody) -> u8,
    origin: [f32; 3],
    dir: [f32; 3],
    world: Option<&dyn dc_sim::Collider>,
) -> AimHit {
    let targets: Vec<crate::arms::Target> = remotes
        .iter()
        .filter(|r| super::marks::shown(r))
        .map(|r| crate::arms::Target {
            obj: r.obj,
            centre: r.pos,
            half: r.half,
        })
        .collect();
    let at = |d: f32| {
        [
            origin[0] + dir[0] * d,
            origin[1] + dir[1] * d,
            origin[2] + dir[2] * d,
        ]
    };
    if let crate::arms::RayHit::Character { obj, dist, .. } =
        crate::arms::trace(origin, dir, PLACE_RANGE, &targets, world)
    {
        let enemy = remotes.iter().find(|r| r.obj == obj).is_some_and(|r| {
            let t = team_of(r);
            own_team.is_some_and(|o| matches!(o, 1 | 2) && matches!(t, 1 | 2) && t != o)
        });
        return if enemy {
            AimHit::Enemy(obj)
        } else {
            AimHit::Point(at(dist))
        };
    }
    match world.and_then(|w| crate::arms::world_hit(w, origin, dir, PLACE_RANGE)) {
        Some(d) => AimHit::Point(at(d)),
        None => AimHit::Nothing,
    }
}

/// A place marker on the screen this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PingMark {
    /// Virtual pixels (1024 x 768).
    pub at: [f32; 2],
    /// Clamped to the screen edge (behind the camera or off the screen).
    pub edge: bool,
    pub metres: i32,
    /// The pinger's name (empty: the own ping or unknown).
    pub name: String,
}

/// The inset of an edge-clamped marker from the screen border (virtual pixels).
pub const EDGE_INSET: f32 = 28.0;
/// Half the marker diamond's diagonal (virtual pixels).
pub const MARK_HALF: f32 = 7.0;

/// A world point's place for a marker: its projection when it is on the screen, else the point of the screen edge
/// (inset by [`EDGE_INSET`]) in its direction from the centre; `true` = clamped.
// DC-DEFAULT: CLI-1952 (proposed)
pub fn marker_place(camera: &dc_render::Camera, p: [f32; 3], size: (u32, u32)) -> ([f32; 2], bool) {
    let (w, h) = (
        dc_ui_data::VIRTUAL_WIDTH as f32,
        dc_ui_data::VIRTUAL_HEIGHT as f32,
    );
    if let Some(at) = super::project_on(camera, p, size) {
        if (EDGE_INSET..=w - EDGE_INSET).contains(&at[0])
            && (EDGE_INSET..=h - EDGE_INSET).contains(&at[1])
        {
            return (at, false);
        }
    }
    let c = camera.view_projection() * glam::Vec4::new(p[0], p[1], p[2], 1.0);
    // the clip x / y keep their screen direction behind the camera too (no division by a negative w)
    let mut d = [c.x * w * 0.5, -c.y * h * 0.5];
    if c.w < 0.0 && d[0].abs() + d[1].abs() < 1e-3 {
        d = [0.0, 1.0];
    }
    let (hx, hy) = (w * 0.5 - EDGE_INSET, h * 0.5 - EDGE_INSET);
    let t = (hx / d[0].abs().max(1e-6)).min(hy / d[1].abs().max(1e-6));
    ([w * 0.5 + d[0] * t, h * 0.5 + d[1] * t], true)
}

/// The draw items of the markers: a diamond in `colour` (a dark one behind it), the distance `<n> m` under it and
/// the pinger's name over it, in font record `font`. No art of ours.
// DC-DEFAULT: CLI-1952 (proposed)
pub fn push_items(
    out: &mut Vec<super::items::DrawItem>,
    marks: &[PingMark],
    colour: [u8; 4],
    font: i32,
) {
    use super::items::DrawItem;
    use super::layout::TextStyle;
    let style = |c: [u8; 4], size: i32| TextStyle {
        font,
        colour: c,
        size: Some(size),
    };
    for m in marks {
        let [x, y] = m.at;
        for (half, tint) in [(MARK_HALF + 2.0, [0, 0, 0, 200]), (MARK_HALF, colour)] {
            let side = half * std::f32::consts::SQRT_2;
            out.push(DrawItem::Image {
                tex: String::new(),
                src: [1.0, 1.0],
                dst: [x - side * 0.5, y - side * 0.5, side, side],
                tint,
                turn: Some(([x, y], std::f32::consts::FRAC_PI_4)),
            });
        }
        out.push(DrawItem::Label {
            style: style([255, 255, 255, 255], 0xe),
            pos: [x, y + MARK_HALF + 3.0],
            text: format!("{} m", m.metres),
            anchor: [0.5, 0.0],
        });
        if !m.name.is_empty() {
            out.push(DrawItem::Label {
                style: style(colour, 0xe),
                pos: [x, y - MARK_HALF - 3.0],
                text: m.name.clone(),
                anchor: [0.5, 1.0],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(obj: u16, team: u8, x: f32) -> crate::world::RemoteBody {
        crate::world::RemoteBody {
            obj,
            pid: obj as u8,
            name: format!("p{obj}"),
            team,
            pos: [x, 0.0, 500.0],
            half: [16.0, 50.0, 16.0],
            yaw: None,
            look: None,
        }
    }

    #[test]
    fn the_offer_gates_the_press_and_the_cooldown_holds() {
        let mut p = TeamPings::default();
        assert_eq!(p.press(true, 100, AimHit::Enemy(9)), None, "no offer");
        p.on_notice(
            Notice::Offer {
                enemy_ms: 3000,
                place_ms: 5000,
            },
            50,
        );
        assert!(p.offered());
        assert_eq!(p.press(false, 100, AimHit::Enemy(9)), None, "dead");
        assert_eq!(p.press(true, 100, AimHit::Nothing), None);
        assert_eq!(
            p.press(true, 100, AimHit::Enemy(9)),
            Some(Request::Enemy { obj: 9 })
        );
        assert_eq!(p.press(true, 900, AimHit::Point([1.0; 3])), None, "1 s");
        assert_eq!(
            p.press(true, 1100, AimHit::Point([1.0; 3])),
            Some(Request::Place { pos: [1.0; 3] })
        );
        p.reset();
        assert!(!p.offered());
        assert_eq!(p.press(true, 5000, AimHit::Enemy(9)), None);
    }

    #[test]
    fn an_enemy_tag_lasts_its_time_and_a_marker_expires() {
        let mut p = TeamPings::default();
        p.on_notice(
            Notice::Offer {
                enemy_ms: 3000,
                place_ms: 5000,
            },
            0,
        );
        p.on_notice(
            Notice::Enemy {
                from: 1,
                obj: 9,
                ms: 3000,
            },
            1000,
        );
        p.on_notice(
            Notice::Place {
                from: 1,
                pos: [0.0, 0.0, 10.0],
                ms: 5000,
            },
            1000,
        );
        assert_eq!(p.view(3999).enemies, vec![9]);
        assert_eq!(p.view(3999).places.len(), 1);
        assert!(p.view(4000).enemies.is_empty(), "3 s");
        assert_eq!(p.view(5999).places.len(), 1);
        assert!(p.view(6000).places.is_empty(), "5 s");
        // a pinger's new place moves his marker; the clock may wrap
        let t = u32::MAX - 10;
        for x in [1.0, 2.0] {
            p.on_notice(
                Notice::Place {
                    from: 4,
                    pos: [x, 0.0, 0.0],
                    ms: 5000,
                },
                t,
            );
        }
        let v = p.view(t.wrapping_add(100));
        assert_eq!(v.places.len(), 1);
        assert_eq!(v.places[0].pos[0], 2.0);
    }

    #[test]
    fn the_crosshair_pings_an_enemy_a_mate_point_or_nothing() {
        let remotes = [body(5, 2, 0.0), body(6, 1, 200.0)];
        let team = |r: &crate::world::RemoteBody| r.team;
        let ahead = [0.0, 0.0, 1.0];
        assert_eq!(
            aim_hit(&remotes, Some(1), &team, [0.0; 3], ahead, None),
            AimHit::Enemy(5)
        );
        // the mate's box: the point on it
        match aim_hit(&remotes, Some(1), &team, [200.0, 0.0, 0.0], ahead, None) {
            AimHit::Point(p) => assert!((p[2] - 484.0).abs() < 0.5, "{p:?}"),
            h => panic!("{h:?}"),
        }
        // no own team known: no enemy ping
        assert!(matches!(
            aim_hit(&remotes, None, &team, [0.0; 3], ahead, None),
            AimHit::Point(_)
        ));
        assert_eq!(
            aim_hit(&remotes, Some(1), &team, [0.0; 3], [0.0, 1.0, 0.0], None),
            AimHit::Nothing
        );
    }

    #[test]
    fn a_marker_off_the_screen_sits_on_its_edge() {
        let cam = dc_render::Camera::looking(glam::Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
        let size = (1024, 768);
        let (at, edge) = marker_place(&cam, [0.0, 0.0, 500.0], size);
        assert!(!edge);
        assert!(
            (at[0] - 512.0).abs() < 1.0 && (at[1] - 384.0).abs() < 1.0,
            "{at:?}"
        );
        // behind the camera, to one side: clamped to a side edge
        let (at, edge) = marker_place(&cam, [-300.0, 0.0, -500.0], size);
        assert!(edge);
        let on_side =
            (at[0] - EDGE_INSET).abs() < 1.0 || (at[0] - (1024.0 - EDGE_INSET)).abs() < 1.0;
        assert!(on_side, "{at:?}");
        // far to one side in front: the same side as its projection would be
        let (right, _) = marker_place(&cam, [5000.0, 0.0, 100.0], size);
        let (left, _) = marker_place(&cam, [-5000.0, 0.0, 100.0], size);
        assert!((right[0] - left[0]).abs() > 900.0, "{right:?} {left:?}");
    }
}

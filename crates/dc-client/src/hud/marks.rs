//! Names over the players' heads and the team-mate panel of the aim cross.
//!
//! The 2009 client keeps one world mark per roster player with a team (its `hud_marker` element: a name caption,
//! rank icons, a health bar for co-op enemies) and decides every frame which ones are shown. In a team match a team
//! mate's name is always shown while his head point is in front of the camera; an enemy's name only while he is the
//! object under the aim cross, and a dead player's name in the dead colour. The aim-cross element adds a panel with
//! the team mate's health while a living team mate is under the cross; the aim cross itself keeps its colour.
//!
//! [`aim_target`] finds the object under the cross (the frame loop's camera ray against the remote boxes, stopped by
//! the world), [`frame_marks`] applies the visibility rule, [`mate_target`] the panel rule; [`super::HudDriver::set_marks`]
//! runs the three once per frame and [`super::items::build`] draws the result.
// EVIDENCE: cshell+0x8a760 (one mark per player: none for an empty name; the roster adds it only for team 1 / 2,
// DISASM cshell+0x2ce24..0x2ce9c), cshell+0x8b2e0 (per-frame colour and visibility, DISASM cshell+0x8b591..0x8bfb2),
// cshell+0x8a420 (drawn while visible and the caption's place is not left of / above the screen),
// cshell+0x89310 (the caption point: the model's name socket, else the object position, projected; see
// [`NAME_SOCKET`]),
// cshell+0x2b27c0 (object under the cross: the first object a camera ray meets), cshell+0x826c0 / cshell+0x2a120
// (mate panel: the target's roster record has the own team and a health above 0)

use std::collections::BTreeMap;
use std::path::Path;

use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};

use super::state::HudState;
use crate::world::RemoteBody;

/// What a mark shows: the caption colour the 2009 marker picks for it.
// EVIDENCE: cshell+0x8b2e0 (dead: the dead colour; own team: the same-team colour; else the enemy colour, or the hit
// colour and its fade after an own hit)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkState {
    Mate,
    Enemy,
    Dead,
}

/// One name over a player's head this frame, at its virtual-screen point.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerMark {
    pub pid: u8,
    pub obj: u16,
    pub name: String,
    pub at: [f32; 2],
    pub state: MarkState,
}

/// The living team mate under the aim cross, with his health when the client knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MateTarget {
    pub pid: u8,
    pub health: Option<u32>,
}

/// Length of the camera ray that finds the object under the cross, world units.
// EVIDENCE: cshell+0x2b27c0 (segment of 100000 units along the camera's forward axis; a hit counts below 100000)
pub const AIM_TARGET_RANGE: f32 = 100_000.0;

/// The literal the 2009 marker asks the character's model for: the name of the attach point (socket) its caption
/// stands on. Read from the user's CShell.dll at run time ([`name_socket_from_rom`]); only its pin is here.
///
/// The 2009 rule (client-tags2): every frame the marker takes the object's position, looks the socket up on the
/// object's model by this name (once, the index is kept in the marker), and when it exists replaces the point by the
/// socket's **world-space** transform position (flag 1). The engine composes that transform from the socket's node
/// in the object's per-frame node cache (the posed skeleton: a node not yet evaluated this frame is evaluated with its
/// parents from the object's animation trackers) times the socket's own rotation and position. So the caption
/// follows the head as the animation moves it (running lean, crouch); the object position is used only for a model
/// without the socket.
// EVIDENCE: cshell+0x89310 (object position via the engine interface +0xa0; the socket index by name via the model
// interface +0x18 with the literal at cshell rva 0x3d657c, cached at marker +0x60; its transform via +0x1c with
// world space = 1; the transform's position replaces the point; then cshell+0x88e80 projects it),
// engine+0xd33b0 (socket by name: the attached models first, then the object's own model), engine+0xd2730 ->
// engine+0x6d350 (socket transform = node transform x socket rotation, position rotated by the node and added),
// engine+0x6d1d0 (node transform; world space keeps it, object space multiplies by the inverse object transform),
// engine+0x6d070 / +0x6ce60 / +0x6c4e0 (the node transform comes from the object's per-frame cache; an invalid node
// is marked with its parent chain and the cache is rebuilt from the object's tracker list)
pub const NAME_SOCKET: Pin = Pin {
    what: "name socket of the world mark",
    rva: 0x3d657c,
    sha16: "7202ac668f784538",
    len: 11,
};

/// The name of the caption's socket, read from the user's CShell.dll (`None`: the file or the pin does not match).
pub fn name_socket_from_rom(rom: &Path) -> Option<String> {
    let pe =
        dc_rom::PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1])).ok()?;
    read_pinned(&pe, &NAME_SOCKET).ok()
}

/// How far below the top of a remote's drawn box its name point lies when the posed name socket is not known,
/// world units.
// DC-DEFAULT: CLI-115 (amended by client-tags2, proposed) the 2009 point is the posed model's name socket
// ([`NAME_SOCKET`]); a remote drawn as a box (no model yet, a model without the socket) takes the box top minus 10.
// OBSERVED: CA_ROM default body 2026-10-25 (bind pose: the name socket on the head node lies 6.6 units above the eyes
// socket; the eyes stand 83 above the box bottom of the 100-unit box, CLI-40), so a standing player's point is about
// 90 above the box bottom = top - 10
pub const MARK_BELOW_TOP: f32 = 10.0;

/// The world point a remote's name is drawn at when its posed name socket is not known (see [`MARK_BELOW_TOP`]).
pub fn mark_point(r: &RemoteBody) -> [f32; 3] {
    [r.pos[0], r.pos[1] + r.half[1] - MARK_BELOW_TOP, r.pos[2]]
}

/// The world point of a remote's caption: its posed name socket from `points` (by object, the model scene's
/// [`crate::characters::ModelScene::name_points`]), else [`mark_point`].
// DC-DEFAULT: CLI-115 (amended), CLI-370 (proposed)
pub fn caption_point(r: &RemoteBody, points: &BTreeMap<u16, [f32; 3]>) -> [f32; 3] {
    points
        .get(&r.obj)
        .copied()
        .filter(|p| p.iter().all(|v| v.is_finite()))
        .unwrap_or_else(|| mark_point(r))
}

fn shown(r: &RemoteBody) -> bool {
    r.look.as_ref().is_none_or(|l| l.visible)
}

/// The remote object under the aim cross: the first remote box the ray from `origin` along `dir` meets before the
/// world stops it (`None`: nothing, or the world first).
// DC-DEFAULT: CLI-117 (proposed) the 2009 ray meets any object (cshell+0x2b27c0, engine segment test with the shell's
// filter); DeCombat tests the boxes drawn for the remote players (the shot boxes of CLI-43) and the world
pub fn aim_target(
    remotes: &[RemoteBody],
    origin: [f32; 3],
    dir: [f32; 3],
    world: Option<&dyn dc_sim::Collider>,
) -> Option<u16> {
    let targets: Vec<crate::arms::Target> = remotes
        .iter()
        .filter(|r| shown(r))
        .map(|r| crate::arms::Target {
            obj: r.obj,
            centre: r.pos,
            half: r.half,
        })
        .collect();
    match crate::arms::trace(origin, dir, AIM_TARGET_RANGE, &targets, world) {
        crate::arms::RayHit::Character { obj, .. } => Some(obj),
        crate::arms::RayHit::Miss => None,
    }
}

/// Mode class of free for all (GAME_MODES_2009.md section 2).
const FFA_CLASS: u8 = 6;

/// Mode class of Spy Hunt (GAME_MODES_2009.md section 5.6).
pub(crate) const SPY_CLASS: u8 = 9;

/// Whether the own player holds intel in Spy Hunt (a spy or the agent): such a player sees no names over heads and
/// no team-mate panel.
// EVIDENCE: cshell+0x8b2e0 (DISASM cshell+0x8bea7..0x8bfb2: with the own spy flag `local_player`+0x47a set, a mark of
// a player on the spy list (cshell+0x5ba60) is shown like a team mate's, so the 2009 spy saw the other spies' names);
// cshell+0x2bd80 (own sub 0 with a team other than 1 sets the flag, team 1 clears it)
// DC-DEFAULT: CLI-1580 (proposed, project decision "spies should not see anyone's nametags"; AMENDED
// 2026-10-31, playtest report "the enemy nametags don't show when shooting someone": a spy sees no standing mark (no
// fellow spies', no dead) and no mate panel, but the living player under his cross is marked as an enemy, so the
// hit flash (CLI-594) works for him as for a hunter); hunters keep the 2009 rule
fn own_spy(s: &HudState) -> bool {
    s.mode_class == Some(SPY_CLASS) && s.spy.own_spy
}

/// The own team for the mark rules: none in a free-for-all match (nobody is a mate there).
// SPEC: GAME_MODES_2009.md §5.4 (FFA: no teams visible)
// DC-DEFAULT: CLI-685 (proposed) in mode class 6 every other player is a foe for the marks and the mate panel,
// whatever team value the roster carries (DSV-148 gives every FFA player his own value 1..127)
fn own_team(s: &HudState) -> Option<u8> {
    // DC-DEFAULT: CLI-1598 (project decision) Spy Hunt's hunters are a team (fellow hunters' tags always,
    // spies and the agent as enemies under the cross, the mate panel); a spy or the agent has no mates (`own_spy`)
    if s.mode_class == Some(FFA_CLASS) {
        return None;
    }
    s.own_pid
        .and_then(|p| s.roster.get(&p))
        .map(|r| r.team)
        .filter(|t| matches!(t, 1 | 2))
}

/// A remote's team: the roster's record (0x6b), else the stream's value.
fn team_of(s: &HudState, r: &RemoteBody) -> u8 {
    s.roster.get(&r.pid).map(|b| b.team).unwrap_or(r.team)
}

/// CLI-1635: whether this remote is a team mate of the viewer, by the mark rules: the same own team as the roster, none
/// in a free-for-all or for a spy or agent of Spy Hunt.
pub fn is_mate(s: &HudState, r: &RemoteBody) -> bool {
    !own_spy(s) && own_team(s).is_some_and(|own| own == team_of(s, r))
}

/// The marks shown this frame: `project` maps a world point to the virtual screen (`None` behind the camera),
/// `target` is the object under the cross ([`aim_target`]), `points` the posed name sockets ([`caption_point`]).
///
/// Rule (2009 client, team match): no mark for the own player, for players without team 1 / 2 or without a name;
/// a team mate's mark is shown, an enemy's only while he is under the cross, a dead player's (either team) in the
/// dead colour; none behind the camera or with its point left of or above the screen.
// EVIDENCE: cshell+0x8b2e0 (DISASM cshell+0x8bea7..0x8bfb2: in front, then own team -> shown; dead -> shown; the
// target object -> shown; else hidden), cshell+0x8a420 (caption x >= 0 and y >= 0)
// DC-DEFAULT: CLI-118 (proposed) the 2009 rule's other reasons to show an enemy (the mode's objective carrier, a
// reveal state of the own player, cshell+0x5b9a0 / 0x5ba60 / player slot +0x94), the spectator rules and the own
// mark in a third-person view are not applied; the dead flag is the board's (0x6d, cleared by a spawn or 0x6b)
pub fn frame_marks(
    s: &HudState,
    remotes: &[RemoteBody],
    project: &dyn Fn([f32; 3]) -> Option<[f32; 2]>,
    target: Option<u16>,
    points: &BTreeMap<u16, [f32; 3]>,
) -> Vec<PlayerMark> {
    frame_marks_forced(s, remotes, project, target, points, &|_| false)
}

/// Whether this client knows a player as not alive: the board's dead flag (0x6d until a spawn / 0x6b) or a DSV-480
/// health of 0 (the server sends 0 while a player is not alive).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 effect 2 (0x6d marks the victim dead)
// DC-DEFAULT: CLI-1631 (amended, project decision) the x-ray tags and bars vanish at once on a death
pub fn not_alive(s: &HudState, pid: u8) -> bool {
    s.roster.get(&pid).is_some_and(|b| b.dead)
        || s.remote_health.get(&pid).is_some_and(|h| h.0 == 0)
}

/// [`frame_marks`] with the x-ray's forced enemy tags: an enemy whose object `forced` names gets the 2009 enemy mark
/// as if he were under the aim cross (same caption, point, colour and hit flash). A forced player who is not alive
/// ([`not_alive`]) or has no board row (he left) gets no mark at all, not even the 2009 dead one; team mates are
/// never forced (they keep the 2009 mate rule).
// DC-DEFAULT: CLI-1631 (amended, project decision "the enemy nametags should just be the actual nametags the
// game displays when you are aimed at the enemy ... When the enemy dies it's nametag and healthbar should disappear")
pub fn frame_marks_forced(
    s: &HudState,
    remotes: &[RemoteBody],
    project: &dyn Fn([f32; 3]) -> Option<[f32; 2]>,
    target: Option<u16>,
    points: &BTreeMap<u16, [f32; 3]>,
    forced: &dyn Fn(u16) -> bool,
) -> Vec<PlayerMark> {
    let own = own_team(s);
    let mut out = Vec::new();
    // CLI-1580: a spy has no mates and sees no standing marks; the player under his cross is an enemy
    let spy = own_spy(s);
    let own = if spy { None } else { own };
    for r in remotes {
        if Some(r.pid) == s.own_pid || !shown(r) {
            continue;
        }
        // an AI character (the NPC id, a caption) is marked like any other entry: its caption and rank icon follow the
        // 2009 rules below (an enemy under the cross), and super::npcbar adds its health bar
        // SPEC: FIRETEAM_2009.md §1.4 (the marker add gives an AI with a caption a caption element and two rank icons;
        // the entry is drawn while its elements exist and its point is valid)
        // DC-DEFAULT: CLI-1870 (USER DECISION 2026-11-05: follow the 2009 evidence; the 2026-11-04 no-tag direction is
        // reversed)
        let team = team_of(s, r);
        // DC-DEFAULT: CLI-685 (proposed) FFA team values run past 2: there any value but 0 is a player's
        let ffa = s.mode_class == Some(FFA_CLASS);
        if !(matches!(team, 1 | 2) || (ffa && team != 0)) {
            continue;
        }
        let row = s.roster.get(&r.pid);
        let name = row
            .map(|b| b.name.clone())
            .unwrap_or_else(|| r.name.clone());
        if name.is_empty() {
            continue;
        }
        let dead = row.is_some_and(|b| b.dead);
        let mate = own == Some(team);
        // CLI-1631 (amended): a forced enemy is tagged as if aimed at; dead or gone, no tag at all
        let forced = !mate && forced(r.obj);
        if forced && (row.is_none() || not_alive(s, r.pid)) {
            continue;
        }
        let aimed = target == Some(r.obj) || forced;
        if spy {
            if dead || !aimed {
                continue;
            }
        } else if !(mate || dead || aimed) {
            continue;
        }
        let Some(at) = project(caption_point(r, points)) else {
            continue;
        };
        if at[0] < 0.0 || at[1] < 0.0 || !at[0].is_finite() || !at[1].is_finite() {
            continue;
        }
        let state = if dead {
            MarkState::Dead
        } else if mate {
            MarkState::Mate
        } else {
            MarkState::Enemy
        };
        out.push(PlayerMark {
            pid: r.pid,
            obj: r.obj,
            name,
            at,
            state,
        });
    }
    out
}

/// The own hit of this frame: when an own hit was confirmed since the last step ([`HudState::own_hit`]), the object
/// under the cross (`target`) takes the hit colour from `now` (a second hit restarts it). Entries whose colour has run
/// out are dropped. Returns whether anything changed.
// EVIDENCE: cshell+0x8b2e0 (per mark, each frame: while the marker's one-frame hit flag (+0x44, cleared at the end of
// the step) is set and the mark's object is the object under the cross, the hit state starts: its time = the layout's
// hit time, its start = now; when the hit time has passed the fade starts (fade time, start = now); when the fade time
// has passed the mark is back to its plain colour; times are the engine clock's seconds)
// DC-DEFAULT: CLI-594 (proposed) the flag's writer is not traced (the 0x2c8b4a feedback block has no clean C); ours
// sets it from the own shot record's hit feedback, the same record that plays the hit sound
pub fn step_hits(
    s: &mut HudState,
    target: Option<u16>,
    now: u32,
    layout: &super::layout::MarkLayout,
) -> bool {
    let mut changed = false;
    if std::mem::take(&mut s.own_hit) {
        if let Some(obj) = target {
            s.mark_hits.insert(obj, now);
            changed = true;
        }
    }
    let span = ((layout.hit_secs.max(0.0) + layout.fade_secs.max(0.0)) * 1000.0) as u32;
    let before = s.mark_hits.len();
    s.mark_hits.retain(|_, t| now.saturating_sub(*t) <= span);
    changed || s.mark_hits.len() != before
}

/// The caption colour of a living enemy's mark at `now`: the hit colour for the layout's hit time after an own hit
/// (`hit_at`), then a fade back to the enemy colour over the fade time, then the enemy colour.
// EVIDENCE: cshell+0x8b2e0 (enemy mark: in the hit state the hit colour, layout set 0; in the fade state four colour
// components computed and clamped to 0..255; else the enemy colour); the per-component blend is not legible in the
// clean C (x87 values): DC-DEFAULT CLI-595 (proposed) a linear blend from the hit colour to the enemy colour by the
// fade's elapsed fraction. Not ported: a hit while the hit colour holds flips a second flag of the mark that picks a
// computed colour instead of the hit colour (a blink); ours restarts the hold
pub fn mark_colour(layout: &super::layout::MarkLayout, hit_at: Option<u32>, now: u32) -> [u8; 4] {
    let Some(t) = hit_at else {
        return layout.enemy;
    };
    let age = now.saturating_sub(t) as f32 / 1000.0;
    let hold = layout.hit_secs.max(0.0);
    let fade = layout.fade_secs.max(0.0);
    if age < hold {
        return layout.hit;
    }
    if age >= hold + fade {
        return layout.enemy;
    }
    let f = (age - hold) / fade.max(1e-6);
    let mut c = [0u8; 4];
    for (i, v) in c.iter_mut().enumerate() {
        let a = layout.hit[i] as f32;
        let b = layout.enemy[i] as f32;
        *v = (a + (b - a) * f).round().clamp(0.0, 255.0) as u8;
    }
    c
}

/// The team-mate panel's subject: the living team mate under the cross while the own player is alive.
// EVIDENCE: cshell+0x826c0 (target object -> roster record with the own team -> health; shown when above 0),
// cshell+0x853f0 (drawn only while the own player state is 1)
// DC-DEFAULT: CLI-119 (proposed) DeCombat keeps no team mate's health: the panel is shown for a mate the board does not
// list as dead, without the number
pub fn mate_target(
    s: &HudState,
    remotes: &[RemoteBody],
    target: Option<u16>,
) -> Option<MateTarget> {
    if s.life_state != 1 || own_spy(s) {
        return None;
    }
    let own = own_team(s)?;
    let r = remotes.iter().find(|r| Some(r.obj) == target)?;
    if Some(r.pid) == s.own_pid || team_of(s, r) != own {
        return None;
    }
    if s.roster.get(&r.pid).is_some_and(|b| b.dead) {
        return None;
    }
    Some(MateTarget {
        pid: r.pid,
        health: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::BoardRow;

    fn body(obj: u16, pid: u8, team: u8, x: f32) -> RemoteBody {
        RemoteBody {
            obj,
            pid,
            name: format!("p{pid}"),
            team,
            pos: [x, 50.0, 300.0],
            half: [16.0, 50.0, 16.0],
            yaw: None,
            look: None,
        }
    }

    fn state() -> HudState {
        let mut s = HudState {
            life_state: 1,
            own_pid: Some(0),
            ..HudState::default()
        };
        for (pid, team, name) in [
            (0u8, 1u8, "me"),
            (1, 1, "mate"),
            (2, 2, "foe"),
            (3, 0, "none"),
        ] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    name: name.into(),
                    team,
                    ..BoardRow::default()
                },
            );
        }
        s
    }

    fn none() -> BTreeMap<u16, [f32; 3]> {
        BTreeMap::new()
    }

    fn flat(p: [f32; 3]) -> Option<[f32; 2]> {
        (p[2] > 0.0).then_some([512.0 + p[0], 384.0 - p[1]])
    }

    #[test]
    fn mates_always_enemies_only_under_the_cross_dead_in_grey() {
        let s = state();
        let remotes = [
            body(10, 1, 1, -100.0),
            body(11, 2, 2, 100.0),
            body(12, 3, 0, 0.0),
        ];
        let m = frame_marks(&s, &remotes, &flat, None, &none());
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(
            (m[0].pid, m[0].name.as_str(), m[0].state),
            (1, "mate", MarkState::Mate)
        );
        assert_eq!(m[0].at, [412.0, 384.0 - (50.0 + 50.0 - MARK_BELOW_TOP)]);
        // the enemy under the cross gets his mark; a no-team player never
        let m = frame_marks(&s, &remotes, &flat, Some(11), &none());
        assert_eq!(
            m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(),
            [(1, MarkState::Mate), (2, MarkState::Enemy)]
        );
        assert!(frame_marks(&s, &remotes, &flat, Some(12), &none())
            .iter()
            .all(|x| x.pid != 3));
        // a dead enemy is marked in the dead colour without being aimed at
        let mut d = s.clone();
        d.roster.get_mut(&2).unwrap().dead = true;
        let m = frame_marks(&d, &remotes, &flat, None, &none());
        assert!(m.iter().any(|x| x.pid == 2 && x.state == MarkState::Dead));
        // behind the camera or left of the screen: nothing
        let mut back = remotes.clone();
        back[0].pos[2] = -300.0;
        assert!(frame_marks(&s, &back, &flat, None, &none()).is_empty());
        let mut left = remotes.clone();
        left[0].pos[0] = -600.0;
        assert!(frame_marks(&s, &left, &flat, None, &none()).is_empty());
    }

    /// CLI-1870 (2009 evidence, FIRETEAM_2009.md §1.4): an AI with a caption is marked like any entry: an enemy AI
    /// under the cross only, an AI of the own team always, the caption being the create's.
    #[test]
    fn an_ai_with_a_caption_gets_the_2009_mark() {
        let s = state();
        let ai = |obj: u16, team: u8, x: f32| RemoteBody {
            pid: crate::net::NPC_PID,
            name: "Guard".into(),
            ..body(obj, 0, team, x)
        };
        let remotes = [ai(20, 2, 0.0), ai(21, 1, 100.0)];
        let m = frame_marks(&s, &remotes, &flat, None, &none());
        assert_eq!(
            m.iter()
                .map(|x| (x.obj, x.name.as_str(), x.state))
                .collect::<Vec<_>>(),
            [(21, "Guard", MarkState::Mate)]
        );
        let m = frame_marks(&s, &remotes, &flat, Some(20), &none());
        assert_eq!(
            m.iter().map(|x| (x.obj, x.state)).collect::<Vec<_>>(),
            [(20, MarkState::Enemy), (21, MarkState::Mate)]
        );
        // no caption, no entry (FIRETEAM_2009.md §1.4)
        let mut bare = remotes.clone();
        bare[0].name.clear();
        assert!(frame_marks(&s, &bare[..1], &flat, Some(20), &none()).is_empty());
    }

    /// CLI-685: in a free-for-all match nobody is a mate: a player of the own team value is marked only under the
    /// cross, and a player with an FFA value past 2 is marked under the cross too.
    #[test]
    fn ffa_marks_every_player_as_a_foe_under_the_cross() {
        let mut s = state();
        s.mode_class = Some(6);
        s.roster.insert(
            5,
            BoardRow {
                pid: 5,
                name: "bot".into(),
                team: 5,
                ..BoardRow::default()
            },
        );
        let remotes = [body(10, 1, 1, -100.0), body(14, 5, 5, 60.0)];
        assert!(frame_marks(&s, &remotes, &flat, None, &none()).is_empty());
        let m = frame_marks(&s, &remotes, &flat, Some(14), &none());
        assert_eq!(
            m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(),
            [(5, MarkState::Enemy)]
        );
        let m = frame_marks(&s, &remotes, &flat, Some(10), &none());
        assert_eq!(
            m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(),
            [(1, MarkState::Enemy)]
        );
        assert!(mate_target(&s, &remotes, Some(10)).is_none());
    }

    /// CLI-1580 (project decision): in Spy Hunt an own spy (or the agent) sees no name at all and no mate
    /// panel; a hunter keeps the team rule (fellow hunters always, a spy under the cross).
    #[test]
    fn spy_hunt_a_spy_sees_no_names_a_hunter_sees_his_side() {
        let mut s = state();
        s.mode_class = Some(9);
        let remotes = [body(10, 1, 1, -100.0), body(11, 2, 2, 100.0)];
        // a hunter (team 1): the fellow hunter always (CLI-1598: the hunters are a team), the spy under the cross
        let m = frame_marks(&s, &remotes, &flat, Some(11), &none());
        assert_eq!(
            m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(),
            [(1, MarkState::Mate), (2, MarkState::Enemy)]
        );
        assert!(mate_target(&s, &remotes, Some(10)).is_some());
        // the own player seizes intel: team 2 and the own spy flag. No standing mark; the player under the cross
        // (a hunter or a fellow spy: everyone hurts everyone, DSV-453) is an enemy, so a hit flashes his tag
        s.roster.get_mut(&0).unwrap().team = 2;
        s.spy.own_spy = true;
        assert!(frame_marks(&s, &remotes, &flat, None, &none()).is_empty());
        for (t, pid) in [(Some(10), 1), (Some(11), 2)] {
            let m = frame_marks(&s, &remotes, &flat, t, &none());
            assert_eq!(
                m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(),
                [(pid, MarkState::Enemy)],
                "{t:?}"
            );
        }
        let mut d = s.clone();
        d.roster.get_mut(&1).unwrap().dead = true;
        assert!(frame_marks(&d, &remotes, &flat, None, &none()).is_empty(), "no dead names either");
        assert!(frame_marks(&d, &remotes, &flat, Some(10), &none()).is_empty(), "nor a dead player under the cross");
        assert!(mate_target(&s, &remotes, Some(11)).is_none(), "no panel over a fellow spy");
        // the flag outside class 9 changes nothing
        s.mode_class = Some(1);
        assert!(!frame_marks(&s, &remotes, &flat, None, &none()).is_empty());
    }

    /// CLI-1631 (amended, project decision): a forced enemy gets the 2009 enemy mark as if aimed at; a mate
    /// is never forced; a forced player who is dead (board flag or health 0) or has no board row gets no mark at all.
    #[test]
    fn a_forced_enemy_gets_the_aimed_at_mark_and_none_when_dead_or_gone() {
        let mut s = state();
        let remotes = [body(10, 1, 1, -100.0), body(11, 2, 2, 100.0), body(15, 7, 2, 0.0)];
        let all = |_: u16| true;
        let m = frame_marks_forced(&s, &remotes, &flat, None, &none(), &all);
        assert_eq!(
            m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(),
            [(1, MarkState::Mate), (2, MarkState::Enemy)],
            "pid 7 has no board row (left): nothing"
        );
        // the same mark as the enemy under the cross: point, name, state
        assert_eq!(m[1], frame_marks(&s, &remotes, &flat, Some(11), &none())[1]);
        // dead by the board flag: no tag (not even the grey one); the dead mate keeps his 2009 dead tag
        let mut d = s.clone();
        d.roster.get_mut(&2).unwrap().dead = true;
        d.roster.get_mut(&1).unwrap().dead = true;
        let m = frame_marks_forced(&d, &remotes, &flat, Some(11), &none(), &all);
        assert_eq!(m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(), [(1, MarkState::Dead)]);
        // dead by the DSV-480 health 0, back with a health above 0
        s.remote_health.insert(2, (0, 100));
        assert!(frame_marks_forced(&s, &remotes, &flat, None, &none(), &all).iter().all(|x| x.pid != 2));
        s.remote_health.insert(2, (1, 100));
        assert!(frame_marks_forced(&s, &remotes, &flat, None, &none(), &all).iter().any(|x| x.pid == 2));
    }

    #[test]
    fn the_caption_stands_on_the_posed_name_socket_else_on_the_box_point() {
        // EVIDENCE: cshell+0x89310 (the socket's world position replaces the object position when the model has it)
        let s = state();
        let remotes = [body(10, 1, 1, -100.0), body(13, 4, 1, 40.0)];
        let mut s = s;
        s.roster.insert(
            4,
            BoardRow {
                pid: 4,
                name: "other".into(),
                team: 1,
                ..BoardRow::default()
            },
        );
        // obj 10's head leans 12 units forward and sits 3 lower (a running pose); obj 13 has no posed point
        let posed = BTreeMap::from([(10u16, [-88.0, 87.0, 300.0])]);
        let m = frame_marks(&s, &remotes, &flat, None, &posed);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(m[0].at, [512.0 - 88.0, 384.0 - 87.0], "the posed socket");
        assert_eq!(
            m[1].at,
            [512.0 + 40.0, 384.0 - (50.0 + 50.0 - MARK_BELOW_TOP)],
            "the box point without a posed socket"
        );
        // a non-finite posed point is ignored
        let bad = BTreeMap::from([(10u16, [f32::NAN, 87.0, 300.0])]);
        let m = frame_marks(&s, &remotes, &flat, None, &bad);
        assert_eq!(m[0].at, [412.0, 384.0 - (50.0 + 50.0 - MARK_BELOW_TOP)]);
    }

    /// A 0x6b record for the roster events of the tests below.
    fn record(pid: u8, team: u8, dead: bool) -> dc_botclient::shell::RosterRecord {
        dc_botclient::shell::RosterRecord {
            name: format!("p{pid}"),
            pid,
            team,
            kind: 2,
            dead,
            complete: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_respawn_refresh_clears_the_dead_look_of_a_mark() {
        use crate::hud::state::HudEvent;
        // EVIDENCE: cshell+0x8a760 (a record of a known client id sets the marker's flag again), spec 3.3.1 effect 2
        // (0x6d marks the victim dead): before this every player who died once kept the dead colour for good
        let mut s = state();
        let remotes = [body(10, 1, 1, -100.0), body(11, 2, 2, 100.0)];
        let kill = |victim: u8| {
            crate::hud::state::HudEvent::Kill(dc_botclient::shell::KillRecord {
                victim,
                killer: 0,
                weapon: 145,
                hit_zone: 3,
                complete: true,
                ..Default::default()
            })
        };
        s.apply(&kill(1), 10, 0.0);
        s.apply(&kill(2), 10, 0.0);
        let m = frame_marks(&s, &remotes, &flat, None, &none());
        assert_eq!(
            m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(),
            [(1, MarkState::Dead), (2, MarkState::Dead)],
            "both dead after the kills (the dead enemy is named without being aimed at)"
        );
        // the respawn refreshes (kind 2, dead 0): the mate is a mate again, the enemy is hidden again
        s.apply(&HudEvent::Roster(record(1, 1, false)), 20, 0.0);
        s.apply(&HudEvent::Roster(record(2, 2, false)), 20, 0.0);
        let m = frame_marks(&s, &remotes, &flat, None, &none());
        assert_eq!(
            m.iter().map(|x| (x.pid, x.state)).collect::<Vec<_>>(),
            [(1, MarkState::Mate)]
        );
        let m = frame_marks(&s, &remotes, &flat, Some(11), &none());
        assert_eq!(m.last().map(|x| x.state), Some(MarkState::Enemy));
        // a record that says dead 1 (the join record of a player not yet spawned) greys him again; a record kind
        // that changes nothing (3) leaves the flag alone
        s.apply(&HudEvent::Roster(record(1, 1, true)), 30, 0.0);
        assert!(s.roster[&1].dead);
        let mut other = record(1, 1, false);
        other.kind = 3;
        s.apply(&HudEvent::Roster(other), 40, 0.0);
        assert!(s.roster[&1].dead, "kind 3 changes no record");
    }

    #[test]
    fn mate_panel_only_for_a_living_mate_while_alive() {
        let s = state();
        let remotes = [body(10, 1, 1, -100.0), body(11, 2, 2, 100.0)];
        assert_eq!(
            mate_target(&s, &remotes, Some(10)),
            Some(MateTarget {
                pid: 1,
                health: None
            })
        );
        assert_eq!(mate_target(&s, &remotes, Some(11)), None);
        assert_eq!(mate_target(&s, &remotes, None), None);
        let mut dead_me = s.clone();
        dead_me.life_state = 2;
        assert_eq!(mate_target(&dead_me, &remotes, Some(10)), None);
        let mut dead_mate = s.clone();
        dead_mate.roster.get_mut(&1).unwrap().dead = true;
        assert_eq!(mate_target(&dead_mate, &remotes, Some(10)), None);
    }

    #[test]
    fn the_cross_finds_the_nearest_box_on_its_ray() {
        let remotes = [body(10, 1, 1, 0.0), {
            let mut b = body(11, 2, 2, 0.0);
            b.pos[2] = 600.0;
            b
        }];
        // from the eye height straight along +Z: the nearer box (obj 10) first
        let eye = [0.0, 60.0, 0.0];
        assert_eq!(aim_target(&remotes, eye, [0.0, 0.0, 1.0], None), Some(10));
        assert_eq!(aim_target(&remotes, eye, [0.0, 0.0, -1.0], None), None);
        assert_eq!(
            aim_target(&remotes[1..], eye, [0.0, 0.0, 1.0], None),
            Some(11)
        );
    }

    #[test]
    fn an_own_hit_flashes_the_aimed_enemys_tag_then_fades_back_at_any_frame_rate() {
        // arms-fixes2 (user 2026-10-27: "when you damage the enemy their name tag should flash brighter red")
        let mut layout = crate::hud::layout::MarkLayout::fallback();
        layout.enemy = [200, 30, 30, 255];
        layout.hit = [255, 90, 90, 255];
        layout.hit_secs = 1.5;
        layout.fade_secs = 0.25;
        let foe = body(20, 2, 2, 0.0);
        // the colour of the foe's tag over 2.5 s after one confirmed hit at 1000 ms, stepping at `fps`
        let run = |fps: u32| -> Vec<(u32, [u8; 4])> {
            let mut s = state();
            let mut out = Vec::new();
            let mut t = 0.0f64;
            let dt = 1000.0 / fps as f64;
            let mut hit_sent = false;
            while t < 3500.0 {
                let now = t as u32;
                if !hit_sent && now >= 1000 {
                    s.apply(&crate::hud::state::HudEvent::OwnHit, now, 0.0);
                    hit_sent = true;
                }
                step_hits(&mut s, Some(foe.obj), now, &layout);
                out.push((
                    now,
                    mark_colour(&layout, s.mark_hits.get(&foe.obj).copied(), now),
                ));
                t += dt;
            }
            out
        };
        for fps in [30, 60, 144, 240] {
            let v = run(fps);
            let at = |ms: u32| v.iter().rev().find(|(t, _)| *t <= ms).unwrap().1;
            let hit_at = v.iter().find(|(t, _)| *t >= 1000).unwrap().0;
            assert_eq!(at(999), layout.enemy, "{fps} fps: before the hit");
            assert_eq!(
                at(hit_at + 1),
                layout.hit,
                "{fps} fps: the hit colour at once"
            );
            assert_eq!(at(hit_at + 1490), layout.hit, "{fps} fps: held 1.5 s");
            // half-way through the fade: half-way between the two colours (within one frame step of the fade)
            let mid = at(hit_at + 1625);
            let step = (55.0 * (1000.0 / fps as f32) / 250.0).ceil() as i32 + 1;
            assert!(
                (mid[1] as i32 - 60).abs() <= step && (mid[0] as i32 - 228).abs() <= step,
                "{fps} fps: mid-fade {mid:?}"
            );
            let frame = 1000 / fps + 1;
            assert_eq!(
                at(hit_at + 1750 + frame),
                layout.enemy,
                "{fps} fps: back after the fade"
            );
            assert!(v
                .iter()
                .filter(|(t, _)| *t > hit_at + 1760)
                .all(|(_, c)| *c == layout.enemy));
        }
        // a hit with nothing under the cross marks nothing; the flag is used up by the step
        let mut s = state();
        s.apply(&crate::hud::state::HudEvent::OwnHit, 0, 0.0);
        assert!(!step_hits(&mut s, None, 0, &layout));
        assert!(s.mark_hits.is_empty() && !s.own_hit);
        step_hits(&mut s, Some(foe.obj), 10, &layout);
        assert!(
            s.mark_hits.is_empty(),
            "a later frame's target does not take the old hit"
        );
    }
}

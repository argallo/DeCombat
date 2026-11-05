//! Fire Team (mode class 5) hit feedback on AI characters: the health bar over an AI and the own points popup.
//!
//! The 2009 client gives an AI with a caption one world-mark entry with a caption, two rank icons and a health bar
//! (FIRETEAM_2009.md §1.4); the bar's fill is the AI's health percent, set at the create and by each 0xa6 type 0x18
//! sub 0x24 note (§1.5), and it is drawn only in class 5 for an AI of another team than the viewer's. The caption and
//! rank icon follow the 2009 marker rules ([`super::marks::frame_marks_forced`], CLI-1870: the user's decision of
//! 2026-11-05 to follow the 2009 evidence); the bar shows while the AI is under the aim cross or for [`BAR_SHOW_MS`]
//! after its health changed (CLI-1871), hanging just below the caption's point so the two do not overlap. The bar is
//! our own two filled boxes: the 2009 element's textures and places come from `NPC_HPBar*` keys of the layout's co-op
//! section that are not pinned here yet.
//!
//! 0xd7 sub 1 (a kill) and sub 3 (a hit) give the own player a points popup (the 2009 client's marker popup with the
//! gained points, S2C §3.5 row 0xd7) and keep each player's total (sub 1 writes the record's points, CLI-1872).
// SPEC: FIRETEAM_2009.md §1.3 (the marker add of the NPC branch), §1.4 (empty caption: no entry; the bar drawn in
// class 5 for another team), §1.5 (the percent; the bar setter fills `trunc(width x value / 100)`, nothing for 0),
// §2.2 rows 1 and 3 (points; the popup for the own pid)

use std::collections::BTreeMap;

use super::items::DrawItem;
use super::state::HudState;
use crate::world::RemoteBody;

/// How long after a health change the bar of an AI stays up when the AI is not under the aim cross.
// DC-DEFAULT: CLI-1871 (proposed)
pub const BAR_SHOW_MS: u32 = 2_000;

/// The bar's size on the 1024 x 768 virtual screen (width, height).
// DC-DEFAULT: CLI-1871 (proposed)
pub const BAR_SIZE: [f32; 2] = [40.0, 5.0];

/// The gap between the caption's point and the bar's top edge (virtual pixels): the caption's line ends on the point
/// (flags 0x24), so the bar hangs below it.
// DC-DEFAULT: CLI-1871 (proposed, amended 2026-11-05 with the caption back, CLI-1870)
pub const BAR_GAP: f32 = 2.0;

/// The bar's back and fill colours (RGBA).
// DC-DEFAULT: CLI-1871 (proposed)
pub const BAR_BACK: [u8; 4] = [0, 0, 0, 170];
pub const BAR_FILL: [u8; 4] = [214, 38, 28, 255];

/// The mode class of Fire Team (GAME_MODES_2009.md §6).
const COOP_CLASS: u8 = 5;

/// How long the own points popup shows and how far it rises (virtual pixels) in that time.
// DC-DEFAULT: CLI-1872 (proposed)
pub const POPUP_MS: u32 = 1_200;
pub const POPUP_RISE: f32 = 24.0;
/// The popup's start point (virtual screen): above the aim cross.
// DC-DEFAULT: CLI-1872 (proposed)
pub const POPUP_AT: [f32; 2] = [512.0, 340.0];
/// The popup's text colour (RGB; the alpha fades out over the last half of its time) and size.
// DC-DEFAULT: CLI-1872 (proposed)
pub const POPUP_COLOUR: [u8; 4] = [255, 214, 64, 255];
pub const POPUP_SIZE: i32 = 16;

/// One AI's bar this frame at its virtual-screen point (the caption's point; the bar's top centre is [`BAR_GAP`]
/// below it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpcBar {
    pub obj: u16,
    pub at: [f32; 2],
    pub percent: u8,
}

/// An AI's last health note: its percent and when it changed (ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NpcHealth {
    pub percent: u8,
    pub changed_at: u32,
}

/// The own points popup: the points gained and when they came (ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointsPopup {
    pub gained: u32,
    pub at: u32,
}

/// Whether a drawn remote is an AI character: the NPC id of [`crate::net::drawn_characters`] (an AI has no roster
/// record) with a caption. Corpses and thrown objects carry the same id with no name.
// DC-DEFAULT: CLI-1857 (proposed) the AI's classification by its record (no roster binding, the NPC tail)
pub fn is_npc(r: &RemoteBody) -> bool {
    r.pid == crate::net::NPC_PID && !r.name.is_empty()
}

/// A 0xa6 sub 0x24 note: the AI's percent; a changed value restarts its show time.
pub fn on_health(map: &mut BTreeMap<u16, NpcHealth>, obj: u16, percent: u8, now: u32) {
    match map.get_mut(&obj) {
        Some(h) if h.percent == percent => {}
        Some(h) => {
            *h = NpcHealth {
                percent,
                changed_at: now,
            }
        }
        None => {
            map.insert(
                obj,
                NpcHealth {
                    percent,
                    changed_at: now,
                },
            );
        }
    }
}

/// The bars shown this frame: for every shown AI of another team than the own one in class 5, while it is the
/// object under the cross (`target`) or within [`BAR_SHOW_MS`] of its last health change; its point is the caption
/// point the name marks use ([`super::marks::caption_point`]), `project` maps it to the virtual screen. An AI
/// without a note shows full (the create's value for a new AI).
// SPEC: FIRETEAM_2009.md §1.4 (draw rule: class 5, another team), §1.3 step 3 (the create's health)
// DC-DEFAULT: CLI-1871 (proposed) the show rule (the 2009 marker shows an enemy's entry while it is under the cross)
pub fn frame_bars(
    s: &HudState,
    remotes: &[RemoteBody],
    project: &dyn Fn([f32; 3]) -> Option<[f32; 2]>,
    target: Option<u16>,
    points: &BTreeMap<u16, [f32; 3]>,
) -> Vec<NpcBar> {
    if s.mode_class != Some(COOP_CLASS) {
        return Vec::new();
    }
    let own_team = s.own_pid.and_then(|p| s.roster.get(&p)).map(|r| r.team);
    let mut out = Vec::new();
    for r in remotes.iter().filter(|r| is_npc(r)) {
        if !r.look.as_ref().is_none_or(|l| l.visible) || own_team == Some(r.team) {
            continue;
        }
        let h = s.npc_health.get(&r.obj);
        let recent = h.is_some_and(|h| s.now_ms.saturating_sub(h.changed_at) <= BAR_SHOW_MS);
        if target != Some(r.obj) && !recent {
            continue;
        }
        let Some(at) = project(super::marks::caption_point(r, points)) else {
            continue;
        };
        if !(at[0].is_finite() && at[1].is_finite()) || at[0] < 0.0 || at[1] < 0.0 {
            continue;
        }
        out.push(NpcBar {
            obj: r.obj,
            at,
            percent: h.map_or(100, |h| h.percent),
        });
    }
    out
}

/// The filled width of a bar of `width` at `percent`: `trunc(width x percent / 100)`, nothing at 0.
// SPEC: FIRETEAM_2009.md §1.5 (bar setter cshell+0x6d800)
pub fn fill_width(width: f32, percent: u8) -> f32 {
    if percent == 0 {
        return 0.0;
    }
    (width * f32::from(percent.min(100)) / 100.0).trunc()
}

/// The draw items of the frame's bars: per bar the back box, its top centre [`BAR_GAP`] below the point (under the
/// caption), and the fill from the left.
pub fn bar_items(bars: &[NpcBar], out: &mut Vec<DrawItem>) {
    let [w, h] = BAR_SIZE;
    for b in bars {
        let (x, y) = (b.at[0] - w / 2.0, b.at[1] + BAR_GAP);
        out.push(DrawItem::Fill {
            rect: [x, y, w, h],
            colour: BAR_BACK,
        });
        let fill = fill_width(w, b.percent);
        if fill > 0.0 {
            out.push(DrawItem::Fill {
                rect: [x, y, fill, h],
                colour: BAR_FILL,
            });
        }
    }
}

/// 0xd7 sub 1 / 3: `pid`'s total; for the own pid a popup with the gained points (none for 0).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.5 row 0xd7 (sub 1 writes the record's points, sub 3 does not; the own pid gets
// the marker popup with `x`)
// DC-DEFAULT: CLI-1872 (proposed) a newer popup replaces the shown one; the total is kept for sub 1 only
pub fn on_points(s: &mut HudState, pid: u32, gained: u32, total: u32, record: bool, now: u32) {
    if record {
        s.coop_points.insert(pid, total);
    }
    if s.own_pid.map(u32::from) != Some(pid) {
        return;
    }
    if gained > 0 {
        s.points_popup = Some(PointsPopup { gained, at: now });
    }
    // CLI-1883 / CLI-1882: the score shown and the own kill run
    super::coopscore::on_own_points(s, total, record, now);
}

/// The draw items of the own points popup at `now`: `+N` rising from [`POPUP_AT`], fading out over its second half.
pub fn popup_items(s: &HudState, out: &mut Vec<DrawItem>) {
    let Some(p) = s.points_popup else {
        return;
    };
    let age = s.now_ms.saturating_sub(p.at);
    if age >= POPUP_MS {
        return;
    }
    let f = age as f32 / POPUP_MS as f32;
    let alpha = if f < 0.5 { 1.0 } else { (1.0 - f) * 2.0 };
    let mut colour = POPUP_COLOUR;
    colour[3] = (f32::from(colour[3]) * alpha).round() as u8;
    out.push(DrawItem::Label {
        style: super::layout::TextStyle {
            font: -1,
            size: Some(POPUP_SIZE),
            colour,
        },
        pos: [POPUP_AT[0], POPUP_AT[1] - POPUP_RISE * f],
        text: format!("+{}", p.gained),
        anchor: [0.5, 1.0],
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::BoardRow;

    fn npc(obj: u16, team: u8) -> RemoteBody {
        RemoteBody {
            obj,
            pid: crate::net::NPC_PID,
            name: "Soldier".into(),
            team,
            pos: [0.0, 50.0, 300.0],
            half: [16.0, 50.0, 16.0],
            yaw: None,
            look: None,
        }
    }

    fn coop(now: u32) -> HudState {
        let mut s = HudState {
            life_state: 1,
            own_pid: Some(0),
            mode_class: Some(5),
            now_ms: now,
            ..HudState::default()
        };
        s.roster.insert(
            0,
            BoardRow {
                pid: 0,
                name: "me".into(),
                team: 1,
                ..BoardRow::default()
            },
        );
        s
    }

    fn centre(_: [f32; 3]) -> Option<[f32; 2]> {
        Some([512.0, 300.0])
    }

    #[test]
    fn a_bar_shows_under_the_cross_and_for_two_seconds_after_a_hit() {
        let mut s = coop(10_000);
        let ais = [npc(40, 2), npc(41, 2)];
        let none = BTreeMap::new();
        // nothing aimed at, no note: no bar
        assert!(frame_bars(&s, &ais, &centre, None, &none).is_empty());
        // under the cross: full without a note
        let b = frame_bars(&s, &ais, &centre, Some(41), &none);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].obj, b[0].percent), (41, 100));
        // a hit on 40: its bar shows with the note's percent for 2 s, then goes
        on_health(&mut s.npc_health, 40, 63, 10_000);
        s.now_ms = 11_900;
        let b = frame_bars(&s, &ais, &centre, None, &none);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].obj, b[0].percent), (40, 63));
        s.now_ms = 12_001;
        assert!(frame_bars(&s, &ais, &centre, None, &none).is_empty());
        // the same percent again does not restart the time
        on_health(&mut s.npc_health, 40, 63, 12_001);
        assert!(frame_bars(&s, &ais, &centre, None, &none).is_empty());
        on_health(&mut s.npc_health, 40, 20, 12_001);
        assert_eq!(frame_bars(&s, &ais, &centre, None, &none).len(), 1);
    }

    #[test]
    fn no_bar_outside_class_5_for_the_own_team_players_or_hidden_ai() {
        let mut s = coop(0);
        let none = BTreeMap::new();
        // an AI of the own team
        assert!(frame_bars(&s, &[npc(40, 1)], &centre, Some(40), &none).is_empty());
        // a roster player under the cross
        let mut p = npc(40, 2);
        p.pid = 3;
        assert!(frame_bars(&s, &[p], &centre, Some(40), &none).is_empty());
        // a corpse (the NPC id, no name)
        let mut c = npc(40, 2);
        c.name.clear();
        assert!(frame_bars(&s, &[c], &centre, Some(40), &none).is_empty());
        // a hidden AI (dead, parked)
        let mut h = npc(40, 2);
        h.look = Some(crate::world::RemoteLook {
            models: Vec::new(),
            styles: Vec::new(),
            weapon: None,
            trackers: Vec::new(),
            visible: false,
            aim: None,
        });
        assert!(frame_bars(&s, &[h], &centre, Some(40), &none).is_empty());
        // another mode class
        s.mode_class = Some(1);
        assert!(frame_bars(&s, &[npc(40, 2)], &centre, Some(40), &none).is_empty());
    }

    #[test]
    fn the_fill_truncates_as_the_2009_setter() {
        assert_eq!(fill_width(40.0, 100), 40.0);
        assert_eq!(fill_width(40.0, 63), 25.0);
        assert_eq!(fill_width(40.0, 1), 0.0);
        assert_eq!(fill_width(40.0, 0), 0.0);
        let mut out = Vec::new();
        bar_items(
            &[NpcBar {
                obj: 1,
                at: [100.0, 50.0],
                percent: 50,
            }],
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                DrawItem::Fill {
                    rect: [80.0, 52.0, 40.0, 5.0],
                    colour: BAR_BACK
                },
                DrawItem::Fill {
                    rect: [80.0, 52.0, 20.0, 5.0],
                    colour: BAR_FILL
                },
            ]
        );
    }

    /// The headless frame (CPU compositor, our own test font) of an AI under the cross at half health: its caption
    /// stands on the point as an enemy's mark (CLI-1870, 2009 evidence), its half filled bar hangs just below it, at
    /// 1024 x 768 and 2560 x 1440.
    #[test]
    fn a_headless_frame_shows_the_ai_caption_and_bar() {
        use crate::testkit::synth_font;
        for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
            let mut hud = crate::hud::Hud::fallback(w, h, Some((synth_font(), 0)));
            let mut s = coop(5_000);
            // the test font draws only A and B
            let mut ai = [npc(40, 2)];
            ai[0].name = "ABBA".into();
            let none = BTreeMap::new();
            let at = |_: [f32; 3]| Some([300.0, 200.0]);
            s.marks = crate::hud::marks::frame_marks(&s, &ai, &at, Some(40), &none);
            assert_eq!(s.marks.len(), 1, "a caption over the AI under the cross");
            assert_eq!(s.marks[0].name, "ABBA");
            assert_eq!(s.marks[0].state, crate::hud::marks::MarkState::Enemy);
            assert_eq!(s.marks[0].at, [300.0, 200.0]);
            on_health(&mut s.npc_health, 40, 50, 5_000);
            s.npc_bars = frame_bars(&s, &ai, &at, Some(40), &none);
            assert_eq!(s.npc_bars.len(), 1);
            let img = hud.render(&s);
            let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
            let px = |vx: f32, vy: f32| img.pixel((vx * sx) as u32, (vy * sy) as u32);
            // the bar: x 280..320, y 202..207; the fill 280..300
            let filled = px(285.0, 204.5);
            assert_eq!(filled[..3], BAR_FILL[..3], "{w}x{h}: fill {filled:?}");
            let back = px(315.0, 204.5);
            assert_eq!(back[..3], [0, 0, 0], "{w}x{h}: back {back:?}");
            assert!(back[3] >= 150, "{w}x{h}: back {back:?}");
            // the caption in the enemy colour stands above the point (its bottom on y 200), none below the bar
            let enemy = hud.layout.marks.enemy;
            let is_caption = |x: u32, y: u32| {
                let p = img.pixel(x, y);
                p[3] > 200 && p[..3] == enemy[..3]
            };
            let (x0, x1) = ((200.0 * sx) as u32, (400.0 * sx) as u32);
            let above = (0..(201.0 * sy) as u32).any(|y| (x0..x1).any(|x| is_caption(x, y)));
            assert!(above, "{w}x{h}: the caption above the bar");
            let below = ((208.0 * sy) as u32..h).any(|y| (x0..x1).any(|x| is_caption(x, y)));
            assert!(!below, "{w}x{h}: nothing in the enemy colour under the bar");
        }
    }

    #[test]
    fn own_points_give_a_popup_and_totals_are_kept() {
        let mut s = coop(1000);
        // another player's kill: his total, no popup
        on_points(&mut s, 3, 40, 120, true, 1000);
        assert_eq!(s.coop_points.get(&3), Some(&120));
        assert_eq!(s.points_popup, None);
        // an own hit (sub 3): popup, no record write
        on_points(&mut s, 0, 10, 10, false, 1000);
        assert_eq!(s.coop_points.get(&0), None);
        assert_eq!(
            s.points_popup,
            Some(PointsPopup {
                gained: 10,
                at: 1000
            })
        );
        let mut out = Vec::new();
        popup_items(&s, &mut out);
        assert!(
            matches!(&out[..], [DrawItem::Label { text, .. }] if text == "+10"),
            "{out:?}"
        );
        // an own kill replaces it and writes the total
        on_points(&mut s, 0, 40, 50, true, 1500);
        assert_eq!(s.coop_points.get(&0), Some(&50));
        assert_eq!(s.points_popup.map(|p| p.gained), Some(40));
        // gone after its time
        s.now_ms = 1500 + POPUP_MS;
        let mut out = Vec::new();
        popup_items(&s, &mut out);
        assert!(out.is_empty());
    }
}

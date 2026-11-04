//! The half-time "teams change sides" picture of the clan classes (3, 4, 8, 13): after a round result that reaches
//! half of the match, a looping 15-frame animation and one still image show for 3 seconds.
//!
//! The 2009 HUD element (cshell+0xcd210 decides, cshell+0xcd170 draws, cshell+0xcd4d0 builds it) keeps two flags, the
//! picture is up and it was shown once (classes 4, 8 and 13), and the kind of still image (1: the mission image of
//! class 3, 2: the other classes' image).
// EVIDENCE: cshell+0xcd210 (the test), cshell+0xcd170 (the draw: frame steps every 100 ms over the 15 frames, back to
// the first after the last; both pictures end 3000 ms after the stamp; the still image of the kind), cshell+0xcd4d0
// (the 15 frames are the layout's animation form with the index as two digits, the animation place and size; the
// two still images with their own texture, place and size), cshell+0xcd330 (cleared by the match settings 0x7d),
// cshell+0x49a40 case 0x46 (called with the received `round_limit`, the two counters while the latch of 0x45 is clear)

use super::items::DrawItem;
use super::layout::Img;
use super::state::HudState;
use dc_ui_data::{HudKey as K, HudView};

/// The animation's frames.
pub const FRAMES: u32 = 15;
/// How long one frame stays and how long the picture stays.
pub const FRAME_MS: u32 = 100;
pub const SHOW_MS: u32 = 3000;

/// The layout of the picture.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TeamChangeLayout {
    /// The frame texture form (the layout value; `<form><index as two digits>.dtx`) and the animation's place and size.
    pub frame_form: Option<String>,
    pub frame_pos: [i32; 2],
    pub frame_size: [i32; 2],
    /// The still images: kind 1 (class 3) and kind 2 (the other classes).
    pub stills: [Option<Img>; 2],
}

impl TeamChangeLayout {
    pub fn load(v: &HudView<'_>) -> Self {
        let still = |t: K, p: K, s: K| {
            Some(Img {
                tex: v.text(t)?.to_string(),
                pos: v.pair(p)?,
                size: v.pair(s)?,
            })
        };
        TeamChangeLayout {
            frame_form: v.text(K::TeamChangeAniTex).map(str::to_string),
            frame_pos: v.pair(K::TeamChangeAniPos).unwrap_or_default(),
            frame_size: v.pair(K::TeamChangeAniSize).unwrap_or_default(),
            stills: [
                still(
                    K::TeamChangeMissionTex,
                    K::TeamChangeMissionPos,
                    K::TeamChangeMissionSize,
                ),
                still(
                    K::TeamChangeTdmTex,
                    K::TeamChangeTdmPos,
                    K::TeamChangeTdmSize,
                ),
            ],
        }
    }
}

/// What the HUD state keeps of the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TeamChange {
    /// The picture is up since `since` (milliseconds of the HUD clock).
    pub shown: bool,
    /// Classes 4, 8 and 13: it was shown once this match.
    pub once: bool,
    /// The still image: 1 or 2 (0: none yet).
    pub kind: u8,
    pub since: u32,
}

impl TeamChange {
    /// The match settings 0x7d clear both flags.
    pub fn reset(&mut self) {
        self.shown = false;
        self.once = false;
    }

    /// A round result with `limit` (the last 0x44's `round_limit`), the two counters of the 0x46 and the round number
    /// of the last 0x44, in the mode `class`, at `now`.
    pub fn on_round_result(
        &mut self,
        class: Option<u8>,
        limit: u32,
        wins: [u32; 2],
        round_no: u32,
        now: u32,
    ) {
        let half = limit / 2;
        if limit == wins[0] || limit == wins[1] {
            self.shown = false;
            return;
        }
        let start = match class {
            Some(3) => {
                self.kind = 1;
                half == wins[0] + wins[1]
            }
            Some(4) => {
                self.kind = 2;
                !self.once && (half <= wins[0] || half <= wins[1])
            }
            Some(8) => {
                self.kind = 2;
                !self.once
            }
            Some(13) => {
                self.kind = 2;
                half == round_no
            }
            _ => false,
        };
        if start {
            self.shown = true;
            if class != Some(3) {
                self.once = true;
            }
            self.since = now;
        }
    }
}

/// Draws the picture: the frame the elapsed time names, then the still image of the kind.
// DC-DEFAULT: CLI-1164 (proposed) the frame is the elapsed time over 100 ms modulo 15 (the 2009 draw steps one frame
// per draw after 100 ms, which is the same at any frame rate above 10)
pub fn draw(out: &mut Vec<DrawItem>, l: &TeamChangeLayout, s: &HudState) {
    let t = &s.team_change;
    if !t.shown {
        return;
    }
    let elapsed = s.now_ms.wrapping_sub(t.since);
    if elapsed >= SHOW_MS {
        return;
    }
    if let Some(form) = &l.frame_form {
        let frame = (elapsed / FRAME_MS) % FRAMES;
        out.push(DrawItem::Image {
            tex: format!("{form}{frame:02}.dtx"),
            src: [l.frame_size[0] as f32, l.frame_size[1] as f32],
            dst: [
                l.frame_pos[0] as f32,
                l.frame_pos[1] as f32,
                l.frame_size[0] as f32,
                l.frame_size[1] as f32,
            ],
            tint: [255; 4],
            turn: None,
        });
    }
    if let Some(Some(i)) = t.kind.checked_sub(1).and_then(|k| l.stills.get(k as usize)) {
        out.push(DrawItem::Image {
            tex: i.tex.clone(),
            src: [i.size[0] as f32, i.size[1] as f32],
            dst: [
                i.pos[0] as f32,
                i.pos[1] as f32,
                i.size[0] as f32,
                i.size[1] as f32,
            ],
            tint: [255; 4],
            turn: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 2009 test, case by case (cshell+0xcd210).
    #[test]
    fn the_half_time_test_follows_the_client() {
        let mut t = TeamChange::default();
        // class 3: the counters sum to half the limit
        t.on_round_result(Some(3), 6, [2, 1], 0, 10);
        assert!(t.shown && t.kind == 1 && !t.once);
        let mut t = TeamChange::default();
        t.on_round_result(Some(3), 6, [2, 0], 0, 10);
        assert!(!t.shown);
        // a counter at the limit: no picture
        let mut t = TeamChange {
            shown: true,
            ..Default::default()
        };
        t.on_round_result(Some(3), 6, [6, 0], 0, 10);
        assert!(!t.shown);
        // class 8: the first result only
        let mut t = TeamChange::default();
        t.on_round_result(Some(8), 6, [0, 0], 0, 10);
        assert!(t.shown && t.once && t.kind == 2);
        t.shown = false;
        t.on_round_result(Some(8), 6, [1, 0], 0, 20);
        assert!(!t.shown, "once");
        // class 4: the first time a counter reaches half
        let mut t = TeamChange::default();
        t.on_round_result(Some(4), 10, [4, 1], 0, 10);
        assert!(!t.shown);
        t.on_round_result(Some(4), 10, [5, 1], 0, 20);
        assert!(t.shown && t.since == 20);
        // class 13: the round number is half the limit
        let mut t = TeamChange::default();
        t.on_round_result(Some(13), 4, [1, 0], 1, 10);
        assert!(!t.shown);
        t.on_round_result(Some(13), 4, [1, 0], 2, 30);
        assert!(t.shown && t.once);
        // other classes never
        let mut t = TeamChange::default();
        t.on_round_result(Some(12), 4, [1, 0], 2, 30);
        assert!(!t.shown);
        // the match settings clear it
        let mut t = TeamChange {
            shown: true,
            once: true,
            kind: 2,
            since: 0,
        };
        t.reset();
        assert!(!t.shown && !t.once);
    }

    /// The HUD events drive it: a clan class-8 round result while the 0x45 latch is clear shows the picture, the
    /// latch holds it back, and the match settings 0x7d clear the once flag.
    #[test]
    fn a_clan_round_result_shows_the_picture_through_the_hud_events() {
        use super::super::state::HudEvent;
        let mut s = HudState {
            mode_class: Some(8),
            ..HudState::default()
        };
        s.apply(
            &HudEvent::RoundNumbers {
                round_no: 1,
                round_limit: 4,
            },
            100,
            0.0,
        );
        let result = HudEvent::RoundResult {
            round_secs: 5,
            win_team: 1,
            wins: [2, 0],
        };
        s.banner_latch = true;
        s.apply(&result, 200, 0.0);
        assert!(!s.team_change.shown, "the latch holds it back");
        s.banner_latch = false;
        s.apply(&result, 300, 0.0);
        assert!(s.team_change.shown && s.team_change.since == 300);
        s.apply(&HudEvent::Goal(4), 400, 0.0);
        assert!(!s.team_change.shown && !s.team_change.once);
    }

    /// The picture: the frame by elapsed time, the still of the kind, gone after 3 seconds.
    #[test]
    fn the_picture_runs_three_seconds() {
        let l = TeamChangeLayout {
            frame_form: Some("f".into()),
            frame_pos: [1, 2],
            frame_size: [30, 40],
            stills: [
                Some(Img {
                    tex: "m.dtx".into(),
                    pos: [5, 6],
                    size: [7, 8],
                }),
                Some(Img {
                    tex: "t.dtx".into(),
                    pos: [9, 10],
                    size: [11, 12],
                }),
            ],
        };
        let mut s = HudState::default();
        s.team_change = TeamChange {
            shown: true,
            once: true,
            kind: 2,
            since: 1000,
        };
        let names = |s: &HudState| {
            let mut out = Vec::new();
            draw(&mut out, &l, s);
            out.iter()
                .filter_map(|d| match d {
                    DrawItem::Image { tex, .. } => Some(tex.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        s.now_ms = 1050;
        assert_eq!(names(&s), vec!["f00.dtx", "t.dtx"]);
        s.now_ms = 1000 + 14 * 100 + 5;
        assert_eq!(names(&s)[0], "f14.dtx");
        s.now_ms = 1000 + 15 * 100 + 5;
        assert_eq!(names(&s)[0], "f00.dtx", "back to the first frame");
        s.now_ms = 1000 + 3000;
        assert!(names(&s).is_empty());
        s.now_ms = 1100;
        s.team_change.kind = 1;
        assert_eq!(names(&s), vec!["f01.dtx", "m.dtx"]);
    }
}

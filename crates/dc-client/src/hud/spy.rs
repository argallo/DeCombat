//! Spy Hunt on the client (mode class 9, the bounty row): 0xcb `GS_BOUNTY_MODE` moves players between the hunters
//! (team 1) and the spies (team 2) with their seized-intel count, sets the total / dropped / spy counters, orders
//! the intel notices, switches the stage (with a banner), resets the agent's weapon slots and sets the stage clock;
//! the HUD draws the mode's element (title, notice, stage message, dropped and spy counters, the own seized / total
//! intel, the seize animation) and plays the intel sounds.
// SPEC: GAME_MODES_2009.md §5.6 (0xcb subs 0..6); GAME_MESSAGES_2009_S2C.md §3.5 row 0xcb
// EVIDENCE: cshell+0x2bd80 (0xcb: sub 0 sets the record's team; own team 1 clears the own spy flag and the seized
// count, own other team shows the seize animation, the count and notice 1 in phase 1 else 0, sets the flag; another
// player's spy team shows notice 3 in phase 1 else 2; sub 1 the three counters; sub 3 / sub 4 notices 4 / 5 outside
// phase 1; sub 5 the phase, in phase 1 a 4000 ms banner and notice 1 (own spy) or 3; sub 6 the clock (seconds, then
// the digits' colour, cshell+0xbe0c0)), cshell+0x70880 (the element's items and places), cshell+0x70760 (draw:
// title, notice while its 5 s run, stage message, dropped / spy icons and digits, the seize animation, and for the
// own living player the seized / total block), cshell+0x71a90 (stage message by phase and own spy flag),
// cshell+0x71c30 (notice n: its texture, 5 s, the sound of states 0 / 2, 1 / 3, 4)

use super::banners::Voice;
use super::items::DrawItem;
use super::layout::{Announce, Img};
use super::state::HudState;
use dc_ui_data::{HudKey as K, HudSection, HudView};

/// One 0xcb message.
#[derive(Debug, Clone, PartialEq)]
pub enum SpyEvent {
    /// Sub 0: `pid` joins `team` (1 hunters, else spies) holding `value` intel.
    Team {
        value: u32,
        team: u8,
        pid: u32,
    },
    /// Sub 1: the total, dropped and spy counters.
    Counts {
        total: u32,
        dropped: u32,
        spies: u32,
    },
    /// Sub 2: the own weapon slots reset (the agent's weapons follow).
    Loadout,
    /// Sub 3: intel dropped; sub 4: intel back at a spawn point.
    Dropped,
    Respawned,
    /// Sub 5: the stage, with the banner for phase 1.
    Phase {
        phase: u8,
        title: String,
        text: String,
    },
    /// Sub 6: the clock's seconds and colour.
    Timer {
        secs: u32,
        colour: u32,
    },
}

/// The six notices in the element's order (cshell+0x71c30 state n = texture n).
// EVIDENCE: cshell+0x70880 (textures at +0x18..+0x2c in this order)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpyNote {
    Seize,
    SeizeAll,
    Stolen,
    StolenAll,
    Drop,
    Respawn,
}

/// How long a notice stays.
// EVIDENCE: cshell+0x71c30 (+0x1f8 := 5.0)
pub const NOTE_SECS: f32 = 5.0;
/// Phase 1 (stage 2, the agent).
pub const PHASE_AGENT: u8 = 1;

/// The Spy Hunt HUD state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpyHud {
    pub phase: u8,
    /// The own player is a spy (`local_player`+0x47a).
    pub own_spy: bool,
    pub seized: u32,
    pub total: u32,
    pub dropped: u32,
    pub spies: u32,
    pub note: Option<(SpyNote, u32)>,
    /// Start of the seize animation.
    pub success: Option<u32>,
}

/// The textures and places of the Spy Hunt element.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpyLayout {
    pub title: Option<Img>,
    pub notes: Vec<Option<String>>,
    pub note_place: Option<([i32; 2], [i32; 2])>,
    /// Stage 1 hunter / spy, stage 2 hunter / spy.
    pub stages: [Option<String>; 4],
    pub stage_place: Option<([i32; 2], [i32; 2])>,
    pub dropped: Option<Img>,
    pub dropped_num: Option<Img>,
    pub spy: Option<Img>,
    pub spy_num: Option<Img>,
    pub seized: Option<Img>,
    pub seized_num: Option<Img>,
    pub delimiter: Option<Img>,
    pub total_num: Option<Img>,
    pub success: Option<Announce>,
}

fn img(v: &HudView<'_>, tex: K, size: K, pos: K) -> Option<Img> {
    Some(Img {
        tex: v.text(tex)?.to_string(),
        pos: v.pair(pos)?,
        size: v.pair(size)?,
    })
}

impl SpyLayout {
    pub fn load(v: &HudView<'_>) -> Self {
        let b = HudSection::SpyHunt;
        // the title is centred across the 1024-wide screen at the key's y
        // EVIDENCE: cshell+0x70880 (x = (0x400 - width) / 2)
        let title = img(v, K::BntModeTex, K::BntModeRect, K::BntModePos).map(|mut i| {
            i.pos[0] = (1024 - i.size[0]) / 2;
            i
        });
        let text = |k: K| v.text(k).map(str::to_string);
        let success = (|| {
            let size = v.pair(K::BntSuccessSize)?;
            let pos = v.pair(K::BntSuccessPos)?;
            let secs = |k: K| v.real(k).unwrap_or(0.0) as f32;
            Some(Announce {
                tex: v.text(K::BntSuccessImg)?.to_string(),
                size,
                centre: pos,
                total: secs(K::BntSuccessTotal),
                create: secs(K::BntSuccessCreate),
                end: secs(K::BntSuccessEnd),
                destroy: secs(K::BntSuccessDestroy),
                first_scale: secs(K::BntSuccessStartSize),
            })
        })();
        SpyLayout {
            title,
            notes: [
                K::BntSeizeMsgTex,
                K::BntSeizeAllMsgTex,
                K::BntStolenMsgTex,
                K::BntStolenAllMsgTex,
                K::BntDropMsgTex,
                K::BntRespawnMsgTex,
            ]
            .into_iter()
            .map(text)
            .collect(),
            note_place: v
                .pair_in(b, K::QrtNotePos)
                .zip(v.pair_in(b, K::QrtNoteSize)),
            stages: [
                text(K::BntStage1HunterTex),
                text(K::BntStage1SpyTex),
                text(K::BntStage2HunterTex),
                text(K::BntStage2SpyTex),
            ],
            stage_place: v.pair(K::BntStagePos).zip(v.pair(K::BntStageRect)),
            dropped: img(v, K::BntDroppedTex, K::BntDroppedRect, K::BntDroppedPos),
            dropped_num: img(
                v,
                K::BntDroppedNumTex,
                K::BntDroppedNumRect,
                K::BntDroppedNumPos,
            ),
            spy: img(v, K::BntSpyTex, K::BntSpyRect, K::BntSpyPos),
            spy_num: img(v, K::BntSpyNumTex, K::BntSpyNumRect, K::BntSpyNumPos),
            seized: img(v, K::BntSeizedTex, K::BntSeizedRect, K::BntSeizedPos),
            seized_num: img(
                v,
                K::BntSeizedNumTex,
                K::BntSeizedNumRect,
                K::BntSeizedNumPos,
            ),
            delimiter: img(v, K::BntDelimTex, K::BntDelimRect, K::BntDelimPos),
            total_num: img(v, K::BntTotalNumTex, K::BntTotalNumRect, K::BntTotalNumPos),
            success,
        }
    }
}

impl HudState {
    /// Notice `n` for 5 s and its sound.
    // EVIDENCE: cshell+0x71c30
    fn spy_note(&mut self, n: SpyNote, now: u32) {
        self.spy.note = Some((n, now));
        let v = match n {
            SpyNote::Seize | SpyNote::Stolen => Some(Voice::BntIntelGet),
            SpyNote::SeizeAll | SpyNote::StolenAll => Some(Voice::BntIntelGetAll),
            SpyNote::Drop => Some(Voice::BntIntelDrop),
            SpyNote::Respawn => None,
        };
        if let Some(v) = v {
            self.banner.voices.push(v);
        }
    }

    /// 0xcb: the record's team, the own spy flag and count, the counters, the notices, the stage and its banner, the
    /// clock.
    // DC-DEFAULT: CLI-1081 (proposed) the client's own copy of the 2009 handler's effects
    pub fn on_spy(&mut self, e: &SpyEvent, now: u32) {
        let phase1 = self.spy.phase == PHASE_AGENT;
        match e {
            SpyEvent::Team { value, team, pid } => {
                let Ok(p) = u8::try_from(*pid) else {
                    return;
                };
                if let Some(r) = self.roster.get_mut(&p) {
                    r.team = *team;
                }
                let own = self.own_pid == Some(p);
                if *team == 1 {
                    if own {
                        self.spy.own_spy = false;
                        self.spy.seized = 0;
                    }
                } else if own {
                    self.spy.success = Some(now);
                    self.spy.seized = *value;
                    self.spy_note(
                        if phase1 {
                            SpyNote::SeizeAll
                        } else {
                            SpyNote::Seize
                        },
                        now,
                    );
                    self.spy.own_spy = true;
                } else {
                    self.spy_note(
                        if phase1 {
                            SpyNote::StolenAll
                        } else {
                            SpyNote::Stolen
                        },
                        now,
                    );
                }
            }
            SpyEvent::Counts {
                total,
                dropped,
                spies,
            } => {
                self.spy.total = *total;
                self.spy.dropped = *dropped;
                self.spy.spies = *spies;
            }
            SpyEvent::Loadout => {}
            SpyEvent::Dropped => {
                if !phase1 {
                    self.spy_note(SpyNote::Drop, now);
                }
            }
            SpyEvent::Respawned => {
                if !phase1 {
                    self.spy_note(SpyNote::Respawn, now);
                }
            }
            SpyEvent::Phase { phase, title, text } => {
                self.spy.phase = *phase;
                if *phase == PHASE_AGENT {
                    // CLI-1213 (playtest report): the two strings are the banner's images, shown by the 0x45 intro
                    // banner's code (before, they were drawn as two text lines)
                    // EVIDENCE: cshell+0x2bd80 (sub 5 phase 1: cshell+0xbbfc0(first, second, 4000, 1.0)),
                    // cshell+0xbbfc0 (each string loaded as a texture: the intro banner)
                    if !title.is_empty() && !text.is_empty() {
                        self.intro = Some(super::state::IntroShow {
                            head: title.clone(),
                            body: text.clone(),
                            at: now,
                        });
                    }
                    let n = if self.spy.own_spy {
                        SpyNote::SeizeAll
                    } else {
                        SpyNote::StolenAll
                    };
                    self.spy_note(n, now);
                }
            }
            SpyEvent::Timer { secs, .. } => self.on_coop_clock(true, *secs, now),
        }
    }
}

fn image(i: &Img, tex: Option<String>) -> DrawItem {
    DrawItem::Image {
        tex: tex.unwrap_or_else(|| i.tex.clone()),
        src: [i.size[0] as f32, i.size[1] as f32],
        dst: [
            i.pos[0] as f32,
            i.pos[1] as f32,
            i.size[0] as f32,
            i.size[1] as f32,
        ],
        tint: [255, 255, 255, 255],
        turn: None,
    }
}

fn number(out: &mut Vec<DrawItem>, i: &Option<Img>, n: u32) {
    if let Some(i) = i {
        out.push(image(i, Some(i.tex.replace("%d", &n.to_string()))));
    }
}

/// Draws the Spy Hunt element (class 9 only).
pub fn draw(out: &mut Vec<DrawItem>, l: &SpyLayout, s: &HudState) {
    if s.mode_class != Some(9) {
        return;
    }
    let now = s.now_ms;
    let h = &s.spy;
    if let Some(t) = &l.title {
        out.push(image(t, None));
    }
    if let (Some((n, t0)), Some((pos, size))) = (h.note, l.note_place) {
        if (now.saturating_sub(t0) as f32) < NOTE_SECS * 1000.0 {
            if let Some(Some(tex)) = l.notes.get(n as usize) {
                out.push(image(
                    &Img {
                        tex: tex.clone(),
                        pos,
                        size,
                    },
                    None,
                ));
            }
        }
    }
    let stage = usize::from(h.phase == PHASE_AGENT) * 2 + usize::from(h.own_spy);
    if let (Some(tex), Some((pos, size))) = (&l.stages[stage], l.stage_place) {
        out.push(image(
            &Img {
                tex: tex.clone(),
                pos,
                size,
            },
            None,
        ));
    }
    for i in [&l.dropped, &l.spy].into_iter().flatten() {
        out.push(image(i, None));
    }
    number(out, &l.dropped_num, h.dropped);
    number(out, &l.spy_num, h.spies);
    if let Some(a) = &l.success {
        super::items::announce(out, a, h.success, now);
    }
    // the own living player's seized / total block
    // DISASM: cshell+0x7082c..0x70879 (drawn while the own player state is 1)
    if s.life_state == 1 {
        if let Some(i) = &l.seized {
            out.push(image(i, None));
        }
        number(out, &l.seized_num, h.seized);
        if let Some(i) = &l.delimiter {
            out.push(image(i, None));
        }
        number(out, &l.total_num, h.total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::BoardRow;

    fn im(t: &str, x: i32) -> Option<Img> {
        Some(Img {
            tex: t.into(),
            pos: [x, 0],
            size: [10, 10],
        })
    }

    fn layout() -> SpyLayout {
        SpyLayout {
            title: im("bh.dtx", 1),
            notes: (0..6).map(|i| Some(format!("note{i}.dtx"))).collect(),
            note_place: Some(([0, 423], [1024, 128])),
            stages: [
                Some("s1h.dtx".into()),
                Some("s1s.dtx".into()),
                Some("s2h.dtx".into()),
                Some("s2s.dtx".into()),
            ],
            stage_place: Some(([256, 108], [512, 14])),
            dropped: im("dropped.dtx", 2),
            dropped_num: im("ns_%d.dtx", 3),
            spy: im("spy.dtx", 4),
            spy_num: im("ns_%d.dtx", 5),
            seized: im("seized.dtx", 6),
            seized_num: im("nl_%d.dtx", 7),
            delimiter: im("slash.dtx", 8),
            total_num: im("ns_%d.dtx", 9),
            success: Some(Announce {
                tex: "fx.dtx".into(),
                size: [512, 256],
                centre: [512, 256],
                total: 1.4,
                create: 0.1,
                end: 1.2,
                destroy: 0.2,
                first_scale: 5.0,
            }),
        }
    }

    fn texs(out: &[DrawItem]) -> Vec<String> {
        out.iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, .. } => Some(tex.clone()),
                DrawItem::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn state() -> HudState {
        let mut s = HudState {
            mode_class: Some(9),
            own_pid: Some(0),
            life_state: 1,
            ..HudState::default()
        };
        for pid in [0u8, 1] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    team: 1,
                    ..BoardRow::default()
                },
            );
        }
        s
    }

    /// CLI-1080..1082: the counters, the own seize (count, notice 0, seize animation, sound), another player's seize
    /// (notice 2), the drop notice, stage 2 (banner, the "all" notice, the stage-2 spy message), back to stage 1.
    #[test]
    fn spy_hunt_counters_notices_stages_and_own_block() {
        let l = layout();
        let mut s = state();
        s.on_spy(
            &SpyEvent::Counts {
                total: 5,
                dropped: 0,
                spies: 0,
            },
            0,
        );
        s.now_ms = 10;
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        let t = texs(&out);
        assert!(
            t.contains(&"bh.dtx".into()) && t.contains(&"s1h.dtx".into()),
            "{t:?}"
        );
        assert!(
            t.contains(&"ns_5.dtx".into()) && t.contains(&"nl_0.dtx".into()),
            "{t:?}"
        );
        // the own player seizes a case
        s.on_spy(
            &SpyEvent::Team {
                value: 1,
                team: 2,
                pid: 0,
            },
            100,
        );
        assert!(s.spy.own_spy);
        assert_eq!(s.roster[&0].team, 2);
        assert_eq!(s.spy.note, Some((SpyNote::Seize, 100)));
        assert_eq!(s.banner.voices.last(), Some(&Voice::BntIntelGet));
        s.now_ms = 200;
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        let t = texs(&out);
        assert!(
            t.contains(&"note0.dtx".into()) && t.contains(&"s1s.dtx".into()),
            "{t:?}"
        );
        assert!(
            t.contains(&"nl_1.dtx".into()) && t.contains(&"fx.dtx".into()),
            "{t:?}"
        );
        // another player seizes one: notice 2; a drop: notice 4 and its sound
        s.on_spy(
            &SpyEvent::Team {
                value: 1,
                team: 2,
                pid: 1,
            },
            300,
        );
        assert_eq!(s.spy.note, Some((SpyNote::Stolen, 300)));
        s.on_spy(&SpyEvent::Dropped, 400);
        assert_eq!(s.spy.note, Some((SpyNote::Drop, 400)));
        assert_eq!(s.banner.voices.last(), Some(&Voice::BntIntelDrop));
        // the notice goes after 5 s
        s.now_ms = 400 + 5_001;
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        assert!(!texs(&out).contains(&"note4.dtx".into()));
        // stage 2: the banner, the "all" notice for the own spy, the stage-2 spy message; drop notices stop
        s.on_spy(
            &SpyEvent::Phase {
                phase: 1,
                title: "T".into(),
                text: "X".into(),
            },
            6_000,
        );
        assert_eq!(s.spy.note, Some((SpyNote::SeizeAll, 6_000)));
        s.on_spy(&SpyEvent::Dropped, 6_100);
        assert_eq!(s.spy.note, Some((SpyNote::SeizeAll, 6_000)));
        s.on_spy(
            &SpyEvent::Timer {
                secs: 180,
                colour: 0,
            },
            6_100,
        );
        assert_eq!(s.clock.secs, 180);
        s.now_ms = 6_200;
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        let t = texs(&out);
        assert!(t.contains(&"s2s.dtx".into()), "{t:?}");
        // CLI-1213: the banner images go to the intro banner
        assert_eq!(
            s.intro
                .as_ref()
                .map(|i| (i.head.as_str(), i.body.as_str(), i.at)),
            Some(("T", "X", 6_000))
        );
        // back to a hunter in stage 1
        s.on_spy(
            &SpyEvent::Phase {
                phase: 0,
                title: String::new(),
                text: String::new(),
            },
            7_000,
        );
        s.on_spy(
            &SpyEvent::Team {
                value: 0,
                team: 1,
                pid: 0,
            },
            7_000,
        );
        assert!(!s.spy.own_spy);
        assert_eq!(s.spy.seized, 0);
        s.now_ms = 7_100;
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        assert!(texs(&out).contains(&"s1h.dtx".into()));
        // another class draws nothing
        s.mode_class = Some(1);
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        assert!(out.is_empty());
    }
}

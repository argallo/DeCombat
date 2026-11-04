//! Quarantine on the client (mode class 11, the infection mode): 0xd3 `GS_INFECTION_MODE` moves players between the
//! humans (team 1) and the infected (team 2), starts the outbreak countdown with the round counters, and orders the
//! state notices; the HUD draws the mode's top block (title, the two team marks, the current round and the round
//! limit), the countdown digits, the notice and the own infected screen, and plays the intro and the countdown
//! sounds.
// SPEC: GAME_MODES_2009.md §5.7 (0xd3 subs 0 / 1 / 4, C-GM-2, C-GM-3); GAME_MESSAGES_2009_S2C.md §3.5 row 0xd3
// EVIDENCE: cshell+0x2a1e0 (0xd3: sub 0 sets the record's team, the own player infected shows notice 3 for 5 s,
// the own infector counts one; sub 1 starts the countdown and sets the two counters; sub 4 shows notice 2 for a
// human, 1 for an infected, 5 s), cshell+0xb29c0 (the element's items and places), cshell+0xb10f0 (notice n of the
// five: selecting, host, human, infected, welcome), cshell+0xb1190 (countdown; the intro sound for 19 < secs <= 20),
// cshell+0xb27b0 (minutes / seconds digits; a countdown sound per whole second below 11), cshell+0x857e0 /
// cshell+0x83260 (the infected screen picture)

use super::banners::Voice;
use super::items::DrawItem;
use super::layout::{Announce, Img};
use super::state::HudState;
use dc_ui_data::{HudKey as K, HudSection, HudView};
use std::collections::BTreeSet;

/// One 0xd3 message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QrtEvent {
    /// Sub 0: `pid` joins `team` (2 = infected) by `infector` (team 2 only; `u32::MAX` = nobody).
    Team { team: u8, infector: u32, pid: u32 },
    /// Sub 1: the countdown of `secs` and the two counters `a` / `b` (round, round limit).
    Countdown { a: u8, b: u8, secs: f32 },
    /// Sub 4: the notice by the own state.
    Note,
    /// Sub 3: the own player's override body row (-1: none), which the own speed, sprint and sprint gauge read
    /// first (the mover's loadout, [`crate::player::LocalPlayer`]).
    // EVIDENCE: cshell+0x2a1e0 (sub 3: an 8-bit signed value into the own player's info +0x2ac), cshell+0x5c270 /
    // cshell+0x1e2bc0 (the speed penalty reads the override row of that slot first)
    Override(i8),
}

/// The notices in the element's order.
// EVIDENCE: cshell+0xb29c0 (textures at +0x54..+0x64 in this order), cshell+0xb10f0 (index = the notice number)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrtNote {
    Selecting,
    Host,
    Human,
    Infected,
    Welcome,
}

/// How long the state notices stay.
// EVIDENCE: cshell+0x2a1e0 (cshell+0xb10f0(n, 5.0))
pub const NOTE_SECS: f32 = 5.0;
/// The countdown digits: line, texels of one cell (each cell is drawn at the layout's `HostTimerScale`), the cell
/// step in units of that scale and the cells' places (minutes tens, minutes ones, separator, seconds tens, seconds
/// ones) in steps from the screen centre.
// EVIDENCE: cshell+0xb29c0 (digit and separator sprites set to 0x10 x 0x1c texels, scale `HostTimerScale`, y 0x5a)
// DISASM: cshell+0xb3a3a..0xb3b50 (x = 512 - 2.5 x 13s, 512 - 1.5 x 13s, separator 512 - 0.5 x 13s, 512 + 0.5 x 13s,
// 512 + 1.5 x 13s, truncated; then each sprite scaled by s)
pub const TIMER_Y: i32 = 0x5a;
pub const TIMER_TEXELS: [i32; 2] = [0x10, 0x1c];
pub const TIMER_STEP: f32 = 13.0;
pub const TIMER_STEPS: [f32; 5] = [-2.5, -1.5, -0.5, 0.5, 1.5];
/// The countdown cell's left edge and size at scale `s` (CLI-1210: playtest report "the countdown is cut off": the cells
/// took 13 x 22 texels of the 16 x 28 digit).
pub fn timer_cell(i: usize, s: f32) -> ([f32; 2], [f32; 2]) {
    let x = (512.0 + TIMER_STEPS[i] * TIMER_STEP * s).trunc();
    (
        [x, TIMER_Y as f32],
        [TIMER_TEXELS[0] as f32 * s, TIMER_TEXELS[1] as f32 * s],
    )
}
/// The countdown cells' colour (ARGB 0xffff0013, a red over the pale timer digits). The round clock keeps the
/// layout set's colour: the 2009 client sets it only from the layout (cshell+0x51a50, white in the ROM) or the Spy
/// Hunt 0xcb sub 6 wire colour; the reference stack's red round clock in class 11 has no 2009 counterpart.
// DISASM: cshell+0xb39f9..0xb3a35 (the five host timer cells +0x14..+0x24, each cshell+0x3436a0(0xffff0013), the
// sprite colour at +0x5b4)
pub const TIMER_TINT: [u8; 4] = [0xff, 0x00, 0x13, 0xff];

/// The Quarantine HUD state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QrtHud {
    /// The countdown: start and seconds.
    pub countdown: Option<(u32, f32)>,
    /// The round counters of the last sub 1.
    pub counters: Option<[u8; 2]>,
    pub infected: BTreeSet<u8>,
    pub note: Option<(QrtNote, u32, f32)>,
    /// Infections by the own player since his last team change.
    pub own_count: u32,
    /// Start of the infected message (an own infection) and of the kill message (the own player killed an infected).
    pub infect_msg: Option<u32>,
    pub kill_msg: Option<u32>,
    /// The whole second of the last countdown sound.
    pub last_sec: Option<u32>,
}

/// The textures and places of the infection element.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QrtLayout {
    pub title: Option<Img>,
    pub humans: Option<Img>,
    pub infected: Option<Img>,
    /// Current round tens / ones, limit tens / ones places (the digit size).
    pub digits: [Option<[i32; 2]>; 4],
    pub digit_size: [i32; 2],
    /// The texels of one score digit (the team counters' digit size).
    pub digit_cell: [i32; 2],
    pub digit_prefix: String,
    pub slash: Option<Img>,
    pub word: Option<Img>,
    pub timer: String,
    pub separator: String,
    /// `HostTimerScale` (1 when absent).
    pub timer_scale: f32,
    pub notes: Vec<Option<String>>,
    pub note_place: Option<([i32; 2], [i32; 2])>,
    pub screen: Option<Img>,
    /// The own infection count: the icon (place of the first, size and step), the count image (form with the
    /// count) and the times sign.
    pub own_icon: Option<Img>,
    pub own_num: Option<Img>,
    pub times: Option<Img>,
    /// The two animated messages (one timing set).
    pub infect_msg: Option<Announce>,
    pub kill_msg: Option<Announce>,
}

/// From this many own infections the count is drawn as one icon, the times sign and the count image.
// EVIDENCE: cshell+0xb2260 (count < 4: `count` icons at first place + i x icon width; else one icon, the count image
// with the form of the count and the times sign shown)
pub const OWN_ICONS_MAX: u32 = 3;

fn img(v: &HudView<'_>, tex: K, pos: K, size: K) -> Option<Img> {
    Some(Img {
        tex: v.text(tex)?.to_string(),
        pos: v.pair(pos)?,
        size: v.pair(size)?,
    })
}

impl QrtLayout {
    pub fn load(v: &HudView<'_>) -> Self {
        let q = HudSection::Quarantine;
        QrtLayout {
            title: img(v, K::QrtTitleTex, K::QrtTitlePos, K::QrtTitleSize),
            humans: img(v, K::QrtHumansTex, K::QrtHumansPos, K::QrtHumansSize),
            infected: img(v, K::QrtZombiesTex, K::QrtZombiesPos, K::QrtZombiesSize),
            digits: [
                v.pair(K::QrtCurTensPos),
                v.pair(K::QrtCurOnesPos),
                v.pair_in(q, K::MaxRoundTensPos),
                v.pair_in(q, K::MaxRoundOnesPos),
            ],
            digit_size: v.pair(K::GoalDigitSize).unwrap_or([18, 20]),
            digit_cell: v.pair(K::Team1DigitSize).unwrap_or([36, 40]),
            digit_prefix: v.text(K::ScoreDigits).unwrap_or_default().to_string(),
            slash: img(v, K::QrtSlashTex, K::QrtSlashPos, K::QrtSlashSize),
            word: v
                .text_in(q, K::RoundWordTex)
                .zip(v.pair_in(q, K::RoundWordPos))
                .zip(v.pair_in(q, K::RoundWordSize))
                .map(|((t, p), s)| Img {
                    tex: t.to_string(),
                    pos: p,
                    size: s,
                }),
            timer: v.text(K::QrtTimerDigits).unwrap_or_default().to_string(),
            separator: v.text(K::QrtTimerSeparator).unwrap_or_default().to_string(),
            timer_scale: v
                .real(K::QrtTimerScale)
                .map(|r| r as f32)
                .filter(|r| *r > 0.0)
                .unwrap_or(1.0),
            notes: [
                K::QrtNoteSelecting,
                K::QrtNoteHost,
                K::QrtNoteHuman,
                K::QrtNoteZombie,
                K::QrtNoteWelcome,
            ]
            .into_iter()
            .map(|k| v.text(k).map(str::to_string))
            .collect(),
            note_place: v.pair(K::QrtNotePos).zip(v.pair(K::QrtNoteSize)),
            screen: img(
                v,
                K::ZombieScreenTex,
                K::ZombieScreenPos,
                K::ZombieScreenSize,
            ),
            own_icon: img(v, K::QrtOwnIconTex, K::QrtOwnIconPos, K::QrtOwnIconSize),
            own_num: img(v, K::QrtOwnNumTex, K::QrtOwnNumPos, K::QrtOwnNumSize),
            times: img(v, K::QrtTimesTex, K::QrtTimesPos, K::QrtTimesSize),
            infect_msg: msg(v, K::QrtInfectMsgTex),
            kill_msg: msg(v, K::QrtKillMsgTex),
        }
    }
}

/// One of the two animated messages: its image with the shared size, place and timing. The image's top edge is at
/// the place's y, centred on its x.
// EVIDENCE: cshell+0xb29c0 (the message object placed at (x - width / 2, y) with the total, start, creating, blowing,
// end, destroying times and the start, blow and last sizes)
// DC-DEFAULT: CLI-1071 (proposed) drawn with the announcement rule of CLI-76 (creating, end and destroying times, the
// start size; the blowing phase and the blow / last sizes are not traced)
fn msg(v: &HudView<'_>, tex: K) -> Option<Announce> {
    let size = v.pair(K::QrtMsgSize)?;
    let pos = v.pair(K::QrtMsgPos)?;
    let secs = |k: K| v.real(k).unwrap_or(0.0) as f32;
    Some(Announce {
        tex: v.text(tex)?.to_string(),
        size,
        centre: [pos[0], pos[1] + size[1] / 2],
        total: secs(K::QrtMsgTotal),
        create: secs(K::QrtMsgCreate),
        end: secs(K::QrtMsgEnd),
        destroy: secs(K::QrtMsgDestroy),
        first_scale: secs(K::QrtMsgStartSize),
    })
}

impl HudState {
    fn own_infected(&self) -> bool {
        self.own_pid.is_some_and(|p| self.qrt.infected.contains(&p))
    }

    /// 0xd3: the record's team and the infected set; the own notices, the own infection count, the countdown and
    /// its intro sound.
    // DC-DEFAULT: CLI-1060 (proposed) the client's own copy of the 2009 handler's effects
    pub fn on_qrt(&mut self, e: &QrtEvent, now: u32) {
        let own = self.own_pid.map(u32::from);
        match *e {
            QrtEvent::Team {
                team,
                infector,
                pid,
            } => {
                let Ok(p) = u8::try_from(pid) else {
                    return;
                };
                if let Some(r) = self.roster.get_mut(&p) {
                    r.team = team;
                }
                // EVIDENCE: cshell+0x2a1e0 (the own player's team change resets the own count, cshell+0xb2260(0))
                if own == Some(pid) {
                    self.qrt.own_count = 0;
                }
                if team == 2 {
                    self.qrt.infected.insert(p);
                    if own == Some(pid) {
                        self.qrt.note = Some((QrtNote::Infected, now, NOTE_SECS));
                    }
                    // EVIDENCE: cshell+0x2a1e0 (the own infector: count + 1, then the infected message, cshell+0xb10b0)
                    if infector != u32::MAX && own == Some(infector) {
                        self.qrt.own_count += 1;
                        self.qrt.infect_msg = Some(now);
                    }
                } else {
                    self.qrt.infected.remove(&p);
                }
            }
            QrtEvent::Countdown { a, b, secs } => {
                if secs > 19.0 && secs <= 20.0 {
                    self.banner.voices.push(Voice::QrtIntro);
                }
                self.qrt.countdown = Some((now, secs));
                self.qrt.counters = Some([a, b]);
                self.qrt.last_sec = None;
                self.qrt.note = Some((QrtNote::Selecting, now, secs));
            }
            QrtEvent::Override(_) => {}
            QrtEvent::Note => {
                let n = if self.own_infected() {
                    QrtNote::Host
                } else {
                    QrtNote::Human
                };
                self.qrt.note = Some((n, now, NOTE_SECS));
            }
        }
    }

    /// 0x6d in class 11: the own player killed an infected player (the kill message).
    // EVIDENCE: cshell+0x2c430 (an own kill of another team's character whose object has the infected flag:
    // cshell+0xb10d0)
    pub fn on_qrt_kill(&mut self, killer: u8, victim: u8, now: u32) {
        if self.mode_class == Some(11)
            && self.own_pid == Some(killer)
            && killer != victim
            && self.qrt.infected.contains(&victim)
        {
            self.qrt.kill_msg = Some(now);
        }
    }

    /// The countdown sound once per whole second below 11.
    pub fn tick_qrt(&mut self, now: u32) {
        let Some((at, secs)) = self.qrt.countdown else {
            return;
        };
        let left = secs - now.saturating_sub(at) as f32 / 1000.0;
        if left < 0.0 {
            self.qrt.countdown = None;
            return;
        }
        let whole = left as u32;
        if self.qrt.last_sec != Some(whole) {
            self.qrt.last_sec = Some(whole);
            if whole < 11 {
                self.banner.voices.push(Voice::QrtCount(whole as u8));
            }
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

fn at(tex: String, pos: [i32; 2], size: [i32; 2]) -> DrawItem {
    image(&Img { tex, pos, size }, None)
}

/// Draws the infection element (class 11 only).
pub fn draw(out: &mut Vec<DrawItem>, l: &QrtLayout, s: &HudState) {
    if s.mode_class != Some(11) {
        return;
    }
    let now = s.now_ms;
    if s.own_infected() {
        if let Some(sc) = &l.screen {
            out.push(image(sc, None));
        }
    }
    for i in [&l.title, &l.humans, &l.infected, &l.slash, &l.word]
        .into_iter()
        .flatten()
    {
        out.push(image(i, None));
    }
    // DC-DEFAULT: CLI-1111 (proposed, playtest report) the round / limit digits draw the score digit's cell (the team
    // counters' digit size, 36 x 40 texels) shrunk into the goal digit size: the top-left 18 x 20 texels of the
    // digit showed only a cut piece of the glyph
    if let Some([a, b]) = s.qrt.counters {
        let d = [a / 10 % 10, a % 10, b / 10 % 10, b % 10];
        for (p, d) in l.digits.iter().zip(d) {
            if let Some(p) = p {
                out.push(DrawItem::Image {
                    tex: format!("{}{d}.dtx", l.digit_prefix),
                    src: [l.digit_cell[0] as f32, l.digit_cell[1] as f32],
                    dst: [
                        p[0] as f32,
                        p[1] as f32,
                        l.digit_size[0] as f32,
                        l.digit_size[1] as f32,
                    ],
                    tint: [255, 255, 255, 255],
                    turn: None,
                });
            }
        }
    }
    if let Some((t0, secs)) = s.qrt.countdown {
        let left = (secs - now.saturating_sub(t0) as f32 / 1000.0).max(0.0) as u32;
        let (m, sec) = (left / 60, left % 60);
        let cells = [
            Some(m / 10 % 10),
            Some(m % 10),
            None,
            Some(sec / 10),
            Some(sec % 10),
        ];
        for (i, c) in cells.into_iter().enumerate() {
            let tex = match c {
                Some(d) => l.timer.replace("%d", &d.to_string()),
                None => l.separator.clone(),
            };
            let (p, size) = timer_cell(i, l.timer_scale);
            out.push(DrawItem::Image {
                tex,
                src: [TIMER_TEXELS[0] as f32, TIMER_TEXELS[1] as f32],
                dst: [p[0], p[1], size[0], size[1]],
                tint: TIMER_TINT,
                turn: None,
            });
        }
    }
    // the own infection count
    // EVIDENCE: cshell+0xb2260
    if let Some(icon) = &l.own_icon {
        let n = s.qrt.own_count;
        let icons = if n > OWN_ICONS_MAX { 1 } else { n };
        for i in 0..icons as i32 {
            out.push(at(
                icon.tex.clone(),
                [icon.pos[0] + i * icon.size[0], icon.pos[1]],
                icon.size,
            ));
        }
        if n > OWN_ICONS_MAX {
            if let Some(t) = &l.times {
                out.push(image(t, None));
            }
            if let Some(c) = &l.own_num {
                out.push(image(c, Some(c.tex.replace("%d", &n.to_string()))));
            }
        }
    }
    if let Some(a) = &l.infect_msg {
        super::items::announce(out, a, s.qrt.infect_msg, now);
    }
    if let Some(a) = &l.kill_msg {
        super::items::announce(out, a, s.qrt.kill_msg, now);
    }
    if let (Some((n, t0, secs)), Some((pos, size))) = (s.qrt.note, l.note_place) {
        if (now.saturating_sub(t0) as f32) < secs * 1000.0 {
            if let Some(Some(tex)) = l.notes.get(n as usize) {
                out.push(at(tex.clone(), pos, size));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::BoardRow;

    fn layout() -> QrtLayout {
        let im = |t: &str, x: i32| {
            Some(Img {
                tex: t.into(),
                pos: [x, 0],
                size: [10, 10],
            })
        };
        QrtLayout {
            title: im("qrt.dtx", 1),
            humans: im("humans.dtx", 2),
            infected: im("zombies.dtx", 3),
            digits: [Some([4, 0]), Some([5, 0]), Some([6, 0]), Some([7, 0])],
            digit_size: [18, 20],
            digit_cell: [36, 40],
            digit_prefix: "sd".into(),
            slash: im("slash.dtx", 8),
            word: im("round.dtx", 9),
            timer: "t_%d.dtx".into(),
            separator: "t_sep.dtx".into(),
            timer_scale: 0.8,
            notes: (0..5).map(|i| Some(format!("note{i}.dtx"))).collect(),
            note_place: Some(([256, 114], [512, 64])),
            screen: im("screen.dtx", 0),
            own_icon: Some(Img {
                tex: "my.dtx".into(),
                pos: [600, 40],
                size: [20, 20],
            }),
            own_num: Some(Img {
                tex: "num_%d.dtx".into(),
                pos: [650, 40],
                size: [30, 20],
            }),
            times: im("x.dtx", 630),
            infect_msg: Some(Announce {
                tex: "infect_msg.dtx".into(),
                size: [200, 50],
                centre: [512, 225],
                total: 3.0,
                create: 0.5,
                end: 2.5,
                destroy: 0.5,
                first_scale: 2.0,
            }),
            kill_msg: Some(Announce {
                tex: "kill_msg.dtx".into(),
                size: [200, 50],
                centre: [512, 225],
                total: 3.0,
                create: 0.5,
                end: 2.5,
                destroy: 0.5,
                first_scale: 2.0,
            }),
        }
    }

    fn texs(out: &[DrawItem]) -> Vec<(String, f32)> {
        out.iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, dst, .. } => Some((tex.clone(), dst[0])),
                _ => None,
            })
            .collect()
    }

    fn state() -> HudState {
        let mut s = HudState {
            mode_class: Some(11),
            own_pid: Some(0),
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

    /// CLI-1060 / CLI-1061: sub 1 [2][5][20.0] plays the intro, draws round 02 / 05, the countdown 00:20 and the
    /// selecting notice; the countdown sounds below 11 s once per second; sub 0 team 2 for the own player moves
    /// his record, shows the infected notice and the infected screen; a TDM room draws nothing.
    #[test]
    fn quarantine_countdown_infection_and_top_block() {
        let mut s = state();
        s.on_qrt(
            &QrtEvent::Countdown {
                a: 2,
                b: 5,
                secs: 20.0,
            },
            1000,
        );
        assert_eq!(s.banner.voices, vec![Voice::QrtIntro]);
        s.now_ms = 1000;
        let mut out = Vec::new();
        draw(&mut out, &layout(), &s);
        let t = texs(&out);
        for want in [
            ("qrt.dtx", 1.0),
            ("humans.dtx", 2.0),
            ("zombies.dtx", 3.0),
            ("sd0.dtx", 4.0),
            ("sd2.dtx", 5.0),
            ("sd0.dtx", 6.0),
            ("sd5.dtx", 7.0),
            ("t_0.dtx", 486.0),
            ("t_0.dtx", 496.0),
            ("t_sep.dtx", 506.0),
            ("t_2.dtx", 517.0),
            ("t_0.dtx", 527.0),
            ("note0.dtx", 256.0),
        ] {
            assert!(t.contains(&(want.0.to_string(), want.1)), "{want:?} {t:?}");
        }
        assert!(!t.iter().any(|x| x.0 == "screen.dtx"));
        // CLI-1210: each countdown cell shows the whole 16 x 28 texel digit at the timer scale (it took 13 x 22
        // texels: the cut digits)
        for d in &out {
            if let DrawItem::Image { tex, src, dst, .. } = d {
                if tex.starts_with("t_") {
                    assert_eq!(*src, [16.0, 28.0], "{tex}");
                    assert_eq!(dst[2..], [16.0 * 0.8, 28.0 * 0.8], "{tex}");
                    assert_eq!(dst[1], 90.0);
                }
            }
        }
        // the countdown cells are red (cshell+0xb39f9), the top block keeps its own colour
        for d in &out {
            if let DrawItem::Image { tex, tint, .. } = d {
                let want = if tex.starts_with("t_") {
                    [0xff, 0x00, 0x13, 0xff]
                } else {
                    [255, 255, 255, 255]
                };
                assert_eq!(*tint, want, "{tex}");
            }
        }
        s.tick_qrt(1000 + 9_500);
        assert_eq!(s.banner.voices.last(), Some(&Voice::QrtCount(10)));
        let n = s.banner.voices.len();
        s.tick_qrt(1000 + 9_700);
        assert_eq!(s.banner.voices.len(), n);
        s.on_qrt(
            &QrtEvent::Team {
                team: 2,
                infector: u32::MAX,
                pid: 0,
            },
            21_000,
        );
        assert_eq!(s.roster[&0].team, 2);
        assert_eq!(s.qrt.note.map(|n| n.0), Some(QrtNote::Infected));
        s.now_ms = 22_000;
        let mut out = Vec::new();
        draw(&mut out, &layout(), &s);
        let t = texs(&out);
        assert!(t.contains(&("screen.dtx".into(), 0.0)));
        assert!(t.contains(&("note3.dtx".into(), 256.0)));
        // an infection by the own player counts
        s.on_qrt(
            &QrtEvent::Team {
                team: 2,
                infector: 0,
                pid: 1,
            },
            23_000,
        );
        assert_eq!(s.qrt.own_count, 1);
        // back to the humans
        s.on_qrt(
            &QrtEvent::Team {
                team: 1,
                infector: u32::MAX,
                pid: 0,
            },
            24_000,
        );
        assert_eq!(s.roster[&0].team, 1);
        assert!(!s.qrt.infected.contains(&0));
        s.mode_class = Some(1);
        let mut out = Vec::new();
        draw(&mut out, &layout(), &s);
        assert!(out.is_empty());
    }

    /// CLI-1070 / CLI-1071: the own infections draw one icon each up to three, from four one icon, the times sign
    /// and the count image; an own infection shows the infected message, an own kill of an infected the kill
    /// message (each for its total time); the own team change resets the count.
    #[test]
    fn own_infection_count_and_the_two_messages() {
        let mut s = state();
        s.on_qrt(
            &QrtEvent::Team {
                team: 2,
                infector: u32::MAX,
                pid: 0,
            },
            1000,
        );
        let infect = |s: &mut HudState, pid: u32, now: u32| {
            s.on_qrt(
                &QrtEvent::Team {
                    team: 2,
                    infector: 0,
                    pid,
                },
                now,
            )
        };
        let mine = |s: &HudState| {
            let mut out = Vec::new();
            draw(&mut out, &layout(), s);
            texs(&out)
                .into_iter()
                .filter(|t| {
                    ["my.dtx", "x.dtx", "infect_msg.dtx", "kill_msg.dtx"].contains(&t.0.as_str())
                        || t.0.starts_with("num_")
                })
                .collect::<Vec<_>>()
        };
        s.now_ms = 1500;
        assert!(mine(&s).is_empty(), "nothing before an own infection");
        infect(&mut s, 1, 2000);
        infect(&mut s, 1, 2100);
        s.now_ms = 2100 + 1000;
        let t = mine(&s);
        assert!(t.contains(&("my.dtx".into(), 600.0)) && t.contains(&("my.dtx".into(), 620.0)));
        assert!(t.iter().any(|x| x.0 == "infect_msg.dtx"), "{t:?}");
        for now in [2200, 2300] {
            infect(&mut s, 1, now);
        }
        s.now_ms = 2300 + 4000;
        let t = mine(&s);
        assert_eq!(
            t,
            vec![
                ("my.dtx".into(), 600.0),
                ("x.dtx".into(), 630.0),
                ("num_4.dtx".into(), 650.0)
            ],
            "four: one icon, times sign, count; the message is over"
        );
        // the own kill of an infected player shows the kill message; a human victim does not
        s.qrt.infected.insert(1);
        s.on_qrt_kill(0, 1, 7000);
        s.now_ms = 7500;
        assert!(mine(&s).iter().any(|x| x.0 == "kill_msg.dtx"));
        s.qrt.kill_msg = None;
        s.qrt.infected.remove(&1);
        s.on_qrt_kill(0, 1, 8000);
        assert_eq!(s.qrt.kill_msg, None);
        // the own player back with the humans: the count starts again
        s.on_qrt(
            &QrtEvent::Team {
                team: 1,
                infector: u32::MAX,
                pid: 0,
            },
            9000,
        );
        assert_eq!(s.qrt.own_count, 0);
    }
}

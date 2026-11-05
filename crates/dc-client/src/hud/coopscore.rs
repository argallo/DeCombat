//! Fire Team (mode class 5) score, drop-item notes and kill run on the client.
//!
//! - The score: the co-op section's "score" picture and six digit pictures at the top of the screen, the group
//!   centred on the screen's middle. It shows the own player's points total of the last 0xd7 sub 1 / sub 3 naming the
//!   own id, counted up from the previous total over [`SCORE_ROLL_MS`]; leading zeros are drawn faint.
//! - The drop-item note: 0xc9 sub 6 `[pickup:u8]` shows the health, ammunition or ammunition-box picture of the co-op
//!   section for its show time, then fades it out; up to [`NOTE_SLOTS`] notes stack from the section's place by its
//!   per-note offset, a fifth drops the oldest. Each note plays its pickup sound.
//! - The kill run: the 2009 client shows "N Kills" and "N COMBO" banners when the server sends 0xc8 (the banners of
//!   [`super::banners`]). Our server sends one per AI kill (DSV-602); against a server that sends none the client
//!   counts the own kills of 0xd7 sub 1 and
//!   times the chain itself, and starts the same banners as a 0xc8 with those counts would (CLI-1882). A received
//!   0xc8 turns this off for the rest of the match: then the server orders the banners.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2 row 0xc9 (sub 6), §3.5 row 0xd7 (subs 1 / 3: the own pid's scoreboard value),
// §3.3 row 0xc8; FIRETEAM_2009.md §2.2 rows 1 and 3, §2.4 row 0xc9 sub 6
// EVIDENCE: cshell+0x7fca8..0x7fe07 (the note's keys: size, place, per-note offset, show and fade times x 1000, the
// three pictures), cshell+0x80090 (a note: 1 the health picture, 2 the ammunition, 3 the ammunition box, other values
// nothing; the first free of four slots, all four taken: the oldest dropped and the new one last), cshell+0x81e69..
// 0x81fb6 (per frame: alpha 255 while the age is at most the show time, then 255 - 255 x (age - show) / fade, gone
// after show + fade; empty slots closed up, slot k placed at place + k x offset)
// EVIDENCE: cshell+0x8ebe0 (the score keys: cshell+0x9005e..0x90245 the label and six digits, digit k at the first
// place + k x (digit width + spacing); cshell+0x90247..0x902ff the label and digits shifted so that the group from the
// label's left to the last digit's right is centred on x 512), cshell+0x9272c..0x9277c (class 5 shows the label and
// the six digits), cshell+0x91c90 (a new value: the old one kept, the roll started), cshell+0x924f0 (the roll: old +
// (new - old) x t / 150 until t reaches 150 ms), cshell+0x91cd0 (six decimal digits, the form + digit + `.dtx`; a zero
// before the first other digit at alpha 0x4c)

use super::banners::Voice;
use super::items::DrawItem;
use super::layout::Img;
use super::state::HudState;
use dc_ui_data::{HudKey as K, HudView};

/// The mode class of Fire Team (GAME_MODES_2009.md §6).
const COOP_CLASS: u8 = 5;

/// 0xc9 sub 6 values.
pub const PICKUP_HEALTH: u8 = 1;
pub const PICKUP_AMMO: u8 = 2;
pub const PICKUP_AMMO_BOX: u8 = 3;

/// How many drop-item notes stand at once.
// EVIDENCE: cshell+0x800b3 (four slot pointers scanned), cshell+0x81e6f (four slots per frame)
pub const NOTE_SLOTS: usize = 4;

/// The score's digits, the roll time and the alpha of a leading zero.
// EVIDENCE: cshell+0x901cd / cshell+0x91e7e (six digits), cshell+0x9250a (150 ms), cshell+0x91e72 (alpha 0x4c)
pub const SCORE_DIGITS: usize = 6;
pub const SCORE_ROLL_MS: u32 = 150;
pub const LEAD_ZERO_ALPHA: u8 = 0x4c;

/// The longest gap between two own kills of one chain.
// DC-DEFAULT: CLI-1882 (proposed) 3 s: the owner's notes of a 2009 recording show the chain growing through one belt
// of machine-gun fire and ending over a reload; 3 s spans the gap between kills in a burst and is shorter than a
// machine-gun reload. No 2009 number: the 2009 server sent the counts (0xc8)
pub const COMBO_WINDOW_MS: u32 = 3_000;

/// One drop-item note: the item byte and when it came (ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickupNote {
    pub item: u8,
    pub at: u32,
}

/// The co-op score, notes and kill run of the own player.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CoopScoreHud {
    /// The notes standing, oldest first (at most [`NOTE_SLOTS`]).
    pub notes: Vec<PickupNote>,
    /// The last total of the own player (the roll's end).
    pub score: u32,
    /// The total before it (the roll's start) and when the roll began.
    pub score_from: u32,
    pub score_at: Option<u32>,
    /// The own kills counted from 0xd7 sub 1, the current chain and the time of its last kill.
    pub kills: u32,
    pub combo: u32,
    pub last_kill: Option<u32>,
    /// A 0xc8 came in this match: the server orders the kill banners, the client does not.
    pub server_runs: bool,
}

impl CoopScoreHud {
    /// The score shown at `now`: the roll from the previous total to the new one over [`SCORE_ROLL_MS`], truncated.
    pub fn score_at(&self, now: u32) -> u32 {
        let Some(at) = self.score_at else {
            return self.score;
        };
        let t = now.saturating_sub(at);
        if t >= SCORE_ROLL_MS {
            return self.score;
        }
        let d = f64::from(self.score) - f64::from(self.score_from);
        let v = f64::from(self.score_from) + d * f64::from(t) / f64::from(SCORE_ROLL_MS);
        v.trunc().max(0.0) as u32
    }

    /// The chain at `now`: 0 once [`COMBO_WINDOW_MS`] passed since its last kill.
    pub fn combo_at(&self, now: u32) -> u32 {
        match self.last_kill {
            Some(t) if now.saturating_sub(t) <= COMBO_WINDOW_MS => self.combo,
            _ => 0,
        }
    }

    /// Whether a part changes on screen at `now` (a roll or a note).
    pub fn moving(&self, now: u32, l: &CoopScoreLayout) -> bool {
        self.score_at
            .is_some_and(|t| now.saturating_sub(t) <= SCORE_ROLL_MS)
            || self.notes.iter().any(|n| note_alpha(l, n, now).is_some())
    }
}

/// The drop-item note's look and times.
#[derive(Debug, Clone, PartialEq)]
pub struct PickupLayout {
    /// The pictures of items 1, 2, 3 (health, ammunition, ammunition box).
    pub pictures: [Option<String>; 3],
    pub size: [i32; 2],
    /// The first note's top-left and the step to the next note.
    pub pos: [i32; 2],
    pub offset: [i32; 2],
    pub show_ms: u32,
    pub fade_ms: u32,
}

/// The score's digits: the picture form (`form + digit + ".dtx"`), size, first place and spacing.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreDigits {
    pub form: String,
    pub size: [i32; 2],
    pub first: [i32; 2],
    pub space: i32,
}

/// The co-op section's score and note keys.
#[derive(Debug, Clone, PartialEq)]
pub struct CoopScoreLayout {
    pub label: Option<Img>,
    pub digits: Option<ScoreDigits>,
    pub pickup: PickupLayout,
}

impl Default for CoopScoreLayout {
    fn default() -> Self {
        CoopScoreLayout::fallback()
    }
}

impl CoopScoreLayout {
    /// The co-op section's keys; a missing time takes our own.
    pub fn load(v: &HudView<'_>) -> Self {
        let own = CoopScoreLayout::fallback();
        let text = |k: K| v.text(k).map(str::to_string);
        let ms = |k: K, d: u32| {
            v.real(k)
                .map_or(d, |s| (s * 1000.0).clamp(0.0, f64::from(u32::MAX)) as u32)
        };
        let label = (|| {
            Some(Img {
                tex: text(K::CoopScoreImgTex)?,
                pos: v.pair(K::CoopScoreImgPos).unwrap_or_default(),
                size: v.pair(K::CoopScoreImgRect)?,
            })
        })();
        let digits = (|| {
            Some(ScoreDigits {
                form: text(K::CoopScoreNumTex)?,
                size: v.pair(K::CoopScoreNumRect)?,
                first: v.pair(K::CoopScoreNumFirstPos).unwrap_or_default(),
                space: v.int(K::CoopScoreNumSpace).unwrap_or(0),
            })
        })();
        CoopScoreLayout {
            label,
            digits,
            pickup: PickupLayout {
                pictures: [
                    text(K::CoopDropHealthTex),
                    text(K::CoopDropAmmoTex),
                    text(K::CoopDropAmmoBoxTex),
                ],
                size: v.pair(K::CoopDropRect).unwrap_or(own.pickup.size),
                pos: v.pair(K::CoopDropPos).unwrap_or(own.pickup.pos),
                offset: v.pair(K::CoopDropOffset).unwrap_or(own.pickup.offset),
                show_ms: ms(K::CoopDropPlayTime, own.pickup.show_ms),
                fade_ms: ms(K::CoopDropFadeTime, own.pickup.fade_ms),
            },
        }
    }

    /// Without a ROM: no pictures (nothing drawn), our own places and times for a layout that lacks the keys.
    // DC-DEFAULT: CLI-1881 (proposed) the no-ROM and missing-key values
    pub fn fallback() -> Self {
        CoopScoreLayout {
            label: None,
            digits: None,
            pickup: PickupLayout {
                pictures: [None, None, None],
                size: [64, 64],
                pos: [880, 420],
                offset: [0, -72],
                show_ms: 2_000,
                fade_ms: 500,
            },
        }
    }
}

/// 0xc9 sub 6: a note for items 1..3 (with its pickup sound); other values nothing.
pub fn on_pickup(s: &mut HudState, item: u8, now: u32) {
    let voice = match item {
        PICKUP_HEALTH => Voice::CoopMedikit,
        PICKUP_AMMO => Voice::CoopMagazine,
        PICKUP_AMMO_BOX => Voice::CoopBulletbox,
        _ => return,
    };
    let c = &mut s.coop;
    c.notes.push(PickupNote { item, at: now });
    if c.notes.len() > NOTE_SLOTS {
        c.notes.remove(0);
    }
    s.banner.voices.push(voice);
}

/// 0xd7 sub 1 / 3 naming the own id: the new total (rolled up to) and, for sub 1 (a kill) in class 5, the kill run.
// DC-DEFAULT: CLI-1882 (proposed) the own kill count and chain from 0xd7 sub 1, shown as the banners a 0xc8
// `[kills][head 0][combo]` starts; a gap longer than [`COMBO_WINDOW_MS`] starts a new chain at 1; score and kills keep
pub fn on_own_points(s: &mut HudState, total: u32, kill: bool, now: u32) {
    let c = &mut s.coop;
    if c.score != total {
        c.score_from = c.score;
        c.score = total;
        c.score_at = Some(now);
    }
    if !kill || s.mode_class != Some(COOP_CLASS) {
        return;
    }
    c.kills = c.kills.saturating_add(1);
    c.combo = c.combo_at(now).saturating_add(1);
    c.last_kill = Some(now);
    if !c.server_runs {
        let (kills, combo) = (c.kills.min(u32::from(u16::MAX)) as u16, c.combo);
        super::banners::on_run(s, kills, false, combo, now);
    }
}

/// The alpha of note `n` at `now`: 255 through its show time, then fading; `None` once gone.
pub fn note_alpha(l: &CoopScoreLayout, n: &PickupNote, now: u32) -> Option<u8> {
    let p = &l.pickup;
    let age = now.saturating_sub(n.at);
    if age <= p.show_ms {
        return Some(255);
    }
    if age > p.show_ms.saturating_add(p.fade_ms) {
        return None;
    }
    let f = f64::from(age - p.show_ms) / f64::from(p.fade_ms.max(1));
    Some((255.0 - 255.0 * f).trunc().clamp(0.0, 255.0) as u8)
}

/// The score's label and digit places: `(label, digits)`, the group centred on x 512.
pub fn score_places(l: &CoopScoreLayout) -> Option<(Img, [[i32; 2]; SCORE_DIGITS], [i32; 2])> {
    let (label, d) = (l.label.as_ref()?, l.digits.as_ref()?);
    let step = d.size[0] + d.space;
    let mut at = [[0; 2]; SCORE_DIGITS];
    for (k, p) in at.iter_mut().enumerate() {
        *p = [d.first[0] + k as i32 * step, d.first[1]];
    }
    let right = at[SCORE_DIGITS - 1][0] + d.size[0];
    let left = (1024 - (right - label.pos[0])) / 2;
    let shift = left - label.pos[0];
    for p in &mut at {
        p[0] += shift;
    }
    let mut label = label.clone();
    label.pos[0] = left;
    Some((label, at, d.size))
}

/// The six digits of `v` (the low six decimal places), most significant first.
pub fn score_digits(v: u32) -> [u32; SCORE_DIGITS] {
    let mut out = [0; SCORE_DIGITS];
    let mut n = v;
    for d in out.iter_mut().rev() {
        *d = n % 10;
        n /= 10;
    }
    out
}

fn picture(tex: &str, pos: [i32; 2], size: [i32; 2], alpha: u8) -> DrawItem {
    let [w, h] = [size[0] as f32, size[1] as f32];
    DrawItem::Image {
        tex: tex.to_string(),
        src: [w, h],
        dst: [pos[0] as f32, pos[1] as f32, w, h],
        tint: [255, 255, 255, alpha],
        turn: None,
    }
}

/// The score and the notes at `s.now_ms` (class 5 only).
pub fn draw(out: &mut Vec<DrawItem>, l: &CoopScoreLayout, s: &HudState) {
    if s.mode_class != Some(COOP_CLASS) {
        return;
    }
    let now = s.now_ms;
    if let (Some((label, at, size)), Some(d)) = (score_places(l), l.digits.as_ref()) {
        out.push(picture(&label.tex, label.pos, label.size, 255));
        let mut lead = true;
        for (digit, p) in score_digits(s.coop.score_at(now)).into_iter().zip(at) {
            lead &= digit == 0;
            let alpha = if lead { LEAD_ZERO_ALPHA } else { 255 };
            out.push(picture(&format!("{}{digit}.dtx", d.form), p, size, alpha));
        }
    }
    // a note without a picture takes no slot (cshell+0x800fc: no picture, no note)
    let p = &l.pickup;
    let shown = s.coop.notes.iter().filter_map(|n| {
        let tex = p
            .pictures
            .get(usize::from(n.item).wrapping_sub(1))?
            .as_ref()?;
        Some((tex, note_alpha(l, n, now)?))
    });
    for (k, (tex, alpha)) in shown.enumerate() {
        let k = k as i32;
        let pos = [p.pos[0] + k * p.offset[0], p.pos[1] + k * p.offset[1]];
        out.push(picture(tex, pos, p.size, alpha));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::banners::Slot;
    use crate::hud::compose::{Tex, TextureSource};
    use crate::hud::npcbar::on_points;
    use crate::hud::state::BoardRow;

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

    fn layout() -> CoopScoreLayout {
        let mut l = CoopScoreLayout::fallback();
        l.label = Some(Img {
            tex: "t\\label.dtx".into(),
            pos: [400, 10],
            size: [80, 20],
        });
        l.digits = Some(ScoreDigits {
            form: "t\\digit".into(),
            size: [16, 20],
            first: [490, 10],
            space: 2,
        });
        l.pickup.pictures = [
            Some("t\\health.dtx".into()),
            Some("t\\ammo.dtx".into()),
            Some("t\\box.dtx".into()),
        ];
        l
    }

    fn images(out: &[DrawItem]) -> Vec<(String, [f32; 4], u8)> {
        out.iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, dst, tint, .. } => Some((tex.clone(), *dst, tint[3])),
                _ => None,
            })
            .collect()
    }

    /// Notes stack from the place by the offset, hold, fade, close up when the oldest goes; a fifth drops the oldest;
    /// each plays its pickup sound; an unknown item shows and plays nothing.
    #[test]
    fn pickup_notes_stack_fade_and_close_up() {
        let l = layout();
        let mut s = coop(0);
        on_pickup(&mut s, PICKUP_HEALTH, 0);
        on_pickup(&mut s, PICKUP_AMMO, 500);
        on_pickup(&mut s, 9, 600);
        assert_eq!(s.banner.voices, [Voice::CoopMedikit, Voice::CoopMagazine]);
        s.now_ms = 1000;
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        let notes: Vec<_> = images(&out).into_iter().skip(7).collect();
        assert_eq!(
            notes,
            vec![
                ("t\\health.dtx".into(), [880.0, 420.0, 64.0, 64.0], 255),
                ("t\\ammo.dtx".into(), [880.0, 348.0, 64.0, 64.0], 255),
            ]
        );
        // the first fades (show 2000 ms, fade 500 ms): half-way at 2250
        assert_eq!(note_alpha(&l, &s.coop.notes[0], 2250), Some(127));
        assert_eq!(note_alpha(&l, &s.coop.notes[0], 2501), None);
        // gone: the second moves down to the first place
        s.now_ms = 2600;
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        let notes: Vec<_> = images(&out).into_iter().skip(7).collect();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].1, [880.0, 420.0, 64.0, 64.0]);
        // five at once: the oldest dropped
        let mut s = coop(0);
        for (k, item) in [1, 2, 3, 1, 3].into_iter().enumerate() {
            on_pickup(&mut s, item, k as u32 * 10);
        }
        assert_eq!(s.coop.notes.len(), NOTE_SLOTS);
        assert_eq!(s.coop.notes[0], PickupNote { item: 2, at: 10 });
        assert_eq!(s.banner.voices.last(), Some(&Voice::CoopBulletbox));
        // nothing outside class 5
        s.mode_class = Some(1);
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        assert!(out.is_empty());
    }

    /// Three own kills inside the window: "1 Kill", then "2 COMBO", "3 COMBO"; a gap ends the chain, the next kill
    /// shows "4 Kills"; score and kills keep. Hits (sub 3) and other players' kills start nothing; no kill-feed line
    /// in class 5 (CLI-1858). After a 0xc8 the client orders no banner.
    #[test]
    fn the_combo_counts_own_kills_inside_the_window() {
        let mut s = coop(0);
        let banner = |s: &HudState| s.banner.show.map(|b| (b.slot, b.number));
        on_points(&mut s, 0, 24, 24, true, 1000);
        assert_eq!(banner(&s), Some((Slot::OneKill, Some(1))));
        on_points(&mut s, 0, 9, 33, false, 1500);
        assert_eq!(s.coop.combo_at(1500), 1, "a hit is no kill");
        on_points(&mut s, 3, 24, 24, true, 1600);
        assert_eq!(s.coop.kills, 1, "another player's kill");
        on_points(&mut s, 0, 54, 87, true, 2000);
        assert_eq!(banner(&s), Some((Slot::Combo, Some(2))));
        on_points(&mut s, 0, 24, 111, true, 5000);
        assert_eq!(banner(&s), Some((Slot::Combo, Some(3))));
        assert_eq!(s.coop.combo_at(5000 + COMBO_WINDOW_MS), 3);
        assert_eq!(s.coop.combo_at(5001 + COMBO_WINDOW_MS), 0, "the chain ends");
        // the next kill after the gap starts a new chain; kills and score keep
        on_points(&mut s, 0, 24, 135, true, 9000);
        assert_eq!(banner(&s), Some((Slot::OneKills, Some(4))));
        assert_eq!((s.coop.kills, s.coop.combo, s.coop.score), (4, 1, 135));
        assert!(s.feed.is_empty(), "no kill-feed line in class 5");
        // a server that sends 0xc8 orders the banners itself
        s.apply(
            &crate::hud::state::HudEvent::KillRun {
                kills: 0,
                head: false,
                combo: 0,
            },
            9100,
            0.0,
        );
        assert!(s.coop.server_runs);
        s.banner.show = None;
        on_points(&mut s, 0, 24, 159, true, 9200);
        assert_eq!(banner(&s), None);
        assert_eq!((s.coop.kills, s.coop.combo), (5, 2), "still counted");
        // outside class 5 no kill run
        let mut t = coop(0);
        t.mode_class = Some(1);
        on_points(&mut t, 0, 24, 24, true, 100);
        assert_eq!((t.coop.kills, banner(&t)), (0, None));
    }

    /// The score: the last own total of sub 1 / 3, counted up from the previous one over 150 ms; six digits with
    /// faint leading zeros; the label and digits centred as a group on x 512.
    #[test]
    fn the_score_rolls_and_shows_six_digits_centred() {
        let l = layout();
        let mut s = coop(0);
        on_points(&mut s, 0, 24, 24, true, 1000);
        on_points(&mut s, 0, 9, 33, false, 2000);
        on_points(&mut s, 2, 9, 900, false, 2000);
        assert_eq!(s.coop.score_at(2000), 24);
        assert_eq!(s.coop.score_at(2075), 28, "24 + 9 x 75 / 150");
        assert_eq!(s.coop.score_at(2150), 33);
        assert_eq!(score_digits(1_234_567), [2, 3, 4, 5, 6, 7]);
        let (label, at, size) = score_places(&l).unwrap();
        // group 400..596 (196 wide) moved to start at (1024 - 196) / 2 = 414
        assert_eq!(label.pos, [414, 10]);
        assert_eq!(at[0], [504, 10]);
        assert_eq!(at[5], [594, 10]);
        assert_eq!(size, [16, 20]);
        s.now_ms = 3000;
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        let im = images(&out);
        assert_eq!(im[0].0, "t\\label.dtx");
        let digits: Vec<_> = im[1..7].iter().map(|d| (d.0.as_str(), d.2)).collect();
        assert_eq!(
            digits,
            [
                ("t\\digit0.dtx", LEAD_ZERO_ALPHA),
                ("t\\digit0.dtx", LEAD_ZERO_ALPHA),
                ("t\\digit0.dtx", LEAD_ZERO_ALPHA),
                ("t\\digit0.dtx", LEAD_ZERO_ALPHA),
                ("t\\digit3.dtx", 255),
                ("t\\digit3.dtx", 255),
            ]
        );
        // a zero after the first other digit is full
        s.coop = CoopScoreHud {
            score: 105,
            ..CoopScoreHud::default()
        };
        let mut out = Vec::new();
        draw(&mut out, &l, &s);
        assert_eq!(
            images(&out)[5],
            ("t\\digit0.dtx".into(), [576.0, 10.0, 16.0, 20.0], 255)
        );
    }

    /// With the user's ROM: the co-op section's score and drop-item keys resolve (label, digit form, three pictures,
    /// sizes, times) and their textures load; the composed frame draws the label and the health picture opaque.
    /// Skipped without CA_ROM.
    #[test]
    fn rom_score_and_pickup_keys_resolve_and_draw() {
        let Ok(rom) = std::env::var("CA_ROM") else {
            println!("skipped: CA_ROM unset");
            return;
        };
        for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
            let mut hud =
                crate::hud::Hud::from_rom(std::path::Path::new(&rom), w, h, None).expect("ROM HUD");
            let l = hud.layout.top.coop.clone();
            let (label, at, size) = score_places(&l).expect("score keys");
            assert!(label.size[0] > 0 && size[0] > 0 && size[1] > 0);
            assert!(at[0][0] > label.pos[0], "digits right of the label");
            let p = &l.pickup;
            assert!(p.pictures.iter().all(Option::is_some), "three pictures");
            assert!(p.size[0] > 0 && p.size[1] > 0 && p.show_ms > 0);
            let d = l.digits.as_ref().unwrap();
            for t in [label.tex.clone(), format!("{}7.dtx", d.form)]
                .into_iter()
                .chain(p.pictures.iter().flatten().cloned())
            {
                assert!(hud.assets.texture(&t).is_some(), "texture {t}");
            }
            let mut s = coop(0);
            on_pickup(&mut s, PICKUP_HEALTH, 0);
            s.now_ms = 100;
            let img = hud.render(&s);
            let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
            let opaque = |r: [i32; 4]| {
                let (x0, y0) = ((r[0] as f32 * sx) as u32, (r[1] as f32 * sy) as u32);
                let (x1, y1) = (
                    ((r[0] + r[2]) as f32 * sx) as u32,
                    ((r[1] + r[3]) as f32 * sy) as u32,
                );
                (y0..y1.min(h))
                    .flat_map(|y| (x0..x1.min(w)).map(move |x| (x, y)))
                    .filter(|&(x, y)| img.pixel(x, y)[3] > 200)
                    .count()
            };
            let lp = [label.pos[0], label.pos[1], label.size[0], label.size[1]];
            assert!(opaque(lp) > 10, "{w}x{h}: the score label");
            let pp = [p.pos[0], p.pos[1], p.size[0], p.size[1]];
            assert!(opaque(pp) > 10, "{w}x{h}: the health picture");
        }
    }

    /// Solid textures by name: the label red, the score digits green, the health pack blue, the combo picture yellow
    /// and the banner digits cyan.
    struct Solid;
    impl TextureSource for Solid {
        fn load(&mut self, path: &str) -> Option<Tex> {
            let c: [u8; 3] = if path.contains("label") {
                [200, 0, 0]
            } else if path.contains("bdigit") {
                [0, 200, 200]
            } else if path.contains("digit") {
                [0, 200, 0]
            } else if path.contains("health") {
                [0, 0, 200]
            } else if path.contains("combo") {
                [200, 200, 0]
            } else {
                return None;
            };
            let n = 128u32;
            let rgba = (0..n * n).flat_map(|_| [c[0], c[1], c[2], 255]).collect();
            Some(Tex {
                width: n,
                height: n,
                rgba,
            })
        }
    }

    /// The headless frame (CPU compositor) at 1024 x 768 and 2560 x 1440 after three quick own kills and a health
    /// pickup: the score label and digits at the top (leading zeros faint), the health picture at its place and the
    /// "3 COMBO" banner (digit then picture).
    #[test]
    fn a_headless_frame_shows_score_pickup_and_combo() {
        use crate::testkit::synth_font;
        for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
            let mut hud = crate::hud::Hud::fallback(w, h, Some((synth_font(), 0)));
            hud.assets.textures = Box::new(Solid);
            hud.layout.top.coop = layout();
            let b = &mut hud.banners;
            b.slots
                .insert(Slot::Combo, ("t\\combo.dtx".into(), [100, 30]));
            b.slots
                .insert(Slot::OneKill, ("t\\combo.dtx".into(), [100, 30]));
            b.text_pos = [512, 300];
            b.digits = Some("t\\bdigit".into());
            b.digit_size = [20, 30];
            b.spacing = [2, 0];
            b.spacing2 = [4, 0];
            b.timing = Some(crate::hud::layout::Announce {
                tex: String::new(),
                size: [0, 0],
                centre: [0, 0],
                total: 3.0,
                create: 0.0,
                end: 2.0,
                destroy: 1.0,
                first_scale: 1.0,
            });
            let mut s = coop(0);
            for (k, t) in [1000u32, 1500, 2000].into_iter().enumerate() {
                on_points(&mut s, 0, 24, 24 * (k as u32 + 1), true, t);
            }
            on_pickup(&mut s, PICKUP_HEALTH, 2000);
            s.now_ms = 2300;
            assert_eq!(
                s.banner.show.map(|b| (b.slot, b.number)),
                Some((Slot::Combo, Some(3)))
            );
            let img = hud.render(&s);
            let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
            let px = |vx: f32, vy: f32| img.pixel((vx * sx) as u32, (vy * sy) as u32);
            // the label 414..494 x 10..30
            assert_eq!(px(450.0, 20.0)[..3], [200, 0, 0], "{w}x{h}: label");
            // 000072: digit 0 (504..520) faint, digit 4 ("7", 576..592) full
            let faint = px(512.0, 20.0);
            assert_eq!(faint[..3], [0, 200, 0], "{w}x{h}: faint digit {faint:?}");
            assert!(
                (40..120).contains(&faint[3]),
                "{w}x{h}: faint digit {faint:?}"
            );
            let full = px(584.0, 20.0);
            assert_eq!(full, [0, 200, 0, 255], "{w}x{h}: full digit");
            // the health picture 880..944 x 420..484
            assert_eq!(px(912.0, 452.0), [0, 0, 200, 255], "{w}x{h}: pickup");
            // "3 COMBO": total 20 + 4 + 100 = 124 from 450: the digit 450..470, the picture 474..574, y 300..330
            assert_eq!(px(460.0, 315.0)[..3], [0, 200, 200], "{w}x{h}: combo digit");
            assert_eq!(
                px(524.0, 315.0)[..3],
                [200, 200, 0],
                "{w}x{h}: combo picture"
            );
        }
    }
}

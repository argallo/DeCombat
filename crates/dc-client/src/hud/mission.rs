//! Fire Team on the client (mode class 5): 0xc9 `GS_COOP_MISSION` sub 1 rebuilds the mission's goal list, sub 5
//! adds, removes or sets one goal, sub 2 starts the mission message banner. The HUD draws the newest five goals in
//! the top-left corner (a state icon and the goal's text in the state's colour) and the banner across the upper
//! middle of the screen (a back image that opens, the text that zooms in, holds for the message's seconds, then
//! fades and the back image closes), and plays the mode's message sound with each banner. Sub 3 shows a dialogue
//! caption at the bottom of the screen (picture, speaker, the text typed out letter by letter, held for the message's
//! seconds after the last letter, then faded over 1 s; CLI-1860).
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (0xc9 subs 1, 2, 5); FIRETEAM_2009.md §2.4
// EVIDENCE: cshell+0x31490 (0xc9: sub 1 clears the goals, reads n x [id][label][state] into the goal map and the
// goal list in message order, the timers and the two scoreboard bytes; sub 2 the banner text and seconds; sub 5 op 1
// adds a goal (state 0) unless its id is known, op 2 removes it, op 3 sets its state), cshell+0x30bd0 (the newest
// five of the goal list, in list order), cshell+0x7f290 (the element's five rows, their keys), cshell+0x81930 (per
// frame, class 5 only: each row's icon, colour and size by the goal's state, the rows placed down from the text
// place), cshell+0xbcfd0 (the banner's keys), cshell+0xbdd20 (a banner starts: text, hold time, both shown),
// cshell+0xbc7d0 (the banner's phases)

use super::banners::Voice;
use super::items::DrawItem;
use super::layout::{Img, TextStyle};
use super::state::HudState;
use dc_ui_data::{HudKey as K, HudView};

/// The state byte of a goal: open (in progress), done, failed.
// EVIDENCE: cshell+0x81b2e..0x81bcd (state 0 -> the in-progress icon, 1 -> the done icon, 2 -> the failed icon; the
// three icon keys of cshell+0x7f290 name the states)
pub const GOAL_OPEN: u8 = 0;
pub const GOAL_DONE: u8 = 1;
pub const GOAL_FAILED: u8 = 2;

/// How many goals the element shows (the newest ones).
// EVIDENCE: cshell+0x81ad9 (cshell+0x30bd0 asked for 5), cshell+0x7f4ca (five rows built)
pub const GOAL_ROWS: usize = 5;

/// The text font record of the goal rows and the text size per state (open, done, failed).
// EVIDENCE: cshell+0x7f4a4..0x7f4ae (font record 0x1a), cshell+0x81b67 / 0x81b96 / 0x81bc7 (size 0xe failed, 0xe
// done, 0x10 open)
pub const GOAL_FONT: i32 = 0x1a;
pub const GOAL_SIZES: [i32; 3] = [0x10, 0xe, 0xe];
/// The text colour per state (open, done, failed), straight RGBA of the client's ARGB words.
// EVIDENCE: cshell+0x81bad (0xffc3fbfc open), cshell+0x81b78 (0xff6e6e6e done), cshell+0x81b4d (0xffff1414 failed)
pub const GOAL_COLOURS: [[u8; 4]; 3] = [
    [0xc3, 0xfb, 0xfc, 0xff],
    [0x6e, 0x6e, 0x6e, 0xff],
    [0xff, 0x14, 0x14, 0xff],
];

/// The banner's phases in milliseconds from its start: the back image widens, then grows in height, then the text
/// zooms in and fades in; the intro ends at `INTRO_MS`; after the hold the text fades out, the back image shrinks in
/// height and narrows.
// EVIDENCE: cshell+0xbc7d0 (intro: < 200 width t x 0.005 at height 0.2, < 500 height (t - 200) x 0.0045 up to 1,
// text from 0x320, its alpha (t - 800) x 0.31875 up to 255 and its size + (1000 - t) x 0.19, the intro over at 1999;
// then the hold while t <= the hold time; outro u from its start: < 600 the text's alpha 255 - 1.275 u down to 0 and
// its size + 0.19 u, < 800 the back image's height 0.2 + (800 - u) x 0.004, < 1000 its width (1000 - u) x 0.005,
// then both hidden)
pub const INTRO_MS: u32 = 2000;
pub const OUTRO_MS: u32 = 1000;

/// One goal of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    pub id: u32,
    pub label: String,
    pub state: u8,
}

/// One 0xc9 message the HUD takes.
#[derive(Debug, Clone, PartialEq)]
pub enum MissionEvent {
    /// Sub 1: the whole goal list in message order, the timers, the two scoreboard bytes (`stage`, `goal`).
    List {
        goals: Vec<Goal>,
        timers: Vec<f32>,
        stage: u8,
        goal: u8,
    },
    /// Sub 2: a banner text shown for `secs`.
    Banner { text: String, secs: f32 },
    /// Sub 5 op 1: a goal added (state 0) unless its id is in the list.
    Added { id: u32, label: String },
    /// Sub 5 op 2: a goal removed.
    Removed { id: u32 },
    /// Sub 5 op 3: a goal's state set.
    State { id: u32, state: u8 },
    /// Sub 3: a dialogue caption `picture|speaker|text`, held `secs` after its last letter.
    Caption { text: String, secs: f32 },
}

/// The caption on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptionShow {
    /// The speaker's picture (a texture of the ROM; not drawn when it does not load).
    pub picture: String,
    pub speaker: String,
    /// The caption's text, wrapped ([`wrap_caption`]).
    pub lines: Vec<String>,
    pub at: u32,
    /// The hold after the last letter (the message's seconds x 1000).
    pub hold_ms: u32,
}

impl CaptionShow {
    /// The letters typed out (the wrapped lines' characters).
    pub fn letters(&self) -> usize {
        self.lines.iter().map(|l| l.chars().count()).sum()
    }

    /// The wrapped lines with only the first `n` letters.
    pub fn typed(&self, n: usize) -> Vec<String> {
        let mut left = n;
        let mut out = Vec::new();
        for l in &self.lines {
            if left == 0 {
                break;
            }
            let k = l.chars().count().min(left);
            out.push(l.chars().take(k).collect());
            left -= k;
        }
        out
    }
}

/// The time between two letters of a caption.
// DC-DEFAULT: CLI-1860 (proposed) 40 ms: the 2009 typing rate is not traced (cshell+0x81fe0 types the text out)
pub const CAPTION_LETTER_MS: u32 = 40;
/// The caption's fade after its hold.
// EVIDENCE: cshell+0x81930 (the caption fades over 1000 ms once its hold after the last letter is over; DSV-579)
pub const CAPTION_FADE_MS: u32 = 1000;
/// The caption's wrap width (characters) and most lines.
// DC-DEFAULT: CLI-1860 (proposed)
pub const CAPTION_LINE_CHARS: usize = 64;
pub const CAPTION_LINES: usize = 4;

/// The caption's look on the 1024 x 768 virtual screen: a dark box at the bottom, the picture on its left, the
/// speaker and the text right of it.
// DC-DEFAULT: CLI-1860 (proposed) our own places and sizes (the 2009 element's keys are not traced); the colours are
// the 2009 ones: EVIDENCE cshell+0x81fe0 (speaker 0xffc9ffff, text 0xffebebeb)
pub const CAPTION_BOX: [f32; 4] = [192.0, 590.0, 640.0, 108.0];
pub const CAPTION_BOX_COLOUR: [u8; 4] = [0, 0, 0, 160];
pub const CAPTION_PICTURE: [f32; 4] = [200.0, 596.0, 96.0, 96.0];
pub const CAPTION_SPEAKER_POS: [f32; 2] = [308.0, 598.0];
pub const CAPTION_TEXT_POS: [f32; 2] = [308.0, 620.0];
pub const CAPTION_LINE_STEP: f32 = 18.0;
pub const CAPTION_SPEAKER_SIZE: i32 = 16;
pub const CAPTION_TEXT_SIZE: i32 = 14;
pub const CAPTION_SPEAKER_COLOUR: [u8; 4] = [0xc9, 0xff, 0xff, 0xff];
pub const CAPTION_TEXT_COLOUR: [u8; 4] = [0xeb, 0xeb, 0xeb, 0xff];

/// The three fields of a caption text: picture, speaker, caption. Any other field count is no caption.
// DISASM: cshell+0x81fe0 (a `|` appended when the text lacks it, split at `|`, the caption shown only with three
// fields)
// DC-DEFAULT: CLI-1860 (proposed) one trailing `|` ends the last field; an empty field counts
pub fn split_caption(text: &str) -> Option<(String, String, String)> {
    let t = text.strip_suffix('|').unwrap_or(text);
    let f: Vec<&str> = t.split('|').collect();
    match f[..] {
        [p, s, c] => Some((p.trim().into(), s.trim().into(), c.trim().into())),
        _ => None,
    }
}

/// Word-wraps a caption to [`CAPTION_LINE_CHARS`] characters per line, at most [`CAPTION_LINES`] lines (the rest is
/// dropped); a word longer than a line is cut.
// DC-DEFAULT: CLI-1860 (proposed)
pub fn wrap_caption(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let used = cur.chars().count();
            let room = if used == 0 {
                CAPTION_LINE_CHARS
            } else {
                CAPTION_LINE_CHARS.saturating_sub(used + 1)
            };
            if word.len() <= room {
                if used > 0 {
                    cur.push(' ');
                }
                cur.extend(word.iter());
                break;
            }
            if used == 0 {
                // longer than a whole line: cut
                cur.extend(word.drain(..CAPTION_LINE_CHARS));
            }
            lines.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines.truncate(CAPTION_LINES);
    lines
}

/// The caption `t` ms after its start: the letters shown and the alpha (255 while typed and held), `None` once it
/// is over. Letter `k` (from 1) shows at `(k - 1) x` [`CAPTION_LETTER_MS`]; the hold runs from the last letter.
// EVIDENCE: cshell+0x81930 (the hold after the last letter, then the 1000 ms fade)
pub fn caption_phase(t: u32, letters: usize, hold_ms: u32) -> Option<(usize, f32)> {
    let last = (letters.saturating_sub(1) as u32).saturating_mul(CAPTION_LETTER_MS);
    let shown = ((t / CAPTION_LETTER_MS) as usize)
        .saturating_add(1)
        .min(letters);
    let fade_at = last.saturating_add(hold_ms);
    if t <= fade_at {
        return Some((shown, 255.0));
    }
    let u = t - fade_at;
    if u >= CAPTION_FADE_MS {
        return None;
    }
    Some((letters, 255.0 * (1.0 - u as f32 / CAPTION_FADE_MS as f32)))
}

/// The banner on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct BannerShow {
    pub text: String,
    pub at: u32,
    /// The hold time from the start (the message's seconds x 1000).
    // EVIDENCE: cshell+0x316e1..0x316eb (secs x 1000.0, truncated, handed to cshell+0xbdd20)
    pub hold_ms: u32,
}

/// What the mission HUD keeps.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MissionHud {
    /// The goals in list order (oldest first).
    pub goals: Vec<Goal>,
    /// The last sub 1's scoreboard bytes.
    pub stage: Option<(u8, u8)>,
    pub banner: Option<BannerShow>,
    pub caption: Option<CaptionShow>,
}

impl MissionHud {
    /// The goals the element shows: the newest [`GOAL_ROWS`], in list order.
    pub fn shown(&self) -> &[Goal] {
        let n = self.goals.len();
        &self.goals[n.saturating_sub(GOAL_ROWS)..]
    }

    /// The caption's letters and alpha at `now`, or `None` once it is over.
    pub fn caption_at(&self, now: u32) -> Option<(usize, f32)> {
        let c = self.caption.as_ref()?;
        caption_phase(now.saturating_sub(c.at), c.letters(), c.hold_ms)
    }

    /// The banner's state at `now`, or `None` once it is over.
    pub fn banner_at(&self, now: u32) -> Option<BannerPhase> {
        let b = self.banner.as_ref()?;
        banner_phase(now.saturating_sub(b.at), b.hold_ms)
    }
}

/// The banner at one moment: the back image's scale across and down (about its centre), the text's alpha and size
/// added to the layout's size (`None`: the text is hidden).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BannerPhase {
    pub back_scale: [f32; 2],
    pub text: Option<(f32, f32)>,
}

/// The banner `t` ms after its start with the hold time `hold` (see [`INTRO_MS`]).
// DC-DEFAULT: CLI-1859 (proposed) the back image scales about its centre; a text size the client would set to 12 or
// less (its "no size" value) is the layout's size
pub fn banner_phase(t: u32, hold: u32) -> Option<BannerPhase> {
    let t32 = t as f32;
    if t < INTRO_MS {
        let back_scale = if t < 200 {
            [t32 * 0.005, 0.2]
        } else if t < 500 {
            [1.0, ((t32 - 200.0) * 0.0045).min(1.0)]
        } else {
            [1.0, 1.0]
        };
        let text = (t >= 800).then(|| {
            let alpha = ((t32 - 800.0) * 0.31875).min(255.0);
            (alpha, ((1000.0 - t32) * 0.19).max(0.0))
        });
        return Some(BannerPhase { back_scale, text });
    }
    let outro_at = hold.max(INTRO_MS);
    if t <= outro_at {
        return Some(BannerPhase {
            back_scale: [1.0, 1.0],
            text: Some((255.0, 0.0)),
        });
    }
    let u = t - outro_at;
    let u32f = u as f32;
    if u < 600 {
        let alpha = (255.0 - 1.275 * u32f).max(0.0);
        return Some(BannerPhase {
            back_scale: [1.0, 1.0],
            text: (alpha > 0.0).then_some((alpha, 0.19 * u32f)),
        });
    }
    if u < 800 {
        return Some(BannerPhase {
            back_scale: [1.0, (0.2 + (800.0 - u32f) * 0.004).max(0.2)],
            text: None,
        });
    }
    if u < OUTRO_MS {
        return Some(BannerPhase {
            back_scale: [((1000.0 - u32f) * 0.005).max(0.0), 0.2],
            text: None,
        });
    }
    None
}

/// The banner's look.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionBannerLayout {
    pub back: Option<Img>,
    /// The text's centre.
    pub text_pos: [i32; 2],
    pub font: i32,
    pub size: i32,
    pub colour: [u8; 4],
}

/// The goal rows and the banner.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionLayout {
    /// The icon per state (open, done, failed).
    pub icons: [Option<String>; 3],
    pub icon_size: [i32; 2],
    /// The icons' x (the key's y is not used: each icon is centred on its row's text).
    pub icon_x: i32,
    /// The first row's text place (top-left).
    pub text_pos: [i32; 2],
    /// Added to the place from one row to the next, besides the row's text height.
    pub step: [i32; 2],
    pub font: i32,
    pub sizes: [i32; 3],
    pub banner: MissionBannerLayout,
}

impl Default for MissionLayout {
    fn default() -> Self {
        MissionLayout::fallback()
    }
}

impl MissionLayout {
    /// The co-op section's keys.
    pub fn load(v: &HudView<'_>) -> Self {
        let own = MissionLayout::fallback();
        let text = |k: K| v.text(k).map(str::to_string);
        let back = (|| {
            Some(Img {
                tex: v.text(K::CoopBannerBackTex)?.to_string(),
                pos: v.pair(K::CoopBannerBackPos)?,
                size: v.pair(K::CoopBannerBackSize)?,
            })
        })();
        MissionLayout {
            icons: [
                text(K::CoopGoalOpenIcon),
                text(K::CoopGoalDoneIcon),
                text(K::CoopGoalFailedIcon),
            ],
            icon_size: v.pair(K::CoopGoalIconSize).unwrap_or([0, 0]),
            icon_x: v.pair(K::CoopGoalIconPos).map_or(own.icon_x, |p| p[0]),
            text_pos: v.pair(K::CoopGoalTextPos).unwrap_or(own.text_pos),
            step: v.pair(K::CoopGoalTextStep).unwrap_or(own.step),
            font: GOAL_FONT,
            sizes: GOAL_SIZES,
            banner: MissionBannerLayout {
                back,
                text_pos: v.pair(K::CoopBannerTextPos).unwrap_or(own.banner.text_pos),
                font: v.int(K::CoopBannerFont).unwrap_or(own.banner.font),
                size: v.int(K::CoopBannerFontSize).unwrap_or(own.banner.size),
                colour: v
                    .rgba(K::CoopBannerColour, None)
                    .unwrap_or(own.banner.colour),
            },
        }
    }

    /// Our own layout without a ROM: text rows only (no icons), the banner's text without a back image.
    // DC-DEFAULT: CLI-1858 (proposed) part of the no-ROM layout
    pub fn fallback() -> Self {
        MissionLayout {
            icons: [None, None, None],
            icon_size: [0, 0],
            icon_x: 8,
            text_pos: [12, 12],
            step: [0, 6],
            font: -1,
            sizes: [18, 16, 16],
            banner: MissionBannerLayout {
                back: None,
                text_pos: [512, 236],
                font: -1,
                size: 18,
                colour: [255, 255, 255, 255],
            },
        }
    }
}

/// The text height of a row of `size` (one line: the cell height the text is asked for).
// DC-DEFAULT: CLI-1858 (proposed) the 2009 row height is the drawn text's height (cshell+0x81be0 reads it from the
// text item before this frame's text is set); a goal text is one line, so the cell height stands for it
fn row_height(size: i32) -> i32 {
    size.max(1)
}

/// The goal-list rows: `(icon place, text place, state index)` of each shown goal, top to bottom.
// EVIDENCE: cshell+0x81ae1..0x81cd2 (text at (x, y); icon at (icon x, y + (text height - icon height) x 0.5); then
// x += step x, y += step y + text height)
pub fn goal_rows(l: &MissionLayout, goals: &[Goal]) -> Vec<([i32; 2], [i32; 2], usize)> {
    let [mut x, mut y] = l.text_pos;
    goals
        .iter()
        .map(|g| {
            let k = state_index(g.state);
            let h = row_height(l.sizes[k]);
            let icon = [l.icon_x, y + (h - l.icon_size[1]) / 2];
            let at = [x, y];
            x += l.step[0];
            y += l.step[1] + h;
            (icon, at, k)
        })
        .collect()
}

/// The style index of a state byte; an unknown state draws as open.
// DC-DEFAULT: CLI-1858 (proposed) the 2009 row keeps its previous look for a state other than 0 / 1 / 2
// (cshell+0x81b3a falls through), which a fresh row does not have: open
fn state_index(state: u8) -> usize {
    match state {
        GOAL_DONE => 1,
        GOAL_FAILED => 2,
        _ => 0,
    }
}

/// The dialogue caption (any class: only the co-op worlds send it).
// DC-DEFAULT: CLI-1860 (proposed)
fn draw_caption(
    out: &mut Vec<DrawItem>,
    l: &MissionLayout,
    s: &HudState,
    tex_size: super::items::TexSize<'_>,
) {
    let m = &s.mission;
    let (Some(c), Some((n, alpha))) = (m.caption.as_ref(), m.caption_at(s.now_ms)) else {
        return;
    };
    let fade = |mut colour: [u8; 4]| {
        colour[3] = (alpha.clamp(0.0, 255.0) * colour[3] as f32 / 255.0) as u8;
        colour
    };
    let b = CAPTION_BOX;
    out.push(DrawItem::Fill {
        rect: [b[0], b[1], b[0] + b[2], b[1] + b[3]],
        colour: fade(CAPTION_BOX_COLOUR),
    });
    if let Some(size) = (!c.picture.is_empty())
        .then(|| tex_size(&c.picture))
        .flatten()
    {
        out.push(DrawItem::Image {
            tex: c.picture.clone(),
            src: size,
            dst: CAPTION_PICTURE,
            tint: fade([255, 255, 255, 255]),
            turn: None,
        });
    }
    let text = |size, colour, pos, text| DrawItem::Text {
        style: TextStyle {
            font: l.font,
            colour: fade(colour),
            size: Some(size),
        },
        pos,
        text,
        right: false,
    };
    out.push(text(
        CAPTION_SPEAKER_SIZE,
        CAPTION_SPEAKER_COLOUR,
        CAPTION_SPEAKER_POS,
        c.speaker.clone(),
    ));
    for (k, line) in c.typed(n).into_iter().enumerate() {
        let pos = [
            CAPTION_TEXT_POS[0],
            CAPTION_TEXT_POS[1] + k as f32 * CAPTION_LINE_STEP,
        ];
        out.push(text(CAPTION_TEXT_SIZE, CAPTION_TEXT_COLOUR, pos, line));
    }
}

/// The goal list, the caption and the banner (the list only in mode class 5).
pub fn draw(
    out: &mut Vec<DrawItem>,
    l: &MissionLayout,
    s: &HudState,
    tex_size: super::items::TexSize<'_>,
) {
    let m = &s.mission;
    if s.mode_class == Some(5) {
        let shown = m.shown();
        for ((icon, at, k), g) in goal_rows(l, shown).into_iter().zip(shown) {
            if let Some(tex) = &l.icons[k] {
                out.push(DrawItem::Image {
                    tex: tex.clone(),
                    src: [l.icon_size[0] as f32, l.icon_size[1] as f32],
                    dst: [
                        icon[0] as f32,
                        icon[1] as f32,
                        l.icon_size[0] as f32,
                        l.icon_size[1] as f32,
                    ],
                    tint: [255, 255, 255, 255],
                    turn: None,
                });
            }
            // EVIDENCE: cshell+0x7f4da (the row's text item: flags 0x40011, neither centred nor right-aligned)
            out.push(DrawItem::Text {
                style: TextStyle {
                    font: l.font,
                    colour: GOAL_COLOURS[k],
                    size: Some(l.sizes[k]),
                },
                pos: [at[0] as f32, at[1] as f32],
                text: g.label.clone(),
                right: false,
            });
        }
    }
    draw_caption(out, l, s, tex_size);
    let Some(b) = m.banner.as_ref() else {
        return;
    };
    let Some(p) = m.banner_at(s.now_ms) else {
        return;
    };
    let bl = &l.banner;
    if let Some(back) = &bl.back {
        let (w, h) = (back.size[0] as f32, back.size[1] as f32);
        let (sw, sh) = (w * p.back_scale[0], h * p.back_scale[1]);
        let c = [back.pos[0] as f32 + w * 0.5, back.pos[1] as f32 + h * 0.5];
        if sw > 0.0 && sh > 0.0 {
            out.push(DrawItem::Image {
                tex: back.tex.clone(),
                src: [w, h],
                dst: [c[0] - sw * 0.5, c[1] - sh * 0.5, sw, sh],
                tint: [255, 255, 255, 255],
                turn: None,
            });
        }
    }
    if let Some((alpha, grow)) = p.text {
        let mut colour = bl.colour;
        colour[3] = (alpha.clamp(0.0, 255.0) * bl.colour[3] as f32 / 255.0) as u8;
        // EVIDENCE: cshell+0xbd3f7 (the text item's flags 0x44: centred across and down on its place)
        out.push(DrawItem::Label {
            style: TextStyle {
                font: bl.font,
                colour,
                size: Some(bl.size + grow as i32),
            },
            pos: [bl.text_pos[0] as f32, bl.text_pos[1] as f32],
            text: b.text.clone(),
            anchor: [0.5, 0.5],
        });
    }
}

/// A text of the wire (the client converts it with the system code page): UTF-8 when it is, else one character per
/// byte.
// DC-DEFAULT: CLI-1858 (proposed) the text's bytes as UTF-8, Latin-1 when they are not UTF-8
pub fn wire_text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

impl HudState {
    /// One 0xc9 message.
    pub fn on_mission(&mut self, e: &MissionEvent, now: u32) {
        let m = &mut self.mission;
        match e {
            MissionEvent::List {
                goals, stage, goal, ..
            } => {
                m.goals = goals.clone();
                m.stage = Some((*stage, *goal));
            }
            MissionEvent::Added { id, label } => {
                if !m.goals.iter().any(|g| g.id == *id) {
                    m.goals.push(Goal {
                        id: *id,
                        label: label.clone(),
                        state: GOAL_OPEN,
                    });
                }
            }
            MissionEvent::Removed { id } => m.goals.retain(|g| g.id != *id),
            MissionEvent::State { id, state } => {
                if let Some(g) = m.goals.iter_mut().find(|g| g.id == *id) {
                    g.state = *state;
                }
            }
            // EVIDENCE: cshell+0x31701 (the mode's message sound with each banner, cshell+0x300a0)
            MissionEvent::Banner { text, secs } => {
                m.banner = Some(BannerShow {
                    text: text.clone(),
                    at: now,
                    hold_ms: (secs.max(0.0) * 1000.0) as u32,
                });
                self.banner.voices.push(Voice::CoopMessage);
            }
            // EVIDENCE: cshell+0x31490 case 3 (the mode's message sound after the caption call, whatever the fields)
            MissionEvent::Caption { text, secs } => {
                if let Some((picture, speaker, caption)) = split_caption(text) {
                    m.caption = Some(CaptionShow {
                        picture,
                        speaker,
                        lines: wrap_caption(&caption),
                        at: now,
                        hold_ms: (secs.max(0.0) * 1000.0) as u32,
                    });
                }
                self.banner.voices.push(Voice::CoopMessage);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(id: u32, label: &str, state: u8) -> Goal {
        Goal {
            id,
            label: label.into(),
            state,
        }
    }

    #[test]
    fn the_list_keeps_message_order_and_shows_the_newest_five() {
        let mut h = HudState::default();
        let goals: Vec<Goal> = (0..7).map(|i| goal(i, &format!("g{i}"), 0)).collect();
        h.on_mission(
            &MissionEvent::List {
                goals,
                timers: vec![],
                stage: 1,
                goal: 0,
            },
            0,
        );
        let ids: Vec<u32> = h.mission.shown().iter().map(|g| g.id).collect();
        assert_eq!(ids, [2, 3, 4, 5, 6]);
        h.on_mission(&MissionEvent::Removed { id: 6 }, 0);
        h.on_mission(&MissionEvent::State { id: 3, state: 1 }, 0);
        h.on_mission(
            &MissionEvent::Added {
                id: 3,
                label: "again".into(),
            },
            0,
        );
        h.on_mission(
            &MissionEvent::Added {
                id: 9,
                label: "new".into(),
            },
            0,
        );
        let shown: Vec<(u32, u8)> = h.mission.shown().iter().map(|g| (g.id, g.state)).collect();
        assert_eq!(shown, [(2, 0), (3, 1), (4, 0), (5, 0), (9, 0)]);
        assert_eq!(
            h.mission.goals[3].label, "g3",
            "a known id is not added twice"
        );
    }

    #[test]
    fn rows_stack_down_with_the_text_height_and_centre_the_icons() {
        let l = MissionLayout {
            icons: [Some("o".into()), Some("d".into()), Some("f".into())],
            icon_size: [30, 30],
            icon_x: 2,
            text_pos: [40, 10],
            step: [0, 5],
            font: GOAL_FONT,
            sizes: GOAL_SIZES,
            banner: MissionLayout::fallback().banner,
        };
        let rows = goal_rows(&l, &[goal(0, "a", 0), goal(1, "b", 1), goal(2, "c", 2)]);
        // open: 16 tall; done / failed: 14
        assert_eq!(rows[0], ([2, 10 + (16 - 30) / 2], [40, 10], 0));
        assert_eq!(rows[1], ([2, 31 + (14 - 30) / 2], [40, 31], 1));
        assert_eq!(rows[2], ([2, 50 + (14 - 30) / 2], [40, 50], 2));
    }

    #[test]
    fn the_banner_opens_holds_for_its_seconds_and_closes() {
        let p = |t| banner_phase(t, 3000);
        assert_eq!(p(0).unwrap().back_scale, [0.0, 0.2]);
        assert_eq!(p(100).unwrap().back_scale, [0.5, 0.2]);
        assert!(p(300).unwrap().text.is_none(), "the text waits for 800 ms");
        let (a, grow) = p(900).unwrap().text.unwrap();
        assert!((a - 31.875).abs() < 0.01 && (grow - 19.0).abs() < 0.01);
        assert_eq!(p(1700).unwrap().text, Some((255.0, 0.0)));
        assert_eq!(p(2500).unwrap().text, Some((255.0, 0.0)), "held");
        assert_eq!(
            p(3000).unwrap().text,
            Some((255.0, 0.0)),
            "the hold's last ms"
        );
        let out = p(3100).unwrap();
        assert!(out.text.unwrap().0 < 255.0, "fading out");
        assert!(p(3300).unwrap().text.is_none());
        assert!(p(3700).unwrap().back_scale[1] < 1.0);
        assert!(p(3900).unwrap().back_scale[0] < 1.0);
        assert!(p(4001).is_none(), "over 1 s after the hold");
        // a hold shorter than the intro starts the outro at the intro's end
        assert!(banner_phase(2000, 500).is_some());
        assert!(banner_phase(3001, 500).is_none());
    }

    #[test]
    fn a_banner_plays_the_message_sound() {
        let mut h = HudState::default();
        h.on_mission(
            &MissionEvent::Banner {
                text: "Wave 2 / 5".into(),
                secs: 3.0,
            },
            500,
        );
        assert_eq!(h.mission.banner.as_ref().unwrap().hold_ms, 3000);
        assert_eq!(h.banner.voices, [Voice::CoopMessage]);
    }

    /// A co-op state with three goals: open, done, failed (labels the test font can draw).
    fn coop(now: u32) -> HudState {
        let mut s = HudState {
            mode_class: Some(5),
            life_state: 1,
            now_ms: now,
            ..HudState::default()
        };
        s.on_mission(
            &MissionEvent::List {
                goals: vec![goal(0, "ABAB", 0), goal(1, "BABA", 1), goal(2, "AABB", 2)],
                timers: vec![120.0],
                stage: 2,
                goal: 1,
            },
            now,
        );
        s
    }

    /// A layout with icons (our own made-up values, not the ROM's).
    fn with_icons() -> super::super::HudLayout {
        let mut l = super::super::HudLayout::fallback();
        l.top.mission = MissionLayout {
            icons: [
                Some("t\\open.dtx".into()),
                Some("t\\done.dtx".into()),
                Some("t\\failed.dtx".into()),
            ],
            icon_size: [24, 24],
            icon_x: 4,
            text_pos: [32, 14],
            step: [0, 6],
            font: GOAL_FONT,
            sizes: GOAL_SIZES,
            banner: MissionLayout::fallback().banner,
        };
        l
    }

    fn mission_items(l: &super::super::HudLayout, s: &HudState) -> Vec<DrawItem> {
        let mut spans = Vec::new();
        let items = super::super::items::build_marked(l, s, &|_, _| 0.0, &|_| None, &mut spans);
        let at = spans.iter().position(|e| e.0 == "coop_mission").unwrap();
        items[spans[at].1..spans[at + 1].1].to_vec()
    }

    /// A virtual rectangle `[x0, y0, x1, y1]` on a `w` x `h` target (the compositor's per-axis scale).
    fn on_target(r: [f32; 4], w: u32, h: u32) -> [f32; 4] {
        let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
        [r[0] * sx, r[1] * sy, r[2] * sx, r[3] * sy]
    }

    fn apart(a: [f32; 4], b: [f32; 4]) -> bool {
        a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1]
    }

    /// The goal rows in class 5: per state the icon, the colour and the size; at 1024 x 768 and 2560 x 1440 the rows
    /// stay clear of the round clock and of Gun Game's lines; no other class draws them.
    #[test]
    fn goal_rows_draw_each_state_and_stay_clear_of_the_clock_and_the_gun_game_lines() {
        let l = with_icons();
        let s = coop(1000);
        let items = mission_items(&l, &s);
        let icons: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, .. } => Some(tex.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(icons, ["t\\open.dtx", "t\\done.dtx", "t\\failed.dtx"]);
        type Row = (String, [u8; 4], Option<i32>, [f32; 2]);
        let texts: Vec<Row> = items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Text {
                    style, pos, text, ..
                } => Some((text.clone(), style.colour, style.size, *pos)),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            [
                ("ABAB".into(), GOAL_COLOURS[0], Some(16), [32.0, 14.0]),
                ("BABA".into(), GOAL_COLOURS[1], Some(14), [32.0, 36.0]),
                ("AABB".into(), GOAL_COLOURS[2], Some(14), [32.0, 56.0]),
            ]
        );
        // the rows' area: icons and texts (a generous text width: 40 characters of the open size)
        let rows = goal_rows(&l.top.mission, s.mission.shown());
        let bottom = rows.last().map(|r| r.1[1] + 16).unwrap() as f32;
        let area = [0.0, 0.0, 32.0 + 40.0 * 16.0 * 0.6, bottom];
        let c = &l.clock;
        let mut clock = [f32::MAX, f32::MAX, 0.0f32, 0.0f32];
        for p in [
            c.minutes[0],
            c.minutes[1],
            c.separator_pos,
            c.seconds[0],
            c.seconds[1],
        ] {
            clock[0] = clock[0].min(p[0] as f32);
            clock[1] = clock[1].min(p[1] as f32);
            clock[2] = clock[2].max((p[0] + c.size[0]) as f32);
            clock[3] = clock[3].max((p[1] + c.size[1]) as f32);
        }
        let gun_y = super::super::items::gun_rung_y(&l);
        let gun = [
            412.0,
            gun_y,
            612.0,
            gun_y + super::super::items::GUN_NEXT_STEP + 22.0,
        ];
        let gun_line = [362.0, 204.0, 662.0, 224.0];
        for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
            let a = on_target(area, w, h);
            for (name, r) in [("clock", clock), ("gun game", gun), ("gun line", gun_line)] {
                assert!(
                    apart(a, on_target(r, w, h)),
                    "{w}x{h}: rows {a:?} vs {name}"
                );
            }
        }
        // other classes: no rows
        let mut tdm = coop(1000);
        tdm.mode_class = Some(1);
        assert!(mission_items(&l, &tdm).is_empty());
    }

    /// Class 5 has no kill feed: a kill adds no line, so nothing is drawn where the rows are.
    #[test]
    fn the_coop_class_keeps_no_kill_feed_lines() {
        use dc_botclient::shell::KillRecord;
        let kill = KillRecord {
            killer: 1,
            victim: 2,
            ..KillRecord::default()
        };
        let mut s = coop(1000);
        s.on_kill(&kill, 1000);
        assert!(s.feed.is_empty());
        s.mode_class = Some(1);
        s.on_kill(&kill, 1000);
        assert_eq!(s.feed.len(), 1, "a team match keeps its feed");
    }

    /// The banner: back image scaled about its centre, the text centred on its place, faded in.
    #[test]
    fn the_banner_draws_its_back_and_centred_text() {
        let mut l = with_icons();
        l.top.mission.banner.back = Some(Img {
            tex: "t\\box.dtx".into(),
            pos: [300, 230],
            size: [400, 30],
        });
        let mut s = coop(0);
        s.on_mission(
            &MissionEvent::Banner {
                text: "AB".into(),
                secs: 3.0,
            },
            0,
        );
        s.now_ms = 100;
        let early = mission_items(&l, &s);
        let back = early.iter().find_map(|i| match i {
            DrawItem::Image { tex, dst, .. } if tex.contains("box") => Some(*dst),
            _ => None,
        });
        assert_eq!(
            back,
            Some([400.0, 242.0, 200.0, 6.0]),
            "half wide, a fifth high, centred"
        );
        s.now_ms = 2500;
        let held = mission_items(&l, &s);
        let label = held.iter().find_map(|i| match i {
            DrawItem::Label {
                text,
                anchor,
                style,
                ..
            } => Some((text.clone(), *anchor, style.colour[3])),
            _ => None,
        });
        assert_eq!(label, Some(("AB".into(), [0.5, 0.5], 255)));
        s.now_ms = 5000;
        assert!(
            !mission_items(&l, &s)
                .iter()
                .any(|i| matches!(i, DrawItem::Label { .. })),
            "over"
        );
    }

    /// The headless frame (CPU compositor, our own test font): the three goal rows show in their colours at both
    /// target sizes, inside the rows' area and clear of the round clock.
    #[test]
    fn a_headless_frame_shows_the_goals_in_their_colours() {
        use crate::testkit::synth_font;
        for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
            let mut hud = super::super::Hud::fallback(w, h, Some((synth_font(), 0)));
            let s = coop(1000);
            let img = hud.render(&s);
            let l = &hud.layout.top.mission;
            let rows = goal_rows(l, s.mission.shown());
            let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
            for (k, (_, at, _)) in rows.iter().enumerate() {
                let want = GOAL_COLOURS[k];
                let (x0, y0) = ((at[0] as f32 * sx) as u32, (at[1] as f32 * sy) as u32);
                let (x1, y1) = (
                    x0 + (200.0 * sx) as u32,
                    y0 + (l.sizes[k] as f32 * sy) as u32 + 2,
                );
                let mut hits = 0;
                for y in y0..y1.min(h) {
                    for x in x0..x1.min(w) {
                        let p = img.pixel(x, y);
                        if p[3] > 200 && p[..3] == want[..3] {
                            hits += 1;
                        }
                    }
                }
                assert!(hits > 4, "{w}x{h}: row {k} has {hits} pixels of its colour");
            }
            // nothing of the goal colours near the clock (top middle)
            for y in 0..(100.0 * sy) as u32 {
                for x in (440.0 * sx) as u32..(600.0 * sx) as u32 {
                    let p = img.pixel(x, y);
                    let goal = GOAL_COLOURS.iter().any(|c| p[3] > 0 && p[..3] == c[..3]);
                    assert!(!goal, "{w}x{h}: a goal pixel at ({x}, {y})");
                }
            }
        }
    }

    /// With the user's ROM: the co-op keys resolve (three icons, the icon size, the banner's back image and font), and
    /// a composed frame at 1024 x 768 and 2560 x 1440 shows the goal texts in their colours, their icons drawn, clear
    /// of the round clock's digits. Skipped without CA_ROM.
    #[test]
    fn rom_goal_list_resolves_and_renders_clear_of_the_clock() {
        let Ok(rom) = std::env::var("CA_ROM") else {
            println!("skipped: CA_ROM unset");
            return;
        };
        for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
            let mut hud = super::super::Hud::from_rom(std::path::Path::new(&rom), w, h, None)
                .expect("ROM HUD");
            let l = hud.layout.top.mission.clone();
            assert!(l.icons.iter().all(Option::is_some), "three state icons");
            assert!(l.icon_size[0] > 0 && l.icon_size[1] > 0);
            assert!(l.banner.back.is_some() && l.banner.size > 0);
            let s = coop(1000);
            let items = hud.items(&s);
            let icons = items
                .iter()
                .filter(|i| matches!(i, DrawItem::Image { tex, .. } if l.icons.iter().flatten().any(|t| t == tex)))
                .count();
            assert_eq!(icons, 3);
            let img = hud.render(&s);
            let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
            let rows = goal_rows(&l, s.mission.shown());
            let c = &hud.layout.clock;
            let clock_top = [c.minutes[0], c.separator_pos, c.seconds[1]]
                .iter()
                .map(|p| p[1])
                .min()
                .unwrap() as f32;
            let clock_left = c.minutes[0][0] as f32;
            let mut seen = [0usize; 3];
            for y in 0..h {
                for x in 0..w {
                    let p = img.pixel(x, y);
                    if p[3] < 200 {
                        continue;
                    }
                    if let Some(k) = GOAL_COLOURS.iter().position(|c| p[..3] == c[..3]) {
                        seen[k] += 1;
                        // a goal pixel left of the clock or above its top
                        assert!(
                            (x as f32) < clock_left * sx || (y as f32) < clock_top * sy,
                            "{w}x{h}: goal colour at ({x}, {y}) reaches the clock"
                        );
                    }
                }
            }
            for (k, n) in seen.iter().enumerate() {
                assert!(*n > 4, "{w}x{h}: row {k} ({:?}) drew {n} pixels", rows[k]);
            }
        }
    }

    /// A caption shows only with exactly three fields (one trailing `|` allowed); each sub 3 plays the message
    /// sound.
    #[test]
    fn a_caption_needs_three_fields() {
        assert_eq!(
            split_caption("p\\a.dtx|Ray|Go now"),
            Some(("p\\a.dtx".into(), "Ray".into(), "Go now".into()))
        );
        assert_eq!(
            split_caption("p|Ray|Go now|"),
            Some(("p".into(), "Ray".into(), "Go now".into()))
        );
        assert_eq!(
            split_caption("|||"),
            Some((String::new(), String::new(), String::new()))
        );
        for bad in ["Go now", "Ray|Go now", "p|Ray|Go|now", "p|Ray|Go now||"] {
            assert_eq!(split_caption(bad), None, "{bad}");
        }
        let mut h = HudState::default();
        h.on_mission(
            &MissionEvent::Caption {
                text: "Ray|Go now".into(),
                secs: 4.0,
            },
            0,
        );
        assert!(h.mission.caption.is_none(), "two fields: nothing shown");
        h.on_mission(
            &MissionEvent::Caption {
                text: "p|Ray|Go now".into(),
                secs: 4.0,
            },
            100,
        );
        let c = h.mission.caption.as_ref().unwrap();
        assert_eq!((c.speaker.as_str(), c.at, c.hold_ms), ("Ray", 100, 4000));
        assert_eq!(c.lines, ["Go now"]);
        assert_eq!(h.banner.voices, [Voice::CoopMessage, Voice::CoopMessage]);
    }

    /// The text is typed out letter by letter, held its seconds after the last letter, then fades over 1 s.
    #[test]
    fn a_caption_types_holds_and_fades() {
        let l = CAPTION_LETTER_MS;
        // five letters, 2 s hold: the last letter at 4 x 40 ms
        let p = |t| caption_phase(t, 5, 2000);
        assert_eq!(p(0), Some((1, 255.0)));
        assert_eq!(p(l - 1), Some((1, 255.0)));
        assert_eq!(p(l), Some((2, 255.0)));
        assert_eq!(p(4 * l), Some((5, 255.0)), "the last letter");
        assert_eq!(p(4 * l + 2000), Some((5, 255.0)), "the hold's last ms");
        let (n, a) = p(4 * l + 2500).unwrap();
        assert_eq!(n, 5);
        assert!((a - 127.5).abs() < 0.01, "half faded: {a}");
        assert!(p(4 * l + 2000 + CAPTION_FADE_MS).is_none(), "over");
        // the typed text crosses the wrapped lines in order
        let c = CaptionShow {
            picture: String::new(),
            speaker: "S".into(),
            lines: vec!["abc".into(), "de".into()],
            at: 0,
            hold_ms: 0,
        };
        assert_eq!(c.letters(), 5);
        assert_eq!(c.typed(2), ["ab"]);
        assert_eq!(c.typed(4), ["abc", "d"]);
        assert_eq!(c.typed(9), ["abc", "de"]);
    }

    #[test]
    fn a_caption_wraps_at_words_and_keeps_four_lines() {
        let w = "word ".repeat(13);
        let lines = wrap_caption(&w);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 64);
        let lines = wrap_caption(&"word ".repeat(14));
        assert_eq!(lines.len(), 2);
        assert!(lines
            .iter()
            .all(|l| l.chars().count() <= CAPTION_LINE_CHARS));
        let lines = wrap_caption(&"x".repeat(150));
        assert_eq!(
            lines.iter().map(String::len).collect::<Vec<_>>(),
            [64, 64, 22],
            "a long word is cut"
        );
        assert_eq!(wrap_caption(&"word ".repeat(200)).len(), CAPTION_LINES);
    }

    /// The caption's items: the box, the picture when it loads, the speaker and the typed text in the 2009 colours;
    /// a picture that does not load is left out; faded alpha after the hold.
    #[test]
    fn the_caption_draws_its_box_picture_speaker_and_typed_text() {
        let l = MissionLayout::fallback();
        let mut s = HudState {
            now_ms: 0,
            ..HudState::default()
        };
        s.on_mission(
            &MissionEvent::Caption {
                text: "t\\face.dtx|Ray|ABCD".into(),
                secs: 1.0,
            },
            0,
        );
        let items = |s: &HudState, loads: bool| {
            let mut out = Vec::new();
            let size = |t: &str| (loads && t == "t\\face.dtx").then_some([64.0, 32.0]);
            draw(&mut out, &l, s, &size);
            out
        };
        s.now_ms = CAPTION_LETTER_MS;
        let out = items(&s, true);
        assert!(matches!(out[0], DrawItem::Fill { colour, .. } if colour == CAPTION_BOX_COLOUR));
        assert!(matches!(
            &out[1],
            DrawItem::Image { tex, src, dst, .. } if tex == "t\\face.dtx" && *src == [64.0, 32.0] && *dst == CAPTION_PICTURE
        ));
        let texts: Vec<(String, [u8; 4])> = out
            .iter()
            .filter_map(|i| match i {
                DrawItem::Text { text, style, .. } => Some((text.clone(), style.colour)),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            [
                ("Ray".into(), CAPTION_SPEAKER_COLOUR),
                ("AB".into(), CAPTION_TEXT_COLOUR)
            ]
        );
        assert!(
            !items(&s, false)
                .iter()
                .any(|i| matches!(i, DrawItem::Image { .. })),
            "no picture when it does not load"
        );
        // the hold is over 1 s after the last letter: fading
        s.now_ms = 3 * CAPTION_LETTER_MS + 1000 + 500;
        let out = items(&s, true);
        let alpha = out.iter().find_map(|i| match i {
            DrawItem::Text { text, style, .. } if text == "ABCD" => Some(style.colour[3]),
            _ => None,
        });
        assert_eq!(alpha, Some(127));
        s.now_ms += 600;
        assert!(items(&s, true).is_empty(), "over");
    }

    /// The headless frame (CPU compositor, our own test font): a held caption shows its speaker and text colours
    /// inside its box at the bottom at 1024 x 768 and 2560 x 1440, and nothing of them above the box.
    #[test]
    fn a_headless_frame_shows_the_caption_in_its_box() {
        use crate::testkit::synth_font;
        for (w, h) in [(1024u32, 768u32), (2560, 1440)] {
            let mut hud = super::super::Hud::fallback(w, h, Some((synth_font(), 0)));
            let mut s = coop(0);
            s.on_mission(
                &MissionEvent::Caption {
                    text: "nothing.dtx|ABAB|BABA ABAB".into(),
                    secs: 5.0,
                },
                0,
            );
            s.now_ms = 2000;
            let img = hud.render(&s);
            let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
            let b = CAPTION_BOX;
            let inside = |x: u32, y: u32| {
                let (vx, vy) = (x as f32 / sx, y as f32 / sy);
                vx >= b[0] && vx < b[0] + b[2] && vy >= b[1] && vy < b[1] + b[3]
            };
            let mut seen = [0usize; 2];
            for y in 0..h {
                for x in 0..w {
                    let p = img.pixel(x, y);
                    if p[3] < 200 {
                        continue;
                    }
                    for (k, c) in [CAPTION_SPEAKER_COLOUR, CAPTION_TEXT_COLOUR]
                        .iter()
                        .enumerate()
                    {
                        if p[..3] == c[..3] {
                            assert!(inside(x, y), "{w}x{h}: caption colour at ({x}, {y})");
                            seen[k] += 1;
                        }
                    }
                }
            }
            assert!(seen.iter().all(|&n| n > 4), "{w}x{h}: {seen:?}");
            // the box darkens the bottom
            let p = img.pixel((b[0] * sx) as u32 + 2, ((b[1] + b[3]) * sy) as u32 - 2);
            assert_eq!(p[..3], [0, 0, 0]);
            assert!(p[3] >= 150, "{p:?}");
        }
    }

    #[test]
    fn wire_texts_fall_back_to_one_character_per_byte() {
        assert_eq!(wire_text(b"Rescue the pilot"), "Rescue the pilot");
        assert_eq!(wire_text(&[0x41, 0xe9]), "A\u{e9}");
    }
}

//! The HUD as a display list: [`build`] turns a [`HudLayout`] and a [`HudState`] into [`DrawItem`]s in the
//! virtual 1024 x 768 screen; the compositor ([`super::compose`]) rasterises the list. A [`HudDrawHook`] sees and
//! may edit the list before it is drawn (the seam for the `hud.draw` mod hook, HOSTING_AND_MODS.md §3.1).

use super::layout::{
    AimCrossLayout, Announce, Digits, HudLayout, Img, IntroLayout, StepGauge, TextStyle,
};
use super::marks::MarkState;
use super::state::{
    sector_angle, ChatKind, HudState, MkKind, Relation, ResultMark, DAMAGE_SECTORS,
};

/// One thing to draw. Positions and sizes are virtual pixels (1024 x 768).
#[derive(Debug, Clone, PartialEq)]
pub enum DrawItem {
    /// `src` texels (from the texture's top-left) stretched over `dst` (x, y, w, h), multiplied by `tint`, optionally
    /// turned by `angle` radians (clockwise on screen) about `pivot`.
    Image {
        tex: String,
        src: [f32; 2],
        dst: [f32; 4],
        tint: [u8; 4],
        turn: Option<([f32; 2], f32)>,
    },
    /// Text with its line top at `pos` (`right`: `pos` is the right end).
    Text {
        style: TextStyle,
        pos: [f32; 2],
        text: String,
        right: bool,
    },
    /// A filled rectangle.
    Fill { rect: [f32; 4], colour: [u8; 4] },
    /// The aim_mark, drawn in target pixels ([`crate::spread::aim_mark_rects`]).
    AimMark(crate::spread::AimMarkView),
    /// A mod's drawn crosshair (CLI-1633), vector shapes resolved on the target's pixels.
    Crosshair(super::crosshair::DrawnCrosshair),
    /// Text placed by an anchor: `pos` minus `anchor` x (text width, cell height) is the line's top-left.
    Label {
        style: TextStyle,
        pos: [f32; 2],
        text: String,
        anchor: [f32; 2],
    },
    /// The aim_cross: four 1-pixel bars about the target centre; `bar` and `half_gap` in virtual pixels (the
    /// compositor scales the gap and the horizontal bars by the vertical factor, the vertical bars by the
    /// horizontal factor, as the 2009 client's bars measure).
    AimCross {
        bar: f32,
        half_gap: f32,
        colour: [u8; 4],
    },
}

/// Edits the display list of a frame (mods replace or add elements here).
pub trait HudDrawHook {
    fn draw(&mut self, state: &HudState, layout: &HudLayout, items: &mut Vec<DrawItem>);

    /// [`HudDrawHook::draw`] with the target size and the named element spans of the list (`(name, items)` in list
    /// order, see [`build_marked`]); the default ignores both.
    fn draw_frame(
        &mut self,
        state: &HudState,
        layout: &HudLayout,
        _screen: (u32, u32),
        _spans: &[(&'static str, std::ops::Range<usize>)],
        items: &mut Vec<DrawItem>,
    ) {
        self.draw(state, layout, items)
    }
}

/// The screen centre the damage arcs turn about.
pub const VIRTUAL_CENTRE: [f32; 2] = [512.0, 384.0];

fn image(i: &Img, tint: [u8; 4]) -> DrawItem {
    DrawItem::Image {
        tex: i.tex.clone(),
        src: [i.size[0] as f32, i.size[1] as f32],
        dst: [
            i.pos[0] as f32,
            i.pos[1] as f32,
            i.size[0] as f32,
            i.size[1] as f32,
        ],
        tint,
        turn: None,
    }
}

const OPAQUE: [u8; 4] = [255, 255, 255, 255];

/// The three digits of a health or armour value; a value above 999 (the Spy Hunt agent's health, DSV-451) gets a
/// fourth digit left of the hundreds, one place step further left.
// EVIDENCE: cshell+0x96210 (the block's six digits: three for health, three for armour)
// DC-DEFAULT: CLI-1581 (proposed, project decision "the super spy did not have enough extra hp": the agent's
// 1000 health showed as 999) the thousands digit at `pos[0] - (pos[1] - pos[0])`, values above 9999 show 9999
fn digits(out: &mut Vec<DrawItem>, d: &Digits, value: u32) {
    let v = value.min(9999);
    if v > 999 {
        let step = [d.pos[1][0] - d.pos[0][0], d.pos[1][1] - d.pos[0][1]];
        out.push(image(
            &Img {
                tex: format!("{}{}.dtx", d.prefix, v / 1000),
                pos: [d.pos[0][0] - step[0], d.pos[0][1] - step[1]],
                size: d.size,
            },
            OPAQUE,
        ));
    }
    for (i, digit) in [(v / 100) % 10, (v / 10) % 10, v % 10].into_iter().enumerate() {
        out.push(image(
            &Img {
                tex: format!("{}{digit}.dtx", d.prefix),
                pos: d.pos[i],
                size: d.size,
            },
            OPAQUE,
        ));
    }
}

/// The FFA top block: title, my-score image, own kills and deaths as tens / ones digits with their words.
// EVIDENCE: cshell+0x8d470 (items), cshell+0x8dba0 / 0x8dd00 (digits `prefix + d + ".dtx"` of value / 10, value % 10)
// DC-DEFAULT: CLI-687 (proposed) the counts are the own board row's kills and deaths (the 2009 client set them from
// the roster record at each counted kill); above 99 they show 99
// DC-DEFAULT: CLI-1134 (proposed, playtest report "One Man Army 30 kills shows 15 in game") the title names the room
// goal of 0x7d once it is known (the 2009 goal setter's class-6 path), the creation-time 15-kill title before
fn ffa_top(out: &mut Vec<DrawItem>, f: &super::layout::FfaTop, s: &HudState) {
    if let Some(t) = &f.title {
        let tex = match s.goal {
            Some(goal) if !f.title_prefix.is_empty() => format!("{}{goal}kill.dtx", f.title_prefix),
            _ => t.tex.clone(),
        };
        out.push(image(
            &Img {
                tex,
                pos: t.pos,
                size: t.size,
            },
            OPAQUE,
        ));
    }
    if let Some(i) = &f.own_score {
        out.push(image(i, OPAQUE));
    }
    let own = s.own_pid.and_then(|p| s.roster.get(&p));
    let kills = own.map_or(0, |r| r.kills.max(0) as u32);
    let deaths = own.map_or(0, |r| r.deaths.max(0) as u32);
    for (places, word, v) in [
        (&f.kills, &f.kill_word, kills),
        (&f.deaths, &f.death_word, deaths),
    ] {
        if let Some(places) = places {
            let v = v.min(99);
            for (p, d) in places.iter().zip([v / 10, v % 10]) {
                out.push(image(
                    &Img {
                        tex: format!("{}{d}.dtx", f.digit_prefix),
                        pos: p.pos,
                        size: p.size,
                    },
                    OPAQUE,
                ));
            }
        }
        if let Some(w) = word {
            out.push(image(w, OPAQUE));
        }
    }
}

/// `(v + 5) - (v + 5) % 10` for v in 0..=100.
fn step10(v: f32) -> u32 {
    let v = v.clamp(0.0, 100.0) as u32 + 5;
    (v - v % 10).min(100)
}

fn gauge(out: &mut Vec<DrawItem>, g: &StepGauge, percent: f32) {
    out.push(image(
        &Img {
            tex: format!("{}{}.dtx", g.prefix, step10(percent)),
            pos: g.pos,
            size: g.size,
        },
        OPAQUE,
    ));
}

/// The red system banner of `GS_CHAT` and the open chat input (its images, the typed line, the blinking caret).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (banner); EVIDENCE: cshell+0xbd920 (banner text item: font record 4,
// size 0x12, at the element's place), cshell+0x729f0 (input draw order), cshell+0x72130 (input layout)
// PLAUSIBLE (not proven): the banner is centred on that place; the only alignment-like input is the trailing argument
// 4 handed to the text-item init cshell+0x2eafa0, whose meaning is not traced
// DC-DEFAULT: CLI-494 (proposed) the typed line and its caret are drawn after the input's images (the 2009 edit
// control draws first; its text batch order is not traced), the caret is a `|` after the line (the line is measured
// with the input font, `Hud::items` lists it among the measured texts), shown for the first
// half of each toggle period from the box's opening; the clan scope button and the edit box background are not drawn
fn chat_widgets(out: &mut Vec<DrawItem>, layout: &HudLayout, s: &HudState, measure: Measure<'_>) {
    let look = &layout.chat_look;
    if let Some((t, _)) = &s.chat_banner {
        out.push(DrawItem::Label {
            style: TextStyle {
                font: super::chat::BANNER_FONT,
                colour: super::chat::BANNER_COLOUR,
                size: Some(super::chat::BANNER_SIZE),
            },
            pos: [
                super::chat::BANNER_POS[0] as f32,
                super::chat::BANNER_POS[1] as f32,
            ],
            text: t.clone(),
            anchor: [0.5, 0.0],
        });
    }
    let Some(mode) = s.chat_input.mode() else {
        return;
    };
    let img = |i: &super::chat::InputImage, tex: String| {
        image(
            &Img {
                tex,
                pos: i.pos,
                size: i.size,
            },
            OPAQUE,
        )
    };
    for i in &look.images {
        out.push(img(i, i.tex.clone()));
    }
    for (b, m) in [
        (&look.all_button, super::chat::ChatScope::All),
        (&look.team_button, super::chat::ChatScope::Team),
    ] {
        if let Some(b) = b {
            out.push(img(b, look.mode_tex(&b.tex, m == mode)));
        }
    }
    let style = TextStyle {
        font: look.input_font,
        colour: look.input_colour,
        size: None,
    };
    let [x, y] = look.input_pos;
    let line = s.chat_input.text().to_string();
    let w = measure(look.input_font, &line);
    out.push(text(style, [x, y], line));
    let period = look.caret_ms.max(1);
    if (s.now_ms.saturating_sub(s.chat_input.opened_at) / period).is_multiple_of(2) {
        out.push(DrawItem::Text {
            style: TextStyle {
                colour: look.caret_colour,
                ..style
            },
            pos: [x as f32 + w, y as f32],
            text: "|".into(),
            right: false,
        });
    }
}

/// Gun Game's lines: "12 / 20" and the next gun under the round clock, the promotion or demotion line for a moment at
/// the system banner's height, in the system banner's font (no art of ours; no 2009 counterpart).
// DC-DEFAULT: CLI-1639 (proposed), placement CLI-1849 (proposed)
fn gun_game_lines(out: &mut Vec<DrawItem>, layout: &HudLayout, s: &HudState) {
    let Some(g) = &s.gun_game else {
        return;
    };
    let style = |colour: [u8; 4], size: i32| TextStyle {
        font: super::chat::BANNER_FONT,
        colour,
        size: Some(size),
    };
    let mut label = |st: TextStyle, y: f32, text: String| {
        out.push(DrawItem::Label {
            style: st,
            pos: [VIRTUAL_CENTRE[0], y],
            text,
            anchor: [0.5, 0.0],
        })
    };
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const SOFT: [u8; 4] = [210, 210, 210, 255];
    let total = g.names.len();
    let rung_y = gun_rung_y(layout);
    label(
        style(WHITE, 0x12),
        rung_y,
        format!("{} / {}", (g.rung + 1).min(total), total),
    );
    let next = match g.names.get(g.rung + 1) {
        Some(n) => format!("Next: {n}"),
        None => "Last gun".to_string(),
    };
    label(style(SOFT, 0xe), rung_y + GUN_NEXT_STEP, next);
    if let Some((t, knifed, at)) = &g.line {
        if s.now_ms.saturating_sub(*at) < super::state::GUN_LINE_MS {
            let colour = if *knifed {
                super::chat::BANNER_COLOUR
            } else {
                [255, 214, 110, 255]
            };
            label(style(colour, 0x12), GUN_LINE_Y, t.clone());
        }
    }
}

/// Virtual height of the rung line: a gap under the lowest edge of the round clock (its digit images, or its text
/// line in a layout without art), so the "12 / 20" and "Next:" lines never reach the timer at any target size (the
/// vertical scale is monotone, so an order kept in virtual pixels holds on every target).
// DC-DEFAULT: CLI-1849 (proposed) the rung line starts GUN_CLOCK_GAP virtual pixels under the clock
pub fn gun_rung_y(layout: &HudLayout) -> f32 {
    let c = &layout.clock;
    let images = [
        c.minutes[0],
        c.minutes[1],
        c.separator_pos,
        c.seconds[0],
        c.seconds[1],
    ]
    .iter()
    .map(|p| p[1] + c.size[1])
    .max()
    .unwrap_or(0);
    // a text clock (the fallback layout) is about one line tall
    let text = c.text.map_or(0, |(at, _)| at[1] + GUN_CLOCK_TEXT_H);
    (images.max(text) + GUN_CLOCK_GAP) as i32 as f32
}

/// Virtual pixels between the clock's bottom edge and the rung line; the rung line to the "Next:" line; a text clock's
/// height.
pub const GUN_CLOCK_GAP: i32 = 4;
pub const GUN_NEXT_STEP: f32 = 22.0;
const GUN_CLOCK_TEXT_H: i32 = 24;
/// Virtual height of the promotion / demotion line (above the system banner).
const GUN_LINE_Y: f32 = 204.0;

fn text(style: TextStyle, pos: [i32; 2], s: String) -> DrawItem {
    DrawItem::Text {
        style,
        pos: [pos[0] as f32, pos[1] as f32],
        text: s,
        right: false,
    }
}

/// Fills a printf form with one integer conversion.
pub fn fill_form(form: &str, n: u32) -> String {
    form.replacen("%d", &n.to_string(), 1)
}

/// Splits a client text into coloured runs: `^0x<argb>^<text>^` draws `<text>` in that colour, everything else
/// in `base`.
// EVIDENCE: cshell+0x2ebc10 (the text item finds "^0x", splits at "^", reads the colour with %x and keeps the rest)
pub fn markup_segments(text: &str, base: [u8; 4]) -> Vec<([u8; 4], String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("^0x") {
        if at > 0 {
            out.push((base, rest[..at].to_string()));
        }
        let tail = &rest[at + 1..];
        let Some(end_hex) = tail.find('^') else {
            rest = &rest[at..];
            break;
        };
        let argb = u32::from_str_radix(tail[2..end_hex].trim(), 16).unwrap_or(0xffff_ffff);
        let colour = [
            (argb >> 16) as u8,
            (argb >> 8) as u8,
            argb as u8,
            (argb >> 24) as u8,
        ];
        let body = &tail[end_hex + 1..];
        let (seg, next) = match body.find('^') {
            Some(e) => (&body[..e], &body[e + 1..]),
            None => (body, ""),
        };
        if !seg.is_empty() {
            out.push((colour, seg.to_string()));
        }
        rest = next;
    }
    if !rest.is_empty() {
        out.push((base, rest.to_string()));
    }
    out
}

/// Approximate text width for the layout pass (the compositor measures exactly; the builder only needs the pen
/// advance between the name and the icons of a feed line).
pub type Measure<'a> = &'a dyn Fn(i32, &str) -> f32;

/// Texture size lookup for images drawn at their own size (the intro text image).
pub type TexSize<'a> = &'a dyn Fn(&str) -> Option<[f32; 2]>;

/// Alpha 0..255 as a tint.
/// A colour blended toward white by `f` (0 = unchanged, 1 = white), alpha raised the same way.
pub fn flash_colour(c: [u8; 4], f: f32) -> [u8; 4] {
    if f <= 0.0 {
        return c;
    }
    let f = f.min(1.0);
    let mix = |v: u8| (v as f32 + (255.0 - v as f32) * f).round() as u8;
    [mix(c[0]), mix(c[1]), mix(c[2]), mix(c[3])]
}

fn alpha_tint(a: f32) -> [u8; 4] {
    [255, 255, 255, a.clamp(0.0, 255.0) as u8]
}

/// An image of `size` scaled by `scale` about its centre `c`.
fn centred(tex: &str, size: [f32; 2], c: [f32; 2], scale: f32, tint: [u8; 4]) -> DrawItem {
    let (w, h) = (size[0] * scale, size[1] * scale);
    DrawItem::Image {
        tex: tex.to_string(),
        src: size,
        dst: [c[0] - w * 0.5, c[1] - h * 0.5, w, h],
        tint,
        turn: None,
    }
}

/// An announcement `secs` after its start: (scale, alpha), `None` when not shown.
// DC-DEFAULT: CLI-76 (proposed) the scale falls from the start size to 1 over the creating time while the alpha
// rises; full until the end time; the alpha falls over the destroying time; gone at the total time (the 2009
// blow / last-size phases are not traced)
pub fn announce_phase(a: &Announce, secs: f32) -> Option<(f32, f32)> {
    if !(0.0..a.total).contains(&secs) {
        return None;
    }
    if secs < a.create && a.create > 0.0 {
        let f = secs / a.create;
        return Some((a.first_scale + (1.0 - a.first_scale) * f, 255.0 * f));
    }
    if secs < a.end {
        return Some((1.0, 255.0));
    }
    if a.destroy > 0.0 && secs < a.end + a.destroy {
        return Some((1.0, 255.0 * (1.0 - (secs - a.end) / a.destroy)));
    }
    None
}

pub(crate) fn announce(out: &mut Vec<DrawItem>, a: &Announce, start: Option<u32>, now: u32) {
    let Some(at) = start else {
        return;
    };
    let secs = now.saturating_sub(at) as f32 / 1000.0;
    if let Some((scale, alpha)) = announce_phase(a, secs) {
        out.push(centred(
            &a.tex,
            [a.size[0] as f32, a.size[1] as f32],
            [a.centre[0] as f32, a.centre[1] as f32],
            scale,
            alpha_tint(alpha),
        ));
    }
}

/// The intro banner `t` ms after its start: (title scale, text scale, alpha, title y shift, text y shift).
// EVIDENCE: cshell+0xbc7d0 (t = now - start - delay; t < 0: nothing changes (alpha 0); t <= fade-in: scales from
// the start scales to 1, alpha t / fade-in x 255; fade-in < t <= hold: scale 1, alpha 255; hold < t <= slide end:
// d = trunc(slide x u^2 / T^2) with u = t - hold, T = slide end - hold, title up by d, text down by d, alpha
// trunc(255 - 255 u / T); removed when t > duration)
// DC-DEFAULT: CLI-74 (proposed) both images scale about their centres (the scale origin of the image item is not traced)
pub fn intro_phase(l: &IntroLayout, since_ms: u32) -> Option<(f32, f32, f32, f32, f32)> {
    let t = since_ms as i64 - l.delay as i64;
    if t < 0 || t > l.duration as i64 {
        return None;
    }
    let t = t as f32;
    let (fade, hold, end) = (l.fade_in as f32, l.hold as f32, l.slide_end as f32);
    if t <= fade {
        let f = if fade > 0.0 { t / fade } else { 1.0 };
        return Some((
            l.head_scale + (1.0 - l.head_scale) * f,
            l.body_scale + (1.0 - l.body_scale) * f,
            (f * 255.0).trunc(),
            0.0,
            0.0,
        ));
    }
    if t <= hold {
        return Some((1.0, 1.0, 255.0, 0.0, 0.0));
    }
    if t <= end && end > hold {
        let (u, span) = (t - hold, end - hold);
        let d = (l.slide * u * u / (span * span)).trunc();
        let a = (255.0 - 255.0 * u / span).trunc();
        return Some((1.0, 1.0, a, -d, d));
    }
    None
}

/// The aim cross item (the compositor places it at the target centre in target pixels).
pub fn aim_cross_item(c: &AimCrossLayout) -> DrawItem {
    DrawItem::AimCross {
        bar: c.bar,
        half_gap: c.half_gap,
        colour: c.colour,
    }
}

/// The display list of one frame.
pub fn build(
    layout: &HudLayout,
    s: &HudState,
    measure: Measure<'_>,
    tex_size: TexSize<'_>,
) -> Vec<DrawItem> {
    build_marked(layout, s, measure, tex_size, &mut Vec::new())
}

/// [`build`], also noting where each named element starts in the list: `elems` gets `(name, first index)` in list
/// order; an element runs to the next mark (CLI-1614, the element names a `hud.draw` hook sees).
pub fn build_marked(
    layout: &HudLayout,
    s: &HudState,
    measure: Measure<'_>,
    tex_size: TexSize<'_>,
    elems: &mut Vec<(&'static str, usize)>,
) -> Vec<DrawItem> {
    let mut out = Vec::new();
    elems.push(("result_board", out.len()));
    // ---- after 0x3f the in-game HUD is gone: only the result board stays until the result screen opens
    // DISASM: cshell+0x1bc590 (the per-frame draw: interface state 1 draws the HUD through cshell+0x1b3190, state 5
    // only the current screen through cshell+0x1bb780), cshell+0x4ac5c (0x3f enters state 5 with screen 7)
    // DC-DEFAULT: CLI-80 (amended, proposed) the result board between 0x3f and the leave
    if s.result_screen.is_some() {
        result_screen(&mut out, layout, s);
        return out;
    }
    let alive = s.life_state == 1;
    // w2-board: the layer-1 elements (chat, name marks, respawn wait, clock, multi-kill, revenge) are hidden at render
    // level 0 (life state 5); see `super::death::NOTE_LAYER`
    let notes = super::death::layer_shown(s, super::death::NOTE_LAYER);
    let now = s.now_ms;
    // the layer-3 elements (aim cross with its damage arcs, health, ammunition) only at render level 3
    let live = super::death::live_widgets(s);
    elems.push(("damage_arcs", out.len()));
    // ---- damage arcs (under everything else)
    // DC-DEFAULT: CLI-47 (proposed) each sector draws the arc's `uv` texels at its layout place, turned about the
    // screen centre to the sector's centre angle, alpha = the sector value
    for k in 0..DAMAGE_SECTORS {
        let v = s.damage[k];
        if v <= 0.0 || !live {
            continue;
        }
        let d = &layout.damage;
        let a = (v.clamp(0.0, 1.0) * 255.0) as u8;
        let mut tint = d.colour;
        tint[3] = ((tint[3] as u32 * a as u32) / 255) as u8;
        let dst = [
            d.pos[0] as f32,
            d.pos[1] as f32,
            d.uv[0] as f32,
            d.uv[1] as f32,
        ];
        let turn = Some((VIRTUAL_CENTRE, sector_angle(k)));
        match &d.tex {
            Some(t) => out.push(DrawItem::Image {
                tex: t.clone(),
                src: [d.uv[0] as f32, d.uv[1] as f32],
                dst,
                tint,
                turn,
            }),
            None => out.push(DrawItem::Image {
                tex: String::new(),
                src: [1.0, 1.0],
                dst,
                tint,
                turn,
            }),
        }
    }
    elems.push(("player_names", out.len()));
    // ---- names over the players' heads (under the aim cross)
    // EVIDENCE: cshell+0x8a420 (the caption at its point), cshell+0x88da0 (caption text item: font record 3, size 14,
    // alignment flags 0x24, kept by cshell+0x2eafa0 / +0x2eb030 in the string at +0x0c), cshell+0x2ee7c0 (drawn x =
    // x - width / 2 with flag 0x04, x - width with 0x02), cshell+0x2ee810 (drawn y = y - height / 2 with flag 0x40,
    // y - height with 0x20), cshell+0x2ed230 (one line: width and height = the font's extent of the text): flags 0x24
    // = centred across, the line's BOTTOM on the point; DISASM cshell+0x8bae3..0x8bb5e (point = the projected socket
    // in target pixels / the HUD scale, set every frame), cshell+0x8bb78..0x8bc00 (rank icon at x - icon - half the
    // caption width, centred 3 pixels above the point)
    // marks are drawn before the aim cross
    let mk = &layout.marks;
    for m in s.marks.iter().filter(|_| notes) {
        let colour = match m.state {
            MarkState::Mate => mk.mate,
            MarkState::Enemy => {
                super::marks::mark_colour(mk, s.mark_hits.get(&m.obj).copied(), s.now_ms)
            }
            MarkState::Dead => mk.dead,
        };
        // client-hudgaps: the rank icon left of the caption (super::rank)
        if let Some(ri) = &layout.rank_icons {
            let rank = s.ranks.get(&m.pid).copied().unwrap_or(0);
            if let Some(tex) = ri.texture(rank, m.state == MarkState::Dead, &m.name) {
                let at = super::rank::icon_place(m.at, measure(mk.style.font, &m.name));
                let n = super::rank::ICON as f32;
                out.push(DrawItem::Image {
                    tex,
                    src: [n, n],
                    dst: [at[0], at[1], n, n],
                    tint: OPAQUE,
                    turn: None,
                });
            }
        }
        out.push(DrawItem::Label {
            style: TextStyle { colour, ..mk.style },
            pos: m.at,
            text: m.name.clone(),
            anchor: [0.5, 1.0],
        });
    }
    // CLI-1952: the team's place markers, in the names' element, in the mate colour and the system banner's font
    if notes {
        super::teamping::push_items(&mut out, &s.ping_marks, mk.mate, super::chat::BANNER_FONT);
    }
    // Fire Team: the AI health bars where the names stand and the own points popup, with the marks' layer
    // DC-DEFAULT: CLI-1871, CLI-1872 (proposed)
    if notes {
        super::npcbar::bar_items(&s.npc_bars, &mut out);
        super::npcbar::popup_items(s, &mut out);
    }
    elems.push(("crosshair", out.len()));
    // ---- aim_mark (the first element of the 2009 HUD list, drawn after the arcs of the same element)
    // SPEC: FMT_CRES_UI.md §8 (element 1, update slot 1); EVIDENCE: cshell+0x853f0 (draw slot calls cshell+0x845f0)
    // the aim element draws nothing while the scope picture (slot 0) is up, nor for a weapon whose row disables
    // the cross (the snipers: no static cross either)
    // DISASM: cshell+0x8544c..0x854b5 (overlay slot 0 filled: return before the marks), cshell+0x85520..0x855ad
    // (row DisableCrosshair: only the DotSightName picture); DC-DEFAULT: CLI-616
    // the row's aim picture (the knife's red X), centred on the layout's DotSightPos; drawn after the marks for a row
    // that has them, alone for a row that disables them
    // DISASM: cshell+0x85520..0x855ad, cshell+0x85682..0x856ed (DotSightName texture on the quad of cshell+0x83260);
    // DC-DEFAULT: CLI-630
    let picture = match (&s.aim_picture, &layout.dot_sight) {
        (Some(tex), Some((at, size))) if alive && !s.scope.scope_up() => Some(DrawItem::Image {
            tex: tex.clone(),
            src: [size[0] as f32, size[1] as f32],
            dst: [
                (at[0] - size[0] / 2) as f32,
                (at[1] - size[1] / 2) as f32,
                size[0] as f32,
                size[1] as f32,
            ],
            tint: OPAQUE,
            turn: None,
        }),
        _ => None,
    };
    // DC-DEFAULT: CLI-1103 (proposed, playtest report "cross-hair is showing in between rounds") no aim mark and no
    // static aim cross from a round result until the next round begins (the end camera holds the weapons there,
    // CLI-982, and the static cross took the mark's place)
    if alive && !s.aim_cross_hidden && !s.scope.scope_up() && !s.between_rounds {
        // the hit flash of a 0x74 turns the mark white and fades (cshell+0x835d0: colour white, alpha 255)
        // DC-DEFAULT: CLI-87 (proposed) the flash blends the mark's colour toward white by the flash strength
        let flash = s.hit_flash();
        if let Some(mut v) = s.aim_mark {
            // use-items-sweep: a reticle item's shape and colour (CLI-1878)
            // EVIDENCE: cshell+0x845f0 (the mark's colour is its +0x798 when set, else the weapon's)
            if let Some((shape, colour)) = s.reticle {
                v.shape = shape;
                if colour[3] != 0 {
                    v.colour = colour;
                }
            }
            v.colour = flash_colour(v.colour, flash);
            out.push(DrawItem::AimMark(v));
        } else if let (Some(c), None) = (&layout.aim_cross, s.result_screen) {
            // no weapon view: the static aim cross of the HUD layout (client-hud2 CLI-81)
            let mut item = aim_cross_item(c);
            if let DrawItem::AimCross { colour, .. } = &mut item {
                *colour = flash_colour(*colour, flash);
            }
            out.push(item);
        }
    }
    out.extend(picture);
    elems.push(("mate_panel", out.len()));
    // ---- the team-mate panel of the aim cross (a living mate under the cross, own player alive)
    if let (Some(mi), Some(t), true, None) =
        (&layout.mate_info, s.mate_target, alive, s.result_screen)
    {
        if let Some(p) = &mi.panel {
            out.push(image(p, OPAQUE));
        }
        if let Some((r, c)) = mi.fill {
            out.push(DrawItem::Fill {
                rect: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
                colour: c,
            });
        }
        if let (Some((st, at)), Some(hp)) = (mi.health, t.health) {
            // EVIDENCE: cshell+0x826c0 (`%d` of the health)
            out.push(text(st, at, hp.to_string()));
        }
    }
    elems.push(("health", out.len()));
    // ---- health and armour
    // DC-DEFAULT: CLI-1853 (proposed) a living player's block never shows 0
    let hp = s.shown_health();
    let ap = s.armour.unwrap_or(0.0).max(0.0);
    let hidden = super::layout::HealthLayout::default();
    let h = if live { &layout.health } else { &hidden };
    // client-hudgaps: the parts of the block from here to the armour digits take the low-health colour
    let vital_from = out.len();
    if let Some(i) = &h.stamina_back {
        out.push(image(i, OPAQUE));
    }
    if let Some(i) = &h.stamina_cells {
        // the gauge shows the sprint gauge's percent (cshell+0x95850); without a value it is drawn full (CLI-50)
        // DC-DEFAULT: CLI-590 (proposed) the bar's percent is the left part of the cells image (texels and screen
        // width alike); the bar control's own fill direction (cshell+0x441ba0) is not read
        match s.stamina_pct {
            None => out.push(image(i, OPAQUE)),
            Some(p) if p > 0 => {
                let f = p.min(100) as f32 / 100.0;
                out.push(DrawItem::Image {
                    tex: i.tex.clone(),
                    src: [i.size[0] as f32 * f, i.size[1] as f32],
                    dst: [
                        i.pos[0] as f32,
                        i.pos[1] as f32,
                        i.size[0] as f32 * f,
                        i.size[1] as f32,
                    ],
                    tint: OPAQUE,
                    turn: None,
                });
            }
            Some(_) => {}
        }
    }
    let stance = if s.crouched {
        &h.stance_crouch
    } else {
        &h.stance_stand
    };
    if let Some(i) = stance {
        out.push(image(i, OPAQUE));
    }
    if let Some(i) = &h.health_icon {
        out.push(image(i, OPAQUE));
    }
    if let Some(i) = &h.armour_icon {
        out.push(image(i, OPAQUE));
    }
    if let Some(d) = &h.health_digits {
        digits(&mut out, d, hp as u32);
    }
    if let Some(d) = &h.armour_digits {
        digits(&mut out, d, ap as u32);
    }
    let low_tint = super::vitals::tint(s, &layout.low_health);
    if let Some(c) = low_tint {
        super::vitals::apply(&mut out[vital_from..], c);
    }
    // DC-DEFAULT: CLI-56 (proposed) the two arcs beside the aim mark are drawn only while alive (state 1)
    // DC-DEFAULT: CLI-1581 (proposed) above a health maximum of 100 (the Spy Hunt agent) the arc shows the percent of
    // the maximum, so it falls as the agent loses health (cshell+0x95e80 takes the value as given)
    if let (Some(g), true, true) = (&h.health_arc, alive, layout.arcs) {
        let max = s.max_health.filter(|m| *m > 100.0).unwrap_or(100.0);
        gauge(&mut out, g, hp * 100.0 / max);
    }
    if let Some(b) = h.bars {
        for (r, c, v, max) in [
            (b.health, b.health_colour, hp, s.max_health.unwrap_or(100.0)),
            (b.armour, b.armour_colour, ap, s.max_armour.unwrap_or(100.0)),
        ] {
            let mut back = c;
            back[3] /= 3;
            let rect = [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32];
            out.push(DrawItem::Fill { rect, colour: back });
            let f = if max > 0.0 {
                (v / max).clamp(0.0, 1.0)
            } else {
                0.0
            };
            out.push(DrawItem::Fill {
                rect: [rect[0], rect[1], rect[2] * f, rect[3]],
                colour: c,
            });
        }
    }
    if let Some((hp_at, ap_at, st)) = h.text {
        let text_from = out.len();
        out.push(text(st, hp_at, format!("HP {:.0}", hp)));
        out.push(text(st, ap_at, format!("AP {:.0}", ap)));
        // DC-DEFAULT: CLI-280 (proposed) the no-ROM text form takes the low-health colour too
        if let Some(c) = low_tint {
            super::vitals::apply(&mut out[text_from..], c);
        }
    }
    elems.push(("ammo", out.len()));
    // ---- ammunition
    if let (Some(a), true) = (s.ammo, live) {
        let l = &layout.ammo;
        // the block's parts by the held row: `InfiniteAmmo` hides the clip and reserve digits, the slash and the
        // fire-mode image (the name image stays); the fire-mode image is set only for order classes 1 and 2, the
        // others get an empty texture
        // DISASM: cshell+0x6f1b0 (row +0x23c != 0: elements +0x28/+0x2c/+0x30, +0x38/+0x3c/+0x40, +0x34, +0x24
        // hidden, +0x20 shown; the fire-mode texture only when cshell+0x47da0 gives class 1 or 2), cshell+0x6ea90
        // (element order: +0x20 name, +0x24 fire mode, +0x28.. clip digits, +0x34 slash, +0x38.. reserve digits)
        // DC-DEFAULT: CLI-593 an unknown class (0: no weapon table) keeps the fire-mode image
        let counted = !a.infinite;
        let mode_shown = counted && matches!(a.order_class, 0..=2);
        if let (Some(d), true) = (&l.clip, counted) {
            digits(&mut out, d, a.clip);
        }
        if let (Some(i), true) = (&l.slash, counted) {
            out.push(image(i, OPAQUE));
        }
        if let (Some(d), true) = (&l.reserve, counted) {
            digits(&mut out, d, a.reserve);
        }
        let mode = if a.single_shot {
            &l.fire_single
        } else {
            &l.fire_auto
        };
        if let (Some(i), true) = (mode, mode_shown) {
            out.push(image(i, OPAQUE));
        }
        if let (Some(g), Some(size), true, true) = (&l.clip_arc, a.clip_size, alive, layout.arcs) {
            if size > 0 {
                gauge(&mut out, g, a.clip as f32 * 100.0 / size as f32);
            }
        }
        if let (Some((pos, size)), Some(tex)) = (l.name_image, layout.name_images.get(&a.weapon)) {
            out.push(image(
                &Img {
                    tex: tex.clone(),
                    pos,
                    size,
                },
                OPAQUE,
            ));
        }
        if let (Some((at, st)), true) = (l.text, counted) {
            out.push(text(st, at, format!("{} / {}", a.clip, a.reserve)));
        }
    }
    elems.push(("death_panel", out.len()));
    // ---- the death panel (element 15, after the health block in the 2009 list)
    let small: &[String] = layout
        .rank_icons
        .as_ref()
        .map_or(&[], |r| r.small.as_slice());
    super::death::items(&mut out, &layout.death, s, small);
    elems.push(("kill_feed", out.len()));
    // ---- kill feed
    // EVIDENCE: cshell+0x9d4a0 (line top = pos.y + step x line; killer name, then the weapon icon lifted by its
    // offset, the head-shot icon, then the victim name)
    let f = &layout.feed;
    for (i, line) in s.feed.iter().enumerate() {
        let y = (f.pos[1] + f.line_step * i as i32) as f32;
        let mut pen = f.pos[0] as f32;
        let colour = |r: Relation| match r {
            Relation::Mate => f.mate_colour,
            Relation::Opponent => f.opponent_colour,
        };
        let style = |r: Relation| TextStyle {
            font: f.font,
            colour: colour(r),
            size: None,
        };
        // client-hudgaps: a special form replaces the killer part (super::feedmarks)
        let marks = &layout.feed_marks;
        let special = super::feedmarks::form_image(&mut out, marks, line.marks.form, pen, y);
        if let Some(p) = special {
            pen = p;
        }
        if special.is_none() && !line.killer.is_empty() {
            out.push(DrawItem::Text {
                style: style(line.killer_rel),
                pos: [pen, y],
                text: line.killer.clone(),
                right: false,
            });
            pen += measure(f.font, &line.killer);
        }
        if special.is_none() {
            pen = super::feedmarks::payback(&mut out, marks, line, pen, y);
        }
        match layout
            .weapon_icons
            .get(&line.weapon)
            .filter(|_| special.is_none())
        {
            Some(icon) => out.push(image(
                &Img {
                    tex: icon.clone(),
                    pos: [pen as i32, y as i32 + f.weapon_lift],
                    size: f.weapon_size,
                },
                OPAQUE,
            )),
            None if special.is_some() => {}
            None => {
                // DC-DEFAULT: CLI-46 (proposed) a weapon without a kill icon shows its number in brackets
                let t = format!(" [{}] ", line.weapon);
                out.push(DrawItem::Text {
                    style: TextStyle {
                        font: f.font,
                        colour: [255, 255, 255, 255],
                        size: None,
                    },
                    pos: [pen, y],
                    text: t.clone(),
                    right: false,
                });
                pen += measure(f.font, &t) - f.weapon_size[0] as f32;
            }
        }
        if special.is_none() {
            pen += f.weapon_size[0] as f32;
        }
        if line.head_shot && special.is_none() {
            if let Some(icon) = &f.head_icon {
                out.push(image(
                    &Img {
                        tex: icon.clone(),
                        pos: [pen as i32, y as i32 + f.head_lift],
                        size: f.head_size,
                    },
                    OPAQUE,
                ));
            }
        }
        pen = super::feedmarks::after_weapon(&mut out, f, marks, line, pen, y);
        out.push(DrawItem::Text {
            style: style(line.victim_rel),
            pos: [pen, y],
            text: line.victim.clone(),
            right: false,
        });
    }
    elems.push(("coop_mission", out.len()));
    // ---- the Fire Team goal list and mission banner (super::mission)
    // DC-DEFAULT: CLI-1858 (proposed) drawn with the note layer (the 2009 element's layer is not traced)
    if notes {
        super::mission::draw(&mut out, &layout.top.mission, s, tex_size);
        // DC-DEFAULT: CLI-1881, CLI-1883 (proposed) the class-5 score and the drop-item notes, with the note layer
        super::coopscore::draw(&mut out, &layout.top.coop, s);
    }
    elems.push(("team_counters", out.len()));
    // ---- team counters (classes 1, 4, 7, 8, 10 show the 0xb5 pair; 2, 3, 12, 13 the round wins)
    // EVIDENCE: cshell+0x90a60 (+0x124 / +0x120 for classes 1, 4, 7, 8, 10; +0x138 / +0x13c for 2, 3, 12, 13)
    let pair = match s.mode_class {
        Some(2 | 3 | 12 | 13) => s.round_wins,
        _ => s.team_pair,
    };
    let sc = &layout.score;
    let top = &layout.top;
    // FFA (class 6): the team counters, team names and own-team bar are hidden; the FFA block shows the own kills and
    // deaths (CLI-687)
    // EVIDENCE: cshell+0x92570 (class 6 shows only the FFA block), cshell+0x2c430 0x6d case (class 6: the own kill
    // count to the kill digits, the own death count to the death digits)
    let ffa = s.mode_class == Some(6);
    if ffa {
        ffa_top(&mut out, &top.ffa, s);
    }
    // Spy Hunt (class 9): no team marks and counters either; its own element shows the intel counters (CLI-1082)
    // EVIDENCE: cshell+0x90a60 (the counters only for classes 1, 4, 7, 8, 10 and 2, 3, 12, 13), cshell+0x70760
    // Fire Team (class 5): no team marks or counters either; the co-op score stands there (CLI-1883)
    // EVIDENCE: cshell+0x9259d (cshell+0x91ee0(0) hides the block's parts), cshell+0x9272c..0x9277c (class 5 shows only
    // the mode picture, the score label and its six digits)
    let no_teams = ffa || matches!(s.mode_class, Some(5 | 9));
    // Quarantine (class 11): its element draws its own team marks (HumanTeam / InfectedTeam) where the Alpha / Bravo
    // marks sit; the counters and the own-team bar stay
    // DC-DEFAULT: CLI-1110 (proposed, playtest report "the human and infected logos seem messed up": both mark sets
    // were drawn over each other)
    let own_marks = no_teams || s.mode_class == Some(11);
    // EVIDENCE: cshell+0x90820 (own team 1 or 2: its name in the "on" image and its bar shown; class 6: neither)
    let own = s
        .own_pid
        .and_then(|p| s.roster.get(&p))
        .map(|r| r.team)
        .filter(|_| !no_teams);
    for (team, on, base) in [
        (1u8, &sc.team1_mark, &top.team1_base),
        (2u8, &sc.team2_mark, &top.team2_base),
    ]
    .into_iter()
    .filter(|_| !own_marks)
    {
        let m = if own == Some(team) || base.is_none() {
            on
        } else {
            base
        };
        if let Some(m) = m {
            out.push(image(m, OPAQUE));
        }
    }
    if let (Some(b), Some(t @ (1 | 2))) = (&top.own_bar, own) {
        out.push(image(
            &Img {
                tex: b.tex.clone(),
                pos: if t == 1 { b.team1 } else { b.team2 },
                size: b.size,
            },
            OPAQUE,
        ));
    }
    // TDM: the mode title and the kill goal (digits and word)
    // EVIDENCE: cshell+0x8ebe0 (mode title, goal digits and word items), cshell+0x916a0 (goal digit images)
    // DC-DEFAULT: CLI-75 (proposed) drawn for mode class 1 with a known goal; the titles of the other modes are not drawn
    if s.mode_class == Some(1) {
        if let Some(m) = &top.tdm_title {
            out.push(image(m, OPAQUE));
        }
        if let (Some(g), Some(goal)) = (&top.goal, s.goal) {
            // DC-DEFAULT: CLI-75 (proposed) each goal digit is the whole score-digit texture drawn into the goal digit
            // size (the 2009 client shows whole small digits; the item's scaling there is not traced)
            let v = goal.min(999);
            for (i, d) in [v / 100, (v / 10) % 10, v % 10].into_iter().enumerate() {
                let tex = format!("{}{d}.dtx", g.prefix);
                let src = tex_size(&tex).unwrap_or([g.size[0] as f32, g.size[1] as f32]);
                out.push(DrawItem::Image {
                    tex,
                    src,
                    dst: [
                        g.pos[i][0] as f32,
                        g.pos[i][1] as f32,
                        g.size[0] as f32,
                        g.size[1] as f32,
                    ],
                    tint: OPAQUE,
                    turn: None,
                });
            }
            if let Some(w) = &top.goal_word {
                out.push(image(w, OPAQUE));
            }
        }
    }
    // the round / objective modes' own top blocks (CLI-990)
    super::modetop::draw(&mut out, &top.round, s);
    super::modetop::draw_flag(&mut out, &top.flag, s);
    super::bomb::draw(&mut out, &top.bomb, s);
    super::qrt::draw(&mut out, &top.qrt, s);
    super::spy::draw(&mut out, &top.spy, s);
    super::teamchange::draw(&mut out, &top.team_change, s);
    super::prompt::draw(&mut out, &top.prompt, s, tex_size);
    if !no_teams {
        if let Some(d) = &sc.team1 {
            digits(&mut out, d, pair[0]);
        }
        if let Some(i) = &sc.separator {
            out.push(image(i, OPAQUE));
        }
        if let Some(d) = &sc.team2 {
            digits(&mut out, d, pair[1]);
        }
        if let Some((at, st)) = sc.text {
            out.push(text(st, at, format!("{} : {}", pair[0], pair[1])));
        }
    }
    elems.push(("round_clock", out.len()));
    // ---- round clock
    // EVIDENCE: cshell+0xbe570 (minutes = secs / 60; seconds = secs % 60, or secs itself at 60 s and below)
    // w2-board: the round clock is a layer-1 element (ctor cshell+0xbe4e0), hidden at render level 0 (life state 5)
    if notes && s.clock_left().is_some() {
        let c = &layout.clock;
        let (m, sec) = s.clock.shows(now);
        if !c.digit_form.is_empty() {
            let digit = |out: &mut Vec<DrawItem>, n: u32, pos: [i32; 2]| {
                out.push(image(
                    &Img {
                        tex: fill_form(&c.digit_form, n),
                        pos,
                        size: c.size,
                    },
                    c.tint,
                ))
            };
            digit(&mut out, (m / 10) % 10, c.minutes[0]);
            digit(&mut out, m % 10, c.minutes[1]);
            out.push(image(
                &Img {
                    tex: c.separator.clone(),
                    pos: c.separator_pos,
                    size: c.size,
                },
                c.tint,
            ));
            digit(&mut out, (sec / 10) % 10, c.seconds[0]);
            digit(&mut out, sec % 10, c.seconds[1]);
        }
        if let Some((at, st)) = c.text {
            out.push(text(st, at, format!("{m}:{:02}", sec % 60)));
        }
    }
    elems.push(("respawn", out.len()));
    // ---- respawn wait
    // w2-board: the respawn wait is a layer-1 element (ctor cshell+0xcd0c0). The gate is already implied by
    // `respawn_left` (only life states 2 / 3, render level 2); it stays to name the layer
    if let Some((left_ms, _)) = s
        .respawn_left()
        .filter(|_| super::death::layer_shown(s, super::death::NOTE_LAYER))
    {
        let r = &layout.respawn;
        if let Some(i) = &r.title {
            // the title's alpha swings once per gauge tick (cshell+0xcc520, `super::death::title_alpha`)
            out.push(image(i, super::death::respawn_title_tint(&layout.death, s)));
        }
        if let Some(i) = &r.bar {
            out.push(image(i, OPAQUE));
        }
        // the three-part gauge: left cap, middle bar, right cap (CLI-983; cshell+0xcc520)
        let width = |i: &Option<Img>| i.as_ref().map_or(0, |i| i.size[0]);
        let parts = super::death::gauge_parts(
            super::death::respawn_gauge_ticks(s),
            [width(&r.left), width(&r.fill), width(&r.right)],
        );
        if let Some(i) = r.left.as_ref().filter(|_| parts.left) {
            out.push(image(i, OPAQUE));
        }
        if let Some(i) = r.right.as_ref().filter(|_| parts.right) {
            out.push(image(i, OPAQUE));
        }
        if let Some(fill) = &r.fill {
            // DC-DEFAULT: CLI-532 (proposed) the middle bar fills left to right (amended by CLI-983: its share)
            let frac = parts.middle;
            let w = (fill.size[0] as f32 * frac).round();
            out.push(DrawItem::Image {
                tex: fill.tex.clone(),
                src: [w, fill.size[1] as f32],
                dst: [
                    fill.pos[0] as f32,
                    fill.pos[1] as f32,
                    w,
                    fill.size[1] as f32,
                ],
                tint: OPAQUE,
                turn: None,
            });
        }
        // DC-DEFAULT: CLI-51 (proposed) the number is the whole seconds left, rounded up, at most 9
        let secs = left_ms.div_ceil(1000);
        if let Some((form, pos, size)) = &r.number {
            out.push(image(
                &Img {
                    tex: fill_form(form, secs.min(9)),
                    pos: *pos,
                    size: *size,
                },
                OPAQUE,
            ));
        }
        if let Some(i) = &r.note {
            out.push(image(i, OPAQUE));
        }
        if let Some((at, st)) = r.text {
            out.push(text(st, at, format!("RESPAWN {secs}")));
        }
    }
    elems.push(("revenge", out.len()));
    // ---- the own death: the revenge panel (0x6d with a revenge block); the fallback layout also names the killer
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 14 (`hud_revenge` with the killer, the value and 7000 ms)
    // DISASM: cshell+0xbb670 (the killer's name with a full stop, the count line of res 0x566 with the value)
    // DC-DEFAULT: CLI-90 (proposed) the panel is one centred line at (512, 200) in the kill-feed font: the count line
    // then the name (the 2009 places, RevengeSystem keys of the layout, are not pinned yet)
    if let Some(r) = s.revenge_shown().filter(|_| notes) {
        let line = format!(
            "{}{}.",
            dc_ui_data::strings::format_message(
                &s.texts.revenge,
                &[dc_ui_data::FmtArg::UInt(r.count as u64)]
            ),
            r.killer
        );
        out.push(DrawItem::Label {
            style: TextStyle {
                font: layout.feed.font,
                colour: OPAQUE,
                size: None,
            },
            pos: [512.0, 200.0],
            text: line,
            anchor: [0.5, 0.0],
        });
    }
    if !layout.from_rom && !alive {
        if let (Some(d), Some((at, st))) = (&s.death, layout.respawn.text) {
            // DC-DEFAULT: CLI-44 (fallback layout, ours) the killer above the respawn line
            let who = d.killer.clone().unwrap_or_else(|| "(world)".into());
            out.push(text(st, [at[0], at[1] - 24], format!("KILLED BY {who}")));
        }
    }
    elems.push(("chat", out.len()));
    // ---- chat log
    let ch = &layout.chat;
    for (i, l) in s.chat.iter().enumerate().filter(|_| notes) {
        let base = match l.kind {
            ChatKind::Notice => ch.notice,
            // SPEC: HISEC_2009.md §3.2 (0xffffff00)
            ChatKind::Hisec => [255, 255, 0, 255],
            // SPEC: GAME_MESSAGES_2009_S2C.md §2.4 row 0x68 (team / friendly / enemy / moderator colour)
            ChatKind::Chat { class, .. } => layout.chat_look.colour(class),
            ChatKind::SameLine => layout.chat_look.same_line,
        };
        let mut pen = ch.pos[0] as f32;
        let y = (ch.pos[1] + ch.line_step * i as i32) as f32;
        for (colour, seg) in markup_segments(&l.text, base) {
            out.push(DrawItem::Text {
                style: TextStyle {
                    font: ch.font,
                    colour,
                    size: None,
                },
                pos: [pen, y],
                text: seg.clone(),
                right: false,
            });
            pen += measure(ch.font, &seg);
        }
    }
    chat_widgets(&mut out, layout, s, measure);
    elems.push(("gun_game", out.len()));
    if notes {
        gun_game_lines(&mut out, layout, s);
    }
    elems.push(("mk_notes", out.len()));
    // ---- multi-kill notes (newest at the place, older lines below)
    // DC-DEFAULT: CLI-78 (proposed) older notes one line step lower each, fully opaque for their life time (the
    // per-frame shift and fade of the 2009 notes are not traced)
    if let Some(mk) = layout.mk.as_ref().filter(|_| notes) {
        for (i, n) in s.mk.iter().rev().enumerate() {
            let y = (mk.pos[1] + mk.line_step * i as i32) as f32;
            let style = TextStyle {
                font: mk.font,
                colour: if n.mate { mk.mate } else { mk.enemy },
                size: None,
            };
            out.push(DrawItem::Text {
                style,
                pos: [mk.pos[0] as f32, y],
                text: n.name.clone(),
                right: false,
            });
            let tex = match n.kind {
                MkKind::Opener => &mk.opener_img,
                MkKind::Hisec => &mk.hisec,
                MkKind::Flag(l) => &mk.flag_imgs[l as usize],
            };
            if let Some(t) = tex {
                let x = mk.pos[0] as f32 + measure(mk.font, &n.name) + mk.gap as f32;
                out.push(image(
                    &Img {
                        tex: t.clone(),
                        pos: [x as i32, y as i32 + mk.lift],
                        size: mk.size,
                    },
                    OPAQUE,
                ));
            }
        }
    }
    // ---- the result mark of 0x46 is the scoreboard's header: drawn with the board below
    elems.push(("waiting_banner", out.len()));
    // ---- waiting banner (0x45 `waiting_banner`)
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 step 3 (drawn while the flag is set)
    if let (Some(w), true) = (&layout.waiting, s.waiting_banner) {
        out.push(image(w, OPAQUE));
    }
    elems.push(("intro", out.len()));
    // ---- intro banner (0x45 intro block)
    if let (Some(l), Some(show)) = (&layout.intro, &s.intro) {
        intro(&mut out, l, show, now, tex_size);
    }
    elems.push(("hisec_cases", out.len()));
    // ---- HiSec case indicators and the own pickup banner
    // SPEC: HISEC_2009.md §3.3 (indicator centred on the case's raised screen point, hidden behind the camera or for
    // another owner; the countdown above it while > 0, white, red at <= 5)
    if let Some((tex, size)) = &layout.hisec.indicator {
        for m in &s.case_marks {
            let (Some(p), true) = (m.at, s.case_visible(m.key)) else {
                continue;
            };
            let (w, h) = (size[0] as f32, size[1] as f32);
            out.push(DrawItem::Image {
                tex: tex.clone(),
                src: [w, h],
                dst: [p[0] - w * 0.5, p[1] - h * 0.5, w, h],
                tint: OPAQUE,
                turn: None,
            });
            if m.remaining > 0 {
                out.push(DrawItem::Label {
                    style: TextStyle {
                        font: 0xe,
                        colour: if m.remaining > 5 {
                            OPAQUE
                        } else {
                            [255, 0, 0, 255]
                        },
                        size: None,
                    },
                    pos: [p[0], p[1] - h * 0.5 - 2.0],
                    text: m.remaining.to_string(),
                    // DC-DEFAULT: CLI-79 (proposed) the number centred with its line bottom 2 pixels above the image
                    anchor: [0.5, 1.0],
                });
            }
        }
    }
    elems.push(("case_pickup", out.len()));
    if let Some(a) = &layout.hisec.pickup {
        announce(&mut out, a, s.case_banner_at, now);
    }
    elems.push(("opener_banner", out.len()));
    // ---- the own first-kill announcement
    if let Some(a) = layout.opener_banner.as_ref().filter(|_| notes) {
        announce(&mut out, a, s.combo_at, now);
    }
    elems.push(("scoreboard", out.len()));
    // ---- scoreboard (key held), and the result board 3000 ms after 0x46: the board window with the win / lose /
    // draw header; nothing of the board in the first 3000 ms after the result, key or not
    // EVIDENCE: cshell+0xc1a50 (0x46: header texture by the own team, shown flag, the result time), cshell+0xc02e0
    // (the board's draw waits 3000 ms after the result time), cshell+0xc2490 (the header is a child frame of the
    // board window at the result mark's place and size)
    if s.result_board_shown() || (s.show_board && !s.result_board_held()) {
        board(&mut out, layout, s);
    }
    if s.result_board_shown() {
        result_header(&mut out, layout, s.result_mark);
    }
    out
}

/// The result board's win / lose / draw header.
fn result_header(out: &mut Vec<DrawItem>, layout: &HudLayout, mark: Option<ResultMark>) {
    let rm = &layout.result_mark;
    let tex = match mark {
        Some(ResultMark::Win) => rm.win.as_ref(),
        Some(ResultMark::Lose) => rm.lose.as_ref(),
        Some(ResultMark::Draw) => rm.draw.as_ref(),
        Some(ResultMark::Score) => rm.score.as_ref(),
        Some(ResultMark::HumanWin) => rm.human_win.as_ref(),
        Some(ResultMark::InfectorWin) => rm.infector_win.as_ref(),
        None => None,
    };
    if let Some(t) = tex {
        // EVIDENCE: cshell+0xc2490 (the header is added as a child of the board window at the result mark's
        // key place, so its screen place is the window position plus the key; DISASM: the child add at the
        // window's vtable +0x138 with the 0x52180 pair)
        let o = board_origin(&layout.board);
        out.push(image(
            &Img {
                tex: t.clone(),
                pos: [o[0] + rm.pos[0], o[1] + rm.pos[1]],
                size: rm.size,
            },
            OPAQUE,
        ));
    }
}

fn intro(
    out: &mut Vec<DrawItem>,
    l: &IntroLayout,
    show: &super::state::IntroShow,
    now: u32,
    tex_size: TexSize<'_>,
) {
    let Some((hs, bs, alpha, dh, db)) = intro_phase(l, now.saturating_sub(show.at)) else {
        return;
    };
    let tint = alpha_tint(alpha);
    let hsz = [l.head_size[0] as f32, l.head_size[1] as f32];
    out.push(centred(
        &show.head,
        hsz,
        [
            l.head_pos[0] as f32 + hsz[0] * 0.5,
            l.head_pos[1] as f32 + hsz[1] * 0.5 + dh,
        ],
        hs,
        tint,
    ));
    if let Some(bsz) = tex_size(&show.body) {
        // EVIDENCE: cshell+0xbc038 (x = (1024 - w) >> 1, the texture's own size)
        let x = ((1024.0 - bsz[0]) / 2.0).floor();
        out.push(centred(
            &show.body,
            bsz,
            [x + bsz[0] * 0.5, l.body_y as f32 + bsz[1] * 0.5 + db],
            bs,
            tint,
        ));
    }
}

/// The result screen of 0x3f: the scoreboard window with its rows, no experience / GP gains.
// DC-DEFAULT: CLI-80 (proposed) the 2009 result screen is a menu page (screen 7, cshell+0x1375d0); the HUD shows the
// board window on the end camera until that page opens
// DC-DEFAULT: CLI-660 (proposed) the in-match board draws no gain texts (2009's board window has no gain column, see
// CLI-657); the 0x65 gains show only on the menu result page ([`crate::menu::result`])
// DC-DEFAULT: CLI-682 (proposed) with the last round's win / lose / draw header (the mark 0x3f reset, kept in
// [`super::state::ResultScreen::mark`])
fn result_screen(out: &mut Vec<DrawItem>, layout: &HudLayout, s: &HudState) {
    board(out, layout, s);
    result_header(out, layout, s.result_screen.and_then(|r| r.mark));
}

/// The scoreboard window: rows by team (team 1 left, team 2 one column offset to the right), highest kills first;
/// FFA / bounty / infection: one ranking list over both columns ([`super::board::board_slot`]).
// EVIDENCE: cshell+0xc2490 (window, row origin, row step, team column offset, row fields); roster sorted by kills
// (S2C 3.1 record +0x2c, cshell+0x29a50)
// DC-DEFAULT: CLI-53 (proposed) labels, row origin and fields are relative to the window position (row = window +
// row origin + step x row + field offset; the labels repeat in the team 2 column); no-team players are listed with
// team 1
// client-scoreboard: the 2009 row columns ([`super::board`]): place number right-aligned, level icon, name left,
// kills and deaths centred on their points, the own row under a translucent bar, at most 12 rows per team list
// EVIDENCE: cshell+0xc0ac0 (row widget), cshell+0xbf100 (place `%d`, flag 2), cshell+0x2ee7c0 (flag 4 centres the
// line on the point, flag 2 ends it there); DISASM: cshell+0xc3226..0xc32b8 (place = count + 1 per team list, 12
// rows), cshell+0xc34b7..0xc3533 (own bar)
/// The board window's screen position (the origin of its rows, labels and result header).
fn board_origin(b: &super::layout::BoardLayout) -> [i32; 2] {
    match (&b.window, b.panel) {
        (Some(w), _) => w.pos,
        (None, Some((r, _))) => [r[0], r[1]],
        (None, None) => [0, 0],
    }
}

fn board(out: &mut Vec<DrawItem>, layout: &HudLayout, s: &HudState) {
    let b = &layout.board;
    let cols = &layout.board_columns;
    let origin = board_origin(b);
    if let Some(w) = &b.window {
        out.push(image(w, OPAQUE));
    }
    if let Some((rect, colour)) = b.panel {
        out.push(DrawItem::Fill {
            rect: [
                rect[0] as f32,
                rect[1] as f32,
                rect[2] as f32,
                rect[3] as f32,
            ],
            colour,
        });
    }
    let style = |colour: [u8; 4]| TextStyle {
        font: b.font,
        colour,
        size: b.font_size,
    };
    // the header labels: the layout's header font (the row font when the keys are missing)
    // EVIDENCE: cshell+0xbf540 (header labels with the header font keys, flag 0 = left, top)
    let header = TextStyle {
        font: cols.header_font.unwrap_or(b.font),
        colour: b.alive,
        size: cols.header_font_size.or(b.font_size),
    };
    let ping_x = super::board::ping_column(b.kills, b.deaths, cols.clan_at);
    for team_col in 0..2 {
        let dx = origin[0] + b.team_offset * team_col;
        // w2-board: the 2009 order place, name, clan, kill, death, mission (cshell+0xbf540)
        for (s_label, pos) in [
            &cols.place_label,
            &cols.name_label,
            &cols.clan_label,
            &b.kill_label,
            &b.death_label,
            &cols.mission_label,
        ]
        .into_iter()
        .flatten()
        {
            out.push(text(
                header,
                [pos[0] + dx, pos[1] + origin[1]],
                s_label.clone(),
            ));
        }
        // DC-DEFAULT: CLI-1360 (proposed) the ping column's label, centred over the column at the kill label's
        // height (the 2009 board has no ping column; the label text is ours)
        if let Some((_, dpos)) = &b.kill_label {
            out.push(centred_text(
                header,
                [dx + b.rows[0] + ping_x[0], origin[1] + dpos[1]],
                super::board::PING_LABEL.to_string(),
            ));
        }
    }
    let small: &[String] = layout
        .rank_icons
        .as_ref()
        .map_or(&[], |r| r.small.as_slice());
    let mut rows: Vec<&super::state::BoardRow> = s.roster.values().collect();
    rows.sort_by(|a, b| b.kills.cmp(&a.kills).then(a.pid.cmp(&b.pid)));
    let mut n = [0i32; 2];
    let single = s
        .mode_class
        .is_some_and(|c| super::board::SINGLE_LIST_CLASSES.contains(&c));
    for r in rows {
        let Some((col, row, place)) = super::board::board_slot(single, r.team, &mut n) else {
            continue;
        };
        let x = origin[0] + b.rows[0] + b.team_offset * col as i32;
        let y = origin[1] + b.rows[1] + b.row_step * row;
        let own = Some(r.pid) == s.own_pid;
        let colour = if own {
            b.own
        } else if r.dead {
            b.dead
        } else {
            b.alive
        };
        if own {
            let [ix, iy] = super::board::OWN_BAR_INSET;
            let [w, h] = super::board::OWN_BAR_SIZE;
            out.push(DrawItem::Fill {
                rect: [(x + ix) as f32, (y + iy) as f32, w as f32, h as f32],
                colour: super::board::OWN_BAR_COLOUR,
            });
        }
        // the place number in the alive colour, its right end on the place point
        if let Some(p) = cols.place {
            out.push(DrawItem::Text {
                style: style(b.alive),
                pos: [(x + p[0]) as f32, (y + p[1]) as f32],
                text: place.to_string(),
                right: true,
            });
        }
        // w2-board: the level icon's list position is the fifth look byte (DC-DEFAULT: CLI-531 the 0x6b rank when
        // it is 0, see `super::board::level_index`)
        let mark = s.row_marks.get(&r.pid);
        let rank = s.ranks.get(&r.pid).copied().unwrap_or(0);
        let level = super::board::level_index(mark, rank);
        if let Some(icon) = super::board::row_icon(cols, small, level, &r.name, [x, y]) {
            out.push(image(&icon, OPAQUE));
        }
        let st = style(colour);
        // w2-board: the name in the record's colour word (not on a dead row), the clan name, the mission value
        // DC-DEFAULT: CLI-534 (new) the own row keeps the own colour when dead
        let dead = (!own && r.dead).then_some(b.dead);
        let name_st = style(super::board::name_colour(mark, colour, dead, s.mode_class));
        out.push(text(
            name_st,
            [x + b.name[0], y + b.name[1]],
            r.name.clone(),
        ));
        if let (Some(p), Some((clan, c))) = (
            cols.clan_at,
            super::board::clan_text(cols, mark, dead, b.alive),
        ) {
            out.push(text(style(c), [x + p[0], y + p[1]], clan));
        }
        // CLI-1910: the clan mark (background, then emblem) as the lobby panel draws it; nothing without a clan
        if let Some((bg, emblem, r)) =
            super::board::clan_mark_layers(cols, &layout.clan_marks, mark, [x, y])
        {
            crate::menu::clanpage::mark_cropped(bg, emblem, r, out);
        }
        if let Some(p) = cols.mission {
            out.push(centred_text(
                st,
                [x + p[0], y + p[1]],
                super::board::mission_text(mark, s.mode_class),
            ));
        }
        out.push(centred_text(
            st,
            [x + b.kills[0], y + b.kills[1]],
            r.kills.to_string(),
        ));
        out.push(centred_text(
            st,
            [x + b.deaths[0], y + b.deaths[1]],
            r.deaths.to_string(),
        ));
        // DC-DEFAULT: CLI-1360 (proposed) the last 0xa2 ping, centred like the counts; nothing before the first
        if let Some(p) = r.ping {
            out.push(centred_text(
                st,
                [x + ping_x[0], y + ping_x[1]],
                p.to_string(),
            ));
        }
    }
}

/// A line centred on `pos` horizontally, its top at `pos` (the text control's flag 4).
// EVIDENCE: cshell+0x2ee7c0 (x - width / 2), cshell+0x2ee860 (no vertical flag: top)
fn centred_text(style: TextStyle, pos: [i32; 2], s: String) -> DrawItem {
    DrawItem::Label {
        style,
        pos: [pos[0] as f32, pos[1] as f32],
        text: s,
        anchor: [0.5, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::board::{ClanTag, RowMark};
    use crate::hud::state::{AmmoView, BoardRow};

    fn texts(items: &[DrawItem]) -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn texs(items: &[DrawItem]) -> Vec<String> {
        items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, .. } => Some(tex.clone()),
                _ => None,
            })
            .collect()
    }

    /// CLI-589 / CLI-590: the stamina cells show the gauge's percent (the left part of the image), full without a
    /// value, nothing at 0.
    #[test]
    fn stamina_bar_follows_the_gauge() {
        let mut l = HudLayout::fallback();
        l.health.stamina_cells = Some(Img {
            tex: "cells.dtx".into(),
            pos: [10, 700],
            size: [80, 8],
        });
        let mut s = HudState {
            life_state: 1,
            ..HudState::default()
        };
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let cells = |s: &HudState| {
            build(&l, s, &measure, &|_| None)
                .into_iter()
                .find_map(|i| match i {
                    DrawItem::Image { tex, src, dst, .. } if tex == "cells.dtx" => Some((src, dst)),
                    _ => None,
                })
        };
        assert_eq!(
            cells(&s),
            Some(([80.0, 8.0], [10.0, 700.0, 80.0, 8.0])),
            "no value: full"
        );
        s.stamina_pct = Some(50);
        assert_eq!(cells(&s), Some(([40.0, 8.0], [10.0, 700.0, 40.0, 8.0])));
        s.stamina_pct = Some(0);
        assert_eq!(cells(&s), None, "empty gauge");
    }

    #[test]
    fn colour_markup_runs() {
        let y = [255, 255, 0, 255];
        assert_eq!(
            markup_segments("^0xFFFFFFFF^Bob^ took it.", y),
            vec![
                ([255, 255, 255, 255], "Bob".to_string()),
                (y, " took it.".to_string())
            ]
        );
        assert_eq!(markup_segments("plain", y), vec![(y, "plain".to_string())]);
        assert_eq!(
            markup_segments("a ^0xff00ff00^g^", y),
            vec![(y, "a ".to_string()), ([0, 255, 0, 255], "g".to_string())]
        );
    }

    #[test]
    fn digits_gauges_and_forms() {
        assert_eq!(step10(0.0), 0);
        assert_eq!(step10(4.9), 0);
        assert_eq!(step10(5.0), 10);
        assert_eq!(step10(94.0), 90);
        assert_eq!(step10(100.0), 100);
        assert_eq!(fill_form("a\\n_%d.dtx", 7), "a\\n_7.dtx");
        let mut out = Vec::new();
        let d = Digits {
            prefix: "p\\d_".into(),
            pos: [[0, 0], [10, 0], [20, 0]],
            size: [8, 8],
        };
        digits(&mut out, &d, 85);
        assert_eq!(texs(&out), ["p\\d_0.dtx", "p\\d_8.dtx", "p\\d_5.dtx"]);
    }

    /// CLI-1581 / DSV-451: the agent's statuses (0x0f / 2 = 1000, 0x10 / 3 = 500) reach the block unclamped: four
    /// health digits, three armour digits, the arc in percent of the maximum.
    #[test]
    fn the_agents_1000_health_and_500_armour_are_drawn() {
        let mut l = HudLayout::fallback();
        let d = |p: &str, y: i32| Digits {
            prefix: p.into(),
            pos: [[100, y], [110, y], [120, y]],
            size: [8, 8],
        };
        l.health.health_digits = Some(d("h_", 700));
        l.health.armour_digits = Some(d("a_", 720));
        l.health.health_arc = Some(StepGauge {
            prefix: "arc_".into(),
            pos: [0, 0],
            size: [8, 8],
        });
        l.arcs = true;
        let mut s = HudState {
            life_state: 1,
            ..HudState::default()
        };
        for (k, v) in [(0x0f, 1000.0), (0x10, 500.0), (2, 1000.0), (3, 500.0)] {
            s.on_status(k, v);
        }
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let drawn = |s: &HudState| -> Vec<(String, f32)> {
            build(&l, s, &measure, &|_| None)
                .into_iter()
                .filter_map(|i| match i {
                    DrawItem::Image { tex, dst, .. }
                        if tex.starts_with("h_") || tex.starts_with("a_") || tex.starts_with("arc_") =>
                    {
                        Some((tex, dst[0]))
                    }
                    _ => None,
                })
                .collect()
        };
        let got = drawn(&s);
        let names = |p: &str| -> Vec<String> {
            got.iter().filter(|(t, _)| t.starts_with(p)).map(|(t, _)| t.clone()).collect()
        };
        assert_eq!(names("h_"), ["h_1.dtx", "h_0.dtx", "h_0.dtx", "h_0.dtx"]);
        assert!(got.contains(&("h_1.dtx".to_string(), 90.0)), "thousands left of the hundreds: {got:?}");
        assert_eq!(names("a_"), ["a_5.dtx", "a_0.dtx", "a_0.dtx"]);
        assert_eq!(names("arc_"), ["arc_100.dtx"]);
        s.on_status(2, 500.0);
        assert!(drawn(&s).iter().any(|(t, _)| t == "arc_50.dtx"), "half the agent's maximum");
        // a plain player's 85 keeps the three places
        s.on_status(0x0f, 100.0);
        s.on_status(2, 85.0);
        let got = drawn(&s);
        assert_eq!(
            got.iter().filter(|(t, _)| t.starts_with("h_")).count(),
            3,
            "{got:?}"
        );
        assert!(got.iter().any(|(t, _)| t == "arc_90.dtx"));
    }

    #[test]
    fn the_knife_draws_no_ammunition_digits_slash_or_fire_mode() {
        // arms-fixes2 (user 2026-10-27: the knife showed 999 / 000 with the bullet icons): cshell+0x6f1b0
        let mut l = HudLayout::fallback();
        let img = |t: &str| Img {
            tex: t.into(),
            pos: [0, 0],
            size: [8, 8],
        };
        let dig = |p: &str| Digits {
            prefix: p.into(),
            pos: [[0, 0], [10, 0], [20, 0]],
            size: [8, 8],
        };
        l.ammo.clip = Some(dig("clip_"));
        l.ammo.reserve = Some(dig("res_"));
        l.ammo.slash = Some(img("slash.dtx"));
        l.ammo.fire_single = Some(img("single.dtx"));
        l.ammo.fire_auto = Some(img("auto.dtx"));
        let view = |weapon, clip, reserve, single_shot, order_class, infinite| AmmoView {
            weapon,
            clip,
            reserve,
            clip_size: Some(clip),
            single_shot,
            order_class,
            infinite,
        };
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let ammo_parts = |a: AmmoView| {
            let s = HudState {
                life_state: 1,
                health: Some(100.0),
                ammo: Some(a),
                now_ms: 5000,
                ..HudState::default()
            };
            let items = build(&l, &s, &measure, &|_| None);
            let mut v: Vec<String> = texs(&items)
                .into_iter()
                .filter(|t| {
                    t.starts_with("clip_")
                        || t.starts_with("res_")
                        || ["slash.dtx", "single.dtx", "auto.dtx"].contains(&t.as_str())
                })
                .collect();
            v.extend(texts(&items).into_iter().filter(|t| t.contains(" / ")));
            v
        };
        // the knife (row 0: InfiniteAmmo, order class 3): none of it
        assert_eq!(
            ammo_parts(view(0, 999, 0, false, 3, true)),
            Vec::<String>::new()
        );
        // the rifle 145 (class 1): digits, slash, the auto image and the text form
        let r = ammo_parts(view(145, 25, 90, false, 1, false));
        for want in [
            "clip_2.dtx",
            "clip_5.dtx",
            "res_9.dtx",
            "slash.dtx",
            "auto.dtx",
            "25 / 90",
        ] {
            assert!(r.iter().any(|x| x == want), "{want} in {r:?}");
        }
        // the pistol 164 (class 2): the single image
        assert!(ammo_parts(view(164, 15, 30, true, 2, false)).contains(&"single.dtx".to_string()));
        // a grenade (class 4): its count, no fire-mode image
        let g = ammo_parts(view(7, 1, 1, false, 4, false));
        assert!(
            g.contains(&"slash.dtx".to_string())
                && !g
                    .iter()
                    .any(|x| x.ends_with("single.dtx") || x == "auto.dtx"),
            "{g:?}"
        );
    }

    /// The Gun Game rung and next-gun lines never reach the round clock: at 1024 x 768, 1920 x 1080, 2560 x 1440 and
    /// 2304 x 1296 the first line's screen top (the truncated scaled place) is under the clock images' screen bottom,
    /// for the ROM-art clock and the text clock of the fallback layout.
    #[test]
    fn the_gun_game_lines_stay_under_the_round_clock() {
        use crate::hud::compose::scaled_place;
        use dc_ui_data::scale::VirtualScaler;
        for art in [false, true] {
            let mut l = HudLayout::fallback();
            if art {
                l.clock.digit_form = "d%d.dtx".into();
                l.clock.separator = "colon.dtx".into();
                l.clock.text = None;
            }
            let s = HudState {
                life_state: 1,
                gun_game: Some(crate::hud::state::GunGameHud::new(vec![
                    "A".into(),
                    "B".into(),
                ])),
                clock: crate::hud::state::RoundClock {
                    shown: true,
                    secs: 125,
                    at: 0,
                    running: true,
                },
                now_ms: 5000,
                ..HudState::default()
            };
            let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
            let items = build(&l, &s, &measure, &|_| None);
            let ys: Vec<f32> = items
                .iter()
                .filter_map(|i| match i {
                    DrawItem::Label { pos, text, .. }
                        if text.contains('/') || text.starts_with("Next") =>
                    {
                        Some(pos[1])
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(ys.len(), 2, "{ys:?}");
            // the clock's lowest edge in virtual pixels: the digit images (and the text clock's line)
            let clock_bottom = (l.clock.minutes[0][1] + l.clock.size[1]) as f32;
            assert!(ys[0] >= clock_bottom + GUN_CLOCK_GAP as f32, "{ys:?}");
            assert!(ys[1] > ys[0]);
            for (w, h) in [(1024u32, 768u32), (1920, 1080), (2560, 1440), (2304, 1296)] {
                let sc = VirtualScaler::new(w, h);
                let clock_screen_bottom = (clock_bottom * sc.sy).round();
                let text_top = scaled_place(ys[0], sc.sy);
                assert!(
                    text_top > clock_screen_bottom,
                    "{w}x{h}: text top {text_top} clock bottom {clock_screen_bottom}"
                );
            }
        }
    }

    #[test]
    fn fallback_layout_draws_every_element_as_text() {
        let l = HudLayout::fallback();
        let mut s = HudState {
            life_state: 1,
            health: Some(87.0),
            armour: Some(20.0),
            ammo: Some(AmmoView {
                weapon: 145,
                clip: 25,
                reserve: 90,
                clip_size: Some(30),
                single_shot: false,
                order_class: 1,
                infinite: false,
            }),
            own_pid: Some(0),
            team_pair: [3, 1],
            clock: crate::hud::state::RoundClock {
                shown: true,
                secs: 125,
                at: 0,
                running: true,
            },
            now_ms: 5000,
            show_board: true,
            ..HudState::default()
        };
        s.roster.insert(
            0,
            BoardRow {
                pid: 0,
                name: "me".into(),
                team: 1,
                kills: 2,
                ..BoardRow::default()
            },
        );
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let t = texts(&build(&l, &s, &measure, &|_| None));
        for want in [
            "HP 87", "AP 20", "25 / 90", "3 : 1", "2:00", "me", "2", "K", "D",
        ] {
            assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
        }
    }

    /// CLI-1870: the AI's mark carries a rank icon left of its caption as a player's does (rank 0: the AI's rank is
    /// the server's `npc_rank`, FT-1 default 0, not read by the client yet).
    #[test]
    fn an_ai_mark_has_a_rank_icon_and_a_caption() {
        use crate::hud::marks::{MarkState, PlayerMark};
        let mut l = HudLayout::fallback();
        l.rank_icons = Some(crate::hud::rank::RankIcons {
            small: vec!["r0.dtx".into()],
            rank_mark: vec!["0".into()],
            forms: ["%s_O.dtx".into(), "%s_G.dtx".into()],
        });
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let mut s = HudState {
            life_state: 1,
            ..HudState::default()
        };
        s.marks = vec![PlayerMark {
            pid: crate::net::NPC_PID,
            obj: 40,
            name: "Guard".into(),
            at: [500.0, 300.0],
            state: MarkState::Enemy,
        }];
        let items = build(&l, &s, &measure, &|_| None);
        assert!(items
            .iter()
            .any(|i| matches!(i, DrawItem::Image { tex, .. } if tex == "r0_O.dtx")));
        assert!(items
            .iter()
            .any(|i| matches!(i, DrawItem::Label { text, .. } if text == "Guard")));
    }

    #[test]
    fn marks_are_centred_labels_and_the_mate_panel_follows_the_target() {
        use crate::hud::marks::{MarkState, MateTarget, PlayerMark};
        let l = HudLayout::fallback();
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let mut s = HudState {
            life_state: 1,
            now_ms: 1000,
            ..HudState::default()
        };
        s.marks = vec![
            PlayerMark {
                pid: 1,
                obj: 10,
                name: "mate".into(),
                at: [500.0, 300.0],
                state: MarkState::Mate,
            },
            PlayerMark {
                pid: 2,
                obj: 11,
                name: "gone".into(),
                at: [700.0, 320.0],
                state: MarkState::Dead,
            },
        ];
        s.mate_target = Some(MateTarget {
            pid: 1,
            health: Some(87),
        });
        let items = build(&l, &s, &measure, &|_| None);
        let labels: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Label {
                    text,
                    pos,
                    anchor,
                    style,
                } => Some((text.as_str(), *pos, *anchor, style.colour)),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                ("mate", [500.0, 300.0], [0.5, 1.0], l.marks.mate),
                ("gone", [700.0, 320.0], [0.5, 1.0], l.marks.dead)
            ],
            "centred across, the line's bottom on the point (flags 0x24)"
        );
        let fill = l.mate_info.as_ref().unwrap().fill.unwrap().1;
        assert!(items
            .iter()
            .any(|i| matches!(i, DrawItem::Fill { colour, .. } if *colour == fill)));
        assert!(
            texts(&items).iter().any(|t| t == "87"),
            "the mate's health number"
        );
        // no panel while dead or without a mate under the cross
        s.mate_target = None;
        let items = build(&l, &s, &measure, &|_| None);
        assert!(!items
            .iter()
            .any(|i| matches!(i, DrawItem::Fill { colour, .. } if *colour == fill)));
    }

    /// CLI-1103: a round result hides the aim cross until the next round begins (0x44, or 0x45).
    #[test]
    fn no_aim_cross_between_rounds() {
        let l = HudLayout::fallback();
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let has_cross = |s: &HudState| {
            build(&l, s, &measure, &|_| None)
                .iter()
                .any(|i| matches!(i, DrawItem::AimCross { .. } | DrawItem::AimMark(_)))
        };
        let mut s = HudState {
            life_state: 1,
            now_ms: 1000,
            mode_class: Some(2),
            ..HudState::default()
        };
        assert!(has_cross(&s));
        s.on_round_result(5, 1, [1, 0], 1000);
        assert!(!has_cross(&s), "after the round result");
        s.on_round_begin(75, 9000);
        assert!(has_cross(&s), "the next round");
    }

    #[test]
    fn hit_flash_whitens_the_cross_and_the_dead_see_their_killer() {
        let l = HudLayout::fallback();
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let cross = |items: &[DrawItem]| {
            items.iter().find_map(|i| match i {
                DrawItem::AimCross { colour, .. } => Some(*colour),
                _ => None,
            })
        };
        let mut s = HudState {
            life_state: 1,
            now_ms: 1000,
            ..HudState::default()
        };
        let base = cross(&build(&l, &s, &measure, &|_| None)).expect("the aim cross");
        s.hit_flash_at = Some(1000);
        assert_eq!(
            cross(&build(&l, &s, &measure, &|_| None)),
            Some([255, 255, 255, 255]),
            "full flash"
        );
        s.now_ms = 1000 + crate::hud::state::HIT_FLASH_MS;
        assert_eq!(cross(&build(&l, &s, &measure, &|_| None)), Some(base));
        assert_eq!(flash_colour([0, 255, 0, 255], 0.5), [128, 255, 128, 255]);
        // dead: no cross; the fallback layout names the killer above the respawn line; the revenge panel line
        s.life_state = 2;
        s.mode_class = Some(1);
        s.death_at = Some(1000);
        s.respawn_wait_ms = Some(7000);
        s.texts.revenge = "%1 times by ".into();
        s.death = Some(crate::hud::state::DeathNote {
            at: 1000,
            killer: Some("foe".into()),
            killer_rel: Relation::Opponent,
            weapon: 145,
            head_shot: false,
        });
        s.revenge = Some(crate::hud::state::RevengePanel {
            at: 1000,
            killer: "foe".into(),
            count: 2,
        });
        let items = build(&l, &s, &measure, &|_| None);
        assert_eq!(cross(&items), None);
        let t = texts(&items);
        assert!(t.iter().any(|x| x == "KILLED BY foe"), "{t:?}");
        assert!(t.iter().any(|x| x.starts_with("RESPAWN")), "{t:?}");
        assert!(
            items
                .iter()
                .any(|i| matches!(i, DrawItem::Label { text, .. } if text == "2 times by foe.")),
            "revenge panel"
        );
    }

    /// CLI-1360: the 0xa2 ping left of the kills, centred like the counts, under a "Ping" label in both team columns;
    /// a row without a ping yet draws none, a table entry for an unknown id changes nothing.
    #[test]
    fn board_rows_draw_the_ping_column_from_the_ping_table() {
        let mut l = HudLayout::fallback();
        l.board.window = None;
        l.board.panel = None;
        l.board.rows = [10, 100];
        l.board.row_step = 30;
        l.board.team_offset = 400;
        l.board.kills = [250, 0];
        l.board.deaths = [300, 0];
        l.board.kill_label = Some(("K".into(), [255, 70]));
        l.board.death_label = Some(("D".into(), [305, 70]));
        l.board_columns = crate::hud::board::BoardColumns::default();
        let mut s = HudState {
            show_board: true,
            life_state: 1,
            ..HudState::default()
        };
        for (pid, kills) in [(0u8, 9), (1, 3)] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    name: format!("p{pid}"),
                    team: 1,
                    kills,
                    ..BoardRow::default()
                },
            );
        }
        s.apply(&crate::hud::state::HudEvent::Pings(vec![(0, 118), (7, 55), (300, 1)]), 10, 0.0);
        assert_eq!(s.roster[&0].ping, Some(118));
        assert_eq!(s.roster[&1].ping, None);
        assert!(!s.roster.contains_key(&7));
        let mut out = Vec::new();
        board(&mut out, &l, &s);
        let labels: Vec<(String, [f32; 2], [f32; 2])> = out
            .iter()
            .filter_map(|i| match i {
                DrawItem::Label {
                    text, pos, anchor, ..
                } => Some((text.clone(), *pos, *anchor)),
                _ => None,
            })
            .collect();
        let at = |t: &str| -> Vec<[f32; 2]> {
            labels
                .iter()
                .filter(|l| l.0 == t)
                .map(|l| {
                    assert_eq!(l.2, [0.5, 0.0], "{t} centred");
                    l.1
                })
                .collect()
        };
        // one count step (50) left of the kills: row x 10 + 200
        assert_eq!(at("118"), vec![[210.0, 100.0]]);
        assert_eq!(at("Ping"), vec![[210.0, 70.0], [610.0, 70.0]]);
        // the second row has no ping text: kills, deaths and nothing else centred in its row
        assert_eq!(labels.iter().filter(|l| l.1[1] == 130.0).count(), 2);
        // the ROM layout's points: one step left of the kills; a clan point keeps its name's room, never onto the kills
        use crate::hud::board::ping_column;
        assert_eq!(ping_column([343, 9], [384, 9], Some([211, 9])), [302, 9]);
        assert_eq!(ping_column([343, 9], [384, 9], None), [302, 9]);
        assert_eq!(ping_column([300, 0], [350, 0], Some([180, 0])), [260, 0]);
        assert_eq!(ping_column([300, 0], [350, 0], Some([240, 0])), [268, 0]);
    }

    #[test]
    fn board_rows_have_place_numbers_level_icons_centred_counts_and_the_own_bar() {
        // client-scoreboard: the 2009 row columns
        let mut l = HudLayout::fallback();
        l.board.window = None;
        l.board.panel = None;
        l.board.rows = [10, 100];
        l.board.row_step = 30;
        l.board.team_offset = 400;
        l.board.name = [60, 0];
        l.board.kills = [250, 0];
        l.board.deaths = [300, 0];
        l.board_columns = crate::hud::board::BoardColumns {
            place: Some([20, 2]),
            level: Some(([26, 0], [30, 22])),
            ..Default::default()
        };
        l.rank_icons = Some(crate::hud::rank::RankIcons {
            small: vec!["r0.dtx".into(), "r1.dtx".into()],
            ..Default::default()
        });
        let mut s = HudState {
            own_pid: Some(1),
            show_board: true,
            life_state: 1,
            ..HudState::default()
        };
        for pid in 0..15u8 {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    name: format!("p{pid}"),
                    team: if pid < 14 { 1 } else { 2 },
                    kills: i32::from(20 - pid),
                    ..BoardRow::default()
                },
            );
        }
        s.ranks.insert(1, 1);
        let mut out = Vec::new();
        board(&mut out, &l, &s);
        // place numbers: right-aligned on the place point, 1..12 in team 1, 1 in team 2
        let places: Vec<(String, [f32; 2])> = out
            .iter()
            .filter_map(|i| match i {
                DrawItem::Text {
                    text, pos, right, ..
                } if *right => Some((text.clone(), *pos)),
                _ => None,
            })
            .collect();
        assert_eq!(
            places.len(),
            13,
            "12 rows in team 1, 1 in team 2: {places:?}"
        );
        assert_eq!(places[0], ("1".to_string(), [30.0, 102.0]));
        assert_eq!(places[11], ("12".to_string(), [30.0, 102.0 + 11.0 * 30.0]));
        assert_eq!(places[12], ("1".to_string(), [430.0, 102.0]));
        // the level icon by the rank: own row rank 1, the others rank 0
        let icons = texs(&out);
        assert_eq!(icons.len(), 13);
        assert_eq!(icons[1], "r1.dtx");
        assert!(icons.iter().filter(|t| *t == "r0.dtx").count() == 12);
        // kills / deaths centred on their points
        let p0_kills = out.iter().find_map(|i| match i {
            DrawItem::Label {
                text, pos, anchor, ..
            } if text == "20" => Some((*pos, *anchor)),
            _ => None,
        });
        assert_eq!(p0_kills, Some(([260.0, 100.0], [0.5, 0.0])));
        // the own row (second in team 1) under the translucent bar
        let bar = out.iter().find_map(|i| match i {
            DrawItem::Fill { rect, colour } => Some((*rect, *colour)),
            _ => None,
        });
        assert_eq!(
            bar,
            Some(([8.0, 131.0, 286.0, 28.0], [255, 255, 255, 0x4c]))
        );
        assert!(
            !texts(&out).iter().any(|t| t == "p13"),
            "row 13 of team 1 is not drawn"
        );
    }

    fn row_mark(look_level: u32, name_colour: u32, clan: Option<(&str, u64, u32)>) -> RowMark {
        RowMark {
            look_level,
            name_colour,
            clan: clan.map(|(n, g, c)| ClanTag {
                name: n.into(),
                group_no: g,
                colour: c,
                mark: [0; 2],
            }),
            mission: [3, 4, 5],
        }
    }

    fn text_at(items: &[DrawItem], want: &str) -> Option<([f32; 2], [u8; 4])> {
        items.iter().find_map(|i| match i {
            DrawItem::Text {
                text, pos, style, ..
            }
            | DrawItem::Label {
                text, pos, style, ..
            } if text == want => Some((*pos, style.colour)),
            _ => None,
        })
    }

    #[test]
    fn board_rows_draw_the_clan_name_colour_mission_and_look_level() {
        // w2-board: the 2009 row's clan text, name colour word, mission value, header labels and level index
        let mut l = HudLayout::fallback();
        l.board.window = None;
        l.board.panel = None;
        l.board.rows = [10, 100];
        l.board.row_step = 30;
        l.board.team_offset = 400;
        l.board.name = [60, 0];
        l.board.kills = [250, 0];
        l.board.deaths = [300, 0];
        l.board.alive = [200, 200, 200, 255];
        l.board.dead = [90, 90, 90, 255];
        l.board.own = [255, 153, 0, 255];
        l.board_columns = crate::hud::board::BoardColumns {
            level: Some(([26, 0], [30, 22])),
            clan_at: Some([140, 2]),
            clan_colour: Some([10, 20, 30, 255]),
            mission: Some([350, 0]),
            clan_label: Some(("CLAN".into(), [140, 70])),
            mission_label: Some(("MISSION".into(), [350, 70])),
            ..Default::default()
        };
        l.rank_icons = Some(crate::hud::rank::RankIcons {
            small: ["r0.dtx", "r1.dtx", "r2.dtx", "r3.dtx"]
                .map(String::from)
                .to_vec(),
            ..Default::default()
        });
        let mut s = HudState {
            own_pid: Some(9),
            show_board: true,
            life_state: 1,
            mode_class: Some(1),
            ..HudState::default()
        };
        for (pid, name, kills, dead) in [
            (1u8, "ace", 9, false),
            (2, "bob", 5, true),
            (3, "cid", 1, false),
        ] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    name: name.into(),
                    team: 1,
                    kills,
                    dead,
                    ..BoardRow::default()
                },
            );
        }
        // ace: clan with its own colour, a name colour word, look level 3; bob: dead, clan without a colour;
        // cid: no clan, no look level but the rank 2
        s.row_marks
            .insert(1, row_mark(3, 0xff00ff00, Some(("Owls", 7, 0xffff0000))));
        s.row_marks
            .insert(2, row_mark(0, 0xff0000ff, Some(("Owls", 7, 0))));
        s.row_marks.insert(3, row_mark(0, 0, None));
        s.ranks.insert(3, 2);
        let mut out = Vec::new();
        board(&mut out, &l, &s);
        // header labels in both team columns
        assert_eq!(text_at(&out, "CLAN").map(|t| t.0), Some([140.0, 70.0]));
        assert_eq!(text_at(&out, "MISSION").map(|t| t.0), Some([350.0, 70.0]));
        // clan text: row-relative top-left, the record's colour word; on a dead row the dead colour
        let owls: Vec<([f32; 2], [u8; 4])> = out
            .iter()
            .filter_map(|i| match i {
                DrawItem::Text {
                    text, pos, style, ..
                } if text == "Owls" => Some((*pos, style.colour)),
                _ => None,
            })
            .collect();
        assert_eq!(
            owls,
            vec![
                ([150.0, 102.0], [255, 0, 0, 255]),
                ([150.0, 132.0], [90, 90, 90, 255])
            ]
        );
        // name colours: the word for a living row, the dead colour for a dead row, the row colour without a word
        assert_eq!(text_at(&out, "ace").map(|t| t.1), Some([0, 255, 0, 255]));
        assert_eq!(text_at(&out, "bob").map(|t| t.1), Some([90, 90, 90, 255]));
        assert_eq!(
            text_at(&out, "cid").map(|t| t.1),
            Some([200, 200, 200, 255])
        );
        // mission value v64 centred on its point (one per row)
        let missions = out
            .iter()
            .filter(|i| matches!(i, DrawItem::Label { text, anchor, .. } if text == "3" && *anchor == [0.5, 0.0]))
            .count();
        assert_eq!(missions, 3);
        // level icons: ace by its look level 3, bob rank 0, cid by the rank 2
        assert_eq!(texs(&out), ["r3.dtx", "r0.dtx", "r2.dtx"]);
        // mode class 11 keeps the row colour for the name; classes 7 / 8 show v68/v6c
        s.mode_class = Some(11);
        let mut out = Vec::new();
        board(&mut out, &l, &s);
        assert_eq!(
            text_at(&out, "ace").map(|t| t.1),
            Some([200, 200, 200, 255])
        );
        s.mode_class = Some(7);
        let mut out = Vec::new();
        board(&mut out, &l, &s);
        assert!(text_at(&out, "4/5").is_some());
    }

    #[test]
    fn board_rows_draw_the_clan_mark_of_clan_members_only() {
        // CLI-1910 (playtest 2026-11-05: no clan marks on the scoreboard): background then emblem, the art's 51 x 51
        // texels over 21 x 21 at the row's clan mark point; nothing for a clanless row or a mark of no table entry
        let mut l = HudLayout::fallback();
        l.board.window = None;
        l.board.panel = None;
        l.board.rows = [10, 100];
        l.board.row_step = 30;
        l.board_columns = crate::hud::board::BoardColumns {
            clan_mark_at: Some([110, 1]),
            ..Default::default()
        };
        l.clan_marks = crate::menu::clan::ClanMarks {
            emblems: vec![None, Some("e1.dtx".into()), Some("e2.dtx".into())],
            backgrounds: vec![None, Some("b1.dtx".into())],
            ..Default::default()
        };
        let mut s = HudState {
            show_board: true,
            life_state: 1,
            mode_class: Some(1),
            ..HudState::default()
        };
        for (pid, kills) in [(1u8, 9), (2, 5), (3, 1), (4, 0)] {
            s.roster.insert(
                pid,
                BoardRow {
                    pid,
                    name: format!("p{pid}"),
                    team: 1,
                    kills,
                    ..BoardRow::default()
                },
            );
        }
        let clan = |mark: [u32; 2]| RowMark {
            clan: Some(ClanTag {
                name: "Owls".into(),
                group_no: 7,
                colour: 0,
                mark,
            }),
            ..RowMark::default()
        };
        s.row_marks.insert(1, clan([2, 1]));
        s.row_marks.insert(2, RowMark::default());
        s.row_marks.insert(3, clan([0, 9]));
        s.row_marks.insert(4, clan([1, 0]));
        let mut out = Vec::new();
        board(&mut out, &l, &s);
        let marks: Vec<(&str, [f32; 2], [f32; 4])> = out
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, src, dst, .. } => Some((tex.as_str(), *src, *dst)),
                _ => None,
            })
            .collect();
        assert_eq!(
            marks,
            vec![
                ("b1.dtx", [51.0, 51.0], [120.0, 101.0, 21.0, 21.0]),
                ("e2.dtx", [51.0, 51.0], [120.0, 101.0, 21.0, 21.0]),
                // row 4 (place 4): only the emblem layer resolves, it shows alone (cshell+0x343cc0)
                ("e1.dtx", [51.0, 51.0], [120.0, 191.0, 21.0, 21.0]),
            ]
        );
        // without the layout point: no mark
        l.board_columns.clan_mark_at = None;
        let mut out = Vec::new();
        board(&mut out, &l, &s);
        assert!(!out.iter().any(|i| matches!(i, DrawItem::Image { .. })));
    }

    #[test]
    fn the_end_camera_result_board_keeps_the_win_or_lose_header() {
        // CLI-682 (playtest report): 0x3f reset the mark, so the board on the end camera (0x3f .. the leave) drew no
        // win / lose header; it is drawn at the board window + ResultMarkPos
        let mut l = HudLayout::fallback();
        l.board.window = Some(Img {
            tex: "window".into(),
            pos: [15, 149],
            size: [994, 470],
        });
        l.result_mark.pos = [332, 11];
        l.result_mark.size = [360, 64];
        l.result_mark.win = Some("win".into());
        l.result_mark.lose = Some("lose".into());
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let at = [347.0, 160.0, 360.0, 64.0];
        for (mark, tex) in [(ResultMark::Lose, "lose"), (ResultMark::Win, "win")] {
            let mut s = HudState {
                result_mark: Some(mark),
                result_mark_at: Some(1000),
                now_ms: 7000,
                ..HudState::default()
            };
            s.on_result_screen(false, 4, 7000);
            let out = build(&l, &s, &measure, &|_| None);
            assert!(
                out.iter().any(|i| matches!(i,
                    DrawItem::Image { tex: t, dst, .. } if t == tex && *dst == at)),
                "{tex} header at window + key: {out:?}"
            );
        }
    }

    #[test]
    fn the_hud_result_board_draws_no_gain_texts() {
        // CLI-660: the in-match result board drew the 0x65 exp / gp gains (CLI-80 / CLI-657); they belong to the
        // menu result page only
        let mut l = HudLayout::fallback();
        l.board.window = None;
        l.board.panel = None;
        l.board.rows = [10, 100];
        l.board.row_step = 30;
        l.board.team_offset = 400;
        l.board.kills = [250, 0];
        l.board.deaths = [300, 0];
        l.board_columns.mission = Some([350, 0]);
        let mut s = HudState::default();
        s.roster.insert(
            1,
            BoardRow {
                pid: 1,
                team: 1,
                kills: 7,
                ..BoardRow::default()
            },
        );
        s.summary.insert(
            1,
            dc_botclient::shell::SummaryRow {
                exp_gain: 120,
                gp_gain: 340,
                ..Default::default()
            },
        );
        let mut out = Vec::new();
        result_screen(&mut out, &l, &s);
        assert!(text_at(&out, "+120").is_none() && text_at(&out, "+340").is_none());
        assert!(text_at(&out, "120").is_none() && text_at(&out, "340").is_none());
        // the board rows themselves are still drawn
        assert!(text_at(&out, "7").is_some());
    }

    /// CLI-687: mode class 6 draws the FFA block (own kills / deaths) and no team names, counters or score text.
    #[test]
    fn ffa_top_shows_own_kills_and_deaths_not_the_team_counters() {
        let mut l = HudLayout::fallback();
        let im = |t: &str| Img {
            tex: t.into(),
            pos: [0, 0],
            size: [10, 10],
        };
        l.top.team1_base = Some(im("alpha.dtx"));
        l.top.team2_base = Some(im("bravo.dtx"));
        l.score.team1 = Some(Digits {
            prefix: "team".into(),
            pos: [[0, 0]; 3],
            size: [8, 8],
        });
        l.top.ffa = crate::hud::layout::FfaTop {
            title: Some(im("title.dtx")),
            own_score: Some(im("mine.dtx")),
            kills: Some([im(""), im("")]),
            kill_word: Some(im("kill.dtx")),
            deaths: Some([im(""), im("")]),
            death_word: Some(im("death.dtx")),
            digit_prefix: "sd".into(),
            title_prefix: "ffa_".into(),
        };
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let mut s = HudState {
            life_state: 1,
            own_pid: Some(0),
            mode_class: Some(6),
            ..HudState::default()
        };
        s.roster.insert(
            0,
            BoardRow {
                pid: 0,
                team: 2,
                kills: 12,
                deaths: 3,
                ..BoardRow::default()
            },
        );
        let out = build(&l, &s, &measure, &|_| None);
        let t = texs(&out);
        for want in [
            "title.dtx",
            "mine.dtx",
            "sd1.dtx",
            "sd2.dtx",
            "sd0.dtx",
            "sd3.dtx",
            "kill.dtx",
            "death.dtx",
        ] {
            assert!(t.iter().any(|x| x == want), "{want} {t:?}");
        }
        assert!(
            !t.iter()
                .any(|x| x == "alpha.dtx" || x == "bravo.dtx" || x.starts_with("team")),
            "{t:?}"
        );
        assert!(!texts(&out).iter().any(|x| x.contains(" : ")));
        // CLI-1134: with the room goal of 0x7d the title names it (a 30-kill room drew the 15-kill title)
        s.goal = Some(30);
        let t = texs(&build(&l, &s, &measure, &|_| None));
        assert!(t.iter().any(|x| x == "ffa_30kill.dtx"), "{t:?}");
        assert!(!t.iter().any(|x| x == "title.dtx"), "{t:?}");
        // a team match keeps the team names and draws no FFA block
        s.mode_class = Some(1);
        let t = texs(&build(&l, &s, &measure, &|_| None));
        assert!(t.iter().any(|x| x == "alpha.dtx"), "{t:?}");
        assert!(!t.iter().any(|x| x == "title.dtx"), "{t:?}");
    }

    #[test]
    fn life_state_5_hides_the_layer_1_elements() {
        // w2-board: render level 0 hides chat, name marks, round clock and revenge (the respawn wait is already
        // gone outside life states 2 / 3); the kill feed stays
        let l = HudLayout::fallback();
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let mut s = HudState {
            life_state: 3,
            now_ms: 2000,
            mode_class: Some(1),
            death_at: Some(1000),
            respawn_wait_ms: Some(7000),
            ..HudState::default()
        };
        s.notice("hello there".into(), 1500);
        s.revenge = Some(crate::hud::state::RevengePanel {
            at: 1000,
            killer: "foe".into(),
            count: 2,
        });
        s.texts.revenge = "%1 times by ".into();
        s.clock = crate::hud::state::RoundClock {
            shown: true,
            secs: 125,
            at: 0,
            running: true,
        };
        let shown = texts(&build(&l, &s, &measure, &|_| None));
        assert!(shown.iter().any(|t| t == "hello there"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "2:03"), "{shown:?}");
        assert!(shown.iter().any(|t| t.starts_with("RESPAWN")), "{shown:?}");
        assert!(shown.iter().any(|t| t == "2 times by foe."), "{shown:?}");
        s.life_state = 5;
        let hidden = texts(&build(&l, &s, &measure, &|_| None));
        assert!(!hidden.iter().any(|t| t == "hello there"), "{hidden:?}");
        assert!(!hidden.iter().any(|t| t == "2:03"), "{hidden:?}");
        assert!(
            !hidden.iter().any(|t| t.starts_with("RESPAWN")),
            "{hidden:?}"
        );
        assert!(!hidden.iter().any(|t| t == "2 times by foe."), "{hidden:?}");
    }

    /// CLI-983: the respawn gauge's end caps: hidden at the start, the left cap once the ticks pass its share of the
    /// three widths, the right cap (with the middle full) once fewer ticks than its share are left.
    #[test]
    fn the_respawn_gauge_draws_its_end_caps_as_it_fills() {
        use crate::hud::death::{gauge_parts, GaugeParts};
        // made-up widths: caps 10 px, middle 180 px (shares 5 % each, middle range 90)
        let w = [10, 180, 10];
        assert_eq!(
            gauge_parts(0, w),
            GaugeParts {
                left: false,
                middle: 0.0,
                right: false
            }
        );
        assert!(!gauge_parts(5, w).left && gauge_parts(6, w).left);
        assert!((gauge_parts(50, w).middle - 45.0 / 90.0).abs() < 1e-6);
        assert!(!gauge_parts(95, w).right);
        let end = gauge_parts(96, w);
        assert!(end.left && end.right && end.middle == 1.0);
        // no caps in the layout: the middle alone, in percent
        assert!((gauge_parts(40, [0, 180, 0]).middle - 0.4).abs() < 1e-6);
        // the draw: both caps are drawn at the end of the wait, none at the start
        let mut layout = HudLayout::fallback();
        let cap = |x: i32| Img {
            tex: format!("cap{x}.dtx"),
            pos: [x, 600],
            size: [10, 12],
        };
        layout.respawn.left = Some(cap(400));
        layout.respawn.right = Some(cap(590));
        layout.respawn.fill = Some(Img {
            tex: "mid.dtx".into(),
            pos: [410, 600],
            size: [180, 12],
        });
        let mut s = HudState {
            life_state: 2,
            death_at: Some(0),
            respawn_wait_ms: Some(7000),
            ..HudState::default()
        };
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let caps = |s: &HudState| {
            texs(&build(&layout, s, &measure, &|_| None))
                .iter()
                .filter(|t| t.starts_with("cap"))
                .count()
        };
        s.now_ms = 10;
        assert_eq!(caps(&s), 0);
        s.now_ms = 6900;
        assert_eq!(caps(&s), 2);
    }

    #[test]
    fn the_respawn_gauge_fills_by_the_element_ticks() {
        // w2-board: the fill is the tick count in percent, not the elapsed part of the wait
        let mut s = HudState {
            life_state: 2,
            death_at: Some(0),
            respawn_wait_ms: Some(7000),
            ..HudState::default()
        };
        let at = |s: &mut HudState, t: u32| {
            s.now_ms = t;
            crate::hud::death::respawn_gauge_fraction(s)
        };
        assert_eq!(at(&mut s, 0), 0.0);
        // the 2009 timetable (100 / 6 s = 16 ticks a second at first) stretched over the whole 7 s wait (CLI-1351):
        // 1 s gives about 14 %
        let one = at(&mut s, 1000);
        assert!((0.13..=0.15).contains(&one), "{one}");
        let mut last = 0.0;
        for t in (0..9000).step_by(50) {
            let f = at(&mut s, t);
            assert!(f >= last && f <= 1.0);
            last = f;
        }
        assert_eq!(at(&mut s, 7000), 1.0, "full at the end of the wait");
    }

    /// CLI-1853, the user's Possession match of 2026-11-04 (net log, 356.368 s): a blast after the move's closing
    /// teleport took 55 of 55.63 health, the 0x66 said 0.348 and no death followed; the block showed 000 for 27 s.
    /// A living player shows at least 1; a health of 0 without the death keeps the last live value; a real death
    /// (0x66 0 then 0x67 3, 2 in one block) hides the block as before.
    #[test]
    fn a_living_player_never_shows_zero_health() {
        let mut l = HudLayout::fallback();
        l.health.health_digits = Some(Digits {
            prefix: "h_".into(),
            pos: [[100, 700], [110, 700], [120, 700]],
            size: [8, 8],
        });
        let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
        let shown = |s: &HudState| -> Vec<String> {
            build(&l, s, &measure, &|_| None)
                .into_iter()
                .filter_map(|i| match i {
                    DrawItem::Image { tex, .. } if tex.starts_with("h_") => Some(tex),
                    _ => None,
                })
                .collect()
        };
        // the logged 0x66 heads after `0d 66`
        let status = |hex: &str| -> (u8, f32) {
            let b: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            match dc_botclient::shell::read_player_status(&mut dc_wire::udp::BitReader::new(&b)) {
                dc_botclient::shell::PlayerStatus::Value { kind, value, .. } => (kind, value),
                other => panic!("{other:?}"),
            }
        };
        let mut s = HudState {
            life_state: 1,
            ..HudState::default()
        };
        let (k, v) = status("02000020855e42");
        assert!(k == 2 && (v - 55.63).abs() < 1e-3, "{k} {v}");
        s.on_status(k, v);
        assert_eq!(shown(&s), ["h_0.dtx", "h_5.dtx", "h_5.dtx"]);
        let (k, v) = status("020000006db23e");
        assert!((v - 0.348).abs() < 1e-3, "{v}");
        s.on_status(k, v);
        assert_eq!(s.health, Some(v), "the server's value is kept as sent");
        assert_eq!(
            shown(&s),
            ["h_0.dtx", "h_0.dtx", "h_1.dtx"],
            "alive: 1, not 0"
        );
        // the 2009 whole number above 1 is unchanged
        s.on_status(2, 25.795);
        assert_eq!(shown(&s), ["h_0.dtx", "h_2.dtx", "h_5.dtx"]);
        // a 0 that contradicts the alive state: the last live value
        s.on_status(2, 0.0);
        assert_eq!(shown(&s), ["h_0.dtx", "h_2.dtx", "h_5.dtx"]);
        s.on_status(2, f32::NAN);
        assert_eq!(shown(&s), ["h_0.dtx", "h_2.dtx", "h_5.dtx"]);
        // a death: the block is gone, and the respawn's 100 is shown
        s.on_status(2, 0.0);
        s.on_life(3, 1000);
        s.on_life(2, 1000);
        assert!(shown(&s).is_empty(), "{:?}", shown(&s));
        s.on_life(1, 5000);
        s.on_status(2, 100.0);
        assert_eq!(shown(&s), ["h_1.dtx", "h_0.dtx", "h_0.dtx"]);
    }
}

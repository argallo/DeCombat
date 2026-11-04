//! The kill-feed marks beside the names: what a 0x6d line shows besides the killer name, the weapon icon, the
//! head-shot icon and the victim name.
//!
//! The 2009 line builder takes the message's fields as arguments and draws, in this order: the killer name; the
//! payback mark when the kill settled a score; the weapon icon; the head-shot icon (zone 1); the groin mark (zone 5);
//! then at one place the flag mark (relation bit: which image depends on the victim's team), the bounty mark (field
//! 9) and the freeze mark (field 7); then the victim name after all of their widths. Three forms replace the killer
//! part: feed value 5 or 10 (a self kill image), and with no killer name feed value 7 (barrel), 3 (fire) or 9 (toxic).
//! There is no team-kill mark: a team kill is a normal line whose names have the same colour.
// EVIDENCE: cshell+0x9d4a0 (line builder), cshell+0x9c3d0 (its layout keys), cshell+0x2e21e..0x2e297 (the 0x6d
// handler's arguments: relation 0 without the bit or the victim record, else 1 when the victim's team (+0x70) is the
// own team, 2 otherwise)

use super::layout::FeedLayout;
use super::state::FeedLine;
use super::DrawItem;
use dc_ui_data::{HudKey as K, HudView};

/// The special form of a line (the killer part replaced by an image).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeedForm {
    /// Killer name, payback mark, weapon icon.
    #[default]
    Normal,
    /// Feed value 5 or 10.
    SelfKill,
    /// No killer name, feed value 7.
    Barrel,
    /// No killer name, feed value 3.
    Burn,
    /// No killer name, feed value 9.
    Toxic,
}

/// The marks of one line, from its 0x6d fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeedMarks {
    pub form: FeedForm,
    /// The kill settled a score (0x6d field 15).
    pub payback: bool,
    /// Zone 5.
    pub groin: bool,
    /// The flag mark: `Some(true)` the victim is on the own team, `Some(false)` on another; `None` no mark.
    pub flag_victim_mate: Option<bool>,
    /// Field 9 (the bounty mark).
    pub bounty: bool,
    /// Field 7 (the freeze mark).
    pub freeze: bool,
}

impl FeedMarks {
    /// The form of a line from its feed value and whether it has a killer name.
    // EVIDENCE: cshell+0x9d4a0 (feed value 5 / 10 first; then an empty killer name with 7, 3, 9; else the killer name)
    pub fn form_of(feed_value: u8, killer_named: bool) -> FeedForm {
        match (feed_value, killer_named) {
            (5 | 10, _) => FeedForm::SelfKill,
            (7, false) => FeedForm::Barrel,
            (3, false) => FeedForm::Burn,
            (9, false) => FeedForm::Toxic,
            _ => FeedForm::Normal,
        }
    }
}

/// One image of the marks: texture, size, lift (vertical offset against the line top, negative = up).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MarkImg {
    pub tex: Option<String>,
    pub size: [i32; 2],
    pub lift: i32,
}

/// The layout of the marks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FeedMarkLayout {
    pub self_kill: MarkImg,
    /// Barrel; the fire and toxic images use its size and lift.
    pub barrel: MarkImg,
    pub burn: Option<String>,
    pub toxic: Option<String>,
    pub payback: MarkImg,
    /// The flag images (victim on the own team / on another team) share size and lift.
    pub flag_victim_mate: Option<String>,
    pub flag_victim_other: Option<String>,
    pub flag: MarkImg,
    pub bounty: MarkImg,
    /// The freeze mark is placed with the flag lift.
    pub freeze: MarkImg,
    pub groin: MarkImg,
}

impl FeedMarkLayout {
    /// The keys of the layout routine and of the accessors the line builder calls.
    // EVIDENCE: cshell+0x9c3d0 (self kill +0x6c / size +0x70 / lift +0x58; barrel +0x78 / size +0x7c / lift +0x5c;
    // fire +0xb8, toxic +0xbc; payback size +0x40 / lift +0x54; flag images +0x84 (team) / +0x88 (enemy), size +0x8c,
    // lift +0x60; bounty +0x94 / size +0x98 / lift +0x64; freeze +0xa0 / size +0xa4; groin +0xac / size +0xb0 / lift
    // +0x50), cshell+0x9d4a0 (the payback image read through its accessor at line time)
    pub fn from_view(v: &HudView<'_>) -> Self {
        let img = |t: K, s: Option<K>, l: Option<K>| MarkImg {
            tex: v.text(t).map(str::to_string),
            size: s.and_then(|s| v.pair(s)).unwrap_or_default(),
            lift: l.and_then(|l| v.int(l)).unwrap_or(0),
        };
        let flag = MarkImg {
            tex: None,
            size: v.pair(K::FeedFlagSize).unwrap_or_default(),
            lift: v.int(K::FeedFlagLift).unwrap_or(0),
        };
        FeedMarkLayout {
            self_kill: img(
                K::FeedSelfIcon,
                Some(K::FeedSelfSize),
                Some(K::FeedSelfLift),
            ),
            barrel: img(
                K::FeedBarrelIcon,
                Some(K::FeedBarrelSize),
                Some(K::FeedBarrelLift),
            ),
            burn: v.text(K::FeedBurnIcon).map(str::to_string),
            toxic: v.text(K::FeedToxicIcon).map(str::to_string),
            payback: img(
                K::FeedPaybackIcon,
                Some(K::FeedPaybackSize),
                Some(K::FeedPaybackLift),
            ),
            // +0x88 (the layout's enemy-flag image) for relation 1 = victim on the own team
            flag_victim_mate: v.text(K::FeedFoeFlagIcon).map(str::to_string),
            flag_victim_other: v.text(K::FeedMateFlagIcon).map(str::to_string),
            freeze: MarkImg {
                lift: flag.lift,
                ..img(K::FeedFreezeIcon, Some(K::FeedFreezeSize), None)
            },
            flag,
            bounty: img(
                K::FeedBountyIcon,
                Some(K::FeedBountySize),
                Some(K::FeedBountyLift),
            ),
            groin: img(
                K::FeedGroinIcon,
                Some(K::FeedGroinSize),
                Some(K::FeedGroinLift),
            ),
        }
    }
}

fn push(out: &mut Vec<DrawItem>, tex: &str, x: f32, y: f32, size: [i32; 2]) {
    out.push(DrawItem::Image {
        tex: tex.to_string(),
        src: [size[0] as f32, size[1] as f32],
        dst: [x, y, size[0] as f32, size[1] as f32],
        tint: [255, 255, 255, 255],
        turn: None,
    });
}

/// The special-form image of a line at the line start; returns the pen after it (`None`: a normal line).
// EVIDENCE: cshell+0x9d4a0 (the form image at the line's x, its lift added to the line top; the pen moves by its
// width; without a texture the pen stays at the line's x)
pub fn form_image(
    out: &mut Vec<DrawItem>,
    m: &FeedMarkLayout,
    form: FeedForm,
    x: f32,
    y: f32,
) -> Option<f32> {
    let (tex, size, lift) = match form {
        FeedForm::Normal => return None,
        FeedForm::SelfKill => (
            m.self_kill.tex.as_deref(),
            m.self_kill.size,
            m.self_kill.lift,
        ),
        FeedForm::Barrel => (m.barrel.tex.as_deref(), m.barrel.size, m.barrel.lift),
        FeedForm::Burn => (m.burn.as_deref(), m.barrel.size, m.barrel.lift),
        FeedForm::Toxic => (m.toxic.as_deref(), m.barrel.size, m.barrel.lift),
    };
    let Some(t) = tex else {
        return Some(x);
    };
    push(out, t, x, y + lift as f32, size);
    Some(x + size[0] as f32)
}

/// The payback mark after the killer name; returns the pen after it.
// EVIDENCE: cshell+0x9d4a0 (revenge argument: the image at the pen, lift +0x54, pen += its width)
pub fn payback(
    out: &mut Vec<DrawItem>,
    m: &FeedMarkLayout,
    line: &FeedLine,
    pen: f32,
    y: f32,
) -> f32 {
    match (&m.payback.tex, line.marks.payback) {
        (Some(t), true) => {
            push(out, t, pen, y + m.payback.lift as f32, m.payback.size);
            pen + m.payback.size[0] as f32
        }
        _ => pen,
    }
}

/// The marks after the weapon icon (`after_weapon`: the pen after the weapon icon; `head` the head-shot icon is
/// drawn before them by the caller, `f.head_size` wide when shown). Returns the victim name's x.
// EVIDENCE: cshell+0x9d4a0 (normal form: groin at the head icon's x with the lifts added up, the pen after head and
// groin; flag / bounty / freeze all at that pen, their lifts added up (freeze with the flag lift); at the end the
// victim's x = the pen after the weapon icon plus the widths of head, groin, flag, bounty, freeze (special forms add
// the widths without drawing the marks))
pub fn after_weapon(
    out: &mut Vec<DrawItem>,
    f: &FeedLayout,
    m: &FeedMarkLayout,
    line: &FeedLine,
    after_weapon: f32,
    y: f32,
) -> f32 {
    let mk = &line.marks;
    let normal = mk.form == FeedForm::Normal;
    let head_w = if line.head_shot && f.head_icon.is_some() {
        f.head_size[0] as f32
    } else {
        0.0
    };
    let mut pen = after_weapon + head_w;
    let mut top = y + if line.head_shot {
        f.head_lift as f32
    } else {
        0.0
    };
    let mut widths = head_w;
    if mk.groin {
        if let (Some(t), true) = (&m.groin.tex, normal) {
            top += m.groin.lift as f32;
            push(out, t, after_weapon, top, m.groin.size);
        }
        pen += m.groin.size[0] as f32;
        widths += m.groin.size[0] as f32;
    }
    let mut stack = y;
    if let Some(mate) = mk.flag_victim_mate {
        let tex = if mate {
            &m.flag_victim_mate
        } else {
            &m.flag_victim_other
        };
        if let (Some(t), true) = (tex, normal) {
            stack += m.flag.lift as f32;
            push(out, t, pen, stack, m.flag.size);
        }
        widths += m.flag.size[0] as f32;
    }
    if mk.bounty {
        if let (Some(t), true) = (&m.bounty.tex, normal) {
            stack += m.bounty.lift as f32;
            push(out, t, pen, stack, m.bounty.size);
        }
        widths += m.bounty.size[0] as f32;
    }
    if mk.freeze {
        if let (Some(t), true) = (&m.freeze.tex, normal) {
            push(out, t, pen, stack + m.freeze.lift as f32, m.freeze.size);
        }
        widths += m.freeze.size[0] as f32;
    }
    after_weapon + widths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_follow_the_feed_value_and_the_killer_name() {
        assert_eq!(FeedMarks::form_of(0, true), FeedForm::Normal);
        assert_eq!(FeedMarks::form_of(5, true), FeedForm::SelfKill);
        assert_eq!(FeedMarks::form_of(10, false), FeedForm::SelfKill);
        assert_eq!(FeedMarks::form_of(7, false), FeedForm::Barrel);
        assert_eq!(FeedMarks::form_of(7, true), FeedForm::Normal);
        assert_eq!(FeedMarks::form_of(3, false), FeedForm::Burn);
        assert_eq!(FeedMarks::form_of(9, false), FeedForm::Toxic);
        assert_eq!(FeedMarks::form_of(1, false), FeedForm::Normal);
    }
}

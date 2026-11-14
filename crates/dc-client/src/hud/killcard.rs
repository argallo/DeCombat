//! CLI-1957: the kill card: while the kill zoom holds on the killer ([`crate::killzoom`]), a card in the lower third of
//! the screen names him and shows his health and armour after the kill, so the player sees how close the fight was.
//! Our own two filled bars and text (no 2009 element). The state ([`KillCardShown`] in
//! [`super::state::HudState::kill_card`]) is independent of the drawing, so another HUD style can draw it its own way.
//! Room is left for the killer's clan tag and calling card (not sent yet).
//!
//! Provenance: ORIGINAL (no 2009 counterpart; owner request 2026-11-10).
// DC-DEFAULT: CLI-1957 (proposed, owner request 2026-11-10 "freeze on them for one second with their name and in the
// future clan tag + calling card and show their HP etc so you know how close you were to killing them")

use super::items::DrawItem;
use super::layout::TextStyle;
use dc_wire::killcard::Card;

/// What the card shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillCardShown {
    pub name: String,
    pub card: Card,
}

/// The card's box on the 1024 x 768 virtual screen: centred, in the lower third (above the health block's place,
/// which is hidden while dead).
pub const BOX: [f32; 4] = [362.0, 520.0, 300.0, 92.0];
pub const BACK: [u8; 4] = [10, 12, 16, 210];
/// The stripe on the box's left edge.
pub const STRIPE_W: f32 = 4.0;
pub const STRIPE: [u8; 4] = [214, 38, 28, 255];
pub const CAPTION: &str = "KILLED BY";
pub const CAPTION_COLOUR: [u8; 4] = [170, 176, 186, 255];
pub const NAME_COLOUR: [u8; 4] = [255, 255, 255, 255];
pub const BAR_W: f32 = 180.0;
pub const BAR_H: f32 = 8.0;
pub const BAR_BACK: [u8; 4] = [255, 255, 255, 40];
pub const HEALTH_FILL: [u8; 4] = [214, 38, 28, 255];
pub const ARMOR_FILL: [u8; 4] = [90, 160, 230, 255];
pub const LABEL_COLOUR: [u8; 4] = [210, 214, 222, 255];

fn style(size: i32, colour: [u8; 4]) -> TextStyle {
    TextStyle {
        font: -1,
        size: Some(size),
        colour,
    }
}

/// The filled share of a bar: `value / max`, 0..=1 (0 for a zero maximum).
pub fn share(value: u16, max: u16) -> f32 {
    if max == 0 {
        return 0.0;
    }
    (f32::from(value) / f32::from(max)).clamp(0.0, 1.0)
}

/// The draw items of the card (nothing without one).
pub fn items(out: &mut Vec<DrawItem>, shown: Option<&KillCardShown>) {
    let Some(k) = shown else {
        return;
    };
    let [x, y, w, h] = BOX;
    out.push(DrawItem::Fill {
        rect: [x, y, w, h],
        colour: BACK,
    });
    out.push(DrawItem::Fill {
        rect: [x, y, STRIPE_W, h],
        colour: STRIPE,
    });
    let left = x + 16.0;
    out.push(DrawItem::Label {
        style: style(11, CAPTION_COLOUR),
        pos: [left, y + 8.0],
        text: CAPTION.into(),
        anchor: [0.0, 0.0],
    });
    out.push(DrawItem::Label {
        style: style(20, NAME_COLOUR),
        pos: [left, y + 22.0],
        text: k.name.clone(),
        anchor: [0.0, 0.0],
    });
    let c = &k.card;
    let rows = [
        ("HP", c.health, c.health_max, HEALTH_FILL, y + 56.0),
        ("AP", c.armor, c.armor_max, ARMOR_FILL, y + 74.0),
    ];
    for (label, v, max, fill, ry) in rows {
        if label == "AP" && max == 0 {
            continue;
        }
        out.push(DrawItem::Label {
            style: style(11, LABEL_COLOUR),
            pos: [left, ry + BAR_H / 2.0],
            text: label.into(),
            anchor: [0.0, 0.5],
        });
        let bx = left + 26.0;
        out.push(DrawItem::Fill {
            rect: [bx, ry, BAR_W, BAR_H],
            colour: BAR_BACK,
        });
        let fw = (BAR_W * share(v, max)).round();
        if fw > 0.0 {
            out.push(DrawItem::Fill {
                rect: [bx, ry, fw, BAR_H],
                colour: fill,
            });
        }
        out.push(DrawItem::Label {
            style: style(11, LABEL_COLOUR),
            pos: [bx + BAR_W + 8.0, ry + BAR_H / 2.0],
            text: format!("{v} / {max}"),
            anchor: [0.0, 0.5],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(health: u16, armor_max: u16) -> KillCardShown {
        KillCardShown {
            name: "Viper".into(),
            card: Card {
                killer: 3,
                obj: 9,
                health,
                health_max: 100,
                armor: 0,
                armor_max,
            },
        }
    }

    fn texts(out: &[DrawItem]) -> Vec<String> {
        out.iter()
            .filter_map(|i| match i {
                DrawItem::Label { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_card_names_the_killer_and_shows_his_health_and_armour() {
        let mut out = Vec::new();
        items(&mut out, None);
        assert!(out.is_empty());
        items(&mut out, Some(&shown(37, 100)));
        assert_eq!(
            texts(&out),
            ["KILLED BY", "Viper", "HP", "37 / 100", "AP", "0 / 100"]
        );
        // the health bar's fill is 37 % of the bar
        assert!(out.iter().any(|i| matches!(i,
            DrawItem::Fill { rect, colour } if *colour == HEALTH_FILL && (rect[2] - (BAR_W * 0.37).round()).abs() < 1e-3)));
        // no armour on this server: no armour row
        let mut out = Vec::new();
        items(&mut out, Some(&shown(5, 0)));
        assert_eq!(texts(&out), ["KILLED BY", "Viper", "HP", "5 / 100"]);
        assert_eq!(share(5, 0), 0.0);
        assert_eq!(share(150, 100), 1.0);
    }

    /// The card over the ROM HUD of a dead player (the death panel's state), saved as
    /// `<render out>/dc-client/killcard_rom.png` to look at.
    #[test]
    fn rom_kill_card_draws_over_the_death_hud() {
        use crate::hud::{Hud, HudDriver};
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let mut d = HudDriver::new(Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD"));
        d.state.life_state = 2;
        d.state.death = Some(crate::hud::state::DeathNote {
            at: 0,
            killer: Some("Viper".into()),
            killer_rel: crate::hud::state::Relation::Opponent,
            weapon: 1,
            head_shot: false,
        });
        d.set_kill_card(Some(shown(37, 100)));
        let items = crate::hud::items::build(&d.hud.layout, &d.state, &|_, _| 0.0, &|_| None);
        assert!(items
            .iter()
            .any(|i| matches!(i, DrawItem::Label { text, .. } if text == "37 / 100")));
        if let Some(img) = d.image(true) {
            let out = dc_render::default_out_dir().join("dc-client");
            let _ = std::fs::create_dir_all(&out);
            if let Some(png) = image::RgbaImage::from_raw(img.width, img.height, img.rgba) {
                let _ = png.save(out.join("killcard_rom.png"));
            }
        }
    }
}

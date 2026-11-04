//! The weapon panel (bottom right): after every local weapon change the 2009 client shows, for 4 s, the carried
//! weapons of the selected weapon's class as weapon-row images stacked upward from a layout place: the selected one
//! with its "active" image, the others with their "inactive" image, the ones after the selected one moved by the
//! inactive gap. A spawn shows it because the client walks its loadout with weapon changes; a death clears it.
//!
//! Classes are the groups of the weapon-order section (1-based, in section order); the element enables classes 1..5.
// EVIDENCE: cshell+0xa11e0 (element constructor, layer 3: hidden while dead), cshell+0xa06d0 (layout: place, size,
// gap, inactive gap; classes 0..5 enabled), cshell+0xa0850 (an entry per carried weapon: x = place x, y = place y -
// ((k % 10) + 1) x gap + adjustment (-3 at screen widths 640 / 800, else -1), the inactive image first), cshell+0x9f750
// (shown flag, class, show time on a weapon change), cshell+0xa0c60 (the selected entry gets the active image; the
// entries after it move up by the inactive gap), cshell+0x47da0 (class = 1-based group of the weapon in the order
// list), DISASM cshell+0x9fca0..0x9fd68 (drawn while now - show time < 4000 ms, game state != 6, entries of the
// selected one's group of ten), cshell+0xa0bd0 (cleared by the 0x67 death handlers cshell+0x2a1e0 / 0x2bd80)

use super::state::HudState;
use super::DrawItem;
use dc_ui_data::{HudKey as K, HudView};
use std::collections::BTreeMap;

/// How long the panel stays after a weapon change.
// DISASM: cshell+0x9fcf3..0x9fd04 (`cmp eax, 0xfa0`)
pub const SHOW_MS: u32 = 4000;

/// The classes the element accepts (1-based).
// DISASM: cshell+0xa0739 (groups 0..5 enabled), cshell+0x9f750 (a class outside them clears the panel)
pub const CLASSES: std::ops::RangeInclusive<u8> = 1..=5;

/// Vertical adjustment of every entry at the virtual 1024-wide screen.
// EVIDENCE: cshell+0xa08e4 (-3 when the screen is 640 or 800 wide, else -1); DC-DEFAULT: CLI-282 (proposed) the HUD's
// virtual screen is 1024 wide, so -1
pub const ROW_ADJUST: i32 = -1;

/// Weapon number -> (active image, inactive image).
pub type PanelImages = BTreeMap<u16, (Option<String>, Option<String>)>;

/// The panel's layout and the weapon rows' images and classes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeaponPanelLayout {
    pub pos: [i32; 2],
    pub size: [i32; 2],
    pub gap: i32,
    pub idle_gap: i32,
    /// Weapon number -> (active image, inactive image).
    pub images: PanelImages,
    /// Weapon number -> class (1-based group of the weapon-order section).
    pub classes: BTreeMap<u16, u8>,
}

impl WeaponPanelLayout {
    /// The layout keys of set `v` and the weapon rows of the ROM (`None` when the place or the size is missing).
    pub fn from_view(
        v: &HudView<'_>,
        images: PanelImages,
        classes: BTreeMap<u16, u8>,
    ) -> Option<Self> {
        Some(WeaponPanelLayout {
            pos: v.pair(K::WeaponPanelPos)?,
            size: v.pair(K::WeaponPanelSize)?,
            gap: v.int(K::WeaponPanelGap).unwrap_or(0),
            idle_gap: v.int(K::WeaponPanelIdleGap).unwrap_or(0),
            images,
            classes,
        })
    }
}

/// The weapon rows' panel images (`[Weapon<n>]` fields named by the two pinned row keys) and every weapon's class.
// OBSERVED: ROM table WEAPONS.TXT [Weapon<n>] (the image keys of the user's CShell), [WeaponOrder] Class<k>, <class><i>
// (as `arms::WeaponTable`); DISASM cshell+0x17e20d / 0x17e273 (the row fields +0x164 / +0x16c read by those keys)
pub fn rom_rows(
    rom: &std::path::Path,
    keys: &dc_ui_data::HudKeys,
) -> (PanelImages, BTreeMap<u16, u8>) {
    let vfs = dc_rez::RezVfs::new(rom);
    let Ok(Some(doc)) = dc_bute::ButeDoc::try_from_vfs(
        &vfs,
        crate::arms::ATTRIBUTES_REZ,
        crate::arms::WEAPONS_ENTRY,
    ) else {
        return Default::default();
    };
    let (ka, ki) = (
        keys.literal(dc_ui_data::HudLiteral::WeaponPanelActive),
        keys.literal(dc_ui_data::HudLiteral::WeaponPanelIdle),
    );
    let val = |sec: &dc_bute::SectionMap, k: &str| {
        sec.iter()
            .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.trim().trim_matches('"').trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let mut images = BTreeMap::new();
    let mut names: BTreeMap<String, u16> = BTreeMap::new();
    for (name, sec) in doc.iter() {
        let Some(n) = name
            .strip_prefix("Weapon")
            .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|r| r.parse::<u16>().ok())
        else {
            continue;
        };
        images.insert(n, (val(sec, ka), val(sec, ki)));
        if let Some(nm) = val(sec, crate::arms::key::NAME) {
            names.entry(dc_bute::fold_latin1(&nm)).or_insert(n);
        }
    }
    let mut classes = BTreeMap::new();
    if let Some(order) = doc.get(crate::arms::WEAPON_ORDER_SECTION) {
        for k in 0..64u32 {
            let Some(class) = val(order, &format!("{}{k}", crate::arms::KEY_CLASS)) else {
                break;
            };
            for i in 0..4096u32 {
                let Some(entry) = val(order, &format!("{class}{i}")) else {
                    break;
                };
                if let Some(w) = names.get(&dc_bute::fold_latin1(&entry)) {
                    classes.entry(*w).or_insert((k + 1).min(255) as u8);
                }
            }
        }
    }
    (images, classes)
}

/// A shown panel: when, the carried weapons in carry order, and the selected one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelShow {
    pub at: u32,
    pub carried: Vec<u16>,
    pub selected: u16,
}

/// The panel's draw items at the state's time (appended to `out`).
pub fn draw(l: &WeaponPanelLayout, s: &HudState, out: &mut Vec<DrawItem>) {
    let Some(p) = &s.weapon_panel else {
        return;
    };
    if !super::death::live_widgets(s)
        || s.now_ms.saturating_sub(p.at) >= SHOW_MS
        || s.death.as_ref().is_some_and(|d| d.at >= p.at)
    {
        return;
    }
    // DC-DEFAULT: CLI-282 (proposed) a weapon without a class in the order section is its own class (shown alone)
    let class = l.classes.get(&p.selected).copied();
    if class.is_some_and(|c| !CLASSES.contains(&c)) {
        return;
    }
    let entries: Vec<u16> = p
        .carried
        .iter()
        .copied()
        .filter(|w| match class {
            Some(c) => l.classes.get(w) == Some(&c),
            None => *w == p.selected,
        })
        .collect();
    let Some(sel) = entries.iter().position(|w| *w == p.selected) else {
        return;
    };
    for (k, w) in entries.iter().enumerate() {
        // only the entries of the selected one's group of ten
        if k / 10 != sel / 10 {
            continue;
        }
        let (active, idle) = l.images.get(w).cloned().unwrap_or_default();
        let tex = if k == sel { active } else { idle };
        let Some(tex) = tex else {
            continue;
        };
        let mut y = l.pos[1] - ((k % 10) as i32 + 1) * l.gap + ROW_ADJUST;
        if k > sel {
            y -= l.idle_gap;
        }
        out.push(DrawItem::Image {
            tex,
            src: [l.size[0] as f32, l.size[1] as f32],
            dst: [
                l.pos[0] as f32,
                y as f32,
                l.size[0] as f32,
                l.size[1] as f32,
            ],
            tint: [255, 255, 255, 255],
            turn: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> WeaponPanelLayout {
        WeaponPanelLayout {
            pos: [771, 622],
            size: [256, 128],
            gap: 45,
            idle_gap: 30,
            images: [
                (145, "a145", "i145"),
                (146, "a146", "i146"),
                (164, "a164", "i164"),
            ]
            .into_iter()
            .map(|(w, a, i)| (w, (Some(a.to_string()), Some(i.to_string()))))
            .collect(),
            classes: [(145, 1), (146, 1), (164, 2)].into_iter().collect(),
        }
    }

    fn alive(at: u32, now: u32, carried: &[u16], selected: u16) -> HudState {
        HudState {
            life_state: 1,
            now_ms: now,
            weapon_panel: Some(PanelShow {
                at,
                carried: carried.to_vec(),
                selected,
            }),
            ..HudState::default()
        }
    }

    fn images(out: &[DrawItem]) -> Vec<(String, f32)> {
        out.iter()
            .filter_map(|i| match i {
                DrawItem::Image { tex, dst, .. } => Some((tex.clone(), dst[1])),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_weapon_per_class_shows_one_active_image_for_four_seconds() {
        let l = layout();
        let mut out = Vec::new();
        draw(&l, &alive(1000, 1000, &[145, 164], 145), &mut out);
        assert_eq!(images(&out), vec![("a145".to_string(), 576.0)]);
        out.clear();
        draw(&l, &alive(1000, 4999, &[145, 164], 145), &mut out);
        assert_eq!(out.len(), 1);
        out.clear();
        draw(&l, &alive(1000, 5000, &[145, 164], 145), &mut out);
        assert!(out.is_empty(), "gone after 4000 ms");
        let mut dead = alive(1000, 1500, &[145], 145);
        dead.life_state = 2;
        out.clear();
        draw(&l, &dead, &mut out);
        assert!(out.is_empty(), "layer 3: not while dead");
    }

    #[test]
    fn several_weapons_of_a_class_stack_with_the_selected_one_active() {
        let l = layout();
        let mut out = Vec::new();
        draw(&l, &alive(0, 10, &[145, 164, 146], 145), &mut out);
        assert_eq!(
            images(&out),
            vec![("a145".to_string(), 576.0), ("i146".to_string(), 501.0)]
        );
        out.clear();
        draw(&l, &alive(0, 10, &[145, 164, 146], 146), &mut out);
        assert_eq!(
            images(&out),
            vec![("i145".to_string(), 576.0), ("a146".to_string(), 531.0)]
        );
    }
}

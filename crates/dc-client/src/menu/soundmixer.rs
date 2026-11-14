//! CLI-1955: the Sound tab's "Sound Mixer" page: one slider per sound group ([`crate::soundmix::GROUPS`]) under
//! the groups' headings, on the same settings page the mods use (MOD-24: rows that scroll, Reset to defaults, a
//! share code). A change is heard at once; the options' OK keeps it, Cancel drops it as it does the Sound tab's own
//! rows.
//!
//! Provenance: ORIGINAL (DeCombat's own page; the 2009 options have one effects volume).
// DC-DEFAULT: CLI-1955 (proposed, owner request 2026-11-09), see DC_DEFAULTS.md

use std::collections::BTreeMap;

use dc_mod_lua::{ModSetting, NumberRange, SettingKind};

use super::modbrowser::{Button, ButtonKind};
use super::modsettings::SettingsPage;
use crate::soundmix::{Mix, DEFAULT_MIX, GROUPS};

/// The Sound tab's button that opens the page.
pub const CMD_SOUND_MIXER: i32 = 125_000;
/// The page's id (its share code names it).
pub const PAGE_ID: &str = "sound_mixer";
pub const T_BUTTON: &str = "Sound Mixer";
pub const T_TITLE: &str = "Sound Mixer";
/// The page's line under the title.
pub const T_NOTE: &str =
    "Volume of each kind of sound, on top of Sound Effects. Heard at once; OK keeps it.";
/// The button's size and its gap under the last row of the tab.
pub const BUTTON: [i32; 2] = [130, 25];
pub const BUTTON_GAP: i32 = 18;
/// The sliders' grid: 0..=100 in steps of 5, as the tab's own volume sliders step.
pub const STEP: f64 = 5.0;

/// The page's rows: one number per group.
pub fn decl() -> Vec<ModSetting> {
    GROUPS
        .iter()
        .map(|g| ModSetting {
            key: g.key().to_string(),
            label: g.label().to_string(),
            kind: SettingKind::Number(NumberRange {
                min: 0.0,
                max: 100.0,
                step: STEP,
            }),
            default: DEFAULT_MIX[g.index()].to_string(),
            group: Some(g.heading().to_string()),
        })
        .collect()
}

/// The page with the volumes of `mix`.
pub fn page(mix: &Mix) -> SettingsPage {
    let stored: BTreeMap<String, String> = GROUPS
        .iter()
        .map(|g| (g.key().to_string(), mix[g.index()].to_string()))
        .collect();
    SettingsPage::of(
        PAGE_ID,
        T_TITLE,
        std::path::Path::new(""),
        &decl(),
        Some(&stored),
    )
}

/// The volumes the page shows (a value it cannot read keeps the one in `base`).
pub fn mix_of(page: &SettingsPage, base: &Mix) -> Mix {
    let mut out = *base;
    for g in GROUPS {
        if let Some(v) = page
            .values
            .get(g.key())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
        {
            out[g.index()] = v.round().clamp(0.0, 100.0) as u8;
        }
    }
    out
}

/// The button under the tab's last row (`below`: that row's bottom), at the rows' control column `x`.
pub fn button(x: i32, below: i32) -> Button {
    Button {
        cmd: CMD_SOUND_MIXER,
        at: [x, below + BUTTON_GAP],
        size: BUTTON,
        label: T_BUTTON.into(),
        on: true,
        kind: ButtonKind::Art,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soundmix::SoundGroup;

    #[test]
    fn the_page_shows_the_mix_and_gives_back_what_was_moved() {
        let mut mix = DEFAULT_MIX;
        mix[SoundGroup::HitMarker.index()] = 35;
        let mut p = page(&mix);
        assert_eq!(p.decl.len(), GROUPS.len());
        assert_eq!(p.group_of(0), Some("Weapons"));
        assert_eq!(mix_of(&p, &DEFAULT_MIX), mix);
        // one step down on the first slider (own gunshots): 100 -> 95
        assert!(p.nudge(0, false));
        let back = mix_of(&p, &DEFAULT_MIX);
        assert_eq!(back[SoundGroup::OwnFire.index()], 95);
        assert_eq!(back[SoundGroup::HitMarker.index()], 35);
        assert!(p.reset());
        assert_eq!(mix_of(&p, &mix), DEFAULT_MIX);
    }
}

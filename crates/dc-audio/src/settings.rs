//! User settings: the four sound console variables and the profile keys (volumes, on/off, class gains).
// SPEC: AUDIO_2009.md §11. Keys are matched by hash; defaults are the spec's.
//
// Open (SPEC §11.2, §13.9): a profile without volume keys reads as 0 (silent) through the shell's profile
// reader, while the options page defaults to 100. `AudioSettings::default()` follows the options page
// (console defaults); `AudioSettings::from_profile` follows the profile reader.

use dc_bute::{ButeDoc, SectionMap};

use crate::mixer::Mixer;
use crate::KeyId;

// ---- console variables (options page, §11.1) -------------------------------------------------------
pub const CVAR_EFFECTS_VOLUME: KeyId = KeyId::new("2827d45df3bb7cac", 11);
pub const CVAR_EFFECTS_ON: KeyId = KeyId::new("0fcc24cf755dc13c", 11);
pub const CVAR_MUSIC_VOLUME: KeyId = KeyId::new("71ba43c050d8ad45", 11);
pub const CVAR_MUSIC_ON: KeyId = KeyId::new("676657ff4ce8a4b4", 11);

// ---- profile section and keys (§11.2) ---------------------------------------------------------------
pub const PROFILE_SECTION: KeyId = KeyId::new("46124e023603420f", 5);
pub const PROFILE_EFFECTS_VOLUME: KeyId = KeyId::new("2827d45df3bb7cac", 11);
pub const PROFILE_MUSIC_VOLUME: KeyId = KeyId::new("71ba43c050d8ad45", 11);
pub const PROFILE_EFFECTS_ON: KeyId = KeyId::new("b3f675e47c4fab82", 11);
pub const PROFILE_MUSIC_ON: KeyId = KeyId::new("eebd38d581fdb8be", 11);
pub const PROFILE_CLASS0_GAIN: KeyId = KeyId::new("fb1627408915be99", 22);
pub const PROFILE_CLASS1_GAIN: KeyId = KeyId::new("15a7676adee0abd1", 22);
pub const PROFILE_CLASS2_GAIN: KeyId = KeyId::new("e9c37bf66ba9e9b3", 21);
pub const PROFILE_16BIT: KeyId = KeyId::new("b913979e1883378e", 10);
pub const PROFILE_MUSIC_QUALITY: KeyId = KeyId::new("bb2efc1d7fa01e40", 12);

// ---- start-up switches (§11.3) -----------------------------------------------------------------------
pub const FLAG_DISABLE_MUSIC: KeyId = KeyId::new("af936edcba3a0520", 12);
pub const FLAG_DISABLE_SOUND: KeyId = KeyId::new("30c8747270e5405e", 12);
pub const FLAG_DISABLE_FILTERS: KeyId = KeyId::new("b123fa1cd7fa1694", 19);

/// Class defaults when the profile key is absent. // SPEC: AUDIO_2009.md §4.2, §11.2
pub const DEFAULT_CLASS_GAINS: [f32; 3] = [0.75, 1.0, 1.0];

/// The shell's three sound classes. // SPEC: AUDIO_2009.md §4.2
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundClass {
    Default = 0,
    Weapons = 1,
    Speech = 2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSettings {
    /// 0..=100.
    pub effects_volume: i32,
    pub effects_on: bool,
    pub music_volume: i32,
    pub music_on: bool,
    /// Gains of classes 0, 1, 2 in `[0, 1]`.
    pub class_gain: [f32; 3],
    /// "16-bit sound" switch and "music quality" switch: stored, audible effect UNPROVEN (SPEC §11.2).
    pub sixteen_bit: i32,
    pub music_quality: i32,
}

impl Default for AudioSettings {
    /// Options-page defaults: volumes 100, both switches on. // SPEC: AUDIO_2009.md §11.1
    fn default() -> Self {
        Self {
            effects_volume: 100,
            effects_on: true,
            music_volume: 100,
            music_on: true,
            class_gain: DEFAULT_CLASS_GAINS,
            sixteen_bit: 0,
            music_quality: 0,
        }
    }
}

fn find(sec: &SectionMap, id: KeyId) -> Option<&str> {
    sec.iter()
        .find(|(k, _)| id.matches(k))
        .map(|(_, v)| v.as_str())
}
fn p_int(s: Option<&str>, d: i32) -> i32 {
    s.and_then(|v| {
        let t = v.trim();
        t.parse::<i32>()
            .ok()
            .or_else(|| t.parse::<f64>().ok().map(|f| f as i32))
    })
    .unwrap_or(d)
}
fn p_bool(s: Option<&str>, d: bool) -> bool {
    match s.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "true" => true,
        Some(v) if v == "false" => false,
        Some(v) => v.parse::<i64>().map(|n| n != 0).unwrap_or(d),
        None => d,
    }
}
fn p_f32(s: Option<&str>, d: f32) -> f32 {
    s.and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|x| x.is_finite())
        .unwrap_or(d)
}

impl AudioSettings {
    /// Reads the profile section as the shell's profile reader does: volumes default to 0, switches to on,
    /// class gains to 0.75 / 1.0 / 1.0; the music volume is forced to 0 when music is not allowed.
    /// // SPEC: AUDIO_2009.md §11.2 (cshell+0x1ecef0)
    pub fn from_profile_section(sec: &SectionMap, music_allowed: bool) -> Self {
        let mut s = Self {
            effects_volume: p_int(find(sec, PROFILE_EFFECTS_VOLUME), 0),
            effects_on: p_bool(find(sec, PROFILE_EFFECTS_ON), true),
            music_volume: p_int(find(sec, PROFILE_MUSIC_VOLUME), 0),
            music_on: p_bool(find(sec, PROFILE_MUSIC_ON), true),
            class_gain: [
                p_f32(find(sec, PROFILE_CLASS0_GAIN), DEFAULT_CLASS_GAINS[0]),
                p_f32(find(sec, PROFILE_CLASS1_GAIN), DEFAULT_CLASS_GAINS[1]),
                p_f32(find(sec, PROFILE_CLASS2_GAIN), DEFAULT_CLASS_GAINS[2]),
            ],
            sixteen_bit: p_int(find(sec, PROFILE_16BIT), 0),
            music_quality: p_int(find(sec, PROFILE_MUSIC_QUALITY), 0),
        };
        if !music_allowed {
            s.music_volume = 0;
        }
        s
    }

    /// Finds the profile section by hash in `doc`; `None` when absent.
    pub fn from_profile(doc: &ButeDoc, music_allowed: bool) -> Option<Self> {
        doc.iter()
            .find(|(name, _)| PROFILE_SECTION.matches(name))
            .map(|(_, sec)| Self::from_profile_section(sec, music_allowed))
    }

    /// SFX master value the options page applies: the volume, or 0 when effects are disabled.
    /// // SPEC: AUDIO_2009.md §11.1
    pub fn effective_effects(&self) -> u16 {
        if self.effects_on {
            self.effects_volume.clamp(0, 100) as u16
        } else {
            0
        }
    }
    pub fn effective_music(&self) -> u16 {
        if self.music_on {
            self.music_volume.clamp(0, 100) as u16
        } else {
            0
        }
    }

    /// Pushes the settings into a mixer: both masters and the three class gains (classes 0 and 1
    /// master-scaled, speech not). // SPEC: AUDIO_2009.md §4.2, §4.3
    pub fn apply(&self, m: &mut Mixer) {
        m.set_effects_volume(self.effective_effects());
        m.set_music_volume(self.effective_music());
        for (i, g) in self.class_gain.iter().enumerate() {
            m.classes.set_gain(i, g.clamp(0.0, 1.0));
        }
        m.classes
            .set_master_scaled(SoundClass::Default as usize, true);
        m.classes
            .set_master_scaled(SoundClass::Weapons as usize, true);
        m.classes
            .set_master_scaled(SoundClass::Speech as usize, false);
    }
}

/// Start-up switches (command line / console), all default off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartupFlags {
    pub disable_music: bool,
    pub disable_sound: bool,
    pub disable_filters: bool,
}

impl StartupFlags {
    /// True if `name` is one of the switches' names (by hash); sets it.
    pub fn set_by_name(&mut self, name: &str) -> bool {
        if FLAG_DISABLE_MUSIC.matches(name) {
            self.disable_music = true;
        } else if FLAG_DISABLE_SOUND.matches(name) {
            self.disable_sound = true;
        } else if FLAG_DISABLE_FILTERS.matches(name) {
            self.disable_filters = true;
        } else {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_effective_values() {
        let s = AudioSettings::default();
        assert_eq!((s.effective_effects(), s.effective_music()), (100, 100));
        let mut s = s;
        s.effects_on = false;
        s.music_volume = 250;
        assert_eq!((s.effective_effects(), s.effective_music()), (0, 100));
        s.music_on = false;
        assert_eq!(s.effective_music(), 0);
        assert_eq!(s.class_gain, [0.75, 1.0, 1.0]);
    }

    #[test]
    fn empty_profile_reads_silent() {
        let s = AudioSettings::from_profile_section(&SectionMap::new(), true);
        assert_eq!((s.effects_volume, s.music_volume), (0, 0));
        assert!(s.effects_on && s.music_on);
        assert_eq!(s.class_gain, [0.75, 1.0, 1.0]);
    }

    #[test]
    fn apply_sets_mixer() {
        let mut m = Mixer::new();
        let s = AudioSettings {
            effects_volume: 40,
            music_on: false,
            ..AudioSettings::default()
        };
        s.apply(&mut m);
        assert_eq!((m.effects_volume(), m.music_volume()), (40, 0));
        assert_eq!(m.classes.gain(0), 0.75);
        assert!(!m.classes.master_scaled(2));
    }

    #[test]
    fn key_hashes_are_distinct_and_lengths_sane() {
        let ids = [
            CVAR_EFFECTS_VOLUME,
            CVAR_EFFECTS_ON,
            CVAR_MUSIC_VOLUME,
            CVAR_MUSIC_ON,
            PROFILE_SECTION,
            PROFILE_EFFECTS_ON,
            PROFILE_MUSIC_ON,
            PROFILE_CLASS0_GAIN,
            PROFILE_CLASS1_GAIN,
            PROFILE_CLASS2_GAIN,
            PROFILE_16BIT,
            PROFILE_MUSIC_QUALITY,
            FLAG_DISABLE_MUSIC,
            FLAG_DISABLE_SOUND,
            FLAG_DISABLE_FILTERS,
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a.hex16, b.hex16);
            }
            assert_eq!(a.hex16.len(), 16);
        }
    }
}

//! Reverb rooms: parameter table only, no DSP yet.
// SPEC: AUDIO_2009.md §7
// TODO(dsp): the original delegates to the mixer's EAX 2 room; a reverb unit is not implemented here.

/// The 26 public EAX 2.0 listener environment presets in their public enumeration order.
/// // PUBLIC: EAX 2.0 listener environments
pub const PRESET_NAMES: [&str; 26] = [
    "generic",
    "paddedcell",
    "room",
    "bathroom",
    "livingroom",
    "stoneroom",
    "auditorium",
    "concerthall",
    "cave",
    "arena",
    "hangar",
    "carpetedhallway",
    "hallway",
    "stonecorridor",
    "alley",
    "forest",
    "city",
    "mountains",
    "quarry",
    "plain",
    "parkinglot",
    "sewerpipe",
    "underwater",
    "drugged",
    "dizzy",
    "psychotic",
];

/// Preset index for a name (case-insensitive, spaces/underscores ignored); `None` = engine error 0x3d.
pub fn preset_index(name: &str) -> Option<usize> {
    let norm: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    PRESET_NAMES.iter().position(|p| *p == norm)
}

/// Custom-mode parameter set with the sent defaults. // SPEC: AUDIO_2009.md §7.2
#[derive(Clone, Debug, PartialEq)]
pub struct CustomReverb {
    pub room_mb: i32,
    pub room_hf_mb: i32,
    pub room_rolloff: f32,
    pub decay_time_s: f32,
    pub decay_hf_ratio: f32,
    pub reflections_mb: i32,
    pub reflections_delay_s: f32,
    pub reverb_mb: i32,
    pub reverb_delay_s: f32,
    pub environment_size_m: f32,
    pub air_absorption_hf_mb: f32,
    pub diffusion: f32,
    /// Constructor value of the damping argument. // SPEC: AUDIO_2009.md §7.1
    pub damping: f32,
}

impl Default for CustomReverb {
    fn default() -> Self {
        Self {
            room_mb: -1000,
            room_hf_mb: -100,
            room_rolloff: 0.0,
            decay_time_s: 1.49,
            decay_hf_ratio: 0.83,
            reflections_mb: -2602,
            reflections_delay_s: 0.007,
            reverb_mb: 200,
            reverb_delay_s: 0.011,
            environment_size_m: 7.5,
            air_absorption_hf_mb: -5.0,
            diffusion: 1.0,
            damping: 1.0,
        }
    }
}

/// Which reverb mode is selected.
#[derive(Clone, Debug, PartialEq)]
pub enum ReverbSelect {
    None,
    Preset(usize),
    Custom(CustomReverb),
}

/// Selected filter plus the active flag. A voice gets the wet send only if reverb was active when it
/// started and its own reverb flag is set. // SPEC: AUDIO_2009.md §7.1
#[derive(Clone, Debug, PartialEq)]
pub struct ReverbState {
    pub select: ReverbSelect,
    pub active: bool,
}

impl Default for ReverbState {
    fn default() -> Self {
        Self {
            select: ReverbSelect::None,
            active: false,
        }
    }
}

impl ReverbState {
    /// `enable(on)`: active only with a selected filter. // SPEC: AUDIO_2009.md §7.1
    pub fn enable(&mut self, on: bool) {
        self.active = on && self.select != ReverbSelect::None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets() {
        assert_eq!(PRESET_NAMES.len(), 26);
        assert_eq!(preset_index("Generic"), Some(0));
        assert_eq!(preset_index("concert_hall"), Some(7));
        assert_eq!(preset_index("nope"), None);
    }

    #[test]
    fn enable_needs_selection() {
        let mut s = ReverbState::default();
        s.enable(true);
        assert!(!s.active);
        s.select = ReverbSelect::Preset(3);
        s.enable(true);
        assert!(s.active);
    }
}

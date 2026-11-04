//! dc-audio: the audible-behaviour core of the 2009 client's sound system.
//!
//! Written from `rust/specs/AUDIO_2009.md` (SPEC), not from the third-party mixer's API: a software mixer
//! with 255 voices and two output groups (effects, music) at 44.1 kHz, the engine-side attenuation law,
//! the per-voice level pipeline, the BGM two-slot player, the sound attribute records with parent-chain
//! inheritance and the user settings. Device output is behind the `device` feature (off by default; cpal);
//! without it the [`output::NullSink`] renders the mix into a buffer nobody hears (headless runs).
//!
//! Strings that the original looks up (attribute keys, profile keys, console variables) are matched by
//! `sha256(utf8)[..16]` + length as the spec gives them, so no ROM text is reproduced in this tree.

pub mod attrib;
pub mod bgm;
pub mod decode;
pub mod law;
pub mod mixer;
pub mod output;
pub mod reverb;
pub mod settings;

#[cfg(feature = "device")]
pub mod device;

/// Output sample rate of both drivers. // SPEC: AUDIO_2009.md §2
pub const OUTPUT_RATE: u32 = 44_100;
/// Mixer voice handles and active-list capacity. // SPEC: AUDIO_2009.md §2 step 5, §3.4 step 3
pub const MAX_VOICES: usize = 255;
/// Sound classes the manager keeps. // SPEC: AUDIO_2009.md §4.2
pub const CLASS_COUNT: usize = 16;

/// A key the original looks up, identified by `sha256(text)[..16]` (hex) and the text length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyId {
    pub hex16: &'static str,
    pub len: usize,
}

impl KeyId {
    pub const fn new(hex16: &'static str, len: usize) -> Self {
        Self { hex16, len }
    }

    /// True when `text` is the key this id names.
    pub fn matches(&self, text: &str) -> bool {
        text.len() == self.len && hash16(text) == self.hex16
    }
}

/// `sha256(text)[..16]` as lowercase hex.
pub fn hash16(text: &str) -> String {
    dc_rom::sha256_hex(text.as_bytes())[..16].to_string()
}

//! `.spr` sprite animation. SPEC: FMT_SPR_LTC_FX.md §2.

use crate::error::{latin1_cstr, Bin, FxError};

/// Longest frame path the engine accepts. // EVIDENCE: engine+0xa7430 (`cmp ax,0x3e8`)
pub const MAX_FRAME_NAME: usize = 1000;

/// One parsed sprite: a single animation of texture paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sprite {
    pub frames_per_second: u32,
    /// Low byte of header word 3. Meaning unproven (SPEC §2.2); stored as read.
    pub flag_a: u8,
    /// Low byte of header word 4. Meaning unproven.
    pub flag_b: u8,
    /// Header word 5, kept whole. Meaning unproven.
    pub key_word: u32,
    /// Texture paths, cut at the first NUL, one char per byte.
    pub frames: Vec<String>,
    /// Bytes after the last frame (0 in every ROM file; the engine ignores them).
    pub trailing: usize,
}

impl Sprite {
    /// Parse a whole `.spr` entry. SPEC: FMT_SPR_LTC_FX.md §2.2.
    pub fn parse(data: &[u8]) -> Result<Sprite, FxError> {
        if data.is_empty() {
            return Err(FxError::Empty);
        }
        let mut r = Bin::new(data);
        let frames_declared = r.u32("sprite frames_declared")?;
        let frames_per_second = r.u32("sprite fps")?;
        let flag_a = r.u32("sprite flag_a")? as u8;
        let flag_b = r.u32("sprite flag_b")? as u8;
        let key_word = r.u32("sprite key_word")?;
        // Every frame costs at least its u16 length, so a larger count cannot be satisfied.
        if frames_declared as usize > r.remaining() / 2 {
            return Err(FxError::Truncated {
                what: "sprite frames",
                offset: r.pos,
            });
        }
        let mut frames = Vec::with_capacity(frames_declared as usize);
        for _ in 0..frames_declared {
            let len = r.u16("sprite frame length")? as usize;
            if len > MAX_FRAME_NAME {
                return Err(FxError::NameTooLong {
                    what: "sprite frame name",
                    len,
                });
            }
            let name = r.bytes(len, "sprite frame name")?;
            frames.push(latin1_cstr(name, MAX_FRAME_NAME));
        }
        Ok(Sprite {
            frames_per_second,
            flag_a,
            flag_b,
            key_word,
            frames,
            trailing: r.remaining(),
        })
    }

    /// `1000 / fps` in integer milliseconds; 0 when `fps` is 0 (the 2009 code would fault) or above 1000.
    /// // EVIDENCE: engine+0xa7430 (div at 0xa7551); fps == 0 guarded per SPEC §2.5.
    pub fn frame_period_ms(&self) -> u32 {
        1000u32.checked_div(self.frames_per_second).unwrap_or(0)
    }

    /// Animation length in milliseconds (`frame_period_ms * frames_declared`).
    pub fn length_ms(&self) -> u32 {
        self.frame_period_ms()
            .wrapping_mul(self.frames.len() as u32)
    }
}

/// Playback cursor. // EVIDENCE: engine+0xa72f0 (init), engine+0xa7330 (advance)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpriteCursor {
    pub time_ms: u32,
    pub playing: bool,
    pub looping: bool,
    /// Index into [`Sprite::frames`], or `None` when the animation has no length.
    pub frame: Option<usize>,
}

impl SpriteCursor {
    /// Time 0, frame 0, playing and looping (flags 3).
    pub fn new(sprite: &Sprite) -> Self {
        SpriteCursor {
            time_ms: 0,
            playing: true,
            looping: true,
            frame: if sprite.frames.is_empty() {
                None
            } else {
                Some(0)
            },
        }
    }

    /// Advance by `delta_ms` and recompute the current frame. SPEC: FMT_SPR_LTC_FX.md §2.4.
    pub fn advance(&mut self, sprite: &Sprite, delta_ms: u32) {
        if self.playing {
            self.time_ms = self.time_ms.wrapping_add(delta_ms);
        }
        let length = sprite.length_ms();
        if length == 0 {
            self.frame = None;
            return;
        }
        if self.time_ms < length || self.looping {
            self.time_ms %= length;
        } else {
            self.time_ms = length - 1;
        }
        let idx = (self.time_ms / sprite.frame_period_ms()) as usize % sprite.frames.len();
        self.frame = Some(idx);
    }
}

//! Low health: the health block (stance image, health / armour icons, stamina gauge, both digit groups) turns red at
//! 30 health or less and pulses between two reds, and a heartbeat sound plays at a fixed interval while it shows.
//!
//! The 2009 element keeps the colours, the pulse rate and step, and the beat interval from its layout set
//! ([`LowHealthLayout::from_view`]); the refresh on a health change picks white or red, the per-frame part of the
//! draw slot steps the pulse and the heartbeat. Here the colour is a function of the time since the block first
//! showed low ([`LowHealthClock`]), so it does not depend on the frame rate.
// EVIDENCE: cshell+0x967a0 (refresh: health and armour clamped at 0, then cshell+0x95f90(health)), cshell+0x96b30
// (draw slot of the element, layer 3, calls cshell+0x96210 after drawing its parts), cshell+0x975f0 (layout keys)

use super::state::HudState;
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::{HudKey as K, HudView};
use std::path::Path;

/// Health at or below which the block is red (the 2009 test is `30 < health` for white).
// EVIDENCE: cshell+0x95f90 (`0x1e < health` and no special-state flag: every part white, pulse flags cleared; else
// every part gets the bright red with full alpha and the pulse flag is set)
pub const LOW_HEALTH: i32 = 30;

/// The element's colours before its layout runs: bright red 0xff0000, dark red 0xb20000 (alpha byte 0).
// EVIDENCE: cshell+0x97200 (constructor: +0x74 = 0xff0000, +0x78 = 0xb20000)
pub const DEFAULT_BRIGHT: u32 = 0x00ff_0000;
pub const DEFAULT_DARK: u32 = 0x00b2_0000;

/// The pulse and heartbeat parameters of the health block.
#[derive(Debug, Clone, PartialEq)]
pub struct LowHealthLayout {
    /// Bright and dark red as `0x00RRGGBB` (the layout colours with the alpha byte cleared).
    pub bright: u32,
    pub dark: u32,
    /// Pulse steps per second (0: no pulse, no heartbeat).
    pub rate: i32,
    /// Seconds of one sweep from dark to bright (the step per tick derives from it).
    pub sweep: f32,
    /// Seconds between two heartbeats.
    pub beat_secs: f32,
    /// The heartbeat sound file (character sound table), `None` without a ROM.
    pub beat_file: Option<String>,
}

impl Default for LowHealthLayout {
    /// Without layout values the element keeps its constructor colours and never pulses: a steady bright red.
    fn default() -> Self {
        LowHealthLayout {
            bright: DEFAULT_BRIGHT,
            dark: DEFAULT_DARK,
            rate: 0,
            sweep: 0.0,
            beat_secs: 0.0,
            beat_file: None,
        }
    }
}

fn pack(c: [f64; 3]) -> u32 {
    let b = |v: f64| v.clamp(0.0, 255.0) as u32;
    (b(c[0]) << 16) | (b(c[1]) << 8) | b(c[2])
}

impl LowHealthLayout {
    /// The five layout values of the health block (absent keys keep the constructor's values).
    // EVIDENCE: cshell+0x975f0 (bright colour -> +0x74 and dark colour -> +0x78 through cshell+0x5a7f0 with the alpha
    // byte cleared; the pulse rate (int) -> +0x7c; the sweep (real) -> +0x80; the beat interval (real) -> +0x84)
    pub fn from_view(v: &HudView<'_>) -> Self {
        let d = LowHealthLayout::default();
        LowHealthLayout {
            bright: v
                .colour(K::LowHealthMaxColour)
                .map(pack)
                .unwrap_or(d.bright),
            dark: v.colour(K::LowHealthMinColour).map(pack).unwrap_or(d.dark),
            rate: v.int(K::LowHealthPulseRate).unwrap_or(0),
            sweep: v.real(K::LowHealthPulseTime).unwrap_or(0.0) as f32,
            beat_secs: v.real(K::LowHealthBeatSecs).unwrap_or(0.0) as f32,
            beat_file: None,
        }
    }

    /// The pulse runs (and with it the heartbeat): a positive rate and a positive sweep.
    // DISASM: cshell+0x9625a..0x9627a (rate <= 0 or sweep <= 0 leaves the per-frame part at once)
    pub fn pulses(&self) -> bool {
        self.rate > 0 && self.sweep > 0.0
    }

    /// Milliseconds between two pulse steps: the 2009 test steps when MORE than `1000 / rate` ms passed since the
    /// last step, on a millisecond clock, so the steps are `1000 / rate + 1` ms apart.
    // DISASM: cshell+0x962c0..0x962d6 (now - last > 1000 / rate, integer division, unsigned compare)
    pub fn step_ms(&self) -> u32 {
        (1000 / self.rate.max(1)) as u32 + 1
    }

    /// The colour step per tick in red units: `trunc(((bright red - dark red) / sweep) / rate)`.
    // DISASM: cshell+0x9629d..0x962bc (red bytes +0x76 and +0x7a, divided by the sweep, then by the rate, truncated)
    pub fn step(&self) -> i32 {
        let red = |c: u32| ((c >> 16) & 0xff) as i32;
        let d = (red(self.bright) - red(self.dark)) as f64;
        let q = d / self.sweep as f64 / self.rate as f64;
        if q == 0.0 || !q.is_finite() {
            0
        } else {
            q.trunc() as i32
        }
    }

    /// The pulse colour (`0x00RRGGBB`) after `steps` ticks: the first tick happens on the first low frame, starting
    /// from the dark red going up; at or above the bright red the direction turns down, at or below the dark red up;
    /// each tick moves the red byte by [`LowHealthLayout::step`], clamped to the bounds (whole-word compares).
    // DISASM: cshell+0x96239..0x96257 (first low frame: current = dark, direction up, last step time 1 so the first
    // tick is due at once), cshell+0x962e2..0x96324 (direction tests, step << 16 added or subtracted, clamps)
    pub fn pulse(&self, steps: u64) -> u32 {
        let (hi, lo) = (self.bright, self.dark);
        let delta = (self.step() as u32).wrapping_shl(16);
        let tick = |cur: u32, up: bool| -> (u32, bool) {
            let mut up = up;
            if cur >= hi {
                up = false;
            }
            if cur <= lo {
                up = true;
            }
            if up {
                (cur.wrapping_add(delta).min(hi), up)
            } else {
                (cur.wrapping_sub(delta).max(lo), up)
            }
        };
        // the sequence of (colour, direction) is periodic after a few ticks: find the cycle and skip whole rounds
        let mut seen: Vec<(u32, bool)> = Vec::new();
        let (mut cur, mut up) = (lo, true);
        let mut n = steps;
        while n > 0 {
            if let Some(p) = seen.iter().position(|s| *s == (cur, up)) {
                let len = (seen.len() - p) as u64;
                for _ in 0..(n % len) {
                    (cur, up) = tick(cur, up);
                }
                return cur;
            }
            seen.push((cur, up));
            (cur, up) = tick(cur, up);
            n -= 1;
            if seen.len() > 4096 {
                break;
            }
        }
        cur
    }
}

/// The section literal of the character sound lookup and the heartbeat key the element passes to it.
// EVIDENCE: cshell+0x1b2c44 (the section literal pushed by the lookup cshell+0x1b2c30), DISASM: cshell+0x964f6 (the
// key literal pushed before cshell+0x1b2c30; the path is then played by cshell+0x1b1e20 with priority 6, flags 0,
// volume 100, pitch 1.0)
pub const CHARACTER_SOUND_SECTION: Pin = Pin {
    what: "character sound section",
    rva: 0x3eb858,
    sha16: "1405a63e8a78b85c",
    len: 9,
};
pub const HEARTBEAT_KEY: Pin = Pin {
    what: "heartbeat sound key",
    rva: 0x3d6de0,
    sha16: "c8cbd842ebddee7c",
    len: 18,
};

/// The heartbeat file: the pinned key looked up in the client attribute file's character section (`None` when
/// anything does not resolve).
// OBSERVED: ROM table ATTRIBUTES.rez CLIENTBUTES.TXT (the section of the pinned literal holds the key; value read at
// run time)
pub fn load_beat_file(rom: &Path) -> Option<String> {
    let cs = CSHELL_REL_PATH;
    let pe = dc_rom::PeFile::from_file(&rom.join(cs[0]).join(cs[1])).ok()?;
    let section = read_pinned(&pe, &CHARACTER_SOUND_SECTION).ok()?;
    let key = read_pinned(&pe, &HEARTBEAT_KEY).ok()?;
    let vfs = dc_rez::RezVfs::new(rom);
    let doc = dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", crate::sound::CLIENT_ENTRY)?;
    let sec = doc.get(&section)?;
    sec.get(&key)
        .map(|f| f.trim().trim_matches('"').to_string())
        .filter(|f| !f.is_empty())
}

/// The board window's open-sound key (looked up in the character sound section like the heartbeat).
// EVIDENCE: cshell+0xc28e0 (the board's show: on the off -> on flip outside the result mode it looks this literal
// up through cshell+0x1b2c00 and plays the path through cshell+0x1b20c0; closing plays nothing)
pub const BOARD_OPEN_KEY: Pin = Pin {
    what: "board open sound key",
    rva: 0x3d5ca8,
    sha16: "f477a69772fcf22e",
    len: 18,
};

/// The board's open-sound file (`None` when anything does not resolve).
// OBSERVED: ROM table ATTRIBUTES.rez CLIENTBUTES.TXT (the section of the pinned literal; value read at run time)
pub fn load_board_sound(rom: &Path) -> Option<String> {
    load_key_file(rom, &BOARD_OPEN_KEY)
}

fn load_key_file(rom: &Path, key: &Pin) -> Option<String> {
    let cs = CSHELL_REL_PATH;
    let pe = dc_rom::PeFile::from_file(&rom.join(cs[0]).join(cs[1])).ok()?;
    let section = read_pinned(&pe, &super::banners::SOUND_SECTION).ok()?;
    let key = read_pinned(&pe, key).ok()?;
    let vfs = dc_rez::RezVfs::new(rom);
    let doc = dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", crate::sound::CLIENT_ENTRY)?;
    doc.get(&section)?
        .get(&key)
        .map(|f| f.trim().trim_matches('"').to_string())
        .filter(|f| !f.is_empty())
}

/// When the block went low and when the last heartbeat played.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LowHealthClock {
    /// HUD time of the first frame drawn low since the health was last above [`LOW_HEALTH`].
    pub since: Option<u32>,
    /// HUD time of the last heartbeat (kept for the whole session, as the 2009 timer is a static).
    pub beat_last: Option<u32>,
    /// Heartbeats due since the last [`super::HudDriver::take_sound_cues`].
    pub beats: u32,
}

/// The health is low by the 2009 refresh rule (health as an integer, clamped at 0).
pub fn is_low(s: &HudState) -> bool {
    s.health.is_some_and(|h| (h.max(0.0) as i32) <= LOW_HEALTH)
}

/// Advances the clock to `now`: the pulse and the heartbeat run only while the health block is drawn (own player
/// alive at render level 3) and low.
// EVIDENCE: cshell+0x96b30 (the pulse runs from the element's draw slot, i.e. only while the element is drawn);
// DISASM: cshell+0x9647f..0x96519 (the beat timer is a function static set on first use; a beat when more than
// interval x 1000 ms passed, then the timer restarts)
pub fn tick(s: &mut HudState, l: &LowHealthLayout, now: u32) {
    if !is_low(s) {
        // EVIDENCE: cshell+0x95f90 (above the limit the pulse flags are cleared: the next low phase starts dark)
        s.low_health.since = None;
        return;
    }
    if !super::death::live_widgets(s) {
        return;
    }
    if s.low_health.since.is_none() {
        s.low_health.since = Some(now);
    }
    if !l.pulses() {
        return;
    }
    match s.low_health.beat_last {
        None => s.low_health.beat_last = Some(now),
        Some(t) if now.saturating_sub(t) as f32 > l.beat_secs * 1000.0 => {
            s.low_health.beat_last = Some(now);
            s.low_health.beats += 1;
        }
        Some(_) => {}
    }
}

/// The tint of the health block at the state's time: `None` (white) above [`LOW_HEALTH`]; the bright red on the
/// refresh and, while pulsing, the pulse colour of the steps since the low phase began.
pub fn tint(s: &HudState, l: &LowHealthLayout) -> Option<[u8; 4]> {
    if !is_low(s) {
        return None;
    }
    let c = match (s.low_health.since, l.pulses()) {
        (Some(t0), true) => {
            let steps = (s.now_ms.saturating_sub(t0) / l.step_ms()) as u64 + 1;
            l.pulse(steps)
        }
        _ => l.bright,
    };
    Some([(c >> 16) as u8, (c >> 8) as u8, c as u8, 255])
}

/// Multiplies the tint into every image and text item of `items` (the parts of the health block).
// EVIDENCE: cshell+0x96210 (every part of the block gets the pulse colour: the eight stance images, the two icons, the
// stamina back and gauge, the six digits; the health arc beside the aim mark (+0x5c) does not)
pub fn apply(items: &mut [super::DrawItem], tint: [u8; 4]) {
    let mul = |c: &mut [u8; 4]| {
        for i in 0..4 {
            c[i] = ((c[i] as u32 * tint[i] as u32) / 255) as u8;
        }
    };
    for it in items {
        match it {
            super::DrawItem::Image { tint: c, .. } => mul(c),
            super::DrawItem::Text { style, .. } => mul(&mut style.colour),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom_like() -> LowHealthLayout {
        LowHealthLayout {
            bright: 0x00ff_0000,
            dark: 0x00b2_0000,
            rate: 50,
            sweep: 0.26,
            beat_secs: 1.0,
            beat_file: None,
        }
    }

    #[test]
    fn step_and_pulse_follow_the_2009_arithmetic() {
        let l = rom_like();
        // (255 - 178) / 0.26 / 50 = 5.92 -> 5
        assert_eq!(l.step(), 5);
        assert_eq!(l.step_ms(), 21);
        assert_eq!(l.pulse(0), 0x00b2_0000);
        assert_eq!(l.pulse(1), 0x00b7_0000);
        // 16 ticks up: 0xb2 + 80 = 0x102 -> clamped to 0xff on the 16th
        assert_eq!(l.pulse(15), 0x00b2_0000 + (75 << 16));
        assert_eq!(l.pulse(16), 0x00ff_0000);
        assert_eq!(l.pulse(17), 0x00fa_0000, "turns down at the bright red");
        // periodic: the same colour one whole cycle later, also for huge step counts
        let cycle =
            (1..200).find(|n| l.pulse(17 + n) == l.pulse(17) && l.pulse(18 + n) == l.pulse(18));
        let c = cycle.unwrap();
        assert_eq!(l.pulse(1_000_017 * c), l.pulse(17 * c));
        // no pulse: zero step stays dark
        let flat = LowHealthLayout {
            sweep: 1000.0,
            ..rom_like()
        };
        assert_eq!(flat.step(), 0);
        assert_eq!(flat.pulse(99), 0x00b2_0000);
    }

    #[test]
    fn tint_threshold_clock_and_heartbeat() {
        let l = rom_like();
        let mut s = HudState {
            life_state: 1,
            health: Some(31.0),
            ..HudState::default()
        };
        tick(&mut s, &l, 1000);
        assert_eq!(tint(&s, &l), None, "31 is white");
        s.health = Some(30.0);
        s.now_ms = 1000;
        tick(&mut s, &l, 1000);
        assert_eq!(s.low_health.since, Some(1000));
        assert_eq!(tint(&s, &l), Some([0xb7, 0, 0, 255]), "first tick at once");
        s.now_ms = 1000 + 21 * 15;
        assert_eq!(tint(&s, &l), Some([0xff, 0, 0, 255]));
        // heartbeat: the timer starts on the first low frame, a beat when more than 1 s passed
        tick(&mut s, &l, 2000);
        assert_eq!(s.low_health.beats, 0);
        tick(&mut s, &l, 2001);
        assert_eq!(s.low_health.beats, 1);
        tick(&mut s, &l, 3001);
        assert_eq!(s.low_health.beats, 1);
        tick(&mut s, &l, 3002);
        assert_eq!(s.low_health.beats, 2);
        // dead: nothing runs, the low phase stays; healed: the phase ends
        s.life_state = 2;
        tick(&mut s, &l, 9000);
        assert_eq!(s.low_health.beats, 2);
        s.life_state = 1;
        s.health = Some(100.0);
        tick(&mut s, &l, 9100);
        assert_eq!(s.low_health.since, None);
        assert_eq!(tint(&s, &l), None);
        // without layout values: steady bright red, no beats
        let d = LowHealthLayout::default();
        s.health = Some(5.0);
        tick(&mut s, &d, 9200);
        assert_eq!(tint(&s, &d), Some([255, 0, 0, 255]));
        tick(&mut s, &d, 99_999);
        assert_eq!(s.low_health.beats, 2);
    }
}

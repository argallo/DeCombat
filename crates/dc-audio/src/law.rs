//! The engine-side gain laws as pure functions.
// SPEC: AUDIO_2009.md §5.3 (distance law, stage A), §4.1 (stage B), §4.3 (masters), §4.2 (classes)

/// Squared radii of a positional/ambient voice, derived once at voice init.
/// `span2` is `(outer - inner)^2`, deliberately not `outer^2 - inner^2`. // SPEC: AUDIO_2009.md §5.3
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radii {
    pub in2: f32,
    pub out2: f32,
    pub span2: f32,
}

impl Radii {
    pub fn new(inner: f32, outer: f32) -> Self {
        let span = outer - inner;
        Self {
            in2: inner * inner,
            out2: outer * outer,
            span2: span * span,
        }
    }
}

/// Stage A update. Inside the OPEN interval `in2 < d2 < out2`:
/// `A = (1 - (d2 - in2) / span2) * volume / 100`; otherwise `prev` is returned unchanged
/// (1.0 at voice start). The result can be negative between `sqrt(in2 + span2)` and the outer radius;
/// the mixer clamps at use. // SPEC: AUDIO_2009.md §5.3
pub fn stage_a(prev: f32, d2: f32, r: Radii, voice_volume: u8) -> f32 {
    if r.in2 < d2 && d2 < r.out2 {
        let t = (d2 - r.in2) / r.span2;
        (1.0 - t) * f32::from(voice_volume) / 100.0
    } else {
        prev
    }
}

/// Stage B (voice volume levels, L = R):
/// `floor(floor(v * class_gain * m) * 127 / 100) * 0.01` where `m = fx_level / 100` for master-scaled
/// classes and 1.0 otherwise. Both floors are truncations; the 127/100 step is integer arithmetic, so
/// `v = 100, gain 1, m 1` gives 1.27. // SPEC: AUDIO_2009.md §4.1
pub fn stage_b(voice_volume: u8, class_gain: f32, master_scale: Option<f32>) -> f32 {
    let m = master_scale.unwrap_or(1.0);
    let first = (f32::from(voice_volume) * class_gain * m) as i32;
    let second = (first * 127) / 100;
    second as f32 * 0.01
}

/// Stage C: a driver master from a 0..=100 volume setting (clamped); `value * 0.01`.
/// // SPEC: AUDIO_2009.md §4.3
pub fn master(value: u16) -> f32 {
    f32::from(value.min(100)) * 0.01
}

/// Per-class gain and master-scaled flag table (16 classes, defaults 1.0 / master-scaled).
/// // SPEC: AUDIO_2009.md §4.2
#[derive(Clone, Debug, PartialEq)]
pub struct ClassTable {
    gain: [f32; crate::CLASS_COUNT],
    master_scaled: [bool; crate::CLASS_COUNT],
}

impl Default for ClassTable {
    fn default() -> Self {
        Self {
            gain: [1.0; crate::CLASS_COUNT],
            master_scaled: [true; crate::CLASS_COUNT],
        }
    }
}

impl ClassTable {
    /// Rejects an index above 15 or a gain outside `[0, 1]` (returns false, no change).
    pub fn set_gain(&mut self, class: usize, gain: f32) -> bool {
        if class >= crate::CLASS_COUNT || !(0.0..=1.0).contains(&gain) {
            return false;
        }
        self.gain[class] = gain;
        true
    }
    /// 1.0 for a bad index.
    pub fn gain(&self, class: usize) -> f32 {
        self.gain.get(class).copied().unwrap_or(1.0)
    }
    pub fn set_master_scaled(&mut self, class: usize, on: bool) -> bool {
        match self.master_scaled.get_mut(class) {
            Some(f) => {
                *f = on;
                true
            }
            None => false,
        }
    }
    /// true for a bad index.
    pub fn master_scaled(&self, class: usize) -> bool {
        self.master_scaled.get(class).copied().unwrap_or(true)
    }

    /// Voice level (stage B) for a voice of `class` at SFX setting `fx_level` (0..=100).
    pub fn level(&self, voice_volume: u8, class: usize, fx_level: u16) -> f32 {
        let m = self.master_scaled(class).then(|| master(fx_level));
        stage_b(voice_volume, self.gain(class), m)
    }
}

/// Total linear gain applied to a voice's samples before panning: `A (clamped >= 0) * B * C`.
/// For master-scaled classes `fx_level` is applied in B and again in C, so the result is proportional
/// to `fx_level^2` (up to the integer steps). // SPEC: AUDIO_2009.md §4.1
pub fn voice_gain(a: f32, b: f32, c: f32) -> f32 {
    a.max(0.0) * b * c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_a_boundaries_are_open() {
        let r = Radii::new(2.0, 10.0);
        // exactly at the inner and outer squared radii: unchanged
        assert_eq!(stage_a(0.7, 4.0, r, 100), 0.7);
        assert_eq!(stage_a(0.7, 100.0, r, 100), 0.7);
        // just inside the inner edge: ~1.0 * vol
        let a = stage_a(0.7, 4.01, r, 80);
        assert!((a - 0.8).abs() < 1e-3, "{a}");
    }

    #[test]
    fn stage_a_inner_zero_is_quadratic() {
        let r = Radii::new(0.0, 10.0);
        let a = stage_a(1.0, 25.0, r, 100); // d = 5
        assert!((a - 0.75).abs() < 1e-6, "{a}");
    }

    #[test]
    fn stage_a_span_is_not_difference_of_squares() {
        // inner 2, outer 10: span2 = 64, A hits 0 at d2 = 4 + 64 = 68 (< 100) and goes negative after.
        let r = Radii::new(2.0, 10.0);
        assert_eq!(r.span2, 64.0);
        let zero = stage_a(1.0, 68.0, r, 100);
        assert!(zero.abs() < 1e-6, "{zero}");
        let neg = stage_a(1.0, 90.0, r, 100);
        assert!(neg < 0.0);
        assert_eq!(voice_gain(neg, 1.0, 1.0), 0.0);
    }

    #[test]
    fn stage_a_keeps_last_value_outside() {
        let r = Radii::new(1.0, 5.0);
        let a = stage_a(1.0, 4.0, r, 100);
        assert!(a < 1.0);
        assert_eq!(stage_a(a, 400.0, r, 100), a);
        assert_eq!(stage_a(a, 0.5, r, 100), a);
    }

    #[test]
    fn stage_b_unity_is_1_27() {
        assert!((stage_b(100, 1.0, Some(1.0)) - 1.27).abs() < 1e-6);
        assert!((stage_b(100, 1.0, None) - 1.27).abs() < 1e-6);
    }

    #[test]
    fn stage_b_class_default_075() {
        // floor(75.0) = 75; 75 * 127 / 100 = 95 (integer); 0.95
        assert!((stage_b(100, 0.75, None) - 0.95).abs() < 1e-6);
        // v = 50, gain 1, m = 0.5 -> 25 -> 31 -> 0.31
        assert!((stage_b(50, 1.0, Some(0.5)) - 0.31).abs() < 1e-6);
    }

    #[test]
    fn double_master_for_classes_0_and_1_only() {
        let mut t = ClassTable::default();
        assert!(t.set_gain(0, 1.0));
        assert!(t.set_gain(1, 1.0));
        assert!(t.set_gain(2, 1.0));
        assert!(t.set_master_scaled(2, false)); // speech: master-scaled only through C
        let fx = 50u16;
        let c = master(fx);
        let g0 = voice_gain(1.0, t.level(100, 0, fx), c);
        let g2 = voice_gain(1.0, t.level(100, 2, fx), c);
        // class 0: B = floor(50)*127/100 = 63 -> 0.63, times C 0.5 = 0.315
        assert!((g0 - 0.315).abs() < 1e-5, "{g0}");
        // class 2: B = 1.27, times C 0.5 = 0.635
        assert!((g2 - 0.635).abs() < 1e-5, "{g2}");
        // full volume: both reach 1.27
        assert!((voice_gain(1.0, t.level(100, 0, 100), master(100)) - 1.27).abs() < 1e-6);
    }

    #[test]
    fn class_table_rules() {
        let mut t = ClassTable::default();
        assert!(!t.set_gain(16, 0.5));
        assert!(!t.set_gain(0, 1.5));
        assert!(!t.set_gain(0, -0.1));
        assert_eq!(t.gain(99), 1.0);
        assert!(t.master_scaled(99));
        assert_eq!(master(250), 1.0);
    }
    /// The radii of a footstep (outer 1200, inner 17 / 255 of it = 80): stage A never exceeds `volume / 100`, at
    /// the radii themselves it keeps its value, and a gain above 1 comes only from stage B.
    // EVIDENCE: engine+0xdb750 (A = (1 - t) * volume * 0.01 strictly inside (in2, out2), else no call: A unchanged)
    // DISASM: engine+0xa6839..0xa6875 (in2 = pow(inner, 2.0), out2 = pow(outer, 2.0), span2 = pow(outer - inner, 2.0))
    // EVIDENCE: engine+0xa63a0 (B = ((v * 127) & 0xffff) / 100 * 0.01: 1.27 at v 100 x class gain 1.0)
    #[test]
    fn stage_a_at_the_inner_and_outer_radii_of_a_footstep() {
        let (inner, outer) = (1200.0f32 * 17.0 / 255.0, 1200.0f32);
        let r = Radii::new(inner, outer);
        // exactly at the inner radius: unchanged (1.0 at the start); just outside it: 1.0 from below
        assert_eq!(stage_a(1.0, inner * inner, r, 100), 1.0);
        let a = stage_a(1.0, (inner + 0.01).powi(2), r, 100);
        assert!(a <= 1.0 && a > 0.9999, "{a}");
        // exactly at the outer radius: unchanged; A crosses 0 at sqrt(in2 + span2) < outer and is negative after
        assert_eq!(stage_a(0.3, outer * outer, r, 100), 0.3);
        let zero = (inner * inner + (outer - inner).powi(2)).sqrt();
        assert!((zero - 1122.85).abs() < 0.01, "{zero}");
        assert!(stage_a(1.0, (zero - 1.0).powi(2), r, 100) > 0.0);
        assert!(stage_a(1.0, (outer - 0.01).powi(2), r, 100) < 0.0);
        assert_eq!(
            voice_gain(stage_a(1.0, (outer - 0.01).powi(2), r, 100), 1.27, 1.0),
            0.0
        );
        // the whole range: A <= volume / 100
        for d in (0..=1300).step_by(5) {
            let d2 = (d as f32).powi(2);
            assert!(stage_a(1.0, d2, r, 100) <= 1.0, "{d}");
            assert!(stage_a(0.5, d2, r, 50) <= 0.5, "{d}");
        }
        // ~400 units: A = 0.8776. The client-audio verifier's gain 1.113 is A x B with class gain 1.0 (B = 1.27),
        // not the distance law; with the shell's class 0 gain (profile default 0.75, B = 0.95) it is 0.834.
        let a400 = stage_a(1.0, 400.0f32.powi(2), r, 100);
        assert!((a400 - 0.87755).abs() < 1e-4, "{a400}");
        let b_unit = stage_b(100, 1.0, Some(1.0));
        let b_default = stage_b(100, 0.75, Some(1.0));
        assert!((voice_gain(a400, b_unit, 1.0) - 1.1145).abs() < 1e-3);
        assert!((voice_gain(a400, b_default, 1.0) - 0.8337).abs() < 1e-3);
    }
}

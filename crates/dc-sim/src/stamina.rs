//! The sprint gauge (stamina) of the 2009 client: how long a sprint lasts and how fast it comes back.
//!
//! The 2009 client keeps the gauge in one object (the global at VA 0x3759e110): a float value, a mode and the
//! three rates of the own character. The mover's sampling sets the mode every frame, and the gauge element of
//! the health HUD advances the value by the real time since its last update:
//!
//! - mode **sprint** (the `SPRINT` bit was set this frame): `value -= t x drain`, not below 0;
//! - mode **spent** (forward, run and the sprint command held, but the gauge is empty): `value := 0`, so a held
//!   sprint key keeps the gauge empty until it is let go;
//! - mode **rest** (anything else): `value += t x regen`, not above `max`.
//!
//! A sprint starts only while the value truncated to an integer is above 0. The rates come from the character's
//! rows: `max = trunc(figure.Maxstamina x (1 + head.MaxStamina + goggles.MaxStamina + body.MaxStamina +
//! backpack.MaxStamina))`, `drain = figure.DashGauge`, `regen = figure.GaugeRec` (per second).
//!
//! Here the law runs once per simulation step with the step's real time (`frame_dt`), so the gauge is a function
//! of elapsed time only: the same at 30, 60 or 240 steps per second (frame-rate independent).
// EVIDENCE: cshell+0x95850 (the law: value slot +0xec, drain slot +0xe0, regen slot +0xe8, last time slot +0xd0 /
// +0xd4, mode slot +0xf0; mode 1: `v -= ms x drain x 0.001`, floor 0; mode 2: `v := 0`; else `v += ms x regen x
// 0.001`, cap max; then the value is stored back through slot +0x104), called from the health HUD draw
// cshell+0x96b30 while its gauge item (+0x40) exists
// DISASM: cshell+0x63d70 (slot +0xcc sprint start), cshell+0x63da0 (+0xd8 sprint off), cshell+0x640a0 /
// cshell+0x640b0 / cshell+0x640c0 / cshell+0x640d0 (getters of +0x174 / +0x178 / +0x17c / +0x180), cshell+0x64130
// (+0xf4 mode := arg), cshell+0x64120 (+0x104 value := arg) of the object with vtable VA 0x374d3874 (constructor
// cshell+0x63e60, destructor cshell+0x64550 clears VA 0x3759e110)
// DISASM: cshell+0x62d40 (init: drain / jump cost / regen / value := the inventory's +0x190 / +0x194 / +0x198 /
// +0x19c), cshell+0x63d50 (slot +0x108 refill: value := the inventory's max)
// DISASM: cshell+0x1d33d0 (the inventory's rates: +0x19c := trunc(figure Maxstamina x (head + 1, + goggles,
// + body, + backpack MaxStamina terms)), +0x190 := figure DashGauge, +0x194 := DashJumpGauge, +0x198 := GaugeRec)
// EVIDENCE: cshell+0x1ff980 (sampling: forward -> slot +0xec value truncated (cshell+0x3a5f00) > 0 with command
// 0x77 and RUN, not on a ladder -> SPRINT, mode 1; the same keys with an empty gauge -> mode 2 and the stamina-end
// notice; otherwise or without forward -> mode 0)
// OBSERVED: capture 20261025_000014_tdm1 (the real client's 0xca notices: two sprints from a
// rested gauge ended by the stamina path after 3.312 s and 3.313 s; re-sprints after short rests lasted about 0.77 x
// the rest)

/// The rates of one character (the 2009 inventory fields), integers as the client keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaminaRates {
    /// Full gauge.
    pub max: i32,
    /// Units per second while sprinting (figure `DashGauge`).
    pub drain: i32,
    /// Units per second at rest (figure `GaugeRec`).
    pub regen: i32,
}

/// What the gauge does this step (the 2009 mode slot +0xf0 / +0xf4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GaugeMode {
    /// Mode 0: refilling.
    #[default]
    Rest,
    /// Mode 1: draining (the `SPRINT` bit is set).
    Sprint,
    /// Mode 2: the sprint keys are held with an empty gauge: kept at 0.
    Spent,
}

/// The gauge state of one body.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stamina {
    /// Current value; `None` until the first step with rates (it then starts full).
    pub value: Option<f32>,
    pub mode: GaugeMode,
}

impl Stamina {
    /// A sprint may start: no gauge rule, or the value truncated to an integer is above 0.
    // EVIDENCE: cshell+0x1ff980 (slot +0xec, then the float-to-integer call, then `0 < value`)
    pub fn can_sprint(&self, rates: Option<StaminaRates>) -> bool {
        match (rates, self.value) {
            (None, _) => true,
            (Some(r), None) => r.max > 0,
            (Some(_), Some(v)) => (v as i32) > 0,
        }
    }

    /// Fills the gauge (spawn, round start).
    // DISASM: cshell+0x63d50 (slot +0x108: value := the inventory's max), called on shell 0x45 (cshell+0x49a40)
    pub fn refill(&mut self) {
        self.value = None;
    }

    /// Advances the gauge by `t` seconds in the current mode.
    // EVIDENCE: cshell+0x95850
    pub fn advance(&mut self, rates: Option<StaminaRates>, t: f32) {
        let Some(r) = rates else {
            return;
        };
        let max = r.max as f32;
        let v = self.value.unwrap_or(max);
        let v = match self.mode {
            GaugeMode::Sprint => {
                let n = v - t * r.drain as f32;
                if n < 0.0 {
                    0.0
                } else {
                    n
                }
            }
            GaugeMode::Spent => 0.0,
            GaugeMode::Rest => {
                let n = t * r.regen as f32 + v;
                if max < n {
                    max
                } else {
                    n
                }
            }
        };
        self.value = Some(v);
    }

    /// The value for display and checks (full when not started; `None` without a gauge rule).
    pub fn level(&self, rates: Option<StaminaRates>) -> Option<f32> {
        rates.map(|r| self.value.unwrap_or(r.max as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // made-up rates (not ROM values)
    const R: StaminaRates = StaminaRates {
        max: 100,
        drain: 30,
        regen: 20,
    };

    #[test]
    fn sprint_drains_by_time_and_stops_at_zero() {
        let mut s = Stamina {
            mode: GaugeMode::Sprint,
            ..Stamina::default()
        };
        s.advance(Some(R), 1.0);
        assert_eq!(s.value, Some(70.0));
        s.advance(Some(R), 10.0);
        assert_eq!(s.value, Some(0.0));
        assert!(!s.can_sprint(Some(R)));
    }

    #[test]
    fn rest_refills_up_to_max_and_spent_holds_zero() {
        let mut s = Stamina {
            value: Some(0.5),
            mode: GaugeMode::Spent,
        };
        s.advance(Some(R), 1.0);
        assert_eq!(
            s.value,
            Some(0.0),
            "a held sprint key on an empty gauge keeps it empty"
        );
        s.mode = GaugeMode::Rest;
        s.advance(Some(R), 0.025);
        // 0.5 units: truncated to 0, no sprint yet
        assert!(!s.can_sprint(Some(R)));
        s.advance(Some(R), 0.025);
        assert!(s.can_sprint(Some(R)));
        s.advance(Some(R), 100.0);
        assert_eq!(s.value, Some(100.0));
    }

    #[test]
    fn no_rates_means_no_gauge() {
        let mut s = Stamina {
            mode: GaugeMode::Sprint,
            ..Stamina::default()
        };
        s.advance(None, 100.0);
        assert!(s.can_sprint(None));
        assert_eq!(s.level(None), None);
    }

    #[test]
    fn the_law_depends_on_elapsed_time_only() {
        // 2 s of sprint in 1 / 30, 1 / 60 and 1 / 240 s steps: the same value (within binary32 rounding)
        let run = |hz: u32| {
            let mut s = Stamina {
                mode: GaugeMode::Sprint,
                ..Stamina::default()
            };
            for _ in 0..2 * hz {
                s.advance(Some(R), 1.0 / hz as f32);
            }
            s.value.unwrap()
        };
        for hz in [30, 60, 240] {
            assert!((run(hz) - 40.0).abs() < 1e-3, "{hz} Hz: {}", run(hz));
        }
    }
}

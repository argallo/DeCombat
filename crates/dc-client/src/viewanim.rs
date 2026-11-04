//! The first-person model's sprint animation: while the local player sprints and the weapon is otherwise idle, the
//! held weapon's first-person model plays its sprint-entry clip once, then its sprint loop; when the sprint ends it
//! plays its sprint-exit clip once and returns to the idle clip. Sans-IO: the state advances on the session clock
//! (milliseconds), so the clip times are the same at every frame rate.
//!
//! The 2009 rule (CShell, the client weapon object):
//! - the three clips are looked up by name on the first-person model next to select / reload / idle / fire and kept
//!   in three slots of the weapon object (an absent clip = none);
//! - every weapon update asks the movement object whether the player sprints;
//! - sprinting: a running reload or select finishes first, a playing fire clip finishes first; then, unless the
//!   entry clip or the loop is already playing, the entry clip starts (play once); when the entry clip is done the
//!   loop starts (looping). An exit clip that is still playing is cut by a new entry clip. Without an entry clip no
//!   sprint clip plays (the loop is only ever started after the entry clip);
//! - not sprinting while an entry, loop or exit clip plays: the exit clip starts once (if the weapon has one,
//!   otherwise the idle clip at once); when it is done the weapon goes back to its idle state and clip.
// EVIDENCE: cshell+0x233ec0 (the clip lookups by name into weapon +0x12c / +0x130 / +0x134, -1 = none)
// EVIDENCE: cshell+0x238a20 (entry clip once unless entry / loop / exit plays or forced; loop when the entry is done)
// EVIDENCE: cshell+0x23ac50 (the weapon update, vtable slot +0x14c: sprint query; leaving: exit clip, then idle when it
// is done; reload / select states finish first; a fire clip that is not done keeps playing)
// EVIDENCE: cshell+0x64090 (the sprint query returns the movement object's sprint flag +0x164)
// EVIDENCE: cshell+0x2304f0 (the "exit clip playing" test that forces a new entry clip)

/// Which sprint clip plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SprintPart {
    /// The entry clip (played once).
    Enter,
    /// The loop (repeats while the sprint lasts).
    Loop,
    /// The exit clip (played once).
    Exit,
}

/// The lengths (ms) of a weapon's three sprint clips; `None` = the model has no such clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SprintLens {
    pub enter: Option<u32>,
    pub run: Option<u32>,
    pub exit: Option<u32>,
}

/// The sprint clip state of the held weapon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SprintAnim {
    /// The playing part and the session time it started.
    part: Option<(SprintPart, u32)>,
}

impl SprintAnim {
    /// The playing sprint part and the time into its clip at `now` (ms), `None` when no sprint clip plays. A play-once
    /// part is held at its last frame (the caller's pose wraps a time at the clip length, so the held time stays
    /// just inside the clip).
    pub fn playing(&self, now: u32, lens: &SprintLens) -> Option<(SprintPart, f32)> {
        let (part, at) = self.part?;
        let t = now.saturating_sub(at) as f32;
        let once = |len: Option<u32>| match len {
            Some(l) => t.clamp(0.0, (l as f32 - crate::anims::HOLD_BEFORE_END_MS).max(0.0)),
            None => t,
        };
        Some(match part {
            SprintPart::Enter => (part, once(lens.enter)),
            SprintPart::Loop => (part, t),
            SprintPart::Exit => (part, once(lens.exit)),
        })
    }

    /// Clears the state (spawn, death, weapon change, a reload or select that took over the model).
    pub fn reset(&mut self) {
        self.part = None;
    }

    /// One weapon update at `now`: `sprinting` is the frame's sprint state, `idle` whether the weapon is free for the
    /// sprint clips (no select, reload, fire or throw clip running). Transitions happen at the exact clip ends on
    /// the session clock, so a long frame passes through the entry clip into the loop the same way short frames do.
    pub fn step(&mut self, now: u32, sprinting: bool, idle: bool, lens: &SprintLens) {
        if !idle {
            // the weapon's own clip owns the model; the sprint clips start again once it is done
            self.part = None;
            return;
        }
        if sprinting {
            match self.part {
                None | Some((SprintPart::Exit, _)) => {
                    self.part = lens.enter.map(|_| (SprintPart::Enter, now));
                }
                _ => {}
            }
            if let (Some((SprintPart::Enter, at)), Some(len)) = (self.part, lens.enter) {
                let end = at.saturating_add(len);
                if now >= end && lens.run.is_some() {
                    self.part = Some((SprintPart::Loop, end));
                }
            }
        } else {
            match self.part {
                Some((SprintPart::Enter | SprintPart::Loop, _)) => {
                    self.part = lens.exit.map(|_| (SprintPart::Exit, now));
                }
                Some((SprintPart::Exit, at))
                    if now >= at.saturating_add(lens.exit.unwrap_or(0)) =>
                {
                    self.part = None;
                }
                Some((SprintPart::Exit, _)) | None => {}
            }
        }
    }

    /// True while the sprint state still holds the weapon after the sprint ended: the entry clip or the loop is
    /// left for the exit clip (the frame the sprint key goes up) or the exit clip has not reached its end at `now`.
    /// The fire key does not count meanwhile; without an exit clip the weapon is free at once.
    // EVIDENCE: cshell+0x23ac50 (sprint state 0xe, not sprinting: an entry / loop clip starts the exit clip and stays
    // in 0xe; a playing exit clip that is not done stays in 0xe; done (or no exit clip) -> state 1 and the idle clip;
    // the 0xe handler runs no fire step)
    pub fn holds_weapon(&self, now: u32, lens: &SprintLens) -> bool {
        match self.part {
            Some((SprintPart::Enter | SprintPart::Loop, _)) => lens.exit.is_some(),
            Some((SprintPart::Exit, at)) => now < at.saturating_add(lens.exit.unwrap_or(0)),
            None => false,
        }
    }

    /// True while a sprint clip plays.
    pub fn active(&self) -> bool {
        self.part.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LENS: SprintLens = SprintLens {
        enter: Some(233),
        run: Some(620),
        exit: Some(333),
    };

    fn run(frames: &[(u32, bool)]) -> Vec<Option<(SprintPart, f32)>> {
        let mut s = SprintAnim::default();
        frames
            .iter()
            .map(|&(t, sp)| {
                s.step(t, sp, true, &LENS);
                s.playing(t, &LENS)
            })
            .collect()
    }

    #[test]
    fn enter_then_loop_then_exit_then_idle() {
        let out = run(&[
            (0, false),
            (100, true),
            (200, true),
            (333, true),
            (1000, true),
            (1100, false),
            (1300, false),
            (1433, false),
        ]);
        assert_eq!(out[0], None);
        assert_eq!(out[1], Some((SprintPart::Enter, 0.0)));
        assert_eq!(out[2], Some((SprintPart::Enter, 100.0)));
        // the entry clip ended at 333 exactly: the loop starts there
        assert_eq!(out[3], Some((SprintPart::Loop, 0.0)));
        assert_eq!(out[4], Some((SprintPart::Loop, 667.0)));
        assert_eq!(out[5], Some((SprintPart::Exit, 0.0)));
        assert_eq!(out[6], Some((SprintPart::Exit, 200.0)));
        assert_eq!(
            out[7], None,
            "back to the idle clip when the exit clip is done"
        );
    }

    #[test]
    fn frame_rate_independent() {
        // the same sprint sampled every 7 ms and every 100 ms gives the same part and clip time at shared instants
        let fine: Vec<(u32, bool)> = (0..=300u32).map(|i| (i * 7, i * 7 < 1500)).collect();
        let coarse: Vec<(u32, bool)> = (0..=21u32).map(|i| (i * 100, i * 100 < 1500)).collect();
        let a = run(&fine);
        let b = run(&coarse);
        for (i, &(t, _)) in coarse.iter().enumerate() {
            if let Some(j) = fine.iter().position(|f| f.0 == t) {
                if t > 0 && t != 1500 {
                    assert_eq!(a[j], b[i], "at {t} ms");
                }
            }
        }
        // a hitch longer than the entry clip lands in the loop with the right phase
        let out = run(&[(0, true), (1000, true)]);
        assert_eq!(out[1], Some((SprintPart::Loop, 767.0)));
    }

    #[test]
    fn exit_is_cut_by_a_new_sprint_and_busy_weapon_resets() {
        let mut s = SprintAnim::default();
        s.step(0, true, true, &LENS);
        s.step(400, false, true, &LENS);
        assert_eq!(s.playing(400, &LENS), Some((SprintPart::Exit, 0.0)));
        s.step(500, true, true, &LENS);
        assert_eq!(s.playing(500, &LENS), Some((SprintPart::Enter, 0.0)));
        // a reload takes the model: no sprint clip until the weapon is free again
        s.step(600, true, false, &LENS);
        assert!(!s.active());
        s.step(2000, true, true, &LENS);
        assert_eq!(s.playing(2000, &LENS), Some((SprintPart::Enter, 0.0)));
    }

    #[test]
    fn missing_clips() {
        // no entry clip: no sprint clip at all
        let no_enter = SprintLens {
            enter: None,
            ..LENS
        };
        let mut s = SprintAnim::default();
        s.step(0, true, true, &no_enter);
        s.step(1000, true, true, &no_enter);
        assert!(!s.active());
        // no loop: the entry clip holds its last frame
        let no_loop = SprintLens { run: None, ..LENS };
        let mut s = SprintAnim::default();
        s.step(0, true, true, &no_loop);
        s.step(1000, true, true, &no_loop);
        assert_eq!(s.playing(1000, &no_loop), Some((SprintPart::Enter, 232.5)));
        // no exit clip: straight back to idle
        let no_exit = SprintLens { exit: None, ..LENS };
        let mut s = SprintAnim::default();
        s.step(0, true, true, &no_exit);
        s.step(300, false, true, &no_exit);
        assert!(!s.active());
    }
}

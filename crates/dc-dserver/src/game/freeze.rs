//! DSV-77: the players stop at the round result. With `GS_ROUND_RESULT` every living, introduced player gets the
//! mover freeze (`GS_MOVER_UPDATE` block 0x0080, bit 1); one whose last accepted trailer was airborne is frozen at
//! his first grounded trailer instead, at the latest `freeze_wait_ms` after the result (a freeze in the air would
//! hang him there). A later spawn of a frozen player sends the freeze bit 0 before the spawn set's teleport.
// SPEC: MOVEMENT_2009.md §12.4 (flag 0x0080 `[freeze:1 bit]`: the client stores its position and re-applies it
// every frame until a 0), GAME_MESSAGES_2009_S2C.md §2.1 (0x46)
// EVIDENCE: cshell+0x1ff430 (flag 0x0080: position stored at mover+0x33c, mover+0x338 := bit), cshell+0x1ff3f0
// (every frame: back to the stored position unless mover+0x348)
// OBSERVED: capture 20261025_000014_tdm1 c5 seq 410 (`0d a0 80 00 01` then `0d a0 80 00 00` in
// the spawn set; the real client accepted both; REFERENCE)
// project decision (rust7: "it still lets me move around when it says bravo team wins"); DC-DEFAULT: DSV-77
// (reinstated, proposed)

use super::{Game, Life, RoundPhase};
use crate::gs;
use dc_net_engine::time::later;
use dc_net_engine::Millis;

/// DSV-77 on by default.
// DC-DEFAULT: DSV-77 (reinstated, proposed) on
pub const ROUND_END_FREEZE: bool = true;
/// The longest wait for an airborne player's landing before he is frozen where he is.
// DC-DEFAULT: DSV-77 (reinstated, proposed) 1000 ms
pub const FREEZE_WAIT_MS: u32 = 1_000;

/// The mover freeze bit alone.
fn freeze_record(on: bool) -> dc_wire::udp::BitWriter {
    gs::mover_update(&gs::MoverUpdate {
        freeze: Some(on),
        ..gs::MoverUpdate::default()
    })
}

impl Game {
    /// With the round result: freeze every living, introduced player now, or mark an airborne one to be frozen at
    /// his landing (or when the wait runs out). Dead players are left as they are.
    pub(super) fn freeze_at_round_end(&mut self, now: Millis) {
        if !self.cfg.round_end_freeze {
            return;
        }
        let wait = self.cfg.freeze_wait_ms;
        let mut now_list = Vec::new();
        for p in self.players.values_mut() {
            if !p.introduced || p.life != Life::Alive || p.frozen {
                continue;
            }
            if p.fall.is_some_and(|f| f.airborne) && wait > 0 {
                p.freeze_due = Some(now.wrapping_add(wait));
            } else {
                now_list.push(p.pid);
            }
        }
        for pid in now_list {
            self.freeze_player(pid, "at the round result");
        }
    }

    /// Sends the freeze 1 to `pid` (living players only) and records it.
    fn freeze_player(&mut self, pid: u8, why: &str) {
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        p.freeze_due = None;
        if p.life != Life::Alive || p.frozen {
            return;
        }
        p.frozen = true;
        self.send(pid, freeze_record(true));
        self.log(format!("client {pid}: frozen {why} (DSV-77)"));
    }

    /// After an accepted trailer: a player waiting for his landing is frozen once the trailer is grounded.
    pub(super) fn freeze_on_landing(&mut self, pid: u8) {
        let landed = self
            .players
            .get(&pid)
            .is_some_and(|p| p.freeze_due.is_some() && !p.fall.is_some_and(|f| f.airborne));
        if landed {
            self.freeze_player(pid, "at the landing after the round result");
        }
    }

    /// Every tick after the result: a player still airborne when his wait ran out is frozen where he is.
    pub(super) fn tick_freeze_wait(&mut self, now: Millis) {
        if !matches!(self.phase, RoundPhase::Over { .. }) {
            return;
        }
        let due: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.freeze_due.is_some_and(|t| !later(t, now)))
            .map(|p| p.pid)
            .collect();
        for pid in due {
            self.freeze_player(pid, "after the landing wait");
        }
    }

    /// At a spawn: a frozen player is released first (freeze 0, before the spawn set's teleport); a pending freeze
    /// is dropped. Returns whether the release went out.
    pub(super) fn release_for_spawn(&mut self, pid: u8) -> bool {
        let Some(p) = self.players.get_mut(&pid) else {
            return false;
        };
        p.freeze_due = None;
        if !std::mem::take(&mut p.frozen) {
            return false;
        }
        self.send(pid, freeze_record(false));
        self.log(format!("client {pid}: released for the spawn (DSV-77)"));
        true
    }

    /// DSV-300: a plant or defuse holds the player in place: the mover freeze goes out with the hold's 0xad and the
    /// release when the hold ends alive (let go, broken, completed). A hold that ends with the round leaves the
    /// player frozen like the round-end stop (released at his next spawn).
    // SPEC: MOVEMENT_2009.md §12.4 (the mover freeze), GAME_MODES_2009.md §4.5 (the hold)
    // EVIDENCE: cshell+0x275ad0 (the 2009 hold ends when the use command goes off or the player is no longer in the
    // target's zone), cshell+0x48a70 (no weapon change while the hold runs)
    // DC-DEFAULT: DSV-300 (proposed, playtest report "planting allows the player to still move around")
    pub(super) fn freeze_for_hold(&mut self, pid: u8) {
        self.freeze_player(pid, "for the plant / defuse hold (DSV-300)");
    }

    /// DSV-300: the hold ended: a living, frozen holder gets the freeze 0 (not after the round result).
    pub(super) fn release_hold_freeze(&mut self, pid: u8) {
        if !matches!(self.phase, RoundPhase::Running { .. }) {
            return;
        }
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        if p.life != Life::Alive || !std::mem::take(&mut p.frozen) {
            return;
        }
        self.send(pid, freeze_record(false));
        self.log(format!("client {pid}: released after the hold (DSV-300)"));
    }
}

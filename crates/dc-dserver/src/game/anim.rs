//! The hooks of the upper-body table (DSV-125): the held weapon's animation family, the play-once actions the
//! client's messages start, the hit reaction (DSV-127) and the ground state for the jump clip.
// DC-DEFAULT: DSV-125 (proposed); DSV-126 (proposed); DSV-127 (proposed; on by default since 2026-10-26)

use super::Game;
use crate::anims::Act;
use dc_net_engine::time::since;
use dc_net_engine::Millis;

impl Game {
    /// The weapon's `AniType` (the ROM weapon row's key).
    pub(super) fn ani_of(&self, weapon: u16) -> Option<u8> {
        self.tables.weapons.weapons.get(&weapon)?.ani_type
    }

    /// Starts an action on the player's upper body (no-op without a character or when the table is off).
    pub(super) fn pose_action(&mut self, pid: u8, act: Act, now: Millis) -> bool {
        if !self.cfg.directional_anims {
            return false;
        }
        let Some(p) = self.players.get_mut(&pid) else {
            return false;
        };
        let Some(c) = p.character.as_ref() else {
            return false;
        };
        p.pose.start(act, &c.anims.table, p.motion, now)
    }

    /// `UP_FIRE`: the release of a held throw (an `UP_FIRE` inside the throw window) plays the throw phase, any
    /// other the fire one-shot (restarted by every pellet message).
    // SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (the release is the 0x82 after the last 0x4d)
    // OBSERVED: capture 20261025_000014_tdm1 (hold phase on the 0x4d, release phase on the
    // throw, fire one-shot per shot; REFERENCE)
    pub(super) fn pose_on_fire(&mut self, pid: u8, now: Millis) {
        let throw = self.players.get(&pid).is_some_and(|p| {
            p.last_throw
                .is_some_and(|t| since(now, t) <= super::THROW_FIRE_WINDOW_MS)
        });
        self.pose_action(pid, if throw { Act::Throw } else { Act::Fire }, now);
    }

    /// `UP_WEAPON_CHANGE`: the new weapon's family, and its select one-shot (the reference stack's still / moving select
    /// clips on each change, also the one right after the spawn).
    // OBSERVED: capture 20261025_000014_tdm1 (select one-shot on every change; REFERENCE)
    pub(super) fn pose_weapon_changed(&mut self, pid: u8, weapon: u16, now: Millis) {
        let drawn = self
            .players
            .get(&pid)
            .and_then(|p| p.current.and_then(|i| p.weapons.get(i)).map(|w| w.weapon))
            .unwrap_or(weapon);
        let ani = self.ani_of(drawn);
        if let Some(p) = self.players.get_mut(&pid) {
            if p.pose.ani != ani {
                p.pose.stop();
            }
            p.pose.ani = ani;
        }
        self.pose_action(pid, Act::Select, now);
    }

    /// A hit that took health and did not kill: the hit reaction once on the victim's upper body when `hit_clip` is
    /// on (the default; the back clip for a shot travelling along the victim's facing).
    // DC-DEFAULT: DSV-127 (proposed; project decision) on by default
    pub(super) fn pose_on_hit(&mut self, vpid: u8, dir: [f32; 3], now: Millis) {
        if !self.cfg.hit_clip {
            return;
        }
        let back = self
            .players
            .get(&vpid)
            .is_some_and(|p| hit_from_behind(p.yaw, dir));
        self.pose_action(vpid, Act::Hit(back), now);
    }
}

/// True when a shot travelling along `dir` struck a victim facing `yaw` from behind (it travels the way the victim
/// faces); the victim's forward vector for a facing `yaw` about +Y is (sin, 0, cos).
// DC-DEFAULT: DSV-127 (proposed) the back clip for a shot from behind
pub(super) fn hit_from_behind(yaw: f32, dir: [f32; 3]) -> bool {
    dir[0] * yaw.sin() + dir[2] * yaw.cos() > 0.0
}

#[cfg(test)]
mod tests {
    use super::hit_from_behind;

    #[test]
    fn a_shot_along_the_facing_is_from_behind() {
        // facing +Z (yaw 0): a shot travelling +Z came from behind, one travelling -Z from the front
        assert!(hit_from_behind(0.0, [0.0, 0.0, 1.0]));
        assert!(!hit_from_behind(0.0, [0.0, 0.0, -1.0]));
        // facing +X (yaw 90 deg)
        let q = std::f32::consts::FRAC_PI_2;
        assert!(hit_from_behind(q, [1.0, 0.0, 0.0]));
        assert!(!hit_from_behind(q, [-1.0, -0.2, 0.0]));
        // straight from the side or from above: the front clip
        assert!(!hit_from_behind(0.0, [1.0, 0.0, 0.0]));
        assert!(!hit_from_behind(0.0, [0.0, -1.0, 0.0]));
    }
}

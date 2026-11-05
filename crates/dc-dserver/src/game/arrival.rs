//! Round-2 rules around a spawn and the match start: the spawn placed on the floor below the spawn point (DSV-48),
//! the slot status of the melee and throwable slots after each spawn set (DSV-49), the match intro of `GS_MATCH_BEGIN`
//! from the room line (DSV-50) and the death in a kill volume (DSV-52).

use super::{Game, Life, WeaponState};
use crate::gs;
use dc_net_engine::Millis;

/// How far a spawn is lifted before it is dropped (clears a spawn point that lies a little inside the floor) and how
/// far it may drop at most.
// DC-DEFAULT: DSV-48 (proposed) lift 18, drop at most 1000 units
pub const SPAWN_LIFT: f32 = 18.0;
pub const SPAWN_MAX_DROP: f32 = 1000.0;

/// The weapon slots whose status 0x66 kind 1 follows the spawn set: the melee slot and the throwable slot.
// OBSERVED: capture 20261025_065016_tdm6 (after each spawn set the reference stack sent 0x66 kind 1 for
// slots 2 and 3 with the granted weapon, ammunition kind and rounds and `extra` 1, before health and armour; REFERENCE,
// accepted by the real client)
// DC-DEFAULT: DSV-49 (proposed)
pub const STATUS_SLOTS: [u8; 2] = [2, 3];
/// `extra` of those status messages.
// OBSERVED: tdm6 (`extra` 1 in every slot status after a spawn; meaning UNPROVEN, GAME_MESSAGES_2009_S2C.md §4.3)
pub const TODO_SLOT_STATUS_EXTRA: u16 = 1;

/// Weapon number of the kill feed line of a kill-volume death (no killer): the victim's drawn weapon, else this.
// DC-DEFAULT: DSV-52 (proposed)
pub const VOLUME_DEATH_WEAPON: u16 = 0;
/// Damage type (0x74 `zone`) of a kill-volume death.
// DC-DEFAULT: DSV-52 (proposed) 0
pub const VOLUME_DEATH_TYPE: u8 = 0;

/// The capital letters of a mode filter text (the form a mode key abbreviates it).
fn mode_initials(text: &[u8]) -> Vec<u8> {
    text.iter()
        .copied()
        .filter(u8::is_ascii_uppercase)
        .collect()
}

/// `x,y,z` text of the room line as a vector.
fn vec3_text(s: &str) -> Option<[f32; 3]> {
    let v: Vec<f32> = s
        .split(',')
        .map(|x| x.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .ok()?;
    (v.len() == 3 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2]])
}

/// The DeCombat mode names of a `modes` filter text, when the text is one: it contains a comma, or equals one of
/// `dc_config::modes::MODE_NAMES` ignoring ASCII case. The names are the comma-separated words, trimmed, in order
/// (an empty word is dropped). Any other text (a ROM world's filter) is `None`.
// DC-DEFAULT: MAP-21
pub fn dc_mode_list(filter: &[u8]) -> Option<Vec<String>> {
    let text = String::from_utf8_lossy(filter);
    let is_list = text.contains(',')
        || dc_config::modes::MODE_NAMES
            .iter()
            .any(|n| n.eq_ignore_ascii_case(text.trim()));
    is_list.then(|| {
        text.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    })
}

impl Game {
    /// Whether a spawn point's mode filter text names the room's mode: its capital letters equal the room's mode key
    /// (any letter case of the key). A point without a filter does not match. The ROM rule; a DeCombat list is
    /// decided by [`Game::dc_mode_serves`] first.
    // DC-DEFAULT: DSV-50 (proposed) the initials rule; DSV-59 (proposed) the same rule for the spawn choice
    pub(super) fn mode_matches(&self, filter: Option<&[u8]>) -> bool {
        let key = self.room.mode_key().to_ascii_uppercase();
        filter.is_some_and(|m| mode_initials(m) == key.as_bytes())
    }

    /// The room's DeCombat mode name, derived from the room line's `type=` class (`rules.mode_class`); `None` for a
    /// class no name maps to (class 10, the generic class 0).
    // DC-DEFAULT: MAP-6 (the name of a class), MAP-21 (no new room-line key)
    pub fn dc_mode_name(&self) -> Option<&'static str> {
        dc_config::modes::mode_name_of_class(u32::from(self.rules.mode_class))
    }

    /// The DeCombat rule for a `modes` filter against the room `room_name` (a DeCombat mode name): `Some(true)` /
    /// `Some(false)` when the rule decides, `None` when the filter is a ROM text (the initials rule applies). A
    /// filter that is a DeCombat list ([`dc_mode_list`]) serves the room when any of its names does
    /// (`dc_config::modes::serves`: equal names, `_CLAN` twins, a `BOMB` set for an `ELIM` room). An EMPTY or absent
    /// filter on a vocabulary object (`dc_object`) serves every mode; on a ROM object it serves none (the ROM
    /// rule).
    // DC-DEFAULT: MAP-21
    pub fn dc_mode_serves(
        filter: Option<&[u8]>,
        dc_object: bool,
        room_name: Option<&str>,
    ) -> Option<bool> {
        let text = filter.unwrap_or(b"");
        if text.trim_ascii().is_empty() {
            return dc_object.then_some(true);
        }
        let names = dc_mode_list(text)?;
        Some(room_name.is_some_and(|room| names.iter().any(|n| dc_config::modes::serves(n, room))))
    }

    /// A spawn point moved down onto the floor of the level geometry: the player's box rests on the first solid below
    /// (lifted by [`SPAWN_LIFT`] first when that does not overlap), not at the point's own height. Without the level
    /// geometry, or with nothing below, the point is kept.
    // SPEC: MOVEMENT_2009.md §12.4 flag 0x0020 (the server places the body with the teleport); dc-sim `drop_to_floor`
    // OBSERVED: capture 20261025_081704_rust3 (spawned at the raw spawn-point height, the real
    // client fell 36 units in 0.27 s at every spawn); 20261025_065016_tdm6 (the reference stack's teleports to the same points
    // at the floor height; the height is decided by our own collision model, not taken from the reference stack)
    // DC-DEFAULT: DSV-48 (proposed) on (`-rawspawn` off)
    pub fn floor_point(&self, at: [f32; 3], dims: [f32; 3]) -> [f32; 3] {
        if !self.cfg.spawn_on_floor {
            return at;
        }
        let Some(w) = self.trace.as_deref() else {
            return at;
        };
        let shape = dc_sim::Cylinder::from_dims(dc_sim::V3::from_array(dims));
        let p = dc_sim::motion::drop_to_floor(
            w,
            &shape,
            dc_sim::V3::from_array(at),
            SPAWN_LIFT,
            SPAWN_MAX_DROP,
        );
        let out = [p.x, p.y, p.z];
        if out.iter().all(|c| c.is_finite()) {
            out
        } else {
            at
        }
    }

    /// 0x66 kind 1 for the melee and throwable slots of a spawn set: the granted weapon, its ammunition kind and the
    /// rounds of the grant.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.3 (kind 1 `[weapon][ammo_kind][ammo][slot][extra]`, cshell+0x60dda: the slot's
    // weapon and ammunition count)
    // DC-DEFAULT: DSV-49 (proposed)
    pub(super) fn slot_status(&mut self, pid: u8, weapons: &[WeaponState]) {
        for w in weapons.iter().filter(|w| STATUS_SLOTS.contains(&w.slot)) {
            self.send(
                pid,
                gs::player_ammo(
                    w.weapon,
                    w.ammo,
                    w.told_amount(), // the rounds the client was told (clip + stock; a lone grenade = 1.0, not 0.0)
                    w.slot,
                    TODO_SLOT_STATUS_EXTRA,
                ),
            );
        }
    }

    /// The intro block of the round's first spawn (tests and tools).
    pub fn match_intro(&self) -> gs::Intro {
        self.intro_block()
    }

    /// The intro block from the room line our lobby writes from the room's mode row (keys `obj`, `cmt`, `large`,
    /// `radar`): mission caption = `obj`, mission text = the first field of `cmt`, large map = `large` fields 0, 1 (the
    /// two pictures), 2, 3 (the corners), radar = `radar` fields 0 (picture), 1, 2 (corners), 3 (zoom), 4 (rotation),
    /// 5, 6 (the two sizes). A missing or malformed field keeps the fallback of [`Game::intro_block`].
    // SPEC: GAME_MESSAGES_2009_S2C.md §2.1.1 (intro block rows b-h); DSERVER_PIPE.md §3.2 (room line)
    // OBSERVED: capture 20261025_065016_tdm6 and 20261025_002236_tdm4 (the reference stack's intro to the
    // round's first spawn: the same strings and numbers the room line of that map carries in our lobby's room line,
    // REFERENCE, accepted); capture 20261025_081704_rust3 (our 0x45 without the intro)
    // DC-DEFAULT: DSV-50 (proposed)
    pub(super) fn apply_room_intro(&self, i: &mut gs::Intro) {
        let field = |k: &str, n: usize| -> Option<String> {
            self.room
                .get(k)
                .and_then(|v| v.split('|').nth(n).map(|s| s.trim().to_string()))
        };
        if let Some(s) = self
            .room
            .get("obj")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            i.notice_head = s.as_bytes().to_vec();
        }
        if let Some(s) = field("cmt", 0).filter(|s| !s.is_empty()) {
            i.notice_body = s.into_bytes();
        }
        if let Some(s) = field("large", 0).filter(|s| !s.is_empty()) {
            i.map_image_a = s.into_bytes();
        }
        if let Some(s) = field("large", 1).filter(|s| !s.is_empty()) {
            i.map_image_b = s.into_bytes();
        }
        if let (Some(a), Some(b)) = (
            field("large", 2).and_then(|s| vec3_text(&s)),
            field("large", 3).and_then(|s| vec3_text(&s)),
        ) {
            i.map_min = a;
            i.map_max = b;
        }
        if let Some(s) = field("radar", 0).filter(|s| !s.is_empty()) {
            i.radar_image = s.into_bytes();
        }
        if let (Some(a), Some(b)) = (
            field("radar", 1).and_then(|s| vec3_text(&s)),
            field("radar", 2).and_then(|s| vec3_text(&s)),
        ) {
            i.radar_min = a;
            i.radar_max = b;
        }
        let real =
            |n| field("radar", n).and_then(|s| s.parse::<f32>().ok().filter(|x| x.is_finite()));
        let int = |n| field("radar", n).and_then(|s| s.parse::<u32>().ok());
        if let Some(z) = real(3) {
            i.radar_zoom = z;
        }
        if let Some(r) = real(4) {
            i.radar_rot = r;
        }
        if let Some(p) = int(5) {
            i.radar_p3 = p;
        }
        if let Some(p) = int(6) {
            i.radar_p4 = p;
        }
        // the team bases: the centre of the team's spawn points of the room's mode, the mode matched by the capital
        // letters of the spawn point's mode filter text against the room's mode key
        // OBSERVED: tdm4 / tdm6 (the reference stack's team bases equal the centres of the team's spawn points whose mode filter
        // names team deathmatch, on both worlds; REFERENCE)
        // DC-DEFAULT: DSV-50 (proposed) the initials rule; without a matching spawn point the base of
        // [`Game::intro_block`] stays
        // DC-DEFAULT: DSV-470 (project decision) the points of the room's spawn set (Search and Destroy and
        // Elimination Pro: the S&D set), so the base shown is where the team spawns
        let set = self.spawn_set();
        for (team, base) in [(1u8, &mut i.base_team1), (2u8, &mut i.base_team2)] {
            let pts: Vec<[f32; 3]> = self
                .spawns
                .iter()
                .zip(&set)
                .filter(|(s, m)| s.2 == Some(team) && **m)
                .map(|(s, _)| s.0)
                .collect();
            if pts.is_empty() {
                continue;
            }
            // summed in f32 in the world's order, then times the f32 reciprocal of the count
            // OBSERVED: capture 20261025_065016_tdm6, 20261025_002236_tdm4 (this form gives the
            // reference stack's four team bases bit for bit; an f64 sum with one division missed tdm4 by one f32 step;
            // tests/round2_rom.rs)
            let mut c = [0f32; 3];
            for p in &pts {
                for k in 0..3 {
                    c[k] += p[k];
                }
            }
            let inv = 1.0 / pts.len() as f32;
            *base = c.map(|v| v * inv);
        }
    }

    /// A living player whose position lies in a kill volume dies at once: no killer, the drawn weapon in the kill
    /// feed, the usual damage, kill feed, death states and death presentation.
    // SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 2 (`killer` 0xff = no killer: world / fall; the victim's death is
    // counted, no kill)
    // DC-DEFAULT: DSV-52 (proposed) kill volumes act on the server
    pub fn volume_death(&mut self, pid: u8, now: Millis) {
        let Some(p) = self.players.get(&pid) else {
            return;
        };
        if p.life != Life::Alive || !matches!(self.phase, super::RoundPhase::Running { .. }) {
            return;
        }
        let weapon = p.hand_weapon.unwrap_or(VOLUME_DEATH_WEAPON);
        // more than health and armour together: lethal whatever the armour absorbs (DSV-30)
        let lethal = p.health + p.armor + 1.0;
        self.log(format!("client {pid}: died in a kill volume"));
        let src = super::combat::DamageSource {
            weapon,
            dtype: VOLUME_DEATH_TYPE,
        };
        // DC-DEFAULT: DSV-596 (proposed) the drop through the volume is a death by falling: the fall voice
        self.plunge = Some((pid, false));
        self.apply_damage(
            pid,
            None,
            lethal,
            [0.0, -1.0, 0.0],
            src,
            super::TODO_KILL_ZONE_BODY,
            now,
        );
        self.plunge = None;
    }
}

#[cfg(test)]
mod dc_mode_tests {
    use super::*;

    /// MAP-21: a comma list or one DeCombat mode name is a DeCombat filter; a ROM-style text is not.
    #[test]
    fn dc_mode_lists_are_recognised() {
        assert_eq!(
            dc_mode_list(b"TDM,FFA, elim ,").as_deref(),
            Some(&["TDM".to_string(), "FFA".into(), "elim".into()][..])
        );
        assert_eq!(
            dc_mode_list(b"bomb").as_deref(),
            Some(&["bomb".to_string()][..])
        );
        assert_eq!(
            dc_mode_list(b" FLAG_CLAN ").as_deref(),
            Some(&["FLAG_CLAN".to_string()][..])
        );
        assert_eq!(dc_mode_list(b"some rom filter text"), None);
        assert_eq!(dc_mode_list(b"TDMx"), None);
        assert_eq!(dc_mode_list(b""), None);
    }

    /// MAP-21: the DeCombat rule decides a list (serves, twins, BOMB for ELIM) and an empty filter on a vocabulary
    /// object; a ROM text is left to the initials rule (`None`); a room without a DeCombat name serves no list.
    #[test]
    fn dc_mode_serves_decides_lists_and_empty_vocabulary_filters() {
        let s = Game::dc_mode_serves;
        assert_eq!(s(Some(b"TDM,FFA"), true, Some("TDM")), Some(true));
        assert_eq!(s(Some(b"tdm"), false, Some("TDM_CLAN")), Some(true));
        assert_eq!(s(Some(b"TDM,FFA"), true, Some("BOMB")), Some(false));
        assert_eq!(s(Some(b"BOMB"), true, Some("ELIM")), Some(true));
        assert_eq!(s(Some(b"ELIM"), true, Some("BOMB")), Some(false));
        assert_eq!(s(Some(b"FLAG"), true, None), Some(false));
        assert_eq!(s(Some(b""), true, Some("TDM")), Some(true));
        assert_eq!(s(None, true, None), Some(true));
        assert_eq!(s(Some(b""), false, Some("TDM")), None);
        assert_eq!(s(None, false, Some("TDM")), None);
        assert_eq!(s(Some(b"some rom filter text"), true, Some("TDM")), None);
    }
}

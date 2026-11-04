//! Movement relay: the player's reported body state from `UP_PLAYER_UPDATE` (validated, corrected only by a
//! teleport) and the other players' objects in the per-tick object stream.
// SPEC: GAME_MESSAGES_2009_C2S.md §2.1; MOVEMENT_2009.md §12; UDP_SESSION_2009_B.md §4.3, §9.1

use super::{
    fall, finite3, fold_yaw, len3, sub3, Game, Life, Motion, VIEW_REPAIR_INTERVAL_MS,
    VIEW_REPAIR_MAX_RAD,
};
use crate::gs;
use crate::modelobj::Tracker;
use crate::objects::PosBox;
use crate::shell64::Shell64Update;
use crate::stream::{yaw_quat, StreamEntry};
use dc_net_engine::time::since;
use dc_net_engine::Millis;

/// A remote player's velocity is streamed as 0 once its last accepted update is older than this.
// DC-DEFAULT: DSV-21 (staged proposal) 250 ms
pub const STALE_VELOCITY_MS: u32 = 250;

/// Seconds between two vitals notes of a player whose health and armour did not change.
// DC-DEFAULT: DSV-521 (proposed, project decision; the reference stack's pace, GAME_MESSAGES_2009_S2C.md §8.3)
pub const VITALS_REFRESH_MS: u32 = 2000;

/// The health and armour notes due for a player with a character: the shown values (rounded, 0..=999) when they
/// differ from the last notes or [`VITALS_REFRESH_MS`] passed since them; `None` otherwise.
// DC-DEFAULT: DSV-521 (proposed, project decision)
pub fn vitals_due(
    has_character: bool,
    health: f32,
    armour: f32,
    last: Option<([u16; 2], Millis)>,
    now: Millis,
) -> Option<[u16; 2]> {
    if !has_character {
        return None;
    }
    let shown = |v: f32| {
        if v.is_finite() {
            v.round().clamp(0.0, 999.0) as u16
        } else {
            0
        }
    };
    let v = [shown(health), shown(armour)];
    match last {
        Some((was, at)) if was == v && since(now, at) < VITALS_REFRESH_MS => None,
        _ => Some(v),
    }
}

/// The main tracker of a dead character until its next spawn: the corpse's death clip, not looping (`state1` 0),
/// its time the ms since the death held at the clip's end; `None` for a living player. The two body trackers keep
/// running as before the death.
// OBSERVED: capture 20261025_002236_tdm4 (the real client's link, bot character 408 killed at
// seq 3132: from the entry of the death tick, seq 3135, to the end of the death the main tracker is the corpse's
// clip 76 with state bits 1 / 0 and time 0, 40, 72 ... 1328, then 1336 held; trackers 1 / 2 kept their clips and
// running times; REFERENCE, accepted by the real client); 20261025_065016_tdm6 (character 252 from seq 10134: clip 86
// = the corpse's, state bits 1 / 0; REFERENCE); 20261025_095358_rust4 (ours kept the standing loop, clip 0 state bits
// 1 / 1, for the whole death)
// DC-DEFAULT: DSV-44 (amendment, proposed)
fn dead_main_tracker(p: &super::Player, now: Millis) -> Option<Tracker> {
    let died = match p.life {
        Life::Dying { died } | Life::Waiting { died } => died,
        _ => return None,
    };
    let (anim, len_ms) = p.death_clip?;
    Some(Tracker {
        anim,
        state0: true,
        state1: false,
        time_ms: since(now, died).min(len_ms),
        len_ms,
        secondary: None,
    })
}

impl Game {
    /// `UP_PLAYER_UPDATE`: the reported body state while alive. Updates sent before the client applied the last
    /// teleport (older `move_seq`) are ignored; implausible ones are corrected with a teleport back.
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1 server answer; MOVEMENT_2009.md §12.1, §12.4
    // DC-DEFAULT: DSV-14 (staged proposal) validation values
    pub fn on_update(&mut self, pid: u8, u: &Shell64Update, now: Millis) {
        let cfg_check = self.cfg.move_check;
        let (max_speed, slack, margin, dims) = (
            self.cfg.max_speed,
            self.cfg.move_slack,
            self.cfg.box_margin,
            self.dims_of(pid),
        );
        let pbox = self.pbox;
        let directional = self.cfg.directional_anims;
        let Some(p) = self.players.get_mut(&pid) else {
            return;
        };
        p.updates += 1;
        // DC-DEFAULT: DSV-483 (proposed) the server drives the body during a Possession move
        if p.flight.is_some() {
            p.stale_updates += 1;
            return;
        }
        if u.state_form.is_some() || !u.complete || p.life != Life::Alive {
            return;
        }
        // the character's animation follows the reported control word (MOVEMENT_2009.md §5.1 bits)
        // DC-DEFAULT: DSV-28 (refinement, proposed) gait and direction sets
        if let Some(c) = u.control() {
            let m = Motion::from_control(c, directional);
            if m != p.motion {
                p.motion = m;
                p.motion_since = now;
            }
        }
        let Some(t) = u.trailer else {
            return;
        };
        if t.move_seq() != p.move_seq {
            p.stale_updates += 1;
            return;
        }
        let pos = t.pos();
        let mut bad = None;
        if !finite3(pos) || !finite3(t.vel()) {
            bad = Some("not finite");
        } else if cfg_check {
            let outside =
                (0..3).any(|i| pos[i] < pbox.min[i] - margin || pos[i] > pbox.max[i] + margin);
            if outside {
                bad = Some("outside the world box");
            } else if let Some(last) = p.last_move {
                let dt = (since(now, last) as f32 / 1000.0).max(0.05);
                if len3(sub3(pos, p.pos)) > max_speed * dt + slack {
                    bad = Some("too fast");
                }
            }
        }
        if let Some(why) = bad {
            p.move_seq = p.move_seq.wrapping_add(1);
            p.corrections += 1;
            p.last_move = Some(now);
            let (seq, back) = (p.move_seq, p.pos);
            // DC-DEFAULT: DSV-42 (proposed) a teleport starts a new track
            p.track.restart(now, back);
            self.send(pid, gs::mover_teleport(seq, back, dims, None));
            self.log(format!(
                "client {pid}: update rejected ({why}), teleport back (seq {seq})"
            ));
            return;
        }
        p.pos = pos;
        p.track.push(now, pos);
        p.vel = t.vel();
        // DC-DEFAULT: DSV-483 (proposed) the end point of a move onto this player
        p.on_ground_report = t.on_ground();
        if t.on_ground()
            && u.control()
                .is_none_or(|c| c & dc_sim::player::bits::CROUCH == 0)
        {
            p.stand_pos = Some(pos);
        }
        // DC-DEFAULT: DSV-76 (proposed) the fall track follows the accepted trailers of the current teleport sequence
        let fell = fall::track(&mut p.fall, p.move_seq, pos[1], t.on_ground(), t.surface());
        // DC-DEFAULT: DSV-126 (proposed) a ground report ends the jump clip
        p.pose.set_ground(t.on_ground(), now);
        let mut repair = None;
        if let Some(y) = u.yaw() {
            if y.is_finite() {
                p.yaw = fold_yaw(y);
                // DC-DEFAULT: CV2-5 (proposed) a runaway yaw accumulator is folded back with 0x6a kind 0
                if (y as f64).abs() > VIEW_REPAIR_MAX_RAD
                    && p.view_repair_at
                        .is_none_or(|t| since(now, t) >= VIEW_REPAIR_INTERVAL_MS)
                {
                    p.view_repair_at = Some(now);
                    p.view_repairs += 1;
                    let pitch = u
                        .wide
                        .map(|w| f32::from_bits(w[0]))
                        .filter(|x| x.is_finite())
                        .unwrap_or(0.0);
                    repair = Some([pitch, p.yaw, 0.0]);
                }
            }
        }
        p.last_move = Some(now);
        // the player's aim to every client in the world (the owner included) when its aux words changed
        // OBSERVED: 20261025_001409_tdm2 (sub 0x80 only on a change of the words: 1 equal pair in 593; median gap
        // 78 ms, the update pace; unreliable; to both links); DC-DEFAULT: DSV-153 (proposed)
        let aim = match (u.wide, p.character.is_some()) {
            (Some(w), true) if p.aim_sent != Some(w) => {
                p.aim_sent = Some(w);
                Some(gs::aim_note(p.obj, w))
            }
            _ => None,
        };
        // the player's health and armour to every client in the world when they changed, else every 2 s: the death
        // panel of a dead team mate who follows this player shows them (cshell+0xcbea0 via the type-0x18 object's
        // +0x5ae / +0x5b0, cshell+0x26f60)
        // OBSERVED: the reference stack sent subs 0x14 / 0x15 to every client after each hit and about every 2 s per
        // character (GAME_MESSAGES_2009_S2C.md §8.3; REFERENCE)
        // DC-DEFAULT: DSV-521 (proposed, project decision)
        let vitals = vitals_due(p.character.is_some(), p.health, p.armor, p.vitals_sent, now);
        if let Some(v) = vitals {
            p.vitals_sent = Some((v, now));
        }
        let obj = p.obj;
        if let Some(m) = aim {
            for t in self.introduced() {
                self.out.push(super::GameOut::Unreliable(t, m.clone()));
            }
        }
        if let Some(v) = vitals {
            for t in self.introduced() {
                for (sub, value) in [(gs::NOTE_HEALTH, v[0]), (gs::NOTE_ARMOUR, v[1])] {
                    self.out.push(super::GameOut::Unreliable(
                        t,
                        gs::vitals_note(obj, sub, value),
                    ));
                }
            }
        }
        if let Some(a) = repair {
            self.send(pid, gs::view_angles(gs::ANGLES_ALL, a));
            self.log(format!(
                "client {pid}: reported yaw out of range, view angles reset to yaw {:.3}",
                a[1]
            ));
        }
        if let Some(h) = fell {
            self.fall_damage(pid, h, now);
        }
        // DC-DEFAULT: DSV-77 (reinstated, proposed) a player airborne at the round result stops at his landing
        self.freeze_on_landing(pid);
    }

    /// Stream entries for `viewer`: every other placed player's object, the longest unsent first; rotation only when
    /// it changed since the last send to this viewer; velocity 0 when the player sent no accepted update for
    /// [`STALE_VELOCITY_MS`] (the client stops extrapolating). The viewer's own object is not streamed to it (the
    /// client owns its body; corrections go by teleport).
    // SPEC: UDP_SESSION_2009_B.md §4.2 (own view object: no rotation; follow-up record with velocity 0), §4.3 writer
    // rules
    // DC-DEFAULT: CD-65 (approved) order by staleness; CD-67 (approved) own rotation never sent; DSV-21 (staged
    // proposal) own object not streamed, stale velocity 0
    pub fn stream_for(&self, viewer: u8, now: Millis) -> Vec<StreamEntry> {
        let Some(v) = self.players.get(&viewer) else {
            return Vec::new();
        };
        let mut list: Vec<(u32, StreamEntry)> = self
            .players
            .values()
            .filter(|p| p.pid != viewer || (self.cfg.stream_own && p.character.is_some()))
            .map(|p| {
                let q = yaw_quat(p.yaw);
                // characters: yaw8 and the animation block in every entry, as the reference stack streamed them
                // SPEC: UDP_REFERENCE_FINDINGS_botclient.md reference run (player entries `uf` 0xd every tick; REFERENCE);
                // UDP_SESSION_2009_B.md §4.3 (flag 0x0800 objects get yaw8)
                let anim = p.character.as_ref().map(|c| {
                    // DC-DEFAULT: DSV-125 / DSV-126 (proposed) upper body by weapon family and action, jump clip
                    let mut t = c.anims.trackers_posed(
                        p.motion,
                        &p.pose,
                        now,
                        since(now, p.spawned_at),
                        since(now, p.motion_since),
                        self.cfg.directional_anims,
                    );
                    if let Some(main) = dead_main_tracker(p, now) {
                        t[0] = main;
                    }
                    t
                });
                let seen = v.stream_seen.get(&p.obj);
                let rot = match seen.and_then(|s| s.1) {
                    Some(old) if old.iter().zip(q.iter()).all(|(a, b)| (a - b).abs() < 1e-4) => {
                        None
                    }
                    _ => Some(q),
                };
                let age = seen.map_or(u32::MAX, |s| since(now, s.0));
                (
                    age,
                    StreamEntry {
                        obj: p.obj,
                        pos: p.pos,
                        vel: if p
                            .last_move
                            .is_some_and(|t| since(now, t) <= STALE_VELOCITY_MS)
                        {
                            p.vel
                        } else {
                            [0.0; 3]
                        },
                        rot: if anim.is_some() { None } else { rot },
                        yaw8: anim.is_some().then_some(p.yaw),
                        anim,
                    },
                )
            })
            .collect();
        // co-op AI characters (FIRETEAM_2009.md §1.8: trackers in the object stream as for remote players)
        for e in self.ai_stream_entries(now) {
            let seen = v.stream_seen.get(&e.obj);
            let age = seen.map_or(u32::MAX, |s| since(now, s.0));
            list.push((age, e));
        }
        list.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.obj.cmp(&b.1.obj)));
        list.into_iter().map(|x| x.1).collect()
    }

    /// Records what [`Game::stream_for`] entries were actually sent.
    pub fn mark_streamed(&mut self, viewer: u8, sent: &[StreamEntry], now: Millis) {
        if let Some(v) = self.players.get_mut(&viewer) {
            for e in sent {
                let old = v.stream_seen.get(&e.obj).and_then(|s| s.1);
                v.stream_seen.insert(e.obj, (now, e.rot.or(old)));
            }
        }
    }

    /// World box used by the stream's `pos48`.
    pub fn pos_box(&self) -> PosBox {
        self.pbox
    }
}

#[cfg(test)]
mod vitals_tests {
    use super::*;

    /// DSV-521: the notes go out at once, again on a change, and otherwise every 2 s.
    #[test]
    fn vitals_go_out_on_a_change_and_every_two_seconds() {
        assert_eq!(
            vitals_due(false, 100.0, 50.0, None, 0),
            None,
            "no character, no note"
        );
        assert_eq!(vitals_due(true, 99.6, 50.2, None, 0), Some([100, 50]));
        let last = Some(([100, 50], 1000));
        assert_eq!(vitals_due(true, 100.0, 50.0, last, 2999), None);
        assert_eq!(vitals_due(true, 57.0, 50.0, last, 1100), Some([57, 50]));
        assert_eq!(vitals_due(true, 100.0, 50.0, last, 3000), Some([100, 50]));
        assert_eq!(vitals_due(true, -3.0, f32::NAN, None, 0), Some([0, 0]));
    }

    /// The note is the 2009 sub layout: type 0x18, object, sub, 16-bit value.
    #[test]
    fn the_vitals_note_layout() {
        let m = gs::vitals_note(0x0123, gs::NOTE_HEALTH, 87);
        assert_eq!(
            m.as_bytes(),
            &[0x0d, 0xa6, 0x18, 0x23, 0x01, 0x14, 87, 0][..]
        );
    }
}

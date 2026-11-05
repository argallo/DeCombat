//! Event sounds of the match (DSV-64): which file plays where and when. The record form is the reference stack's
//! ([`crate::sounds`]); the choices here are ours.
// DC-DEFAULT: DSV-64 (proposed)

use super::{Game, GameOut};
use crate::sounds::{self, SoundForm, SoundOut, VoiceFamily};
use dc_net_engine::time::later;
use dc_net_engine::Millis;

impl Game {
    /// Play time of a ROM sound file in ms (cached; [`sounds::UNKNOWN_PLAY_MS`] when it cannot be read).
    fn play_ms(&mut self, file: &str) -> u32 {
        if let Some(&ms) = self.play_ms_cache.get(file) {
            return ms;
        }
        let ms = self
            .vfs
            .as_ref()
            .and_then(|v| sounds::rom_play_ms(v, file))
            .unwrap_or(sounds::UNKNOWN_PLAY_MS);
        self.play_ms_cache.insert(file.to_string(), ms);
        ms
    }

    /// Queues one event sound for every client in the world; returns its play time.
    pub(super) fn event_sound(&mut self, file: String, pos: [f32; 3], form: SoundForm) -> u32 {
        let play = self.play_ms(&file);
        self.out.push(GameOut::Sound(Box::new(SoundOut {
            file,
            pos,
            form,
            life_ms: play.saturating_add(sounds::TRACK_TAIL_MS),
        })));
        play
    }

    /// A world object's sound (a door's opening / closing sound, DSV-302) at `pos` with the object's radius.
    // DC-DEFAULT: DSV-302 (proposed) the grenade impact's priority and inner ratio, the object's radius as outer
    pub fn object_sound(&mut self, file: String, pos: [f32; 3], radius: f32) {
        let form = SoundForm {
            outer: radius.clamp(1.0, f32::from(u16::MAX)) as u16,
            ..sounds::BOUNCE_FORM
        };
        self.event_sound(file, pos, form);
    }

    /// The voice family of a player's character: the body row his `user` line names (`model=`), else the body row
    /// of his equipment (a mercenary item, else the vest: DSV-271, the row his body is drawn with), else the operator
    /// default, through the model table's figure row (DSV-151).
    // DC-DEFAULT: DSV-24 (proposed) the body row; DSV-151 (proposed) the family; DSV-420 (proposed) the equipment's
    // body row as in `character_for` (EVIDENCE: cshell+0x1e4760 reads the figure's sound template per body row,
    // cshell+0x1d32e0 the body row of the look: the character item, else the vest row)
    pub(super) fn voice_family(&self, pid: u8) -> VoiceFamily {
        let Some(p) = self.players.get(&pid) else {
            return VoiceFamily::Default;
        };
        let d = self.cfg.character;
        let rows = p.roster.as_ref().map(|e| e.character_rows());
        let loadout = p.roster.as_ref().and_then(|e| e.loadout_items());
        let row = crate::chars::voice_row(
            &self.tables.chars,
            rows.and_then(|r| r.0),
            loadout.as_deref(),
            rows.and_then(|r| r.2).unwrap_or(d.face_row),
        )
        .unwrap_or(d.model_row);
        self.tables.event_sounds.family_of_body(row)
    }

    /// A player took damage: a pain sound at its position (a random pain variant of the ROM's preload table, of the
    /// character's voice family) unless its last pain sound is still playing; a head-zone hit also plays the
    /// head-shot sound.
    // OBSERVED: capture 20261025_002236_tdm4 c6 (four hits on bot 408 in 1.0 s, pain sounds at
    // the first and the fourth: seq 2968 / 3132, the second after the first track's removal), 20261025_065016_tdm6 c6
    // seq 10134 (the head-shot kill: the head-shot sound and a pain sound in one message); REFERENCE
    // DC-DEFAULT: DSV-64 (proposed) one pain sound at a time per player, the variant at random; DSV-151 (proposed)
    // the variants of the character's voice family
    pub(super) fn pain_sound(&mut self, vpid: u8, head: bool, now: Millis) {
        if !self.cfg.event_sounds {
            return;
        }
        let Some(pos) = self.players.get(&vpid).map(|p| p.pos) else {
            return;
        };
        if head {
            if let Some(f) = self.tables.event_sounds.headshot.clone() {
                self.event_sound(f, pos, sounds::PAIN_FORM);
            }
        }
        if self.pain_until.get(&vpid).is_some_and(|&t| later(t, now)) {
            return;
        }
        let family = self.voice_family(vpid);
        let n = self.tables.event_sounds.pain_of(family).len();
        if n == 0 {
            return;
        }
        let pick = (self.next_rand() % n as u64) as usize;
        let f = self.tables.event_sounds.pain_of(family)[pick].clone();
        let play = self.event_sound(f, pos, sounds::PAIN_FORM);
        self.pain_until.insert(vpid, now.wrapping_add(play));
    }

    /// A landing that hurts: the fall-hurt sound of the character's voice family at the player, in the pain form,
    /// unless the player's last pain (or fall) sound is still playing. It counts as the pain sound of the landing's
    /// damage, so the damage that follows in the same tick starts no pain variant. Returns whether it started.
    // SPEC: UDP_SESSION_2009_B.md §8.3 "Pain, fall and bounce sounds" (the fall's tracked sound in the pain form:
    // `sflags` 0x0132, prio 2, outer 500, ratio 17; tdm4 c6 seq 6695)
    // OBSERVED: capture 20261025_002236_tdm4 c6 seq 6695, 20261025_000014_tdm1 c5 seq 6660 (the
    // reference stack's fall-hurt file in the tick of the landing, before the 0x74; no pain variant with it); tdm1 c5 seq 7933
    // (a fall without the sound); REFERENCE, accepted by the real client
    // DC-DEFAULT: DSV-152 (proposed) the fall-hurt file instead of a pain variant, one sound at a time per player
    pub(super) fn fall_hurt_sound(&mut self, pid: u8, now: Millis) -> bool {
        if !self.cfg.event_sounds {
            return false;
        }
        let Some(pos) = self.players.get(&pid).map(|p| p.pos) else {
            return false;
        };
        if self.pain_until.get(&pid).is_some_and(|&t| later(t, now)) {
            return false;
        }
        let family = self.voice_family(pid);
        let Some(f) = self.tables.event_sounds.fall_of(family).cloned() else {
            return false;
        };
        let play = self.event_sound(f, pos, sounds::PAIN_FORM);
        self.pain_until.insert(pid, now.wrapping_add(play));
        true
    }

    /// A death by falling: the fall voice of the character's voice family at the player in the fall-death form, also
    /// when a pain sound is still playing (a death is never swallowed by the grunt before it). Returns whether it
    /// started (false without event sounds or a fall row: the usual pain sound plays then).
    // DC-DEFAULT: DSV-596 (proposed) the fall row of the preload table is the only fall voice of the ROM (no row names
    // a fall-death file: the CharDeathSound rows of the client table name files the archives do not hold, and the
    // client reads no such key); a fall death plays it instead of the pain variant
    pub(super) fn fall_death_sound(&mut self, pid: u8, now: Millis) -> bool {
        if !self.cfg.event_sounds {
            return false;
        }
        let Some(pos) = self.players.get(&pid).map(|p| p.pos) else {
            return false;
        };
        let family = self.voice_family(pid);
        let Some(f) = self.tables.event_sounds.fall_of(family).cloned() else {
            return false;
        };
        let play = self.event_sound(f, pos, sounds::FALL_DEATH_FORM);
        self.pain_until.insert(pid, now.wrapping_add(play));
        true
    }

    /// The voice of a hit that is a fall (the damage of a landing, or the drop through a kill volume), chosen once the
    /// hit's outcome is known: a lethal one plays the fall-death sound, a hurting landing the fall-hurt sound. Returns
    /// whether a voice started, so the hit's own pain sound is left out. Shots, blasts and the rest return false.
    // DC-DEFAULT: DSV-596 (proposed), DSV-152 (the hurt case)
    pub(super) fn plunge_voice(&mut self, pid: u8, lethal: bool, now: Millis) -> bool {
        match self.plunge {
            Some((p, _)) if p == pid && lethal => self.fall_death_sound(pid, now),
            Some((p, true)) if p == pid => self.fall_hurt_sound(pid, now),
            _ => false,
        }
    }

    /// A thrown object hit a surface of the given ROM surface id: that surface's grenade impact sound at the point.
    // OBSERVED: capture 20261025_065016_tdm6 c6 seq 9409 / 9423 (two bounces of one throw on
    // JUNK_FLEA: the wood and the stone impact files, the surfaces' `GrenadeImpactSnd`), 20261025_002236_tdm4 c6 seq
    // 7690 / 7715 / 7765 (stone, then terrain twice); REFERENCE
    // DC-DEFAULT: DSV-64 (proposed) one sound per bounce
    pub(super) fn bounce_sound(&mut self, pos: [f32; 3], surface: u8) {
        if !self.cfg.event_sounds {
            return;
        }
        let Some(f) = self
            .tables
            .event_sounds
            .grenade_impact
            .get(&u32::from(surface))
            .cloned()
        else {
            return;
        };
        self.event_sound(f, pos, sounds::BOUNCE_FORM);
    }
}

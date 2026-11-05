//! What the statements of a mission or round world show the clients (DSV-579..DSV-584): the dialogue captions and
//! banner texts their caption and message triggers `play`, the music their music triggers switch, the characters
//! `hidecharacter` hides (logged only), and what a player who joins a running mission is told (the goal list or the
//! round, the music). The camera statements are the session's (`camshot`, DSV-580); the weapon rows of the rounds
//! are the drops' (`drops::coop_weapon_rows`, DSV-583).
//!
//! The wire forms are the 2009 client's (0xc9 sub 3 caption, 0xc9 sub 2 banner, 0xd9 music); which trigger class
//! maps to which form, the show times and the late joiner's catch-up are DeCombat rules.
// SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (0xc9), §4.2 (0xd9); FIRETEAM_2009.md §2.4
// OBSERVED: ROM SANDHOG_CP1 and CabinFever (caption triggers `play` the GMS dialogue keys whose texts have three
// `|` fields, the message trigger `play`s plain texts; music triggers name a track file, a fade time and a loop flag)
// DC-DEFAULT: DSV-579..DSV-584 (proposed)

use super::coop::WavePhase;
use super::Game;
use crate::gs;
use crate::script::Cue;
use dc_net_engine::Millis;

/// Show time of a caption or message trigger without the property (seconds).
// DC-DEFAULT: DSV-579 (proposed) 4 s, the mission map's value
pub const TEXT_SECS_DEFAULT: f32 = 4.0;
/// Fade time of a music trigger without the property (seconds).
// DC-DEFAULT: DSV-581 (proposed) 1 s, the value the 2009 shell passes for its own menu music (cshell+0x1fba00)
pub const MUSIC_FADE_DEFAULT: f32 = 1.0;
/// Entries kept in [`Shown::log`].
const SHOWN_LOG_MAX: usize = 512;

/// What the statements showed so far.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shown {
    /// The music trigger playing (its index in the world's music triggers).
    pub music: Option<usize>,
    /// `hidecharacter 1` is in force (logged only: no 2009 message is known).
    pub hidden: bool,
    pub captions: u32,
    pub banners: u32,
    pub music_changes: u32,
    pub hide_changes: u32,
    /// The weapon rows laid out as drops (DSV-583).
    pub weapon_drops: u32,
    /// `play` statements whose trigger or text the world and the GMS do not have.
    pub unresolved: u32,
    /// (time, what, name): `c` caption key, `b` banner key, `m` music trigger on, `s` music stop, `h` / `v` characters
    /// hidden / shown (tests and logs).
    pub log: Vec<(Millis, char, Vec<u8>)>,
}

impl Shown {
    fn note(&mut self, now: Millis, what: char, name: &[u8]) {
        if self.log.len() < SHOWN_LOG_MAX {
            self.log.push((now, what, name.to_vec()));
        }
    }
}

impl Game {
    /// The presentation cues of a statement run that the session's cameras did not take (DSV-579, DSV-581,
    /// DSV-582).
    pub fn coop_cues(&mut self, cues: Vec<Cue>, now: Millis) {
        if self.coop.is_none() {
            return;
        }
        for cue in cues {
            match cue {
                Cue::Play { trigger, key } => self.coop_play(&trigger, &key, now),
                Cue::Switch { target, on } => self.coop_music(&target, on, now),
                // DC-DEFAULT: DSV-582 (proposed) `hidecharacter` is logged only: the 2009 message that hides the
                // characters during a cut scene is not known
                Cue::HideCharacters(h) => {
                    let Some(c) = self.coop.as_mut() else {
                        return;
                    };
                    if c.show.hidden != h {
                        c.show.hidden = h;
                        c.show.hide_changes += 1;
                        c.show.note(now, if h { 'h' } else { 'v' }, b"");
                        self.log(format!(
                            "co-op: characters {} (not sent)",
                            if h { "hidden" } else { "shown" }
                        ));
                    }
                }
                // a camera statement whose camera the world does not have
                Cue::MoveTo { .. } | Cue::Fov { .. } => {
                    if let Some(c) = self.coop.as_mut() {
                        c.show.unresolved += 1;
                    }
                }
            }
        }
    }

    /// `msg <trigger> (play <key>)`: a caption trigger shows the dialogue text of `key` as the mission caption
    /// (0xc9 sub 3), a message trigger as a banner (0xc9 sub 2), each for the trigger's show time.
    // SPEC: GAME_MESSAGES_2009_S2C.md §6.2 (sub 2 banner, sub 3 caption: three `|` fields, cshell+0x81fe0)
    // DC-DEFAULT: DSV-579 (proposed)
    fn coop_play(&mut self, trigger: &[u8], key: &[u8], now: Millis) {
        let t = self
            .coop_world
            .texts
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(trigger))
            .cloned();
        let text = self
            .coop
            .as_ref()
            .and_then(|c| c.plan.dialog_text(key))
            .map(|s| s.as_bytes().to_vec());
        let (Some(t), Some(text)) = (t, text) else {
            if let Some(c) = self.coop.as_mut() {
                c.show.unresolved += 1;
            }
            self.log(format!(
                "co-op: play {} on {}: no such text trigger or dialogue key",
                String::from_utf8_lossy(key),
                String::from_utf8_lossy(trigger)
            ));
            return;
        };
        let secs = t.secs.unwrap_or(TEXT_SECS_DEFAULT);
        let m = if t.caption {
            gs::coop_caption(&text, secs)
        } else {
            gs::coop_banner(&text, secs)
        };
        self.broadcast(&m);
        if let Some(c) = self.coop.as_mut() {
            if t.caption {
                c.show.captions += 1;
                c.show.note(now, 'c', key);
            } else {
                c.show.banners += 1;
                c.show.note(now, 'b', key);
            }
        }
        self.log(format!(
            "co-op: {} {} ({secs} s)",
            if t.caption { "caption" } else { "banner" },
            String::from_utf8_lossy(key)
        ));
    }

    /// `msg <music trigger> on|off`: `on` plays its track (0xd9 op 1 with its loop flag and fade time), `off` stops
    /// the music when it is the track playing (0xd9 op 0). Other names are no music trigger: nothing.
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.2 row 0xd9
    // DC-DEFAULT: DSV-581 (proposed)
    fn coop_music(&mut self, target: &[u8], on: bool, now: Millis) {
        let Some(i) = self
            .coop_world
            .music
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(target))
        else {
            return;
        };
        let m = self.coop_world.music[i].clone();
        let playing = self.coop.as_ref().and_then(|c| c.show.music);
        if on {
            let fade = m.fade.unwrap_or(MUSIC_FADE_DEFAULT);
            self.broadcast(&gs::music_play(&m.file, m.looped, fade));
        } else if playing == Some(i) {
            self.broadcast(&gs::music_stop());
        } else {
            return;
        }
        if let Some(c) = self.coop.as_mut() {
            c.show.music = on.then_some(i);
            c.show.music_changes += 1;
            c.show.note(now, if on { 'm' } else { 's' }, &m.name);
        }
        self.log(format!(
            "co-op: music {} {}",
            String::from_utf8_lossy(&m.name),
            if on { "on" } else { "off" }
        ));
    }

    /// A player who enters a running co-op match: the goal list of a mission world, or the current round (0xc9 sub 1
    /// `a`) of a wave world, and the looping track that plays.
    // SPEC: FIRETEAM_2009.md §2.4 (sub 1: the list is rebuilt, `a` the stage); GAME_MESSAGES_2009_S2C.md §4.2 (0xd9)
    // DC-DEFAULT: DSV-584 (proposed)
    pub(super) fn coop_catch_up(&mut self, pid: u8, now: Millis) {
        let Some(c) = self.coop.as_ref() else {
            return;
        };
        let list = if c.mission {
            self.coop_goal_list(now)
        } else if c.phase != WavePhase::Idle || c.round > 0 || c.waves_cleared > 0 {
            let secs = self.round_secs_left(now);
            let timers: Vec<f32> = if secs > 0 {
                vec![secs as f32]
            } else {
                Vec::new()
            };
            Some(gs::coop_mission_list(
                &[],
                &timers,
                (c.wave + 1).min(255) as u8,
                c.goal as u8,
            ))
        } else {
            None
        };
        let music = c
            .show
            .music
            .and_then(|i| self.coop_world.music.get(i))
            .filter(|m| m.looped)
            .map(|m| gs::music_play(&m.file, true, m.fade.unwrap_or(MUSIC_FADE_DEFAULT)));
        if let Some(m) = list {
            self.send(pid, m);
        }
        if let Some(m) = music {
            self.send(pid, m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Game, GameOut, GameTuning};
    use crate::coop_rom::WeaponSpawnRow;
    use crate::gs;
    use crate::pipe::RoomSettings;
    use crate::rom_tables::RomTables;
    use crate::script::Cue;
    use crate::world::{
        synthetic_world_bytes, world_from_bytes, ItemPoint, MusicTrigger, TextTrigger,
    };

    const CAP: &str = "pic.dtx|-Speaker-| Defend all entrances!";

    /// A co-op room (class 5, no ROM) on a world with one caption trigger, one message trigger, a looping and a
    /// one-shot music trigger and an item spawn point; one introduced human (client 1).
    fn coop_game(tables: RomTables) -> Game {
        let mut w = world_from_bytes(&synthetic_world_bytes(), "Worlds\\S.dat", "S").unwrap();
        w.coop.texts = vec![
            TextTrigger {
                name: b"CapTRG01".to_vec(),
                caption: true,
                secs: Some(10.0),
            },
            TextTrigger {
                name: b"MsgTRG01".to_vec(),
                caption: false,
                secs: None,
            },
        ];
        w.coop.music = vec![
            MusicTrigger {
                name: b"Ambient".to_vec(),
                file: b"BGM/a.mp3".to_vec(),
                looped: true,
                fade: Some(4.0),
            },
            MusicTrigger {
                name: b"Intro".to_vec(),
                file: b"BGM/i.mp3".to_vec(),
                looped: false,
                fade: None,
            },
        ];
        w.coop.item_points = vec![ItemPoint {
            name: b"WeaponSpawnPoint00".to_vec(),
            pos: [10.0, 0.0, 20.0],
            yaw: 0.0,
        }];
        let mut g = Game::new(GameTuning::default(), &w, tables);
        let mut room = RoomSettings::default();
        room.pairs.insert("type".into(), "5".into());
        g.set_room(&room);
        g.prepare_player(1, None, None, 10, [0.0; 3], [0.0; 3], None, 0);
        g.players.get_mut(&1).unwrap().introduced = true;
        let c = g.coop.as_mut().expect("a co-op room");
        c.plan.dialog = vec![
            ("Cap101".into(), CAP.into()),
            ("Msg001".into(), "Enter the temple.".into()),
        ];
        g.take_out();
        g
    }

    fn sent_to(out: &[GameOut], pid: u8) -> Vec<dc_wire::udp::BitWriter> {
        out.iter()
            .filter_map(|o| match o {
                GameOut::To(p, m) if *p == pid => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    /// DSV-579 / DSV-581 / DSV-582: a caption trigger's `play` is the 0xc9 sub 3 caption with its show time, a message
    /// trigger's the sub 2 banner (4 s without the property); an unknown key is counted, nothing is sent; music `on`
    /// plays the track, `off` of another track does nothing, `off` of the track playing stops it; a non-music name
    /// does nothing; `hidecharacter` sends nothing.
    #[test]
    fn captions_banners_and_music_follow_the_cues() {
        let mut g = coop_game(RomTables::default());
        let play = |t: &str, k: &str| Cue::Play {
            trigger: t.as_bytes().to_vec(),
            key: k.as_bytes().to_vec(),
        };
        let switch = |t: &str, on| Cue::Switch {
            target: t.as_bytes().to_vec(),
            on,
        };
        g.coop_cues(
            vec![
                play("captrg01", "cap101"),
                play("MsgTRG01", "Msg001"),
                play("CapTRG01", "Cap999"),
                switch("Ambient", true),
                switch("Intro", false),
                switch("Door", true),
                Cue::HideCharacters(true),
            ],
            1_000,
        );
        let sent = sent_to(&g.take_out(), 1);
        assert_eq!(
            sent,
            vec![
                gs::coop_caption(CAP.as_bytes(), 10.0),
                gs::coop_banner(b"Enter the temple.", super::TEXT_SECS_DEFAULT),
                gs::music_play(b"BGM/a.mp3", true, 4.0),
            ]
        );
        let s = &g.coop.as_ref().unwrap().show;
        assert_eq!((s.captions, s.banners, s.unresolved), (1, 1, 1));
        assert_eq!((s.music, s.hidden, s.music_changes), (Some(0), true, 1));
        g.coop_cues(vec![switch("AMBIENT", false)], 2_000);
        assert_eq!(sent_to(&g.take_out(), 1), vec![gs::music_stop()]);
        assert_eq!(g.coop.as_ref().unwrap().show.music, None);
    }

    /// DSV-584: a player who enters a running mission gets the goal list (states kept), on a wave world the round
    /// (`a` of 0xc9 sub 1), and the looping track that plays; before the start nothing.
    #[test]
    fn a_late_joiner_gets_the_goals_or_the_round_and_the_music() {
        let mut g = coop_game(RomTables::default());
        g.coop_catch_up(1, 0);
        assert!(sent_to(&g.take_out(), 1).is_empty(), "nothing has started");
        {
            let c = g.coop.as_mut().unwrap();
            c.mission = true;
            c.plan.mission_list = vec![(0, "Enter".into()), (1, "Rescue".into())];
            c.goals = vec![
                (0, super::super::coop::GOAL_DONE),
                (1, super::super::coop::GOAL_OPEN),
            ];
            c.show.music = Some(0);
        }
        g.coop_catch_up(1, 5_000);
        let want = g.coop_goal_list(5_000).unwrap();
        assert_eq!(
            sent_to(&g.take_out(), 1),
            vec![want, gs::music_play(b"BGM/a.mp3", true, 4.0)]
        );
        // a round world in round 3, a one-shot track playing: the round, no music
        {
            let c = g.coop.as_mut().unwrap();
            c.mission = false;
            c.round_world = true;
            c.round = 3;
            c.wave = 2;
            c.show.music = Some(1);
        }
        g.coop_catch_up(1, 5_000);
        let sent = sent_to(&g.take_out(), 1);
        assert_eq!(sent.len(), 1);
        let goal = g.coop.as_ref().unwrap().goal as u8;
        assert_eq!(sent[0], gs::coop_mission_list(&[], &[], 3, goal));
    }

    /// DSV-583: a round's weapon row lies at its item spawn point as a drop with the row's lifetime; a row of another
    /// round and a row whose place the world lacks give nothing.
    #[test]
    fn weapon_rows_lie_at_their_places_in_their_round() {
        let doc = dc_bute::ButeDoc::parse(concat!(
            "[Weapon3]\nName = \"G3\"\nHHModel = \"HH\\\\g3.ltb\"\nHHSkin0 = \"HH\\\\g3.dtx\"\n",
            "[WeaponOrder]\nClass0 = \"P\"\nP0 = \"G3\"\n",
        ));
        let tables = RomTables {
            weapons: crate::rom_tables::WeaponTable::from_doc(&doc),
            ..RomTables::default()
        };
        let mut g = coop_game(tables);
        g.coop.as_mut().unwrap().plan.weapon_spawns = vec![
            WeaponSpawnRow {
                round: 11,
                weapon: 3,
                life_s: Some(180.0),
                place: "weaponspawnpoint00".into(),
            },
            WeaponSpawnRow {
                round: 11,
                weapon: 3,
                life_s: None,
                place: "Nowhere".into(),
            },
            WeaponSpawnRow {
                round: 13,
                weapon: 3,
                life_s: None,
                place: "WeaponSpawnPoint00".into(),
            },
        ];
        g.coop_weapon_rows(10, 1_000);
        assert!(g.weapon_drops().is_empty(), "no row of round 10");
        g.coop_weapon_rows(11, 1_000);
        let d = g.weapon_drops();
        assert_eq!(d.len(), 1, "{d:?}");
        let (key, weapon, _rounds, rest) = d[0];
        assert_eq!(weapon, 3);
        assert_eq!((rest[0], rest[2]), (10.0, 20.0));
        assert!(g
            .take_out()
            .iter()
            .any(|o| matches!(o, GameOut::DropCreate(c) if c.key == key)));
        assert_eq!(g.coop.as_ref().unwrap().show.weapon_drops, 1);
        // the row's 180 s, not the drop rules' 30 s
        g.tick_drops(1_000 + 30_001);
        assert_eq!(g.weapon_drops().len(), 1);
        g.tick_drops(1_000 + 180_000);
        assert!(g.weapon_drops().is_empty());
    }
}

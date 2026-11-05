//! The ROM data a co-op (mode class 5) room needs, read from the user's ROM at run time and never copied: the
//! room's GMS file (found through the room table's map rows whose co-op GMS names this world), its per-difficulty
//! row, its spawn tables, health rates, score table and NPC name table, and the AI body rows of the model table.
//!
//! The 2009 client reads none of this beyond the world name (FIRETEAM_2009 §3.3): it is server-only data and the
//! mod surface. Keys are cited as `OBSERVED: ROM table key`; values are read at run time.

use std::collections::BTreeMap;

use dc_bute::{eq_fold, ButeDoc, GameRoomData, SectionMap};
use dc_rez::RezVfs;

use crate::chars::{self, Rig};

/// The co-op GMS keys and sections.
// OBSERVED: ROM table key GAMEROOM.TXT [Map<n>] GMS_<mode key> (two map rows carry the co-op one); GMS [MapInfo]
// Map; FIRETEAM_2009.md §3.1
pub const GMS_KEY_PREFIX: &str = "GMS_";
pub const KEY_MAPINFO_WORLD: (&str, &str) = ("MapInfo", "Map");
/// The per-difficulty row `[Object<goal>]` and its keys.
// OBSERVED: ROM table key GMS [Object<n>] TimeLimit, LifeCount, MaxLifeCount, AIDamageFactor, ScoreFactor,
// ShowCoopResultDelayTime (three rows, FIRETEAM_2009.md §3.1)
pub const DIFFICULTY_SECTION: &str = "Object";
/// `[AISpawnTable<n>]` rows and their keys.
// OBSERVED: ROM table key GMS [AISpawnTable<n>] Round, Spawner<k>, LimitMaxAliveNum, SpawnAIName<k>, SpawnDelay<k>,
// SpawnNum<k> (20 rows in one co-op GMS, none in the other; 2026-10-25 scratch run)
pub const SPAWN_TABLE_SECTION: &str = "AISpawnTable";
/// `[AIHealthRateRoundTable<goal>]` `Round<n>`.
// OBSERVED: ROM table key GMS [AIHealthRateRoundTable<n>] Round<k> (three rows, FIRETEAM_2009.md §3.1)
pub const HEALTH_RATE_SECTION: &str = "AIHealthRateRoundTable";
/// `[CooperativeModeScoreTable]` keys.
// OBSERVED: ROM table key GMS [CooperativeModeScoreTable] DamageScoreType_A..D, DamagePlusByEtcDeath,
// DamagePlusByBossDeath, DamagePlusByHeadShotDeath (FIRETEAM_2009.md §3.1; DC_DEFAULTS FT-13)
pub const SCORE_SECTION: &str = "CooperativeModeScoreTable";
/// `[CooperativeModeNPCNameTable]` `NPCNameBoss`.
// OBSERVED: ROM table key GMS [CooperativeModeNPCNameTable] NPCNameBoss (FIRETEAM_2009.md §3.1; FT-14)
pub const NPC_NAME_SECTION: &str = "CooperativeModeNPCNameTable";
/// `[AIModel<n>]` and `[AIFace<n>]` rows of the model table.
// OBSERVED: ROM table key MODELBUTES.TXT [AIModel<n>] Name, ModelFile, Skin<k>, RenderStyle<k>, Skeleton, AIName,
// AttachFace, AIFace; [AIFace<n>] Name, ModelFile, Skin0, RenderStyle<k> (FIRETEAM_2009.md §3.2)
pub const AI_BODY_SECTION: &str = "AIModel";
/// `[CooperativeModeDialogMsg]`: the dialogue texts the caption and message triggers `play` (DSV-579).
// OBSERVED: ROM table key GMS [CooperativeModeDialogMsg] Cap<nnn> (three fields: picture, speaker, text, split by `|`),
// Msg<nnn> (a plain text); FIRETEAM_2009.md §3.1 (32 keys on one map)
pub const DIALOG_SECTION: &str = "CooperativeModeDialogMsg";
/// `[WeaponSpawn<n>]` rows and their keys (DSV-583).
// OBSERVED: ROM table key GMS [WeaponSpawn<n>] Round, WeaponID, LifeTime, Position (five rows on the round map, none on
// the mission map; Position names an item spawn point of the world)
pub const WEAPON_SPAWN_SECTION: &str = "WeaponSpawn";
pub const AI_FACE_SECTION: &str = "AIFace";

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim()
}

fn text(sec: Option<&SectionMap>, key: &str) -> Option<String> {
    sec?.get(key)
        .map(|v| unquote(v).to_string())
        .filter(|v| !v.is_empty())
}

fn num(sec: Option<&SectionMap>, key: &str) -> Option<f32> {
    let t = text(sec, key)?;
    let t = t.strip_suffix(['f', 'F']).unwrap_or(&t);
    t.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

fn index_of(section: &str, prefix: &str) -> Option<u32> {
    let rest = section.strip_prefix(prefix)?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// The difficulty row's values (`None` = not in the ROM row).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Difficulty {
    pub time_limit_s: Option<u32>,
    pub lives: Option<u32>,
    pub max_lives: Option<u32>,
    pub ai_damage_factor: Option<f32>,
    pub score_factor: Option<f32>,
    pub result_delay_ms: Option<u32>,
    /// The health and armour a human recovers at `healinpart` (DSV-578).
    // OBSERVED: ROM table key GMS [Object<n>] RecoverRoundTotalHP (the round map's three rows, falling with the
    // difficulty)
    pub recover_hp: Option<f32>,
}

/// One spawn group of a spawn-table row: `count` AIs of `template`, one every `delay_s` seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnGroup {
    pub template: String,
    pub delay_s: f32,
    pub count: u32,
}

/// One spawn-table row: a wave.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WaveRow {
    pub round: i32,
    /// Spawner object names.
    pub spawners: Vec<String>,
    pub max_alive: Option<u32>,
    pub groups: Vec<SpawnGroup>,
}

/// The co-op score table (`None` = key absent).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScoreTable {
    /// Hit points by zone: head, body, arms, legs.
    pub hit: [Option<f32>; 4],
    pub kill: Option<f32>,
    pub boss_death: Option<f32>,
    pub head_kill: Option<f32>,
}

/// One weapon row of the GMS: a weapon that lies at a named place from a round on (DSV-583).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponSpawnRow {
    pub round: i32,
    pub weapon: u16,
    /// Seconds it lies there (`None`: the drop rules' lifetime).
    pub life_s: Option<f32>,
    /// The item spawn point's name.
    pub place: String,
}

/// What a co-op room takes from its GMS file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CoopPlan {
    pub difficulty: Difficulty,
    /// Waves in `Round` order (empty: the world has no spawn tables).
    pub rounds: Vec<WaveRow>,
    /// AI health multipliers by wave index (the last repeats).
    pub health_rates: Vec<f32>,
    pub score: ScoreTable,
    pub boss_name: Option<String>,
    /// The difficulty row's goal texts `MissionList<k>` by goal number `k` (the goals a mission world's statements
    /// add and complete, DSV-574).
    pub mission_list: Vec<(u32, String)>,
    /// The dialogue texts by key (DSV-579).
    pub dialog: Vec<(String, String)>,
    /// The weapon rows in `Round` order (DSV-583).
    pub weapon_spawns: Vec<WeaponSpawnRow>,
}

impl CoopPlan {
    /// The plan of a parsed GMS document for difficulty `goal`.
    pub fn from_gms(doc: &ButeDoc, goal: u32) -> CoopPlan {
        let o = doc.get(&format!("{DIFFICULTY_SECTION}{goal}"));
        let int = |k: &str| num(o, k).filter(|v| *v >= 0.0).map(|v| v as u32);
        let difficulty = Difficulty {
            time_limit_s: int("TimeLimit"),
            lives: int("LifeCount"),
            max_lives: int("MaxLifeCount"),
            ai_damage_factor: num(o, "AIDamageFactor").filter(|v| *v >= 0.0),
            score_factor: num(o, "ScoreFactor").filter(|v| *v >= 0.0),
            result_delay_ms: int("ShowCoopResultDelayTime"),
            recover_hp: num(o, "RecoverRoundTotalHP").filter(|v| *v >= 0.0),
        };
        let mut rounds: Vec<WaveRow> = doc
            .iter()
            .filter(|(n, _)| index_of(n, SPAWN_TABLE_SECTION).is_some())
            .map(|(_, sec)| {
                let sec = Some(sec);
                let spawners = (0..64)
                    .filter_map(|k| text(sec, &format!("Spawner{k}")))
                    .collect();
                let groups = (0..16)
                    .filter_map(|k| {
                        let template = text(sec, &format!("SpawnAIName{k}"))?;
                        let count = num(sec, &format!("SpawnNum{k}"))
                            .filter(|v| *v >= 1.0)
                            .map(|v| v as u32)?;
                        let delay_s = num(sec, &format!("SpawnDelay{k}"))
                            .filter(|v| *v >= 0.0)
                            .unwrap_or(0.0);
                        Some(SpawnGroup {
                            template,
                            delay_s,
                            count,
                        })
                    })
                    .collect();
                WaveRow {
                    round: num(sec, "Round").map_or(0, |v| v as i32),
                    spawners,
                    max_alive: num(sec, "LimitMaxAliveNum")
                        .filter(|v| *v >= 1.0)
                        .map(|v| v as u32),
                    groups,
                }
            })
            .filter(|r: &WaveRow| !r.groups.is_empty())
            .collect();
        // stable: rows of the same round keep their section order
        rounds.sort_by_key(|r| r.round);
        let hr = doc.get(&format!("{HEALTH_RATE_SECTION}{goal}"));
        let health_rates = (0..64)
            .map_while(|k| num(hr, &format!("Round{k}")))
            .filter(|v| *v > 0.0)
            .collect();
        let st = doc.get(SCORE_SECTION);
        let score = ScoreTable {
            hit: [
                num(st, "DamageScoreType_A"),
                num(st, "DamageScoreType_B"),
                num(st, "DamageScoreType_C"),
                num(st, "DamageScoreType_D"),
            ],
            kill: num(st, "DamagePlusByEtcDeath"),
            boss_death: num(st, "DamagePlusByBossDeath"),
            head_kill: num(st, "DamagePlusByHeadShotDeath"),
        };
        CoopPlan {
            difficulty,
            rounds,
            health_rates,
            score,
            boss_name: text(doc.get(NPC_NAME_SECTION), "NPCNameBoss"),
            // OBSERVED: ROM table key GMS [Object<n>] MissionList<k> (five per row on the mission map)
            mission_list: (0..32u32)
                .filter_map(|k| Some((k, text(o, &format!("MissionList{k}"))?)))
                .collect(),
            dialog: doc
                .get(DIALOG_SECTION)
                .map(|sec| {
                    sec.iter()
                        .map(|(k, v)| (k.trim().to_string(), unquote(v).to_string()))
                        .filter(|(_, v)| !v.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            weapon_spawns: weapon_rows(doc),
        }
    }

    /// The dialogue text of `key` (ASCII case-insensitive).
    pub fn dialog_text(&self, key: &[u8]) -> Option<&str> {
        self.dialog
            .iter()
            .find(|(k, _)| k.as_bytes().eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// The `[WeaponSpawn<n>]` rows with a round, a weapon id and a place, in `Round` order (stable).
fn weapon_rows(doc: &ButeDoc) -> Vec<WeaponSpawnRow> {
    let mut rows: Vec<WeaponSpawnRow> = doc
        .iter()
        .filter(|(n, _)| index_of(n, WEAPON_SPAWN_SECTION).is_some())
        .filter_map(|(_, sec)| {
            let sec = Some(sec);
            Some(WeaponSpawnRow {
                round: num(sec, "Round")? as i32,
                weapon: num(sec, "WeaponID").filter(|v| *v >= 0.0 && *v <= 65535.0)? as u16,
                life_s: num(sec, "LifeTime").filter(|v| *v > 0.0),
                place: text(sec, "Position")?,
            })
        })
        .collect();
    rows.sort_by_key(|r| r.round);
    rows
}

impl CoopPlan {
    /// The difficulty's last round on a round world (DSV-584): the highest `Round<k>` of the difficulty's health-rate
    /// row (its `Round0` stands before the first round), when it lies within the spawn tables; else the spawn-table
    /// rows.
    // DC-DEFAULT: DSV-584 (proposed)
    pub fn last_round(&self) -> usize {
        let n = self.health_rates.len().saturating_sub(1);
        if n >= 1 && n <= self.rounds.len() {
            n
        } else {
            self.rounds.len()
        }
    }
}

/// The base name of a world path without folder and `.dat` extension, upper case.
fn world_base(p: &str) -> String {
    let n = p.replace('/', "\\");
    let b = n.rsplit('\\').next().unwrap_or(&n).to_ascii_uppercase();
    b.strip_suffix(".DAT").map(str::to_string).unwrap_or(b)
}

/// The GMS file of the room's mode (`key=` of the room line) whose `[MapInfo]` world is this world: the map rows
/// of the room table are tried in order.
// SPEC: DSERVER_PIPE.md §3.2 (room line `key`); FIRETEAM_2009.md §3.1 (GMS `[MapInfo] Map` names the world)
pub fn find_gms(vfs: &RezVfs, mode_key: &str, world: &str) -> Option<ButeDoc> {
    let gr = GameRoomData::from_vfs(vfs).ok()?;
    let mode = gr
        .modes(false)
        .iter()
        .position(|m| m.get("Key").is_some_and(|k| eq_fold(unquote(k), mode_key)))?;
    let want = world_base(world);
    let maps = gr.maps();
    for (i, m) in maps.iter().enumerate() {
        if !m.contains_key(&format!("{GMS_KEY_PREFIX}{mode_key}")) {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(vfs, &gr, i, mode, false);
        let names_this = doc
            .get_str(KEY_MAPINFO_WORLD.0, KEY_MAPINFO_WORLD.1)
            .is_some_and(|w| world_base(unquote(w)) == want);
        if names_this {
            return Some(doc);
        }
    }
    None
}

/// The first GMS file of the room's mode, whatever world it names: a community map has no row of its own.
// DC-DEFAULT: MAP-113 (proposed)
pub fn first_mode_gms(vfs: &RezVfs, mode_key: &str) -> Option<ButeDoc> {
    let gr = GameRoomData::from_vfs(vfs).ok()?;
    let mode = gr
        .modes(false)
        .iter()
        .position(|m| m.get("Key").is_some_and(|k| eq_fold(unquote(k), mode_key)))?;
    let row = format!("{GMS_KEY_PREFIX}{mode_key}");
    let map = gr.maps().iter().position(|m| m.contains_key(&row))?;
    Some(dc_bute::gms_file_with_vfs(vfs, &gr, map, mode, false).1)
}

/// One `[AIModel<n>]` row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AiBodyRow {
    pub index: u32,
    pub name: String,
    pub model: String,
    pub skins: Vec<String>,
    pub styles: Vec<String>,
    pub skeleton: Option<u32>,
    pub ai_name: Option<String>,
    /// `AttachFace` non-zero and the `[AIFace<n>]` row named by `AIFace`.
    pub face: Option<String>,
}

/// One `[AIFace<n>]` row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AiFaceRow {
    pub name: String,
    pub model: String,
    pub skins: Vec<String>,
    pub styles: Vec<String>,
}

/// The AI rows of the model table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AiTables {
    pub bodies: BTreeMap<u32, AiBodyRow>,
    pub faces: Vec<AiFaceRow>,
}

fn numbered(sec: &SectionMap, key: &str, max: u32) -> Vec<String> {
    (0..max)
        .map_while(|k| text(Some(sec), &format!("{key}{k}")))
        .collect()
}

impl AiTables {
    pub fn from_doc(doc: &ButeDoc) -> AiTables {
        let mut t = AiTables::default();
        for (name, sec) in doc.iter() {
            if let Some(n) = index_of(name, AI_BODY_SECTION) {
                let Some(model) = text(Some(sec), "ModelFile") else {
                    continue;
                };
                let attach = num(Some(sec), "AttachFace").is_some_and(|v| v != 0.0);
                t.bodies.insert(
                    n,
                    AiBodyRow {
                        index: n,
                        name: text(Some(sec), "Name").unwrap_or_default(),
                        model,
                        skins: numbered(sec, "Skin", 8),
                        styles: numbered(sec, "RenderStyle", chars::BODY_STYLES),
                        skeleton: num(Some(sec), "Skeleton")
                            .filter(|v| *v >= 0.0)
                            .map(|v| v as u32),
                        ai_name: text(Some(sec), "AIName"),
                        face: text(Some(sec), "AIFace").filter(|_| attach),
                    },
                );
            } else if index_of(name, AI_FACE_SECTION).is_some() {
                let Some(model) = text(Some(sec), "ModelFile") else {
                    continue;
                };
                t.faces.push(AiFaceRow {
                    name: text(Some(sec), "Name").unwrap_or_default(),
                    model,
                    skins: numbered(sec, "Skin", 8),
                    styles: numbered(sec, "RenderStyle", chars::FACE_STYLES),
                });
            }
        }
        t
    }

    pub fn from_vfs(vfs: &RezVfs) -> AiTables {
        ButeDoc::try_from_vfs(
            vfs,
            crate::rom_tables::ATTRIBUTES_REZ,
            chars::MODELBUTES_ENTRY,
        )
        .ok()
        .flatten()
        .map(|d| Self::from_doc(&d))
        .unwrap_or_default()
    }

    /// The body row whose `Name` is `name` (ASCII case-insensitive).
    pub fn body_named(&self, name: &str) -> Option<&AiBodyRow> {
        self.bodies.values().find(|b| eq_fold(&b.name, name))
    }

    pub fn face_named(&self, name: &str) -> Option<&AiFaceRow> {
        self.faces.iter().find(|f| eq_fold(&f.name, name))
    }
}

/// Whether an AI body shares the player rig, so that the player's three trackers, clip indices and the weapon at
/// socket 2 apply unchanged: the same clip child models as the player body, the weapon socket key at the grip
/// socket, and the stand and walk clips.
// SPEC: FIRETEAM_2009.md §1.8 (9 of the 12 AI rows: 53 nodes, socket 2 with the player weapon-socket key, the same
// single child clip model; the other 3 need their own clips and cannot hold a weapon at socket 2)
// DC-DEFAULT: FT-S2 (proposed) only player-rig AI bodies are used in slice 1
pub fn shares_player_rig(ai: &Rig, player: &Rig, socket_is_weapon: bool) -> bool {
    socket_is_weapon
        && !ai.children.is_empty()
        && ai.children.len() == player.children.len()
        && ai
            .children
            .iter()
            .zip(player.children.iter())
            .all(|(a, b)| eq_fold(a, b))
        && [
            chars::CLIP_UPPER_STAND,
            chars::CLIP_UPPER_WALK,
            chars::CLIP_LOWER_STAND,
            chars::CLIP_LOWER_WALK,
        ]
        .iter()
        .all(|k| ai.clip(*k).is_some())
}

/// Whether the body model file has the weapon-socket key at the socket the rig names (read again: the rig's grip
/// socket falls back to 2 when the key is missing).
// SPEC: FIRETEAM_2009.md §1.8 (socket index 2 = the player weapon-socket key on the 9 rows)
pub fn weapon_socket_present(vfs: &RezVfs, body: &str, socket: u32) -> bool {
    let Some((rez, entry)) = chars::archive_of(body) else {
        return false;
    };
    let Ok(Some(bytes)) = vfs.entry(&rez, &entry) else {
        return false;
    };
    let Ok(m) = dc_model::Model::parse_without_tracks(&bytes) else {
        return false;
    };
    m.sockets
        .get(socket as usize)
        .is_some_and(|s| chars::name_key(&s.name.0) == chars::SOCKET_WEAPON)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GMS: &str = concat!(
        "[MapInfo]\n",
        "Map = \"Worlds\\\\Synth.dat\"\n",
        "[Object1]\n",
        "TimeLimit = 600\n",
        "LifeCount = 3\n",
        "MaxLifeCount = 5\n",
        "AIDamageFactor = \"1.5\"\n",
        "ScoreFactor = 2\n",
        "[AISpawnTable1]\n",
        "Round = 2\n",
        "Spawner0 = \"s3\"\n",
        "LimitMaxAliveNum = 4\n",
        "SpawnAIName0 = \"t1\"\n",
        "SpawnDelay0 = \"0.5\"\n",
        "SpawnNum0 = 2\n",
        "[AISpawnTable0]\n",
        "Round = 1\n",
        "Spawner0 = \"s1\"\n",
        "Spawner1 = \"s2\"\n",
        "SpawnAIName0 = \"t0\"\n",
        "SpawnDelay0 = 1\n",
        "SpawnNum0 = 3\n",
        "SpawnAIName1 = \"\"\n",
        "SpawnNum1 = 0\n",
        "[AIHealthRateRoundTable1]\n",
        "Round0 = \"1.0\"\n",
        "Round1 = \"1.5\"\n",
        "[CooperativeModeScoreTable]\n",
        "DamageScoreType_A = 10\n",
        "DamagePlusByEtcDeath = 40\n",
        "[CooperativeModeNPCNameTable]\n",
        "NPCNameBoss = \"big\"\n",
    );

    #[test]
    fn plan_from_a_synthetic_gms() {
        let d = ButeDoc::parse(GMS);
        let p = CoopPlan::from_gms(&d, 1);
        assert_eq!(p.difficulty.time_limit_s, Some(600));
        assert_eq!(p.difficulty.lives, Some(3));
        assert_eq!(p.difficulty.ai_damage_factor, Some(1.5));
        assert_eq!(p.rounds.len(), 2);
        assert_eq!(p.rounds[0].round, 1, "sorted by Round");
        assert_eq!(
            p.rounds[0].spawners,
            vec!["s1".to_string(), "s2".to_string()]
        );
        assert_eq!(p.rounds[0].groups.len(), 1, "an empty group is dropped");
        assert_eq!(p.rounds[0].groups[0].count, 3);
        assert_eq!(p.rounds[1].max_alive, Some(4));
        assert_eq!(p.rounds[1].groups[0].delay_s, 0.5);
        assert_eq!(p.health_rates, vec![1.0, 1.5]);
        assert_eq!(p.score.hit[0], Some(10.0));
        assert_eq!(p.score.kill, Some(40.0));
        assert_eq!(p.boss_name.as_deref(), Some("big"));
        // another difficulty: no row, no rates
        let q = CoopPlan::from_gms(&d, 0);
        assert_eq!(q.difficulty, Difficulty::default());
        assert!(q.health_rates.is_empty());
        assert_eq!(world_base("WORLDS\\SYNTH.DAT"), "SYNTH");
    }

    #[test]
    fn ai_rows_from_a_synthetic_table() {
        let d = ButeDoc::parse(concat!(
            "[AIModel0]\n",
            "Name = \"a\"\n",
            "ModelFile = \"M\\\\a.ltb\"\n",
            "Skin0 = \"M\\\\a.dtx\"\n",
            "Skin1 = \"M\\\\b.dtx\"\n",
            "RenderStyle0 = \"R\\\\a.ltb\"\n",
            "Skeleton = 1\n",
            "AIName = \"Alpha\"\n",
            "AttachFace = 1\n",
            "AIFace = \"f\"\n",
            "[AIModel1]\n",
            "Name = \"b\"\n",
            "ModelFile = \"M\\\\b.ltb\"\n",
            "AttachFace = 0\n",
            "AIFace = \"f\"\n",
            "[AIFace0]\n",
            "Name = \"f\"\n",
            "ModelFile = \"F\\\\f.ltb\"\n",
            "Skin0 = \"F\\\\f.dtx\"\n",
            "[Model0]\n",
            "ModelFile = \"P\\\\p.ltb\"\n",
        ));
        let t = AiTables::from_doc(&d);
        assert_eq!(t.bodies.len(), 2);
        let a = t.body_named("A").unwrap();
        assert_eq!((a.skins.len(), a.styles.len(), a.skeleton), (2, 1, Some(1)));
        assert_eq!(a.face.as_deref(), Some("f"));
        assert_eq!(t.body_named("b").unwrap().face, None, "AttachFace 0");
        assert_eq!(t.face_named("F").unwrap().skins.len(), 1);
    }
}

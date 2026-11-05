//! The killer's own kill banners of the 2009 HUD: the one-kill banner with the own kill count, the head-shot and
//! groin-shot banners, the multi-kill banners (levels 2 to 7 of the 0x6d `kill_kind`), the ice-crush banner, and the
//! server-ordered banners of 0xc8 (the "N COMBO" form and its one-kill / head-shot forms), each with the client's own
//! voice line from the interface sound table. The first-kill banner stays with [`super::items`] (`opener_banner`);
//! this module only plays its voice line.
//!
//! The 2009 element is `hud_multi_kill`: one active banner slot at a time, a new own kill clears the old one first.
//! Images, sizes, places and timing come from the HUD layout set (ROM keys pinned in [`dc_ui_data::HudKey`]); the
//! voice files from the client attribute file's interface section, both read at run time. No ROM text is here: the
//! sound keys are read from the user's CShell.dll at their pinned rvas.
// EVIDENCE: cshell+0x2e0bd..0x2e1d6 (0x6d: the own head-shot / groin-shot flags, then the announcement for any known
// killer), cshell+0xa5fe0 (the announcement: banner and voice only when the killer's name is the own name)
// EVIDENCE: cshell+0x2d8ca..0x2d918 (0xc8), cshell+0xa52a0 (combo form), cshell+0xa57b0 (one-kill / head-shot form)
// EVIDENCE: cshell+0xa1320 (layout: 17 banner slots of 0x88 bytes, images and sizes by slot, the head-shot and
// groin-shot decorations, the count digits)

use super::items::{announce_phase, DrawItem};
use super::layout::Announce;
use super::state::HudState;
use dc_botclient::shell::{KillRecord, NO_KILLER};
use dc_ui_data::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::{HudKey as K, HudView};
use std::collections::BTreeMap;
use std::path::Path;

/// `hit_zone` of a head hit and of the groin zone (the client's two kill marks).
// SPEC: GAME_MESSAGES_2009_S2C.md §3.3.1 field 5 (1 = head; 5 = the special zone)
// EVIDENCE: cshell+0x2db9e..0x2dbe2 (zone 1 sets the head-shot flag, zone 5 the special flag that selects the groin
// banner in cshell+0xa62cf)
pub const ZONE_HEAD: u8 = 1;
pub const ZONE_GROIN: u8 = 5;

/// A banner slot of `hud_multi_kill` (the slot index is the 2009 one).
// EVIDENCE: cshell+0xa1320 (slot order of the image keys), cshell+0xa5fe0 (slot numbers it selects)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    Opener = 0,
    OneKill = 1,
    OneKills = 2,
    Double = 3,
    Multi = 4,
    Ultra = 5,
    FiveFold = 6,
    Unbel = 7,
    UnbelPlus = 8,
    HeadHit = 9,
    Combo = 10,
    IceCrush = 12,
    GroinShot = 13,
}

/// A voice line of the interface sound table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Voice {
    Opener,
    OneKill,
    HeadHit,
    Double,
    Multi,
    Ultra,
    FiveFold,
    Unbel,
    UnbelPlus,
    IceCrush,
    GroinMale,
    GroinFemale,
    /// Round flow: the 0x45 start voice (own state 1), or the first-spawn voice when the waiting flag is set.
    RoundStart,
    FirstSpawn,
    /// 0x46: the win voice of the first / second team; class 11 plays the human / infector pair instead.
    WinAlpha,
    WinBravo,
    WinHuman,
    WinInfector,
    /// 0x46 with no winner: no 2009 evidence of a voice; only played when the draw voice is switched on.
    // DC-DEFAULT: CLI-410 (staged) off
    RoundDraw,
    /// 0xd6: the HiSec case sounds (dropped, timer running out, picked up by me, picked up).
    CaseDrop,
    CaseTimer,
    CaseByMe,
    CasePicked,
    /// 0xc6 (flag modes, cshell+0x86da0): the own taking / returning, the enemy flag taken / captured / returned,
    /// the own flag taken / captured / returned.
    FlagByMe,
    FlagEnemyTaken,
    FlagOwnTaken,
    FlagOwnReturned,
    FlagEnemyReturned,
    FlagEnemyCaptured,
    FlagOwnCaptured,
    /// 0x51 (cshell+0x4afee): the bomb is planted.
    BombSet,
    /// The bomb timer's beep per whole second (cshell+0x6fda0): above 30 s, 20 s, 10 s, 5 s, 0 s left.
    BombBeep1,
    BombBeep2,
    BombBeep3,
    BombBeep4,
    BombBeep5,
    /// 0xd3 sub 1 with a countdown just under 20 s (cshell+0xb1190): the infection mode's intro.
    QrtIntro,
    /// The outbreak countdown's whole seconds below 11 (cshell+0xb27b0): the key's value is a form with one `%d`.
    QrtCount(u8),
    /// Spy Hunt: an intel case dropped (notice state 4), one seized (states 0 / 2), every case seized (1 / 3).
    // EVIDENCE: cshell+0x70880 (the three keys), cshell+0x71c30 (state 4 -> +0x68, 0 / 2 -> +0xe7, 1 / 3 -> +0x166)
    BntIntelDrop,
    BntIntelGet,
    BntIntelGetAll,
    /// Fire Team: the mode's message sound with each 0xc9 banner (sub 2) and text note (sub 3).
    // EVIDENCE: cshell+0x31490 (the key pushed to cshell+0x300a0 after cshell+0xbdd20 and cshell+0x81fe0)
    CoopMessage,
    /// Fire Team: the pickup sound of a drop-item note (0xc9 sub 6: 1 the health pack, 2 the ammunition, 3 the
    /// ammunition box).
    // EVIDENCE: cshell+0x3194a..0x3197b (the item byte selects one of three keys pushed to cshell+0x300a0)
    CoopMedikit,
    CoopMagazine,
    CoopBulletbox,
}

impl Voice {
    pub const ALL: [Voice; 45] = [
        Voice::Opener,
        Voice::OneKill,
        Voice::HeadHit,
        Voice::Double,
        Voice::Multi,
        Voice::Ultra,
        Voice::FiveFold,
        Voice::Unbel,
        Voice::UnbelPlus,
        Voice::IceCrush,
        Voice::GroinMale,
        Voice::GroinFemale,
        Voice::RoundStart,
        Voice::FirstSpawn,
        Voice::WinAlpha,
        Voice::WinBravo,
        Voice::WinHuman,
        Voice::WinInfector,
        Voice::RoundDraw,
        Voice::CaseDrop,
        Voice::CaseTimer,
        Voice::CaseByMe,
        Voice::CasePicked,
        Voice::FlagByMe,
        Voice::FlagEnemyTaken,
        Voice::FlagOwnTaken,
        Voice::FlagOwnReturned,
        Voice::FlagEnemyReturned,
        Voice::FlagEnemyCaptured,
        Voice::FlagOwnCaptured,
        Voice::BombSet,
        Voice::BombBeep1,
        Voice::BombBeep2,
        Voice::BombBeep3,
        Voice::BombBeep4,
        Voice::BombBeep5,
        Voice::QrtIntro,
        Voice::QrtCount(0),
        Voice::BntIntelDrop,
        Voice::BntIntelGet,
        Voice::BntIntelGetAll,
        Voice::CoopMessage,
        Voice::CoopMedikit,
        Voice::CoopMagazine,
        Voice::CoopBulletbox,
    ];

    /// Our name of the line (sound log).
    pub fn label(self) -> &'static str {
        match self {
            Voice::Opener => "opener",
            Voice::OneKill => "single_kill",
            Voice::HeadHit => "head_shot",
            Voice::Double => "pair_kill",
            Voice::Multi => "multi_kill_3",
            Voice::Ultra => "multi_kill_4",
            Voice::FiveFold => "multi_kill_5",
            Voice::Unbel => "multi_kill_6",
            Voice::UnbelPlus => "multi_kill_7",
            Voice::IceCrush => "ice_crush",
            Voice::GroinMale => "groin_male",
            Voice::GroinFemale => "groin_female",
            Voice::RoundStart => "round_start",
            Voice::FirstSpawn => "first_spawn",
            Voice::WinAlpha => "round_win_first_team",
            Voice::WinBravo => "round_win_second_team",
            Voice::WinHuman => "round_win_humans",
            Voice::WinInfector => "round_win_infectors",
            Voice::RoundDraw => "round_draw",
            Voice::CaseDrop => "case_drop",
            Voice::CaseTimer => "case_timer_running_out",
            Voice::CaseByMe => "case_picked_up_by_me",
            Voice::CasePicked => "case_picked_up",
            Voice::FlagByMe => "flag_by_me",
            Voice::FlagEnemyTaken => "flag_enemy_taken",
            Voice::FlagOwnTaken => "flag_own_taken",
            Voice::FlagOwnReturned => "flag_own_returned",
            Voice::FlagEnemyReturned => "flag_enemy_returned",
            Voice::FlagEnemyCaptured => "flag_enemy_captured",
            Voice::FlagOwnCaptured => "flag_own_captured",
            Voice::BombSet => "bomb_set",
            Voice::BombBeep1 => "bomb_beep_1",
            Voice::BombBeep2 => "bomb_beep_2",
            Voice::BombBeep3 => "bomb_beep_3",
            Voice::BombBeep4 => "bomb_beep_4",
            Voice::BombBeep5 => "bomb_beep_5",
            Voice::QrtIntro => "quarantine_intro",
            Voice::QrtCount(_) => "quarantine_count",
            Voice::BntIntelDrop => "spy_intel_drop",
            Voice::BntIntelGet => "spy_intel_get",
            Voice::BntIntelGetAll => "spy_intel_get_all",
            Voice::CoopMessage => "coop_message",
            Voice::CoopMedikit => "coop_pickup_health",
            Voice::CoopMagazine => "coop_pickup_ammo",
            Voice::CoopBulletbox => "coop_pickup_ammo_box",
        }
    }

    /// The pinned key literal the client passes to its interface sound lookup.
    // EVIDENCE: cshell+0xa6238..0xa6794 (the key pushed per form before cshell+0x1b2c00), cshell+0xa52fe and
    // cshell+0xa586c (the head-shot key), cshell+0xa58a5 (the one-kill key); cshell+0x49f2f / cshell+0x49f16 (the
    // round-start and first-spawn keys), cshell+0xc1a50 (the team-win and infection-pair keys, pushed before
    // cshell+0x1b2c00), cshell+0x98d50 (the four HiSec keys by kind 0..3, same lookup)
    // The draw voice has no literal in the module (cshell+0xc1a50 plays nothing for win team 0): it shares the
    // first team's win pin and `load_voices` derives its key from that.
    pub fn pin(self) -> Pin {
        let (rva, sha16, len) = match self {
            Voice::Opener => (0x3d8254, "a9bd663d8c6d3c3a", 14),
            Voice::OneKill => (0x3d8194, "f91e811b98663635", 12),
            Voice::HeadHit => (0x3d8180, "904e868c7e83bc46", 17),
            Voice::Double => (0x3d8204, "c01dd8d9eb03cd72", 15),
            Voice::Multi => (0x3d81f4, "eba2923e4845a0eb", 14),
            Voice::Ultra => (0x3d81e4, "2510eeb300b0ae8c", 14),
            Voice::FiveFold => (0x3d81d0, "926d876518f27204", 18),
            Voice::Unbel => (0x3d81bc, "399a2a85f48e5268", 17),
            Voice::UnbelPlus => (0x3d81a4, "f620bddcb4f7d1d3", 21),
            Voice::IceCrush => (0x3d8244, "cd1e34b185084c55", 13),
            Voice::GroinMale => (0x3d822c, "1b1bfe691c4777fb", 20),
            Voice::GroinFemale => (0x3d8214, "4d43909e475c7b1c", 22),
            Voice::RoundStart => (0x3d0dc4, "ea9ef9bf4d197c53", 15),
            Voice::FirstSpawn => (0x3d0dd4, "fdabbb65adcd9cf9", 15),
            Voice::WinAlpha | Voice::RoundDraw => (0x3d9aa4, "0f52ec0f51f27358", 22),
            Voice::WinBravo => (0x3d9a8c, "2cf848f69ac4d5e3", 22),
            Voice::WinHuman => (0x3d9af8, "aba0aba95847d2ce", 16),
            Voice::WinInfector => (0x3d9ad0, "bdd4bf0ecedc09ee", 19),
            Voice::CaseDrop => (0x3d7364, "d0667574f811e531", 13),
            Voice::CaseTimer => (0x3d7348, "42e2cd74c01f9a73", 24),
            Voice::CaseByMe => (0x3d731c, "c22e6aed2709a04c", 21),
            Voice::CasePicked => (0x3d7334, "18c6f39e819acbed", 17),
            // EVIDENCE: cshell+0x86210 (the seven keys read through cshell+0x1b2c00 into the flag element),
            // cshell+0x86da0 (which one plays for an event)
            Voice::FlagByMe => (0x3d628c, "bdf4dce1e72e9927", 17),
            Voice::FlagEnemyTaken => (0x3d6278, "4cbe16a87829845f", 19),
            Voice::FlagOwnTaken => (0x3d6264, "5f6caec5fd15d5a5", 18),
            Voice::FlagOwnReturned => (0x3d6250, "481cbb2973b683e1", 19),
            Voice::FlagEnemyReturned => (0x3d623c, "e2a00b522727dc8b", 18),
            Voice::FlagEnemyCaptured => (0x3d6224, "703937ce9bea32c4", 22),
            Voice::FlagOwnCaptured => (0x3d6210, "23bdde1e7f429c35", 19),
            // EVIDENCE: cshell+0x4afee (0x51: the key read through cshell+0x1b2c00, played by cshell+0x1b20c0),
            // cshell+0x6fda0 (the five beep keys, same lookup)
            Voice::BombSet => (0x3d0a84, "9d55b46ede8635c0", 20),
            Voice::BombBeep1 => (0x3d4bac, "2e456cf661d79cec", 15),
            Voice::BombBeep2 => (0x3d4b9c, "ff7d7ef2ea0375f0", 15),
            Voice::BombBeep3 => (0x3d4b8c, "2ada01f4fd8a6c44", 15),
            Voice::BombBeep4 => (0x3d4b7c, "0fd1a33dc04cebf7", 15),
            Voice::BombBeep5 => (0x3d4b6c, "bfb89d1aac6293be", 15),
            // EVIDENCE: cshell+0xb1190 (the intro key), cshell+0xb27b0 (the countdown form key, `sprintf` with the
            // whole seconds left)
            Voice::QrtIntro => (0x3d89a4, "4cfe7164f0e2794b", 13),
            Voice::QrtCount(_) => (0x3d8a3c, "2acd2a951bad7a86", 12),
            // EVIDENCE: cshell+0x70880 (the three interface sound keys of the Spy Hunt element)
            Voice::BntIntelDrop => (0x3d4cdc, "3cecedd91b47f529", 25),
            Voice::BntIntelGet => (0x3d4cc0, "9cdeb100a223358c", 24),
            Voice::BntIntelGetAll => (0x3d4ca4, "9033dbd43c23ec19", 27),
            // EVIDENCE: cshell+0x300a0 (the key looked up through cshell+0x1b2c00 and played by cshell+0x1b20c0)
            Voice::CoopMessage => (0x3cee90, "eeac58fd8dad2b9d", 16),
            Voice::CoopMedikit => (0x3cee54, "156f337822ca99ca", 16),
            Voice::CoopMagazine => (0x3cee68, "9e5134d8fb981f07", 17),
            Voice::CoopBulletbox => (0x3cee7c, "adf84eb64b4c0000", 18),
        };
        Pin {
            what: "interface sound key",
            rva,
            sha16,
            len,
        }
    }
}

/// The section literal of the interface sound lookup (the client attribute file's section of these keys).
// EVIDENCE: cshell+0x1b2c14 (the section literal pushed by the lookup)
pub const SOUND_SECTION: Pin = Pin {
    what: "interface sound section",
    rva: 0x3dea28,
    sha16: "c26b3ed4ce70e202",
    len: 9,
};

/// What a kill or a 0xc8 starts: the slot, the count digits (one-kill and combo forms) and the voice line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub slot: Option<Slot>,
    pub number: Option<u32>,
    pub voice: Option<Voice>,
}

/// The form of an own kill (cshell+0xa5fe0 as the 0x6d handler calls it): the first-kill flag wins, then the
/// ice-crush flag (`feed2`), then the level: 1 the head-shot banner on a head hit, the groin banner on the groin
/// zone, else the kill-count banner with the own kills (singular image for 1); 2..6 the multi-kill forms; 7 and
/// more the last form; 0 nothing.
// DISASM: cshell+0x2e163..0x2e1d6 (`streak_flag` set: arguments level 0 and first 1; else the level and first 0;
// `feed2` is the seventh argument), cshell+0xa6213 (first), cshell+0xa6253 (ice-crush), cshell+0xa628b (level - 1
// in 0..5 by the jump table at VA 0x371a6bb0, 7 and more at cshell+0xa676b, 0 nothing), cshell+0xa629e (level 1:
// head flag, groin flag, else the count: slot 1 for one kill, else 2)
// DC-DEFAULT: CLI-175 (staged) the groin voice in its male form (the client picks by the own character's gender,
// cshell+0x1e1ad0, which the HUD does not know)
pub fn pick_kill(
    level: u8,
    first: bool,
    ice_crush: bool,
    head: bool,
    groin: bool,
    own_kills: i32,
) -> Pick {
    let p = |slot, number, voice| Pick {
        slot: Some(slot),
        number,
        voice: Some(voice),
    };
    if first {
        return Pick {
            slot: None,
            number: None,
            voice: Some(Voice::Opener),
        };
    }
    if ice_crush {
        return p(Slot::IceCrush, None, Voice::IceCrush);
    }
    match level {
        0 => Pick {
            slot: None,
            number: None,
            voice: None,
        },
        1 if head => p(Slot::HeadHit, None, Voice::HeadHit),
        1 if groin => p(Slot::GroinShot, None, Voice::GroinMale),
        1 => {
            let n = own_kills.max(0) as u32;
            let slot = if n == 1 {
                Slot::OneKill
            } else {
                Slot::OneKills
            };
            p(slot, Some(n), Voice::OneKill)
        }
        2 => p(Slot::Double, None, Voice::Double),
        3 => p(Slot::Multi, None, Voice::Multi),
        4 => p(Slot::Ultra, None, Voice::Ultra),
        5 => p(Slot::FiveFold, None, Voice::FiveFold),
        6 => p(Slot::Unbel, None, Voice::Unbel),
        _ => p(Slot::UnbelPlus, None, Voice::UnbelPlus),
    }
}

/// The form of a 0xc8 `[kills:u16][head:bool1][combo:u32]`: `combo` > 1 the combo banner with its digits (a voice
/// only for `head`); else the head-shot banner for `head`, else the kill-count banner with `kills` (nothing for 0).
// DISASM: cshell+0x2d8ef..0x2d918, cshell+0xa52a0 (slot 10; head-shot key only when the flag is set),
// cshell+0xa5853..0xa58c2 (head: slot 9; else `kills` > 0: slot 1 for 1, else 2, with the one-kill key)
pub fn pick_run(kills: u16, head: bool, combo: u32) -> Pick {
    if combo > 1 {
        return Pick {
            slot: Some(Slot::Combo),
            number: Some(combo),
            voice: head.then_some(Voice::HeadHit),
        };
    }
    if head {
        return Pick {
            slot: Some(Slot::HeadHit),
            number: None,
            voice: Some(Voice::HeadHit),
        };
    }
    if kills == 0 {
        return Pick {
            slot: None,
            number: None,
            voice: None,
        };
    }
    let slot = if kills == 1 {
        Slot::OneKill
    } else {
        Slot::OneKills
    };
    Pick {
        slot: Some(slot),
        number: Some(u32::from(kills)),
        voice: Some(Voice::OneKill),
    }
}

/// The banner on screen: its slot, digits, decorations and start time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BannerShow {
    pub slot: Slot,
    pub number: Option<u32>,
    /// The head-shot decoration (shown with any slot when the kill was a head shot).
    pub skull: bool,
    /// The groin-shot decoration.
    pub groin: bool,
    pub at: u32,
}

/// The banner state kept in [`HudState::banner`]: the banner on screen and the voice lines not yet played.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BannerState {
    pub show: Option<BannerShow>,
    pub voices: Vec<Voice>,
    /// Banners started (reports).
    pub started: u32,
}

impl BannerState {
    fn start(&mut self, pick: Pick, skull: bool, groin: bool, now: u32) {
        self.show = pick.slot.map(|slot| BannerShow {
            slot,
            number: pick.number,
            skull,
            groin,
            at: now,
        });
        if self.show.is_some() {
            self.started += 1;
        }
        if let Some(v) = pick.voice {
            self.voices.push(v);
        }
    }
}

/// 0x6d after the counting: the own kill's banner and voice line (other killers start nothing here).
// EVIDENCE: cshell+0x2e0fe..0x2e124 (the own-name test before the head-shot flags; a known killer record for the
// announcement), cshell+0xa6073..0xa6186 (an own kill first clears the banner on screen), cshell+0xa69a7 (the
// banner and its voice only for the own kill)
pub fn on_kill(s: &mut HudState, k: &KillRecord, now: u32) {
    if k.killer == NO_KILLER || !s.roster.contains_key(&k.killer) || s.own_pid != Some(k.killer) {
        return;
    }
    let (head, groin) = (k.hit_zone == ZONE_HEAD, k.hit_zone == ZONE_GROIN);
    // DC-DEFAULT: CLI-1850 (proposed) in Gun Game the roster's kill column is the rung (0 at the first kill, and it
    // drops on a knife death), so the banner's number is the HUD's own count of the foe kills it saw
    let own_kills = match &s.gun_game {
        Some(g) => g.kills.min(i32::MAX as u32) as i32,
        None => s.roster.get(&k.killer).map_or(0, |r| r.kills),
    };
    let pick = pick_kill(
        k.kill_kind,
        k.streak_flag != 0,
        k.feed2,
        head,
        groin,
        own_kills,
    );
    s.banner.show = None;
    if pick.slot.is_some() {
        // one banner at a time: the first-kill banner of the opener goes too
        s.combo_at = None;
    }
    s.banner.start(pick, head, groin, now);
}

/// 0xc8: the ordered banner on the receiving client.
// EVIDENCE: cshell+0xa52e0 / cshell+0xa57f9 (the flag is the head-shot decoration, the third argument 0 the groin)
pub fn on_run(s: &mut HudState, kills: u16, head: bool, combo: u32, now: u32) {
    let pick = pick_run(kills, head, combo);
    if pick.slot.is_some() {
        s.combo_at = None;
        s.banner.show = None;
    }
    s.banner.start(pick, head, false, now);
}

/// The banner layout: one image and size per slot, the text place, the decorations, the count digits, the timing,
/// and the voice files.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BannerLayout {
    pub slots: BTreeMap<Slot, (String, [i32; 2])>,
    /// The banner text place: x is the centre, y the top.
    pub text_pos: [i32; 2],
    /// (image, place: x centre / y top, size) of the head-shot and groin-shot decorations.
    pub skull: Option<(String, [i32; 2], [i32; 2])>,
    pub groin: Option<(String, [i32; 2], [i32; 2])>,
    /// Digit texture prefix (`<prefix><d>.dtx`), digit size, the gap between digits and the gap before the text.
    pub digits: Option<String>,
    pub digit_size: [i32; 2],
    pub spacing: [i32; 2],
    pub spacing2: [i32; 2],
    pub timing: Option<Announce>,
    pub voices: BTreeMap<Voice, String>,
}

/// Digit texture of `d`.
// EVIDENCE: cshell+0xa1320 (the ten digit textures are the digit key's value + `%d.dtx`)
fn digit_tex(prefix: &str, d: u32) -> String {
    format!("{prefix}{d}.dtx")
}

impl BannerLayout {
    /// The layout from the HUD layout set; `timing` is the first-kill banner's (the element shares one timing).
    // EVIDENCE: cshell+0xa1320 (slot images and sizes, text place, decorations, digits), cshell+0xa29c0 (timing)
    pub fn from_view(v: &HudView<'_>, timing: Option<&Announce>) -> BannerLayout {
        let rows = [
            (Slot::OneKill, K::BannerOneKillImg, K::BannerOneKillSize),
            (Slot::OneKills, K::BannerOneKillsImg, K::BannerOneKillsSize),
            (Slot::Double, K::BannerDoubleImg, K::BannerDoubleSize),
            (Slot::Multi, K::BannerMultiImg, K::BannerMultiSize),
            (Slot::Ultra, K::BannerUltraImg, K::BannerUltraSize),
            (
                Slot::FiveFold,
                K::BannerFantasticImg,
                K::BannerFantasticSize,
            ),
            (Slot::Unbel, K::BannerUnbelImg, K::BannerUnbelSize),
            (
                Slot::UnbelPlus,
                K::BannerUnbelPlusImg,
                K::BannerUnbelPlusSize,
            ),
            (Slot::HeadHit, K::BannerHeadShotImg, K::BannerHeadShotSize),
            (Slot::Combo, K::BannerComboImg, K::BannerComboSize),
            (Slot::IceCrush, K::BannerIceCrushImg, K::BannerIceCrushSize),
            (Slot::GroinShot, K::BannerNutShotImg, K::BannerNutShotSize),
        ];
        let mut slots = BTreeMap::new();
        for (slot, img, size) in rows {
            if let (Some(t), Some(sz)) = (v.text(img), v.pair(size)) {
                slots.insert(slot, (t.to_string(), sz));
            }
        }
        let deco = |img: K, pos: K, size: K| {
            Some((
                v.text(img)?.to_string(),
                v.pair(pos).unwrap_or_default(),
                v.pair(size)?,
            ))
        };
        BannerLayout {
            slots,
            text_pos: v.pair(K::ComboPos).unwrap_or_default(),
            skull: deco(K::BannerSkullImg, K::BannerSkullPos, K::BannerSkullSize),
            groin: deco(K::BannerNutsImg, K::BannerNutsPos, K::BannerNutsSize),
            digits: v.text(K::BannerDigits).map(str::to_string),
            digit_size: v.pair(K::BannerDigitSize).unwrap_or_default(),
            spacing: v.pair(K::BannerDigitSpacing).unwrap_or_default(),
            spacing2: v.pair(K::BannerDigitSpacing2).unwrap_or_default(),
            timing: timing.cloned(),
            voices: BTreeMap::new(),
        }
    }

    /// Reads the voice files: each pinned key of the user's CShell.dll looked up in the client attribute file's
    /// section (also pinned). Keys or files that do not resolve stay without a file.
    // EVIDENCE: cshell+0x1b2c00 (the lookup: section literal, key, into a 0x80 path buffer); OBSERVED: ROM table
    // ATTRIBUTES.rez CLIENTBUTES.TXT, the section of the pinned literal holds the twelve keys (values read at run
    // time)
    pub fn load_voices(&mut self, rom: &Path) {
        let p = rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]);
        let Ok(pe) = dc_rom::PeFile::from_file(&p) else {
            return;
        };
        let Ok(section) = read_pinned(&pe, &SOUND_SECTION) else {
            return;
        };
        let vfs = dc_rez::RezVfs::new(rom);
        let Some(doc) =
            dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", crate::sound::CLIENT_ENTRY)
        else {
            return;
        };
        let Some(sec) = doc.get(&section) else {
            return;
        };
        for v in Voice::ALL {
            if let Ok(mut key) = read_pinned(&pe, &v.pin()) {
                if v == Voice::RoundDraw {
                    // the sibling of the team-win keys: their first and last five characters around the draw word
                    // DC-DEFAULT: CLI-410 (staged) the key is derived, not read from a literal
                    let n = key.len();
                    key = format!("{}Draw{}", &key[..5], &key[n - 5..]);
                }
                if let Some(f) = sec
                    .get(&key)
                    .map(|f| f.trim().trim_matches('"').to_string())
                {
                    if !f.is_empty() {
                        if v == Voice::QrtCount(0) {
                            // EVIDENCE: cshell+0xb27b0 (the form takes the seconds 0..10)
                            for n in 0..=10u8 {
                                self.voices
                                    .insert(Voice::QrtCount(n), f.replace("%d", &n.to_string()));
                            }
                        } else {
                            self.voices.insert(v, f);
                        }
                    }
                }
            }
        }
    }

    /// The voice file of `v` (`None` without a ROM).
    pub fn voice_file(&self, v: Voice) -> Option<&str> {
        self.voices.get(&v).map(String::as_str)
    }

    /// The banner's draw items at `s.now_ms` (appended to `out`).
    // DISASM: cshell+0xa63c0..0xa6500 and cshell+0xa52a0 (count forms: total width = text width + gap 2 + digit,
    // plus gap + digit for each further digit; the digits start at the text place's x minus half the total, the
    // text image follows the ones digit after gap 2 + digit width); cshell+0xa1320 (other slots: the text image
    // centred on the text place's x, its top at its y; decorations centred on their x, top at their y)
    // DC-DEFAULT: CLI-176 (staged) the element's scale and alpha follow the first-kill banner's timing
    // (`announce_phase`), each part scaled about its own centre; the digits white
    pub fn draw(&self, s: &HudState, out: &mut Vec<DrawItem>) {
        let (Some(b), Some(t)) = (s.banner.show, self.timing.as_ref()) else {
            return;
        };
        let secs = s.now_ms.saturating_sub(b.at) as f32 / 1000.0;
        let Some((scale, alpha)) = announce_phase(t, secs) else {
            return;
        };
        let Some((tex, size)) = self.slots.get(&b.slot) else {
            return;
        };
        let tint = [255, 255, 255, alpha.clamp(0.0, 255.0) as u8];
        let mut put = |tex: &str, x: f32, y: f32, w: f32, h: f32| {
            let (sw, sh) = (w * scale, h * scale);
            out.push(DrawItem::Image {
                tex: tex.to_string(),
                src: [w, h],
                dst: [x + (w - sw) * 0.5, y + (h - sh) * 0.5, sw, sh],
                tint,
                turn: None,
            });
        };
        let [tx, ty] = [self.text_pos[0] as f32, self.text_pos[1] as f32];
        let (w, h) = (size[0] as f32, size[1] as f32);
        match (b.number, &self.digits) {
            (Some(n), Some(prefix)) => {
                let n = n.min(999);
                let (dw, dh) = (self.digit_size[0] as f32, self.digit_size[1] as f32);
                let (g1, g2) = (self.spacing[0] as f32, self.spacing2[0] as f32);
                let mut total = w + g2 + dw;
                if n > 9 {
                    total += g1 + dw;
                }
                if n > 99 {
                    total += g1 + dw;
                }
                let mut x = tx - (total / 2.0).trunc();
                if n > 99 {
                    put(&digit_tex(prefix, n / 100), x, ty, dw, dh);
                    x += g1 + dw;
                }
                if n > 9 {
                    put(&digit_tex(prefix, (n / 10) % 10), x, ty, dw, dh);
                    x += g1 + dw;
                }
                put(&digit_tex(prefix, n % 10), x, ty, dw, dh);
                x += g2 + dw;
                put(tex, x, ty, w, h);
            }
            _ => put(tex, tx - (w / 2.0).trunc(), ty, w, h),
        }
        for (on, deco) in [(b.skull, &self.skull), (b.groin, &self.groin)] {
            if let (true, Some((img, pos, sz))) = (on, deco) {
                let (w, h) = (sz[0] as f32, sz[1] as f32);
                put(img, pos[0] as f32 - (w / 2.0).trunc(), pos[1] as f32, w, h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every announcer and HiSec key resolves in the user's ROM: the pins hold and the key has a value in the
    /// section (a few values name no archive file, which the sound path leaves silent). Skipped without CA_ROM.
    #[test]
    fn announcer_and_hisec_pins_resolve_in_the_rom() {
        let Ok(rom) = std::env::var("CA_ROM") else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let mut l = BannerLayout::default();
        l.load_voices(Path::new(&rom));
        for v in [
            Voice::RoundStart,
            Voice::WinAlpha,
            Voice::WinBravo,
            Voice::CaseDrop,
            Voice::CaseTimer,
            Voice::CaseByMe,
            Voice::CasePicked,
            Voice::FlagByMe,
            Voice::FlagEnemyTaken,
            Voice::FlagOwnTaken,
            Voice::FlagOwnReturned,
            Voice::FlagEnemyReturned,
            Voice::FlagEnemyCaptured,
            Voice::FlagOwnCaptured,
            Voice::BombSet,
            Voice::BombBeep1,
            Voice::BombBeep5,
            Voice::QrtIntro,
            Voice::QrtCount(3),
            Voice::BntIntelDrop,
            Voice::BntIntelGet,
            Voice::BntIntelGetAll,
            Voice::CoopMessage,
            Voice::CoopMedikit,
            Voice::CoopMagazine,
            Voice::CoopBulletbox,
        ] {
            assert!(l.voice_file(v).is_some(), "{}", v.label());
        }
        assert!(l
            .voice_file(Voice::QrtCount(3))
            .is_some_and(|f| f.contains('3') && !f.contains("%d")));
        // the pinned literals themselves
        let p = Path::new(&rom)
            .join(CSHELL_REL_PATH[0])
            .join(CSHELL_REL_PATH[1]);
        let pe = dc_rom::PeFile::from_file(&p).unwrap();
        for v in Voice::ALL {
            read_pinned(&pe, &v.pin()).unwrap_or_else(|_| panic!("pin {}", v.label()));
        }
    }

    use crate::hud::state::HudEvent;
    use dc_botclient::shell::RosterRecord;

    fn rec(pid: u8, team: u8) -> RosterRecord {
        RosterRecord {
            name: format!("p{pid}"),
            pid,
            team,
            complete: true,
            ..RosterRecord::default()
        }
    }

    fn state() -> HudState {
        let mut s = HudState::default();
        for e in [
            HudEvent::Own {
                pid: Some(0),
                mode_class: Some(1),
            },
            HudEvent::Roster(rec(0, 1)),
            HudEvent::Roster(rec(1, 2)),
            HudEvent::Roster(rec(2, 1)),
        ] {
            s.apply(&e, 0, 0.0);
        }
        s
    }

    fn kill(victim: u8, killer: u8, level: u8, zone: u8) -> KillRecord {
        KillRecord {
            victim,
            killer,
            kill_kind: level,
            weapon: 1,
            hit_zone: zone,
            feed_value: 2,
            complete: true,
            ..KillRecord::default()
        }
    }

    #[test]
    fn kill_forms_follow_the_level() {
        let mut s = state();
        let mut seen = Vec::new();
        for (i, (level, zone)) in [(1, 2), (2, 2), (3, 1), (1, 1), (0, 1), (7, 2), (9, 2)]
            .into_iter()
            .enumerate()
        {
            s.apply(
                &HudEvent::Kill(kill(1, 0, level, zone)),
                100 * i as u32,
                0.0,
            );
            seen.push(s.banner.show.map(|b| (b.slot, b.number, b.skull)));
        }
        assert_eq!(
            seen,
            vec![
                Some((Slot::OneKill, Some(1), false)),
                Some((Slot::Double, None, false)),
                Some((Slot::Multi, None, true)),
                Some((Slot::HeadHit, None, true)),
                None,
                Some((Slot::UnbelPlus, None, false)),
                Some((Slot::UnbelPlus, None, false)),
            ]
        );
        assert_eq!(
            s.banner.voices,
            vec![
                Voice::OneKill,
                Voice::Double,
                Voice::Multi,
                Voice::HeadHit,
                Voice::UnbelPlus,
                Voice::UnbelPlus
            ]
        );
        // the count banner shows the own kills (the plural image after the first)
        s.banner.voices.clear();
        s.apply(&HudEvent::Kill(kill(1, 0, 1, 2)), 900, 0.0);
        assert_eq!(
            s.banner.show.map(|b| (b.slot, b.number)),
            Some((Slot::OneKills, Some(8)))
        );
    }

    /// The first own kill (not the match's first, so no opener flag) shows the singular "1 Kill" banner and the
    /// one-kill voice, in a normal mode (roster count) and in Gun Game (the roster's kill column is the rung, 0 here).
    #[test]
    fn the_first_own_kill_is_a_one_kill_banner_not_zero() {
        let mut s = state();
        s.apply(&HudEvent::Kill(kill(1, 0, 1, 2)), 100, 0.0);
        assert_eq!(
            s.banner.show.map(|b| (b.slot, b.number)),
            Some((Slot::OneKill, Some(1)))
        );
        assert_eq!(s.banner.voices, vec![Voice::OneKill]);
        // Gun Game: the rung stays 0 until the server's 0x6b; the banner counts the kills it saw
        let mut g = state();
        g.gun_game = Some(crate::hud::state::GunGameHud::new(vec![
            "A".into(),
            "B".into(),
            "C".into(),
        ]));
        g.apply(&HudEvent::Kill(kill(1, 0, 1, 2)), 100, 0.0);
        assert_eq!(
            g.banner.show.map(|b| (b.slot, b.number)),
            Some((Slot::OneKill, Some(1)))
        );
        assert_eq!(g.banner.voices, vec![Voice::OneKill]);
        assert_eq!(g.roster[&0].kills, 0, "the kill column stays the rung");
        // a knife death in between drops the rung but not the count
        g.apply(&HudEvent::Roster(rec(0, 1)), 150, 0.0);
        g.apply(&HudEvent::Kill(kill(1, 0, 1, 2)), 200, 0.0);
        assert_eq!(
            g.banner.show.map(|b| (b.slot, b.number)),
            Some((Slot::OneKills, Some(2)))
        );
        // another player's kill and a team kill count nothing for the own banner
        g.apply(&HudEvent::Kill(kill(0, 1, 1, 2)), 300, 0.0);
        g.apply(&HudEvent::Kill(kill(2, 0, 0, 2)), 400, 0.0);
        assert_eq!(g.gun_game.as_ref().unwrap().kills, 2);
    }

    #[test]
    fn team_kill_level_0_and_other_killers_start_nothing() {
        let mut s = state();
        // a team kill: level 0 (DSV-106), also on a head hit; the client counts nothing
        s.apply(&HudEvent::Kill(kill(2, 0, 0, 1)), 100, 0.0);
        assert_eq!(s.banner.show, None);
        assert!(s.banner.voices.is_empty());
        assert_eq!(s.roster[&0].kills, 0);
        // another killer's double kill: no banner on this client
        s.apply(&HudEvent::Kill(kill(0, 1, 2, 2)), 200, 0.0);
        assert_eq!(s.banner.show, None);
        // the first-kill flag: its voice, the opener draws the banner
        let mut k = kill(1, 0, 1, 2);
        k.streak_flag = 1;
        s.apply(&HudEvent::Kill(k), 300, 0.0);
        assert_eq!(s.banner.show, None);
        assert_eq!(s.combo_at, Some(300));
        assert_eq!(s.banner.voices, vec![Voice::Opener]);
        // the next own banner replaces the first-kill banner
        s.apply(&HudEvent::Kill(kill(1, 0, 2, 2)), 400, 0.0);
        assert_eq!(s.combo_at, None);
        assert_eq!(s.banner.show.map(|b| b.slot), Some(Slot::Double));
    }

    #[test]
    fn multi_count_message_forms() {
        assert_eq!(pick_run(0, false, 2).slot, Some(Slot::Combo));
        assert_eq!(pick_run(0, false, 2).number, Some(2));
        assert_eq!(
            pick_run(0, false, 2).voice,
            None,
            "the combo form has no voice"
        );
        assert_eq!(pick_run(0, true, 3).voice, Some(Voice::HeadHit));
        assert_eq!(pick_run(5, true, 1).slot, Some(Slot::HeadHit));
        assert_eq!(pick_run(1, false, 0).slot, Some(Slot::OneKill));
        assert_eq!(pick_run(4, false, 1).number, Some(4));
        assert_eq!(pick_run(0, false, 1).slot, None);
    }

    #[test]
    fn count_banner_places_digits_before_the_text() {
        let mut l = BannerLayout {
            text_pos: [512, 200],
            digits: Some("d".into()),
            digit_size: [20, 30],
            spacing: [2, 0],
            spacing2: [6, 0],
            timing: Some(Announce {
                tex: String::new(),
                size: [0, 0],
                centre: [0, 0],
                total: 3.0,
                create: 0.0,
                end: 2.0,
                destroy: 0.5,
                first_scale: 1.0,
            }),
            ..BannerLayout::default()
        };
        l.slots.insert(Slot::OneKills, ("kills".into(), [100, 30]));
        let mut s = state();
        s.banner.show = Some(BannerShow {
            slot: Slot::OneKills,
            number: Some(12),
            skull: false,
            groin: false,
            at: 0,
        });
        s.now_ms = 1000;
        let mut out = Vec::new();
        l.draw(&s, &mut out);
        let xs: Vec<(String, f32)> = out
            .iter()
            .map(|i| match i {
                DrawItem::Image { tex, dst, .. } => (tex.clone(), dst[0]),
                _ => unreachable!(),
            })
            .collect();
        // total 100 + 6 + 20 + 2 + 20 = 148: digits from 512 - 74 = 438
        assert_eq!(
            xs,
            vec![
                ("d1.dtx".into(), 438.0),
                ("d2.dtx".into(), 460.0),
                ("kills".into(), 486.0)
            ]
        );
        // gone after the total time
        s.now_ms = 3100;
        out.clear();
        l.draw(&s, &mut out);
        assert!(out.is_empty());
    }
}

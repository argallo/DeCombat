//! Event sounds the server starts as tracked sounds (DSV-64): a pain sound at a hit player, a head-shot sound at a
//! head-zone hit, and a grenade impact at each bounce of a thrown object, in the record form the 2009 client
//! accepted from the reference stack. The sound files are named by the user's ROM tables at run time (the preload
//! table's pain and head-shot rows, the surface table's grenade impact sound per surface id); which file, when and for
//! how long are DeCombat rules.
// SPEC: UDP_SESSION_2009_B.md §8.3 (tracked sound record; removal through the removed-object record, ids shared
// with the objects), "Pain, fall and bounce sounds" note
// OBSERVED: capture 20261025_002236_tdm4 c6 seq 2968 / 3132 / 8065 (pain), 7690 / 7715 / 7765
// (bounces), 20261025_065016_tdm6 c6 seq 2942 .. 23227 (pain, head shot, bounces); each record removed 0.2 .. 1.5 s
// after its start (seq 3100, 3261, 6861, 7715, 7765, 7981, 8237); REFERENCE, accepted by the real client
// DC-DEFAULT: DSV-64 (proposed)

use crate::rom_tables::ATTRIBUTES_REZ;
use dc_bute::ButeDoc;
use dc_rez::RezVfs;
use std::collections::BTreeMap;

/// The surface table (row `Id`, the grenade impact sound of the row).
pub const SURFACE_ENTRY: &str = "ATTRIBUTES\\SURFACE.TXT";
pub const SURFACE_SECTION: &str = "Surface";
pub const KEY_SURFACE_ID: &str = "Id";
pub const KEY_GRENADE_IMPACT: &str = "GrenadeImpactSnd";
/// The preload table: its sound section names the pain variants and the head-shot sound.
pub const PRELOAD_ENTRY: &str = "ATTRIBUTES\\PRELOADLIST.TXT";
pub const PRELOAD_SOUND_SECTION: &str = "PreLoadSound";
pub const KEY_PAIN_PREFIX: &str = "Hurtpain_M_";
pub const KEY_HEADSHOT: &str = "HEADSHOT";
/// The preload table's second pain family (`Hurtpain_W_<n>`) and the two fall-hurt rows (the plain key and the same
/// key with the second family's suffix).
// OBSERVED: ROM table key PRELOADLIST.TXT [PreLoadSound] Hurtpain_W_1..3, Hurtfalling, Hurtfalling_W (key names only;
// the files are read at run time)
pub const KEY_PAIN_W_PREFIX: &str = "Hurtpain_W_";
pub const KEY_FALL: &str = "Hurtfalling";
pub const KEY_FALL_W: &str = "Hurtfalling_W";
/// The figure rows of the model table and their pain sound directory; a body row names its figure row.
// OBSERVED: ROM table key MODELBUTES.TXT [FigureType<n>] PlayerPainSoundDir, [Model<n>] FigureType
// EVIDENCE: cshell+0x1e4760 (the 2009 client reads `PlayerPainSoundDir` of every figure row into the row at +0xb8,
// 32 bytes, and `FigureType` of every body row into the body row at +0x48)
pub const FIGURE_SECTION: &str = "FigureType";
pub const KEY_PAIN_DIR: &str = "PlayerPainSoundDir";
pub const KEY_BODY_FIGURE: &str = "FigureType";

/// `sflags` of an event sound: the world-sound base bits and positional with direction, no loop, volume 100.
// OBSERVED: tdm4 c6 seq 2968 / 7690 (`sflags` 0x0132 on every pain and bounce record; REFERENCE)
pub const SFLAGS_EVENT: u16 =
    crate::worldobj::SFLAGS_WORLD_BASE | crate::worldobj::SFLAG_POSITIONAL;
/// Priority, outer radius and inner ratio of a pain or head-shot sound.
// OBSERVED: tdm4 c6 seq 2968 / 3132 / 6695 / 8065, tdm6 c6 seq 10134 (prio 2, outer 500, ratio 17; REFERENCE)
pub const PAIN_FORM: SoundForm = SoundForm {
    prio: 2,
    outer: 500,
    ratio: 17,
};
/// Priority, outer radius and inner ratio of a grenade impact.
// OBSERVED: tdm4 c6 seq 7690 / 7715, tdm6 c6 seq 9409 / 9423 / 20098 (prio 1, outer 1000, ratio 17; REFERENCE)
pub const BOUNCE_FORM: SoundForm = SoundForm {
    prio: 1,
    outer: 1000,
    ratio: 17,
};
/// How long a track stays after its file's play time before the server removes it.
// DC-DEFAULT: DSV-64 (proposed) 500 ms (the reference stack removed its pain tracks 0.35 .. 0.5 s after the file's end)
pub const TRACK_TAIL_MS: u32 = 500;
/// Play time assumed when the file's length is unknown (no ROM, unreadable file).
// DC-DEFAULT: DSV-64 (proposed)
pub const UNKNOWN_PLAY_MS: u32 = 1_500;

/// The cached fields of an event sound record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundForm {
    pub prio: u8,
    pub outer: u16,
    pub ratio: u8,
}

/// An event sound the session starts on every client in the world and removes after `life_ms`.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundOut {
    pub file: String,
    pub pos: [f32; 3],
    pub form: SoundForm,
    pub life_ms: u32,
}

/// The ROM rows the event sounds come from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EventSoundTables {
    /// Pain variants of the default figure, in key order.
    pub pain: Vec<String>,
    pub headshot: Option<String>,
    /// Grenade impact sound by surface id.
    pub grenade_impact: BTreeMap<u32, String>,
    /// Every surface id of the surface table.
    pub surface_ids: Vec<u32>,
    /// Pain variants of the second family (`Hurtpain_W_<n>`), in key order.
    pub pain_w: Vec<String>,
    /// The fall-hurt sound of the default family and of the second family.
    pub fall: Option<String>,
    pub fall_w: Option<String>,
    /// Figure row of each body row (`[Model<n>] FigureType`).
    pub body_figure: BTreeMap<u32, u32>,
    /// The figure rows whose pain sound directory is the directory of the second pain family's files.
    pub figures_w: std::collections::BTreeSet<u32>,
    /// The infection voices (DSV-216): the infected's and the host's, of the default and the second family.
    pub infected: [Option<String>; 2],
    pub host: [Option<String>; 2],
}

/// The infection voice files of the character sound archive: infected (default / second family), host (default /
/// second family), found by their pinned keys.
// OBSERVED: ROM archive layout (the character sound archive's infection folders: one infected and one host-selected
// wave per family)
// EVIDENCE: cshell+0x2a1e0 (0xd3 sub 0 team 2 for the own infector or the own infected: the infected character's
// voice row, sound kind 4, played)
// DC-DEFAULT: DSV-216 (proposed)
pub const INFECTED_VOICE: [crate::chars::NameKey; 2] = [
    ([0x9c, 0x05, 0x8e, 0x46, 0xc0, 0xa3, 0xd2, 0x45], 34),
    ([0x8b, 0x69, 0xea, 0xb1, 0xbe, 0xd5, 0x53, 0xd9], 38),
];
pub const HOST_VOICE: [crate::chars::NameKey; 2] = [
    ([0x7c, 0x41, 0x32, 0x38, 0x51, 0x1f, 0xa6, 0xf1], 36),
    ([0x15, 0xbb, 0xb9, 0x88, 0x80, 0xed, 0x47, 0xc9], 40),
];

/// Every entry of the ROM's archives whose upper-case path has one of `keys` (in key order; `None` when absent).
pub fn files_by_key(vfs: &RezVfs, keys: &[crate::chars::NameKey]) -> Vec<Option<String>> {
    let mut out = vec![None; keys.len()];
    let mut rez: Vec<String> = std::fs::read_dir(vfs.rom_dir().join("Game"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.to_ascii_lowercase().ends_with(".rez"))
                .collect()
        })
        .unwrap_or_default();
    rez.sort();
    for name in rez {
        let Ok(list) = vfs.list_entries(&name) else {
            continue;
        };
        for (entry, _, _) in list {
            let up = entry.to_ascii_uppercase();
            let k = crate::chars::name_key(up.as_bytes());
            if let Some(i) = keys.iter().position(|x| *x == k) {
                out[i].get_or_insert(up);
            }
        }
    }
    out
}

/// The voice family of a character's event sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceFamily {
    /// The default family (`Hurtpain_M_<n>`, `Hurtfalling`).
    Default,
    /// The second family (`Hurtpain_W_<n>`, `Hurtfalling_W`).
    Second,
}

/// The last directory name of a ROM path (lower case; empty without a directory).
fn last_dir(path: &str) -> String {
    let p = unquote(path).replace('/', "\\");
    let mut parts: Vec<&str> = p.split('\\').collect();
    parts.pop();
    parts.pop().unwrap_or("").to_ascii_lowercase()
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim()
}

impl EventSoundTables {
    pub fn from_docs(
        preload: Option<&ButeDoc>,
        surface: Option<&ButeDoc>,
        models: Option<&ButeDoc>,
    ) -> Self {
        let mut t = EventSoundTables::default();
        if let Some(doc) = preload {
            let mut pain: Vec<(u32, String)> = Vec::new();
            let mut pain_w: Vec<(u32, String)> = Vec::new();
            for (name, sec) in doc.iter() {
                if !name.eq_ignore_ascii_case(PRELOAD_SOUND_SECTION) {
                    continue;
                }
                for (k, v) in sec.iter() {
                    let v = unquote(v);
                    if v.is_empty() {
                        continue;
                    }
                    let numbered = |prefix: &str| {
                        (k.len() > prefix.len() && k[..prefix.len()].eq_ignore_ascii_case(prefix))
                            .then(|| k[prefix.len()..].parse::<u32>().ok())
                            .flatten()
                    };
                    if k.eq_ignore_ascii_case(KEY_HEADSHOT) {
                        t.headshot = Some(v.to_string());
                    } else if k.eq_ignore_ascii_case(KEY_FALL) {
                        t.fall = Some(v.to_string());
                    } else if k.eq_ignore_ascii_case(KEY_FALL_W) {
                        t.fall_w = Some(v.to_string());
                    } else if let Some(n) = numbered(KEY_PAIN_PREFIX) {
                        pain.push((n, v.to_string()));
                    } else if let Some(n) = numbered(KEY_PAIN_W_PREFIX) {
                        pain_w.push((n, v.to_string()));
                    }
                }
            }
            pain.sort();
            t.pain = pain.into_iter().map(|p| p.1).collect();
            pain_w.sort();
            t.pain_w = pain_w.into_iter().map(|p| p.1).collect();
        }
        if let Some(doc) = surface {
            for (name, sec) in doc.iter() {
                let Some(rest) = name.strip_prefix(SURFACE_SECTION) else {
                    continue;
                };
                if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                let get = |key: &str| {
                    sec.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(key))
                        .map(|(_, v)| unquote(v).to_string())
                };
                let Some(id) = get(KEY_SURFACE_ID).and_then(|v| v.parse::<u32>().ok()) else {
                    continue;
                };
                t.surface_ids.push(id);
                if let Some(snd) = get(KEY_GRENADE_IMPACT).filter(|s| !s.is_empty()) {
                    t.grenade_impact.insert(id, snd);
                }
            }
        }
        if let Some(doc) = models {
            t.read_figures(doc);
        }
        t
    }

    /// The body rows' figure rows, and the figure rows whose pain sound directory is the directory the second pain
    /// family's files lie in (compared as the last directory name, case folded). Nothing is of the second family when
    /// its files share no directory with a figure row.
    // EVIDENCE: cshell+0x1e4760 (`PlayerPainSoundDir` per figure row, `FigureType` per body row)
    // DC-DEFAULT: DSV-151 (proposed) a figure's voice family = the pain family whose files lie in its pain directory
    fn read_figures(&mut self, doc: &ButeDoc) {
        let w_dir = self
            .pain_w
            .first()
            .map(|f| last_dir(f))
            .filter(|d| !d.is_empty());
        for (name, sec) in doc.iter() {
            let get = |key: &str| {
                sec.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| unquote(v).to_string())
            };
            let numbered = |prefix: &str| {
                name.strip_prefix(prefix)
                    .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|r| r.parse::<u32>().ok())
            };
            if let Some(n) = numbered(FIGURE_SECTION) {
                let dir = get(KEY_PAIN_DIR).map(|d| d.to_ascii_lowercase());
                if w_dir.is_some() && dir == w_dir {
                    self.figures_w.insert(n);
                }
            } else if let Some(n) = numbered(crate::chars::BODY_SECTION) {
                if let Some(f) = get(KEY_BODY_FIGURE).and_then(|v| v.parse::<u32>().ok()) {
                    self.body_figure.insert(n, f);
                }
            }
        }
    }

    /// The voice family of a character with the body row `row`.
    // DC-DEFAULT: DSV-151 (proposed)
    pub fn family_of_body(&self, row: u32) -> VoiceFamily {
        match self.body_figure.get(&row) {
            Some(f) if self.figures_w.contains(f) => VoiceFamily::Second,
            _ => VoiceFamily::Default,
        }
    }

    /// The pain variants of a family (the default family's when the second has none).
    // DC-DEFAULT: DSV-151 (proposed)
    pub fn pain_of(&self, family: VoiceFamily) -> &[String] {
        match family {
            VoiceFamily::Second if !self.pain_w.is_empty() => &self.pain_w,
            _ => &self.pain,
        }
    }

    /// The infection voice of a family: the host's for an outbreak infection, else the infected's (the default
    /// family's when the second has none).
    // DC-DEFAULT: DSV-216 (proposed)
    pub fn infection_voice(&self, family: VoiceFamily, host: bool) -> Option<&String> {
        let v = if host { &self.host } else { &self.infected };
        match family {
            VoiceFamily::Second => v[1].as_ref().or(v[0].as_ref()),
            VoiceFamily::Default => v[0].as_ref(),
        }
    }

    /// The fall-hurt sound of a family (the default family's when the second has none).
    // DC-DEFAULT: DSV-152 (proposed)
    pub fn fall_of(&self, family: VoiceFamily) -> Option<&String> {
        match family {
            VoiceFamily::Second => self.fall_w.as_ref().or(self.fall.as_ref()),
            VoiceFamily::Default => self.fall.as_ref(),
        }
    }

    pub fn from_vfs(vfs: &RezVfs) -> Result<Self, String> {
        let pre = ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, PRELOAD_ENTRY)
            .map_err(|e| format!("preload table: {e}"))?;
        let sur = ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, SURFACE_ENTRY)
            .map_err(|e| format!("surface table: {e}"))?;
        if pre.is_none() && sur.is_none() {
            return Err("preload and surface tables missing".into());
        }
        let models = ButeDoc::try_from_vfs(vfs, ATTRIBUTES_REZ, crate::chars::MODELBUTES_ENTRY)
            .ok()
            .flatten();
        let mut t = Self::from_docs(pre.as_ref(), sur.as_ref(), models.as_ref());
        let keys = [
            INFECTED_VOICE[0],
            INFECTED_VOICE[1],
            HOST_VOICE[0],
            HOST_VOICE[1],
        ];
        let f = files_by_key(vfs, &keys);
        t.infected = [f[0].clone(), f[1].clone()];
        t.host = [f[2].clone(), f[3].clone()];
        Ok(t)
    }
}

/// Play time of a PCM wave file in ms (`data` bytes / byte rate of the `fmt ` chunk), `None` when the bytes are not
/// a RIFF wave with both chunks.
// Public format knowledge: RIFF / WAVE (`RIFF` size `WAVE`, chunks `fmt ` with the byte rate at +8 of its body,
// `data`)
pub fn wave_play_ms(b: &[u8]) -> Option<u32> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let (mut rate, mut data) = (None, None);
    let mut i = 12usize;
    while i + 8 <= b.len() {
        let n = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        let body = i + 8;
        match &b[i..i + 4] {
            b"fmt " if body + 12 <= b.len() => {
                rate = Some(u32::from_le_bytes([
                    b[body + 8],
                    b[body + 9],
                    b[body + 10],
                    b[body + 11],
                ]));
            }
            b"data" => data = Some(n as u64),
            _ => {}
        }
        i = body.saturating_add(n).saturating_add(n & 1);
    }
    let rate = u64::from(rate.filter(|r| *r > 0)?);
    Some(u32::try_from(data? * 1000 / rate).unwrap_or(u32::MAX))
}

/// Play time of a ROM sound file (read from the user's archive at run time).
pub fn rom_play_ms(vfs: &RezVfs, path: &str) -> Option<u32> {
    let (rez, entry) = crate::chars::archive_of(path)?;
    let b = vfs.entry(&rez, &entry).ok()??;
    wave_play_ms(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_from_synthetic_docs() {
        // made-up rows in the key shape of the ROM tables
        let pre = ButeDoc::parse(concat!(
            "[PreLoadSound]\n",
            "hurtpain_m_2 = a2.wav\n",
            "Hurtpain_M_1 = a1.wav\n",
            "Hurtpain_W_1 = w.wav\n",
            "HEADSHOT = h.wav\n",
            "Other = o.wav\n",
        ));
        let sur = ButeDoc::parse(
            "[Surface0]\nId = 0\nGrenadeImpactSnd = \"s0.wav\"\n[Surface1]\nId = 110\n[Surface2]\nId = 7\nGrenadeImpactSnd = \"\"\n[Other]\nId = 9\n",
        );
        let t = EventSoundTables::from_docs(Some(&pre), Some(&sur), None);
        assert_eq!(t.pain, vec!["a1.wav".to_string(), "a2.wav".to_string()]);
        assert_eq!(t.pain_w, vec!["w.wav".to_string()]);
        assert_eq!(t.headshot.as_deref(), Some("h.wav"));
        assert_eq!(t.surface_ids, vec![0, 110, 7]);
        assert_eq!(t.grenade_impact.len(), 1);
        assert_eq!(t.grenade_impact[&0], "s0.wav");
    }

    /// DSV-151 / DSV-152: made-up rows in the key shape of the ROM tables: the figure whose pain directory holds the
    /// second family's files gets that family; a body row names its figure; the second fall file may lie elsewhere.
    #[test]
    fn voice_family_from_the_figure_rows() {
        let pre = ButeDoc::parse(concat!(
            "[PreLoadSound]\n",
            "Hurtpain_M_1 = \"snd\\dirA\\p1.wav\"\n",
            "Hurtpain_W_1 = \"snd\\DirB\\q1.wav\"\n",
            "Hurtpain_W_2 = \"snd\\DirB\\q2.wav\"\n",
            "Hurtfalling = \"snd\\dirA\\f.wav\"\n",
            "Hurtfalling_W = \"snd\\dirA\\g.wav\"\n",
        ));
        let models = ButeDoc::parse(concat!(
            "[Model0]\n",
            "FigureType = 0\n",
            "[Model1]\n",
            "FigureType = 1\n",
            "[Model2]\n",
            "Skeleton = 0\n",
            "[FigureType0]\nPlayerPainSoundDir = \"dirA\"\n",
            "[FigureType1]\nPlayerPainSoundDir = \"dirb\"\n",
        ));
        let t = EventSoundTables::from_docs(Some(&pre), None, Some(&models));
        assert_eq!(t.fall.as_deref(), Some("snd\\dirA\\f.wav"));
        assert_eq!(t.fall_w.as_deref(), Some("snd\\dirA\\g.wav"));
        assert_eq!(t.family_of_body(0), VoiceFamily::Default);
        assert_eq!(t.family_of_body(1), VoiceFamily::Second);
        assert_eq!(t.family_of_body(2), VoiceFamily::Default, "no figure row");
        assert_eq!(t.family_of_body(9), VoiceFamily::Default, "no body row");
        assert_eq!(t.pain_of(VoiceFamily::Second).len(), 2);
        assert_eq!(t.pain_of(VoiceFamily::Default).len(), 1);
        assert_eq!(t.fall_of(VoiceFamily::Second), t.fall_w.as_ref());
        assert_eq!(t.fall_of(VoiceFamily::Default), t.fall.as_ref());
        // without a second pain family nothing is "second"
        let t = EventSoundTables::from_docs(None, None, Some(&models));
        assert!(t.figures_w.is_empty());
        assert_eq!(t.family_of_body(1), VoiceFamily::Default);
    }

    /// ROM-gated (DSV-151 / DSV-152): the user's ROM has both pain families and both fall rows, exactly one figure row
    /// of the second family, body rows on both families, and the two fall files differ. Nothing is asserted by value.
    #[test]
    fn rom_voice_families() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let vfs = RezVfs::new(&rom);
        let t = EventSoundTables::from_vfs(&vfs).expect("tables");
        assert!(!t.pain.is_empty() && !t.pain_w.is_empty());
        let (f, fw) = (
            t.fall.as_ref().expect("fall"),
            t.fall_w.as_ref().expect("fall w"),
        );
        assert_ne!(f, fw);
        assert_eq!(t.figures_w.len(), 1, "{:?}", t.figures_w);
        let rows = |fam| {
            t.body_figure
                .keys()
                .filter(|r| t.family_of_body(**r) == fam)
                .count()
        };
        assert!(rows(VoiceFamily::Default) > 0 && rows(VoiceFamily::Second) > 0);
        assert_eq!(t.fall_of(VoiceFamily::Second), Some(fw));
        assert_eq!(t.fall_of(VoiceFamily::Default), Some(f));
    }

    #[test]
    fn wave_length_from_the_chunks() {
        // a made-up 16-bit mono 22050 Hz header (byte rate 44100) with 22050 data bytes = 500 ms
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&22050u32.to_le_bytes());
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&22050u32.to_le_bytes());
        b.extend(std::iter::repeat_n(0u8, 22050));
        assert_eq!(wave_play_ms(&b), Some(500));
        assert_eq!(wave_play_ms(b"RIFF0000WAVX"), None);
        assert_eq!(wave_play_ms(&b[..36]), None, "no data chunk");
    }
}

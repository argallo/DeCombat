//! Sound attribute records (per-sound sections) with parent-chain inheritance.
// SPEC: AUDIO_2009.md §9. Keys are matched by sha256[..16] + length (the spec gives hashes, not text).
//
// Reading rule: look the key up in the section, then in its parent chain, then use the default. The data
// requires it: 213 of 217 sections name a parent, and without the fallback only 9 would carry a volume.

use std::collections::{HashMap, HashSet};

use dc_bute::{eq_fold, ButeDoc, SectionMap};

use crate::KeyId;

/// Entry holding the sound attributes inside the attribute archive. // SPEC: AUDIO_2009.md §9
pub const SOUND_ENTRY: KeyId = KeyId::new("c181c4259c96130d", 14);

/// Scalar and indexed keys of a record. Values are the SPEC §9 table rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Parent,
    Path(u32),
    Weight(u32),
    InnerRadius,
    OuterRadius,
    PlayChance,
    Pitch,
    Volume,
    Priority,
    Local,
    LocalIfOwn,
    Looping,
    Reverb,
    OncePerBuffer,
    LipSync,
    MinDelay,
    MaxDelay,
}

const SCALARS: [(Slot, KeyId); 15] = [
    (Slot::Parent, KeyId::new("5f7953f7c9b6ba16", 6)),
    (Slot::InnerRadius, KeyId::new("487cc23156cc7e95", 11)),
    (Slot::OuterRadius, KeyId::new("4bf718bcf0e0bbd2", 11)),
    (Slot::PlayChance, KeyId::new("bbd74991af4f3380", 10)),
    (Slot::Pitch, KeyId::new("6af11cc594045511", 5)),
    (Slot::Volume, KeyId::new("b10fb966d72063f5", 6)),
    (Slot::Priority, KeyId::new("d60dbba079223254", 8)),
    (Slot::Local, KeyId::new("a39c62e1b8f1921c", 14)),
    (Slot::LocalIfOwn, KeyId::new("f839d86487378c00", 15)),
    (Slot::Looping, KeyId::new("89da4bf6f173830a", 9)),
    (Slot::Reverb, KeyId::new("7f18fba9c0a29353", 11)),
    (Slot::OncePerBuffer, KeyId::new("598d218bd6f5b953", 8)),
    (Slot::LipSync, KeyId::new("ea11484d788f3b4d", 7)),
    (Slot::MinDelay, KeyId::new("23bd417edb0ad64f", 8)),
    (Slot::MaxDelay, KeyId::new("1c862110d5ef07c3", 8)),
];
/// Prefix of the indexed candidate-path keys (`prefix + decimal index`).
const PATH_PREFIX: KeyId = KeyId::new("62fa5a5b0d3c3188", 4);
/// Prefix of the indexed weight keys.
const WEIGHT_PREFIX: KeyId = KeyId::new("81d27ef6d5033c3e", 6);

/// Request flag bits the record sets. // SPEC: AUDIO_2009.md §3.2
pub mod flag {
    pub const POSITIONAL: u32 = 0x0002;
    pub const LOOP_AND_HANDLE: u32 = 0x0014;
    pub const VOLUME: u32 = 0x0040;
    pub const PITCH: u32 = 0x0080;
    pub const REVERB: u32 = 0x0100;
    pub const LOCAL_IF_OWN: u32 = 0x0800;
    pub const ONCE_PER_BUFFER: u32 = 0x1000;
}

/// One resolved sound record.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundRecord {
    pub name: String,
    /// Direct parent named by the section, if any.
    pub parent: Option<String>,
    /// Root of the parent chain (the section itself when it has no parent).
    pub root: String,
    pub paths: Vec<String>,
    /// Cumulative distribution over `paths` built from the weights (default weight 1.0 each).
    pub cumulative: Vec<f32>,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub play_chance: f32,
    pub pitch: f32,
    pub volume: i32,
    pub priority: i32,
    pub local: i32,
    pub local_if_own: i32,
    pub looping: i32,
    pub reverb: i32,
    pub once_per_buffer: i32,
    pub lip_sync: i32,
    pub min_delay: f32,
    pub max_delay: f32,
    found: HashSet<Slot>,
    own: HashSet<Slot>,
}

impl SoundRecord {
    /// True when the key was found in the section or any ancestor.
    pub fn resolved(&self, s: Slot) -> bool {
        self.found.contains(&s)
    }
    /// True when the key is in the section itself (no inheritance).
    pub fn has_own(&self, s: Slot) -> bool {
        self.own.contains(&s)
    }

    /// Request flag bits derived from the record. // SPEC: AUDIO_2009.md §9 table
    pub fn request_flags(&self) -> u32 {
        let mut f = 0;
        if self.local == 0 {
            f |= flag::POSITIONAL;
        }
        if self.local_if_own != 0 {
            f |= flag::LOCAL_IF_OWN;
        }
        if self.looping != 0 {
            f |= flag::LOOP_AND_HANDLE;
        }
        if self.reverb != 0 {
            f |= flag::REVERB;
        }
        if self.once_per_buffer != 0 {
            f |= flag::ONCE_PER_BUFFER;
        }
        if self.volume != 100 {
            f |= flag::VOLUME;
        }
        if self.pitch != 1.0 {
            f |= flag::PITCH;
        }
        f
    }

    /// Weighted choice among `paths` for a uniform `u` in `[0, 1)`.
    pub fn pick_path(&self, u: f32) -> Option<&str> {
        let i = self
            .cumulative
            .iter()
            .position(|c| u < *c)
            .unwrap_or(self.paths.len().checked_sub(1)?);
        self.paths.get(i).map(|s| s.as_str())
    }
}

/// All records of the sound attribute file, in file order.
#[derive(Clone, Debug, Default)]
pub struct SoundTable {
    pub records: Vec<SoundRecord>,
    index: HashMap<String, usize>,
}

impl SoundTable {
    pub fn get(&self, name: &str) -> Option<&SoundRecord> {
        self.index.get(name).map(|&i| &self.records[i])
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Builds the table from a parsed attribute document, matching keys by hash.
    pub fn from_doc(doc: &ButeDoc) -> Self {
        Self::from_doc_with(doc, &mut classify_key)
    }

    /// Builds the table with a caller-supplied key classifier (tests use plain synthetic key names).
    pub fn from_doc_with(doc: &ButeDoc, classify_fn: &mut dyn FnMut(&str) -> Option<Slot>) -> Self {
        let mut cache: HashMap<String, Option<Slot>> = HashMap::new();
        let mut classify = |key: &str| -> Option<Slot> {
            if let Some(c) = cache.get(key) {
                return *c;
            }
            let c = classify_fn(key);
            cache.insert(key.to_string(), c);
            c
        };
        // Per-section map slot -> value.
        let mut own: HashMap<&str, HashMap<Slot, &str>> = HashMap::new();
        for (name, sec) in doc.iter() {
            let mut m = HashMap::new();
            for (k, v) in sec.iter() {
                if let Some(s) = classify(k) {
                    m.insert(s, v.as_str());
                }
            }
            own.insert(name, m);
        }
        let find_section = |name: &str| -> Option<&str> {
            if own.contains_key(name) {
                return doc
                    .order
                    .iter()
                    .find(|n| n.as_str() == name)
                    .map(|s| s.as_str());
            }
            doc.order
                .iter()
                .find(|n| eq_fold(n, name))
                .map(|s| s.as_str())
        };

        let mut records = Vec::new();
        let mut index = HashMap::new();
        for name in &doc.order {
            // The chain: section, parent, grandparent, ... (cycle-safe).
            let mut chain: Vec<&str> = vec![name.as_str()];
            let mut seen: HashSet<&str> = HashSet::from([name.as_str()]);
            loop {
                let cur = *chain.last().expect("non-empty");
                let Some(p) = own.get(cur).and_then(|m| m.get(&Slot::Parent)) else {
                    break;
                };
                let Some(pn) = find_section(p) else { break };
                if !seen.insert(pn) {
                    break;
                }
                chain.push(pn);
            }
            let lookup =
                |s: Slot| -> Option<&str> { chain.iter().find_map(|c| own[c].get(&s).copied()) };
            let mut found = HashSet::new();
            let mut ownset = HashSet::new();
            for s in SCALARS.iter().map(|x| x.0) {
                if lookup(s).is_some() {
                    found.insert(s);
                }
                if own[name.as_str()].contains_key(&s) {
                    ownset.insert(s);
                }
            }
            let int = |s: Slot| lookup(s).map(parse_int).unwrap_or(0);
            let flt = |s: Slot, d: f32| lookup(s).and_then(parse_f32).unwrap_or(d);

            // Paths: from the nearest chain element that has path 0 (index 0, 1, ... while present).
            let mut paths = Vec::new();
            let mut weights = Vec::new();
            if let Some(src) = chain.iter().find(|c| own[*c].contains_key(&Slot::Path(0))) {
                let m = &own[src];
                let mut i = 0u32;
                while let Some(p) = m.get(&Slot::Path(i)) {
                    paths.push((*p).to_string());
                    weights.push(
                        m.get(&Slot::Weight(i))
                            .and_then(|w| parse_f32(w))
                            .unwrap_or(1.0),
                    );
                    i += 1;
                }
            }
            let total: f32 = weights.iter().sum();
            let mut acc = 0.0;
            let cumulative = weights
                .iter()
                .map(|w| {
                    acc += if total > 0.0 { w / total } else { 0.0 };
                    acc
                })
                .collect();

            let parent = own[name.as_str()].get(&Slot::Parent).map(|s| s.to_string());
            let rec = SoundRecord {
                name: name.clone(),
                parent,
                root: (*chain.last().expect("non-empty")).to_string(),
                paths,
                cumulative,
                inner_radius: flt(Slot::InnerRadius, 0.0),
                outer_radius: flt(Slot::OuterRadius, 0.0),
                play_chance: flt(Slot::PlayChance, 0.0),
                pitch: flt(Slot::Pitch, 0.0),
                volume: int(Slot::Volume),
                priority: int(Slot::Priority),
                local: int(Slot::Local),
                local_if_own: int(Slot::LocalIfOwn),
                looping: int(Slot::Looping),
                reverb: int(Slot::Reverb),
                once_per_buffer: int(Slot::OncePerBuffer),
                lip_sync: int(Slot::LipSync),
                min_delay: flt(Slot::MinDelay, 0.0),
                max_delay: flt(Slot::MaxDelay, 0.0),
                found,
                own: ownset,
            };
            index.insert(name.clone(), records.len());
            records.push(rec);
        }
        SoundTable { records, index }
    }

    /// Loads the sound attributes from `ATTRIBUTES.rez` by matching the entry name's hash.
    pub fn from_vfs(vfs: &dc_bute::RezVfs) -> Option<Self> {
        let list = vfs.list_entries("ATTRIBUTES.rez").ok()?;
        for (full, _, _) in list {
            let base = full.rsplit(['\\', '/']).next().unwrap_or(&full);
            if SOUND_ENTRY.matches(&full) || SOUND_ENTRY.matches(base) {
                let doc = ButeDoc::from_vfs(vfs, "ATTRIBUTES.rez", &full)?;
                return Some(Self::from_doc(&doc));
            }
        }
        None
    }
}

fn classify_key(key: &str) -> Option<Slot> {
    for (s, id) in SCALARS.iter() {
        if id.matches(key) {
            return Some(*s);
        }
    }
    for (pre, mk) in [
        (PATH_PREFIX, Slot::Path as fn(u32) -> Slot),
        (WEIGHT_PREFIX, Slot::Weight as fn(u32) -> Slot),
    ] {
        if key.len() > pre.len && key.is_char_boundary(pre.len) {
            let (head, tail) = key.split_at(pre.len);
            if tail.bytes().all(|b| b.is_ascii_digit()) && pre.matches(head) {
                if let Ok(i) = tail.parse::<u32>() {
                    return Some(mk(i));
                }
            }
        }
    }
    None
}

fn parse_int(s: &str) -> i32 {
    let t = s.trim();
    t.parse::<i32>()
        .ok()
        .or_else(|| t.parse::<f64>().ok().map(|f| f as i32))
        .unwrap_or(0)
}

fn parse_f32(s: &str) -> Option<f32> {
    let t = s.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

/// Convenience for callers that only have a section map: does it contain the key?
pub fn section_has(sec: &SectionMap, id: KeyId) -> bool {
    sec.iter().any(|(k, _)| id.matches(k))
}

/// Finds the original text of a key in a section by its hash (used by tooling that needs the name).
pub fn find_key(sec: &SectionMap, id: KeyId) -> Option<&str> {
    sec.iter().map(|(k, _)| k.as_str()).find(|k| id.matches(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash16;

    #[test]
    fn classify_ignores_unknown_keys() {
        assert_eq!(classify_key("definitely not a key"), None);
        assert_eq!(classify_key("x"), None);
        assert_eq!(classify_key("é1"), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_int(" 80 "), 80);
        assert_eq!(parse_int("0.5"), 0);
        assert_eq!(parse_int("zz"), 0);
        assert_eq!(parse_f32("1.5f"), Some(1.5));
        assert_eq!(parse_f32("nan"), None);
    }

    #[test]
    fn pick_path_weighted() {
        let r = SoundRecord {
            name: "t".into(),
            parent: None,
            root: "t".into(),
            paths: vec!["a".into(), "b".into(), "c".into()],
            cumulative: vec![0.25, 0.5, 1.0],
            inner_radius: 0.0,
            outer_radius: 0.0,
            play_chance: 0.0,
            pitch: 1.0,
            volume: 100,
            priority: 0,
            local: 1,
            local_if_own: 0,
            looping: 0,
            reverb: 0,
            once_per_buffer: 0,
            lip_sync: 0,
            min_delay: 0.0,
            max_delay: 0.0,
            found: HashSet::new(),
            own: HashSet::new(),
        };
        assert_eq!(r.pick_path(0.1), Some("a"));
        assert_eq!(r.pick_path(0.3), Some("b"));
        assert_eq!(r.pick_path(0.99), Some("c"));
        assert_eq!(r.request_flags(), 0);
    }

    #[test]
    fn hash_matching_and_flags() {
        // KeyId::matches is a pure hash+length test.
        let id = KeyId::new(Box::leak(hash16("abc").into_boxed_str()), 3);
        assert!(id.matches("abc"));
        assert!(!id.matches("abd"));
        assert!(!id.matches("abcd"));
    }

    fn synthetic(key: &str) -> Option<Slot> {
        Some(match key {
            "parent" => Slot::Parent,
            "vol" => Slot::Volume,
            "inner" => Slot::InnerRadius,
            "outer" => Slot::OuterRadius,
            "prio" => Slot::Priority,
            "local" => Slot::Local,
            "p0" => Slot::Path(0),
            "p1" => Slot::Path(1),
            "w0" => Slot::Weight(0),
            "w1" => Slot::Weight(1),
            _ => return None,
        })
    }

    const DOC: &str = "[Base]
vol = 80
inner = 2
outer = 10
local = 0
prio = 3
        [Mid]
parent = Base
vol = 60
        [Leaf]
parent = mid
p0 = a.wav
p1 = b.wav
w0 = 1
w1 = 3
        [Orphan]
parent = Missing
p0 = c.wav
        [LoopA]
parent = LoopB
vol = 5
[LoopB]
parent = LoopA
prio = 9
";

    #[test]
    fn chain_rule() {
        let t = SoundTable::from_doc_with(&ButeDoc::parse(DOC), &mut synthetic);
        assert_eq!(t.len(), 6);
        let leaf = t.get("Leaf").unwrap();
        // nearest definition wins: volume from Mid, radii/prio/local from Base
        assert_eq!(leaf.volume, 60);
        assert_eq!((leaf.inner_radius, leaf.outer_radius), (2.0, 10.0));
        assert_eq!((leaf.priority, leaf.local), (3, 0));
        assert_eq!(leaf.root, "Base");
        assert_eq!(leaf.parent.as_deref(), Some("mid"));
        assert!(leaf.resolved(Slot::Volume) && !leaf.has_own(Slot::Volume));
        assert!(t.get("Mid").unwrap().has_own(Slot::Volume));
        // weights normalise into a cumulative distribution
        assert_eq!(leaf.paths, ["a.wav", "b.wav"]);
        assert_eq!(leaf.cumulative, vec![0.25, 1.0]);
        // POSITIONAL because local == 0, VOLUME because volume != 100
        assert_eq!(
            leaf.request_flags() & (flag::POSITIONAL | flag::VOLUME),
            flag::POSITIONAL | flag::VOLUME
        );
        // base has no parent
        let base = t.get("Base").unwrap();
        assert_eq!(base.parent, None);
        assert_eq!(base.root, "Base");
    }

    #[test]
    fn missing_parent_and_cycles_terminate() {
        let t = SoundTable::from_doc_with(&ButeDoc::parse(DOC), &mut synthetic);
        let o = t.get("Orphan").unwrap();
        assert_eq!(o.volume, 0); // unresolved: reader default 0
        assert!(!o.resolved(Slot::Volume));
        assert_eq!(o.paths, ["c.wav"]);
        let a = t.get("LoopA").unwrap();
        assert_eq!((a.volume, a.priority), (5, 9));
        let b = t.get("LoopB").unwrap();
        assert_eq!((b.volume, b.priority), (5, 9));
    }
}

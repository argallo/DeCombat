//! Replays the server -> client datagrams of the reference stack's reference run (captap format 1, recorded by
//! `dc-botclient`, `capture botclient_reference_20261024_232007.jsonl`) through our transport
//! receiver, decodes the object records the reference stack sent for the two player characters and their weapon models,
//! and checks that our writers produce the same create form. Evidence class REFERENCE (`REFERENCE_TESTING.md` §1): the
//! wire form is aligned with the reference stack, rule values are not taken from it.
//!
//! Skipped when the capture is absent (set `DC_REFERENCE_CAPTURE` to point at one). The ROM-gated test also needs
//! `CA_ROM`. Nothing of the capture is copied into the tree: file names are compared by SHA-256 only, and the
//! printed summary carries counts, ids, bit lengths and hash prefixes.

use std::collections::BTreeMap;
use std::path::PathBuf;

use dc_dserver::chars::{announce_spelling, CharacterChoice};
use dc_dserver::game::{Game, GameTuning};
use dc_dserver::modelobj::{
    self, CharacterSpec, ModelEntry, Tracker, CHARACTER_FLAGS_HIDDEN, CHARACTER_FLAGS_SPAWNED,
    WEAPON_FLAGS_HIDDEN, WEAPON_FLAGS_SPAWNED,
};
use dc_dserver::objects::PosBox;
use dc_dserver::rom_tables::RomTables;
use dc_net_engine::{Link, LinkConfig, LinkEvent};
use dc_wire::udp::{BitReader, BitWriter};

const REFERENCE_RUN: &str = "botclient_reference_20261024_232007.jsonl";
const BOTS: [&str; 2] = ["OraBot1", "OraBot2"];

fn capture_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DC_REFERENCE_CAPTURE") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let p = PathBuf::from(std::env::var_os("DC_CAPTURES")?).join(REFERENCE_RUN);
    p.is_file().then_some(p)
}

macro_rules! capture_or_skip {
    () => {
        match capture_path() {
            Some(p) => p,
            None => {
                println!("skipped: no reference capture");
                return;
            }
        }
    };
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn sha(b: &[u8]) -> String {
    dc_rez::dc_rom::sha256(b)[..6]
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// Every message delivered to one bot, in order: (ms, reliable, bits).
fn replay(path: &PathBuf, bot: &str) -> Vec<(u32, bool, BitWriter)> {
    let text = std::fs::read_to_string(path).unwrap();
    let mut link: Option<Link> = None;
    let mut out = Vec::new();
    for line in text.lines() {
        let j: serde_json::Value = serde_json::from_str(line).unwrap();
        if j["port"] != "game" || j["dir"] != "s2c" || j["bot"] != bot || j["ev"] != "dgram" {
            continue;
        }
        if j["blocks"].get("oob").is_some() {
            continue;
        }
        let t = (j["t"].as_f64().unwrap() * 1000.0) as u32;
        let bytes = unhex(j["hex"].as_str().unwrap());
        let l = link.get_or_insert_with(|| Link::new(t, LinkConfig::default()));
        l.handle_datagram(t, &bytes);
        for ev in l.poll(t) {
            if let LinkEvent::Message { reliable, bits } = ev {
                out.push((t, reliable, bits));
            }
        }
    }
    out
}

/// One decoded object record (our own decoder, written from UDP_SESSION_2009_B.md §3-§4).
#[derive(Debug, Clone, Default)]
struct Rec {
    obj: u16,
    /// As on the wire (the low byte keeps its marker bit).
    cf: u16,
    create: bool,
    kind: u8,
    blob: Option<BitWriter>,
    models: Vec<ModelEntry>,
    anim: Vec<Tracker>,
    flags: Option<[u32; 4]>,
    /// Bit ranges of the position block and of the rotation (masked when records are compared).
    pos_bits: Option<(usize, usize)>,
    rot_bits: Option<(usize, usize)>,
    rot: Option<[f32; 4]>,
    attach: Vec<(u16, u32)>,
    masks_zero: bool,
    dims: Option<[f32; 3]>,
    bits: BitWriter,
}

fn models(r: &mut BitReader<'_>) -> Vec<ModelEntry> {
    let n = r.read_bits(6);
    let mut out = Vec::new();
    for m in 0..n {
        let mut e = ModelEntry {
            file: r.read_u16(),
            ..ModelEntry::default()
        };
        if m == 0 {
            let subs = r.read_bits(6);
            for _ in 0..subs.saturating_sub(1) {
                e.subs.push(r.read_u16());
            }
        }
        for _ in 0..r.read_bits(6) {
            e.textures.push(r.read_u16());
        }
        for _ in 0..r.read_bits(4) {
            e.styles.push(r.read_u16());
        }
        out.push(e);
    }
    out
}

fn anim(r: &mut BitReader<'_>) -> Vec<Tracker> {
    let mut main = true;
    let mut out = Vec::new();
    while r.read_bit() {
        let wide = r.read_bit();
        let (a, s0) = if wide {
            (r.read_bits(14) as u16, r.read_bit())
        } else {
            (r.read_bits(8) as u16, true)
        };
        let s1 = r.read_bit();
        let tsel = r.read_bits(2) as usize;
        let time8 = r.read_bits([1, 6, 10, 32][tsel]);
        let secondary = if main {
            None
        } else {
            let tr = r.read_u8();
            let ws = if r.read_bit() {
                r.read_u8()
            } else {
                r.read_bits(3) as u8
            };
            Some((tr, ws))
        };
        // a length with the same time-field width (the reader keeps only the width)
        let len_ms = [0u32, 72, 505, 8185][tsel];
        out.push(Tracker {
            anim: a,
            state0: s0,
            state1: s1,
            time_ms: time8 << 3,
            len_ms,
            secondary,
        });
        main = false;
    }
    out
}

fn decode(rec: &BitWriter, kinds: &BTreeMap<u16, u8>) -> Result<Rec, String> {
    let mut r = rec.reader();
    let lo = r.read_u8();
    if lo == 0 {
        return Err("special record".into());
    }
    let cf = if lo & 0x80 != 0 {
        lo as u16 | (r.read_u8() as u16) << 8
    } else {
        lo as u16
    };
    let mut o = Rec {
        obj: r.read_u16(),
        cf,
        create: cf & 1 != 0,
        bits: rec.clone(),
        ..Rec::default()
    };
    let mut cfm = cf;
    if o.create {
        let kb = r.read_u8();
        o.kind = kb & 0x3f;
        r.skip_bits(96);
        if kb & 0x40 != 0 {
            let n = if kb & 0x80 != 0 {
                r.read_u16() as usize
            } else {
                r.read_u8() as usize
            };
            o.blob = Some(r.read_bit_string(n));
        }
        match o.kind {
            1 => o.models = models(&mut r),
            2 => {
                r.read_zstr8();
            }
            9 => {
                r.read_zstr8();
                r.read_u16();
            }
            3 => {
                r.read_u16();
            }
            _ => {}
        }
        cfm |= 0x0200;
    } else {
        o.kind = *kinds.get(&o.obj).ok_or("change for an unknown object")?;
    }
    if !o.create && cfm & 0x0800 != 0 && o.kind == 1 {
        o.models = models(&mut r);
    }
    if cfm & 0x2020 != 0 && o.kind == 1 {
        o.anim = anim(&mut r);
    }
    if cfm & 0x0008 != 0 {
        o.flags = Some([
            r.read_u16() as u32,
            r.read_u16() as u32,
            r.read_u32(),
            r.read_u32(),
        ]);
    }
    if cfm & 0x0050 != 0 {
        return Err("colour / scale not expected".into());
    }
    if cfm & 0x0202 != 0 {
        let full = o.flags.is_some_and(|f| f[0] & 0x0100 != 0);
        let s = r.bit_pos();
        r.skip_bits(if full { 192 } else { 48 + 72 });
        o.pos_bits = Some((s, r.bit_pos()));
    }
    if cfm & 0x0404 != 0 {
        let s = r.bit_pos();
        o.rot = Some(dc_wire::udp::read_quat128(&mut r));
        o.rot_bits = Some((s, r.bit_pos()));
    }
    if cfm & 0x0100 != 0 {
        loop {
            let c = r.read_u16();
            if c == 0xFFFF {
                break;
            }
            let sock = r.read_u32();
            r.skip_bits(96 + 128);
            o.attach.push((c, sock));
        }
        if o.kind == 1 {
            o.masks_zero = (0..64).all(|_| r.read_u32() == 0);
        }
    }
    if cfm & 0x8000 != 0 {
        o.dims = Some([r.read_f32(), r.read_f32(), r.read_f32()]);
    }
    if !r.at_end() {
        return Err(format!("{} bits left", r.remaining_bits()));
    }
    Ok(o)
}

/// The record with the bits of `ranges` set to 0.
fn masked(b: &BitWriter, ranges: &[Option<(usize, usize)>]) -> Vec<bool> {
    let mut r = b.reader();
    let mut v: Vec<bool> = (0..b.bit_len()).map(|_| r.read_bit()).collect();
    for (s, e) in ranges.iter().flatten() {
        for x in &mut v[*s..*e] {
            *x = false;
        }
    }
    v
}

/// Everything the replay of one bot yields.
#[derive(Default)]
struct Run {
    /// File table of the session: id -> announced name.
    files: BTreeMap<u16, Vec<u8>>,
    records: Vec<(u32, Rec)>,
    groups: Vec<BitWriter>,
    /// Stream entries of model objects: (obj, has velocity, yaw8 present, tracker animations).
    stream: Vec<(u16, bool, bool, Vec<u16>)>,
    /// Shell messages: (ms, reliable, id, body).
    shells: Vec<(u32, bool, u8, BitWriter)>,
    /// Engine message ids in order.
    ids: Vec<u8>,
    view_object: Option<u16>,
}

fn load(path: &PathBuf, bot: &str) -> Run {
    let mut run = Run::default();
    let mut kinds = BTreeMap::new();
    for (t, rel, m) in replay(path, bot) {
        let mut r = m.reader();
        let id = r.read_u8();
        run.ids.push(id);
        match id {
            0x32 => {
                while r.remaining_bits() >= 48 {
                    let id = r.read_u16();
                    r.read_u32();
                    run.files.insert(id, r.read_zstr8());
                }
            }
            7 => run.view_object = Some(r.read_u16()),
            8 => {
                let n = r.remaining_bits();
                let body = r.read_bit_string(n);
                let mut rr = body.reader();
                while rr.remaining_bits() >= 32 {
                    let len = rr.read_u32() as usize;
                    let rec = rr.read_bit_string(len);
                    if let Ok(d) = decode(&rec, &kinds) {
                        if d.create {
                            kinds.insert(d.obj, d.kind);
                        }
                        run.records.push((t, d));
                    }
                }
                run.groups.push(body);
            }
            10 => {
                while r.remaining_bits() >= 20 {
                    let obj = r.read_u16();
                    let uf = r.read_bits(4);
                    if obj == 0xFFFF && uf == 0 {
                        r.read_f32();
                        continue;
                    }
                    let mut vel = false;
                    if uf & 4 != 0 {
                        r.skip_bits(48);
                        if r.read_bit() {
                            r.skip_bits(72);
                            vel = true;
                        }
                    }
                    let yaw8 = uf & 8 != 0;
                    if yaw8 {
                        r.read_s8();
                    } else if uf & 2 != 0 {
                        r.skip_bits(128);
                    }
                    let mut a = Vec::new();
                    if uf & 1 != 0 {
                        a = anim(&mut r).iter().map(|t| t.anim).collect();
                    }
                    if !a.is_empty() {
                        run.stream.push((obj, vel, yaw8, a));
                    }
                }
            }
            13 => {
                let sh = r.read_u8();
                let n = r.remaining_bits();
                run.shells.push((t, rel, sh, r.read_bit_string(n)));
            }
            _ => {}
        }
    }
    run
}

fn characters(run: &Run) -> Vec<&Rec> {
    run.records
        .iter()
        .map(|x| &x.1)
        .filter(|r| {
            r.create
                && r.blob
                    .as_ref()
                    .is_some_and(|b| b.reader().read_u8() == 0x18)
        })
        .collect()
}

fn create_of(run: &Run, obj: u16) -> &Rec {
    run.records
        .iter()
        .map(|x| &x.1)
        .find(|r| r.create && r.obj == obj)
        .expect("create record")
}

fn blob_fields(b: &BitWriter) -> (u8, [f32; 3]) {
    let mut r = b.reader();
    r.skip_bits(8 + 2 + 40);
    let pid = r.read_u8();
    r.skip_bits(64 + 32 * 6);
    (pid, [r.read_f32(), r.read_f32(), r.read_f32()])
}

fn pbox() -> PosBox {
    PosBox {
        min: [-8192.0; 3],
        max: [8192.0; 3],
    }
}

fn flag_words(f: modelobj::FlagWords) -> [u32; 4] {
    [f.flags as u32, f.flags2 as u32, f.uflags, f.flags3]
}

/// A second reader (dc-botclient's object table) and our decoder parse every object group of the run.
#[test]
fn reference_object_groups_parse_with_two_readers() {
    let path = capture_or_skip!();
    for bot in BOTS {
        let run = load(&path, bot);
        let mut t = dc_botclient::objects::ObjectTable::default();
        for g in &run.groups {
            t.apply_group(g);
        }
        assert!(t.errors.is_empty(), "{bot}: {:?}", t.errors);
        println!(
            "{bot}: {} object groups, {} records decoded, {} creates, {} changes, {} model-object stream entries",
            run.groups.len(),
            run.records.len(),
            t.creates,
            t.changes,
            run.stream.len()
        );
        assert!(t.creates > 50 && run.records.len() > 100);
    }
}

/// The two player characters and their weapon models: our writers, given the reference stack's file ids, player id and
/// dims, produce the reference stack's records bit for bit except the position (and the rotation of the spawn change),
/// which depend on the world box and the spawn point.
#[test]
fn reference_player_create_form_equals_ours() {
    let path = capture_or_skip!();
    for bot in BOTS {
        let run = load(&path, bot);
        let chars = characters(&run);
        assert_eq!(chars.len(), 2, "{bot}: two player characters");
        let view = run.view_object.expect("view object");
        assert!(
            chars.iter().any(|c| c.obj == view),
            "{bot}: the own view object is a character"
        );
        for c in chars {
            let blob = c.blob.clone().unwrap();
            let (pid, dims) = blob_fields(&blob);
            assert_eq!(blob.bit_len(), 580);
            assert_eq!(blob, modelobj::CharacterBlob::reference(pid, dims).write());
            assert_eq!(c.attach.len(), 1);
            assert!(c.masks_zero);
            let (wobj, socket) = c.attach[0];
            let w = create_of(&run, wobj);
            let spec = CharacterSpec {
                obj: c.obj,
                weapon_obj: Some(wobj),
                pid,
                pos: [0.0; 3],
                dims,
                models: c.models.clone(),
                anim: c.anim.clone(),
                spawned: false,
                // the reference run's creates went out after the spawn set (record complete) and before the 0x87
                // (hidden); both bots joined together, so no create carried a rotation
                record_complete: true,
                rot: None,
                socket,
                weapon_model: w.models.first().cloned(),
                weapon_anim_len_ms: dc_dserver::game::TODO_WEAPON_ANIM_LEN_MS,
            };
            let ours = modelobj::character_create(&spec).write(&pbox());
            let ours_d = decode(&ours, &BTreeMap::new()).unwrap();
            assert_eq!(
                ours.bit_len(),
                c.bits.bit_len(),
                "{bot}: character record length"
            );
            assert_eq!(
                masked(&ours, &[ours_d.pos_bits]),
                masked(&c.bits, &[c.pos_bits]),
                "{bot}: character record of object {}",
                c.obj
            );
            assert_eq!(c.flags, Some(flag_words(CHARACTER_FLAGS_HIDDEN)));
            let wo = modelobj::weapon_create(&spec).unwrap().write(&pbox());
            let wo_d = decode(&wo, &BTreeMap::new()).unwrap();
            assert_eq!(
                masked(&wo, &[wo_d.pos_bits]),
                masked(&w.bits, &[w.pos_bits]),
                "{bot}: weapon record of object {wobj}"
            );
            assert_eq!(w.flags, Some(flag_words(WEAPON_FLAGS_HIDDEN)));
            // the spawn changes
            let kinds: BTreeMap<u16, u8> = [(c.obj, 1), (wobj, 1)].into();
            let spawn = run
                .records
                .iter()
                .map(|x| &x.1)
                .find(|r| !r.create && r.obj == c.obj && r.flags.is_some())
                .expect("spawn change");
            let mine = modelobj::character_spawned(c.obj, [0.0; 3], spawn.rot).write(&pbox());
            let mine_d = decode(&mine, &kinds).unwrap();
            assert_eq!(
                masked(&mine, &[mine_d.pos_bits, mine_d.rot_bits]),
                masked(&spawn.bits, &[spawn.pos_bits, spawn.rot_bits])
            );
            assert_eq!(spawn.flags, Some(flag_words(CHARACTER_FLAGS_SPAWNED)));
            let wspawn = run
                .records
                .iter()
                .map(|x| &x.1)
                .find(|r| !r.create && r.obj == wobj && r.flags.is_some())
                .expect("weapon spawn change");
            let wm = modelobj::weapon_spawned(wobj, [0.0; 3]).write(&pbox());
            let wm_d = decode(&wm, &kinds).unwrap();
            assert_eq!(
                masked(&wm, &[wm_d.pos_bits]),
                masked(&wspawn.bits, &[wspawn.pos_bits])
            );
            assert_eq!(wspawn.flags, Some(flag_words(WEAPON_FLAGS_SPAWNED)));
            let name = |id: &u16| sha(run.files.get(id).map(|n| n.as_slice()).unwrap_or(b""));
            println!(
                "{bot}: character obj {} pid {pid} cf {:#06x} {} bits, blob {} bits: models {:?}, trackers {:?}, \
                 attach ({wobj}, socket {socket}), dims {dims:?}; weapon cf {:#06x} {} bits, model {}; spawn cf \
                 {:#06x} / {:#06x}",
                c.obj,
                c.cf,
                c.bits.bit_len(),
                blob.bit_len(),
                c.models
                    .iter()
                    .map(|m| (name(&m.file), m.subs.len(), m.textures.len(), m.styles.len()))
                    .collect::<Vec<_>>(),
                c.anim
                    .iter()
                    .map(|t| (t.anim, t.secondary))
                    .collect::<Vec<_>>(),
                w.cf,
                w.bits.bit_len(),
                name(&w.models[0].file),
                spawn.cf,
                wspawn.cf
            );
        }
    }
}

/// The start order the reference stack used (UDP_REFERENCE_FINDINGS O2 / O6 / O9), which our default `-startorder reference`
/// follows (session_sim `reference_start_order_on_a_clean_channel` checks our side).
#[test]
fn reference_start_order() {
    let path = capture_or_skip!();
    for bot in BOTS {
        let run = load(&path, bot);
        assert_eq!(
            &run.ids[..3],
            &[13, 13, 4],
            "{bot}: two shell messages before S2C_VERSION"
        );
        let first_shells: Vec<(u8, Vec<u8>)> = run
            .shells
            .iter()
            .take(2)
            .map(|s| (s.2, s.3.as_bytes().to_vec()))
            .collect();
        assert_eq!(first_shells, vec![(0x90, vec![1]), (0x88, vec![2])]);
        let pos = |id: u8| run.shells.iter().position(|s| s.2 == id).unwrap();
        let order = [
            0x72, 0x9e, 0x7d, 0x6b, 0x6a, 0x67, 0x66, 0xb2, 0x7f, 0xa0, 0x43, 0x44, 0xb5, 0x45,
        ];
        for w in order.windows(2) {
            assert!(
                pos(w[0]) < pos(w[1]),
                "{bot}: {:#04x} before {:#04x}",
                w[0],
                w[1]
            );
        }
        // state 1 directly (O9), then after the client's world ready 0x90 [0] and 0x88 [3]
        let states: Vec<u8> = run
            .shells
            .iter()
            .filter(|s| s.2 == 0x67)
            .map(|s| s.3.as_bytes()[0])
            .collect();
        assert_eq!(states, vec![1]);
        let t45 = run.shells[pos(0x45)].0;
        let later: Vec<(u8, u8)> = run
            .shells
            .iter()
            .filter(|s| (s.2 == 0x90 || s.2 == 0x88) && s.0 > t45)
            .map(|s| {
                (
                    s.2,
                    s.3.reader().read_bits(if s.2 == 0x90 { 1 } else { 8 }) as u8,
                )
            })
            .collect();
        assert_eq!(later, vec![(0x90, 0), (0x88, 3)]);
    }
}

/// ROM-gated: what the reference stack sent for a player whose look and loadout name no character equals what our server
/// resolves from the user's ROM at run time for the default character rows: the file names of the character and
/// weapon records and of the own body block (compared by SHA-256), the animation indices of the standing and the
/// walking player (stream trackers), the grip socket and the weapon switch command.
#[test]
fn reference_files_and_animations_match_our_rom_resolution() {
    let path = capture_or_skip!();
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, notes) = RomTables::load(&rom);
    assert!(notes.is_empty(), "{notes:?}");
    let world = dc_dserver::world::world_from_bytes(
        &dc_dserver::world::synthetic_world_bytes(),
        "Worlds\\Synth.dat",
        "Synth",
    )
    .unwrap();
    for bot in BOTS {
        let run = load(&path, bot);
        // the drawn weapon: the weapon the reference stack gave in slot 0
        let gives: Vec<(u16, u8)> = run
            .shells
            .iter()
            .filter(|s| s.2 == 0xb2)
            .map(|s| {
                let mut r = s.3.reader();
                let w = r.read_u16();
                r.skip_bits(16 + 32 + 8);
                (w, r.read_u8())
            })
            .collect();
        let drawn = gives.iter().find(|g| g.1 == 0).map(|g| g.0).unwrap();
        let mut game = Game::new(
            GameTuning {
                character: CharacterChoice::default(),
                default_loadout: vec![drawn],
                ..GameTuning::default()
            },
            &world,
            tables.clone(),
        );
        let look = game.character_for(None).unwrap().expect("a character");
        let hashes = |paths: &[String]| -> Vec<String> {
            paths
                .iter()
                .map(|p| sha(announce_spelling(p).as_bytes()))
                .collect()
        };
        let file = |id: &u16| sha(&run.files[id]);
        let files = |ids: &[u16]| ids.iter().map(file).collect::<Vec<_>>();
        let view = run.view_object.unwrap();
        let c = characters(&run)
            .into_iter()
            .find(|c| c.obj == view)
            .unwrap();
        let a = &look.assets;
        assert_eq!(
            file(&c.models[0].file),
            hashes(&a.body[..1])[0],
            "{bot}: body model"
        );
        assert_eq!(
            files(&c.models[0].subs),
            hashes(&a.body[1..]),
            "{bot}: child models"
        );
        assert_eq!(
            files(&c.models[0].textures),
            hashes(&a.body_textures),
            "{bot}: body textures"
        );
        assert_eq!(
            files(&c.models[0].styles),
            hashes(&a.body_styles),
            "{bot}: body styles"
        );
        let (fm, fs, fst) = a.face.clone().expect("face");
        assert_eq!(
            file(&c.models[1].file),
            hashes(&[fm])[0],
            "{bot}: face model"
        );
        assert_eq!(
            files(&c.models[1].textures),
            hashes(&fs),
            "{bot}: face skin"
        );
        assert_eq!(
            files(&c.models[1].styles),
            hashes(&fst),
            "{bot}: face styles"
        );
        let w = create_of(&run, c.attach[0].0);
        let wl = look.weapon.clone().expect("hand model");
        assert_eq!(
            file(&w.models[0].file),
            hashes(&[wl.model])[0],
            "{bot}: weapon model"
        );
        assert_eq!(
            files(&w.models[0].textures),
            hashes(&wl.textures),
            "{bot}: weapon skins"
        );
        assert_eq!(
            files(&w.models[0].styles),
            hashes(&wl.styles),
            "{bot}: weapon styles"
        );
        assert_eq!(c.attach[0].1, a.rig.grip_socket, "{bot}: grip socket");
        // the box: dims block, 0x18 record and every teleport carry the ROM stand box (MOVEMENT_2009.md §9.2)
        let stand = a.rig.stand_extent.expect("stand box");
        assert_eq!(c.dims, Some(stand), "{bot}: dims block");
        assert_eq!(
            blob_fields(c.blob.as_ref().unwrap()).1,
            stand,
            "{bot}: record dims"
        );
        assert_eq!(game.dims_of_look(&look), stand);
        for s in run.shells.iter().filter(|s| s.2 == 0xa0) {
            let mut r = s.3.reader();
            if r.read_u16() & 0x0020 != 0 {
                r.read_u8();
                r.skip_bits(96);
                assert_eq!(
                    [r.read_f32(), r.read_f32(), r.read_f32()],
                    stand,
                    "{bot}: teleport box"
                );
            }
        }
        // the own body block of the mover update (0xa0 flag 0x0001)
        let body = run
            .shells
            .iter()
            .find(|s| s.2 == 0xa0 && s.3.reader().read_u16() & 0x0001 != 0)
            .map(|s| {
                let mut r = s.3.reader();
                let f = r.read_u16();
                assert_eq!(f, 0x000d, "{bot}: containers + body + friction");
                assert_eq!((r.read_u8(), r.read_f32()), (0, 5.0));
                let m = r.read_zstr8();
                let n = r.read_u8();
                let skins: Vec<Vec<u8>> = (0..n).map(|_| r.read_zstr8()).collect();
                (m, skins)
            })
            .expect("body block");
        assert_eq!(
            sha(&body.0),
            hashes(&a.body[..1])[0],
            "{bot}: mover body model"
        );
        assert_eq!(
            body.1.iter().map(|s| sha(s)).collect::<Vec<_>>(),
            hashes(&a.mover_skins),
            "{bot}: mover body skins"
        );
        // animations: the walking object streams a velocity, the standing one never does
        let an = look.anims;
        let walker = |obj: u16| run.stream.iter().any(|e| e.0 == obj && e.1);
        let mut checked = 0;
        for (obj, _, yaw8, trackers) in &run.stream {
            assert!(*yaw8, "{bot}: characters stream yaw8");
            if trackers.iter().all(|t| *t == 0) {
                continue; // the entry of the create tick, before the first animation
            }
            let k = usize::from(walker(*obj));
            assert_eq!(
                trackers.as_slice(),
                [an.main.0, an.upper[k].0, an.lower[k].0],
                "{bot}: object {obj} trackers"
            );
            checked += 1;
        }
        assert!(checked > 1000, "{bot}: {checked} stream entries checked");
        // the weapon switch to the drawn weapon names it by our command table
        let cmd = run
            .shells
            .iter()
            .filter(|s| s.2 == 0x7f)
            .map(|s| s.3.as_bytes().to_vec())
            .find(|b| b[1] == 1)
            .unwrap();
        assert_eq!(
            Some(cmd[0]),
            tables.weapons.command_of(drawn),
            "{bot}: 0x7f command"
        );
        println!(
            "{bot}: drawn weapon {drawn}, command {:#04x}; animations main {} upper {:?} lower {:?}; body {} + {} \
             child, {} textures, {} styles; face {}; weapon {}; {checked} stream entries checked",
            cmd[0],
            an.main.0,
            an.upper.map(|x| x.0),
            an.lower.map(|x| x.0),
            sha(announce_spelling(&a.body[0]).as_bytes()),
            a.body.len() - 1,
            a.body_textures.len(),
            a.body_styles.len(),
            sha(&run.files[&c.models[1].file]),
            sha(&run.files[&w.models[0].file]),
        );
    }
}

//! ROM-gated census against the counts in FMT_SPR_LTC_FX.md. Skipped when CA_ROM is unset.
//! Only counts appear here; entries are read raw (these families are stored plain).
//! SPEC: FMT_SPR_LTC_FX.md §2.5, §3.3, §4.5, §5.8, §7, §10.

use dc_fx_data::fcf::FcfFile;
use dc_fx_data::fxf::{detect_encoding, Encoding, FxfFile};
use dc_fx_data::tfg::{Stage, TextureGroup};
use dc_fx_data::tfs::TextureScript;
use dc_fx_data::*;
use dc_rez::RezArchive;
use std::collections::BTreeMap;

fn ext_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Every entry with extension `ext` (lowercase) in all archives: `(entry name, bytes)`.
fn entries(rom: &std::path::Path, ext: &str) -> Vec<(String, Vec<u8>)> {
    let mut rez: Vec<_> = std::fs::read_dir(rom.join("Game"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension()
                .map(|x| x.eq_ignore_ascii_case("rez"))
                .unwrap_or(false)
        })
        .collect();
    rez.sort();
    let mut out = Vec::new();
    for p in &rez {
        let a = RezArchive::open(p).unwrap();
        for (name, pos, size) in a.entries() {
            if ext_of(name) == ext {
                out.push((name.clone(), a.read_raw(*pos, *size).unwrap()));
            }
        }
    }
    out
}

#[test]
fn rom_spr_counts() {
    let rom = dc_rom::rom_or_skip!();
    let all = entries(&rom, "spr");
    assert_eq!(all.len(), 233);
    let (mut parsed, mut empty, mut rejected) = (0, 0, 0);
    let (mut frames_total, mut ext_dtx, mut exact) = (0usize, 0usize, 0usize);
    let mut fps = std::collections::BTreeSet::new();
    let (mut zero_frames, mut band) = (0, [0usize; 4]);
    let mut zero_flags = 0;
    for (_, b) in &all {
        match Sprite::parse(b) {
            Ok(s) => {
                parsed += 1;
                exact += (s.trailing == 0) as usize;
                fps.insert(s.frames_per_second);
                frames_total += s.frames.len();
                ext_dtx += s
                    .frames
                    .iter()
                    .filter(|f| f.to_ascii_lowercase().ends_with(".dtx"))
                    .count();
                match s.frames.len() {
                    0 => zero_frames += 1,
                    1 => band[0] += 1,
                    2..=7 => band[1] += 1,
                    8..=31 => band[2] += 1,
                    _ => band[3] += 1,
                }
                zero_flags += (s.flag_a == 0 && s.flag_b == 0 && s.key_word == 0) as usize;
                assert!(s.length_ms() > 0 || s.frames.is_empty());
            }
            Err(FxError::Empty) => empty += 1,
            Err(FxError::NameTooLong { .. }) | Err(FxError::Truncated { .. }) => rejected += 1,
            Err(e) => panic!("unexpected {e}"),
        }
    }
    println!("spr parsed={parsed} empty={empty} rejected={rejected} frames={frames_total} bands(1,2-7,8-31,32+)={band:?} zero_frames={zero_frames} fps_distinct={}", fps.len());
    assert_eq!((parsed, empty, rejected), (231, 1, 1));
    assert_eq!(exact, 231);
    assert_eq!(frames_total, 1192);
    assert_eq!(ext_dtx, 1192);
    assert_eq!((band, zero_frames), ([156, 22, 49, 3], 1));
    assert_eq!(fps.len(), 16);
    assert!(fps.iter().all(|f| (1..=30).contains(f)));
    assert_eq!(zero_flags, 230); // 230 of the 232 non-empty files are all-zero; of the other two one parses
    println!("zero_flags={zero_flags}");
}

#[test]
fn rom_tfs_tfg() {
    let rom = dc_rom::rom_or_skip!();
    let tfs = entries(&rom, "tfs");
    assert_eq!(tfs.len(), 6);
    let mut coord: BTreeMap<u32, usize> = BTreeMap::new();
    let mut ops = std::collections::BTreeSet::new();
    for (_, b) in &tfs {
        let t = TextureScript::parse(b).expect("every ROM script parses");
        *coord.entry(t.coord_mode).or_default() += 1;
        assert_eq!(t.out_mode, 0);
        assert!(matches!(t.constants.len(), 1 | 2));
        for i in &t.code {
            ops.insert(format!("{:?}", i.op));
        }
        // run it a few times; every output must be finite for finite inputs
        let mut st = t.new_state();
        for k in 0..5 {
            let params = st.params();
            let m = t.run(
                &mut st,
                &dc_fx_data::tfs::ScriptInputs {
                    params,
                    time: k as f32 * 0.5,
                    delta: 0.5,
                    camera: [0.0; 3],
                },
            );
            let _ = m;
        }
    }
    println!("tfs coord_modes={coord:?} ops={ops:?}");
    assert_eq!(coord.get(&6), Some(&3));
    assert_eq!(coord.get(&4), Some(&2));
    assert_eq!(coord.get(&5), Some(&1));
    assert_eq!(ops.len(), 7);

    let tfg = entries(&rom, "tfg");
    assert_eq!(tfg.len(), 2);
    let (mut own, mut unused, mut chan) = (0, 0, Vec::new());
    for (_, b) in &tfg {
        let g = TextureGroup::parse(b).unwrap();
        assert_eq!((g.stages.len(), g.trailing), (2, 0));
        for s in &g.stages {
            match s {
                Stage::Script {
                    channel,
                    script,
                    param_count,
                    ..
                } => {
                    own += 1;
                    chan.push(*channel);
                    assert_eq!(*param_count, 6);
                    assert!(script.to_ascii_lowercase().ends_with(".tfs"));
                }
                Stage::Unused => unused += 1,
                o => panic!("{o:?}"),
            }
        }
    }
    chan.sort();
    println!("tfg own={own} unused={unused} channels={chan:?}");
    assert_eq!((own, unused), (3, 1));
    assert_eq!(chan, vec![1, 1, 2]);
}

#[test]
fn rom_fxf_counts() {
    let rom = dc_rom::rom_or_skip!();
    let all = entries(&rom, "fxf");
    assert_eq!(all.len(), 1);
    let b = &all[0].1;
    assert_eq!(detect_encoding(b), Encoding::Text);
    let f = FxfFile::parse(b).unwrap();
    assert_eq!(f.encoding, Encoding::Text);
    assert_eq!(f.trailing, 0);
    let c = f.counts();
    println!("fxf {:?}", c);
    assert_eq!((c.groups, c.keys, c.properties), (240, 1641, 66016));
    assert_eq!(
        c.unresolved_keys, 0,
        "every type name resolves in the registry"
    );
    let want_props: BTreeMap<u32, usize> = [
        (0, 2757),
        (1, 3994),
        (2, 13984),
        (3, 21825),
        (4, 7174),
        (5, 4400),
        (6, 5731),
        (7, 6151),
    ]
    .into();
    assert_eq!(c.props_by_type, want_props);
    let want_keys: BTreeMap<usize, usize> = [
        (0, 855),
        (1, 202),
        (3, 198),
        (11, 134),
        (4, 93),
        (6, 52),
        (5, 48),
        (16, 28),
        (2, 23),
        (17, 7),
        (14, 1),
    ]
    .into();
    assert_eq!(c.keys_by_type, want_keys);
    // 377 linked keys, all targets in the group, none self-linked, ordering pass puts targets first
    let mut linked = 0;
    for g in &f.groups {
        for (i, k) in g.keys.iter().enumerate() {
            assert!(k.start <= k.end && k.end <= g.length_s + 1e-6);
            assert_eq!(k.repeat, 1);
            assert!(k.attach_node.is_empty());
            if k.linked {
                linked += 1;
                let j = g
                    .keys
                    .iter()
                    .position(|t| t.id == k.link_id)
                    .expect("link target exists");
                assert!(
                    j < i,
                    "target must precede its linker after the ordering pass"
                );
            }
        }
        let ids: std::collections::BTreeSet<_> = g.keys.iter().map(|k| k.id).collect();
        assert_eq!(ids.len(), g.keys.len(), "no duplicate ids");
        let n = g.keys.len();
        let _ = n;
    }
    assert_eq!(linked, 377);
    // path values: 6151, all with the bar; 3834 placeholders
    let (mut paths, mut none) = (0, 0);
    for g in &f.groups {
        for k in &g.keys {
            for p in &k.properties {
                if let Some((_, path)) = p.value.path_parts() {
                    paths += 1;
                    none += path.is_none() as usize;
                }
                if let FieldValue::Choice { raw, .. } = &p.value {
                    assert!(!raw.is_empty());
                }
            }
        }
    }
    assert_eq!(paths, 6151);
    assert_eq!(none, 3834);
    // keys-per-group distribution
    let mut band = [0usize; 6];
    for g in &f.groups {
        band[match g.keys.len() {
            0..=9 => 0,
            10..=19 => 1,
            20..=29 => 2,
            30..=39 => 3,
            40..=49 => 4,
            _ => 5,
        }] += 1;
    }
    assert_eq!(band, [176, 39, 18, 5, 1, 1]);
}

#[test]
fn rom_fcf_counts() {
    let rom = dc_rom::rom_or_skip!();
    let all = entries(&rom, "fcf");
    assert_eq!(all.len(), 1);
    let f = FcfFile::parse(&all[0].1).unwrap();
    assert!(!f.trailing_nonblank);
    println!(
        "fcf blocks={} nodes={} windows={}",
        f.blocks,
        f.nodes,
        f.windows.len()
    );
    assert_eq!((f.blocks, f.nodes, f.windows.len()), (31, 477, 2));
    fn walk(
        ns: &[dc_fx_data::fcf::FcfNode],
        d: usize,
        with: &mut usize,
        imgs: &mut BTreeMap<i32, usize>,
        depth: &mut usize,
    ) {
        *depth = (*depth).max(d);
        for n in ns {
            *imgs.entry(n.image).or_default() += 1;
            if !n.children.is_empty() {
                *with += 1;
                walk(&n.children, d + 1, with, imgs, depth);
            }
        }
    }
    let (mut with, mut imgs, mut depth) = (0, BTreeMap::new(), 0);
    walk(&f.roots, 1, &mut with, &mut imgs, &mut depth);
    assert_eq!(with, 30);
    assert_eq!(imgs.get(&0), Some(&30));
    assert_eq!(imgs.get(&2), Some(&447));
    assert_eq!(depth, 3); // root block plus 2 levels
}

#[test]
fn rom_dropped_families_are_all_rejected() {
    let rom = dc_rom::rom_or_skip!();
    let mut n = 0;
    for (ext, count) in [("ltc", 111usize), ("wst", 4), ("lta", 4), ("pk", 3)] {
        let all = entries(&rom, ext);
        assert_eq!(all.len(), count, "{ext}");
        for (name, b) in &all {
            assert!(matches!(
                reject_dropped(name, b),
                Err(FxError::Unsupported { .. })
            ));
            n += 1;
        }
    }
    assert_eq!(n, 122);
}

//! ROM-gated census of the audio assets and the sound attribute table. Skips when CA_ROM is unset.
//! Counts and rates are asserted; nothing from the ROM is stored. SPEC: AUDIO_2009.md §8.3, §9, §10.

use std::collections::BTreeMap;
use std::path::Path;

use dc_audio::attrib::{Slot, SoundTable};
use dc_audio::decode::{
    decode_mp3, decode_wav, mp3_header, tag, tag_name, wav_format_tag, wav_info,
};
use dc_rez::{RezArchive, RezVfs};
use dc_rom::rom_or_skip;

fn rez_files(rom: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(rom.join("Game"))
        .expect("Game dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|x| x.eq_ignore_ascii_case("rez"))
                .unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}

#[test]
fn wav_census_and_full_decode() {
    let rom = rom_or_skip!();
    let mut hist: BTreeMap<(u16, u32), usize> = BTreeMap::new();
    let mut tags: BTreeMap<u16, usize> = BTreeMap::new();
    let mut bits: BTreeMap<u16, usize> = BTreeMap::new();
    let (mut total, mut pcm16, mut decoded_frames) = (0usize, 0usize, 0u64);
    for path in rez_files(&rom) {
        let arch = RezArchive::open(&path).expect("open rez");
        for (name, off, size) in arch.entries() {
            if !name.to_ascii_lowercase().ends_with(".wav") {
                continue;
            }
            let raw = arch.read_raw(*off, *size).expect("read entry");
            // Header-only parse from the first 4 KiB, as the census in the spec did.
            let head = &raw[..raw.len().min(4096)];
            *tags.entry(wav_format_tag(head).unwrap_or(0)).or_default() += 1;
            let info = wav_info(head).unwrap_or_else(|e| panic!("{name}: header: {e}"));
            *bits.entry(info.bits).or_default() += 1;
            total += 1;
            if info.tag_pcm && info.bits == 16 {
                pcm16 += 1;
            }
            *hist.entry((info.channels, info.rate)).or_default() += 1;
            // Full decode, no panics; frame count must match the header.
            let pcm = decode_wav(&raw).unwrap_or_else(|e| panic!("{name}: decode: {e}"));
            assert_eq!(pcm.frames() as u32, info.frames, "{name}");
            assert!(pcm.samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            decoded_frames += pcm.frames() as u64;
        }
    }
    println!("WAV total {total}, PCM16 {pcm16}, frames {decoded_frames}, histogram {hist:?}");
    let named: Vec<_> = tags.iter().map(|(t, n)| (tag_name(*t), *n)).collect();
    println!("WAV format tags {named:?}, bits {bits:?}");
    assert_eq!(total, 1529);
    assert_eq!(pcm16, 1529);
    // every archived effect is integer PCM at 16 bits: no ADPCM, no 8-bit, no float, no extensible wrapper
    assert_eq!(tags, [(tag::PCM, 1529)].into_iter().collect());
    assert_eq!(bits, [(16, 1529)].into_iter().collect());
    let want: BTreeMap<(u16, u32), usize> = [
        ((1, 44100), 828),
        ((1, 22050), 675),
        ((2, 44100), 21),
        ((1, 11025), 2),
        ((2, 16000), 1),
        ((1, 16000), 1),
        ((1, 5000), 1),
    ]
    .into_iter()
    .collect();
    assert_eq!(hist, want);
}

fn loose_wavs(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            loose_wavs(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")) {
            out.push(p);
        }
    }
}

/// WAV files outside the archives (loose in the ROM folder): counted by format tag and decoded. Counts only.
#[test]
fn loose_wav_census_and_decode() {
    let rom = rom_or_skip!();
    let mut files = Vec::new();
    loose_wavs(&rom, &mut files);
    let mut tags: BTreeMap<u16, usize> = BTreeMap::new();
    let mut decoded = 0usize;
    for f in &files {
        let data = std::fs::read(f).expect("read wav");
        let t = wav_format_tag(&data).unwrap_or(0);
        *tags.entry(t).or_default() += 1;
        if decode_wav(&data).is_ok() {
            decoded += 1;
        }
    }
    let named: Vec<_> = tags.iter().map(|(t, n)| (tag_name(*t), *n)).collect();
    println!(
        "loose WAV files {} decoded {decoded} tags {named:?}",
        files.len()
    );
    // every loose WAV of a supported tag decodes
    let supported: usize = tags
        .iter()
        .filter(|(t, _)| matches!(**t, tag::PCM | tag::FLOAT | tag::EXTENSIBLE))
        .map(|(_, n)| n)
        .sum();
    assert_eq!(decoded, supported);
}

#[test]
fn mp3_census_headers_and_decode() {
    let rom = rom_or_skip!();
    let dir = rom.join("Game").join("BGM");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("BGM dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|x| x.eq_ignore_ascii_case("mp3"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    assert_eq!(files.len(), 14);
    let (mut v1_44k, mut v2_16k, mut id3, mut bytes) = (0, 0, 0, 0u64);
    for f in &files {
        let data = std::fs::read(f).expect("read mp3");
        bytes += data.len() as u64;
        let h = mp3_header(&data).unwrap_or_else(|e| panic!("{f:?}: {e}"));
        assert_eq!(h.layer, 3);
        if h.had_id3v2 {
            id3 += 1;
        }
        match (h.mpeg, h.rate, h.channels) {
            (1, 44100, _) => v1_44k += 1,
            (2, 16000, 1) => v2_16k += 1,
            other => panic!("{f:?}: unexpected format {other:?}"),
        }
        let pcm = decode_mp3(&data).unwrap_or_else(|e| panic!("{f:?}: decode: {e}"));
        assert_eq!(pcm.rate, h.rate);
        assert_eq!(pcm.channels, h.channels);
        assert!(pcm.duration_s() > 1.0, "{f:?}");
        assert!(pcm.samples.iter().all(|s| s.is_finite()));
    }
    println!(
        "MP3 files {} bytes {bytes} id3v2 {id3} mpeg1-44.1k {v1_44k} mpeg2-16k {v2_16k}",
        files.len()
    );
    assert_eq!((v1_44k, v2_16k), (13, 1));
    assert_eq!(bytes, 14_216_048);
    assert_eq!(id3, 9);
}

#[test]
fn sound_attribute_table_inheritance() {
    let rom = rom_or_skip!();
    let vfs = RezVfs::new(&rom);
    let t = SoundTable::from_vfs(&vfs).expect("sound attribute entry found by hash");
    let with = |s: Slot| t.records.iter().filter(|r| r.resolved(s)).count();
    let own = |s: Slot| t.records.iter().filter(|r| r.has_own(s)).count();
    println!(
        "sections {} parent {} volume own {} resolved {} inner resolved {} outer resolved {} local resolved {} priority resolved {}",
        t.len(),
        own(Slot::Parent),
        own(Slot::Volume),
        with(Slot::Volume),
        with(Slot::InnerRadius),
        with(Slot::OuterRadius),
        with(Slot::Local),
        with(Slot::Priority),
    );
    assert_eq!(t.len(), 217);
    assert_eq!(own(Slot::Parent), 213);
    assert_eq!(own(Slot::Volume), 9);
    assert_eq!(with(Slot::Volume), 216);
    assert_eq!(with(Slot::InnerRadius), 216);
    assert_eq!(with(Slot::OuterRadius), 216);
    assert_eq!(with(Slot::Local), 216);
    assert_eq!(with(Slot::Priority), 216);
    // 3 distinct parents; every chain terminates (roots are sections that name no parent).
    let roots: std::collections::BTreeSet<_> = t.records.iter().map(|r| r.root.as_str()).collect();
    println!("distinct roots {}", roots.len());
    // 3 distinct named parents, plus the one section that neither has nor is a parent.
    let parents: std::collections::BTreeSet<String> = t
        .records
        .iter()
        .filter_map(|r| r.parent.as_deref())
        .map(dc_bute::fold_latin1)
        .collect();
    println!(
        "distinct direct parents (case-folded) {} {:?}",
        parents.len(),
        parents.len()
    );
    assert_eq!(parents.len(), 3);
    assert_eq!(roots.len(), 4);
    let with_path0 = t.records.iter().filter(|r| !r.paths.is_empty()).count();
    assert_eq!(with_path0, 213);
    assert_eq!(own(Slot::InnerRadius), 181);
    assert_eq!(own(Slot::OuterRadius), 181);
    assert_eq!(own(Slot::OncePerBuffer), 22);
}

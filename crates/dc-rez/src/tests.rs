use super::*;
use dc_rom::sha256_hex;
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};

const SYNTH_KEY: &[u8] = b"synthetic-test-key";

static N: AtomicU32 = AtomicU32::new(0);
fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ca_rez_{}_{}_{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

// ---- synthetic archive builder (our own writer of the public container layout) ---------------------------

fn ext_to_rtype(ext: &str) -> u32 {
    let mut b = [0u8; 4];
    for (i, c) in ext.bytes().rev().enumerate().take(4) {
        b[i] = c;
    }
    u32::from_le_bytes(b)
}

fn file_rec(pos: u32, size: u32, name: &str, ext: &str, nkeys: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for x in [0u32, pos, size, 0, 0, ext_to_rtype(ext), nkeys] {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.extend_from_slice(name.as_bytes());
    v.push(0);
    v.push(0); // empty comment
    v.extend_from_slice(&vec![0u8; 4 * nkeys as usize]);
    v
}

fn dir_rec(pos: u32, size: u32, name: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for x in [1u32, pos, size, 0] {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.extend_from_slice(name.as_bytes());
    v.push(0);
    v
}

/// `(folder, name, ext, bytes)`; folder "" is the root.
fn build_rez(files: &[(&str, &str, &str, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0u8; REZ_HDR_OFFSET as usize + 12];
    let mut folders: Vec<&str> = Vec::new();
    for f in files {
        if !f.0.is_empty() && !folders.contains(&f.0) {
            folders.push(f.0);
        }
    }
    let mut recs: HashMap<&str, Vec<u8>> = HashMap::new();
    for (folder, name, ext, bytes) in files {
        let pos = out.len() as u32;
        out.extend_from_slice(bytes);
        recs.entry(folder)
            .or_default()
            .extend(file_rec(pos, bytes.len() as u32, name, ext, 0));
    }
    let mut root = recs.remove("").unwrap_or_default();
    for folder in folders {
        let stream = recs.remove(folder).unwrap();
        let pos = out.len() as u32;
        out.extend_from_slice(&stream);
        root.extend(dir_rec(pos, stream.len() as u32, folder));
    }
    let rpos = out.len() as u32;
    out.extend_from_slice(&root);
    let h = REZ_HDR_OFFSET as usize;
    out[h..h + 4].copy_from_slice(&1u32.to_le_bytes());
    out[h + 4..h + 8].copy_from_slice(&rpos.to_le_bytes());
    out[h + 8..h + 12].copy_from_slice(&(root.len() as u32).to_le_bytes());
    out
}

fn synth_rom() -> (PathBuf, Vec<u8>) {
    let rom = tmp("rom");
    fs::create_dir_all(rom.join("Game")).unwrap();
    let plain = b"[Mode0]\r\nKey = \"PLAIN\"\r\n".to_vec();
    let secret = encrypt_with_key(
        b"[Mode1]\r\nKey = \"SECRET\"\r\nPad to length........\r\n",
        SYNTH_KEY,
    )
    .unwrap();
    assert!(!is_plain(&secret));
    let rez = build_rez(&[
        ("", "ALPHA", "TXT", plain.clone()),
        ("SUB", "BETA", "GMS", secret),
        ("SUB", "GAMMA", "TXT", b"tail".to_vec()),
    ]);
    fs::write(rom.join("Game").join("TEST.rez"), rez).unwrap();
    (rom, plain)
}

// ---- cipher ----------------------------------------------------------------------------------------------

#[test]
fn blowfish_roundtrip_with_synthetic_key() {
    let sample = b"[MapInfo]\r\nMap = \"Worlds\\\\Synthetic.dat\"\r\nTail!";
    let enc = encrypt_with_key(sample, SYNTH_KEY).unwrap();
    assert!(!is_plain(&enc));
    assert_eq!(
        &enc[enc.len() - 5..],
        &sample[sample.len() - 5..],
        "partial tail block stays plain"
    );
    assert_eq!(decrypt_with_key(&enc, SYNTH_KEY).unwrap(), sample);
}

#[test]
fn blowfish_known_answer_vector() {
    // Public Blowfish test vector (Eric Young / Schneier): key 0000000000000000, plaintext 0, big-endian.
    let bf = Blowfish::new(&[0u8; 8]).unwrap();
    assert_eq!(bf.encrypt_block(0, 0), (0x4ef99745, 0x6198dd78));
    assert_eq!(bf.decrypt_block(0x4ef99745, 0x6198dd78), (0, 0));
}

#[test]
fn blowfish_key_length_is_checked() {
    assert!(matches!(Blowfish::new(&[]), Err(RezError::BadKeyLength(0))));
    assert!(matches!(
        Blowfish::new(&[1, 2, 3]),
        Err(RezError::BadKeyLength(3))
    ));
    assert!(Blowfish::new(&[7u8; 4]).is_ok());
    assert!(Blowfish::new(&[7u8; 56]).is_ok());
    assert!(matches!(
        Blowfish::new(&[7u8; 57]),
        Err(RezError::BadKeyLength(57))
    ));
    assert!(decrypt_with_key(b"x", &[]).is_err());
    assert!(!format!("{:?}", Blowfish::new(&[7u8; 8]).unwrap()).contains('7'));
}

// ---- archive + vfs on synthetic data ---------------------------------------------------------------------

#[test]
fn walk_lists_nested_entries() {
    let (rom, _) = synth_rom();
    let entries = walk_rez(&rom.join("Game").join("TEST.rez")).unwrap();
    let mut names: Vec<_> = entries.iter().map(|e| e.0.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["ALPHA.TXT", "SUB\\BETA.GMS", "SUB\\GAMMA.TXT"]);
    let dir = rez_directory(&rom.join("Game").join("TEST.rez")).unwrap();
    assert!(dir.contains_key("BETA.GMS"));
    let _ = fs::remove_dir_all(&rom);
}

#[test]
fn vfs_reads_plain_and_encrypted_entries_with_explicit_key() {
    let (rom, plain) = synth_rom();
    let vfs = RezVfs::with_key(&rom, SYNTH_KEY).unwrap();
    assert_eq!(vfs.entry("TEST.rez", "alpha.txt").unwrap().unwrap(), plain);
    let beta = vfs
        .entry_text_latin1("test.REZ", "SUB\\BETA.GMS")
        .unwrap()
        .unwrap();
    assert!(beta.starts_with("[Mode1]") && beta.contains("SECRET"));
    // by relative path (documented fallback) and via backslashes
    assert!(vfs.entry("TEST.rez", "SUB\\GAMMA.TXT").unwrap().is_some());
    assert_eq!(vfs.list_entries("TEST.rez").unwrap().len(), 3);
    assert!(matches!(
        vfs.list_entries("NOPE.rez"),
        Err(RezError::ArchiveNotFound(_))
    ));
    let _ = fs::remove_dir_all(&rom);
}

#[test]
fn vfs_without_key_fails_only_for_encrypted_entries() {
    // No Engine.exe in the synthetic ROM: plain entries still work, encrypted ones give a typed key error.
    let (rom, plain) = synth_rom();
    let vfs = RezVfs::new(&rom);
    assert_eq!(vfs.entry("TEST.rez", "ALPHA.TXT").unwrap().unwrap(), plain);
    assert!(matches!(
        vfs.entry("TEST.rez", "SUB\\BETA.GMS"),
        Err(RezError::Rom(RomError::KeyUnavailable(_)))
    ));
    let _ = fs::remove_dir_all(&rom);
}

#[test]
fn vfs_negative_lookups_are_cached_and_typed() {
    let (rom, _) = synth_rom();
    let vfs = RezVfs::with_key(&rom, SYNTH_KEY).unwrap();
    assert_eq!(vfs.entry("TEST.rez", "MISSING.TXT").unwrap(), None);
    assert!(vfs
        .missing
        .lock()
        .unwrap()
        .contains(&("TEST.REZ".to_string(), "P:MISSING.TXT".to_string())));
    assert_eq!(vfs.entry("ABSENT.rez", "X.TXT").unwrap(), None);
    assert!(matches!(
        vfs.archives.lock().unwrap().get("ABSENT.REZ"),
        Some(Ok(None))
    ));
    assert!(matches!(
        vfs.entry("TEST.rez", "../escape.txt"),
        Err(RezError::BadName(_))
    ));
    assert!(matches!(
        vfs.entry("TEST.rez", ""),
        Err(RezError::BadName(_))
    ));
    let _ = fs::remove_dir_all(&rom);
}

#[test]
fn vfs_override_shadowing() {
    let (rom, _) = synth_rom();
    let mut vfs = RezVfs::with_key(&rom, SYNTH_KEY).unwrap();
    let over = tmp("override");
    let sub = over.join("TEST");
    fs::create_dir_all(&sub).unwrap();
    let custom = "[Mode0]\r\nKey = \"CUSTOM\"\r\n";
    fs::write(sub.join("ALPHA.TXT"), custom.as_bytes()).unwrap();
    vfs.add_override_dir(&over);
    assert_eq!(
        vfs.entry_text_latin1("TEST.rez", "ALPHA.TXT")
            .unwrap()
            .unwrap(),
        custom
    );
    // later override dirs win
    let over2 = tmp("override2");
    fs::write(over2.join("ALPHA.TXT"), b"second layer").unwrap();
    vfs.add_override_dir(&over2);
    assert_eq!(
        vfs.entry_text_latin1("TEST.rez", "ALPHA.TXT")
            .unwrap()
            .unwrap(),
        "second layer"
    );
    let _ = fs::remove_dir_all(&rom);
    let _ = fs::remove_dir_all(&over);
    let _ = fs::remove_dir_all(&over2);
}

#[test]
fn truncated_entry_is_a_typed_error() {
    let (rom, _) = synth_rom();
    let path = rom.join("Game").join("TEST.rez");
    let arch = RezArchive::open(&path).unwrap();
    assert!(matches!(
        arch.read_raw(u64::MAX - 2, 10),
        Err(RezError::Truncated { .. })
    ));
    assert!(matches!(
        arch.read_raw(0, u32::MAX),
        Err(RezError::Truncated { .. })
    ));
    // archive whose entry points past EOF: listing works, reading errors
    let mut bytes = fs::read(&path).unwrap();
    let alpha_pos = REZ_HDR_OFFSET as usize + 12;
    let _ = alpha_pos;
    let l = bytes.len();
    // corrupt the root record's first entry size field (ALPHA is in the root stream at the end)
    let rpos = u32::from_le_bytes(
        bytes[REZ_HDR_OFFSET as usize + 4..REZ_HDR_OFFSET as usize + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    bytes[rpos + 8..rpos + 12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(l > rpos + 12);
    fs::write(&path, &bytes).unwrap();
    let arch = RezArchive::open(&path).unwrap();
    assert!(matches!(
        arch.entry_raw("ALPHA.TXT"),
        Err(RezError::Truncated { .. })
    ));
    let vfs = RezVfs::with_key(&rom, SYNTH_KEY).unwrap();
    assert!(matches!(
        vfs.entry("TEST.rez", "ALPHA.TXT"),
        Err(RezError::Truncated { .. })
    ));
    let _ = fs::remove_dir_all(&rom);
}

fn set_root(bytes: &mut [u8], pos: u32, size: u32) {
    let h = REZ_HDR_OFFSET as usize;
    bytes[h + 4..h + 8].copy_from_slice(&pos.to_le_bytes());
    bytes[h + 8..h + 12].copy_from_slice(&size.to_le_bytes());
}

fn walk_bytes(b: &[u8]) -> Result<Vec<(String, u64, u32)>, RezError> {
    walk_reader(&mut Cursor::new(b.to_vec()), b.len() as u64)
}

#[test]
fn hostile_archives_are_rejected() {
    // header only / too short
    assert!(matches!(
        walk_bytes(&[0u8; 10]),
        Err(RezError::Truncated { .. })
    ));
    let base = vec![0u8; REZ_HDR_OFFSET as usize + 12];
    // root range past EOF, overflowing range
    let mut b = base.clone();
    set_root(&mut b, 1_000_000, 16);
    assert!(matches!(walk_bytes(&b), Err(RezError::Truncated { .. })));
    set_root(&mut b, u32::MAX, u32::MAX);
    assert!(matches!(walk_bytes(&b), Err(RezError::Truncated { .. })));

    // directory entry that points to itself: cycle rejected
    let rec_pos = base.len() as u32;
    let rec = dir_rec(rec_pos, 0, "LOOP");
    let mut b = base.clone();
    b.extend_from_slice(&rec);
    let rec = dir_rec(rec_pos, b.len() as u32 - rec_pos, "LOOP");
    b.truncate(base.len());
    b.extend_from_slice(&rec);
    set_root(&mut b, rec_pos, rec.len() as u32);
    assert!(matches!(walk_bytes(&b), Err(RezError::Revisited { .. })));

    // unterminated name, huge key count
    let mut b = base.clone();
    let mut rec = file_rec(0, 0, "NAME", "TXT", 0);
    let nk = 24;
    rec[nk..nk + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    b.extend_from_slice(&rec);
    set_root(&mut b, base.len() as u32, rec.len() as u32);
    assert!(matches!(walk_bytes(&b), Err(RezError::Malformed(_))));
    let mut b = base.clone();
    let rec = vec![
        0u8, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 65, 65, 65, 65,
        66,
    ];
    b.extend_from_slice(&rec);
    set_root(&mut b, base.len() as u32, rec.len() as u32);
    assert!(matches!(walk_bytes(&b), Err(RezError::Malformed(_))));
    // truncated record
    let mut b = base.clone();
    b.extend_from_slice(&[1, 0, 0]);
    set_root(&mut b, base.len() as u32, 3);
    assert!(walk_bytes(&b).is_err());
}

#[test]
fn deep_nesting_is_capped() {
    // chain of 40 nested directories, each in its own stream
    let mut b = vec![0u8; REZ_HDR_OFFSET as usize + 12];
    let mut child: Option<(u32, u32)> = None;
    for level in 0..40 {
        let rec = match child {
            Some((p, s)) => dir_rec(p, s, &format!("D{level}")),
            None => file_rec(0, 0, "LEAF", "TXT", 0),
        };
        let pos = b.len() as u32;
        b.extend_from_slice(&rec);
        child = Some((pos, rec.len() as u32));
    }
    let (p, s) = child.unwrap();
    set_root(&mut b, p, s);
    assert!(matches!(walk_bytes(&b), Err(RezError::TooDeep)));
}

// ---- seeded fuzzing ---------------------------------------------------------------------------------------

struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % (n as u64).max(1)) as usize
    }
}

fn mutate(r: &mut XorShift, seed: &[u8]) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..1 + r.below(10) {
        match r.below(4) {
            0 if !b.is_empty() => {
                let p = r.below(b.len());
                b[p] = r.next() as u8;
            }
            1 if b.len() > 4 => {
                // overwrite a 32-bit field with an interesting value
                let p = r.below(b.len() - 4);
                let v = [
                    0u32,
                    1,
                    0xffff_ffff,
                    0x7fff_ffff,
                    28,
                    b.len() as u32,
                    (b.len() as u32).wrapping_sub(1),
                ][r.below(7)];
                b[p..p + 4].copy_from_slice(&v.to_le_bytes());
            }
            2 => {
                let t = r.below(b.len() + 1);
                b.truncate(t);
            }
            _ => b.push(r.next() as u8),
        }
    }
    b
}

#[test]
fn fuzz_walk_rez_in_memory() {
    let mut r = XorShift(0x2545_f491_4f6c_dd1d);
    let seed = build_rez(&[
        ("", "A", "TXT", b"hello".to_vec()),
        ("D1", "B", "GMS", b"world".to_vec()),
        ("D1", "C", "TXT", b"x".to_vec()),
        ("D2", "E", "TXT", b"y".to_vec()),
    ]);
    let (mut ok, mut err) = (0u32, 0u32);
    for i in 0..10_000 {
        let buf = if i % 5 == 0 {
            let n = r.below(400);
            (0..n).map(|_| r.next() as u8).collect()
        } else {
            mutate(&mut r, &seed)
        };
        match walk_bytes(&buf) {
            Ok(v) => {
                ok += 1;
                assert!(v.len() <= buf.len());
            }
            Err(_) => err += 1,
        }
    }
    println!("fuzz_walk_rez_in_memory: ok={ok} err={err}");
    assert!(ok > 0 && err > 0);
}

#[test]
fn fuzz_walk_rez_over_temp_files() {
    let mut r = XorShift(0x0123_4567_89ab_cdef);
    let dir = tmp("fuzzfiles");
    let seed = build_rez(&[
        ("", "A", "TXT", b"hello".to_vec()),
        ("D1", "B", "GMS", b"world".to_vec()),
    ]);
    let path = dir.join("F.rez");
    for _ in 0..2_000 {
        let buf = mutate(&mut r, &seed);
        fs::write(&path, &buf).unwrap();
        if let Ok(arch) = RezArchive::open(&path) {
            for (name, pos, size) in arch.entries().to_vec() {
                let _ = arch.read_raw(pos, size);
                let _ = arch.entry_raw(&name);
            }
        }
        let _ = walk_rez(&path);
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fuzz_cipher_with_random_data_and_keys() {
    let mut r = XorShift(0xfeed_face_dead_beef);
    for _ in 0..2_000 {
        let klen = r.below(70);
        let key: Vec<u8> = (0..klen).map(|_| r.next() as u8).collect();
        let n = r.below(100);
        let data: Vec<u8> = (0..n).map(|_| r.next() as u8).collect();
        match Blowfish::new(&key) {
            Ok(bf) => assert_eq!(bf.decrypt_bytes(&bf.encrypt_bytes(&data)), data),
            Err(e) => assert!(!(4..=56).contains(&klen), "{e}"),
        }
    }
}

// ---- ROM-gated smoke -------------------------------------------------------------------------------------

#[test]
fn rom_decrypt_smoke_matches_pinned_digest() {
    let rom = dc_rom::rom_or_skip!();
    let vfs = RezVfs::new(&rom); // key comes lazily from the user's own Engine.exe
    let mut count = 0usize;
    let mut canon = Vec::new();
    for rez in ["ATTRIBUTES.rez", "GMS.rez"] {
        let mut entries = vfs.list_entries(rez).unwrap();
        entries.sort();
        for (path, _, _) in entries {
            let bytes = vfs.entry(rez, &path).unwrap().unwrap_or_default();
            canon.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            canon.extend_from_slice(&dc_rom::sha256(&bytes));
            count += 1;
        }
    }
    println!(
        "rom decrypt smoke: {count} entries, digest {}",
        sha256_hex(&canon)
    );
    assert_eq!(count, PINNED_ENTRY_COUNT);
    assert_eq!(sha256_hex(&canon), PINNED_DECRYPT_DIGEST);
}

const PINNED_ENTRY_COUNT: usize = 196;
const PINNED_DECRYPT_DIGEST: &str =
    "303bb66beb124d72c9d2b5559eb8d1cf1f223cd916c82387c894cdcfad09360a";

#[test]
fn rom_every_archive_walks_under_strict_parser() {
    let rom = dc_rom::rom_or_skip!();
    let mut archives = 0usize;
    let mut entries = 0usize;
    for e in fs::read_dir(rom.join("Game")).unwrap() {
        let p = e.unwrap().path();
        if p.extension()
            .map(|x| x.eq_ignore_ascii_case("rez"))
            .unwrap_or(false)
        {
            let arch = RezArchive::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            // every listed entry lies inside the file
            for (_, pos, size) in arch.entries() {
                assert!(arch.read_raw(*pos, *size).is_ok(), "{}", p.display());
            }
            archives += 1;
            entries += arch.entries().len();
        }
    }
    println!("rom walk: {archives} archives, {entries} entries");
    assert!(archives > 0);
}

// ---- plain binary entries and full-path lookup -------------------------------------------------------------

/// Bytes that would pass `is_plain` as text but belong to a binary entry, plus a binary-looking blob.
fn plain_looking_binary() -> Vec<u8> {
    b"PRINTABLE ASCII HEADER ONLY, NOT A TEXT TABLE".to_vec()
}

fn dup_rom() -> (PathBuf, Vec<u8>, Vec<u8>) {
    let rom = tmp("dup");
    fs::create_dir_all(rom.join("Game")).unwrap();
    let a = b"first duplicate\r\n".to_vec();
    let b = b"second duplicate\r\n".to_vec();
    let bin = vec![0xA5u8; 40];
    let rez = build_rez(&[
        ("A", "SAME", "TXT", a.clone()),
        ("B", "SAME", "TXT", b.clone()),
        ("B", "ONLY", "TXT", b"unique\r\n".to_vec()),
        ("B", "MODEL", "LTB", bin),
    ]);
    fs::write(rom.join("Game").join("DUP.rez"), rez).unwrap();
    (rom, a, b)
}

#[test]
fn text_extension_rule_is_case_insensitive() {
    for n in ["a.txt", "A.TXT", "x\\y.Gms", "x/y.bMs"] {
        assert!(is_text_name(n), "{n}");
    }
    for n in [
        "a.ltb",
        "a.dat",
        "a.dtx",
        "txt",
        "a.txt.bin",
        "dir.txt\\file",
        "",
    ] {
        assert!(!is_text_name(n), "{n}");
    }
}

#[test]
fn vfs_returns_binary_entries_as_stored() {
    let rom = tmp("bin");
    fs::create_dir_all(rom.join("Game")).unwrap();
    // A binary blob that is NOT plain text (would be decrypted by the old rule) and one that looks plain.
    let blob: Vec<u8> = (0..64u8)
        .map(|i| i.wrapping_mul(7).wrapping_add(200))
        .collect();
    assert!(!is_plain(&blob));
    let lookalike = plain_looking_binary();
    let secret = encrypt_with_key(
        b"[Mode1]\r\nKey = \"SECRET\"\r\nPad to length........\r\n",
        SYNTH_KEY,
    )
    .unwrap();
    let rez = build_rez(&[
        ("W", "MAP", "DAT", blob.clone()),
        ("M", "BODY", "ltb", lookalike.clone()),
        ("T", "SKIN", "DTX", blob.clone()),
        ("", "TABLE", "txt", secret),
    ]);
    fs::write(rom.join("Game").join("MIX.rez"), rez).unwrap();
    let vfs = RezVfs::with_key(&rom, SYNTH_KEY).unwrap();
    assert_eq!(vfs.entry("MIX.rez", "W\\MAP.DAT").unwrap().unwrap(), blob);
    assert_eq!(
        vfs.entry("MIX.rez", "m/body.LTB").unwrap().unwrap(),
        lookalike
    );
    assert_eq!(vfs.entry("MIX.rez", "T\\SKIN.DTX").unwrap().unwrap(), blob);
    // text entry still decrypted; raw view stays encrypted
    let t = vfs.entry_text("MIX.rez", "table.TXT").unwrap().unwrap();
    assert!(t.starts_with("[Mode1]"));
    let raw = vfs.entry_raw("MIX.rez", "TABLE.TXT").unwrap().unwrap();
    assert!(!is_plain(&raw));
    // no key needed for binary entries
    let nokey = RezVfs::new(&rom);
    assert_eq!(nokey.entry("MIX.rez", "W\\MAP.DAT").unwrap().unwrap(), blob);
    let _ = fs::remove_dir_all(&rom);
}

#[test]
fn override_binary_files_are_not_decrypted() {
    let (rom, _, _) = dup_rom();
    let ov = tmp("ov");
    let blob: Vec<u8> = (0..64u8)
        .map(|i| i.wrapping_mul(3).wrapping_add(130))
        .collect();
    fs::write(ov.join("MODEL.LTB"), &blob).unwrap();
    let mut vfs = RezVfs::with_key(&rom, SYNTH_KEY).unwrap();
    vfs.add_override_dir(&ov);
    assert_eq!(vfs.entry("DUP.rez", "B\\MODEL.LTB").unwrap().unwrap(), blob);
    let _ = fs::remove_dir_all(&rom);
    let _ = fs::remove_dir_all(&ov);
}

#[test]
fn full_path_is_primary_key_for_duplicate_basenames() {
    let (rom, a, b) = dup_rom();
    let vfs = RezVfs::with_key(&rom, SYNTH_KEY).unwrap();
    assert_eq!(vfs.entry("DUP.rez", "A\\SAME.TXT").unwrap().unwrap(), a);
    assert_eq!(vfs.entry("DUP.rez", "b/same.txt").unwrap().unwrap(), b);
    // a bare basename is NOT a full path: no silent pick among duplicates (nor for unique ones)
    assert_eq!(vfs.entry("DUP.rez", "SAME.TXT").unwrap(), None);
    assert_eq!(vfs.entry("DUP.rez", "ONLY.TXT").unwrap(), None);
    // explicit fallback: unique basename resolves, ambiguous basename does not
    assert_eq!(
        vfs.entry_by_basename("DUP.rez", "only.txt")
            .unwrap()
            .unwrap(),
        b"unique\r\n".to_vec()
    );
    assert_eq!(vfs.entry_by_basename("DUP.rez", "SAME.TXT").unwrap(), None);
    let arch = RezArchive::open(&rom.join("Game").join("DUP.rez")).unwrap();
    assert_eq!(arch.basename_count("SAME.TXT"), 2);
    assert_eq!(arch.basename_count("only.txt"), 1);
    assert_eq!(arch.basename_count("nope.txt"), 0);
    assert_ne!(arch.locate("A\\SAME.TXT"), arch.locate("B\\SAME.TXT"));
    assert!(arch.locate("A\\SAME.TXT").is_some());
    assert_eq!(arch.locate("SAME.TXT"), None);
    assert_eq!(arch.locate_basename("SAME.TXT"), None);
    assert!(arch.locate_basename("B\\ONLY.TXT").is_some());
    let _ = fs::remove_dir_all(&rom);
}

#[test]
fn negative_cache_keeps_path_and_basename_lookups_apart() {
    let (rom, _, _) = dup_rom();
    let vfs = RezVfs::with_key(&rom, SYNTH_KEY).unwrap();
    assert_eq!(vfs.entry("DUP.rez", "ONLY.TXT").unwrap(), None);
    assert!(vfs
        .entry_by_basename("DUP.rez", "ONLY.TXT")
        .unwrap()
        .is_some());
    let _ = fs::remove_dir_all(&rom);
}

// a case-sensitive volume: the archive is found under the folder's own spelling of its name
#[test]
fn case_fold_lookup_finds_the_folder_spelling() {
    let d = tmp("casefold");
    fs::write(d.join("GUNS_M_PV_PISTOL.rez"), b"x").unwrap();
    fs::create_dir_all(d.join("Sub.rez")).unwrap();
    // a case-insensitive volume (Windows, default APFS) answers the exact join itself; a case-sensitive one the
    // folder's spelling: both name the same file
    let found = case_fold_lookup(&d, "GUNS_M_PV_Pistol.rez").expect("found by case folding");
    assert!(found.is_file());
    assert_eq!(
        found
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase(),
        "guns_m_pv_pistol.rez"
    );
    assert_eq!(
        case_fold_lookup(&d, "GUNS_M_PV_PISTOL.rez"),
        Some(d.join("GUNS_M_PV_PISTOL.rez"))
    );
    assert_eq!(case_fold_lookup(&d, "missing.rez"), None);
    assert_eq!(
        case_fold_lookup(&d, "sub.rez"),
        None,
        "a folder is not an archive"
    );
    assert_eq!(case_fold_lookup(&d.join("nowhere"), "a.rez"), None);
    let _ = fs::remove_dir_all(&d);
}

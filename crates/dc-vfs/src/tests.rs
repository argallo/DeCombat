// Provenance: ORIGINAL (our own tests; no ROM, temp folders and a fake archive layer)
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fresh empty folder under the system temp dir.
fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc_vfs_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn put(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

/// One archive `A.rez` with entries `X/HELLO.TXT` (stored "XORED", decrypted to "hello") and `X/BIN.DAT`; counts
/// archive reads so the negative cache can be seen.
#[derive(Debug, Default)]
struct FakeLayer {
    reads: AtomicUsize,
}

impl ArchiveLayer for FakeLayer {
    fn read(
        &self,
        archive: &str,
        name: &str,
        _by_basename: bool,
        decode: bool,
    ) -> Result<Option<Vec<u8>>, VfsError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if !archive.eq_ignore_ascii_case("A.rez") {
            return Ok(None);
        }
        Ok(match relpath_key(name).as_str() {
            "X/HELLO.TXT" => Some(if decode {
                b"hello".to_vec()
            } else {
                vec![0xff, 1, 2]
            }),
            "X/BIN.DAT" => Some(vec![9, 9, 9, 9]),
            _ => None,
        })
    }
    fn head(&self, archive: &str, name: &str, n: usize) -> Result<Option<Vec<u8>>, VfsError> {
        Ok(self.read(archive, name, false, false)?.map(|mut v| {
            v.truncate(n);
            v
        }))
    }
    fn list(&self, archive: &str) -> Result<Vec<(String, u64, u32)>, VfsError> {
        if archive.eq_ignore_ascii_case("A.rez") {
            Ok(vec![
                ("X/HELLO.TXT".into(), 0, 3),
                ("X/BIN.DAT".into(), 3, 4),
            ])
        } else {
            Err(VfsError::ArchiveNotFound(archive.into()))
        }
    }
    fn decrypt_text(&self, _name: &str, _raw: &[u8]) -> Result<Vec<u8>, VfsError> {
        Ok(b"decrypted".to_vec())
    }
    fn root(&self) -> &Path {
        Path::new("fake-root")
    }
}

#[test]
fn an_empty_vfs_answers_none_and_lists_nothing() {
    let v = Vfs::new();
    assert_eq!(v.entry("A.rez", "X/HELLO.TXT").unwrap(), None);
    assert_eq!(v.entry_head("A.rez", "X/HELLO.TXT", 2).unwrap(), None);
    assert!(matches!(
        v.list_entries("A.rez"),
        Err(VfsError::ArchiveNotFound(_))
    ));
    assert!(!v.has_archive());
    assert_eq!(v.archive_root(), None);
}

#[test]
fn a_root_answers_by_exact_lower_case_path_only() {
    let root = temp("root_exact");
    put(&root, "weapons/scar/weapon.json", b"{}");
    let mut v = Vfs::new();
    v.add_root(&root);
    // any archive name, separators and case of the request fold to the lower-case relative path
    assert_eq!(
        v.entry("ANY.rez", "Weapons\\SCAR\\weapon.json").unwrap(),
        Some(b"{}".to_vec())
    );
    assert_eq!(
        v.entry_raw("", "weapons/scar/weapon.json").ok(),
        None,
        "an empty archive name is refused"
    );
    // no basename fallback: the bare file name does not find it
    assert_eq!(v.entry("ANY.rez", "weapon.json").unwrap(), None);
    assert_eq!(v.entry_by_basename("ANY.rez", "weapon.json").unwrap(), None);
    assert_eq!(
        v.entry_head("ANY.rez", "weapons/scar/weapon.json", 1)
            .unwrap(),
        Some(b"{".to_vec())
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn layers_answer_override_then_root_then_archive_and_later_folders_win() {
    let (o1, o2, r1, r2) = (
        temp("ord_o1"),
        temp("ord_o2"),
        temp("ord_r1"),
        temp("ord_r2"),
    );
    let layer = Arc::new(FakeLayer::default());
    let mut v = Vfs::with_archive(layer.clone());
    v.add_override_dir(&o1);
    v.add_override_dir(&o2);
    v.add_root(&r1);
    v.add_root(&r2);
    // archive only
    assert_eq!(
        v.entry("A.rez", "X/BIN.DAT").unwrap(),
        Some(vec![9, 9, 9, 9])
    );
    // root over archive; the later root over the earlier one
    put(&r1, "x/bin.dat", b"r1");
    assert_eq!(v.entry("A.rez", "X/BIN.DAT").unwrap(), Some(b"r1".to_vec()));
    put(&r2, "x/bin.dat", b"r2");
    assert_eq!(v.entry("A.rez", "X/BIN.DAT").unwrap(), Some(b"r2".to_vec()));
    // override over root (basename candidate); the later override over the earlier one
    put(&o1, "BIN.DAT", b"o1");
    assert_eq!(v.entry("A.rez", "X/BIN.DAT").unwrap(), Some(b"o1".to_vec()));
    put(&o2, "A.rez/X/BIN.DAT", b"o2");
    assert_eq!(v.entry("A.rez", "X/BIN.DAT").unwrap(), Some(b"o2".to_vec()));
    for d in [o1, o2, r1, r2] {
        let _ = fs::remove_dir_all(d);
    }
}

#[test]
fn text_in_a_folder_is_returned_plain_or_handed_to_the_archive_layer() {
    let root = temp("text");
    put(&root, "plain.txt", b"hello table\r\n");
    put(&root, "cipher.txt", &[0xff, 0x00, 0x10]);
    let mut bare = Vfs::new();
    bare.add_root(&root);
    assert_eq!(
        bare.entry("A.rez", "plain.txt").unwrap(),
        Some(b"hello table\r\n".to_vec())
    );
    assert!(matches!(
        bare.entry("A.rez", "cipher.txt"),
        Err(VfsError::NoKey(_))
    ));
    // raw reads never decrypt
    assert_eq!(
        bare.entry_raw("A.rez", "cipher.txt").unwrap(),
        Some(vec![0xff, 0x00, 0x10])
    );
    let mut keyed = Vfs::with_archive(Arc::new(FakeLayer::default()));
    keyed.add_root(&root);
    assert_eq!(
        keyed.entry("A.rez", "cipher.txt").unwrap(),
        Some(b"decrypted".to_vec())
    );
    assert_eq!(
        keyed.entry_text("A.rez", "X/HELLO.TXT").unwrap().as_deref(),
        Some("hello")
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn archive_misses_are_cached_and_folders_still_answer_first() {
    let layer = Arc::new(FakeLayer::default());
    let mut v = Vfs::with_archive(layer.clone());
    assert_eq!(v.entry("A.rez", "NOPE.TXT").unwrap(), None);
    assert_eq!(v.entry("A.rez", "NOPE.TXT").unwrap(), None);
    assert_eq!(
        layer.reads.load(Ordering::Relaxed),
        1,
        "the second miss is answered by the cache"
    );
    assert!(v.is_cached_missing("a.REZ", "nope.txt", false));
    assert!(!v.is_cached_missing("A.rez", "NOPE.TXT", true));
    // a missing archive is a miss too
    assert_eq!(v.entry("B.rez", "X.TXT").unwrap(), None);
    assert!(v.is_cached_missing("B.rez", "X.TXT", false));
    // a folder added later still answers before the cache
    let o = temp("neg_override");
    put(&o, "NOPE.TXT", b"now here");
    v.add_override_dir(&o);
    assert_eq!(
        v.entry("A.rez", "NOPE.TXT").unwrap(),
        Some(b"now here".to_vec())
    );
    let _ = fs::remove_dir_all(&o);
}

#[test]
fn bad_names_are_refused_before_any_layer() {
    let layer = Arc::new(FakeLayer::default());
    let v = Vfs::with_archive(layer.clone());
    for bad in [
        "",
        "../x.txt",
        "a/../../x",
        "/abs.txt",
        "C:x.txt",
        "nul\0.txt",
    ] {
        assert!(
            matches!(v.entry("A.rez", bad), Err(VfsError::BadName(_))),
            "{bad:?}"
        );
        assert!(
            matches!(v.entry_head("A.rez", bad, 4), Err(VfsError::BadName(_))),
            "{bad:?}"
        );
        assert!(
            matches!(v.entry(bad, "x.txt"), Err(VfsError::BadName(_))),
            "{bad:?}"
        );
    }
    assert!(matches!(
        v.list_entries("../A.rez"),
        Err(VfsError::BadName(_))
    ));
    assert_eq!(layer.reads.load(Ordering::Relaxed), 0);
}

#[test]
fn the_archive_layer_lists_and_reads_heads() {
    let v = Vfs::with_archive(Arc::new(FakeLayer::default()));
    assert_eq!(v.list_entries("A.rez").unwrap().len(), 2);
    assert!(matches!(
        v.list_entries("B.rez"),
        Err(VfsError::ArchiveNotFound(_))
    ));
    assert_eq!(
        v.entry_head("A.rez", "X/BIN.DAT", 2).unwrap(),
        Some(vec![9, 9])
    );
    assert_eq!(v.archive_root(), Some(Path::new("fake-root")));
}

#[test]
fn text_helpers() {
    assert!(is_text_name("a/B.TXT") && is_text_name("x.gms") && is_text_name("y.Bms"));
    assert!(!is_text_name("x.dat") && !is_text_name("txt"));
    assert!(is_plain(b"a\tb\r\n") && !is_plain(b"") && !is_plain(&[0x80]));
    assert_eq!(latin1_text(vec![b'a', 0xe9, 0, 0]), "a\u{e9}");
}

#[test]
fn a_layer_without_an_any_index_finds_nothing_and_is_shared() {
    let v = Vfs::with_archive(Arc::new(FakeLayer::default()));
    let layer = v.archive_layer().expect("the layer");
    assert_eq!(layer.find_any(b"A\\B.DTX").unwrap(), None);
    assert!(!layer.has_any(b"A\\B.DTX"));
    assert_eq!(layer.any_count(), 0);
    assert!(Vfs::new().archive_layer().is_none());
}

// ---- packs, `@mod:` names and `install` (content profiles) ----

#[test]
fn install_orders_pack_over_override_over_root_over_archive() {
    let (p1, p2, o, r) = (
        temp("inst_p1"),
        temp("inst_p2"),
        temp("inst_o"),
        temp("inst_r"),
    );
    let content = Content {
        packs: PackTable::new([Pack::new("one", &p1), Pack::new("two", &p2)]),
        overrides: vec![o.clone()],
        roots: vec![r.clone()],
    };
    let v = install(&content, Some(Arc::new(FakeLayer::default())));
    assert_eq!(v.roots(), std::slice::from_ref(&r));
    assert_eq!(v.override_dirs(), std::slice::from_ref(&o));
    assert_eq!(v.packs(), &content.packs);
    // archive
    assert_eq!(
        v.entry("A.rez", "X/BIN.DAT").unwrap(),
        Some(vec![9, 9, 9, 9])
    );
    // base folder over archive
    put(&r, "x/bin.dat", b"root");
    assert_eq!(
        v.entry("A.rez", "X/BIN.DAT").unwrap(),
        Some(b"root".to_vec())
    );
    // override folder over base folder
    put(&o, "X/BIN.DAT", b"override");
    assert_eq!(
        v.entry("A.rez", "X/BIN.DAT").unwrap(),
        Some(b"override".to_vec())
    );
    // a pack's overrides/ over the override folder; the later pack over the earlier one
    put(&p1, "overrides/X/BIN.DAT", b"pack one");
    assert_eq!(
        v.entry("A.rez", "X/BIN.DAT").unwrap(),
        Some(b"pack one".to_vec())
    );
    put(&p2, "overrides/BIN.DAT", b"pack two");
    assert_eq!(
        v.entry("A.rez", "X/BIN.DAT").unwrap(),
        Some(b"pack two".to_vec())
    );
    assert_eq!(
        v.entry_head("A.rez", "X/BIN.DAT", 4).unwrap(),
        Some(b"pack".to_vec())
    );
    // folders are never cached as missing: a file added later is seen
    assert_eq!(v.entry("A.rez", "Y/NEW.DAT").unwrap(), None);
    put(&r, "y/new.dat", b"late");
    assert_eq!(
        v.entry("A.rez", "Y/NEW.DAT").unwrap(),
        Some(b"late".to_vec())
    );
    for d in [p1, p2, o, r] {
        let _ = fs::remove_dir_all(d);
    }
}

#[test]
fn mod_names_resolve_to_pack_files_only() {
    let (p, q) = (temp("mod_p"), temp("mod_q"));
    put(&p, "assets/maps/arena.dat", b"map bytes");
    put(&p, "assets/notes.txt", b"plain notes\n");
    put(&p, "secret.dat", b"outside assets");
    let mut v = Vfs::new();
    v.add_pack(Pack::new("arena", &p));
    v.add_pack(Pack::new("other", &q));
    // the archive argument plays no part
    assert_eq!(
        v.entry("anything.rez", "@mod:arena/assets/maps/arena.dat")
            .unwrap(),
        Some(b"map bytes".to_vec())
    );
    assert_eq!(
        v.entry_raw("", "@mod:arena/assets\\maps/./arena.dat")
            .unwrap(),
        Some(b"map bytes".to_vec())
    );
    assert_eq!(
        v.entry_text("x", "@mod:arena/assets/notes.txt").unwrap(),
        Some("plain notes\n".to_string())
    );
    assert_eq!(
        v.entry_head("x", "@mod:arena/assets/maps/arena.dat", 3)
            .unwrap(),
        Some(b"map".to_vec())
    );
    assert_eq!(
        v.resolve_mod("@mod:arena/assets/maps/arena.dat").unwrap(),
        p.join("assets/maps/arena.dat")
    );
    // refused: unknown id, outside assets/, traversal, bare assets/, no file part
    for bad in [
        "@mod:nope/assets/maps/arena.dat",
        "@mod:arena/secret.dat",
        "@mod:arena/assets/../secret.dat",
        "@mod:arena/assets/",
        "@mod:arena",
        "@mod:arena/C:/x",
    ] {
        assert!(
            matches!(v.entry("x", bad), Err(VfsError::ModPath(_))),
            "{bad} must be refused"
        );
    }
    // a pack file that does not exist is an I/O error, never a fall-through to the other layers
    assert!(matches!(
        v.entry("x", "@mod:other/assets/missing.dat"),
        Err(VfsError::Io { .. })
    ));
    for d in [p, q] {
        let _ = fs::remove_dir_all(d);
    }
}

#[test]
fn with_no_pack_a_mod_name_is_a_bad_name_as_before() {
    let v = Vfs::new();
    assert!(matches!(
        v.entry("x", "@mod:arena/assets/a.dat"),
        Err(VfsError::BadName(_))
    ));
    assert!(matches!(
        v.resolve_mod("@mod:arena/assets/a.dat"),
        Err(VfsError::ModPath(_))
    ));
}

#[test]
fn a_profile_with_no_archive_reads_its_folders() {
    let (p, r) = (temp("norom_p"), temp("norom_r"));
    put(&r, "models/box.mdl", b"model");
    put(&r, "tables/rules.txt", b"plain = 1\n");
    put(&r, "tables/locked.txt", &[0xff, 0xfe, 0x01]);
    put(&p, "assets/maps/m.dat", b"world");
    let content = Content {
        packs: PackTable::new([Pack::new("m", &p)]),
        overrides: Vec::new(),
        roots: vec![r.clone()],
    };
    let v = install(&content, None);
    assert!(!v.has_archive());
    assert_eq!(v.archive_root(), None);
    assert_eq!(
        v.entry("any.rez", "Models\\BOX.MDL").unwrap(),
        Some(b"model".to_vec())
    );
    assert_eq!(
        v.entry_text("any.rez", "tables/rules.txt").unwrap(),
        Some("plain = 1\n".to_string())
    );
    assert!(matches!(
        v.entry("any.rez", "tables/locked.txt"),
        Err(VfsError::NoKey(_))
    ));
    assert_eq!(
        v.entry("", "@mod:m/assets/maps/m.dat").unwrap(),
        Some(b"world".to_vec())
    );
    assert_eq!(v.entry("A.rez", "X/BIN.DAT").unwrap(), None);
    assert!(matches!(
        v.list_entries("A.rez"),
        Err(VfsError::ArchiveNotFound(_))
    ));
    for d in [p, r] {
        let _ = fs::remove_dir_all(d);
    }
}

#[test]
fn an_empty_content_installs_the_file_system_of_today() {
    // no archive: the same as Vfs::new()
    let v = install(&Content::default(), None);
    let base = Vfs::new();
    assert!(Content::default().is_empty());
    assert_eq!(v.roots(), base.roots());
    assert_eq!(v.override_dirs(), base.override_dirs());
    assert!(v.packs().is_empty() && !v.has_archive());
    // with an archive: the same as Vfs::with_archive(), answers and negative cache included
    let o = temp("same_o");
    put(&o, "BIN.DAT", b"o");
    let content = Content {
        overrides: vec![o.clone()],
        ..Content::default()
    };
    let a = install(&content, Some(Arc::new(FakeLayer::default())));
    let mut b = Vfs::with_archive(Arc::new(FakeLayer::default()));
    b.add_override_dir(&o);
    for (arch, name) in [
        ("A.rez", "X/BIN.DAT"),
        ("A.rez", "X/HELLO.TXT"),
        ("A.rez", "X/NONE.DAT"),
        ("B.rez", "X/BIN.DAT"),
    ] {
        assert_eq!(
            a.entry(arch, name).unwrap(),
            b.entry(arch, name).unwrap(),
            "{arch} {name}"
        );
        assert_eq!(
            a.is_cached_missing(arch, name, false),
            b.is_cached_missing(arch, name, false)
        );
    }
    assert_eq!(a.override_dirs(), b.override_dirs());
    assert_eq!(a.roots(), b.roots());
    assert!(a.packs().is_empty());
    assert!(matches!(
        a.entry("A.rez", "@mod:x/assets/y"),
        Err(VfsError::BadName(_))
    ));
    let _ = fs::remove_dir_all(o);
}

#[test]
fn asset_paths_are_cleaned_or_refused() {
    assert_eq!(
        clean_asset_path(" assets\\a/./b.dat ").unwrap(),
        "assets/a/b.dat"
    );
    for bad in ["", "  ", "../a", "a/../b", "/a", "\\a", "C:/a", "a\0b", "."] {
        assert!(clean_asset_path(bad).is_err(), "{bad:?}");
    }
    assert!(is_mod_path("@mod:x/assets/y") && !is_mod_path("models/x"));
}

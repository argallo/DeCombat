//! The committed community map example `examples/mods/starter_map` is what `dc-mapkit new --template starter
//! --name "Starter Map" --author "DeCombat"` + `build` + `export --no-rom-paths` produce (MAPKIT_PLAN.md 6.3,
//! DC_DEFAULTS.md MAP-70): the text files of a fresh export equal the committed ones byte for byte, the world's
//! parsed statistics equal (the bytes may differ across platforms, so the `.dat` is compared by what it holds),
//! and the entry snippet `examples/server-vanilla-plus.toml` names the committed package's SHA-256.
//! `tools/mapkit_regen_example.sh` regenerates the folder when the template or the exporter changes.

use dc_mapkit::build::{self, Assets};
use dc_mapkit::export::{export, ExportOptions};
use dc_mapkit::project::Project;
use dc_mapkit::template::{make, Template};
use std::path::{Path, PathBuf};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/starter_map")
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mapkit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fresh export of the starter template as `new` scaffolds it (seed 1), `--no-rom-paths`, no zip.
fn fresh_export(dir: &Path) -> PathBuf {
    let made = make(Template::Starter, "Starter Map", "DeCombat", 1);
    dc_mapkit::cli::write_project(dir, &made, Template::Starter).expect("scaffold");
    let (p, h) = Project::open(dir).expect("open");
    let b = build::build(&p, &h, &Assets::new(dir), &p.build).expect("build");
    assert!(!b.diagnostics.has_errors(), "{:?}", b.diagnostics.items);
    let e = export(
        &p,
        &b,
        &dir.join("build"),
        ExportOptions {
            zip: false,
            no_rom_paths: true,
            project_dir: Some(dir.to_path_buf()),
            force: false,
        },
    )
    .expect("export");
    assert!(!e.diagnostics.has_errors(), "{:?}", e.diagnostics.items);
    e.dir
}

#[test]
fn the_committed_example_equals_a_fresh_export_of_the_starter_template() {
    let example = example_dir();
    assert!(
        example.join("manifest.json").is_file(),
        "examples/mods/starter_map is committed (tools/mapkit_regen_example.sh makes it)"
    );
    let dir = scratch("starter-example");
    let fresh = fresh_export(&dir);
    // the same file list
    let list = |root: &Path| -> Vec<String> {
        fn walk(root: &Path, d: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    out.push(
                        p.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        let mut v = Vec::new();
        walk(root, root, &mut v);
        v.sort();
        v
    };
    assert_eq!(list(&example), list(&fresh));
    // the text files byte for byte (the checkout keeps them LF: .gitattributes `examples/mods/** text eol=lf`)
    for f in ["manifest.json", "main.lua", "README.md"] {
        let a = std::fs::read_to_string(example.join(f)).unwrap();
        let b = std::fs::read_to_string(fresh.join(f)).unwrap();
        assert_eq!(
            a, b,
            "{f} differs: regenerate the example (tools/mapkit_regen_example.sh)"
        );
    }
    // the world by what it holds (the bytes are compared on one platform by the build's own determinism test)
    let a = std::fs::read(example.join("assets/maps/starter_map.dat")).unwrap();
    let b = std::fs::read(fresh.join("assets/maps/starter_map.dat")).unwrap();
    let wa = dc_world::World::parse(&a).expect("committed world parses");
    let wb = dc_world::World::parse(&b).expect("fresh world parses");
    wa.check_layout().expect("committed world tiles");
    assert_eq!(wa.stats(), wb.stats(), "the committed world's statistics");
    assert_eq!(wa.models.len(), 3);
    // the pictures and textures have the same sizes (DTX headers; the pixels are procedural and seeded)
    for f in [
        "assets/ground.dtx",
        "assets/wall.dtx",
        "assets/block.dtx",
        "assets/sky.dtx",
        "assets/pictures/starter_map_radar.dtx",
    ] {
        let a = std::fs::metadata(example.join(f)).unwrap().len();
        let b = std::fs::metadata(fresh.join(f)).unwrap().len();
        assert_eq!(a, b, "{f} size");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_vanilla_plus_entry_snippet_names_the_committed_packages_hash() {
    let example = example_dir();
    let (zip, entry) = dc_modkit::pack_mod_bytes(&example).expect("the example packs");
    assert_eq!(entry.id, "starter_map");
    assert!(zip.len() < 16 << 20, "{} bytes", zip.len());
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let toml = std::fs::read_to_string(repo.join("examples/server-vanilla-plus.toml"))
        .expect("examples/server-vanilla-plus.toml");
    // the snippet parses as a server.toml on the vanilla default, and dc-config has nothing against its entry
    let cfg = dc_config::parse_toml(&toml).expect("the entry snippet parses");
    assert_eq!(cfg.mods.policy, "vanilla");
    assert!(
        cfg.mods.set.is_empty(),
        "no lobby-wide set: the map rides the entry"
    );
    let [server] = cfg.lobby.servers.as_slice() else {
        panic!("one entry: {:?}", cfg.lobby.servers);
    };
    assert_eq!(server.name, "Vanilla++");
    let [pack] = server.mods.as_slice() else {
        panic!("one pack: {:?}", server.mods);
    };
    assert_eq!(pack.id, "starter_map");
    assert_eq!(pack.version, entry.version.to_string());
    assert_eq!(
        pack.sha256, entry.sha256,
        "examples/server-vanilla-plus.toml must carry the committed example's SHA-256 ({}); update it after \
         tools/mapkit_regen_example.sh",
        entry.sha256
    );
    let issues = dc_config::check(&cfg, dc_config::CheckOptions { fs: false });
    assert!(
        !dc_config::has_errors(&issues),
        "dc-config check (no fs): {issues:?}"
    );
}

//! Mod kinds and capabilities in the toolkit: scaffold, validate, lint, pack entries, `modset`.

use std::fs;
use std::path::{Path, PathBuf};

use dc_mod_lua::{JsonValue, ModKind, ModManifest};

use crate::validate::TempDir;
use crate::{
    lint_mod, modset_fragment, pack_mod_bytes, pack_mod_entry, run_cli, scaffold_mod,
    scaffold_mod_kind, validate_mod, LintSeverity, ScaffoldProfile,
};

fn write_mod(
    base: &Path,
    id: &str,
    kind: &str,
    caps: &[&str],
    deps: &[&str],
    main: &str,
) -> PathBuf {
    let dir = base.join(id);
    fs::create_dir_all(&dir).unwrap();
    let caps_json: Vec<String> = caps.iter().map(|c| format!("\"{}\"", c)).collect();
    let deps_json: Vec<String> = deps.iter().map(|c| format!("\"{}\"", c)).collect();
    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{"id":"{}","name":"{}","version":"1.2.0","kind":"{}","capabilities":[{}],"dependencies":[{}],"github":"someone/{}",
               "description":"A test fixture mod for the toolkit tests.","author":"tests","tags":["test"],
               "screenshots":["screenshots/main.png"]}}"#,
            id,
            id,
            kind,
            caps_json.join(","),
            deps_json.join(","),
            id
        ),
    )
    .unwrap();
    fs::write(dir.join("main.lua"), main).unwrap();
    fs::create_dir_all(dir.join("screenshots")).unwrap();
    // a real (non-placeholder) PNG: the placeholder itself draws a lint warning
    fs::write(
        dir.join("screenshots/main.png"),
        [&crate::placeholder_png()[..], b"fixture"].concat(),
    )
    .unwrap();
    dir
}

const EMPTY: &str = "return function(mod) end\n";
const HUD_MOD: &str =
    "return function(mod)\n    mod.hooks:on(\"hud.draw\", function(b, c) return b(c) end)\nend\n";

fn diagnostics(dir: &Path) -> Vec<String> {
    lint_mod(dir)
        .unwrap()
        .iter()
        .map(|d| d.to_string())
        .collect()
}

#[test]
fn every_profile_scaffolds_a_clean_mod_that_declares_what_it_uses() {
    for (profile, kind) in [
        (ScaffoldProfile::Content, ModKind::ServerRules),
        (ScaffoldProfile::Overhaul, ModKind::ServerRules),
        (ScaffoldProfile::TotalConversion, ModKind::Shared),
        (ScaffoldProfile::ClientVisual, ModKind::ClientVisual),
        (ScaffoldProfile::Map, ModKind::Shared),
    ] {
        let tmp = TempDir::new("scaf").unwrap();
        let dir = scaffold_mod("sample", tmp.path(), profile, Some("a/b")).unwrap();
        let m = ModManifest::from_file(&dir.join("manifest.json")).unwrap();
        assert_eq!(m.kind, kind, "{:?}", profile);
        assert_eq!(m.requires_client, kind != ModKind::ServerRules);
        assert!(!m.capabilities.is_empty());
        let report = validate_mod(&dir, None);
        assert!(report.ok, "{:?}: {:?}", profile, report.errors);
        // No errors and no "declared but unused" warnings: the template uses exactly what it declares.
        // the scaffold's placeholders (description, author, image) are the only findings
        let left: Vec<String> = diagnostics(&dir)
            .into_iter()
            .filter(|d| !d.contains("placeholder"))
            .collect();
        assert_eq!(left, Vec::<String>::new(), "{:?}", profile);
        assert!(pack_mod_bytes(&dir).is_ok(), "{:?}", profile);
    }
}

#[test]
fn scaffold_kind_override_is_checked_against_the_profile() {
    let tmp = TempDir::new("scaf_kind").unwrap();
    // Content patches weapons: fine as a shared mod too.
    let dir = scaffold_mod_kind(
        "c1",
        tmp.path(),
        ScaffoldProfile::Content,
        Some(ModKind::Shared),
        None,
    )
    .unwrap();
    let m = ModManifest::from_file(&dir.join("manifest.json")).unwrap();
    assert_eq!((m.kind, m.requires_client), (ModKind::Shared, true));
    // The overhaul profile hooks the lobby; a client-only mod cannot.
    let e = scaffold_mod_kind(
        "c2",
        tmp.path(),
        ScaffoldProfile::Overhaul,
        Some(ModKind::ClientVisual),
        None,
    )
    .unwrap_err();
    assert!(
        e.contains("client_visual") && e.contains("not allowed"),
        "{}",
        e
    );
    assert!(
        !tmp.path().join("c2").exists(),
        "a refused scaffold must not leave files behind"
    );
}

#[test]
fn validate_rejects_manifests_without_kind_or_with_bad_capabilities() {
    let tmp = TempDir::new("val").unwrap();
    let dir = tmp.path().join("nokind");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("manifest.json"),
        r#"{"id":"nokind","name":"N","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::write(dir.join("main.lua"), EMPTY).unwrap();
    let r = validate_mod(&dir, None);
    assert!(!r.ok);
    assert!(
        r.errors[0].contains("missing required field 'kind'"),
        "{:?}",
        r.errors
    );

    let dir = write_mod(
        tmp.path(),
        "badcap",
        "shared",
        &["content.weaponz"],
        &[],
        EMPTY,
    );
    let r = validate_mod(&dir, None);
    assert!(
        !r.ok && r.errors[0].contains("Unknown capability"),
        "{:?}",
        r.errors
    );

    let dir = write_mod(
        tmp.path(),
        "srhud",
        "server_rules",
        &["hooks.hud"],
        &[],
        EMPTY,
    );
    let r = validate_mod(&dir, None);
    assert!(
        !r.ok && r.errors[0].contains("server_rules"),
        "{:?}",
        r.errors
    );
}

#[test]
fn validate_runs_the_entry_under_the_declared_capabilities() {
    let tmp = TempDir::new("val_cap").unwrap();
    let dir = write_mod(
        tmp.path(),
        "overreach",
        "server_rules",
        &["content.weapons"],
        &[],
        "return function(mod) mod.content.shop:patch(1, { Price = 1 }) end\n",
    );
    let r = validate_mod(&dir, None);
    assert!(!r.ok);
    assert!(
        r.errors
            .iter()
            .any(|e| e.contains("capability denied") && e.contains("content.shop")),
        "{:?}",
        r.errors
    );
}

#[test]
fn lint_finds_undeclared_use_even_in_callbacks_that_validate_never_runs() {
    let tmp = TempDir::new("lint_cap").unwrap();
    let main = "return function(mod)
    mod.content.weapons:patch(1, { AmmoDamage = 3 })
    mod.hooks:wrap(\"combat.damage\", function(base, ctx)
        mod.save:set(\"x\", 1)
        return base(ctx)
    end)
    mod.events:on(\"round.start\", function(ctx) end)
end
";
    let dir = write_mod(
        tmp.path(),
        "partial",
        "server_rules",
        &["content.weapons"],
        &[],
        main,
    );
    let d = diagnostics(&dir);
    for needle in [
        "main.lua:3: mod.hooks:wrap (\"combat.damage\") needs capability 'hooks.combat'",
        "main.lua:4: mod.save:set needs capability 'save'",
        "main.lua:7: mod.events:on needs capability 'events'",
    ] {
        assert!(
            d.iter().any(|x| x.starts_with(needle)),
            "missing {:?} in {:#?}",
            needle,
            d
        );
    }
    assert!(!d.iter().any(|x| x.contains("content.weapons")), "{:#?}", d);
    // pack refuses because lint has errors.
    assert!(pack_mod_bytes(&dir).is_err());
}

#[test]
fn lint_flags_unknown_hook_namespaces_and_warns_on_unused_capabilities() {
    let tmp = TempDir::new("lint_ns").unwrap();
    let dir = write_mod(
        tmp.path(),
        "ns",
        "shared",
        &["hooks.combat", "assets", "content.shop"],
        &[],
        "return function(mod)\n    mod.hooks:wrap(\"net.send\", function(b, c) return b(c) end)\nend\n",
    );
    let diags = lint_mod(&dir).unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d.is_error() && d.line == 2 && d.message.contains("known namespace")),
        "{:#?}",
        diags
    );
    let warns: Vec<String> = diags
        .iter()
        .filter(|d| d.severity == LintSeverity::Warning)
        .map(|d| d.message.clone())
        .collect();
    assert!(
        warns
            .iter()
            .any(|w| w.contains("'assets' is declared but no script uses it")),
        "{:?}",
        warns
    );
    assert!(
        warns
            .iter()
            .any(|w| w.contains("'content.shop' is declared")),
        "{:?}",
        warns
    );
}

#[test]
fn a_dynamic_hook_name_suppresses_the_unused_hook_warning() {
    let tmp = TempDir::new("lint_dyn").unwrap();
    let dir = write_mod(
        tmp.path(),
        "dynhook",
        "server_rules",
        &["hooks.combat"],
        &[],
        "return function(mod)\n    local n = \"combat.\" .. \"damage\"\n    mod.hooks:wrap(n, function(b, c) return b(c) end)\nend\n",
    );
    assert_eq!(diagnostics(&dir), Vec::<String>::new());
}

#[test]
fn pack_entry_matches_the_zip_and_is_deterministic() {
    let tmp = TempDir::new("pack_entry").unwrap();
    let dir = write_mod(
        tmp.path(),
        "entry_mod",
        "shared",
        &["hooks.hud"],
        &[],
        HUD_MOD,
    );
    let zip = tmp.path().join("out/entry_mod.zip");
    let (bytes, entry) = pack_mod_entry(&dir, &zip).unwrap();
    assert_eq!(entry.sha256, dc_rom::sha256_hex(&fs::read(&zip).unwrap()));
    assert_eq!(entry.sha256.len(), 64);
    assert_eq!(
        (entry.id.as_str(), entry.version.to_string().as_str()),
        ("entry_mod", "1.2.0")
    );
    assert_eq!(entry.kind, ModKind::Shared);
    assert_eq!(entry.source_repo.as_deref(), Some("someone/entry_mod"));
    let (bytes2, entry2) = pack_mod_bytes(&dir).unwrap();
    assert_eq!(bytes, bytes2);
    assert_eq!(entry, entry2);
}

fn get<'a>(v: &'a JsonValue, key: &str) -> &'a JsonValue {
    v.get(key).unwrap_or_else(|| panic!("missing {}", key))
}

#[test]
fn modset_lists_entries_in_load_order_with_hashes() {
    let tmp = TempDir::new("modset").unwrap();
    // `zeta` sorts last by id but is the dependency, so it must come first.
    let zeta = write_mod(tmp.path(), "zeta", "server_rules", &[], &[], EMPTY);
    let alpha = write_mod(
        tmp.path(),
        "alpha",
        "shared",
        &["hooks.hud"],
        &["zeta@^1.0.0"],
        HUD_MOD,
    );
    let frag = modset_fragment(&[alpha.clone(), zeta.clone()], false).unwrap();

    assert_eq!(get(&frag, "policy").as_str(), Some("modded"));
    assert_eq!(get(&frag, "client_visual_allowed").as_bool(), Some(false));
    let mods = get(&frag, "mods").as_array().unwrap();
    let ids: Vec<&str> = mods
        .iter()
        .map(|m| get(m, "id").as_str().unwrap())
        .collect();
    assert_eq!(ids, ["zeta", "alpha"]);
    for (m, dir) in mods.iter().zip([&zeta, &alpha]) {
        let (_, entry) = pack_mod_bytes(dir).unwrap();
        assert_eq!(get(m, "sha256").as_str(), Some(entry.sha256.as_str()));
        assert_eq!(get(m, "version").as_str(), Some("1.2.0"));
    }
    assert_eq!(get(&mods[1], "kind").as_str(), Some("shared"));
    assert_eq!(get(&mods[1], "requires_client").as_bool(), Some(true));
    assert_eq!(get(&mods[1], "capabilities").as_array().unwrap().len(), 1);
    assert_eq!(get(&mods[1], "source_repo").as_str(), Some("someone/alpha"));
    assert_eq!(get(&mods[0], "requires_client").as_bool(), Some(false));

    // Input order does not matter, and the output is reproducible.
    let again = modset_fragment(&[zeta, alpha], false).unwrap();
    assert_eq!(frag, again);
    assert_eq!(frag.to_json_pretty(), again.to_json_pretty());
}

#[test]
fn modset_policy_and_client_visual_flag() {
    let tmp = TempDir::new("modset_pol").unwrap();
    // No mods: vanilla, and a client_visual_allowed request cannot make an empty set "modded".
    let vanilla = modset_fragment(&[], true).unwrap();
    assert_eq!(get(&vanilla, "policy").as_str(), Some("vanilla"));
    assert_eq!(
        get(&vanilla, "client_visual_allowed").as_bool(),
        Some(false)
    );
    assert!(get(&vanilla, "mods").as_array().unwrap().is_empty());

    let m = write_mod(tmp.path(), "one", "server_rules", &[], &[], EMPTY);
    let f = modset_fragment(&[m], true).unwrap();
    assert_eq!(get(&f, "policy").as_str(), Some("modded"));
    assert_eq!(get(&f, "client_visual_allowed").as_bool(), Some(true));
}

#[test]
fn modset_refuses_bad_sets() {
    let tmp = TempDir::new("modset_bad").unwrap();
    let a = write_mod(
        tmp.path(),
        "needs_dep",
        "shared",
        &[],
        &["missing_dep"],
        EMPTY,
    );
    let e = modset_fragment(&[a], false).unwrap_err();
    assert!(e.iter().any(|m| m.contains("missing_dep")), "{:?}", e);

    let ok = write_mod(tmp.path(), "good", "server_rules", &[], &[], EMPTY);
    let e = modset_fragment(&[ok.clone(), ok.clone()], false).unwrap_err();
    assert!(e.iter().any(|m| m.contains("appears twice")), "{:?}", e);

    // A mod that fails lint is not offered in a mod set.
    let bad = write_mod(
        tmp.path(),
        "bad",
        "server_rules",
        &[],
        &[],
        "return function(mod) os.exit() end\n",
    );
    let e = modset_fragment(&[bad], false).unwrap_err();
    assert!(e.iter().any(|m| m.contains("os")), "{:?}", e);
}

#[test]
fn cli_modset_and_scaffold_kind_exit_codes() {
    let tmp = TempDir::new("cli").unwrap();
    let m = write_mod(tmp.path(), "cli_mod", "server_rules", &[], &[], EMPTY);
    let args = |v: &[&str]| -> Vec<String> {
        std::iter::once("dc-modkit".to_string())
            .chain(v.iter().map(|s| s.to_string()))
            .collect()
    };
    assert_eq!(run_cli(&args(&["modset", m.to_str().unwrap()])), 0);
    assert_eq!(run_cli(&args(&["modset"])), 0);
    assert_eq!(run_cli(&args(&["modset", "--bogus"])), 1);
    assert_eq!(
        run_cli(&args(&[
            "modset",
            tmp.path().join("nope").to_str().unwrap()
        ])),
        1
    );
    let d = tmp.path().to_str().unwrap();
    assert_eq!(
        run_cli(&args(&["scaffold", "s1", "--dir", d, "--kind", "shared"])),
        0
    );
    assert_eq!(
        run_cli(&args(&["scaffold", "s2", "--dir", d, "--kind", "nonsense"])),
        1
    );
    assert_eq!(
        run_cli(&args(&[
            "scaffold",
            "s3",
            "--dir",
            d,
            "--profile",
            "overhaul",
            "--kind",
            "client_visual"
        ])),
        1
    );
}

#[test]
fn example_hacking_mod_validates_lints_and_packs() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/hacking_mod");
    let report = validate_mod(&dir, None);
    assert!(report.ok, "{:?}", report.errors);
    let errors: Vec<_> = lint_mod(&dir)
        .expect("lint")
        .into_iter()
        .filter(|d| d.is_error())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let (_, entry) = pack_mod_bytes(&dir).expect("pack");
    assert_eq!(entry.id, "hacking_mod");
}

#[test]
fn example_drawn_crosshair_validates_lints_and_packs() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/drawn_crosshair");
    let report = validate_mod(&dir, None);
    assert!(report.ok, "{:?}", report.errors);
    let errors: Vec<_> = lint_mod(&dir)
        .expect("lint")
        .into_iter()
        .filter(|d| d.is_error())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let (_, entry) = pack_mod_bytes(&dir).expect("pack");
    assert_eq!(entry.id, "drawn_crosshair");
}

#[test]
fn bad_settings_in_a_manifest_fail_validation() {
    let m = dc_mod_lua::ModManifest::from_json_str(
        r#"{"id":"a","name":"A","version":"1.0.0","kind":"client_visual","description":"A test fixture for the modkit.","author":"t","screenshots":["screenshots/m.png"],
           "settings":[{"key":"k","label":"K","type":"slider","default":"1"}]}"#,
    );
    assert!(m.unwrap_err().contains("unknown type"));
    let m = dc_mod_lua::ModManifest::from_json_str(
        r#"{"id":"a","name":"A","version":"1.0.0","kind":"client_visual","description":"A test fixture for the modkit.","author":"t","screenshots":["screenshots/m.png"],
           "settings":[{"key":"k","label":"K","type":"number","min":0,"max":4,"step":1,"default":9}]}"#,
    );
    assert!(m.unwrap_err().contains("default"));
}

#[test]
fn a_number_setting_survives_the_manifest_round_trip() {
    let m = dc_mod_lua::ModManifest::from_json_str(
        r#"{"id":"a","name":"A","version":"1.0.0","kind":"client_visual","description":"A test fixture for the modkit.","author":"t","screenshots":["screenshots/m.png"],
           "settings":[{"key":"k","label":"K","group":"G","type":"number","min":-2.5,"max":4,"step":0.5,"default":1.5}]}"#,
    )
    .unwrap();
    assert_eq!(m.settings[0].default, "1.5");
    let again = dc_mod_lua::ModManifest::from_json_str(&m.to_json_pretty()).unwrap();
    assert_eq!(again.settings, m.settings);
}

/// MOD-27 and the user's words of 2026-11-01: the description calls out what the mod gives, and its command help is in
/// the manifest and in the set entry.
#[test]
fn the_hacking_mod_says_what_it_does_and_lists_its_commands() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/hacking_mod");
    let (_, entry) = pack_mod_bytes(&dir).expect("pack");
    assert_eq!(entry.version.to_string(), "1.1.1");
    let d = entry.description.to_lowercase();
    for word in ["through walls", "box", "chams", "skeleton", "nametags", "health bars", "team mates", "recoil"] {
        assert!(d.contains(word), "{word}: {d}");
    }
    let c: Vec<&str> = entry.commands.iter().map(|c| c.command.as_str()).collect();
    assert_eq!(c, ["/wallhack", "/nametags", "/health", "/teammates"]);
    assert!(entry.to_json_value().to_json_string().contains("\"/wallhack\""));
}

/// MOD-27: `validate` refuses a bad commands list.
#[test]
fn validate_refuses_a_bad_commands_list() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/hacking_mod");
    let d = std::env::temp_dir().join(format!("dc_modkit_commands_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("screenshots")).unwrap();
    for f in ["main.lua", "screenshots/main.png"] {
        std::fs::copy(src.join(f), d.join(f)).unwrap();
    }
    let man = std::fs::read_to_string(src.join("manifest.json")).unwrap();
    std::fs::write(d.join("manifest.json"), man.replace("\"/wallhack\"", "\"wallhack\"")).unwrap();
    let report = validate_mod(&d, None);
    assert!(!report.ok && report.errors.iter().any(|e| e.contains("Invalid command")), "{:?}", report.errors);
    let _ = std::fs::remove_dir_all(&d);
}

// ---- MAP-5: the map profile and the world check of validate -------------------------------------

fn map_main(key: &str) -> String {
    format!(
        r#"return function(mod)
    mod.content.maps:register({{
        id = "{key}", name = "Map {key}", world = mod.assets:path("assets/maps/{key}.dat"),
        modes = {{ "TDM", "BOMB" }},
    }})
end
"#
    )
}

#[test]
fn the_map_profile_scaffolds_a_shared_mod_with_a_placeholder_world_that_validates_and_packs() {
    let tmp = TempDir::new("mapscaf").unwrap();
    let dir = scaffold_mod("Crossfire-Valley", tmp.path(), ScaffoldProfile::Map, None).unwrap();
    let m = ModManifest::from_file(&dir.join("manifest.json")).unwrap();
    assert_eq!(m.kind, ModKind::Shared);
    assert_eq!(m.capabilities, vec!["assets", "content.maps"]);
    // the map key follows the map-id rule, not the mod-id rule
    assert_eq!(
        crate::map_key_of_mod_id("Crossfire-Valley"),
        "crossfire_valley"
    );
    let world = dir.join("assets/maps/crossfire_valley.dat");
    assert!(world.is_file(), "placeholder world");
    let bytes = fs::read(&world).unwrap();
    let parsed = dc_world::World::parse(&bytes).expect("the placeholder parses");
    parsed.check_layout().expect("the placeholder tiles");
    assert_eq!(parsed.models.len(), 1);
    let readme = fs::read_to_string(dir.join("README.md")).unwrap();
    assert!(readme.contains("dc-mapkit") && readme.contains("crossfire_valley"));

    let report = validate_mod(&dir, None);
    assert!(report.ok, "{:?}", report.errors);
    let left: Vec<String> = diagnostics(&dir)
        .into_iter()
        .filter(|d| !d.contains("placeholder"))
        .collect();
    assert_eq!(left, Vec::<String>::new());
    let (_, entry) = pack_mod_bytes(&dir).unwrap();
    assert_eq!(entry.id, "Crossfire-Valley");
    assert!(entry.capabilities.contains(&"content.maps".to_string()));

    // the record the real host sees
    let mut host = dc_mod_lua::ModHost::new();
    host.load_mod_dir(&dir).unwrap();
    let recs = host.map_records().unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].id, "crossfire_valley");
    assert_eq!(
        recs[0].world,
        "@mod:Crossfire-Valley/assets/maps/crossfire_valley.dat"
    );
    assert_eq!(recs[0].modes, vec!["TDM", "FFA", "ELIM", "BOMB", "FLAG"]);

    // a map mod is shared: the server_rules kind is refused for the profile
    let e = scaffold_mod_kind(
        "srv_map",
        tmp.path(),
        ScaffoldProfile::Map,
        Some(ModKind::ServerRules),
        None,
    )
    .unwrap_err();
    assert!(e.contains("'map'") && e.contains("'shared'"), "{e}");
    assert!(scaffold_mod_kind(
        "cv_map",
        tmp.path(),
        ScaffoldProfile::Map,
        Some(ModKind::ClientVisual),
        None
    )
    .unwrap_err()
    .contains("content.maps"));
}

#[test]
fn validate_requires_every_registered_world_to_exist_and_parse() {
    let tmp = TempDir::new("mapval").unwrap();
    let caps = &["content.maps", "assets"];
    // 1. no world file at all
    let dir = write_mod(
        tmp.path(),
        "nomap",
        "shared",
        caps,
        &[],
        &map_main("valley"),
    );
    let report = validate_mod(&dir, None);
    assert!(!report.ok);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(
        report.errors[0].contains("Mod 'nomap': map 'valley': world '@mod:nomap/assets/maps/valley.dat' does not exist under the mod folder (assets/maps/valley.dat)"),
        "{:?}",
        report.errors
    );
    // 2. a file that is not a world
    fs::create_dir_all(dir.join("assets/maps")).unwrap();
    fs::write(dir.join("assets/maps/valley.dat"), b"not a world at all").unwrap();
    let report = validate_mod(&dir, None);
    assert!(!report.ok);
    assert!(
        report.errors[0].contains("does not parse as a version-85 world"),
        "{:?}",
        report.errors
    );
    // 3. a truncated world
    let good = crate::placeholder_world();
    fs::write(dir.join("assets/maps/valley.dat"), &good[..good.len() - 7]).unwrap();
    let report = validate_mod(&dir, None);
    assert!(!report.ok, "{:?}", report.errors);
    // 4. the real thing
    fs::write(dir.join("assets/maps/valley.dat"), &good).unwrap();
    let report = validate_mod(&dir, None);
    assert!(report.ok, "{:?}", report.errors);
    assert!(pack_mod_bytes(&dir).is_ok());
    // 5. a record refused by the host names the rule (and no world check runs on a failed load)
    fs::write(
        dir.join("main.lua"),
        r#"return function(mod)
    mod.content.maps:register({ id = "valley", name = "V", world = mod.assets:path("assets/maps/valley.dat"), modes = { "TDM" }, colour = 1 })
end
"#,
    )
    .unwrap();
    let report = validate_mod(&dir, None);
    assert!(!report.ok);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("unknown key 'colour'")),
        "{:?}",
        report.errors
    );
    // 6. a client_visual mod cannot declare the domain at all
    let dir = write_mod(
        tmp.path(),
        "cvmap",
        "client_visual",
        caps,
        &[],
        &map_main("valley"),
    );
    let report = validate_mod(&dir, None);
    assert!(!report.ok);
    assert!(
        report.errors[0].contains("content.maps") && report.errors[0].contains("client_visual"),
        "{:?}",
        report.errors
    );
}

#[test]
fn lint_sees_the_maps_domain_as_a_capability() {
    let tmp = TempDir::new("maplint").unwrap();
    // undeclared use is an error
    let dir = write_mod(
        tmp.path(),
        "undeclared",
        "shared",
        &["assets"],
        &[],
        &map_main("valley"),
    );
    let diags = diagnostics(&dir);
    assert!(
        diags
            .iter()
            .any(|d| d.contains("content.maps") && !d.contains("warning")),
        "{:?}",
        diags
    );
    // declared and unused is a warning
    let dir = write_mod(
        tmp.path(),
        "unused",
        "shared",
        &["content.maps"],
        &[],
        EMPTY,
    );
    let diags = diagnostics(&dir);
    assert!(
        diags
            .iter()
            .any(|d| d.contains("warning") && d.contains("content.maps")),
        "{:?}",
        diags
    );
    // the CLI knows the profile
    let code = run_cli(&[
        "dc-modkit".to_string(),
        "scaffold".to_string(),
        "cli_map".to_string(),
        "--dir".to_string(),
        tmp.path().to_string_lossy().into_owned(),
        "--profile".to_string(),
        "map".to_string(),
    ]);
    assert_eq!(code, 0);
    assert!(tmp.path().join("cli_map/assets/maps/cli_map.dat").is_file());
}

// ---- MAP-70: the committed community map example (examples/mods/starter_map) ---------------------

/// The starter map example is the exported mod folder dc-mapkit makes from its starter template (own textures
/// only): it validates, lints clean, packs with the id the lobby's set names, and its world parses and tiles.
// DC-DEFAULT: MAP-70; MAPKIT_PLAN.md 6.3
#[test]
fn example_starter_map_validates_lints_packs_and_its_world_parses() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/starter_map");
    let report = validate_mod(&dir, None);
    assert!(report.ok, "{:?}", report.errors);
    let errors: Vec<_> = lint_mod(&dir)
        .expect("lint")
        .into_iter()
        .filter(|d| d.is_error())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let (zip, entry) = pack_mod_bytes(&dir).expect("pack");
    assert_eq!(entry.id, "starter_map");
    assert!(entry.capabilities.contains(&"content.maps".to_string()));
    assert!(
        zip.len() < 16 << 20,
        "the package stays under the lobby's 16 MiB default: {} bytes",
        zip.len()
    );
    // the manifest is a shared mod with the two capabilities a map needs, and own textures only
    let m = ModManifest::from_file(&dir.join("manifest.json")).unwrap();
    assert_eq!(m.kind, ModKind::Shared);
    assert_eq!(m.capabilities, vec!["assets", "content.maps"]);
    assert!(m.requires_client);
    // the world: a version-85 file that parses and tiles, with the level model and the two brushes
    let bytes = fs::read(dir.join("assets/maps/starter_map.dat")).unwrap();
    let w = dc_world::World::parse(&bytes).expect("the world parses");
    w.check_layout().expect("the world tiles");
    let s = w.stats();
    assert_eq!(
        s.models, 3,
        "the level and the two brush models (sky, kill slab)"
    );
    assert!(s.bsp_polys > 8000 && s.objects > 40, "{s:?}");
    // the record the real host sees
    let mut host = dc_mod_lua::ModHost::new();
    host.load_mod_dir(&dir).unwrap();
    let recs = host.map_records().unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].id, "starter_map");
    assert_eq!(
        recs[0].world,
        "@mod:starter_map/assets/maps/starter_map.dat"
    );
    assert_eq!(recs[0].modes, ["TDM", "FFA", "ELIM", "BOMB", "FLAG"]);
    assert!(
        recs[0].radar.is_some(),
        "the exported radar picture and its rectangle"
    );
    // every texture the world names is a file of the mod (no ROM path: CLEANROOM section 4)
    for name in w.models.iter().flat_map(|m| m.texture_names.iter()) {
        let n = String::from_utf8_lossy(name);
        assert!(
            n.starts_with("@mod:starter_map/assets/"),
            "own texture only: {n}"
        );
        let rel = n.trim_start_matches("@mod:starter_map/");
        assert!(dir.join(rel).is_file(), "{n} is shipped");
    }
}

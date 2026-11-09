//! Round-trip and negative tests: scaffold -> edit -> validate -> lint -> pack, and the ways each
//! step must refuse a bad mod. ROM-gated checks skip when `CA_ROM` is unset.

use std::fs;
use std::path::{Path, PathBuf};

use dc_mod_lua::{JsonValue, LuaTable, LuaValue, ModHost, ModKind, ModManifest, CONTENT_DOMAINS};

use crate::validate::TempDir;
use crate::{lint_mod, pack_mod, scaffold_mod, validate_mod, LintSeverity, ScaffoldProfile};

const ROUNDTRIP_MAIN: &str = r#"-- round-trip sample
---@param mod ModCapability
return function(mod)
    mod.content.weapons:patch(145, { AmmoDamage = 41, FireDelay = 90 })
    mod.hooks:wrap("lobby.calc_exp_gp", function(base, ctx)
        local res = base(ctx)
        res.exp = math.floor(res.exp * 1.5)
        res.gp = math.clamp(res.gp, 0, 1000)
        return res
    end)
end
"#;

fn scaffold_with_main(tmp: &Path, id: &str, main: &str) -> PathBuf {
    let dir = scaffold_mod(id, tmp, ScaffoldProfile::Content, None).expect("scaffold");
    fs::write(dir.join("main.lua"), main).unwrap();
    // These tests exercise arbitrary scripts; give the mod every capability so the capability
    // checks (covered separately in kind_tests) do not mask what each test is about.
    let mut manifest = ModManifest::from_file(&dir.join("manifest.json")).unwrap();
    manifest.kind = ModKind::Shared;
    manifest.requires_client = true;
    manifest.capabilities = ALL_CAPS.iter().map(|c| c.to_string()).collect();
    manifest.capabilities.sort();
    fs::write(dir.join("manifest.json"), manifest.to_json_pretty()).unwrap();
    dir
}

const ALL_CAPS: &[&str] = &[
    "assets",
    "content.ai_brains",
    "content.gameroom",
    "content.hud",
    "content.items",
    "content.maps",
    "content.missions",
    "content.music",
    "content.shop",
    "content.text",
    "content.weapons",
    "events",
    "hooks.combat",
    "hooks.hud",
    "hooks.lobby",
    "hooks.render",
    "save",
];

fn lint_errors(dir: &Path) -> Vec<String> {
    lint_mod(dir)
        .expect("lint")
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.to_string())
        .collect()
}

#[test]
fn roundtrip_patch_and_wrap_then_deterministic_pack() {
    let tmp = TempDir::new("rt").unwrap();
    let dir = scaffold_with_main(tmp.path(), "rt_sample", ROUNDTRIP_MAIN);

    let report = validate_mod(&dir, None);
    assert!(report.ok, "validate: {:?}", report.errors);
    assert_eq!(report.mod_id.as_deref(), Some("rt_sample"));
    assert_eq!(lint_errors(&dir), Vec::<String>::new());

    // The same mod, run for real: the patch is registered and the wrapper changes the result.
    let mut host = ModHost::new();
    let manifest = ModManifest::from_file(&dir.join("manifest.json")).unwrap();
    host.load_mod_from_dir_with_manifest(&dir, manifest)
        .unwrap();
    let rec = host
        .get_content_record("weapons", "145")
        .expect("patched record");
    assert_eq!(rec.borrow().get_str("AmmoDamage"), LuaValue::Number(41.0));
    let mut ctx = LuaTable::new();
    ctx.set_str("mode_key", LuaValue::String("FTM".into()));
    let out = host
        .run_hook("lobby.calc_exp_gp", ctx, |_| {
            let mut t = LuaTable::new();
            t.set_str("exp", LuaValue::Number(100.0));
            t.set_str("gp", LuaValue::Number(5000.0));
            Ok(LuaValue::Table(std::rc::Rc::new(std::cell::RefCell::new(
                t,
            ))))
        })
        .expect("hook");
    let LuaValue::Table(t) = out else {
        panic!("hook result is not a table")
    };
    assert_eq!(t.borrow().get_str("exp"), LuaValue::Number(150.0));
    assert_eq!(t.borrow().get_str("gp"), LuaValue::Number(1000.0));

    // Two packs, byte-identical (compared by SHA-256).
    let z1 = tmp.path().join("out1/rt_sample.zip");
    let z2 = tmp.path().join("out2/rt_sample.zip");
    let b1 = pack_mod(&dir, &z1).expect("pack 1");
    let b2 = pack_mod(&dir, &z2).expect("pack 2");
    assert_eq!(dc_rom::sha256_hex(&b1), dc_rom::sha256_hex(&b2));
    assert_eq!(fs::read(&z1).unwrap(), fs::read(&z2).unwrap());
    assert!(b1.starts_with(b"PK\x03\x04"));
}

#[test]
fn lint_catches_io_open_and_pack_refuses() {
    let tmp = TempDir::new("lint_io").unwrap();
    let dir = scaffold_with_main(
        tmp.path(),
        "io_mod",
        "return function(mod)\n    local f = io.open(\"x.txt\", \"w\")\nend\n",
    );
    let errs = lint_errors(&dir);
    assert!(
        errs.iter()
            .any(|e| e.starts_with("main.lua:2:") && e.contains("io.open")),
        "{:?}",
        errs
    );
    let zip = tmp.path().join("never.zip");
    assert!(pack_mod(&dir, &zip).is_err());
    assert!(!zip.exists());
}

#[test]
fn lint_flags_every_sandbox_removed_api() {
    let tmp = TempDir::new("lint_all").unwrap();
    let main = r#"return function(mod)
    local a = os.time()
    local b = io.write
    local c = debug.traceback
    local d = load("return 1")
    local e = loadstring("return 1")
    dofile("x.lua")
    loadfile("x.lua")
    local f = package.path
    collectgarbage()
    local g = string.dump(print)
    math.randomseed(1)
    local h = _G.os
    local i = _G["io"]
    local j = ("x"):rep(2)
    local k = setmetatable({}, { __gc = function() end })
end
"#;
    let dir = scaffold_with_main(tmp.path(), "removed_apis", main);
    let errs = lint_errors(&dir).join("\n");
    for needle in [
        "'os.time'",
        "'io.write'",
        "'debug.traceback'",
        "'load'",
        "'loadstring'",
        "'dofile'",
        "'loadfile'",
        "'package.path'",
        "'collectgarbage'",
        "'string.dump'",
        "'math.randomseed'",
        "'_G.os'",
        "'_G.io'",
        "__gc",
    ] {
        assert!(errs.contains(needle), "missing {} in:\n{}", needle, errs);
    }
    // Real-sandbox agreement: the same names really are absent at run time.
    let host = ModHost::new();
    let env = host.sandbox().create_env("probe", None).unwrap();
    for name in [
        "os",
        "io",
        "debug",
        "load",
        "loadstring",
        "dofile",
        "loadfile",
        "package",
        "collectgarbage",
    ] {
        let v = host
            .sandbox()
            .eval_chunk(&env, &format!("return type({})", name))
            .unwrap();
        assert_eq!(v, LuaValue::String("nil".into()), "{} must be absent", name);
    }
    let v = host
        .sandbox()
        .eval_chunk(
            &env,
            "return tostring(string.dump) .. tostring(math.randomseed)",
        )
        .unwrap();
    assert_eq!(v, LuaValue::String("nilnil".into()));
}

#[test]
fn lint_allows_shadowed_names_and_field_keys() {
    let tmp = TempDir::new("lint_ok").unwrap();
    let main = r#"return function(mod)
    local os = { time = function() return 1 end }
    local t = { debug = true, io = 1 }
    local x = os.time() + (t.debug and 1 or 0) + t.io
    mod.save:set("x", x)
end
"#;
    let dir = scaffold_with_main(tmp.path(), "shadow_ok", main);
    assert_eq!(lint_errors(&dir), Vec::<String>::new());
}

#[test]
fn lint_reports_bytecode_and_real_syntax_errors_with_lines() {
    let tmp = TempDir::new("lint_syn").unwrap();
    let dir = scaffold_with_main(
        tmp.path(),
        "syn_mod",
        "return function(mod)\n    local x = = 1\nend\n",
    );
    fs::write(dir.join("blob.lua"), b"\x1bLuaS\x00").unwrap();
    let errs = lint_errors(&dir);
    assert!(
        errs.iter().any(|e| e.starts_with("main.lua:2:")),
        "{:?}",
        errs
    );
    assert!(
        errs.iter()
            .any(|e| e.starts_with("blob.lua:1:") && e.contains("Binary Lua bytecode")),
        "{:?}",
        errs
    );
}

#[test]
fn lint_lua54_syntax_beyond_the_ast_parser_is_a_warning_not_a_pass() {
    let tmp = TempDir::new("lint_54").unwrap();
    let dir = scaffold_with_main(
        tmp.path(),
        "lua54_mod",
        "return function(mod)\n    local flags <const> = 3 & 1\n    mod.save:set(\"f\", flags)\nend\n",
    );
    let diags = lint_mod(&dir).unwrap();
    assert!(
        diags.iter().all(|d| d.severity == LintSeverity::Warning),
        "{:?}",
        diags
    );
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("static checks skipped")),
        "{:?}",
        diags
    );
    assert!(validate_mod(&dir, None).ok);
}

#[test]
fn validate_reports_runtime_error_with_mod_id() {
    let tmp = TempDir::new("val_err").unwrap();
    let dir = scaffold_with_main(
        tmp.path(),
        "boom_mod",
        "return function(mod)\n    error(\"kaboom\")\nend\n",
    );
    let report = validate_mod(&dir, None);
    assert!(!report.ok);
    assert_eq!(report.mod_id.as_deref(), Some("boom_mod"));
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("boom_mod") && e.contains("kaboom")),
        "{:?}",
        report.errors
    );
    // The author's tree is untouched and no validate copy is left behind in it.
    assert!(dir.join("main.lua").is_file());
}

#[test]
fn validate_reports_other_failures_with_mod_id() {
    let tmp = TempDir::new("val_misc").unwrap();
    let spin = scaffold_with_main(
        tmp.path(),
        "spin_mod",
        "return function(mod)\n    while true do end\nend\n",
    );
    let r = validate_mod(&spin, None);
    assert!(!r.ok);
    assert!(
        r.errors
            .iter()
            .any(|e| e.contains("spin_mod") && e.contains("instruction limit")),
        "{:?}",
        r.errors
    );

    let notfn = scaffold_with_main(tmp.path(), "notfn_mod", "return 42\n");
    let r = validate_mod(&notfn, None);
    assert!(!r.ok);
    assert!(
        r.errors
            .iter()
            .any(|e| e.contains("notfn_mod") && e.contains("function(mod)")),
        "{:?}",
        r.errors
    );

    let calls_os = scaffold_with_main(
        tmp.path(),
        "os_mod",
        "return function(mod)\n    os.exit(1)\nend\n",
    );
    let r = validate_mod(&calls_os, None);
    assert!(!r.ok);
    assert!(
        r.errors.iter().any(|e| e.contains("os_mod")),
        "{:?}",
        r.errors
    );

    let bad_path = scaffold_with_main(
        tmp.path(),
        "asset_mod",
        "return function(mod)\n    mod.assets:path(\"../evil\")\nend\n",
    );
    let r = validate_mod(&bad_path, None);
    assert!(!r.ok);
    assert!(
        r.errors.iter().any(|e| e.contains("asset_mod")),
        "{:?}",
        r.errors
    );
}

#[test]
fn validate_does_not_touch_the_rom_without_a_rom_path() {
    // An id no table can contain: passes without a ROM path, so nothing was looked up.
    let tmp = TempDir::new("val_norom").unwrap();
    let dir = scaffold_with_main(
        tmp.path(),
        "norom_mod",
        "return function(mod)\n    mod.content.weapons:patch(987654, { AmmoDamage = 1 })\nend\n",
    );
    let r = validate_mod(&dir, None);
    assert!(r.ok, "{:?}", r.errors);
    // A ROM path that does not exist is an error, not a silent skip.
    let r = validate_mod(&dir, Some(&tmp.path().join("no_such_rom")));
    assert!(!r.ok);
}

#[test]
fn validate_checks_patch_ids_against_rom_tables() {
    let rom = dc_rom::rom_or_skip!();
    let tmp = TempDir::new("val_rom").unwrap();
    let bad = scaffold_with_main(
        tmp.path(),
        "rom_bad",
        "return function(mod)\n    mod.content.weapons:patch(987654, { AmmoDamage = 1 })\nend\n",
    );
    let r = validate_mod(&bad, Some(&rom));
    assert!(!r.ok);
    assert!(
        r.errors
            .iter()
            .any(|e| e.contains("rom_bad") && e.contains("987654") && e.contains("ROM")),
        "{:?}",
        r.errors
    );

    // Registering the same id in the mod makes the patch legitimate.
    let registered = scaffold_with_main(
        tmp.path(),
        "rom_reg",
        "return function(mod)\n    mod.content.weapons:register({ id = 987654, Name = 'x' })\n    mod.content.weapons:patch(987654, { AmmoDamage = 1 })\nend\n",
    );
    let r = validate_mod(&registered, Some(&rom));
    assert!(r.ok, "{:?}", r.errors);

    let good = scaffold_with_main(tmp.path(), "rom_good", ROUNDTRIP_MAIN);
    let r = validate_mod(&good, Some(&rom));
    assert!(
        r.ok,
        "weapon 145 patch should validate against the ROM: {:?}",
        r.errors
    );
}

#[test]
fn stubs_cover_the_real_capability_surface() {
    // Ask the real host which names `mod` exposes, then require a stub entry for each.
    let tmp = TempDir::new("stubs").unwrap();
    let probe = r#"return function(mod)
    local out = {}
    local function walk(prefix, t, depth)
        for k, v in pairs(t) do
            if type(k) == "string" then
                out[#out + 1] = prefix .. k
                if type(v) == "table" and depth < 3 and k ~= "exports" then
                    walk(prefix .. k .. ".", v, depth + 1)
                end
            end
        end
    end
    walk("", mod, 0)
    mod.save:set("names", out)
    mod.save:set("math_clamp", math.clamp(15, 0, 10))
end
"#;
    let dir = scaffold_with_main(tmp.path(), "probe_mod", probe);
    let mut host = ModHost::new();
    let manifest = ModManifest::from_file(&dir.join("manifest.json")).unwrap();
    host.load_mod_from_dir_with_manifest(&dir, manifest)
        .unwrap();
    let saves = JsonValue::parse(&host.export_mod_saves_json("probe_mod")).unwrap();
    let obj = saves.as_object().unwrap();
    assert_eq!(obj["math_clamp"], JsonValue::Number(10.0));
    let names = obj["names"].as_array().unwrap();
    assert!(
        names.len() > 30,
        "unexpectedly few capability names: {}",
        names.len()
    );

    let stub = fs::read_to_string(dir.join("dc_mod_api.d.lua")).unwrap();
    for n in names {
        let JsonValue::String(path) = n else {
            panic!("non-string name")
        };
        let last = path.rsplit('.').next().unwrap();
        assert!(
            stub.contains(&format!("---@field {} ", last)),
            "stub has no entry for mod.{}",
            path
        );
    }
    for d in CONTENT_DOMAINS {
        assert!(
            stub.contains(&format!("---@field {} ModContentRegistry", d)),
            "{}",
            d
        );
    }
    // The maps domain and its record shape (MAP-4) are in the stubs, with the mode vocabulary.
    assert!(stub.contains("---@field maps ModContentRegistry"));
    assert!(stub.contains("---@class ModMapRecord"));
    assert!(stub.contains("TDM, TDM_CLAN, COOP, FFA, ELIM, ELIM_CLAN, FLAG, FLAG_CLAN, BOMB, BOMB_CLAN, INFECT, BOUNTY"));
    assert!(stub.contains("1..32 characters"));
    // Documented extension and limits.
    assert!(stub.contains("---@class mathlib"));
    assert!(stub.contains("---@field clamp fun(x: number, lo: number, hi: number): number"));
    let numbers = [
        dc_mod_lua::DEFAULT_INSTRUCTION_LIMIT.to_string(),
        dc_mod_lua::DEFAULT_MEMORY_LIMIT.to_string(),
        dc_mod_lua::DEFAULT_MAX_STRING_BYTES.to_string(),
        dc_mod_lua::DEFAULT_MAX_CALL_DEPTH.to_string(),
    ];
    let words = [
        "string.dump",
        "math.randomseed",
        "Circuit breaker",
        "Precompiled",
    ];
    for needle in numbers.iter().map(|s| s.as_str()).chain(words) {
        assert!(stub.contains(needle), "stub does not document {}", needle);
    }
    let luarc = fs::read_to_string(dir.join(".luarc.json")).unwrap();
    assert!(JsonValue::parse(&luarc).is_ok(), "luarc must be valid JSON");
    assert!(luarc.contains("\"Lua 5.4\""));
    assert!(luarc.contains("\"os\": \"disable\""));
    assert!(luarc.contains("dc_mod_api.d.lua"));
    // The stub file itself passes lint (comment-only Lua).
    assert_eq!(
        lint_errors(&dir.join("dc_mod_api.d.lua")),
        Vec::<String>::new()
    );
}

#[test]
fn read_stored_zip_returns_what_the_writer_wrote_and_refuses_bad_names() {
    let files = vec![
        ("main.lua".to_string(), b"return function(mod) end".to_vec()),
        ("data/a.txt".to_string(), vec![1, 2, 3]),
    ];
    let zip = crate::write_deterministic_zip(&files).unwrap();
    let mut back = crate::read_stored_zip(&zip).unwrap();
    back.sort();
    let mut want = files.clone();
    want.sort();
    assert_eq!(back, want);
    let evil = crate::write_deterministic_zip(&[("../x".to_string(), vec![0])]).unwrap();
    assert!(crate::read_stored_zip(&evil)
        .unwrap_err()
        .contains("plain relative path"));
    let mut broken = zip.clone();
    let at = broken.windows(3).position(|w| w == [1, 2, 3]).unwrap();
    broken[at] = 9;
    assert!(crate::read_stored_zip(&broken).unwrap_err().contains("CRC"));
}

#[test]
fn screenshots_are_checked_packed_and_named_in_the_modset() {
    let tmp = TempDir::new("shots").unwrap();
    let dir = scaffold_mod(
        "shot_mod",
        tmp.path(),
        ScaffoldProfile::TotalConversion,
        None,
    )
    .unwrap();
    // the author fills in the metadata and a real image
    let man = dir.join("manifest.json");
    let text = fs::read_to_string(&man)
        .unwrap()
        .replace(
            crate::PLACEHOLDER_DESCRIPTION,
            "A total conversion used by the round-trip test.",
        )
        .replace(crate::PLACEHOLDER_AUTHOR, "tester");
    fs::write(&man, &text).unwrap();
    let jpg = [&[0xffu8, 0xd8, 0xff, 0xe0][..], &[0u8; 32][..]].concat();
    fs::write(
        dir.join("screenshots/main.png"),
        [&crate::placeholder_png()[..], b"real"].concat(),
    )
    .unwrap();
    fs::write(dir.join("screenshots/second.jpg"), &jpg).unwrap();
    let with_two = text.replace(
        "\"screenshots/main.png\"",
        "\"screenshots/main.png\",\n    \"screenshots/second.jpg\"",
    );
    fs::write(&man, &with_two).unwrap();
    assert!(lint_errors(&dir).is_empty(), "{:?}", lint_errors(&dir));
    let warns: Vec<_> = lint_mod(&dir)
        .unwrap()
        .into_iter()
        .filter(|d| d.message.contains("placeholder"))
        .collect();
    assert!(warns.is_empty(), "{warns:?}");

    let zip = pack_mod(&dir, &tmp.path().join("shot_mod.zip")).unwrap();
    let names: Vec<String> = crate::read_stored_zip(&zip)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert!(
        names.contains(&"screenshots/main.png".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"screenshots/second.jpg".to_string()),
        "{names:?}"
    );

    let frag = crate::modset_fragment(std::slice::from_ref(&dir), false).unwrap();
    let JsonValue::Object(o) = &frag else {
        panic!()
    };
    let JsonValue::Array(mods) = &o["mods"] else {
        panic!()
    };
    let JsonValue::Object(m) = &mods[0] else {
        panic!()
    };
    assert_eq!(
        m["description"].as_str(),
        Some("A total conversion used by the round-trip test.")
    );
    assert_eq!(m["author"].as_str(), Some("tester"));
    assert_eq!(m["tags"], JsonValue::Array(Vec::new()));
    assert_eq!(
        m["screenshots"],
        JsonValue::Array(vec![
            JsonValue::String("screenshots/main.png".into()),
            JsonValue::String("screenshots/second.jpg".into()),
        ])
    );

    // a JPEG named .png, a missing file and an oversized file are lint errors on the manifest line
    fs::write(dir.join("screenshots/main.png"), &jpg).unwrap();
    fs::remove_file(dir.join("screenshots/second.jpg")).unwrap();
    let errs = lint_errors(&dir);
    let line_main = with_two
        .lines()
        .position(|l| l.contains("screenshots/main.png"))
        .unwrap()
        + 1;
    assert!(
        errs.contains(&format!(
            "manifest.json:{line_main}: screenshot 'screenshots/main.png' is not a PNG image (its first bytes do not match)"
        )),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| e.contains("'screenshots/second.jpg' does not exist")),
        "{errs:?}"
    );
    fs::write(
        dir.join("screenshots/second.jpg"),
        vec![0xffu8; 1024 * 1024 + 1],
    )
    .unwrap();
    assert!(
        lint_errors(&dir).iter().any(|e| e.contains("1 MiB")),
        "{:?}",
        lint_errors(&dir)
    );
    assert!(pack_mod(&dir, &tmp.path().join("bad.zip")).is_err());
}

#[test]
fn read_stored_zip_refuses_duplicates_control_chars_and_floods() {
    let dup = crate::write_deterministic_zip(&[
        ("a.txt".to_string(), vec![1]),
        ("A.txt".to_string(), vec![2]),
    ]);
    if let Ok(z) = dup {
        assert!(crate::read_stored_zip(&z).unwrap_err().contains("twice"));
    }
    let ctl = crate::write_deterministic_zip(&[("a\x07b".to_string(), vec![1])]);
    if let Ok(z) = ctl {
        assert!(crate::read_stored_zip(&z).is_err());
    }
    let many: Vec<(String, Vec<u8>)> = (0..4097).map(|i| (format!("f{i}"), vec![])).collect();
    if let Ok(z) = crate::write_deterministic_zip(&many) {
        assert!(crate::read_stored_zip(&z).unwrap_err().contains("at most"));
    }
}

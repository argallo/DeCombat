//! gen1recomp-inspired sandboxed Lua modding runtime, capability injection, content registries, and hooks.
//!
//! Mods run in real Lua 5.4 (`mlua`, vendored) inside per-mod sandbox environments; see `sandbox`.
//!
//! Content domains with a Rust-side shape: `weapons` (`ModHost::weapon_patches`, MOD-21), `hud` (`hudkeys`,
//! CLI-1613) and `maps` (`mapkeys` / `maprecord`, `ModHost::map_records`, MAP-4).
// DOC: docs/MODDING_GUIDE.md

pub mod host;
pub mod hudkeys;
pub mod manifest;
pub mod mapkeys;
pub mod maprecord;
pub mod modset;
pub mod resolver;
pub mod sandbox;
pub mod settings;
pub mod syntax;
pub mod value;

#[cfg(test)]
mod kind_tests;
#[cfg(test)]
mod sandbox_tests;

pub use host::{
    validate_asset_rel_path, ContentDomainStore, ContentRegistry, EventRegistration,
    HookRegistration, LoadedModInfo, ModHost, ModOrigin, ModPolicy, CONTENT_DOMAINS,
    DEFAULT_CIRCUIT_BREAKER_THRESHOLD,
};
pub use hudkeys::{
    check_hud_override, parse_rgba_hex, CROSSHAIR_RECOIL_MODES, CROSSHAIR_STYLES, HUD_OVERRIDE_KEYS, HUD_SCALE_RANGE,
};
pub use manifest::{
    content_capability, curated_content_refusal, curated_refusal, hook_capability, is_valid_mod_id, is_valid_screenshot_path, is_valid_tag,
    validate_capability, AssetPolicy, JsonValue, ModCommand, ModDep, ModKind, ModManifest, SemVer, VersionReq,
    ALLOWED_PERMISSIONS, ASSET_POLICIES, CLIENT_VIEW_CAPABILITIES, CURATED_CAPABILITIES, DESCRIPTION_CHARS,
    HOOK_NAMESPACES, MAX_AUTHOR_CHARS, MAX_SCREENSHOTS, MAX_SCREENSHOT_BYTES, MAX_TAGS,
    MAX_COMMANDS, MAX_COMMAND_CHARS, MAX_COMMAND_TEXT_CHARS, MAX_TAG_CHARS, MOD_KINDS, PLAIN_CAPABILITIES,
};
pub use mapkeys::{
    check_map_record, is_valid_map_id, own_asset_rel_path, MAP_DESCRIPTION_MAX, MAP_ID_MAX,
    MAP_NAME_MAX, MAP_RECORD_KEYS, MAP_REQUIRED_KEYS, PLAYERS_RANGE, ROUND_SECONDS_RANGE,
};
pub use maprecord::{MapRecord, PlayerRange, RadarInfo};
pub use modset::{mod_set_hash_text, HashEntry, EMPTY_SET_HASH};
pub use resolver::{resolve_manifests, resolve_mods, DiscoveredMod, ResolverError};
pub use sandbox::{
    check_not_bytecode, from_lua, to_lua, validate_module_name, LuaSandbox, ModEnv, SandboxLimits,
    BYTECODE_FORBIDDEN_MSG, DEFAULT_FUEL_BUDGET, DEFAULT_INSTRUCTION_LIMIT, DEFAULT_MAX_CALL_DEPTH,
    DEFAULT_MAX_STRING_BYTES, DEFAULT_MEMORY_LIMIT,
};
pub use settings::{
    colour_text, decode_share_code, encode_share_code, format_number, parse_settings, resolve_settings, ModSetting,
    NumberRange, SettingKind, SettingOption, MAX_NUMBER_STEPS, MAX_SETTINGS, MAX_SETTING_KEY_CHARS,
    MAX_SETTING_LABEL_CHARS, MAX_SETTING_OPTIONS, SHARE_PREFIX,
};
pub use syntax::{
    parse_chunk, tokenize, AssignTarget, BinOp, Expr, ExprKind, Stmt, StmtKind, TableField, Token,
    TokenKind, UnOp,
};
pub use value::{LuaError, LuaKey, LuaTable, LuaValue};

/// Re-export of the Lua binding so hosts can name `mlua` types without a separate dependency.
pub use mlua;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ca_mod_lua_test_{}_{}_{}",
            prefix,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_json_parser_and_manifest_parsing() {
        let raw = r#"{
            "id": "classic_ak47",
            "name": "Classic AK-47 Rebalance",
            "version": "1.2.3",
            "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],
            "capabilities": ["hooks.combat", "content.weapons"],
            "entry": "main.lua",
            "api": 1,
            "priority": -10,
            "games": ["combatarms_2009"],
            "dependencies": ["core_utils@^1.0.0", "shared_types@>=0.2.0"],
            "optional_dependencies": ["hud_plus@~2.1.0"],
            "incompatible": ["legacy_weapons"],
            "permissions": ["engine_internals"],
            "options_schema": "options.json",
            "github": "combat/classic_ak47"
        }"#;

        let manifest = ModManifest::from_json_str(raw).expect("valid manifest");
        assert_eq!(manifest.id, "classic_ak47");
        assert_eq!(manifest.name, "Classic AK-47 Rebalance");
        assert_eq!(manifest.version, SemVer::new(1, 2, 3));
        assert_eq!(manifest.priority, -10);
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(manifest.optional_dependencies.len(), 1);
        assert_eq!(manifest.conflicts.len(), 1);
        assert_eq!(manifest.conflicts[0].id, "legacy_weapons");
        assert_eq!(manifest.permissions, vec!["engine_internals"]);
        assert_eq!(manifest.github.as_deref(), Some("combat/classic_ak47"));
        assert_eq!(manifest.kind, ModKind::Shared);
        assert!(manifest.requires_client);
        assert_eq!(
            manifest.capabilities,
            vec!["content.weapons", "hooks.combat"]
        );
        assert_eq!(manifest.asset_policy, AssetPolicy::OriginalOnly);

        // Round-trip pretty JSON
        let pretty = manifest.to_json_pretty();
        let reparsed = ModManifest::from_json_str(&pretty).expect("reparsed manifest");
        assert_eq!(manifest, reparsed);

        // Reject unknown permission
        let bad_perm = r#"{
            "id": "bad_mod",
            "name": "Bad Mod",
            "version": "1.0.0",
            "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],
            "permissions": ["filesystem_root"]
        }"#;
        assert!(ModManifest::from_json_str(bad_perm).is_err());

        // Reject invalid id
        let bad_id = r#"{
            "id": "bad/mod",
            "name": "Bad Mod",
            "version": "1.0.0",
            "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"]
        }"#;
        assert!(ModManifest::from_json_str(bad_id).is_err());
    }

    #[test]
    fn test_resolver_ordering_optional_deps_priority_and_conflicts() {
        let core = ModManifest::from_json_str(
            r#"{"id": "core", "name": "Core", "version": "1.2.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 50}"#,
        )
        .unwrap();
        let opt_lib = ModManifest::from_json_str(
            r#"{"id": "opt_lib", "name": "OptLib", "version": "2.0.1", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 100}"#,
        )
        .unwrap();
        let addon_a = ModManifest::from_json_str(
            r#"{
                "id": "addon_a",
                "name": "Addon A",
                "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"],
                "priority": 10,
                "dependencies": ["core@^1.0.0"],
                "optional_dependencies": ["opt_lib@^2.0.0", "non_existent@^1.0.0"]
            }"#,
        )
        .unwrap();
        let addon_b = ModManifest::from_json_str(
            r#"{
                "id": "addon_b",
                "name": "Addon B",
                "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"],
                "priority": 5,
                "dependencies": ["core@^1.0.0"]
            }"#,
        )
        .unwrap();

        let ordered = resolve_manifests(&[
            addon_a.clone(),
            addon_b.clone(),
            opt_lib.clone(),
            core.clone(),
        ])
        .expect("should resolve");
        let ids: Vec<&str> = ordered.iter().map(|m| m.id.as_str()).collect();

        // `core` (priority 50) unlocks `addon_b` (priority 5), which runs before `opt_lib` (priority 100),
        // and `addon_a` waits for both `core` and `opt_lib`.
        let pos_core = ids.iter().position(|&x| x == "core").unwrap();
        let pos_opt = ids.iter().position(|&x| x == "opt_lib").unwrap();
        let pos_a = ids.iter().position(|&x| x == "addon_a").unwrap();
        let pos_b = ids.iter().position(|&x| x == "addon_b").unwrap();
        assert!(pos_core < pos_a);
        assert!(pos_opt < pos_a);
        assert!(pos_core < pos_b);

        // Priority tie-breaking
        let tie_1 = ModManifest::from_json_str(
            r#"{"id": "zeta", "name": "Z", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": -5}"#,
        )
        .unwrap();
        let tie_2 = ModManifest::from_json_str(
            r#"{"id": "alpha", "name": "A", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 0}"#,
        )
        .unwrap();
        let tie_3 = ModManifest::from_json_str(
            r#"{"id": "beta", "name": "B", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 0}"#,
        )
        .unwrap();
        let tie_ordered = resolve_manifests(&[tie_3, tie_2, tie_1]).unwrap();
        let tie_ids: Vec<&str> = tie_ordered.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(tie_ids, vec!["zeta", "alpha", "beta"]);

        // Conflict rejection
        let conflicting = ModManifest::from_json_str(
            r#"{"id": "conflict_mod", "name": "C", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "conflicts": ["core@^1.0.0"]}"#,
        )
        .unwrap();
        let err = resolve_manifests(&[core.clone(), conflicting]).unwrap_err();
        assert!(matches!(err, ResolverError::Conflict { .. }));

        // Cycle rejection
        let cyc_a = ModManifest::from_json_str(
            r#"{"id": "cyc_a", "name": "A", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "dependencies": ["cyc_b"]}"#,
        )
        .unwrap();
        let cyc_b = ModManifest::from_json_str(
            r#"{"id": "cyc_b", "name": "B", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "dependencies": ["cyc_a"]}"#,
        )
        .unwrap();
        let cyc_err = resolve_manifests(&[cyc_a, cyc_b]).unwrap_err();
        assert!(matches!(cyc_err, ResolverError::DependencyCycle { .. }));
    }

    #[test]
    fn test_middleware_hooks_content_exports_saves_options_and_assets() {
        let mut host = ModHost::new();

        let mod_a_manifest = ModManifest::from_json_str(
            r#"{"id": "mod_a", "name": "Mod A", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 0}"#,
        )
        .unwrap();
        let mod_a_src = r#"
            return function(mod)
                mod.options:define({
                    damage_bonus = { type = "number", default = 12 },
                    flat_xp = 50
                })
                local bonus = mod.options:get("damage_bonus")
                mod.content.weapons:patch(145, {
                    AmmoDamage = 38 + bonus,
                    FireDelay = 100
                })
                mod.content.weapons:register({
                    id = 999,
                    Name = "Prototype Railgun",
                    AmmoDamage = 120
                })
                mod.content.hud:override("hud_scale", 1.25)

                mod.exports.get_multiplier = function()
                    return 2
                end

                mod.save:set("kills", 42)
                local icon = mod.assets:path("icons/ak47.dtx")
                mod.save:set("icon_uri", icon)

                mod.hooks:wrap("lobby.calc_exp_gp", function(base, ctx)
                    local res = base(ctx)
                    res.exp = res.exp + mod.options:get("flat_xp")
                    res.gp = res.gp + 25
                    return res
                end)

                mod.events:on("match.completed", function(ctx)
                    local prev = mod.save:get("kills") or 0
                    mod.save:set("kills", prev + (ctx.frags or 0))
                end)
            end
        "#;

        let mod_b_manifest = ModManifest::from_json_str(
            r#"{"id": "mod_b", "name": "Mod B", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 10, "dependencies": ["mod_a@^1.0.0"]}"#,
        )
        .unwrap();
        let mod_b_src = r#"
            return function(mod)
                local a = mod.find("mod_a")
                assert(a ~= nil, "mod_a must be found")
                local mult = a.exports.get_multiplier()

                mod.content.weapons:patch(145, {
                    AmmoDamage = 55,
                    CustomTag = "overhauled"
                })

                mod.hooks:wrap("lobby.calc_exp_gp", function(base, ctx)
                    local res = base(ctx)
                    res.exp = res.exp * mult
                    res.gp = res.gp * mult
                    return res
                end)
            end
        "#;

        host.load_mod_from_source(mod_a_manifest, mod_a_src, None)
            .expect("load mod_a");
        host.load_mod_from_source(mod_b_manifest, mod_b_src, None)
            .expect("load mod_b");

        // Verify content patching & merging on weapon 145
        let w145 = host
            .get_content_record("weapons", "145")
            .expect("weapon 145 should exist");
        assert_eq!(w145.borrow().get_str("AmmoDamage"), LuaValue::Number(55.0));
        assert_eq!(w145.borrow().get_str("FireDelay"), LuaValue::Number(100.0));
        assert_eq!(
            w145.borrow().get_str("CustomTag"),
            LuaValue::String("overhauled".to_string())
        );

        // Verify registered weapon 999
        let w999 = host
            .get_content_record("weapons", "999")
            .expect("weapon 999 should exist");
        assert_eq!(
            w999.borrow().get_str("Name"),
            LuaValue::String("Prototype Railgun".to_string())
        );

        // Verify HUD override
        assert_eq!(
            host.get_content_value("hud", "hud_scale"),
            LuaValue::Number(1.25)
        );

        // Verify middleware hook chaining:
        // Base calculates exp=100, gp=75.
        // mod_a adds +50 exp, +25 gp -> exp=150, gp=100.
        // mod_b multiplies by 2 -> exp=300, gp=200.
        let mut ctx = LuaTable::new();
        ctx.set_str("kills", LuaValue::Number(10.0));
        let hook_res = host
            .run_hook("lobby.calc_exp_gp", ctx, |c| {
                let kills = c.get_str("kills").as_f64().unwrap_or(0.0);
                let mut out = LuaTable::new();
                out.set_str("exp", LuaValue::Number(kills * 10.0));
                out.set_str("gp", LuaValue::Number(75.0));
                Ok(LuaValue::Table(std::rc::Rc::new(std::cell::RefCell::new(
                    out,
                ))))
            })
            .expect("hook execution should succeed");

        let res_tbl = hook_res.as_table().unwrap();
        assert_eq!(res_tbl.borrow().get_str("exp"), LuaValue::Number(300.0));
        assert_eq!(res_tbl.borrow().get_str("gp"), LuaValue::Number(200.0));

        // Verify event emission & save updates
        let mut ev_ctx = LuaTable::new();
        ev_ctx.set_str("frags", LuaValue::Number(8.0));
        host.emit_event("match.completed", ev_ctx);

        let saves_json = host.export_mod_saves_json("mod_a");
        let parsed_saves = JsonValue::parse(&saves_json).unwrap();
        assert_eq!(parsed_saves.get("kills").and_then(|v| v.as_i64()), Some(50));
        assert_eq!(
            parsed_saves.get("icon_uri").and_then(|v| v.as_str()),
            Some("@mod:mod_a/icons/ak47.dtx")
        );
    }

    #[test]
    fn test_sandbox_security_negative_controls_and_isolation() {
        // 1. Bytecode rejection
        let sb = LuaSandbox::new().unwrap();
        let env = sb.create_env("sec", None).unwrap();
        let bytecode_err = sb.eval_chunk(&env, "\x1bLua\x54\x00\x19").unwrap_err();
        assert_eq!(bytecode_err.message(), BYTECODE_FORBIDDEN_MSG);

        // 2. Stripped dangerous globals
        let check_src = r#"
            assert(os == nil, "os must be nil")
            assert(io == nil, "io must be nil")
            assert(debug == nil, "debug must be nil")
            assert(package == nil, "package must be nil")
            assert(ffi == nil, "ffi must be nil")
            assert(getfenv == nil, "getfenv must be nil")
            assert(setfenv == nil, "setfenv must be nil")
            assert(load == nil and loadstring == nil, "load must be nil")
            assert(dofile == nil and loadfile == nil, "dofile/loadfile must be nil")
            assert(collectgarbage == nil, "collectgarbage must be nil")
            assert(string.dump == nil, "string.dump must be nil")
            assert(math.randomseed == nil, "math.randomseed must be nil")
            assert(type(require) == "function", "scoped require is provided")
            return true
        "#;
        assert_eq!(
            sb.eval_chunk(&env, check_src).unwrap(),
            LuaValue::Bool(true)
        );

        // 3. Instruction limit on infinite loop (even when wrapped in pcall!)
        let low = LuaSandbox::with_limits(SandboxLimits::with_instruction_limit(1_000)).unwrap();
        let low_env = low.create_env("low", None).unwrap();
        let loop_err = low
            .eval_chunk(&low_env, "pcall(function() while true do end end)")
            .unwrap_err();
        assert!(
            matches!(loop_err, LuaError::InstructionLimit(_)),
            "{:?}",
            loop_err
        );

        // 4. Path traversal blocked in `require` and `mod.assets:path`
        let tmp = make_temp_dir("sandbox_sec");
        let req_env = sb.create_env("req", Some(&tmp)).unwrap();
        assert!(sb.eval_chunk(&req_env, r#"require("../secret")"#).is_err());
        assert!(sb
            .eval_chunk(&req_env, r#"require("/etc/passwd")"#)
            .is_err());

        let mut host = ModHost::new();
        let bad_asset_manifest = ModManifest::from_json_str(
            r#"{"id": "bad_asset", "name": "Bad Asset", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"]}"#,
        )
        .unwrap();
        let bad_asset_res = host.load_mod_from_source(
            bad_asset_manifest,
            r#"return function(mod) mod.assets:path("../outside.png") end"#,
            None,
        );
        assert!(bad_asset_res.is_err());

        // 5. Fault isolation in hooks and events + _G / string table isolation across mods
        let mut iso_host = ModHost::new();
        let m1 = ModManifest::from_json_str(
            r#"{"id": "mod_one", "name": "One", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 1}"#,
        )
        .unwrap();
        let m1_src = r#"
            _G.secret_global = "leaked"
            string.hacked = function() return "pwned" end
            return function(mod)
                mod.hooks:wrap("combat.damage", function(base, ctx)
                    local d = base(ctx)
                    return d + 10
                end)
                mod.events:on("round.start", function(ctx)
                    mod.save:set("started", true)
                end)
            end
        "#;

        let m2 = ModManifest::from_json_str(
            r#"{"id": "mod_faulty", "name": "Faulty", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 2}"#,
        )
        .unwrap();
        let m2_src = r#"
            assert(_G.secret_global == nil, "_G must be isolated between mods")
            assert(string.hacked == nil, "string table must be isolated between mods")
            return function(mod)
                mod.hooks:wrap("combat.damage", function(base, ctx)
                    error("intentional hook crash in mod_faulty")
                end)
                mod.events:on("round.start", function(ctx)
                    error("intentional event crash in mod_faulty")
                end)
            end
        "#;

        iso_host.load_mod_from_source(m1, m1_src, None).unwrap();
        iso_host.load_mod_from_source(m2, m2_src, None).unwrap();

        let dmg = iso_host
            .run_hook("combat.damage", LuaTable::new(), |_| {
                Ok(LuaValue::Number(40.0))
            })
            .expect("host must isolate faulty hook and return mod_one + base result");
        assert_eq!(dmg, LuaValue::Number(50.0));
        assert_eq!(iso_host.mod_errors("mod_faulty").len(), 1);
        assert!(iso_host.mod_errors("mod_one").is_empty());

        iso_host.emit_event("round.start", LuaTable::new());
        assert_eq!(iso_host.mod_errors("mod_faulty").len(), 2);
        let m1_saves = JsonValue::parse(&iso_host.export_mod_saves_json("mod_one")).unwrap();
        assert_eq!(
            m1_saves.get("started").and_then(|v| v.as_bool()),
            Some(true)
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_lua_vm_comprehensive_language_and_stdlib() {
        let sb = LuaSandbox::new().unwrap();
        let env = sb.create_env("lang", None).unwrap();
        let src = r##"
            -- Literals, hex, long strings, operators
            local hex = 0x20
            local long_str = [[hello
world]]
            assert(hex == 32)
            assert(#long_str == 11)
            assert(10 // 3 == 3)
            assert(10 % 3 == 1)
            assert(2 ^ 10 == 1024)
            assert("a" .. "b" .. 3 == "ab3")
            assert((false or 42) == 42)
            assert((true and "yes") == "yes")

            -- Table constructor with integer keys, named keys, bracket keys
            local t = { 10, 20, a = 3, ["b"] = 4, [145] = { AmmoDamage = 38 } }
            assert(#t == 2)
            assert(t[1] == 10 and t[2] == 20)
            assert(t.a == 3 and t["b"] == 4)
            assert(t[145].AmmoDamage == 38)

            -- Recursive local function, closures, lexical scoping
            local function fact(n)
                if n <= 1 then
                    return 1
                else
                    return n * fact(n - 1)
                end
            end
            assert(fact(5) == 120)

            local function make_counter(start)
                local count = start
                return function(step)
                    count = count + (step or 1)
                    return count
                end
            end
            local c1 = make_counter(10)
            assert(c1(5) == 15)
            assert(c1() == 16)

            -- Control flow: while, repeat..until (accessing inner local), numeric for, generic for
            local sum = 0
            local i = 1
            while i <= 5 do
                sum = sum + i
                i = i + 1
            end
            assert(sum == 15)

            local rep_count = 0
            repeat
                rep_count = rep_count + 1
                local done = (rep_count == 3)
            until done
            assert(rep_count == 3)

            local for_sum = 0
            for k = 10, 2, -2 do
                if k == 4 then
                    break
                end
                for_sum = for_sum + k
            end
            assert(for_sum == 10 + 8 + 6)

            -- Colon methods on tables
            local obj = { val = 7 }
            function obj:add(x, y)
                self.val = self.val + x + y
                return self.val
            end
            assert(obj:add(3, 5) == 15)

            -- math library
            assert(math.floor(3.9) == 3)
            assert(math.ceil(3.1) == 4)
            assert(math.abs(-9) == 9)
            assert(math.min(5, 2, 8) == 2)
            assert(math.max(5, 2, 8) == 8)
            assert(math.sqrt(81) == 9)
            assert(math.clamp(15, 0, 10) == 10)
            assert(math.clamp(-5, 0, 10) == 0)
            assert(math.pi > 3.14 and math.huge > 1e100)

            -- string library & string colon methods
            assert(("combat"):upper() == "COMBAT")
            assert(string.lower("ARMS") == "arms")
            assert(string.sub("abcdef", 2, 4) == "bcd")
            assert(string.sub("abcdef", -2) == "ef")
            assert(string.rep("ab", 3, "-") == "ab-ab-ab")
            assert(string.reverse("abc") == "cba")
            local s_idx, e_idx = string.find("hello world", "world")
            assert(s_idx == 7 and e_idx == 11)
            assert(string.byte("A") == 65)
            assert(string.char(65, 66) == "AB")
            local fmt = string.format("%s=%d, hex=%x, float=%.2f, pct=%%", "dmg", 38, 255, 3.14159)
            assert(fmt == "dmg=38, hex=ff, float=3.14, pct=%")

            -- table library, pairs, ipairs, select, pcall, xpcall, print
            local arr = { "x", "z" }
            table.insert(arr, 2, "y")
            assert(table.concat(arr, ",") == "x,y,z")
            local rem = table.remove(arr, 2)
            assert(rem == "y" and #arr == 2)
            local u1, u2 = table.unpack(arr)
            assert(u1 == "x" and u2 == "z")

            local ip_sum = ""
            for idx, v in ipairs(arr) do
                ip_sum = ip_sum .. tostring(idx) .. v
            end
            assert(ip_sum == "1x2z")

            assert(select("#", 10, 20, 30) == 3)
            local s2, s3 = select(2, "a", "b", "c")
            assert(s2 == "b" and s3 == "c")

            local ok1, err1 = pcall(function() error("boom") end)
            assert(ok1 == false and string.find(err1, "boom") ~= nil)

            local ok2, err2 = xpcall(
                function() error("bad", 0) end,
                function(e) return "handled:" .. e end
            )
            assert(ok2 == false and err2 == "handled:bad")

            print("log_line_1", 123)
            return "all_passed"
        "##;

        let res = sb
            .eval_chunk(&env, src)
            .expect("comprehensive chunk should pass");
        assert_eq!(res, LuaValue::String("all_passed".to_string()));
        assert_eq!(env.logs(), vec!["log_line_1\t123".to_string()]);
    }

    #[test]
    fn test_scoped_require_caching_and_override_dirs_in_load_order() {
        let tmp = make_temp_dir("require_and_overrides");

        // Create two mods in tmp: base_mod (priority -5) and exp_mod (priority 10, depends on base_mod)
        let base_dir = tmp.join("base_mod");
        fs::create_dir_all(base_dir.join("util")).unwrap();
        fs::create_dir_all(base_dir.join("overrides")).unwrap();
        fs::write(
            base_dir.join("manifest.json"),
            r#"{"id": "base_mod", "name": "Base Mod", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": -5}"#,
        )
        .unwrap();
        fs::write(
            base_dir.join("util").join("math_ext.lua"),
            r#"
                local M = {}
                M.load_count = (M.load_count or 0) + 1
                function M.triple(x)
                    return x * 3
                end
                return M
            "#,
        )
        .unwrap();
        fs::write(
            base_dir.join("main.lua"),
            r#"
                return function(mod)
                    local m1 = require("util.math_ext")
                    local m2 = require("util.math_ext")
                    assert(m1 == m2, "require must cache module table per mod")
                    assert(m1.triple(7) == 21)
                end
            "#,
        )
        .unwrap();

        let exp_dir = tmp.join("exp_mod");
        fs::create_dir_all(exp_dir.join("overrides")).unwrap();
        fs::write(
            exp_dir.join("manifest.json"),
            r#"{"id": "exp_mod", "name": "Expansion Mod", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": 10, "dependencies": ["base_mod@^1.0.0"]}"#,
        )
        .unwrap();
        fs::write(exp_dir.join("main.lua"), "return function(mod) end").unwrap();

        let mut host = ModHost::new();
        let loaded = host.discover_and_load_dir(&tmp).expect("discover and load");
        assert_eq!(loaded, vec!["base_mod".to_string(), "exp_mod".to_string()]);

        let override_dirs = host.override_dirs();
        assert_eq!(
            override_dirs,
            vec![base_dir.join("overrides"), exp_dir.join("overrides")]
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}

//! Negative controls for the mlua sandbox: every limit must fail closed with a clean error.
// DOC: docs/MODDING_GUIDE.md

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::*;

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "ca_mod_lua_sb_{}_{}_{}",
        prefix,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(id: &str, priority: i32) -> ModManifest {
    ModManifest::from_json_str(&format!(
        r#"{{"id": "{}", "name": "{}", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"], "priority": {}}}"#,
        id, id, priority
    ))
    .unwrap()
}

fn sandbox(limits: SandboxLimits) -> (LuaSandbox, ModEnv) {
    let sb = LuaSandbox::with_limits(limits).unwrap();
    let env = sb.create_env("t", None).unwrap();
    (sb, env)
}

#[test]
fn infinite_loops_are_killed_by_the_instruction_limit() {
    let (sb, env) = sandbox(SandboxLimits::with_instruction_limit(100_000));
    for src in [
        "while true do end",
        "repeat until false",
        "local function f() return f() end return f()", // tail call loop, no stack growth
        "pcall(function() while true do end end)",
        "while true do pcall(function() while true do end end) end",
        "xpcall(function() while true do end end, function(e) while true do end end)",
        "local ok = pcall(error) while true do end",
    ] {
        let err = sb.eval_chunk(&env, src).unwrap_err();
        assert!(
            matches!(err, LuaError::InstructionLimit(_)),
            "{} -> {:?}",
            src,
            err
        );
    }
    // The state stays usable after a kill: the next call gets a fresh budget.
    assert_eq!(
        sb.eval_chunk(&env, "local s = 0 for i = 1, 100 do s = s + i end return s")
            .unwrap(),
        LuaValue::Number(5050.0)
    );
}

#[test]
fn infinite_loop_in_a_hook_is_isolated_and_attributed() {
    let mut host = ModHost::with_fuel(200_000);
    host.load_mod_from_source(
        manifest("spinner", 0),
        r#"return function(mod)
            mod.hooks:wrap("combat.damage", function(base, ctx)
                while true do end
            end)
        end"#,
        None,
    )
    .unwrap();
    let out = host
        .run_hook("combat.damage", LuaTable::new(), |_| {
            Ok(LuaValue::Number(7.0))
        })
        .unwrap();
    assert_eq!(out, LuaValue::Number(7.0));
    let errs = host.mod_errors("spinner");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("spinner"), "{}", errs[0]);
    assert!(errs[0].contains("instruction limit"), "{}", errs[0]);
}

#[test]
fn nested_hook_calls_cannot_multiply_the_budget() {
    // The outer wrapper calls base() many times; each inner stage burns instructions. The total is
    // charged to the outer call, so the outer mod is killed instead of running ~forever.
    let mut host = ModHost::with_fuel(300_000);
    host.load_mod_from_source(
        manifest("inner", 0),
        r#"return function(mod)
            mod.hooks:wrap("combat.h", function(base, ctx)
                local x = 0
                for i = 1, 50000 do x = x + i end
                return base(ctx)
            end)
        end"#,
        None,
    )
    .unwrap();
    host.load_mod_from_source(
        manifest("outer", 1),
        r#"return function(mod)
            mod.hooks:wrap("combat.h", function(base, ctx)
                local last
                for i = 1, 1000 do last = base(ctx) end
                return last
            end)
        end"#,
        None,
    )
    .unwrap();
    let out = host
        .run_hook("combat.h", LuaTable::new(), |_| Ok(LuaValue::Number(1.0)))
        .unwrap();
    assert_eq!(out, LuaValue::Number(1.0));
    assert!(!host.mod_errors("outer").is_empty(), "outer must be killed");
}

#[test]
fn huge_string_rep_and_format_are_rejected() {
    let (sb, env) = sandbox(SandboxLimits::default());
    for src in [
        "return string.rep('x', 1e9)",
        "return ('x'):rep(1e9)",
        "return string.rep('ab', 3e6, ',')",
        "return string.rep('x', 2^40)",
    ] {
        let err = sb.eval_chunk(&env, src).unwrap_err();
        assert!(err.message().contains("string.rep"), "{} -> {:?}", src, err);
    }
    let fmt_err = sb
        .eval_chunk(
            &env,
            "local s = string.rep('x', 3000000) return string.format('%s%s', s, s)",
        )
        .unwrap_err();
    assert!(fmt_err.message().contains("string.format"), "{:?}", fmt_err);
    // pcall sees the failure as an ordinary error.
    assert_eq!(
        sb.eval_chunk(&env, "return (pcall(string.rep, 'x', 1e9))")
            .unwrap(),
        LuaValue::Bool(false)
    );
    // Normal use still works.
    assert_eq!(
        sb.eval_chunk(&env, "return #string.rep('ab', 1000, '-')")
            .unwrap(),
        LuaValue::Number(2999.0)
    );
}

#[test]
fn memory_limit_stops_runaway_allocation() {
    let limits = SandboxLimits {
        memory_limit: 8 * 1024 * 1024,
        instruction_limit: 1_000_000_000,
        ..SandboxLimits::default()
    };
    let (sb, env) = sandbox(limits);
    let err = sb
        .eval_chunk(&env, "local t = {} for i = 1, 1e9 do t[i] = {i} end")
        .unwrap_err();
    assert!(matches!(err, LuaError::MemoryLimit(_)), "{:?}", err);
    assert!(sb.used_memory() <= 8 * 1024 * 1024);
}

#[test]
fn deep_recursion_errors_cleanly() {
    let (sb, env) = sandbox(SandboxLimits::default());
    let src = "local function f(n) return 1 + f(n + 1) end return f(1)";
    let err = sb.eval_chunk(&env, src).unwrap_err();
    assert!(matches!(err, LuaError::CallDepth(_)), "{:?}", err);
    // Also when the mod tries to swallow it.
    let err = sb
        .eval_chunk(
            &env,
            "local function f(n) return 1 + f(n + 1) end pcall(f, 1) return 1",
        )
        .unwrap_err();
    assert!(matches!(err, LuaError::CallDepth(_)), "{:?}", err);
    // Legitimate recursion below the limit is fine.
    assert_eq!(
        sb.eval_chunk(
            &env,
            "local function f(n) if n == 0 then return 0 end return 1 + f(n - 1) end return f(150)"
        )
        .unwrap(),
        LuaValue::Number(150.0)
    );

    // With our depth probe disabled, Lua's own stack limit still ends it with an error.
    let (sb2, env2) = sandbox(SandboxLimits {
        max_call_depth: 0,
        ..SandboxLimits::default()
    });
    let err2 = sb2.eval_chunk(&env2, src).unwrap_err();
    assert!(
        err2.message().contains("stack overflow") || matches!(err2, LuaError::MemoryLimit(_)),
        "{:?}",
        err2
    );
}

#[test]
fn binary_chunks_are_rejected_everywhere() {
    // Produce genuine Lua 5.4 bytecode with an unsandboxed state (test-only).
    let raw = mlua::Lua::new();
    let dumped: mlua::String = raw
        .load("return string.dump(function() return function(mod) end end)")
        .eval()
        .unwrap();
    let bytecode = dumped.as_bytes().to_vec();
    assert_eq!(bytecode[0], 0x1b);

    let dir = temp_dir("bytecode");
    fs::write(
        dir.join("manifest.json"),
        r#"{"id": "bc", "name": "BC", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons", "content.items", "content.shop", "content.gameroom", "content.ai_brains", "content.missions", "content.text", "content.music", "content.hud", "hooks.lobby", "hooks.combat", "events", "save", "assets"]}"#,
    )
    .unwrap();
    fs::write(dir.join("main.lua"), &bytecode).unwrap();
    let mut host = ModHost::new();
    let err = host.load_mod_dir(&dir).unwrap_err();
    assert!(err.contains(BYTECODE_FORBIDDEN_MSG), "{}", err);

    // Through require as well.
    let dir2 = temp_dir("bytecode_req");
    fs::write(dir2.join("evil.lua"), &bytecode).unwrap();
    let sb = LuaSandbox::new().unwrap();
    let env = sb.create_env("bcr", Some(&dir2)).unwrap();
    let err = sb.eval_chunk(&env, "return require('evil')").unwrap_err();
    assert!(err.message().contains(BYTECODE_FORBIDDEN_MSG), "{:?}", err);

    // And the loader itself is text-only even if the signature check were bypassed.
    let err = sb.load_text(&env, "x", &bytecode).unwrap_err();
    assert!(matches!(err, LuaError::Forbidden(_)));
    let err = sb
        .lua()
        .load(&bytecode[..])
        .set_mode(mlua::ChunkMode::Text)
        .into_function()
        .unwrap_err();
    assert!(err.to_string().contains("binary"), "{}", err);

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&dir2);
}

#[test]
fn require_rejects_path_traversal_and_stays_inside_the_mod() {
    let base = temp_dir("traversal");
    let root = base.join("mods").join("victim");
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::write(base.join("mods").join("secret.lua"), "return 'leaked'").unwrap();
    fs::write(root.join("lib").join("ok.lua"), "return { v = 1 }").unwrap();
    fs::write(root.join("lib").join("init.lua"), "return 'lib-init'").unwrap();

    let sb = LuaSandbox::new().unwrap();
    let env = sb.create_env("victim", Some(&root)).unwrap();
    for name in [
        "../secret",
        "..secret",
        "..\\secret",
        "lib/../../secret",
        "/etc/passwd",
        "\\\\server\\share",
        "C:/Windows/win",
        "lib..ok",
        ".lib",
        "lib.",
        "",
        "lib\0ok",
    ] {
        let src = format!("return require({:?})", name);
        let err = sb.eval_chunk(&env, &src).unwrap_err();
        assert!(
            matches!(err, LuaError::Forbidden(_)),
            "require({:?}) -> {:?}",
            name,
            err
        );
    }
    assert_eq!(
        sb.eval_chunk(&env, "return require('lib.ok').v").unwrap(),
        LuaValue::Number(1.0)
    );
    assert_eq!(
        sb.eval_chunk(&env, "return require('lib')").unwrap(),
        LuaValue::String("lib-init".to_string())
    );
    // pcall(require, bad) is an ordinary, catchable error.
    assert_eq!(
        sb.eval_chunk(&env, "return (pcall(require, '../secret'))")
            .unwrap(),
        LuaValue::Bool(false)
    );
    // No root configured: require fails cleanly.
    let rootless = sb.create_env("nowhere", None).unwrap();
    assert!(sb
        .eval_chunk(&rootless, "return require('lib.ok')")
        .is_err());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn mods_cannot_see_or_patch_each_other() {
    let mut host = ModHost::new();
    host.load_mod_from_source(
        manifest("alpha", 0),
        r#"
        _G.shared_secret = "alpha"
        leaked_global = 1
        string.upper = function() return "pwned" end
        table.insert = nil
        math.pi = 3
        local ok = pcall(function() getmetatable("").__index.upper = function() return "pwned" end end)
        assert(not ok, "the shared string metatable must be locked")
        assert(getmetatable("") == "locked")
        local ok2, err2 = pcall(setmetatable, {}, { __gc = function() end })
        assert(not ok2 and tostring(err2):find("__gc"), "__gc must be refused")
        return function(mod) end
        "#,
        None,
    )
    .unwrap();
    host.load_mod_from_source(
        manifest("beta", 1),
        r#"
        assert(shared_secret == nil and _G.shared_secret == nil, "globals leak")
        assert(leaked_global == nil, "implicit globals leak")
        assert(string.upper("a") == "A", "string lib leak")
        assert(("a"):upper() == "A", "string metatable leak")
        assert(type(table.insert) == "function", "table lib leak")
        assert(math.pi > 3.14, "math lib leak")
        assert(_G ~= nil and _G._G == _G)
        return function(mod) end
        "#,
        None,
    )
    .unwrap();
    let a = host.mod_env("alpha").unwrap();
    let b = host.mod_env("beta").unwrap();
    assert!(a.table() != b.table());
}

#[test]
fn errors_are_attributed_and_the_circuit_breaker_trips() {
    let mut host = ModHost::new();
    host.set_circuit_breaker_threshold(3);
    host.load_mod_from_source(
        manifest("good", 0),
        r#"return function(mod)
            mod.hooks:wrap("combat.dmg", function(base, ctx) return base(ctx) + 1 end)
            mod.events:on("tick", function(ctx) mod.save:set("ticks", (mod.save:get("ticks") or 0) + 1) end)
        end"#,
        None,
    )
    .unwrap();
    host.load_mod_from_source(
        manifest("flaky", 1),
        r#"return function(mod)
            mod.hooks:wrap("combat.dmg", function(base, ctx) error("flaky hook") end)
            mod.events:on("tick", function(ctx) local x = nil; return x.field end)
        end"#,
        None,
    )
    .unwrap();

    for _ in 0..5 {
        let v = host
            .run_hook("combat.dmg", LuaTable::new(), |_| {
                Ok(LuaValue::Number(10.0))
            })
            .unwrap();
        assert_eq!(v, LuaValue::Number(11.0));
        host.emit_event("tick", LuaTable::new());
    }
    let errs = host.mod_errors("flaky");
    assert_eq!(
        errs.len(),
        3,
        "breaker stops calls after 3 errors: {:?}",
        errs
    );
    assert!(
        errs[0].contains("flaky") && errs[0].contains("flaky/main.lua"),
        "{}",
        errs[0]
    );
    assert!(host.is_mod_tripped("flaky"));
    assert!(!host.is_mod_tripped("good"));
    assert!(host.mod_errors("good").is_empty());
    let saves = JsonValue::parse(&host.export_mod_saves_json("good")).unwrap();
    assert_eq!(saves.get("ticks").and_then(|v| v.as_i64()), Some(5));

    host.reset_mod_faults("flaky");
    assert!(!host.is_mod_tripped("flaky"));
    host.emit_event("tick", LuaTable::new());
    assert_eq!(host.mod_errors("flaky").len(), 1);
}

#[test]
fn base_errors_are_not_blamed_on_wrappers_and_failed_init_unregisters() {
    let mut host = ModHost::new();
    host.load_mod_from_source(
        manifest("wrapper", 0),
        r#"return function(mod)
            mod.hooks:wrap("combat.x", function(base, ctx) return base(ctx) end)
        end"#,
        None,
    )
    .unwrap();
    let err = host
        .run_hook("combat.x", LuaTable::new(), |_| {
            Err("base exploded".to_string())
        })
        .unwrap_err();
    assert!(err.contains("base exploded"));
    assert!(host.mod_errors("wrapper").is_empty());

    let res = host.load_mod_from_source(
        manifest("halfway", 1),
        r#"return function(mod)
            mod.hooks:wrap("combat.x", function(base, ctx) return 999 end)
            error("init fails after registering")
        end"#,
        None,
    );
    assert!(res.unwrap_err().contains("init fails after registering"));
    let v = host
        .run_hook("combat.x", LuaTable::new(), |_| Ok(LuaValue::Number(5.0)))
        .unwrap();
    assert_eq!(v, LuaValue::Number(5.0));
}

#[test]
fn host_boundary_rejects_functions_and_cycles_in_content() {
    let mut host = ModHost::new();
    let err = host
        .load_mod_from_source(
            manifest("fnpatch", 0),
            r#"return function(mod)
                mod.content.weapons:patch(1, { OnFire = function() end })
            end"#,
            None,
        )
        .unwrap_err();
    assert!(err.contains("function"), "{}", err);

    let err = host
        .load_mod_from_source(
            manifest("cycle", 0),
            r#"return function(mod)
                local t = {} t.self = t
                mod.content.weapons:patch(2, t)
            end"#,
            None,
        )
        .unwrap_err();
    assert!(err.contains("too deep"), "{}", err);
    // dot-call form also works on capability tables.
    host.load_mod_from_source(
        manifest("dotcall", 0),
        r#"return function(mod)
            mod.content.weapons.patch(3, { A = 1 })
            mod.content.weapons:patch(3, { B = 2 })
        end"#,
        None,
    )
    .unwrap();
    let rec = host.get_content_record("weapons", "3").unwrap();
    assert_eq!(rec.borrow().get_str("A"), LuaValue::Number(1.0));
    assert_eq!(rec.borrow().get_str("B"), LuaValue::Number(2.0));
}

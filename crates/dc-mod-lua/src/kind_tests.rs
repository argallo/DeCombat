//! Manifest kinds, declared capabilities and origin/policy admission
//! (HOSTING_AND_MODS.md sections 3 and 8). Every refusal test checks the reason, not just `is_err`.

use crate::host::{ModHost, ModOrigin, ModPolicy};
use crate::manifest::{AssetPolicy, ModKind, ModManifest};

fn parse(json: &str) -> Result<ModManifest, String> {
    ModManifest::from_json_str(json)
}

fn manifest(kind: &str, caps: &[&str]) -> ModManifest {
    let caps_json: Vec<String> = caps.iter().map(|c| format!("\"{}\"", c)).collect();
    parse(&format!(
        r#"{{"id": "m", "name": "M", "version": "1.0.0", "kind": "{}", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": [{}]}}"#,
        kind,
        caps_json.join(",")
    ))
    .unwrap_or_else(|e| panic!("manifest {} {:?}: {}", kind, caps, e))
}

fn load(host: &mut ModHost, origin: ModOrigin, m: ModManifest, src: &str) -> Result<(), String> {
    host.load_mod_from_source_as(origin, m, src, None)
}

// ---- manifest parsing -------------------------------------------------------------------------

#[test]
fn all_three_kinds_parse_with_derived_requires_client() {
    let s = manifest("server_rules", &["content.weapons", "hooks.lobby"]);
    assert_eq!(s.kind, ModKind::ServerRules);
    assert!(!s.requires_client);

    let sh = manifest("shared", &["hooks.hud", "hooks.render", "content.hud"]);
    assert_eq!(sh.kind, ModKind::Shared);
    assert!(sh.requires_client);

    let cv = manifest("client_visual", &["content.hud", "content.music", "assets"]);
    assert_eq!(cv.kind, ModKind::ClientVisual);
    assert!(cv.requires_client);
    assert_eq!(cv.asset_policy, AssetPolicy::OriginalOnly);
}

#[test]
fn explicit_requires_client_must_match_the_kind() {
    let ok =
        parse(r#"{"id":"m","name":"M","version":"1.0.0","kind":"shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],"requires_client":true}"#)
            .unwrap();
    assert!(ok.requires_client);

    let e = parse(
        r#"{"id":"m","name":"M","version":"1.0.0","kind":"server_rules", "description": "A test fixture mod for the unit tests.", "author": "tests","requires_client":true}"#,
    )
    .unwrap_err();
    assert!(
        e.contains("requires_client") && e.contains("server_rules"),
        "{}",
        e
    );
    let e = parse(
        r#"{"id":"m","name":"M","version":"1.0.0","kind":"client_visual", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],"requires_client":false}"#,
    )
    .unwrap_err();
    assert!(
        e.contains("requires_client") && e.contains("client_visual"),
        "{}",
        e
    );
    let e =
        parse(r#"{"id":"m","name":"M","version":"1.0.0","kind":"shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],"requires_client":"yes"}"#)
            .unwrap_err();
    assert!(e.contains("requires_client"), "{}", e);
}

#[test]
fn manifest_round_trips_through_json() {
    let m = manifest("shared", &["events", "save", "hooks.hud", "content.text"]);
    let again = parse(&m.to_json_pretty()).unwrap();
    assert_eq!(m, again);
    // Capabilities are normalised to sorted order.
    assert_eq!(
        again.capabilities,
        vec!["content.text", "events", "hooks.hud", "save"]
    );
}

#[test]
fn kind_is_required_and_validated() {
    let e = parse(r#"{"id":"m","name":"M","version":"1.0.0"}"#).unwrap_err();
    assert!(e.contains("missing required field 'kind'"), "{}", e);
    assert!(
        e.contains("server_rules"),
        "message should list the kinds: {}",
        e
    );
    let e = parse(r#"{"id":"m","name":"M","version":"1.0.0","kind":"total", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"]}"#).unwrap_err();
    assert!(e.contains("Unknown mod kind 'total'"), "{}", e);
    let e = parse(r#"{"id":"m","name":"M","version":"1.0.0","kind":7}"#).unwrap_err();
    assert!(e.contains("'kind' must be a string"), "{}", e);
}

#[test]
fn capability_and_asset_policy_validation() {
    let base = |extra: &str| {
        parse(&format!(
            r#"{{"id":"m","name":"M","version":"1.0.0","kind":"shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],{}}}"#,
            extra
        ))
    };
    let e = base(r#""capabilities":["content.weaponz"]"#).unwrap_err();
    assert!(e.contains("Unknown capability 'content.weaponz'"), "{}", e);
    assert!(
        e.contains("hooks."),
        "message should list the vocabulary: {}",
        e
    );
    let e = base(r#""capabilities":["hooks.net"]"#).unwrap_err();
    assert!(e.contains("Unknown capability 'hooks.net'"), "{}", e);
    let e = base(r#""capabilities":["filesystem"]"#).unwrap_err();
    assert!(e.contains("Unknown capability 'filesystem'"), "{}", e);
    let e = base(r#""capabilities":["save","save"]"#).unwrap_err();
    assert!(e.contains("more than once"), "{}", e);
    let e = base(r#""capabilities":"save""#).unwrap_err();
    assert!(e.contains("must be an array"), "{}", e);
    let e = base(r#""capabilities":[1]"#).unwrap_err();
    assert!(e.contains("must be strings"), "{}", e);
    let e = base(r#""asset_policy":"anything_goes""#).unwrap_err();
    assert!(
        e.contains("Unknown asset_policy 'anything_goes'") && e.contains("original_only"),
        "{}",
        e
    );
    assert_eq!(
        base(r#""asset_policy":"original_only""#)
            .unwrap()
            .asset_policy,
        AssetPolicy::OriginalOnly
    );
    // No capabilities declared is valid: the mod can then do nothing with the host.
    assert!(base(r#""name2":"x""#).unwrap().capabilities.is_empty());
}

#[test]
fn kind_restricts_which_capabilities_can_be_declared() {
    let sr = |c: &str| {
        parse(&format!(
            r#"{{"id":"m","name":"M","version":"1.0.0","kind":"server_rules", "description": "A test fixture mod for the unit tests.", "author": "tests","capabilities":["{}"]}}"#,
            c
        ))
    };
    for c in ["hooks.render", "hooks.hud"] {
        let e = sr(c).unwrap_err();
        assert!(e.contains(c) && e.contains("server_rules"), "{}", e);
    }
    assert!(sr("hooks.combat").is_ok());

    let cv = |c: &str| {
        parse(&format!(
            r#"{{"id":"m","name":"M","version":"1.0.0","kind":"client_visual", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],"capabilities":["{}"]}}"#,
            c
        ))
    };
    for c in [
        "hooks.lobby",
        "hooks.combat",
        "content.weapons",
        "content.shop",
        "content.gameroom",
    ] {
        let e = cv(c).unwrap_err();
        assert!(e.contains(c) && e.contains("client_visual"), "{}", e);
    }
    for c in [
        "hooks.hud",
        "hooks.render",
        "content.hud",
        "content.music",
        "content.text",
        "events",
        "save",
        "assets",
    ] {
        assert!(cv(c).is_ok(), "{}", c);
    }
}

// ---- host-side capability enforcement ---------------------------------------------------------

#[test]
fn declared_capabilities_work() {
    let mut host = ModHost::new();
    let m = manifest(
        "shared",
        &[
            "content.weapons",
            "hooks.combat",
            "hooks.hud",
            "events",
            "save",
            "assets",
        ],
    );
    load(
        &mut host,
        ModOrigin::Server,
        m,
        r#"return function(mod)
            mod.content.weapons:patch(1, { AmmoDamage = 5 })
            mod.hooks:wrap("combat.damage", function(base, ctx) return base(ctx) end)
            mod.hooks:on("hud.draw", function(base, ctx) return base(ctx) end)
            mod.events:on("round.start", function(ctx) end)
            mod.save:set("k", 1)
            local a = mod.assets:path("assets/skin.dtx")
        end"#,
    )
    .unwrap();
    assert!(host.get_content_record("weapons", "1").is_some());
}

#[test]
fn content_domain_outside_declared_capabilities_is_refused() {
    for call in [
        "mod.content.shop:patch(1, { Price = 1 })",
        "mod.content.shop:register({ id = 9 })",
        "mod.content.shop:override(1, 2)",
        "mod.content.shop:get(1)",
    ] {
        let mut host = ModHost::new();
        let m = manifest("server_rules", &["content.weapons"]);
        let src = format!("return function(mod) {} end", call);
        let e = load(&mut host, ModOrigin::Server, m, &src).unwrap_err();
        assert!(
            e.contains("capability denied")
                && e.contains("content.shop")
                && e.contains("does not declare"),
            "{} -> {}",
            call,
            e
        );
        assert!(host.load_order().is_empty(), "{} left a loaded mod", call);
        assert!(host.get_content_record("shop", "1").is_none());
    }
}

#[test]
fn hook_outside_declared_namespace_is_refused() {
    let mut host = ModHost::new();
    let m = manifest("server_rules", &["hooks.lobby"]);
    let e = load(
        &mut host,
        ModOrigin::Server,
        m,
        r#"return function(mod) mod.hooks:wrap("combat.damage", function(b, c) return b(c) end) end"#,
    )
    .unwrap_err();
    assert!(
        e.contains("capability denied") && e.contains("hooks.combat"),
        "{}",
        e
    );

    // The declared namespace still works, and `on` is gated the same way as `wrap`.
    let mut host = ModHost::new();
    let m = manifest("server_rules", &["hooks.lobby"]);
    load(
        &mut host,
        ModOrigin::Server,
        m,
        r#"return function(mod) mod.hooks:wrap("lobby.calc_exp_gp", function(b, c) return b(c) end) end"#,
    )
    .unwrap();
    let mut host = ModHost::new();
    let m = manifest("server_rules", &["hooks.lobby"]);
    let e = load(
        &mut host,
        ModOrigin::Server,
        m,
        r#"return function(mod) mod.hooks:on("combat.x", function(b, c) end) end"#,
    )
    .unwrap_err();
    assert!(e.contains("hooks.combat"), "{}", e);
}

#[test]
fn hook_names_without_a_known_namespace_are_refused() {
    for name in ["damage", "net.send", ".x", "combat."] {
        let mut host = ModHost::new();
        let m = manifest(
            "shared",
            &["hooks.combat", "hooks.lobby", "hooks.hud", "hooks.render"],
        );
        let src = format!(
            r#"return function(mod) mod.hooks:wrap("{}", function(b, c) return b(c) end) end"#,
            name
        );
        let e = load(&mut host, ModOrigin::Server, m, &src).unwrap_err();
        assert!(
            e.contains("capability denied") && e.contains("known namespace"),
            "{} -> {}",
            name,
            e
        );
    }
}

#[test]
fn events_save_and_assets_need_their_capability() {
    for (call, cap) in [
        (r#"mod.events:on("x", function() end)"#, "events"),
        (r#"mod.save:set("k", 1)"#, "save"),
        (r#"mod.save:get("k")"#, "save"),
        (r#"mod.assets:path("assets/a.dtx")"#, "assets"),
    ] {
        let mut host = ModHost::new();
        let m = manifest("shared", &["content.weapons"]);
        let src = format!("return function(mod) {} end", call);
        let e = load(&mut host, ModOrigin::Server, m, &src).unwrap_err();
        assert!(
            e.contains("capability denied") && e.contains(&format!("'{}'", cap)),
            "{} -> {}",
            call,
            e
        );
    }
}

#[test]
fn a_mod_cannot_swallow_a_capability_denial_with_pcall() {
    let mut host = ModHost::new();
    let m = manifest("server_rules", &["content.weapons"]);
    let e = load(
        &mut host,
        ModOrigin::Server,
        m,
        r#"return function(mod)
            pcall(function() mod.content.shop:patch(1, { Price = 1 }) end)
            mod.content.weapons:patch(1, { AmmoDamage = 5 })
        end"#,
    )
    .unwrap_err();
    assert!(
        e.contains("capability denied") && e.contains("content.shop"),
        "{}",
        e
    );
    assert!(host.load_order().is_empty());
}

#[test]
fn denied_registrations_leave_no_hook_behind() {
    let mut host = ModHost::new();
    let m = manifest("server_rules", &["hooks.lobby"]);
    let _ = load(
        &mut host,
        ModOrigin::Server,
        m,
        r#"return function(mod)
            mod.hooks:wrap("lobby.a", function(b, c) return 1 end)
            mod.hooks:wrap("combat.b", function(b, c) return 2 end)
        end"#,
    )
    .unwrap_err();
    let out = host
        .run_hook("lobby.a", crate::value::LuaTable::new(), |_| {
            Ok(crate::value::LuaValue::Number(0.0))
        })
        .unwrap();
    assert_eq!(out, crate::value::LuaValue::Number(0.0));
}

// ---- origin and policy ------------------------------------------------------------------------

const EMPTY_MOD: &str = "return function(mod) end";

#[test]
fn vanilla_policy_refuses_every_mod() {
    for (kind, origin) in [
        ("server_rules", ModOrigin::Server),
        ("shared", ModOrigin::Server),
        ("client_visual", ModOrigin::Server),
        ("client_visual", ModOrigin::Local),
    ] {
        let mut host = ModHost::new();
        host.set_policy(ModPolicy::Vanilla);
        let e = load(&mut host, origin, manifest(kind, &[]), EMPTY_MOD).unwrap_err();
        assert!(
            e.contains("vanilla") && e.contains("no mods"),
            "{} {:?} -> {}",
            kind,
            origin,
            e
        );
        assert!(host.load_order().is_empty());
    }
}

#[test]
fn render_and_hud_capabilities_need_a_server_declared_mod() {
    let m = || manifest("client_visual", &["hooks.hud", "content.hud"]);
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Modded {
        client_visual_allowed: true,
    });
    let e = load(&mut host, ModOrigin::Local, m(), EMPTY_MOD).unwrap_err();
    assert!(
        e.contains("hooks.hud") && e.contains("declared by the server"),
        "{}",
        e
    );

    let m2 = manifest("client_visual", &["hooks.render"]);
    let e = load(&mut host, ModOrigin::Local, m2, EMPTY_MOD).unwrap_err();
    assert!(e.contains("hooks.render"), "{}", e);

    // Same manifest from the server's mod set loads.
    load(&mut host, ModOrigin::Server, m(), EMPTY_MOD).unwrap();
    assert_eq!(host.load_order(), vec!["m"]);
}

#[test]
fn local_mods_must_be_client_visual_and_allowed_by_policy() {
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Modded {
        client_visual_allowed: true,
    });
    for kind in ["server_rules", "shared"] {
        let e = load(&mut host, ModOrigin::Local, manifest(kind, &[]), EMPTY_MOD).unwrap_err();
        assert!(
            e.contains("local mod must be kind 'client_visual'"),
            "{} -> {}",
            kind,
            e
        );
    }
    // A purely cosmetic local mod is fine where the server allows it.
    load(
        &mut host,
        ModOrigin::Local,
        manifest("client_visual", &["content.hud", "assets"]),
        r#"return function(mod) mod.content.hud:override("crosshair", mod.assets:path("assets/c.dtx")) end"#,
    )
    .unwrap();

    let mut strict = ModHost::new();
    strict.set_policy(ModPolicy::Modded {
        client_visual_allowed: false,
    });
    let e = load(
        &mut strict,
        ModOrigin::Local,
        manifest("client_visual", &[]),
        EMPTY_MOD,
    )
    .unwrap_err();
    assert!(
        e.contains("does not allow local client_visual mods"),
        "{}",
        e
    );
}

#[test]
fn directory_loading_applies_the_origin() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("dc_kind_dir_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let moddir = dir.join("hudmod");
    fs::create_dir_all(&moddir).unwrap();
    fs::write(
        moddir.join("manifest.json"),
        r#"{"id":"hudmod","name":"H","version":"1.0.0","kind":"client_visual", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],"capabilities":["hooks.hud"]}"#,
    )
    .unwrap();
    fs::write(moddir.join("main.lua"), EMPTY_MOD).unwrap();

    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Modded {
        client_visual_allowed: true,
    });
    let e = host
        .discover_and_load_dir_as(ModOrigin::Local, &dir)
        .unwrap_err();
    assert!(e.contains("hooks.hud"), "{}", e);
    let ids = host
        .discover_and_load_dir_as(ModOrigin::Server, &dir)
        .unwrap();
    assert_eq!(ids, vec!["hudmod"]);
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- browser metadata

fn meta(kind: &str, extra: &str) -> Result<ModManifest, String> {
    parse(&format!(
        r#"{{"id":"m","name":"M","version":"1.0.0","kind":"{}"{}}}"#,
        kind, extra
    ))
}

const DESC: &str = r#","description":"Doubles the EXP of every match for testing.","author":"me""#;

#[test]
fn description_and_author_are_required() {
    let e = meta("server_rules", r#","author":"me""#).unwrap_err();
    assert!(
        e.contains("missing required string field 'description'"),
        "{e}"
    );
    let e = meta(
        "server_rules",
        r#","description":"Doubles the EXP of every match.""#,
    )
    .unwrap_err();
    assert!(e.contains("missing required string field 'author'"), "{e}");
    let e = meta(
        "server_rules",
        r#","description":"too short","author":"me""#,
    )
    .unwrap_err();
    assert!(
        e.contains("'description' is 9 characters; it must be 20 to 500"),
        "{e}"
    );
    let long = "x".repeat(501);
    let e = meta(
        "server_rules",
        &format!(r#","description":"{long}","author":"me""#),
    )
    .unwrap_err();
    assert!(e.contains("501 characters"), "{e}");
    let e = meta(
        "server_rules",
        r#","description":"Doubles the EXP of every match.","author":"  ""#,
    )
    .unwrap_err();
    assert!(e.contains("'author' must be 1 to 100"), "{e}");
}

#[test]
fn server_rules_needs_no_screenshot_but_the_client_kinds_do() {
    let m = meta("server_rules", DESC).unwrap();
    assert!(m.screenshots.is_empty() && m.tags.is_empty() && m.homepage.is_none());
    for kind in ["shared", "client_visual"] {
        let e = meta(kind, DESC).unwrap_err();
        assert!(
            e.contains("needs at least one image for kind"),
            "{kind}: {e}"
        );
        let ok = meta(
            kind,
            &format!(r#"{DESC},"screenshots":["screenshots/a.png","screenshots/B-2.JPG"]"#),
        )
        .unwrap();
        assert_eq!(ok.screenshots.len(), 2);
    }
}

#[test]
fn bad_screenshot_paths_and_counts_are_refused() {
    for bad in [
        "shots/a.png",
        "screenshots/a.gif",
        "screenshots/../a.png",
        "screenshots/sub/a.png",
        "/screenshots/a.png",
        "screenshots/.png",
        "screenshots/a b.png",
    ] {
        let e = meta(
            "server_rules",
            &format!(r#"{DESC},"screenshots":["{bad}"]"#),
        )
        .unwrap_err();
        assert!(e.contains("Invalid screenshot path"), "{bad}: {e}");
    }
    let seven: Vec<String> = (0..7).map(|i| format!("\"screenshots/{i}.png\"")).collect();
    let e = meta(
        "shared",
        &format!(r#"{DESC},"screenshots":[{}]"#, seven.join(",")),
    )
    .unwrap_err();
    assert!(e.contains("at most 6"), "{e}");
    let e = meta(
        "shared",
        &format!(r#"{DESC},"screenshots":["screenshots/a.png","screenshots/a.png"]"#),
    )
    .unwrap_err();
    assert!(e.contains("more than once"), "{e}");
}

#[test]
fn tags_homepage_and_changelog() {
    let m = meta(
        "server_rules",
        &format!(r#"{DESC},"tags":["exp","fun-2"],"homepage":"https://example.org/m","changelog":"1.0.0: first""#),
    )
    .unwrap();
    assert_eq!(m.tags, vec!["exp".to_string(), "fun-2".to_string()]);
    assert_eq!(m.homepage.as_deref(), Some("https://example.org/m"));
    assert_eq!(m.changelog.as_deref(), Some("1.0.0: first"));
    // the JSON form round-trips
    assert_eq!(ModManifest::from_json_value(&m.to_json_value()).unwrap(), m);
    for bad in [r#"["Exp"]"#, r#"["a b"]"#, r#"[""]"#, r#"["a","a"]"#] {
        let e = meta("server_rules", &format!(r#"{DESC},"tags":{bad}"#)).unwrap_err();
        assert!(e.contains("tag"), "{bad}: {e}");
    }
    let nine: Vec<String> = (0..9).map(|i| format!("\"t{i}\"")).collect();
    assert!(meta(
        "server_rules",
        &format!(r#"{DESC},"tags":[{}]"#, nine.join(","))
    )
    .is_err());
    for bad in ["ftp://x", "https://", "example.org", "https://a b"] {
        let e = meta("server_rules", &format!(r#"{DESC},"homepage":"{bad}""#)).unwrap_err();
        assert!(e.contains("'homepage'"), "{bad}: {e}");
    }
}

// ---- CLI-1613: the HUD override vocabulary ----------------------------------------------------

#[test]
fn an_unknown_hud_key_or_a_bad_value_refuses_the_mod_even_under_pcall() {
    let caps = &["content.hud", "assets"];
    let mut host = ModHost::new();
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest("client_visual", caps),
        r#"return function(mod) pcall(function() mod.content.hud:override("radar_alpha", 1) end) end"#,
    )
    .unwrap_err();
    assert!(
        e.contains("Unknown HUD override key 'radar_alpha'")
            && e.contains("crosshair, crosshair_colour, hitmarker, hud_scale"),
        "{}",
        e
    );
    let e = load(
        &mut ModHost::new(),
        ModOrigin::Server,
        manifest("client_visual", caps),
        r#"return function(mod) mod.content.hud:override("hud_scale", 3) end"#,
    )
    .unwrap_err();
    assert!(
        e.contains("'hud_scale' must be a number from 0.5 to 2"),
        "{}",
        e
    );
    let mut ok = ModHost::new();
    load(
        &mut ok,
        ModOrigin::Server,
        manifest("client_visual", caps),
        r##"return function(mod)
            mod.content.hud:override("crosshair", mod.assets:path("assets/x.dtx"))
            mod.content.hud:override("crosshair_colour", "#00ff00")
        end"##,
    )
    .unwrap();
    let ov = ok.content_overrides("hud");
    assert_eq!(
        ov.keys().cloned().collect::<Vec<_>>(),
        vec!["crosshair", "crosshair_colour"]
    );
}

// MOD-22: world.full_visibility turns the match's culling off, so it is a server-declared capability
#[test]
fn full_visibility_is_for_shared_and_server_rules_only() {
    assert!(
        manifest("shared", &["hooks.render", "world.full_visibility"])
            .has_capability("world.full_visibility")
    );
    assert!(manifest("server_rules", &["world.full_visibility"])
        .has_capability("world.full_visibility"));
    let e = parse(
        r#"{"id":"m","name":"M","version":"1.0.0","kind":"client_visual", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],"capabilities":["world.full_visibility"]}"#,
    )
    .unwrap_err();
    assert!(
        e.contains("world.full_visibility") && e.contains("client_visual"),
        "{}",
        e
    );
}

// MOD-23: combat.no_recoil lets a room turn recoil off, so like world.full_visibility it is server-declared
#[test]
fn no_recoil_is_for_shared_and_server_rules_only() {
    assert!(manifest("shared", &["combat.no_recoil"]).has_capability("combat.no_recoil"));
    assert!(manifest(
        "server_rules",
        &["world.full_visibility", "combat.no_recoil"]
    )
    .has_capability("combat.no_recoil"));
    let e = parse(
        r#"{"id":"m","name":"M","version":"1.0.0","kind":"client_visual", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],"capabilities":["combat.no_recoil"]}"#,
    )
    .unwrap_err();
    assert!(
        e.contains("combat.no_recoil") && e.contains("client_visual"),
        "{}",
        e
    );
}
// ---- MAP-4 / MAP-5: the maps content domain -----------------------------------------------------

const MAP_CAPS: &[&str] = &["content.maps", "assets"];

fn manifest_id(id: &str, kind: &str, caps: &[&str]) -> ModManifest {
    let caps_json: Vec<String> = caps.iter().map(|c| format!("\"{}\"", c)).collect();
    parse(&format!(
        r#"{{"id": "{id}", "name": "M", "version": "1.0.0", "kind": "{kind}", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": [{}]}}"#,
        caps_json.join(",")
    ))
    .unwrap()
}

const VALLEY: &str = r#"return function(mod)
    mod.content.maps:register({
        id = "valley",
        name = "Crossfire Valley",
        description = "Two bases, one valley.",
        world = mod.assets:path("assets/maps/valley.dat"),
        modes = { "TDM", "FFA", "BOMB" },
        goals = { TDM = 100, FFA = 50 },
        round_seconds = 600,
        picture = mod.assets:path("assets/pictures/valley.dtx"),
        loading = mod.assets:path("assets/pictures/valley_load.dtx"),
        radar = { picture = mod.assets:path("assets/pictures/valley_radar.dtx"),
                  min = { -2048, -2048 }, max = { 2048, 2048 } },
        players = { min = 2, max = 16 },
    })
end"#;

#[test]
fn content_maps_is_for_shared_and_server_rules_only() {
    assert!(manifest("shared", MAP_CAPS).has_capability("content.maps"));
    assert!(manifest("server_rules", &["content.maps"]).has_capability("content.maps"));
    let e = parse(
        r#"{"id":"m","name":"M","version":"1.0.0","kind":"client_visual", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"],"capabilities":["content.maps"]}"#,
    )
    .unwrap_err();
    assert!(
        e.contains("content.maps") && e.contains("client_visual"),
        "{}",
        e
    );
    // the domain exists for the capability check and the stubs
    assert!(crate::CONTENT_DOMAINS.contains(&"maps"));
    // and a mod without the capability is denied at the call
    let e = load(
        &mut ModHost::new(),
        ModOrigin::Server,
        manifest("shared", &["assets"]),
        VALLEY,
    )
    .unwrap_err();
    assert!(
        e.contains("capability denied") && e.contains("content.maps"),
        "{}",
        e
    );
}

#[test]
fn a_map_record_round_trips_through_register() {
    let mut host = ModHost::new();
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("valley_mod", "shared", MAP_CAPS),
        VALLEY,
    )
    .unwrap();
    let recs = host.map_records().unwrap();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    assert_eq!(r.mod_id, "valley_mod");
    assert!(r.patched_by.is_empty());
    assert_eq!(r.id, "valley");
    assert_eq!(r.name, "Crossfire Valley");
    assert_eq!(r.description.as_deref(), Some("Two bases, one valley."));
    assert_eq!(r.world, "@mod:valley_mod/assets/maps/valley.dat");
    assert_eq!(r.world_rel_path(), Some("assets/maps/valley.dat"));
    assert_eq!(r.modes, vec!["TDM", "FFA", "BOMB"]);
    assert_eq!(
        r.goals,
        vec![("FFA".to_string(), 50), ("TDM".to_string(), 100)]
    );
    assert_eq!(r.round_seconds, Some(600));
    assert_eq!(
        r.picture.as_deref(),
        Some("@mod:valley_mod/assets/pictures/valley.dtx")
    );
    assert_eq!(
        r.loading.as_deref(),
        Some("@mod:valley_mod/assets/pictures/valley_load.dtx")
    );
    let radar = r.radar.as_ref().unwrap();
    assert_eq!(
        radar.picture,
        "@mod:valley_mod/assets/pictures/valley_radar.dtx"
    );
    assert_eq!(
        (radar.min, radar.max),
        ([-2048.0, -2048.0], [2048.0, 2048.0])
    );
    assert_eq!(r.players, Some(crate::PlayerRange { min: 2, max: 16 }));
    // the generic record is there too, under the map key
    assert!(host.get_content_record("maps", "valley").is_some());
}

#[test]
fn a_bad_map_record_refuses_the_mod_with_the_rule_in_the_message_even_under_pcall() {
    let refused = |src: &str| {
        load(
            &mut ModHost::new(),
            ModOrigin::Server,
            manifest_id("m", "shared", MAP_CAPS),
            src,
        )
        .unwrap_err()
    };
    let register =
        |body: &str| format!("return function(mod) mod.content.maps:register({{ {body} }}) end");
    let ok_fields = r#"id = "valley", name = "Valley", world = mod.assets:path("assets/maps/valley.dat"), modes = { "TDM" }"#;

    let e = refused(&register(&format!("{ok_fields}, colour = 1")));
    assert!(
        e.contains("Mod 'm': mod.content.maps:register: unknown key 'colour'")
            && e.contains("the keys are: id, name, world, modes, description"),
        "{e}"
    );
    let e = refused(&register(
        r#"id = "valley", name = "Valley", modes = { "TDM" }"#,
    ));
    assert!(
        e.contains("missing required key 'world'; the required keys are: id, name, world, modes"),
        "{e}"
    );
    let e = refused(&register(
        r#"id = "Valley Map", name = "Valley", world = mod.assets:path("assets/maps/valley.dat"), modes = { "TDM" }"#,
    ));
    assert!(
        e.contains("'id' must be the map key: 1 to 32 characters of a-z 0-9 _"),
        "{e}"
    );
    let e = refused(&register(
        r#"id = "valley", name = "", world = mod.assets:path("assets/maps/valley.dat"), modes = { "TDM" }"#,
    ));
    assert!(e.contains("'name' must not be empty"), "{e}");
    // the world must be the mod's own asset under assets/, a .dat
    let e = refused(&register(
        r#"id = "valley", name = "Valley", world = "@mod:other/assets/maps/valley.dat", modes = { "TDM" }"#,
    ));
    assert!(
        e.contains("'world' must be this mod's own asset @mod:m/assets/....dat"),
        "{e}"
    );
    let e = refused(&register(
        r#"id = "valley", name = "Valley", world = "Worlds/valley.dat", modes = { "TDM" }"#,
    ));
    assert!(e.contains("'world' must be this mod's own asset"), "{e}");
    let e = refused(&register(
        r#"id = "valley", name = "Valley", world = mod.assets:path("maps/valley.dat"), modes = { "TDM" }"#,
    ));
    assert!(e.contains("'world' must be this mod's own asset"), "{e}");
    let e = refused(&register(
        r#"id = "valley", name = "Valley", world = mod.assets:path("assets/maps/valley.lua"), modes = { "TDM" }"#,
    ));
    assert!(e.contains("'world' must be this mod's own asset"), "{e}");
    // modes
    let e = refused(&register(
        r#"id = "valley", name = "Valley", world = mod.assets:path("assets/maps/valley.dat"), modes = {}"#,
    ));
    assert!(e.contains("'modes' must be a non-empty list of mode names; the names are: TDM, TDM_CLAN, COOP, FFA, ELIM, ELIM_CLAN, FLAG, FLAG_CLAN, BOMB, BOMB_CLAN, INFECT, BOUNTY"), "{e}");
    let e = refused(&register(
        r#"id = "valley", name = "Valley", world = mod.assets:path("assets/maps/valley.dat"), modes = { "TDM", "Zombie" }"#,
    ));
    assert!(
        e.contains("'modes' entry 2 \"Zombie\" is not a mode name"),
        "{e}"
    );
    let e = refused(&register(
        r#"id = "valley", name = "Valley", world = mod.assets:path("assets/maps/valley.dat"), modes = { "TDM", "TDM" }"#,
    ));
    assert!(e.contains("'modes' lists \"TDM\" twice"), "{e}");
    // optional fields
    let e = refused(&register(&format!("{ok_fields}, goals = {{ BOMB = 3 }}")));
    assert!(
        e.contains("'goals' names mode \"BOMB\", which 'modes' does not list"),
        "{e}"
    );
    let e = refused(&register(&format!("{ok_fields}, goals = {{ TDM = -1 }}")));
    assert!(
        e.contains("'goals.TDM' must be a positive whole number"),
        "{e}"
    );
    let e = refused(&register(&format!("{ok_fields}, round_seconds = 10")));
    assert!(
        e.contains("'round_seconds' must be a whole number from 30 to 3600"),
        "{e}"
    );
    let e = refused(&register(&format!(
        "{ok_fields}, description = \"{}\"",
        "d".repeat(401)
    )));
    assert!(
        e.contains("'description' is 401 characters; it must be at most 400"),
        "{e}"
    );
    let e = refused(&register(&format!("{ok_fields}, picture = 7")));
    assert!(e.contains("'picture' must be a texture path string"), "{e}");
    let e = refused(&register(&format!(
        "{ok_fields}, radar = {{ picture = \"p\", min = {{ 0, 0 }} }}"
    )));
    assert!(
        e.contains("'radar.max' must be a table { x, z } of two numbers"),
        "{e}"
    );
    let e = refused(&register(&format!(
        "{ok_fields}, radar = {{ picture = \"p\", min = {{ 0, 0 }}, max = {{ 0, 9 }} }}"
    )));
    assert!(
        e.contains("'radar' min { 0, 0 } must be below max { 0, 9 } on both axes"),
        "{e}"
    );
    let e = refused(&register(&format!(
        "{ok_fields}, players = {{ min = 9, max = 2 }}"
    )));
    assert!(e.contains("'players' min 9 must not be above max 2"), "{e}");
    let e = refused(&register(&format!(
        "{ok_fields}, players = {{ min = 0, max = 2 }}"
    )));
    assert!(
        e.contains("'players.min' must be a whole number from 1 to 16"),
        "{e}"
    );
    // :override is not a way around the check; a pcall does not swallow a refusal
    let e = refused(
        r#"return function(mod) pcall(function() mod.content.maps:override("valley", { id = "valley" }) end) end"#,
    );
    assert!(
        e.contains("mod.content.maps:override is not supported"),
        "{e}"
    );
    let e = refused(&format!("return function(mod) pcall(function() mod.content.maps:register({{ {ok_fields}, colour = 1 }}) end) end"));
    assert!(e.contains("unknown key 'colour'"), "{e}");
    // a patch of a map nobody registered
    let e = refused(r#"return function(mod) mod.content.maps:patch("valley", { name = "V" }) end"#);
    assert!(
        e.contains(
            "mod.content.maps:patch: patches map \"valley\", which no loaded mod registered"
        ),
        "{e}"
    );
    // the same key twice in one set (MAP-5)
    let e = refused(&format!("return function(mod) mod.content.maps:register({{ {ok_fields} }}) mod.content.maps:register({{ {ok_fields} }}) end"));
    assert!(
        e.contains("map \"valley\" is already registered by mod 'm'"),
        "{e}"
    );
}

#[test]
fn map_records_come_in_load_order_then_key_order_and_patches_merge_into_the_owners_record() {
    let mut host = ModHost::new();
    // mod a: two maps, registered out of key order
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("a", "shared", MAP_CAPS),
        r#"return function(mod)
            mod.content.maps:register({ id = "zeta", name = "Zeta", world = mod.assets:path("assets/maps/zeta.dat"), modes = { "FFA" } })
            mod.content.maps:register({ id = "alpha", name = "Alpha", world = mod.assets:path("assets/maps/alpha.dat"), modes = { "TDM" } })
        end"#,
    )
    .unwrap();
    // mod b: one own map and a patch of a's zeta (a deep merge: goals are added, the world stays a's asset)
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("b", "server_rules", &["content.maps"]),
        r#"return function(mod)
            mod.content.maps:register({ id = "beta", name = "Beta", world = "@mod:b/assets/maps/beta.dat", modes = { "BOMB" } })
            mod.content.maps:patch("zeta", { name = "Zeta (night)", goals = { FFA = 30 }, round_seconds = 120 })
        end"#,
    )
    .unwrap();
    let recs = host.map_records().unwrap();
    let keys: Vec<(&str, &str)> = recs
        .iter()
        .map(|r| (r.mod_id.as_str(), r.id.as_str()))
        .collect();
    assert_eq!(keys, vec![("a", "alpha"), ("a", "zeta"), ("b", "beta")]);
    let zeta = &recs[1];
    assert_eq!(zeta.name, "Zeta (night)");
    assert_eq!(zeta.world, "@mod:a/assets/maps/zeta.dat");
    assert_eq!(zeta.goals, vec![("FFA".to_string(), 30)]);
    assert_eq!(zeta.round_seconds, Some(120));
    assert_eq!(zeta.patched_by, vec!["b"]);
    assert!(recs[0].patched_by.is_empty());

    // a third mod may not re-register a's key, and a patch that breaks the record is refused
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest_id("c", "shared", MAP_CAPS),
        r#"return function(mod) mod.content.maps:register({ id = "alpha", name = "Mine", world = mod.assets:path("assets/maps/alpha.dat"), modes = { "TDM" } }) end"#,
    )
    .unwrap_err();
    assert!(
        e.contains("map \"alpha\" is already registered by mod 'a'"),
        "{e}"
    );
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest_id("d", "shared", MAP_CAPS),
        r#"return function(mod) mod.content.maps:patch("alpha", { world = mod.assets:path("assets/maps/alpha.dat") }) end"#,
    )
    .unwrap_err();
    assert!(
        e.contains("mod.content.maps:patch: 'world' must be this mod's own asset @mod:a/assets/"),
        "{e}"
    );
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest_id("e", "shared", MAP_CAPS),
        r#"return function(mod) mod.content.maps:patch("alpha", { id = "alpha2" }) end"#,
    )
    .unwrap_err();
    assert!(e.contains("a patch may not change 'id'"), "{e}");
    // the refused mods left the table as it was
    assert_eq!(host.map_records().unwrap().len(), 3);
    assert_eq!(host.map_records().unwrap()[0].name, "Alpha");
}

/// MOD-24: `mod.settings` holds the checked values and refuses writes; a bad stored value reads as the default.
#[test]
fn mod_settings_are_read_only_and_checked_against_the_declaration() {
    let mut m = manifest("client_visual", &["content.text"]);
    m.id = "look".into();
    m.settings = crate::parse_settings(Some(
        &crate::JsonValue::parse(
            r##"[{"key":"c","label":"C","type":"colour","default":"#112233"},
                 {"key":"on","label":"On","type":"bool","default":true}]"##,
        )
        .unwrap(),
    ))
    .unwrap();
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Modded {
        client_visual_allowed: true,
    });
    let mut stored = std::collections::BTreeMap::new();
    stored.insert("c".to_string(), "garbage".to_string());
    stored.insert("on".to_string(), "false".to_string());
    host.set_setting_values([("look".to_string(), stored)].into_iter().collect());
    host.load_mod_from_source_as(
        ModOrigin::Local,
        m,
        r#"return function(mod)
            local c, on = mod.settings.c, mod.settings.on
            local wrote = pcall(function() mod.settings.c = "x" end)
            local n = 0
            for _ in pairs(mod.settings) do n = n + 1 end
            local mt = pcall(setmetatable, mod.settings, {})
            local line = c .. "," .. tostring(on) .. "," .. tostring(wrote) .. "," .. n .. "," .. tostring(mt) .. "," .. mod.settings.c
            print(line)
        end"#,
        None,
    )
    .unwrap();
    let logs = host.mod_logs("look");
    assert!(
        logs.iter()
            .any(|l| l.contains("#112233,false,false,2,false,#112233")),
        "{logs:?}"
    );
}

#[test]
fn settings_belong_to_client_visual_mods_only() {
    let mut m = manifest("shared", &["content.text"]);
    m.settings = crate::parse_settings(Some(
        &crate::JsonValue::parse(r#"[{"key":"on","label":"On","type":"bool","default":true}]"#)
            .unwrap(),
    ))
    .unwrap();
    let e = m.check_consistency().unwrap_err();
    assert!(e.contains("only for kind 'client_visual'"), "{e}");
}

/// MOD-27: `commands` is text for the panel: read, written back, and every bad line refused with its reason.
#[test]
fn the_commands_list_is_checked_and_round_trips() {
    let with = |list: &str| {
        parse(&format!(
            r#"{{"id": "m", "name": "M", "version": "1.0.0", "kind": "server_rules", "description": "A test fixture mod for the unit tests.", "author": "tests", "commands": {list}}}"#
        ))
    };
    let m = with(r#"[{"command": "/go-1", "description": "Goes."}]"#).unwrap();
    assert_eq!(m.commands[0].command, "/go-1");
    let back = parse(&m.to_json_pretty()).unwrap();
    assert_eq!(back.commands, m.commands);
    assert!(with("[]").unwrap().commands.is_empty());
    assert!(!manifest("server_rules", &["hooks.lobby"])
        .to_json_pretty()
        .contains("commands"));
    for (bad, why) in [
        (r#""/go""#, "must be an array"),
        (r#"[{"command": "/go"}]"#, "need the strings"),
        (
            r#"[{"command": "go", "description": "d"}]"#,
            "Invalid command",
        ),
        (
            r#"[{"command": "/", "description": "d"}]"#,
            "Invalid command",
        ),
        (
            r#"[{"command": "/Go", "description": "d"}]"#,
            "Invalid command",
        ),
        (
            r#"[{"command": "/go now", "description": "d"}]"#,
            "Invalid command",
        ),
        (
            r#"[{"command": "/go", "description": " "}]"#,
            "Invalid command",
        ),
        (
            r#"[{"command": "/go", "description": "a\nb"}]"#,
            "Invalid command",
        ),
        (
            r#"[{"command": "/go", "description": "d"}, {"command": "/go", "description": "e"}]"#,
            "more than once",
        ),
    ] {
        let e = with(bad).unwrap_err();
        assert!(e.contains(why), "{bad}: {e}");
    }
    let long = format!(
        r#"[{{"command": "/go", "description": "{}"}}]"#,
        "x".repeat(121)
    );
    assert!(with(&long).unwrap_err().contains("Invalid command"));
    let many: Vec<String> = (0..17)
        .map(|i| format!(r#"{{"command": "/c{i}", "description": "d"}}"#))
        .collect();
    assert!(with(&format!("[{}]", many.join(",")))
        .unwrap_err()
        .contains("at most"));
}

// LG-313: the curated limit of a featured server entry, at load
#[test]
fn the_curated_policy_takes_the_allow_list_only() {
    assert_eq!(
        crate::CURATED_CAPABILITIES,
        [
            "content.weapons",
            "content.text",
            "content.hud",
            "content.maps",
            "assets"
        ]
    );
    for cap in crate::CURATED_CAPABILITIES {
        assert_eq!(
            crate::curated_refusal(&manifest("shared", &[cap])),
            None,
            "{cap}"
        );
    }
    for cap in [
        "hooks.lobby",
        "hooks.combat",
        "hooks.render",
        "hooks.hud",
        "content.shop",
        "content.items",
        "content.gameroom",
        "world.full_visibility",
        "combat.no_recoil",
        "save",
        "events",
    ] {
        let why = crate::curated_refusal(&manifest("shared", &[cap])).unwrap();
        assert!(why.contains(cap), "{why}");
    }
    assert!(
        crate::curated_refusal(&manifest("server_rules", &["content.weapons"]))
            .unwrap()
            .contains("kind")
    );
    assert!(
        crate::curated_refusal(&manifest("client_visual", &["content.text"]))
            .unwrap()
            .contains("kind")
    );
}

/// LG-426: a curated entry that allows local mods loads a local client_visual one and still refuses a local shared
/// mod, a local render hook and a server pack outside the allow list; without the flag nothing local loads.
#[test]
fn a_curated_entry_that_allows_local_mods_loads_only_local_client_visual_ones() {
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Curated {
        client_visual_allowed: true,
    });
    load(
        &mut host,
        ModOrigin::Local,
        manifest_id("cross", "client_visual", &["content.hud"]),
        EMPTY_MOD,
    )
    .unwrap();
    let e = load(
        &mut host,
        ModOrigin::Local,
        manifest_id("loc", "shared", &["content.text"]),
        EMPTY_MOD,
    )
    .unwrap_err();
    assert!(e.contains("local mod must be kind 'client_visual'"), "{e}");
    let e = load(
        &mut host,
        ModOrigin::Local,
        manifest_id("hook", "client_visual", &["hooks.render"]),
        EMPTY_MOD,
    )
    .unwrap_err();
    assert!(e.contains("hooks.render"), "{e}");
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest_id("shop", "shared", &["content.shop"]),
        EMPTY_MOD,
    )
    .unwrap_err();
    assert!(e.contains("content.shop"), "{e}");
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest_id("srvcv", "client_visual", &["content.text"]),
        EMPTY_MOD,
    )
    .unwrap_err();
    assert!(e.contains("kind"), "{e}");
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("pack", "shared", &["content.text"]),
        EMPTY_MOD,
    )
    .unwrap();
    assert_eq!(host.load_order(), vec!["cross", "pack"]);

    let mut strict = ModHost::new();
    strict.set_policy(ModPolicy::Curated {
        client_visual_allowed: false,
    });
    let e = load(
        &mut strict,
        ModOrigin::Local,
        manifest_id("cross", "client_visual", &["content.hud"]),
        EMPTY_MOD,
    )
    .unwrap_err();
    assert!(e.contains("loads no local mods"), "{e}");
}

#[test]
fn a_new_weapon_row_of_a_curated_pack_stays_in_the_reserved_numbers() {
    use dc_bute::modpatch::{PatchOp, WeaponContent, WeaponPatch};
    let row = |weapon, op| WeaponPatch {
        weapon,
        op,
        fields: Default::default(),
    };
    let c = |r| WeaponContent::new(vec![r], Vec::new()).unwrap();
    assert_eq!(
        crate::curated_content_refusal(&c(row(900, PatchOp::Register))),
        None
    );
    assert_eq!(
        crate::curated_content_refusal(&c(row(999, PatchOp::Register))),
        None
    );
    assert_eq!(
        crate::curated_content_refusal(&c(row(145, PatchOp::Patch))),
        None,
        "an existing gun may be patched"
    );
    assert!(crate::curated_content_refusal(&c(row(500, PatchOp::Register))).is_some());
}

/// A mod whose entry fails after registering content leaves nothing behind (review C9): its map record and
/// key are gone, its patches of other mods' records are undone, `map_records` lists the loaded mods only
/// and a later mod may register the key.
#[test]
fn a_mod_that_fails_after_registering_a_map_leaves_no_record_and_frees_its_key() {
    use std::fs;
    let dir = std::env::temp_dir().join(format!("dc_kind_failmap_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let write_mod = |id: &str, caps: &str, body: &str| {
        let m = dir.join(id);
        fs::create_dir_all(m.join("assets/maps")).unwrap();
        fs::write(
            m.join("manifest.json"),
            format!(
                r#"{{"id": "{id}", "name": "{id}", "version": "1.0.0", "entry": "main.lua", "kind": "shared", "description": "A test fixture mod for the unit tests.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": [{caps}]}}"#
            ),
        )
        .unwrap();
        fs::write(
            m.join("main.lua"),
            format!(
                r#"return function(mod)
                    mod.content.maps:register({{ id = "{id}", name = "fixture {id}", world = mod.assets:path("assets/maps/{id}.dat"), modes = {{ "TDM" }} }})
                    {body}
                end"#
            ),
        )
        .unwrap();
    };
    write_mod("a", r#""content.maps", "assets""#, "");
    // b registers its map, patches a's map and a weapon row, registers a weapon row, then fails
    write_mod(
        "b",
        r#""content.maps", "assets", "content.weapons""#,
        r#"mod.content.maps:patch("a", { name = "a (night)" })
           mod.content.weapons:patch(1, { AmmoDamage = 5 })
           mod.content.weapons:register({ id = 7, Name = "fixture" })
           error('x')"#,
    );
    let mut host = ModHost::new();
    let e = host.discover_and_load_dir(&dir).unwrap_err();
    assert!(e.contains("Mod 'b' initialization failed"), "{e}");
    let loaded: Vec<String> = host.loaded_mods().into_iter().map(|(m, _)| m.id).collect();
    assert_eq!(loaded, vec!["a"]);

    // b's map is gone and a's record is as a registered it; the weapon rows b touched are gone too
    let recs = host.map_records().unwrap();
    let keys: Vec<(&str, &str)> = recs
        .iter()
        .map(|r| (r.mod_id.as_str(), r.id.as_str()))
        .collect();
    assert_eq!(keys, vec![("a", "a")]);
    assert_eq!(recs[0].name, "fixture a");
    assert!(recs[0].patched_by.is_empty());
    assert!(host.get_content_record("maps", "b").is_none());
    assert!(host.get_content_record("weapons", "1").is_none());
    assert!(host.get_content_record("weapons", "7").is_none());

    // the key "b" is free: a later mod registers it
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("c", "shared", MAP_CAPS),
        r#"return function(mod) mod.content.maps:register({ id = "b", name = "c's b", world = mod.assets:path("assets/maps/c.dat"), modes = { "TDM" } }) end"#,
    )
    .unwrap();
    let keys: Vec<(String, String)> = host
        .map_records()
        .unwrap()
        .iter()
        .map(|r| (r.mod_id.clone(), r.id.clone()))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("a".to_string(), "a".to_string()),
            ("c".to_string(), "b".to_string())
        ]
    );

    // the same for a mod that swallowed a capability denial with pcall (it fails to load all the same)
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest_id("d", "shared", MAP_CAPS),
        r#"return function(mod)
            mod.content.maps:register({ id = "d", name = "D", world = mod.assets:path("assets/maps/d.dat"), modes = { "TDM" } })
            pcall(function() mod.content.weapons:patch(1, { AmmoDamage = 5 }) end)
        end"#,
    )
    .unwrap_err();
    assert!(e.contains("capability denied"), "{e}");
    assert!(host.get_content_record("maps", "d").is_none());
    assert_eq!(host.map_records().unwrap().len(), 2);
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("e", "shared", MAP_CAPS),
        r#"return function(mod) mod.content.maps:register({ id = "d", name = "e's d", world = mod.assets:path("assets/maps/e.dat"), modes = { "TDM" } }) end"#,
    )
    .unwrap();
    assert_eq!(host.map_records().unwrap().len(), 3);
    let _ = fs::remove_dir_all(&dir);
}

/// A map patch replaces `modes`, `goals`, `players` and `radar` whole (lists are values, review C12): a
/// patch may shrink the owner's mode list, and the merged record is then checked as a whole.
#[test]
fn a_map_patch_replaces_modes_goals_players_and_radar_whole() {
    let mut host = ModHost::new();
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("a", "shared", MAP_CAPS),
        r#"return function(mod)
            mod.content.maps:register({ id = "a", name = "A", world = mod.assets:path("assets/maps/a.dat"),
                modes = { "TDM", "BOMB" }, goals = { TDM = 80 }, players = { min = 2, max = 8 },
                radar = { picture = mod.assets:path("assets/pictures/a_radar.dtx"), min = { -10, -10 }, max = { 10, 10 } } })
        end"#,
    )
    .unwrap();
    // the mode list shrinks to FFA, so the owner's goals.TDM no longer names a listed mode: refused
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest_id("b", "server_rules", &["content.maps"]),
        r#"return function(mod) mod.content.maps:patch("a", { modes = { "FFA" } }) end"#,
    )
    .unwrap_err();
    assert!(
        e.contains(
            "Mod 'b': mod.content.maps:patch: 'goals' names mode \"TDM\", which 'modes' does not list"
        ),
        "{e}"
    );
    let rec = &host.map_records().unwrap()[0];
    assert_eq!(rec.modes, vec!["TDM", "BOMB"]);
    assert_eq!(rec.goals, vec![("TDM".to_string(), 80)]);
    assert!(rec.patched_by.is_empty());

    // the same patch with goals for the new list is accepted: exactly FFA, exactly goals.FFA
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("c", "server_rules", &["content.maps"]),
        r#"return function(mod) mod.content.maps:patch("a", { modes = { "FFA" }, goals = { FFA = 50 } }) end"#,
    )
    .unwrap();
    let rec = &host.map_records().unwrap()[0];
    assert_eq!(rec.modes, vec!["FFA"]);
    assert_eq!(rec.goals, vec![("FFA".to_string(), 50)]);
    assert_eq!(rec.patched_by, vec!["c"]);
    assert_eq!(rec.players.map(|p| (p.min, p.max)), Some((2, 8)));

    // players and radar are replaced whole too; keys the patch does not carry stay (a deep merge otherwise)
    load(
        &mut host,
        ModOrigin::Server,
        manifest_id("d", "server_rules", &["content.maps"]),
        r#"return function(mod) mod.content.maps:patch("a", { name = "A (small)", players = { min = 4, max = 4 },
            radar = { picture = "@mod:a/assets/pictures/a_radar.dtx", min = { -5, -5 }, max = { 5, 5 } } }) end"#,
    )
    .unwrap();
    let rec = &host.map_records().unwrap()[0];
    assert_eq!(rec.name, "A (small)");
    assert_eq!(rec.modes, vec!["FFA"]);
    assert_eq!(rec.goals, vec![("FFA".to_string(), 50)]);
    assert_eq!(rec.players.map(|p| (p.min, p.max)), Some((4, 4)));
    assert_eq!(
        rec.radar.as_ref().map(|r| (r.min, r.max)),
        Some(([-5.0, -5.0], [5.0, 5.0]))
    );
    assert_eq!(rec.patched_by, vec!["c", "d"]);
    // a partial radar (no picture) is a whole value too, so it is refused as incomplete
    let e = load(
        &mut host,
        ModOrigin::Server,
        manifest_id("e", "server_rules", &["content.maps"]),
        r#"return function(mod) mod.content.maps:patch("a", { radar = { min = { -1, -1 }, max = { 1, 1 } } }) end"#,
    )
    .unwrap_err();
    assert!(e.contains("mod.content.maps:patch: 'radar"), "{e}");
}

// ---- credited assets (MOD-28) ---------------------------------------------------------------------

fn credited(credits: &str) -> Result<ModManifest, String> {
    parse(&format!(
        r#"{{"id": "m", "name": "M", "version": "1.0.0", "kind": "shared", "description": "A test fixture mod for credited assets.", "author": "tests", "screenshots": ["screenshots/main.png"], "capabilities": ["assets"], "asset_policy": "credited", "credits": [{credits}]}}"#
    ))
}

const GOOD_CREDIT: &str = r#"{"title": "a model", "author": "someone", "license": "CC-BY-4.0", "source": "https://example.org/a", "changes": "made smaller"}"#;

#[test]
fn a_credited_mod_keeps_its_credits_through_json() {
    let m = credited(GOOD_CREDIT).unwrap();
    assert_eq!(m.asset_policy, AssetPolicy::Credited);
    assert_eq!(m.credits.len(), 1);
    assert_eq!(m.credits[0].license, "CC-BY-4.0");
    let again = ModManifest::from_json_str(&m.to_json_pretty()).unwrap();
    assert_eq!(again.asset_policy, AssetPolicy::Credited);
    assert_eq!(again.credits, m.credits);
}

#[test]
fn a_credited_mod_must_say_what_it_uses() {
    let e = credited("").unwrap_err();
    assert!(e.contains("at least one"), "{e}");
    let e = credited(
        r#"{"title": "a", "author": "", "license": "CC-BY-4.0", "source": "s", "changes": "c"}"#,
    )
    .unwrap_err();
    assert!(e.contains("no 'author'"), "{e}");
}

#[test]
fn a_license_that_does_not_allow_it_is_refused() {
    for license in ["CC-BY-NC-4.0", "proprietary", ""] {
        let c = format!(
            r#"{{"title": "a", "author": "b", "license": "{license}", "source": "s", "changes": "c"}}"#
        );
        assert!(credited(&c).is_err(), "{license}");
    }
}

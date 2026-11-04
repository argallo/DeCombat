//! dc-config tests: schema round-trip, example validity, DC_DEFAULTS.md key coverage, layering precedence,
//! validation errors, community directory validator, landing-page data parity.

use dc_config::directory::{self, Badge};
use dc_config::*;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn errors(issues: &[Issue]) -> Vec<&Issue> {
    issues.iter().filter(|i| i.is_error()).collect()
}

fn has_err(issues: &[Issue], path: &str) -> bool {
    issues
        .iter()
        .any(|i| i.is_error() && i.path.starts_with(path))
}

// ------------------------------------------------------------------ schema round-trip / example

#[test]
fn default_round_trips_through_toml() {
    let d = Config::default();
    let back = parse_toml(&d.to_toml()).expect("re-parse");
    assert_eq!(d, back);
}

#[test]
fn populated_config_round_trips() {
    let mut c = Config::default();
    c.server.public = true;
    c.server.auth_port = 12000;
    c.mods.policy = "modded".into();
    c.mods.set = vec![
        ModRef {
            id: "a".into(),
            version: "1".into(),
            sha256: "0".repeat(64),
        },
        ModRef {
            id: "b".into(),
            version: "2".into(),
            sha256: "f".repeat(64),
        },
    ];
    c.lobby.channels = vec![LobbyChannelCfg {
        name: "One".into(),
        capacity: 10,
    }];
    c.clan.mode_weight.insert("TEAMDM".into(), 1.5);
    c.shop.coupons.insert("CODE".into(), 77);
    c.gm.extra
        .insert("ban_days".into(), toml::Value::Integer(3));
    c.economy.case_odds = vec![[1, 50], [0, 50]];
    let back = parse_toml(&c.to_toml()).expect("re-parse");
    assert_eq!(c, back);
}

#[test]
fn example_toml_is_the_documented_defaults_and_validates() {
    let c = parse_toml(EXAMPLE_SERVER_TOML).expect("example parses");
    assert_eq!(
        c,
        Config::default(),
        "server.example.toml must document the built-in defaults"
    );
    let issues = check(&c, CheckOptions::default());
    assert!(errors(&issues).is_empty(), "{issues:?}");
    // only the "rom not set" warning is expected
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].path, "paths.rom");
}

#[test]
fn modded_example_in_comment_is_valid_when_uncommented() {
    let t = format!(
        "[mods]\npolicy = \"modded\"\n[[mods.set]]\nid = \"example-hud\"\nversion = \"1.0.0\"\nsha256 = \"{}\"\n",
        "ab".repeat(32)
    );
    let c = parse_toml(&t).unwrap();
    assert!(errors(&check(&c, CheckOptions::default())).is_empty());
}

#[test]
fn unknown_key_is_rejected() {
    let e = parse_toml("[server]\nbnd = \"x\"\n").unwrap_err();
    assert!(e[0].message.contains("bnd"), "{e:?}");
    assert!(parse_toml("[nonsense]\nx = 1\n").is_err());
}

// ------------------------------------------------------------------ DC_DEFAULTS.md coverage

/// Every config key named in the "config key" column of DC_DEFAULTS.md is representable in the schema, and a
/// stated `= value` equals the built-in default.
#[test]
fn defaults_coverage() {
    let text = read("DC_DEFAULTS.md");
    let tree = Config::default().to_value();
    let mut checked = 0;
    let mut cds = 0;
    // Rows under a "PENDING user approval" heading are proposals: their keys enter the schema with the
    // approval commit, so they are not checked here.
    let mut pending = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            pending = line.contains("PENDING");
            continue;
        }
        if !line.starts_with("| CD-") || pending {
            continue;
        }
        cds += 1;
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split(" | ")
            .map(str::trim)
            .collect();
        assert!(cells.len() >= 5, "row has too few cells: {line}");
        // layout: ... | config key | hook   (key is the second-to-last cell)
        let key_cell = cells[cells.len() - 2];
        for seg in key_cell.split('`').skip(1).step_by(2) {
            let (key, rhs) = match seg.split_once(" = ") {
                Some((k, v)) => (k.trim(), Some(v.trim())),
                None => (seg.trim(), None),
            };
            assert!(!key.is_empty());
            let mut node = &tree;
            let parts: Vec<&str> = key.split('.').collect();
            for (i, part) in parts.iter().enumerate() {
                let wildcard = *part == "*" || part.starts_with('<');
                if wildcard {
                    assert!(i + 1 == parts.len(), "wildcard must be last: {key}");
                    assert!(
                        node.is_table(),
                        "{key}: parent of wildcard is not a table in the schema ({line})"
                    );
                    break;
                }
                node = node.get(part).unwrap_or_else(|| {
                    panic!(
                        "config key '{key}' (row: {}) is not representable: no '{part}'",
                        cells[0]
                    )
                });
            }
            if let Some(rhs) = rhs {
                let want: toml::Table = toml::from_str(&format!("v = {rhs}"))
                    .unwrap_or_else(|e| panic!("{key}: bad value {rhs}: {e}"));
                assert_eq!(
                    node, &want["v"],
                    "default of '{key}' differs from DC_DEFAULTS.md"
                );
            }
            checked += 1;
        }
    }
    assert!(cds >= 60, "expected to see the CD rows, saw {cds}");
    assert!(
        checked >= 50,
        "expected to check at least 50 keys, checked {checked}"
    );
}

// ------------------------------------------------------------------ layering / env overrides

#[test]
fn env_overrides_file_and_cli_overrides_env() {
    let file = "[dserver]\ntick_hz = 20\n[server]\nauth_port = 11000\n";
    // file only
    let c = load_layered(Some(file), env(&[]), &CliOverrides::default()).unwrap();
    assert_eq!(c.dserver.tick_hz, 20);
    assert_eq!(c.server.auth_port, 11000);
    // env beats file
    let c = load_layered(
        Some(file),
        env(&[("DC_DSERVER__TICK_HZ", "30")]),
        &CliOverrides::default(),
    )
    .unwrap();
    assert_eq!(c.dserver.tick_hz, 30);
    assert_eq!(
        c.server.auth_port, 11000,
        "untouched keys keep the file value"
    );
    // cli beats env
    let cli = CliOverrides {
        rom: Some("C:/rom".into()),
        public: true,
        ..Default::default()
    };
    let c = load_layered(
        Some(file),
        env(&[
            ("DC_PATHS__ROM", "D:/other"),
            ("DC_SERVER__PUBLIC", "false"),
        ]),
        &cli,
    )
    .unwrap();
    assert_eq!(c.paths.rom, "C:/rom");
    assert!(c.server.public);
    assert_eq!(c.effective_bind(), "0.0.0.0");
    // defaults when nothing is given
    let c = load_layered(None, env(&[]), &CliOverrides::default()).unwrap();
    assert_eq!(c, Config::default());
    assert_eq!(c.effective_bind(), "127.0.0.1");
}

#[test]
fn env_types_nested_and_free_maps() {
    let vars = env(&[
        ("DC_ECONOMY__EXP__KILL", "5"),
        ("DC_ECONOMY__CASE_ODDS", "[[1, 70], [0, 30]]"),
        ("DC_METRICS__ENABLED", "yes"),
        ("DC_LOG__FORMAT", "json"),
        ("DC_CLAN__MODE_WEIGHT__TEAMDM", "2.0"),
        ("DC_GM__MAX_BAN_DAYS", "9"),
        ("DC_SERVER__PUBLIC", "TRUE"),
        ("DC_TARGET", "x86_64"), // build variable, not config
        ("PATH", "x"),
    ]);
    let c = apply_env(&Config::default(), vars).unwrap();
    assert_eq!(c.economy.exp.kill, 5);
    assert_eq!(c.economy.gp.kill, 6, "gp untouched");
    assert_eq!(c.economy.case_odds, vec![[1, 70], [0, 30]]);
    assert!(c.metrics.enabled && c.server.public);
    assert_eq!(c.log.format, "json");
    assert_eq!(c.clan.mode_weight["TEAMDM"], 2.0);
    assert_eq!(c.gm.extra["max_ban_days"], toml::Value::Integer(9));
}

#[test]
fn env_errors_are_clear() {
    let e = apply_env(&Config::default(), env(&[("DC_SERVER__PUBLIK", "true")])).unwrap_err();
    assert!(
        e[0].message.contains("unknown config key 'publik'") && e[0].message.contains("public"),
        "{e:?}"
    );
    assert_eq!(e[0].path, "DC_SERVER__PUBLIK");
    let e = apply_env(&Config::default(), env(&[("DC_DSERVER__TICK_HZ", "fast")])).unwrap_err();
    assert!(e[0].message.contains("not an integer"), "{e:?}");
    let e = apply_env(&Config::default(), env(&[("DC_SERVER__PUBLIC", "maybe")])).unwrap_err();
    assert!(e[0].message.contains("not a boolean"), "{e:?}");
    let e = apply_env(&Config::default(), env(&[("DC_ECONOMY__EXP", "1")])).unwrap_err();
    assert!(e[0].message.contains("table"), "{e:?}");
    // out-of-range value fails typed deserialisation, not silently wraps
    let e = apply_env(
        &Config::default(),
        env(&[("DC_SERVER__AUTH_PORT", "70000")]),
    )
    .unwrap_err();
    assert!(!e.is_empty());
}

// ------------------------------------------------------------------ validation

#[test]
fn check_catches_problems() {
    let bad = |f: &dyn Fn(&mut Config)| {
        let mut c = Config::default();
        f(&mut c);
        check(&c, CheckOptions::default())
    };
    assert!(has_err(
        &bad(&|c| c.server.bind = "0.0.0.0".into()),
        "server.bind"
    ));
    assert!(!has_err(
        &bad(&|c| {
            c.server.bind = "0.0.0.0".into();
            c.server.public = true;
        }),
        "server.bind"
    ));
    assert!(has_err(
        &bad(&|c| c.server.bind = "not-an-ip".into()),
        "server.bind"
    ));
    assert!(
        has_err(&bad(&|c| c.server.lobby_port = 10000), "server.lobby_port"),
        "port clash"
    );
    assert!(
        has_err(&bad(&|c| c.server.auth_port = 65534), "server.lobby_port"),
        "derived port overflow"
    );
    assert!(has_err(
        &bad(&|c| c.dserver.port_range = "30000-29000".into()),
        "dserver.port_range"
    ));
    assert!(
        has_err(
            &bad(&|c| c.dserver.port_range = "27000-28000".into()),
            "dserver.port_range"
        ),
        "contains game_port"
    );
    assert!(has_err(
        &bad(&|c| c.dserver.port_range = "abc".into()),
        "dserver.port_range"
    ));
    assert!(has_err(
        &bad(&|c| c.log.format = "xml".into()),
        "log.format"
    ));
    assert!(has_err(
        &bad(&|c| c.directory.register = true),
        "directory.urls"
    ));
    assert!(has_err(
        &bad(&|c| c.directory.register = true),
        "directory.public_address"
    ));
    assert!(has_err(
        &bad(&|c| c.directory.region = "mars".into()),
        "directory.region"
    ));
    assert!(has_err(&bad(&|c| c.dserver.tick_hz = 0), "dserver.tick_hz"));
    assert!(has_err(
        &bad(&|c| c.economy.case_odds = vec![]),
        "economy.case_odds"
    ));
    assert!(has_err(
        &bad(&|c| c.inventory.default_repair = "everything".into()),
        "inventory.default_repair"
    ));
    assert!(
        !has_err(
            &bad(&|c| c.inventory.default_repair = "all".into()),
            "inventory.default_repair"
        ),
        "CD-52 approved value all"
    );
    assert!(has_err(
        &bad(&|c| c.gm.test_commands = "nobody".into()),
        "gm.test_commands"
    ));
    assert!(has_err(
        &bad(&|c| c.clan.level_steps = vec![5, 9]),
        "clan.level_steps"
    ));
    // mod set
    assert!(
        has_err(&bad(&|c| c.mods.policy = "modded".into()), "mods.set"),
        "modded needs mods"
    );
    assert!(
        has_err(
            &bad(&|c| c.mods.set.push(ModRef {
                id: "x".into(),
                version: "1".into(),
                sha256: "0".repeat(64)
            })),
            "mods.set"
        ),
        "vanilla has none"
    );
    assert!(has_err(
        &bad(&|c| c.mods.client_visual_allowed = true),
        "mods.client_visual_allowed"
    ));
    let m = |f: &dyn Fn(&mut ModRef)| {
        let mut r = ModRef {
            id: "ok".into(),
            version: "1.0".into(),
            sha256: "a".repeat(64),
        };
        f(&mut r);
        bad(&|c| {
            c.mods.policy = "modded".into();
            c.mods.set = vec![r.clone()];
        })
    };
    assert!(errors(&m(&|_| {})).is_empty());
    assert!(
        has_err(&m(&|r| r.sha256 = "A".repeat(64)), "mods.set[0].sha256"),
        "uppercase hex rejected"
    );
    assert!(has_err(
        &m(&|r| r.sha256 = "abc".into()),
        "mods.set[0].sha256"
    ));
    assert!(has_err(&m(&|r| r.id = "Bad Id".into()), "mods.set[0].id"));
    assert!(has_err(
        &m(&|r| r.version = "".into()),
        "mods.set[0].version"
    ));
    let dup = bad(&|c| {
        c.mods.policy = "modded".into();
        let r = ModRef {
            id: "same".into(),
            version: "1".into(),
            sha256: "a".repeat(64),
        };
        c.mods.set = vec![r.clone(), r];
    });
    assert!(has_err(&dup, "mods.set[1].id"));
    // messages say what to do
    let msg = &bad(&|c| c.server.bind = "0.0.0.0".into())[0].message;
    assert!(msg.contains("server.public"), "{msg}");
}

#[test]
fn check_fs_reports_missing_paths() {
    let mut c = Config::default();
    c.paths.rom = "Z:/definitely/not/here".into();
    c.paths.mods_dir = "Z:/nope".into();
    c.paths.db = "Z:/nope/x.db".into();
    c.mods.policy = "modded".into();
    c.mods.set = vec![ModRef {
        id: "a".into(),
        version: "1".into(),
        sha256: "a".repeat(64),
    }];
    let i = check(&c, CheckOptions { fs: true });
    assert!(
        has_err(&i, "paths.rom") && has_err(&i, "paths.db") && has_err(&i, "mods.set[0]"),
        "{i:?}"
    );
    // static check does not touch the disk
    assert!(!has_err(&check(&c, CheckOptions::default()), "paths."));
}

#[test]
fn derived_ports_follow_the_harness_rules() {
    let p = Config::default().resolved_ports();
    assert_eq!(
        (p.auth, p.lobby, p.clan, p.http),
        (10000, 10002, 10003, 18080)
    );
}

// ------------------------------------------------------------------ community directory

fn community() -> directory::Community {
    directory::load_community(&read("directory/community.json")).expect("community.json parses")
}

#[test]
fn example_community_validates() {
    let doc = community();
    let issues = directory::validate_community(&doc);
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(doc.servers.len(), 2);
    assert!(
        doc.servers.iter().any(|s| s.policy == "vanilla")
            && doc.servers.iter().any(|s| s.policy == "modded")
    );
    // fictional hosts only
    for s in &doc.servers {
        assert!(s.address.contains(".invalid:"), "{}", s.address);
    }
}

#[test]
fn community_round_trips_through_json() {
    let doc = community();
    let back: directory::Community =
        serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
    assert_eq!(doc, back);
}

#[test]
fn badges_follow_known_mods() {
    let doc = community();
    let modded = doc.servers.iter().find(|s| s.policy == "modded").unwrap();
    let b = directory::badges(modded, &doc);
    let by = |id: &str| b.iter().find(|(m, _)| m.id == id).unwrap().1;
    assert_eq!(by("example-rules"), Badge::Verified);
    assert_eq!(by("example-hud"), Badge::Verified);
    assert_eq!(by("example-xray"), Badge::Unverified);
}

#[test]
fn community_validator_rejects_bad_documents() {
    let bad = |f: &dyn Fn(&mut directory::Community)| {
        let mut d = community();
        f(&mut d);
        directory::validate_community(&d)
    };
    assert!(has_err(&bad(&|d| d.schema_version = 2), "schema_version"));
    // CLI-1616: a certificate fingerprint is 64 hex digits (colon pairs allowed)
    assert!(has_err(
        &bad(&|d| d.servers[1].fingerprint = Some("12:34".into())),
        "servers[1].fingerprint"
    ));
    assert!(!has_err(
        &bad(&|d| d.servers[1].fingerprint = Some(vec!["AB"; 32].join(":"))),
        "servers[1].fingerprint"
    ));
    assert!(has_err(
        &bad(&|d| d.updated = "yesterday".into()),
        "updated"
    ));
    assert!(
        has_err(
            &bad(&|d| d.servers[1].id = d.servers[0].id.clone()),
            "servers[1].id"
        ),
        "duplicate id"
    );
    assert!(has_err(
        &bad(&|d| d.servers[0].id = "Bad_ID".into()),
        "servers[0].id"
    ));
    assert!(has_err(
        &bad(&|d| d.servers[0].region = "mars".into()),
        "servers[0].region"
    ));
    assert!(has_err(
        &bad(&|d| d.servers[0].address = "no-port".into()),
        "servers[0].address"
    ));
    assert!(has_err(
        &bad(&|d| d.servers[0].address = "host:99999".into()),
        "servers[0].address"
    ));
    assert!(has_err(
        &bad(&|d| d.servers[0].policy = "hybrid".into()),
        "servers[0].policy"
    ));
    assert!(has_err(
        &bad(&|d| d.servers[0].players = Some(directory::Players { current: 9, max: 3 })),
        "servers[0].players"
    ));
    // vanilla with mods
    assert!(has_err(
        &bad(&|d| d.servers[0].mods = d.servers[1].mods.clone()),
        "servers[0].mods"
    ));
    assert!(has_err(
        &bad(&|d| d.servers[0].client_visual_allowed = true),
        "servers[0].client_visual_allowed"
    ));
    // modded without mods
    assert!(has_err(
        &bad(&|d| d.servers[1].mods.clear()),
        "servers[1].mods"
    ));
    // a server claiming a known (id, version) with other bytes
    assert!(has_err(
        &bad(&|d| d.servers[1].mods[0].sha256 = "1".repeat(64)),
        "servers[1].mods[0].sha256"
    ));
    // duplicate mod in one set, bad hash
    assert!(has_err(
        &bad(&|d| d.servers[1].mods[1].id = d.servers[1].mods[0].id.clone()),
        "servers[1].mods[1].id"
    ));
    assert!(has_err(
        &bad(&|d| d.servers[1].mods[2].sha256 = "xyz".into()),
        "servers[1].mods[2].sha256"
    ));
    // known mods
    assert!(has_err(
        &bad(&|d| d.known_mods[0].source_repo = "http://insecure".into()),
        "known_mods[0].source_repo"
    ));
    assert!(
        has_err(
            &bad(&|d| {
                let mut k = d.known_mods[0].clone();
                k.sha256 = "2".repeat(64);
                d.known_mods.push(k);
            }),
            "known_mods[2]"
        ),
        "same id+version, two hashes"
    );
    // unknown fields are a parse error
    assert!(directory::load_community(r#"{"schema_version":1,"updated":"2026-10-24T12:00:00Z","servers":[],"known_mods":[],"extra":1}"#).is_err());
    assert!(directory::load_community("{").is_err());
}

/// directory/schema.json lists exactly the properties the Rust types serialise.
#[test]
fn schema_json_matches_rust_types() {
    let schema: serde_json::Value =
        serde_json::from_str(&read("directory/schema.json")).expect("schema.json is JSON");
    let keys = |v: &serde_json::Value| -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    let doc = serde_json::to_value(community()).unwrap();
    assert_eq!(keys(&schema["properties"]), keys(&doc));
    let server = &doc["servers"][1];
    assert_eq!(
        keys(&schema["$defs"]["server"]["properties"]),
        keys(server),
        "server"
    );
    assert_eq!(
        keys(&schema["$defs"]["mod_ref"]["properties"]),
        keys(&server["mods"][0]),
        "mod_ref"
    );
    assert_eq!(
        keys(&schema["$defs"]["server"]["properties"]["players"]["properties"]),
        keys(&server["players"]),
        "players"
    );
    assert_eq!(
        keys(&schema["$defs"]["known_mod"]["properties"]),
        keys(&doc["known_mods"][0]),
        "known_mod"
    );
    // required lists name real properties
    for def in ["server", "known_mod", "mod_ref"] {
        for r in schema["$defs"][def]["required"].as_array().unwrap() {
            assert!(
                schema["$defs"][def]["properties"]
                    .get(r.as_str().unwrap())
                    .is_some(),
                "{def}.{r}"
            );
        }
    }
    // enum lists agree with the validator constants
    let regions: Vec<&str> = schema["$defs"]["server"]["properties"]["region"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(regions, directory::REGIONS);
    let pol: Vec<&str> = schema["$defs"]["server"]["properties"]["policy"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(pol, directory::POLICIES);
    assert_eq!(
        schema["properties"]["schema_version"]["const"],
        directory::SCHEMA_VERSION
    );
}

/// The landing page serves the same document as directory/, plus an offline copy for file:// review.
#[test]
fn landing_data_matches_directory() {
    let canon: serde_json::Value = serde_json::from_str(&read("directory/community.json")).unwrap();
    let served: serde_json::Value =
        serde_json::from_str(&read("web/landing/community.json")).unwrap();
    assert_eq!(
        canon, served,
        "web/landing/community.json must equal directory/community.json"
    );
    let js = read("web/landing/community.sample.js");
    let body = js
        .split_once("window.DC_COMMUNITY_SAMPLE = ")
        .expect("sample prefix")
        .1
        .trim()
        .trim_end_matches(';');
    let sample: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        canon, sample,
        "community.sample.js must equal community.json"
    );
}

/// MOD-29: the default is the most a client will download, a server may stay lower and may not go higher.
#[test]
fn the_package_limit_is_what_a_client_will_download() {
    assert_eq!(Config::default().mods.max_zip_bytes, 32 * 1024 * 1024);
    let flagged = |bytes: u64| {
        let mut c = Config::default();
        c.mods.max_zip_bytes = bytes;
        has_err(&check(&c, CheckOptions::default()), "mods.max_zip_bytes")
    };
    assert!(!flagged(32 * 1024 * 1024));
    assert!(!flagged(16 * 1024 * 1024));
    assert!(flagged(32 * 1024 * 1024 + 1));
    assert!(flagged(0));
}

#[test]
fn landing_files_exist_and_name_the_project() {
    for f in ["index.html", "app.js", "style.css"] {
        assert!(!read(&format!("web/landing/{f}")).is_empty(), "{f}");
    }
    let html = read("web/landing/index.html");
    assert!(html.contains("DeCombat"));
    assert!(
        html.contains("not affiliated with, endorsed by or sponsored by"),
        "NOTICE wording"
    );
    // links out (Discord, the release page) are fine; nothing the page loads may come from elsewhere
    assert!(!html.contains("src=\"http"), "no remote resources");
    assert!(
        !html.split("<link ").skip(1).any(|l| l
            .split('>')
            .next()
            .unwrap_or("")
            .contains("href=\"http")),
        "no remote <link> resources"
    );
}

// ------------------------------------------------------------------ explicit-set tracking (lobby layering)

#[test]
fn explicit_tracks_file_and_env_paths() {
    let file = "[economy.exp]\nkill = 4\n[shop]\nsale_list = [1, 2]\n[shop.coupons]\n[[mods.set]]\nid = \"a\"\nversion = \"1\"\nsha256 = \"00\"\n";
    let (cfg, ex) = load_layered_explicit(
        Some(file),
        env(&[("DC_ROOM__MAX_PLAYERS_CAP", "8"), ("DC_TARGET", "x86_64")]),
        &CliOverrides::default(),
    )
    .unwrap();
    assert_eq!(cfg.room.max_players_cap, 8);
    // set to the built-in default value: still explicit
    assert!(ex.has("economy.exp.kill") && ex.has("Economy.Exp") && ex.has("economy"));
    assert!(ex.has("shop.sale_list") && ex.has("shop.coupons") && ex.has("mods.set"));
    assert!(ex.has("room.max_players_cap"));
    assert!(
        !ex.has("economy.gp.kill")
            && !ex.has("room.countdown_ticks")
            && !ex.has("dtarget")
            && !ex.has("target")
    );
    assert!(
        !ex.has("shop.sale"),
        "a prefix of a segment is not a path prefix"
    );
}

#[test]
fn new_schema_keys_round_trip_and_env_override() {
    let mut c = Config::default();
    c.shop.price_override.insert("9".into(), 100);
    c.shop.coupon_uses.insert("CODE".into(), 3);
    c.shop.list_override.hot = vec![1, 2];
    c.gm.test_commands = "gm".into();
    c.notice.login_box = "hello".into();
    c.clan.level_steps = vec![0, 10];
    assert_eq!(parse_toml(&c.to_toml()).unwrap(), c);
    let c = apply_env(
        &Config::default(),
        env(&[("DC_SHOP__PRICE_OVERRIDE__77", "5")]),
    )
    .unwrap();
    assert_eq!(c.shop.price_override.get("77"), Some(&5));
    let c = apply_env(
        &Config::default(),
        env(&[
            ("DC_NOTICE__TICKER_REPEAT", "9"),
            ("DC_MESSENGER__WHISPER_FRIENDS_ONLY", "true"),
        ]),
    )
    .unwrap();
    assert_eq!(
        (c.notice.ticker_repeat, c.messenger.whisper_friends_only),
        (9, true)
    );
}

// ------------------------------------------------------------------ [combat] and the proposed-row keys

/// DC_DEFAULTS rows that are not CD rows but name config keys (proposed, with the key in the row text):
/// the key must exist in the schema, with the stated default, and the row must still name it.
// DC-DEFAULT: DSV-30 (proposed) combat.*; LDO-1, RM-1, AUTH-1 (proposed) lobby keys; DSV-35 (proposed)
#[test]
fn proposed_rows_name_keys_that_exist() {
    let text = read("DC_DEFAULTS.md");
    let tree = Config::default().to_value();
    let at = |key: &str| {
        let mut n = &tree;
        for p in key.split('.') {
            n = n
                .get(p)
                .unwrap_or_else(|| panic!("key {key} is not in the schema"));
        }
        n.clone()
    };
    let row = |id: &str| {
        text.lines()
            .find(|l| l.starts_with(&format!("| {id} |")))
            .unwrap_or_else(|| panic!("no DC_DEFAULTS row {id}"))
            .to_string()
    };
    for (id, key, want) in [
        ("LDO-1", "dserver.loadout_tokens", "true"),
        ("RM-1", "room.settings_change_clears_ready", "true"),
        ("AUTH-1", "auth.first_account_no", "1001"),
        ("VIS-1", "vis.enabled", "true"),
        ("VIS-7", "vis.exposure_ms", "1000"),
    ] {
        assert!(row(id).contains(key), "row {id} no longer names {key}");
        let w: toml::Table = toml::from_str(&format!("v = {want}")).unwrap();
        assert_eq!(at(key), w["v"], "default of {key}");
    }
    // DSV-30 names the keys as `-combat key=value` / `combat.<key>`; all 14 are schema keys
    let dsv30 = row("DSV-30");
    assert!(
        dsv30.contains("combat."),
        "DSV-30 should name combat.* keys"
    );
    let combat = at("combat");
    let keys: Vec<&String> = combat.as_table().unwrap().keys().collect();
    assert_eq!(keys.len(), 17, "{keys:?}");
    for k in [
        "head_factor",
        "body_factor",
        "head_fraction",
        "zone_factors",
        "armor_absorb",
        "armor_gear",
        "armor_cost",
        "start_armor",
        "max_armor",
        "start_health",
        "max_health",
        "default_damage",
        "hit_tolerance",
        "origin_tolerance",
        "trace",
        "trace_slack",
        "blast_inner",
    ] {
        assert!(keys.iter().any(|x| x.as_str() == k), "combat.{k} missing");
    }
    assert_eq!(at("dserver.send_gain_lines"), toml::Value::Boolean(true));
}

/// `[vis]` (VIS-1, VIS-7, VIS-8, VIS-12 proposed): four keys, parsed, validated, unknown keys refused.
#[test]
fn vis_table_parses_and_validates() {
    let tree = Config::default().to_value();
    let vis = tree.get("vis").and_then(|v| v.as_table()).expect("[vis]");
    let keys: Vec<&str> = vis.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "always_radius",
            "enabled",
            "exposure_ms",
            "max_segments_per_tick"
        ]
    );
    let c = parse_toml(
        "[vis]
enabled = false
exposure_ms = 1500
always_radius = 400.0
",
    )
    .unwrap();
    assert!(!c.vis.enabled);
    assert_eq!((c.vis.exposure_ms, c.vis.always_radius), (1500, 400.0));
    assert_eq!(c.vis.max_segments_per_tick, 1000);
    assert!(parse_toml(
        "[vis]
radius = 1
"
    )
    .is_err());
    let mut bad = Config::default();
    bad.vis.always_radius = -1.0;
    assert!(errors(&check(&bad, CheckOptions::default()))
        .iter()
        .any(|i| i.path == "vis.always_radius"));
    let mut bad = Config::default();
    bad.vis.exposure_ms = 90_000;
    assert!(errors(&check(&bad, CheckOptions::default()))
        .iter()
        .any(|i| i.path == "vis.exposure_ms"));
    assert!(errors(&check(&Config::default(), CheckOptions::default())).is_empty());
}

#[test]
fn combat_table_parses_rom_or_number_and_validates() {
    let c = parse_toml(
        "[combat]\nhead_factor = 3.0\narmor_absorb = 0.5\nstart_armor = \"rom\"\nmax_armor = 80\nzone_factors = \"off\"\ntrace = \"off\"\n",
    )
    .unwrap();
    assert_eq!(c.combat.head_factor, 3.0);
    assert_eq!(c.combat.armor_absorb, RomOr::Value(0.5));
    assert_eq!(c.combat.start_armor, RomOr::Rom);
    assert_eq!(c.combat.max_armor, RomOr::Value(80.0));
    assert!(errors(&check(&c, CheckOptions::default())).is_empty());
    assert_eq!(parse_toml(&c.to_toml()).unwrap(), c);
    assert!(parse_toml("[combat]\nbogus = 1\n").is_err());
    assert!(parse_toml("[combat]\narmor_absorb = \"lots\"\n").is_err());
    let bad = |f: &dyn Fn(&mut Config)| {
        let mut c = Config::default();
        f(&mut c);
        check(&c, CheckOptions::default())
    };
    assert!(has_err(
        &bad(&|c| c.combat.zone_factors = "x".into()),
        "combat.zone_factors"
    ));
    assert!(has_err(
        &bad(&|c| c.combat.trace = "yes".into()),
        "combat.trace"
    ));
    assert!(has_err(
        &bad(&|c| c.combat.head_factor = -1.0),
        "combat.head_factor"
    ));
    assert!(has_err(
        &bad(&|c| c.combat.armor_absorb = RomOr::Value(f64::NAN)),
        "combat.armor_absorb"
    ));
    assert!(has_err(
        &bad(&|c| c.auth.first_account_no = 0),
        "auth.first_account_no"
    ));
    // the 2009 calibration (project decision): the armour cost from the base damage; the worn armour rows
    // (DSV-430: row 0 for a player without gear); a row may be written as a number or text
    assert_eq!(
        (
            Config::default().combat.armor_gear.as_str(),
            Config::default().combat.armor_cost.as_str()
        ),
        ("worn", "base")
    );
    let c = parse_toml("[combat]\narmor_gear = 2\narmor_cost = \"hit\"\n").unwrap();
    assert_eq!(
        (c.combat.armor_gear.as_str(), c.combat.armor_cost.as_str()),
        ("2", "hit")
    );
    assert!(errors(&check(&c, CheckOptions::default())).is_empty());
    assert_eq!(parse_toml(&c.to_toml()).unwrap(), c);
    let c = parse_toml("[combat]\narmor_gear = \"items\"\n").unwrap();
    assert!(errors(&check(&c, CheckOptions::default())).is_empty());
    assert!(has_err(
        &bad(&|c| c.combat.armor_gear = "vest".into()),
        "combat.armor_gear"
    ));
    assert!(has_err(
        &bad(&|c| c.combat.armor_cost = "all".into()),
        "combat.armor_cost"
    ));
    // pairs() feeds GameTuning::set_combat in COMBAT_KEYS order
    let p = Config::default().combat.pairs();
    assert_eq!(p.len(), 17);
    assert_eq!(p[16], ("blast_inner", "0.25".to_string()));
    assert_eq!(p[0], ("head_factor", "2".to_string()));
    assert_eq!(p[4], ("armor_absorb", "rom".to_string()));
    assert_eq!(p[5], ("armor_gear", "worn".to_string()));
    assert_eq!(p[6], ("armor_cost", "base".to_string()));
}

#[test]
fn combat_and_lobby_keys_layer_env_over_file_and_cli_stays_untouched() {
    let file = "[combat]\nhead_factor = 3.0\narmor_absorb = 0.25\n[room]\nsettings_change_clears_ready = false\n[auth]\nfirst_account_no = 5000\n[dserver]\nloadout_tokens = false\nsend_gain_lines = false\n";
    let (c, ex) = load_layered_explicit(
        Some(file),
        env(&[
            ("DC_COMBAT__HEAD_FACTOR", "4.5"),
            ("DC_COMBAT__ARMOR_ABSORB", "rom"),
            ("DC_COMBAT__TRACE", "off"),
        ]),
        &CliOverrides::default(),
    )
    .unwrap();
    assert_eq!(c.combat.head_factor, 4.5, "env beats the file");
    assert_eq!(
        c.combat.armor_absorb,
        RomOr::Rom,
        "env 'rom' beats the file number"
    );
    assert_eq!(c.combat.trace, "off");
    assert!(!c.room.settings_change_clears_ready);
    assert_eq!(c.auth.first_account_no, 5000);
    assert!(!c.dserver.loadout_tokens && !c.dserver.send_gain_lines);
    assert!(ex.has("combat.head_factor") && ex.has("combat.trace"));
    assert!(!ex.has("combat.body_factor"), "unset keys stay unset");
    assert!(ex.has("auth.first_account_no") && ex.has("dserver.send_gain_lines"));
    // a number through the environment reaches a rom-or-number key
    let c = apply_env(&Config::default(), env(&[("DC_COMBAT__MAX_ARMOR", "120")])).unwrap();
    assert_eq!(c.combat.max_armor, RomOr::Value(120.0));
    // dump shows the table
    assert!(Config::default().to_toml().contains("[combat]"));
}

// ------------------------------------------------------------------ Wave AD: keys that were command-line only

/// `[evars]` (DSV-47 / DSV-60), `combat.blast_inner` (DSV-39 amendment), `room.round_secs.*` (GMD-2) and the
/// lobby HiSec keys (`[hisec]`, `economy.hisec_case_odds`; HS-6..HS-9): parsed, round-tripped, layered from the
/// environment, validated, unknown keys refused.
// DC-DEFAULT: DSV-47, DSV-60, DSV-39 (amendment), GMD-2 (clarification), HS-6..HS-9 (proposed)
#[test]
fn wave_ad_keys_parse_layer_and_validate() {
    let bad = |f: &dyn Fn(&mut Config)| {
        let mut c = Config::default();
        f(&mut c);
        check(&c, CheckOptions::default())
    };
    let d = Config::default();
    assert_eq!((d.evars.mode.as_str(), d.evars.extra.len()), ("world", 0));
    assert_eq!(d.combat.blast_inner, 0.25);
    assert_eq!(
        (
            d.room.round_secs.fallback,
            d.room.round_secs.default,
            d.room.round_secs.classes.len()
        ),
        (true, 600, 0)
    );
    assert_eq!(d.economy.hisec_case_odds, vec![[7, 50], [30, 35], [0, 15]]);
    assert_eq!(
        (
            d.hisec.case_product,
            d.hisec.announce.as_str(),
            d.hisec.no_key_reason
        ),
        (0, "all", 0x99)
    );
    let text = "[evars]\nmode = \"off\"\n[evars.extra]\nMyVar = \"1.5\"\n[combat]\nblast_inner = 0.5\n\
                [room.round_secs]\nfallback = true\ndefault = 300\n1 = 900\n12 = 120\n\
                [hisec]\ncase_product = 77\nannounce = \"channel\"\n[economy]\nhisec_case_odds = [[1, 1]]\n";
    let c = parse_toml(text).unwrap();
    assert_eq!(c.evars.mode, "off");
    assert_eq!(c.evars.extra.get("MyVar").map(String::as_str), Some("1.5"));
    assert_eq!(c.combat.blast_inner, 0.5);
    assert_eq!(c.room.round_secs.default, 300);
    assert_eq!(c.room.round_secs.classes.get("1"), Some(&900));
    assert_eq!(c.room.round_secs.classes.get("12"), Some(&120));
    assert_eq!(
        (c.hisec.case_product, c.hisec.announce.as_str()),
        (77, "channel")
    );
    assert_eq!(c.economy.hisec_case_odds, vec![[1, 1]]);
    assert!(errors(&check(&c, CheckOptions::default())).is_empty());
    assert_eq!(parse_toml(&c.to_toml()).unwrap(), c);
    let (c, ex) = load_layered_explicit(
        Some("[room.round_secs]\n4 = 100\n"),
        env(&[
            ("DC_EVARS__MODE", "off"),
            ("DC_COMBAT__BLAST_INNER", "0"),
            ("DC_ROOM__ROUND_SECS__1", "700"),
            ("DC_HISEC__ANNOUNCE", "off"),
        ]),
        &CliOverrides::default(),
    )
    .unwrap();
    assert_eq!((c.evars.mode.as_str(), c.combat.blast_inner), ("off", 0.0));
    assert_eq!(c.room.round_secs.classes.get("1"), Some(&700));
    assert_eq!(c.room.round_secs.classes.get("4"), Some(&100));
    assert_eq!(c.hisec.announce, "off");
    assert!(ex.has("evars.mode") && ex.has("room.round_secs.1") && ex.has("room.round_secs.4"));
    assert!(!ex.has("room.round_secs.default") && !ex.has("hisec.case_product"));
    // refused values
    assert!(has_err(
        &bad(&|c| c.evars.mode = "all".into()),
        "evars.mode"
    ));
    assert!(has_err(
        &bad(&|c| {
            c.evars.extra.insert("a b".into(), "1".into());
        }),
        "evars.extra"
    ));
    assert!(has_err(
        &bad(&|c| c.combat.blast_inner = 1.5),
        "combat.blast_inner"
    ));
    assert!(has_err(
        &bad(&|c| {
            c.room.round_secs.classes.insert("tdm".into(), 5);
        }),
        "room.round_secs.tdm"
    ));
    assert!(has_err(
        &bad(&|c| {
            c.room.round_secs.classes.insert("300".into(), 5);
        }),
        "room.round_secs.300"
    ));
    assert!(has_err(
        &bad(&|c| c.hisec.announce = "loud".into()),
        "hisec.announce"
    ));
    assert!(has_err(
        &bad(&|c| c.hisec.no_key_reason = 0),
        "hisec.no_key_reason"
    ));
    assert!(has_err(
        &bad(&|c| c.economy.hisec_case_odds = vec![[1, 0]]),
        "economy.hisec_case_odds"
    ));
    assert!(parse_toml("[evars]\nmodes = \"world\"\n").is_err());
    assert!(parse_toml("[hisec]\nenabled = true\n").is_err());
    assert!(parse_toml("[room.round_secs]\n1 = \"long\"\n").is_err());
}

// ------------------------------------------------------------------ hosting examples (docs/SERVER.md, docs/VPS.md)

#[test]
fn hosting_examples_parse_and_validate() {
    // examples/server.toml: the Windows stand-alone server
    let c = parse_toml(&read("examples/server.toml")).expect("examples/server.toml parses");
    assert!(errors(&check(&c, CheckOptions::default())).is_empty());
    // examples/server-vps.toml: the VPS layout; its loopback advertise_ip is only a warning in the file ...
    let text = read("examples/server-vps.toml");
    let c = parse_toml(&text).expect("examples/server-vps.toml parses");
    let issues = check(&c, CheckOptions::default());
    assert!(errors(&issues).is_empty(), "{issues:?}");
    assert!(
        issues.iter().any(|i| i.path == "server.advertise_ip"),
        "{issues:?}"
    );
    assert!(c.server.public && !c.auth.auto_create && c.server.http_enabled);
    assert_eq!(c.effective_bind(), "0.0.0.0");
    assert_eq!(c.server.game_port, 27015);
    assert_eq!(c.dserver.port_range, "27016-27100");
    assert_eq!(c.paths.rom, "/opt/decombat/rom");
    assert_eq!(c.paths.db, "/opt/decombat/data/ca.db");
    assert_eq!(c.paths.log_dir, "/opt/decombat/logs");
    // ... and the address from decombat.env (bootstrap.sh --public-ip) clears it
    let c = load_layered(
        Some(&text),
        env(&[("DC_SERVER__ADVERTISE_IP", "203.0.113.7")]),
        &CliOverrides::default(),
    )
    .unwrap();
    assert_eq!(c.server.advertise_ip, "203.0.113.7");
    let issues = check(&c, CheckOptions::default());
    assert!(issues.is_empty(), "{issues:?}");
    // examples/decombat.env holds only keys the environment layer knows
    let vars: Vec<(String, String)> = read("examples/decombat.env")
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("DC_"))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert!(vars.len() >= 3, "{vars:?}");
    load_layered(Some(&text), vars, &CliOverrides::default()).expect("decombat.env keys apply");
}

/// LG-68: issuing needs an issuer id and a key pair path; accepting needs issuer and public key together.
#[test]
fn check_identity_section() {
    let run = |f: &dyn Fn(&mut Config)| {
        let mut c = Config::default();
        f(&mut c);
        check(&c, CheckOptions::default())
    };
    assert!(!has_err(&run(&|_| {}), "identity."));
    assert!(has_err(
        &run(&|c| c.identity.issue = true),
        "identity.issuer"
    ));
    assert!(has_err(
        &run(&|c| {
            c.identity.issue = true;
            c.identity.issuer = "decombat.vps".into();
            c.paths.db = String::new();
        }),
        "identity.key_file"
    ));
    assert!(!has_err(
        &run(&|c| {
            c.identity.issue = true;
            c.identity.issuer = "decombat.vps".into();
        }),
        "identity."
    ));
    assert!(has_err(
        &run(&|c| c.identity.issuer = "decombat.vps".into()),
        "identity.public_key"
    ));
    assert!(has_err(
        &run(&|c| c.identity.public_key = "A".repeat(43) + "="),
        "identity.public_key"
    ));
    assert!(has_err(
        &run(&|c| {
            c.identity.issuer = "decombat.vps".into();
            c.identity.public_key = "short".into();
        }),
        "identity.public_key"
    ));
    assert!(!has_err(
        &run(&|c| {
            c.identity.issuer = "decombat.vps".into();
            c.identity.public_key = "A".repeat(43) + "=";
        }),
        "identity."
    ));
    assert!(has_err(
        &run(&|c| c.identity.issuer = "bad|id".into()),
        "identity.issuer"
    ));
}

#[test]
fn a_partial_reward_table_fills_from_its_own_defaults() {
    // CD-86: GP agent_win stays 0 (the EXP 30 must not leak in), CD-04 GP finish 20 / objective 25 stay too
    let cfg = dc_config::parse_toml("[economy.gp]\nkill = 9\n").unwrap();
    let mut want = dc_config::schema::RewardRates::gp_default();
    want.kill = 9;
    assert_eq!(cfg.economy.gp, want);
    assert_eq!(cfg.economy.gp.agent_win, 0);
    let cfg = dc_config::parse_toml("[economy.exp]\nkill = 9\n").unwrap();
    assert_eq!(
        (cfg.economy.exp.agent_win, cfg.economy.exp.finish),
        (30, 10)
    );
    assert!(dc_config::parse_toml("[economy.gp]\nbogus = 1\n").is_err());
}

// ------------------------------------------------------------------ remote game hosts (HOST_AGENT.md §2, CD-88)

#[test]
fn hosts_section_defaults_and_validation() {
    // DC-DEFAULT: CD-88 (proposed)
    let d = Config::default();
    assert_eq!((d.hosts.port, d.hosts.agents.len()), (0, 0));
    assert_eq!(d.hosts.resolved_port(10000), 10005);
    let good = "[[lobby.servers]]
name = \"US Central\"
capacity = 1000
[[lobby.servers]]
name = \"US West\"
capacity = 1000
host = \"us-west\"
[[hosts.agents]]
name = \"us-west\"
token = \"0123456789abcdef01234567\"
";
    let c = parse_toml(good).unwrap();
    assert_eq!(
        (
            c.lobby.servers[0].host.as_str(),
            c.lobby.servers[0].channels
        ),
        ("", 3),
        "host and channels default to \"\" and 3"
    );
    assert_eq!(c.lobby.servers[1].host, "us-west");
    assert_eq!(c.hosts.agents[0].public_address, "");
    let issues = check(&c, CheckOptions::default());
    assert!(errors(&issues).is_empty(), "{issues:?}");
    assert!(
        !format!("{:?}", c.hosts.agents[0]).contains("0123456789"),
        "the token is never printed"
    );
    // a host that names no agent, a short token, a duplicate name, an empty name
    let bad = "[[lobby.servers]]
name = \"A\"
capacity = 10
host = \"nobody\"
[[lobby.servers]]
name = \"B\"
capacity = 10
channels = 0
[[hosts.agents]]
name = \"x\"
token = \"short\"
[[hosts.agents]]
name = \"x\"
token = \"0123456789abcdef01234567\"
[[hosts.agents]]
name = \"\"
token = \"0123456789abcdef01234567\"
";
    let issues = check(&parse_toml(bad).unwrap(), CheckOptions::default());
    assert!(has_err(&issues, "lobby.servers[0].host"), "{issues:?}");
    assert!(has_err(&issues, "lobby.servers[1].channels"), "{issues:?}");
    assert!(has_err(&issues, "hosts.agents[0].token"), "{issues:?}");
    assert!(has_err(&issues, "hosts.agents[1].name"), "{issues:?}");
    assert!(has_err(&issues, "hosts.agents[2].name"), "{issues:?}");
    assert!(!has_err(&issues, "hosts.agents[1].token"), "{issues:?}");
    // the host port: out of range, or one of the lobby's own ports
    let port = |p: &str| {
        let t = format!(
            "[server]\nauth_port = {p}\n[[hosts.agents]]\nname = \"a\"\ntoken = \"0123456789abcdef01234567\"\n"
        );
        check(&parse_toml(&t).unwrap(), CheckOptions::default())
    };
    assert!(has_err(&port("65534"), "hosts.port"));
    assert!(!has_err(&port("10000"), "hosts.port"));
    let clash = "[server]\nlobby_port = 10005\n[[hosts.agents]]\nname = \"a\"\ntoken = \"0123456789abcdef01234567\"\n";
    assert!(has_err(
        &check(&parse_toml(clash).unwrap(), CheckOptions::default()),
        "hosts.port"
    ));
    // no agent: no host port, so no port check
    let none =
        "[server]\nauth_port = 65534\nlobby_port = 1000\nclan_port = 1001\nhttp_port = 1002\n";
    assert!(!has_err(
        &check(&parse_toml(none).unwrap(), CheckOptions::default()),
        "hosts.port"
    ));
    // more than 64 channels over the entries
    let many = "[[lobby.servers]]\nname = \"A\"\ncapacity = 1\nchannels = 40\n[[lobby.servers]]\nname = \"B\"\ncapacity = 1\nchannels = 30\n";
    assert!(has_err(
        &check(&parse_toml(many).unwrap(), CheckOptions::default()),
        "lobby.servers"
    ));
}

// ------------------------------------------------------------------ promotion gifts (CD-89)

/// CD-89: `[[rewards.rank_gifts]]` parses from the file (empty by default), and check refuses a rank outside
/// 1..=255, a rank listed twice, and an entry without products or with product id 0.
#[test]
fn rank_gifts_parse_and_are_checked() {
    assert!(Config::default().rewards.rank_gifts.is_empty());
    let c = parse_toml(
        "[[rewards.rank_gifts]]\nrank = 5\nproducts = [101, 102]\n\
         [[rewards.rank_gifts]]\nrank = 10\nproducts = [7]\n",
    )
    .expect("parses");
    assert_eq!(
        c.rewards.rank_gifts,
        vec![
            RankGift {
                rank: 5,
                products: vec![101, 102],
            },
            RankGift {
                rank: 10,
                products: vec![7],
            },
        ]
    );
    assert!(errors(&check(&c, CheckOptions::default())).is_empty());
    let with = |gifts: Vec<RankGift>| {
        let mut c = Config::default();
        c.rewards.rank_gifts = gifts;
        check(&c, CheckOptions::default())
    };
    let gift = |rank: u32, products: Vec<u32>| RankGift { rank, products };
    assert!(has_err(
        &with(vec![gift(0, vec![1])]),
        "rewards.rank_gifts[0].rank"
    ));
    assert!(has_err(
        &with(vec![gift(256, vec![1])]),
        "rewards.rank_gifts[0].rank"
    ));
    assert!(has_err(
        &with(vec![gift(3, vec![1]), gift(3, vec![2])]),
        "rewards.rank_gifts[1].rank"
    ));
    assert!(has_err(
        &with(vec![gift(3, vec![])]),
        "rewards.rank_gifts[0].products"
    ));
    assert!(has_err(
        &with(vec![gift(3, vec![5, 0])]),
        "rewards.rank_gifts[0].products"
    ));
    assert!(parse_toml("[[rewards.rank_gifts]]\nrank = 5\nitems = [1]\n").is_err());
    // the unlocked variant's days and the case product
    let d = Config::default();
    assert_eq!(
        (
            d.rewards.rank_unlock_days,
            d.rewards.rank_case_products.len()
        ),
        (7, 0)
    );
    let c = parse_toml("[rewards]\nrank_unlock_days = 30\nrank_case_products = [5, 6]\n").unwrap();
    assert_eq!(
        (
            c.rewards.rank_unlock_days,
            &c.rewards.rank_case_products[..]
        ),
        (30, &[5, 6][..])
    );
    assert!(errors(&check(&c, CheckOptions::default())).is_empty());
    let bad = |f: &dyn Fn(&mut Config)| {
        let mut c = Config::default();
        f(&mut c);
        check(&c, CheckOptions::default())
    };
    assert!(has_err(
        &bad(&|c| c.rewards.rank_unlock_days = 0),
        "rewards.rank_unlock_days"
    ));
    assert!(has_err(
        &bad(&|c| c.rewards.rank_unlock_days = 3651),
        "rewards.rank_unlock_days"
    ));
    assert!(has_err(
        &bad(&|c| c.rewards.rank_case_products = vec![3, 0]),
        "rewards.rank_case_products"
    ));
}

// DC-DEFAULT: LG-310, LG-311 (proposed): the NX rewards work with no [rewards] section; bad ranges are refused
#[test]
fn nx_rewards_default_on_and_ranges_checked() {
    let cfg = Config::default();
    assert!(cfg.rewards.nx_enabled && cfg.rewards.nx_daily > 0 && cfg.rewards.nx_clan_win > 0);
    let errs = |f: fn(&mut Config)| {
        let mut c = Config::default();
        f(&mut c);
        check(&c, CheckOptions::default())
            .iter()
            .map(|i| format!("{i:?}"))
            .collect::<Vec<_>>()
            .join("|")
    };
    assert!(!errs(|_| {}).contains("rewards.nx_"));
    assert!(errs(|c| c.rewards.nx_daily = 1_000_001).contains("rewards.nx_daily"));
    assert!(errs(|c| c.rewards.nx_clan_win_daily_cap = 0).contains("rewards.nx_clan_win_daily_cap"));
    assert!(errs(|c| c.rewards.nx_clan_min_players = 9).contains("rewards.nx_clan_min_players"));
    assert!(errs(|c| c.rewards.nx_clan_min_seconds = 3601).contains("rewards.nx_clan_min_seconds"));
}

// LG-322: client versions are numbers, the oldest allowed not above the newest
#[test]
fn client_versions_are_checked() {
    let issues = |latest: &str, min: &str| {
        let mut c = Config::default();
        c.client.latest = latest.into();
        c.client.min_version = min.into();
        check(&c, CheckOptions::default())
    };
    let ok = issues("0.4.0", "0.3.0");
    assert!(!has_err(&ok, "client.latest") && !has_err(&ok, "client.min_version"));
    let unset = issues("", "");
    assert!(!has_err(&unset, "client.latest") && !has_err(&unset, "client.min_version"));
    assert!(has_err(&issues("soon", ""), "client.latest"));
    assert!(has_err(&issues("", "1.2"), "client.min_version"));
    assert!(has_err(&issues("0.3.0", "0.4.0"), "client.min_version"));
    let parsed = dc_config::parse_toml("[client]\nlatest = \"0.4.0\"\n").unwrap();
    assert_eq!(parsed.client.latest, "0.4.0");
    assert_eq!(parsed.client.update_url, "decombat.com");
}

// LG-370: the beta section is off by default and, when on, needs plain origins and one reply address
#[test]
fn beta_section_is_checked_only_when_enabled() {
    let issues = |f: &dyn Fn(&mut Config)| {
        let mut c = Config::default();
        c.beta.enabled = true;
        c.mail.dry_run = true;
        f(&mut c);
        check(&c, CheckOptions::default())
    };
    let d = Config::default();
    assert!(
        !d.beta.enabled
            && d.beta.max_mail_per_day == 90
            && d.beta.site_origin == "https://decombat.com"
    );
    let mut off = Config::default();
    off.beta.site_origin = "nonsense".into();
    assert!(!has_err(
        &check(&off, CheckOptions::default()),
        "beta.site_origin"
    ));
    assert!(!has_err(&issues(&|_| {}), "beta.site_origin"));
    assert!(has_err(
        &issues(&|c| c.beta.site_origin = "https://decombat.com/".into()),
        "beta.site_origin"
    ));
    assert!(has_err(
        &issues(&|c| c.beta.site_origin = "https://decombat.com/x".into()),
        "beta.site_origin"
    ));
    assert!(has_err(
        &issues(&|c| c.beta.site_origin = "ftp://decombat.com".into()),
        "beta.site_origin"
    ));
    assert!(has_err(
        &issues(&|c| c.beta.public_url = "https://beta.example.org/".into()),
        "beta.public_url"
    ));
    assert!(has_err(
        &issues(&|c| c.beta.public_url = "beta.example.org".into()),
        "beta.public_url"
    ));
    assert!(has_err(
        &issues(&|c| c.beta.reply_to = "a@b@example.org".into()),
        "beta.reply_to"
    ));
    assert!(!has_err(
        &issues(&|c| c.beta.reply_to = "contact@example.org".into()),
        "beta.reply_to"
    ));
    let parsed = dc_config::parse_toml(
        "[beta]
enabled = true
trust_cf_header = true
",
    )
    .unwrap();
    assert!(parsed.beta.enabled && parsed.beta.trust_cf_header && parsed.beta.invite_days == 14);
}

// LG-382 / LG-383: the admin web section is off by default and, when on, loopback-only with an https origin
#[test]
fn admin_web_section_is_checked_only_when_enabled() {
    let issues = |f: &dyn Fn(&mut Config)| {
        let mut c = Config::default();
        c.admin_web.enabled = true;
        f(&mut c);
        check(&c, CheckOptions::default())
    };
    let d = Config::default();
    assert!(
        !d.admin_web.enabled && d.admin_web.bind == "127.0.0.1:10096" && d.admin_web.max_keys == 4
    );
    let mut off = Config::default();
    off.admin_web.bind = "0.0.0.0:10096".into();
    assert!(!has_err(
        &check(&off, CheckOptions::default()),
        "admin_web.bind"
    ));
    assert!(!has_err(&issues(&|_| {}), "admin_web"));
    assert!(has_err(
        &issues(&|c| c.admin_web.bind = "0.0.0.0:10096".into()),
        "admin_web.bind"
    ));
    assert!(has_err(
        &issues(&|c| c.admin_web.bind = "nonsense".into()),
        "admin_web.bind"
    ));
    assert!(has_err(
        &issues(&|c| c.admin_web.public_origin = "http://admin.example.org".into()),
        "admin_web.public_origin"
    ));
    assert!(has_err(
        &issues(&|c| c.admin_web.public_origin = "https://admin.example.org/".into()),
        "admin_web.public_origin"
    ));
    assert!(has_err(
        &issues(&|c| c.admin_web.rp_id = "example.org".into()),
        "admin_web.rp_id"
    ));
    assert!(!has_err(
        &issues(&|c| {
            c.admin_web.public_origin = "https://admin.example.org".into();
            c.admin_web.rp_id = "example.org".into();
        }),
        "admin_web.rp_id"
    ));
    assert!(has_err(
        &issues(&|c| c.admin_web.max_keys = 0),
        "admin_web.max_keys"
    ));
    let parsed = dc_config::parse_toml(
        "[admin_web]
enabled = true
max_keys = 2
",
    )
    .unwrap();
    assert!(
        parsed.admin_web.enabled
            && parsed.admin_web.max_keys == 2
            && parsed.admin_web.idle_minutes == 30
    );
}

// LG-399: further instances behind the admin web: path-safe unique names, labels, loopback admin ports
#[test]
fn admin_web_instances_are_parsed_and_checked() {
    let parsed = dc_config::parse_toml(
        "[admin_web]
enabled = true
label = \"Vanilla lobby\"
instances = [{ name = \"modded\", label = \"Hacking Mod lobby\", admin = \"127.0.0.1:10091\" }]
",
    )
    .unwrap();
    let a = &parsed.admin_web;
    assert_eq!(a.label, "Vanilla lobby");
    assert_eq!(a.instances.len(), 1);
    assert_eq!(
        (a.instances[0].name.as_str(), a.instances[0].admin.as_str()),
        ("modded", "127.0.0.1:10091")
    );
    assert!(a.instances[0].env_file.is_empty());
    assert!(!has_err(
        &check(&parsed, CheckOptions::default()),
        "admin_web"
    ));
    assert!(Config::default().admin_web.instances.is_empty());
    let bad = |f: &dyn Fn(&mut dc_config::AdminWebInstance)| {
        let mut c = parsed.clone();
        f(&mut c.admin_web.instances[0]);
        check(&c, CheckOptions::default())
    };
    assert!(has_err(
        &bad(&|i| i.name = "Mod".into()),
        "admin_web.instances.name"
    ));
    assert!(has_err(
        &bad(&|i| i.name = "../x".into()),
        "admin_web.instances.name"
    ));
    assert!(has_err(
        &bad(&|i| i.name = String::new()),
        "admin_web.instances.name"
    ));
    assert!(has_err(
        &bad(&|i| i.label = " ".into()),
        "admin_web.instances.label"
    ));
    assert!(has_err(
        &bad(&|i| i.admin = "192.0.2.4:10091".into()),
        "admin_web.instances.admin"
    ));
    assert!(has_err(
        &bad(&|i| i.admin = "127.0.0.1:10096".into()),
        "admin_web.instances.admin"
    ));
    let mut twice = parsed.clone();
    twice
        .admin_web
        .instances
        .push(twice.admin_web.instances[0].clone());
    assert!(has_err(
        &check(&twice, CheckOptions::default()),
        "admin_web.instances.name"
    ));
    assert!(
        dc_config::parse_toml("[admin_web]\ninstances = [{ name = \"m\", token = \"x\" }]\n")
            .is_err()
    );
}

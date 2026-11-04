//! Server-hosted mods (`--mods-dir`). `dc_mod_lua::ModHost` is single-threaded (Lua state, `Rc`), so it lives
//! on its own thread; handlers call it synchronously over a channel with a timeout, and fall back to the DeCombat
//! default when a mod is slow or broken. Hooks exposed so far: `lobby.calc_exp_gp`.
// DOC: RUST_CONVERSION_PLAN.md §4 (server-hosted mods)

use crate::config::Rules;
use crate::logs::Logger;
use crate::rewards::{self, RewardCtx};
use dc_bute::modpatch::WeaponContent;
use dc_mod_lua::{ModHost, ModPolicy};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

enum Request {
    CalcExpGp {
        ctx: RewardCtx,
        rules: Box<Rules>,
        reply: Sender<Result<(i64, i64), String>>,
    },
}

enum SourceKind {
    Discover,
    Declared,
    Curated,
}

enum Source {
    /// Every folder with a `manifest.json` (no declared set).
    Discover(std::path::PathBuf),
    /// The declared set, in its order.
    Declared(Vec<crate::modset::Resolved>),
    /// LG-313: a featured entry's curated set: a pack that fails is logged and left out.
    Curated(Vec<crate::modset::Resolved>),
}

pub struct ModClient {
    tx: Mutex<Sender<Request>>,
    pub loaded: Vec<String>,
    /// MOD-21: the weapon-table content of the loaded set (`mod.content.weapons`), empty without one.
    pub content: WeaponContent,
    /// LG-271 / MOD-21 amendment: the part of [`ModClient::content`] a client computes (only mods with
    /// `requires_client`, i.e. `shared` / `client_visual`): the lobby expects this hash at login.
    pub client_content: WeaponContent,
    /// The content document [`ModClient::write_content`] wrote (passed to every game server as `-content`).
    pub content_file: Option<PathBuf>,
    /// MOD-22: the id of a loaded mod that declares `world.full_visibility` (every game server runs culling off).
    pub culling_off_by: Option<String>,
    /// MOD-23: the id of a loaded mod that declares `combat.no_recoil` (rooms may turn recoil off, LG-274).
    pub no_recoil_by: Option<String>,
    /// MAP-39: the community maps of the loaded set as plain data, in table order (`Core::install_community_maps`).
    pub maps: Vec<crate::maps::CommunityMap>,
}

/// What the mods thread reports once the set is loaded: plain data only (the `ModHost` stays on its thread).
type Loaded = (
    Vec<String>,
    WeaponContent,
    WeaponContent,
    Vec<crate::maps::CommunityMap>,
);

/// Folder of the content documents inside the lobby's data folder (next to the DB), shared with the mod cache.
pub const CONTENT_DIR: &str = "mod_cache";

/// File name of a content document: `content-<hash>.json`.
pub fn content_file_name(content: &WeaponContent) -> String {
    format!("content-{}.json", content.hash())
}

/// File name of the client-visible content document (for an operator to inspect): `content-client-<hash>.json`.
pub fn client_content_file_name(content: &WeaponContent) -> String {
    format!("content-client-{}.json", content.hash())
}

const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// LG-313 / LG-314: loads a pack alone under the curated policy and checks the weapon rows it makes.
fn curated_trial(dir: &Path) -> Result<(), String> {
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Curated);
    host.load_mod_dir(dir)?;
    match dc_mod_lua::curated_content_refusal(&host.weapon_patches()?) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// The community maps of a loaded host: all or the first fault for a declared set (MAP-39), the good ones with a log
/// line per fault for a curated set (MAP-81).
fn curated_or_declared_maps(
    host: &ModHost,
    curated: bool,
    log: &Logger,
) -> Result<Vec<crate::maps::CommunityMap>, String> {
    if !curated {
        return crate::maps::community_maps(host);
    }
    let mut maps = Vec::new();
    for one in crate::maps::community_maps_each(host)? {
        match one {
            Ok(m) => maps.push(m),
            Err(why) => log.line(&format!("mods: curated map left out: {why}")),
        }
    }
    Ok(maps)
}

impl ModClient {
    /// Fallback without a declared set (CLI-only runs, tests): loads every mod folder under `dir` on a dedicated
    /// thread. Load errors are logged; the server runs on.
    pub fn start(dir: &Path, log: Arc<Logger>, policy: Option<ModPolicy>) -> Option<ModClient> {
        Self::spawn(Source::Discover(dir.to_path_buf()), log, policy)
            .ok()
            .flatten()
    }

    /// LG-313: the mod host of a featured entry: its own thread and Lua state under [`ModPolicy::Curated`]. A pack that
    /// the policy refuses, that does not load, or whose weapon rows break LG-314 is logged and left out
    /// (`loaded` names the rest). `Err`: the packs do not fit together; the entry then stays plain.
    pub fn start_curated(
        mods: Vec<crate::modset::Resolved>,
        log: Arc<Logger>,
    ) -> Result<ModClient, String> {
        match Self::spawn(Source::Curated(mods), log, Some(ModPolicy::Curated))? {
            Some(c) => Ok(c),
            None => Err("the mod thread could not start".to_string()),
        }
    }

    /// LG-270: loads exactly the declared mods (`modset::resolve_declared`), in their order. A mod that fails
    /// to load is an error: the caller refuses to start.
    pub fn start_declared(
        mods: Vec<crate::modset::Resolved>,
        log: Arc<Logger>,
        policy: Option<ModPolicy>,
    ) -> Result<ModClient, String> {
        match Self::spawn(Source::Declared(mods), log, policy)? {
            Some(c) => Ok(c),
            None => Err("the mod thread could not start".to_string()),
        }
    }

    fn spawn(
        source: Source,
        log: Arc<Logger>,
        policy: Option<ModPolicy>,
    ) -> Result<Option<ModClient>, String> {
        let (tx, rx) = channel::<Request>();
        let (ready_tx, ready_rx) = channel::<Result<Loaded, String>>();
        let spawned = std::thread::Builder::new()
            .name("mods".into())
            .spawn(move || {
                let mut host = ModHost::new();
                if let Some(p) = policy {
                    // vanilla = every load is refused (HOSTING_AND_MODS.md §8 item 1)
                    host.set_policy(p);
                }
                let source_kind = match &source {
                    Source::Discover(_) => SourceKind::Discover,
                    Source::Declared(_) => SourceKind::Declared,
                    Source::Curated(_) => SourceKind::Curated,
                };
                let loaded = match source {
                    Source::Discover(dir) => match host.discover_and_load_dir(&dir) {
                        Ok(ids) => Ok(ids),
                        Err(e) => {
                            log.line(&format!("mods: loading {} failed: {}", dir.display(), e));
                            Ok(Vec::new())
                        }
                    },
                    Source::Curated(mods) => {
                        let mut ids = Vec::new();
                        for m in mods {
                            // a scratch host first: a refused pack leaves nothing behind in this entry's Lua state
                            let tried = curated_trial(&m.dir)
                                .and_then(|()| host.load_mod_dir(&m.dir).map(|_| ()));
                            match tried {
                                Ok(()) => ids.push(m.id),
                                Err(e) => log
                                    .line(&format!("mods: curated pack '{}' left out: {e}", m.id)),
                            }
                        }
                        Ok(ids)
                    }
                    Source::Declared(mods) => {
                        let mut ids = Vec::new();
                        let mut err = None;
                        for m in mods {
                            match host.load_mod_dir(&m.dir) {
                                Ok(_) => ids.push(m.id),
                                Err(e) => {
                                    err = Some(format!(
                                        "mod '{}' ({}) did not load: {}",
                                        m.id,
                                        m.dir.display(),
                                        e
                                    ));
                                    break;
                                }
                            }
                        }
                        match err {
                            Some(e) => Err(e),
                            None => Ok(ids),
                        }
                    }
                };
                let declared = matches!(source_kind, SourceKind::Declared | SourceKind::Curated);
                let curated = matches!(source_kind, SourceKind::Curated);
                // MOD-21: the content records as plain data; a declared set with a bad record does not start
                let loaded = loaded.and_then(|ids| {
                    match host.weapon_patches().and_then(|all| {
                        host.weapon_patches_of(|m| m.requires_client)
                            .map(|cl| (all, cl))
                    }) {
                        Ok((c, cl)) => Ok((ids, c, cl)),
                        Err(e) if declared => Err(format!("mod content: {e}")),
                        Err(e) => {
                            log.line(&format!("mods: content ignored: {e}"));
                            Ok((ids, WeaponContent::default(), WeaponContent::default()))
                        }
                    }
                });
                // MAP-39: the community maps as plain data, computed here (the host never leaves this thread); a
                // faulty record stops a declared set like a bad content record does; MAP-81: a curated set leaves
                // that record out with a log line and keeps the rest
                let loaded = loaded.and_then(|(ids, c, cl)| {
                    match curated_or_declared_maps(&host, curated, &log) {
                        Ok(maps) => Ok((ids, c, cl, maps)),
                        Err(e) if declared => Err(format!("mod maps: {e}")),
                        Err(e) => {
                            log.line(&format!("mods: maps ignored: {e}"));
                            Ok((ids, c, cl, Vec::new()))
                        }
                    }
                });
                let failed = loaded.is_err();
                let _ = ready_tx.send(loaded);
                if failed {
                    return;
                }
                for req in rx {
                    match req {
                        Request::CalcExpGp { ctx, rules, reply } => {
                            let table = rewards::to_table(&ctx);
                            let r = host.run_hook(rewards::HOOK, table, move |t| {
                                let c = rewards::from_table(t);
                                let (e, g) = rewards::base(&rules, &c);
                                Ok(rewards::reward_table(e, g))
                            });
                            let out = match r {
                                Ok(v) => rewards::read_result(&v)
                                    .ok_or_else(|| "hook returned no {exp, gp} table".to_string()),
                                Err(e) => Err(e),
                            };
                            for (id, errs) in host.all_mod_errors() {
                                if let Some(last) = errs.last() {
                                    log.line(&format!("mods: {} error: {}", id, last));
                                }
                            }
                            let _ = reply.send(out);
                        }
                    }
                }
            });
        if spawned.is_err() {
            return Ok(None);
        }
        let (loaded, content, client_content, maps) = ready_rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| Err("loading the mods timed out".to_string()))?;
        Ok(Some(ModClient {
            tx: Mutex::new(tx),
            loaded,
            content,
            client_content,
            content_file: None,
            culling_off_by: None,
            no_recoil_by: None,
            maps,
        }))
    }

    /// MOD-21 / DSV-478: writes the set's full weapon content (and the client-visible part, `content-client-<hash>.json`) to `<data_dir>/mod_cache/content-<hash>.json` (once: a
    /// file of that name already holds the same document; other `content-*.json` files are removed) and keeps the
    /// path for the game-server launches.
    /// Vanilla content (no patch at all) writes nothing and returns `None`.
    // DC-DEFAULT: MOD-21 (proposed), DSV-478 (proposed)
    pub fn write_content(&mut self, data_dir: &Path) -> Result<Option<PathBuf>, String> {
        if self.content.is_empty() {
            self.content_file = None;
            return Ok(None);
        }
        let dir = data_dir.join(CONTENT_DIR);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let name = content_file_name(&self.content);
        let path = dir.join(&name);
        // the client-visible document (inspection only; the lobby holds its hash)
        let client_name = (!self.client_content.is_empty())
            .then(|| client_content_file_name(&self.client_content));
        // documents of an earlier set are not used by any game server of this start
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                let keep = n == name || Some(&n) == client_name.as_ref();
                if !keep && n.starts_with("content-") && n.ends_with(".json") && e.path().is_file()
                {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let write_once = |p: &Path, doc: &str| -> Result<(), String> {
            if std::fs::read_to_string(p).ok().as_deref() != Some(doc) {
                std::fs::write(p, doc).map_err(|e| format!("cannot write {}: {e}", p.display()))?;
            }
            Ok(())
        };
        write_once(&path, &self.content.to_json())?;
        if let Some(cn) = client_name {
            write_once(&dir.join(cn), &self.client_content.to_json())?;
        }
        self.content_file = Some(path.clone());
        Ok(Some(path))
    }

    pub fn calc_exp_gp(&self, ctx: &RewardCtx, rules: &Rules) -> Result<(i64, i64), String> {
        let (rtx, rrx) = channel();
        let req = Request::CalcExpGp {
            ctx: ctx.clone(),
            rules: Box::new(rules.clone()),
            reply: rtx,
        };
        self.tx
            .lock()
            .map_err(|_| "mod channel poisoned".to_string())?
            .send(req)
            .map_err(|_| "mod thread gone".to_string())?;
        rrx.recv_timeout(CALL_TIMEOUT)
            .map_err(|_| "mod hook timed out".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_wraps_the_default() {
        let dir = std::env::temp_dir().join(format!("ca_lobby_mods_{}", std::process::id()));
        let m = dir.join("double_exp");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("manifest.json"),
            r#"{"id": "double_exp", "name": "Double EXP", "version": "1.0.0", "entry": "main.lua",
       "kind": "server_rules", "description": "A test fixture mod for the unit tests.", "author": "tests", "capabilities": ["hooks.lobby"]}"#,
        )
        .unwrap();
        std::fs::write(
            m.join("main.lua"),
            r#"return function(mod)
                mod.hooks:wrap("lobby.calc_exp_gp", function(base, ctx)
                    local r = base(ctx)
                    r.exp = r.exp * 2
                    if ctx.won then r.gp = r.gp + 1 end
                    return r
                end)
            end"#,
        )
        .unwrap();
        let c = ModClient::start(&dir, Arc::new(Logger::none()), None).expect("mod thread");
        assert_eq!(c.loaded, vec!["double_exp".to_string()]);
        let ctx = RewardCtx {
            kills: 2,
            team: 1,
            winner: 1,
            exp_factor: 1.0,
            gp_factor: 1.0,
            ..Default::default()
        };
        let rules = Rules::default();
        let (e, g) = c.calc_exp_gp(&ctx, &rules).unwrap();
        let (be, bg) = rewards::base(&rules, &ctx);
        assert_eq!((e, g), (be * 2, bg + 1));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn vanilla_policy_refuses_every_load() {
        let dir = std::env::temp_dir().join(format!("ca_lobby_vanilla_{}", std::process::id()));
        let m = dir.join("m1");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("manifest.json"),
            r#"{"id": "m1", "name": "M", "version": "1.0.0", "entry": "main.lua",
       "kind": "server_rules", "description": "A test fixture mod for the unit tests.", "author": "tests", "capabilities": ["hooks.lobby"]}"#,
        )
        .unwrap();
        std::fs::write(m.join("main.lua"), "return function(mod) end").unwrap();
        let c = ModClient::start(&dir, Arc::new(Logger::none()), Some(ModPolicy::Vanilla)).unwrap();
        assert!(
            c.loaded.is_empty(),
            "vanilla = no mods at all: {:?}",
            c.loaded
        );
        let c = ModClient::start(
            &dir,
            Arc::new(Logger::none()),
            Some(ModPolicy::Modded {
                client_visual_allowed: false,
            }),
        )
        .unwrap();
        assert_eq!(c.loaded, vec!["m1".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_mods_means_the_default() {
        let dir = std::env::temp_dir().join(format!("ca_lobby_nomods_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = ModClient::start(&dir, Arc::new(Logger::none()), None).unwrap();
        assert!(c.loaded.is_empty());
        let ctx = RewardCtx {
            kills: 1,
            team: 2,
            winner: 1,
            exp_factor: 1.0,
            gp_factor: 1.0,
            ..Default::default()
        };
        let rules = Rules::default();
        assert_eq!(
            c.calc_exp_gp(&ctx, &rules).unwrap(),
            rewards::base(&rules, &ctx)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// MOD-21: a loaded mod's `mod.content.weapons` records reach the content document the game servers get.
    #[test]
    fn weapon_patches_reach_the_content_document() {
        let dir = std::env::temp_dir().join(format!("ca_lobby_content_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let m = dir.join("mods").join("hot_rifle");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("manifest.json"),
            r#"{"id": "hot_rifle", "name": "Hot Rifle", "version": "1.0.0", "entry": "main.lua",
       "kind": "server_rules", "description": "A test fixture mod that patches one weapon row.", "author": "tests",
       "capabilities": ["content.weapons"]}"#,
        )
        .unwrap();
        std::fs::write(
            m.join("main.lua"),
            r#"return function(mod)
                mod.content.weapons:patch(145, {AmmoDamage = 41})
                mod.content.weapons:patch("Ammo3", {InstDamage = 12.5, Name = "Hot"})
                mod.content.weapons:register({id = "Weapon900", Name = "New Gun", ShotsPerClip = 7})
            end"#,
        )
        .unwrap();
        let mut c = ModClient::start(&dir.join("mods"), Arc::new(Logger::none()), None)
            .expect("mod thread");
        assert_eq!(c.loaded, vec!["hot_rifle".to_string()]);
        std::fs::create_dir_all(dir.join(CONTENT_DIR)).unwrap();
        std::fs::write(dir.join(CONTENT_DIR).join("content-old.json"), "{}").unwrap();
        let path = c.write_content(&dir).unwrap().expect("a content file");
        assert!(
            !dir.join(CONTENT_DIR).join("content-old.json").exists(),
            "an earlier set's document is removed"
        );
        assert_eq!(
            path,
            dir.join(CONTENT_DIR).join(content_file_name(&c.content))
        );
        let doc = std::fs::read_to_string(&path).unwrap();
        assert!(
            doc.contains(r#"{"weapon":145,"op":"patch","fields":{"AmmoDamage":"41"}}"#),
            "{doc}"
        );
        assert!(
            doc.contains(
                r#"{"weapon":900,"op":"register","fields":{"Name":"New Gun","ShotsPerClip":"7"}}"#
            ),
            "{doc}"
        );
        assert!(
            doc.contains(r#"{"ammo":3,"op":"patch","fields":{"InstDamage":"12.5","Name":"Hot"}}"#),
            "{doc}"
        );
        let back = WeaponContent::from_json(&doc).unwrap();
        assert_eq!(back.hash(), c.content.hash());
        assert_eq!(crate::game::content_arg(Some(&c)), Some(path.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn write_mod(root: &Path, id: &str, kind: &str, lua: &str) {
        let m = root.join(id);
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("manifest.json"),
            format!(
                r#"{{"id": "{id}", "name": "{id}", "version": "1.0.0", "entry": "main.lua", "kind": "{kind}",
       "description": "A test fixture mod for the unit tests.", "author": "tests",
       "screenshots": ["screenshots/main.png"], "capabilities": ["content.weapons"]}}"#
            ),
        )
        .unwrap();
        std::fs::write(m.join("main.lua"), lua).unwrap();
    }

    fn hash_of(json: &str) -> String {
        WeaponContent::from_json(json).unwrap().hash()
    }

    /// LG-271 / MOD-21 amendment: the client-visible content covers `requires_client` mods only; the game
    /// servers get both.
    #[test]
    fn the_client_content_is_the_shared_mods_patches_alone() {
        let dir =
            std::env::temp_dir().join(format!("ca_lobby_clientcontent_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mods = dir.join("mods");
        write_mod(
            &mods,
            "a_rules",
            "server_rules",
            "return function(mod) mod.content.weapons:patch(145, {AmmoDamage = 41}) end",
        );
        write_mod(
            &mods,
            "b_shared",
            "shared",
            "return function(mod) mod.content.weapons:patch(146, {ShotsPerClip = 9}) end",
        );
        let mut c = ModClient::start(&mods, Arc::new(Logger::none()), None).expect("mod thread");
        assert_eq!(c.loaded.len(), 2, "{:?}", c.loaded);
        let path = c.write_content(&dir).unwrap().expect("a content file");
        let full = std::fs::read_to_string(&path).unwrap();
        assert!(
            full.contains(r#""weapon":145"#) && full.contains(r#""weapon":146"#),
            "{full}"
        );
        let shared_only = hash_of(
            r#"{"version":1,"weapons":[{"weapon":146,"op":"patch","fields":{"ShotsPerClip":"9"}}],"ammo":[]}"#,
        );
        assert_eq!(c.client_content.hash(), shared_only);
        assert_ne!(c.client_content.hash(), c.content.hash());
        let cf = dir
            .join(CONTENT_DIR)
            .join(client_content_file_name(&c.client_content));
        assert_eq!(
            WeaponContent::from_json(&std::fs::read_to_string(cf).unwrap())
                .unwrap()
                .hash(),
            shared_only
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Only `server_rules` patches: the game servers get them, the client content is empty (login `c=0`).
    #[test]
    fn server_rules_patches_leave_the_client_content_empty() {
        let dir = std::env::temp_dir().join(format!("ca_lobby_rulesonly_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mods = dir.join("mods");
        write_mod(
            &mods,
            "a_rules",
            "server_rules",
            "return function(mod) mod.content.weapons:patch(145, {AmmoDamage = 41}) end",
        );
        let mut c = ModClient::start(&mods, Arc::new(Logger::none()), None).expect("mod thread");
        assert!(c.write_content(&dir).unwrap().is_some());
        assert!(!c.content.is_empty() && c.client_content.is_empty());
        let names: Vec<_> = std::fs::read_dir(dir.join(CONTENT_DIR))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(names.len(), 1, "no client document without client content");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Vanilla content: no file, no `-content`.
    #[test]
    fn no_weapon_patches_write_no_content() {
        let dir = std::env::temp_dir().join(format!("ca_lobby_nocontent_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = ModClient::start(&dir, Arc::new(Logger::none()), None).unwrap();
        assert_eq!(c.write_content(&dir).unwrap(), None);
        assert!(!dir.join(CONTENT_DIR).exists());
        assert_eq!(crate::game::content_arg(Some(&c)), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// MOD-22: the culling-off flag reaches the game-server launch only when a loaded mod asked for it.
    #[test]
    fn culling_arg_needs_a_declaring_mod() {
        let dir = std::env::temp_dir().join(format!("ca_lobby_cull_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = ModClient::start(&dir, Arc::new(Logger::none()), None).unwrap();
        assert_eq!(crate::game::culling_arg(Some(&c)), None);
        assert_eq!(crate::game::culling_arg(None), None);
        c.culling_off_by = Some("xray".into());
        assert_eq!(crate::game::culling_arg(Some(&c)), Some("off"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

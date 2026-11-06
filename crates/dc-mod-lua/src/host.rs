//! Mod host: loads mods into the Lua sandbox, injects the `mod` capability table, and owns the
//! content registries, middleware hooks, events, saves and options.
// DOC: docs/MODDING_GUIDE.md

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use mlua::{Function, Lua, MultiValue, Table, Value};

use crate::manifest::{content_capability, hook_capability, JsonValue, ModKind, ModManifest};
use crate::resolver::{resolve_mods, DiscoveredMod};
use crate::sandbox::{
    check_not_bytecode, from_lua, guarded, host_err, runtime_err, strip_receiver, to_lua,
    LuaSandbox, ModEnv, SandboxLimits,
};
use crate::value::{LuaError, LuaKey, LuaTable, LuaValue};

// DC-DEFAULT: MAP-4 (proposed) `maps`: community map records (MAPKIT_PLAN.md D2), see DC_DEFAULTS.md
pub const CONTENT_DOMAINS: &[&str] = &[
    "weapons",
    "items",
    "shop",
    "gameroom",
    "ai_brains",
    "missions",
    "text",
    "music",
    "hud",
    "maps",
];

/// Who put a mod in front of the host (HOSTING_AND_MODS.md sections 3 and 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModOrigin {
    /// Named in the server's mod set (declared by the server, hash-pinned by the client).
    Server,
    /// Installed by the player on this machine; never part of a server's mod set.
    Local,
}

/// What the server a host is attached to allows. `Vanilla` means no mods at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModPolicy {
    /// Vanilla server: every mod is refused, cosmetic ones included.
    Vanilla,
    /// Modded server; `client_visual_allowed` says whether the player may add local visual mods.
    Modded { client_visual_allowed: bool },
    /// LG-313: a curated server entry: only packs that pass `curated_refusal`. LG-426: with `client_visual_allowed`
    /// the player's local mods load under the same rules as on a modded server (kind `client_visual` only).
    // DC-DEFAULT: LG-426 (proposed) a curated entry may allow local client-visual mods
    Curated { client_visual_allowed: bool },
}

/// Errors recorded against one mod before its hooks and listeners are switched off.
pub const DEFAULT_CIRCUIT_BREAKER_THRESHOLD: usize = 5;

#[derive(Debug, Clone)]
pub struct ContentDomainStore {
    pub records: BTreeMap<String, Rc<RefCell<LuaTable>>>,
    pub overrides: BTreeMap<String, LuaValue>,
    pub next_generated_id: i64,
    /// Record ids made by `:register` (new rows); every other record is a patch of an existing row (MOD-21).
    pub registered: BTreeSet<String>,
    /// The mod that `:register`ed each record id (the owner of a map record, MAP-5).
    pub owners: BTreeMap<String, String>,
    /// The mods that `:patch`ed each record id after its registration, in load order.
    pub patchers: BTreeMap<String, Vec<String>>,
    /// Per mod still loading: the records as they were before each of its `:patch` calls (`None` = the key
    /// did not exist), newest last. Replayed in reverse when the mod's entry fails, dropped when it loads.
    pub patch_undo: BTreeMap<String, Vec<(String, Option<LuaTable>)>>,
}

impl Default for ContentDomainStore {
    fn default() -> Self {
        Self {
            records: BTreeMap::new(),
            overrides: BTreeMap::new(),
            next_generated_id: 10_000,
            registered: BTreeSet::new(),
            owners: BTreeMap::new(),
            patchers: BTreeMap::new(),
            patch_undo: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct ContentRegistry {
    pub domains: BTreeMap<String, ContentDomainStore>,
}

impl ContentRegistry {
    pub fn new() -> Self {
        let mut domains = BTreeMap::new();
        for &d in CONTENT_DOMAINS {
            domains.insert(d.to_string(), ContentDomainStore::default());
        }
        Self { domains }
    }

    pub fn domain_mut(&mut self, name: &str) -> &mut ContentDomainStore {
        self.domains.entry(name.to_string()).or_default()
    }

    pub fn get_record(&self, domain: &str, id_key: &str) -> Option<Rc<RefCell<LuaTable>>> {
        self.domains
            .get(domain)
            .and_then(|d| d.records.get(id_key))
            .map(|t| Rc::new(RefCell::new(t.borrow().deep_clone())))
    }

    pub fn get_value(&self, domain: &str, id_key: &str) -> LuaValue {
        if let Some(d) = self.domains.get(domain) {
            if let Some(rec) = d.records.get(id_key) {
                return LuaValue::Table(Rc::new(RefCell::new(rec.borrow().deep_clone())));
            }
            if let Some(ov) = d.overrides.get(id_key) {
                return ov.deep_clone_tables();
            }
        }
        LuaValue::Nil
    }
}

#[derive(Clone)]
pub struct HookRegistration {
    pub mod_id: String,
    pub priority: i32,
    pub load_index: usize,
    pub callback: Function,
}

#[derive(Clone)]
pub struct EventRegistration {
    pub mod_id: String,
    pub priority: i32,
    pub load_index: usize,
    pub callback: Function,
}

#[derive(Clone)]
pub struct LoadedModInfo {
    pub manifest: ModManifest,
    pub root_dir: Option<PathBuf>,
    /// The mod's live `mod.exports` table inside the shared Lua state.
    pub exports: Table,
    pub logs: Rc<RefCell<Vec<String>>>,
}

struct SharedHostState {
    content: ContentRegistry,
    hooks: BTreeMap<String, Vec<HookRegistration>>,
    events: BTreeMap<String, Vec<EventRegistration>>,
    saves: BTreeMap<String, BTreeMap<String, JsonValue>>,
    options_schemas: BTreeMap<String, Rc<RefCell<LuaTable>>>,
    options_values: BTreeMap<String, BTreeMap<String, LuaValue>>,
    /// MOD-24: the player's stored settings per mod id, read when a mod loads.
    setting_values: BTreeMap<String, BTreeMap<String, String>>,
    loaded_mods: BTreeMap<String, LoadedModInfo>,
    load_order: Vec<String>,
    mod_errors: BTreeMap<String, Vec<String>>,
    validated_assets: BTreeMap<String, Vec<String>>,
    tripped: BTreeSet<String>,
    breaker_threshold: usize,
    /// Capability denials per mod; a mod with any denial fails to load even if it swallowed the error.
    violations: BTreeMap<String, Vec<String>>,
}

impl SharedHostState {
    fn new() -> Self {
        Self {
            content: ContentRegistry::new(),
            hooks: BTreeMap::new(),
            events: BTreeMap::new(),
            saves: BTreeMap::new(),
            options_schemas: BTreeMap::new(),
            options_values: BTreeMap::new(),
            setting_values: BTreeMap::new(),
            loaded_mods: BTreeMap::new(),
            load_order: Vec::new(),
            mod_errors: BTreeMap::new(),
            validated_assets: BTreeMap::new(),
            tripped: BTreeSet::new(),
            breaker_threshold: DEFAULT_CIRCUIT_BREAKER_THRESHOLD,
            violations: BTreeMap::new(),
        }
    }

    /// Records an error against `mod_id` and trips its circuit breaker at the threshold.
    fn record_error(&mut self, mod_id: &str, err: impl Into<String>) {
        let errs = self.mod_errors.entry(mod_id.to_string()).or_default();
        errs.push(err.into());
        if self.breaker_threshold > 0 && errs.len() >= self.breaker_threshold {
            self.tripped.insert(mod_id.to_string());
        }
    }

    /// Drops everything `mod_id` registered before its entry failed: its hooks and listeners, and in every
    /// content domain the records it `:register`ed (record, owner, key, patcher list), its name in the patcher
    /// lists of other mods' records and the content of its `:patch` calls (those records are restored to what
    /// they were). A failed mod then reserves no key, so a later mod may register it, and `map_records`
    /// never lists its maps (MAP-4).
    fn remove_registrations(&mut self, mod_id: &str) {
        for regs in self.hooks.values_mut() {
            regs.retain(|r| r.mod_id != mod_id);
        }
        for regs in self.events.values_mut() {
            regs.retain(|r| r.mod_id != mod_id);
        }
        for store in self.content.domains.values_mut() {
            // the patches first, newest first, so a record it patched after registering is also gone
            if let Some(undo) = store.patch_undo.remove(mod_id) {
                for (key, before) in undo.into_iter().rev() {
                    match before {
                        Some(t) => {
                            store.records.insert(key, Rc::new(RefCell::new(t)));
                        }
                        None => {
                            store.records.remove(&key);
                        }
                    }
                }
            }
            for list in store.patchers.values_mut() {
                list.retain(|m| m != mod_id);
            }
            store.patchers.retain(|_, list| !list.is_empty());
            let owned: Vec<String> = store
                .owners
                .iter()
                .filter(|(_, owner)| owner.as_str() == mod_id)
                .map(|(key, _)| key.clone())
                .collect();
            for key in owned {
                store.records.remove(&key);
                store.owners.remove(&key);
                store.registered.remove(&key);
                store.patchers.remove(&key);
            }
        }
    }

    /// The mod loaded: its patch undo log is no longer needed.
    fn commit_registrations(&mut self, mod_id: &str) {
        for store in self.content.domains.values_mut() {
            store.patch_undo.remove(mod_id);
        }
    }
}

/// Host runtime managing loaded Lua mods, content registries, middleware hooks, and events.
pub struct ModHost {
    state: Rc<RefCell<SharedHostState>>,
    envs: BTreeMap<String, ModEnv>,
    policy: ModPolicy,
    // Declared last so every Lua handle above is dropped before the state closes.
    sandbox: LuaSandbox,
}

impl Default for ModHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ModHost {
    pub fn new() -> Self {
        Self::with_limits(SandboxLimits::default())
    }

    /// Host whose per-call instruction limit is `fuel_limit` (other limits default).
    pub fn with_fuel(fuel_limit: u64) -> Self {
        Self::with_limits(SandboxLimits::with_instruction_limit(fuel_limit))
    }

    pub fn with_limits(limits: SandboxLimits) -> Self {
        Self::try_with_limits(limits).expect("failed to create the Lua sandbox state")
    }

    pub fn try_with_limits(limits: SandboxLimits) -> Result<Self, LuaError> {
        Ok(Self {
            state: Rc::new(RefCell::new(SharedHostState::new())),
            envs: BTreeMap::new(),
            policy: ModPolicy::Modded {
                client_visual_allowed: false,
            },
            sandbox: LuaSandbox::with_limits(limits)?,
        })
    }

    /// Sets the server policy; `ModPolicy::Vanilla` makes every later load fail.
    pub fn set_policy(&mut self, policy: ModPolicy) {
        self.policy = policy;
    }

    pub fn policy(&self) -> ModPolicy {
        self.policy
    }

    /// Load-time admission: policy and origin rules that need only the manifest.
    fn check_admission(&self, origin: ModOrigin, manifest: &ModManifest) -> Result<(), String> {
        let id = &manifest.id;
        let client_visual_allowed = match self.policy {
            ModPolicy::Vanilla => {
                return Err(format!(
                    "Mod '{}' refused: vanilla policy means no mods at all, cosmetic ones included",
                    id
                ))
            }
            ModPolicy::Modded {
                client_visual_allowed,
            } => client_visual_allowed,
            ModPolicy::Curated {
                client_visual_allowed,
            } => {
                if origin == ModOrigin::Local {
                    if !client_visual_allowed {
                        return Err(format!(
                            "Mod '{id}' refused: a curated server entry loads no local mods"
                        ));
                    }
                } else if let Some(why) = crate::manifest::curated_refusal(manifest) {
                    return Err(format!("Mod '{id}' refused: {why}"));
                }
                client_visual_allowed
            }
        };
        if origin == ModOrigin::Local {
            if manifest.kind != ModKind::ClientVisual {
                return Err(format!(
                    "Mod '{}' refused: a local mod must be kind 'client_visual', but its kind is '{}'; \
                     shared and server_rules mods only load from the server's mod set",
                    id, manifest.kind
                ));
            }
            if !client_visual_allowed {
                return Err(format!(
                    "Mod '{}' refused: the server's policy does not allow local client_visual mods",
                    id
                ));
            }
            if let Some(cap) = crate::manifest::CLIENT_VIEW_CAPABILITIES
                .iter()
                .find(|c| manifest.has_capability(c))
            {
                return Err(format!(
                    "Mod '{}' refused: capability '{}' requires a mod declared by the server; \
                     a local mod may not use render or HUD hooks",
                    id, cap
                ));
            }
        }
        Ok(())
    }

    /// Per-call instruction limit of this host.
    pub fn instruction_limit(&self) -> u64 {
        self.sandbox.limits().instruction_limit
    }

    /// Errors after which a mod's hooks and listeners are skipped (0 disables the breaker).
    pub fn set_circuit_breaker_threshold(&mut self, threshold: usize) {
        self.state.borrow_mut().breaker_threshold = threshold;
    }

    /// True when `mod_id` has hit the circuit breaker and is no longer called.
    pub fn is_mod_tripped(&self, mod_id: &str) -> bool {
        self.state.borrow().tripped.contains(mod_id)
    }

    /// Clears recorded errors and re-enables a tripped mod.
    pub fn reset_mod_faults(&mut self, mod_id: &str) {
        let mut st = self.state.borrow_mut();
        st.tripped.remove(mod_id);
        st.mod_errors.remove(mod_id);
    }

    /// Discovers all mod subdirectories containing `manifest.json` under `mods_dir`,
    /// resolves their topological load order, and loads them.
    pub fn discover_and_load_dir(&mut self, mods_dir: &Path) -> Result<Vec<String>, String> {
        self.discover_and_load_dir_as(ModOrigin::Server, mods_dir)
    }

    /// Like `discover_and_load_dir`, with an explicit origin for every discovered mod.
    pub fn discover_and_load_dir_as(
        &mut self,
        origin: ModOrigin,
        mods_dir: &Path,
    ) -> Result<Vec<String>, String> {
        let mut discovered = Vec::new();
        let entries = fs::read_dir(mods_dir).map_err(|e| {
            format!(
                "Failed to read mods directory '{}': {}",
                mods_dir.display(),
                e
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("manifest.json");
                if manifest_path.is_file() {
                    let manifest = ModManifest::from_file(&manifest_path)?;
                    discovered.push(DiscoveredMod {
                        manifest,
                        root_dir: path,
                    });
                }
            }
        }
        self.load_discovered_mods_as(origin, discovered)
    }

    /// Resolves and loads a set of `DiscoveredMod`s in topological order.
    pub fn load_discovered_mods(
        &mut self,
        mods: Vec<DiscoveredMod>,
    ) -> Result<Vec<String>, String> {
        self.load_discovered_mods_as(ModOrigin::Server, mods)
    }

    /// Like `load_discovered_mods`, with an explicit origin.
    pub fn load_discovered_mods_as(
        &mut self,
        origin: ModOrigin,
        mods: Vec<DiscoveredMod>,
    ) -> Result<Vec<String>, String> {
        let ordered = resolve_mods(mods).map_err(|e| e.to_string())?;
        let mut loaded_ids = Vec::with_capacity(ordered.len());
        for dm in ordered {
            let id = dm.manifest.id.clone();
            self.load_mod_from_dir_with_manifest_as(origin, &dm.root_dir, dm.manifest)?;
            loaded_ids.push(id);
        }
        Ok(loaded_ids)
    }

    /// Loads a single mod directory containing `manifest.json` and its entry script.
    pub fn load_mod_dir(&mut self, mod_dir: &Path) -> Result<ModManifest, String> {
        self.load_mod_dir_as(ModOrigin::Server, mod_dir)
    }

    /// Like `load_mod_dir`, with an explicit origin.
    pub fn load_mod_dir_as(
        &mut self,
        origin: ModOrigin,
        mod_dir: &Path,
    ) -> Result<ModManifest, String> {
        let manifest_path = mod_dir.join("manifest.json");
        let manifest = ModManifest::from_file(&manifest_path)?;
        self.load_mod_from_dir_with_manifest_as(origin, mod_dir, manifest.clone())?;
        Ok(manifest)
    }

    pub fn load_mod_from_dir_with_manifest(
        &mut self,
        mod_dir: &Path,
        manifest: ModManifest,
    ) -> Result<(), String> {
        self.load_mod_from_dir_with_manifest_as(ModOrigin::Server, mod_dir, manifest)
    }

    /// Like `load_mod_from_dir_with_manifest`, with an explicit origin.
    pub fn load_mod_from_dir_with_manifest_as(
        &mut self,
        origin: ModOrigin,
        mod_dir: &Path,
        manifest: ModManifest,
    ) -> Result<(), String> {
        self.check_admission(origin, &manifest)?;
        let entry_path = mod_dir.join(&manifest.entry);
        let bytes = fs::read(&entry_path).map_err(|e| {
            format!(
                "Failed to read entry script '{}': {}",
                entry_path.display(),
                e
            )
        })?;
        check_not_bytecode(&bytes).map_err(|e| e.to_string())?;
        let source = String::from_utf8(bytes)
            .map_err(|_| format!("Entry script '{}' is not valid UTF-8", entry_path.display()))?;
        self.load_mod_from_source_as(origin, manifest, &source, Some(mod_dir.to_path_buf()))
    }

    /// Loads a mod directly from a parsed `ModManifest` and `main.lua` source string.
    pub fn load_mod_from_source(
        &mut self,
        manifest: ModManifest,
        entry_source: &str,
        root_dir: Option<PathBuf>,
    ) -> Result<(), String> {
        self.load_mod_from_source_as(ModOrigin::Server, manifest, entry_source, root_dir)
    }

    /// Loads a mod with an explicit `origin`. The policy and origin rules run first, then the
    /// mod's entry in the sandbox; any use beyond `manifest.capabilities` fails the load.
    pub fn load_mod_from_source_as(
        &mut self,
        origin: ModOrigin,
        manifest: ModManifest,
        entry_source: &str,
        root_dir: Option<PathBuf>,
    ) -> Result<(), String> {
        self.check_admission(origin, &manifest)?;
        check_not_bytecode(entry_source.as_bytes()).map_err(|e| e.to_string())?;

        {
            let st = self.state.borrow();
            if st.loaded_mods.contains_key(&manifest.id) {
                return Err(format!("Mod '{}' is already loaded", manifest.id));
            }
            for dep in &manifest.dependencies {
                let Some(loaded) = st.loaded_mods.get(&dep.id) else {
                    return Err(format!(
                        "Mod '{}' requires missing dependency '{}'",
                        manifest.id, dep
                    ));
                };
                if !dep.matches(&loaded.manifest.version) {
                    return Err(format!(
                        "Mod '{}' requires '{}', but loaded version is '{}'",
                        manifest.id, dep, loaded.manifest.version
                    ));
                }
            }
            for conflict in &manifest.conflicts {
                if let Some(loaded) = st.loaded_mods.get(&conflict.id) {
                    if conflict.matches(&loaded.manifest.version) {
                        return Err(format!(
                            "Mod '{}' conflicts with loaded mod '{}@{}'",
                            manifest.id, loaded.manifest.id, loaded.manifest.version
                        ));
                    }
                }
            }
        }

        let id = manifest.id.clone();
        let env = self
            .sandbox
            .create_env(&id, root_dir.as_deref())
            .map_err(|e| format!("Mod '{}' sandbox setup failed: {}", id, e))?;
        let entry_fn = self
            .sandbox
            .load_text(
                &env,
                &format!("{}/{}", id, manifest.entry),
                entry_source.as_bytes(),
            )
            .map_err(|e| format!("Mod '{}' entry evaluation failed: {}", id, e))?;

        let lua = self.sandbox.lua();
        let entry_val: Value = guarded(lua, || {
            let vals: MultiValue = entry_fn.call(())?;
            Ok(vals.into_iter().next().unwrap_or(Value::Nil))
        })
        .map_err(|e| format!("Mod '{}' entry evaluation failed: {}", id, e))?;

        let Value::Function(init_fn) = entry_val else {
            return Err(format!(
                "Mod '{}' entry script '{}' must return a function(mod), got {}",
                id,
                manifest.entry,
                entry_val.type_name()
            ));
        };

        let load_index = self.state.borrow().load_order.len();
        let receiver_mt = self.sandbox.receiver_metatable();
        let built = build_mod_capability_table(
            lua,
            &receiver_mt,
            &manifest,
            load_index,
            Rc::clone(&self.state),
        )
        .map_err(|e| format!("Mod '{}' capability setup failed: {}", id, e));
        let (mod_cap, exports) = built?;

        if let Err(e) = guarded(lua, || init_fn.call::<()>(mod_cap.clone())) {
            // Hooks/listeners registered before the failure must not stay live.
            self.state.borrow_mut().remove_registrations(&id);
            return Err(format!("Mod '{}' initialization failed: {}", id, e));
        }
        // A mod that caught a capability denial with pcall still fails to load.
        let denied = self.state.borrow().violations.get(&id).cloned();
        if let Some(first) = denied.and_then(|v| v.into_iter().next()) {
            self.state.borrow_mut().remove_registrations(&id);
            return Err(format!("Mod '{}' initialization failed: {}", id, first));
        }

        // If the mod reassigned `mod.exports = { ... }`, capture the new table.
        let final_exports = match mod_cap.raw_get::<Value>("exports") {
            Ok(Value::Table(t)) => t,
            _ => exports,
        };

        {
            let mut st = self.state.borrow_mut();
            st.commit_registrations(&id);
            st.load_order.push(id.clone());
            st.loaded_mods.insert(
                id.clone(),
                LoadedModInfo {
                    manifest,
                    root_dir,
                    exports: final_exports,
                    logs: env.logs_rc(),
                },
            );
        }
        self.envs.insert(id, env);
        Ok(())
    }

    /// Returns the list of loaded mod IDs in topological load order.
    pub fn load_order(&self) -> Vec<String> {
        self.state.borrow().load_order.clone()
    }

    /// Returns ordered `<mod_dir>/overrides` paths for loaded mods where the directory exists.
    pub fn override_dirs(&self) -> Vec<PathBuf> {
        self.all_override_dirs()
            .into_iter()
            .filter(|p| p.is_dir())
            .collect()
    }

    /// Returns ordered `<mod_dir>/overrides` paths for all loaded mods with a root directory.
    pub fn all_override_dirs(&self) -> Vec<PathBuf> {
        let st = self.state.borrow();
        let mut dirs = Vec::new();
        for id in &st.load_order {
            if let Some(info) = st.loaded_mods.get(id) {
                if let Some(root) = &info.root_dir {
                    dirs.push(root.join("overrides"));
                }
            }
        }
        dirs
    }

    /// The manifest and unpacked folder of every loaded mod, in load order.
    pub fn loaded_mods(&self) -> Vec<(ModManifest, Option<PathBuf>)> {
        let st = self.state.borrow();
        st.load_order
            .iter()
            .filter_map(|id| st.loaded_mods.get(id))
            .map(|m| (m.manifest.clone(), m.root_dir.clone()))
            .collect()
    }

    /// Every `mod.content.<domain>:override(key, value)` value (the last writer of a key wins), by key.
    pub fn content_overrides(&self, domain: &str) -> BTreeMap<String, LuaValue> {
        self.state
            .borrow()
            .content
            .domains
            .get(domain)
            .map(|d| {
                d.overrides
                    .iter()
                    .map(|(k, v)| (k.clone(), v.deep_clone_tables()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// True when some mod that is not tripped registered `hook_name`.
    pub fn has_hook(&self, hook_name: &str) -> bool {
        let st = self.state.borrow();
        st.hooks
            .get(hook_name)
            .is_some_and(|v| v.iter().any(|h| !st.tripped.contains(&h.mod_id)))
    }

    /// Errors recorded against all mods together (a caller compares it before and after a call).
    pub fn error_count(&self) -> usize {
        self.state.borrow().mod_errors.values().map(Vec::len).sum()
    }

    /// Retrieves a merged content record from `mod.content.<domain>`.
    pub fn get_content_record(&self, domain: &str, id: &str) -> Option<Rc<RefCell<LuaTable>>> {
        self.state.borrow().content.get_record(domain, id)
    }

    /// MOD-21: the weapon-table content of the loaded mods as plain data for the game server and the client.
    /// Every record of `mod.content.weapons` becomes a row change: id `145` or `"Weapon145"` names `[Weapon145]`,
    /// `"Ammo3"` names `[Ammo3]`; a record made by `:register` is a new row, any other a patch of an existing one.
    /// Each field (except `id`) becomes bute value text: a number its shortest text (`41`, `0.5`), a string as
    /// given, a boolean `1` / `0`; a nested table or an id of another form is an error naming the record.
    // DC-DEFAULT: MOD-21 (proposed), see DC_DEFAULTS.md
    pub fn weapon_patches(&self) -> Result<dc_bute::modpatch::WeaponContent, String> {
        use dc_bute::modpatch::{AmmoPatch, PatchOp, WeaponContent, WeaponPatch};
        let st = self.state.borrow();
        let Some(store) = st.content.domains.get("weapons") else {
            return Ok(WeaponContent::default());
        };
        let mut weapons: BTreeMap<u16, WeaponPatch> = BTreeMap::new();
        let mut ammo: BTreeMap<u16, AmmoPatch> = BTreeMap::new();
        for (id_key, rec) in &store.records {
            let what = format!("mod.content.weapons record {:?}", id_key);
            let (is_ammo, num) = if let Some(n) = id_key.strip_prefix("Ammo") {
                (true, n)
            } else {
                (false, id_key.strip_prefix("Weapon").unwrap_or(id_key))
            };
            let id: u16 = num
                .parse()
                .ok()
                .filter(|_| !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()))
                .ok_or_else(|| {
                    format!("{what}: the id must be a row number 0..65535, \"Weapon<n>\" or \"Ammo<n>\"")
                })?;
            let mut fields = BTreeMap::new();
            for (k, v) in rec.borrow().entries() {
                let LuaKey::String(k) = k else {
                    return Err(format!("{what}: field names must be strings"));
                };
                if k == "id" {
                    continue;
                }
                let text = match v {
                    LuaValue::Number(n) if n.is_finite() => {
                        if n.fract() == 0.0 && n.abs() < 1e15 {
                            format!("{}", *n as i64)
                        } else {
                            format!("{}", n)
                        }
                    }
                    LuaValue::String(s) => s.clone(),
                    LuaValue::Bool(b) => if *b { "1" } else { "0" }.to_string(),
                    other => {
                        return Err(format!(
                            "{what}: field {k:?} is a {}, not a number, string or boolean",
                            other.type_name()
                        ))
                    }
                };
                fields.insert(k.clone(), text);
            }
            let op = if store.registered.contains(id_key) {
                PatchOp::Register
            } else {
                PatchOp::Patch
            };
            // `145` and `"Weapon145"` are the same row: later keys win, a register stays a register
            if is_ammo {
                let e = ammo.entry(id).or_insert(AmmoPatch {
                    ammo: id,
                    op,
                    fields: BTreeMap::new(),
                });
                e.op = e.op.max(op);
                e.fields.extend(fields);
            } else {
                let e = weapons.entry(id).or_insert(WeaponPatch {
                    weapon: id,
                    op,
                    fields: BTreeMap::new(),
                });
                e.op = e.op.max(op);
                e.fields.extend(fields);
            }
        }
        WeaponContent::new(
            weapons.into_values().collect(),
            ammo.into_values().collect(),
        )
        .map_err(|e| e.to_string())
    }

    /// LG-271 / MOD-21 amendment: the weapon content of only the loaded mods whose manifest passes `keep`
    /// (the client-visible set: `|m| m.requires_client`), as a host that loaded just those mods, in load order,
    /// would produce it. This is what the client computes from the mods it loads.
    // DC-DEFAULT: LG-271, MOD-21 (amendment 2026-11-01)
    pub fn weapon_patches_of<F: Fn(&ModManifest) -> bool>(
        &self,
        keep: F,
    ) -> Result<dc_bute::modpatch::WeaponContent, String> {
        let dirs: Vec<(String, PathBuf)> = {
            let st = self.state.borrow();
            st.load_order
                .iter()
                .filter_map(|id| st.loaded_mods.get(id).map(|i| (id.clone(), i)))
                .filter(|(_, i)| keep(&i.manifest))
                .filter_map(|(id, i)| i.root_dir.clone().map(|d| (id, d)))
                .collect()
        };
        let mut sub = ModHost::new();
        sub.set_policy(self.policy);
        for (id, dir) in dirs {
            sub.load_mod_dir(&dir)
                .map_err(|e| format!("mod '{id}' (client subset): {e}"))?;
        }
        sub.weapon_patches()
    }

    /// MAP-4: the community maps of the loaded mods as plain data, in the load order of the registering mod,
    /// then by map key. A record whose owner is not a loaded mod is skipped (a mod whose entry failed is
    /// rolled back, so none is expected; defensive). Every record passed `check_map_record` at `:register` /
    /// `:patch`; a record of a loaded mod that fails it here (impossible through the Lua API, defensive) is
    /// an `Err` naming the map, which a lobby treats like a malformed declared set (it does not start).
    // DC-DEFAULT: MAP-4, MAP-5 (proposed), see DC_DEFAULTS.md
    pub fn map_records(&self) -> Result<Vec<crate::maprecord::MapRecord>, String> {
        let st = self.state.borrow();
        let Some(store) = st.content.domains.get("maps") else {
            return Ok(Vec::new());
        };
        let position = |mod_id: &str| st.load_order.iter().position(|m| m == mod_id);
        let mut keyed: Vec<(usize, &String)> = store
            .records
            .keys()
            .filter_map(|id| {
                let owner = store.owners.get(id).map(String::as_str).unwrap_or("");
                let pos = position(owner).filter(|_| st.loaded_mods.contains_key(owner))?;
                Some((pos, id))
            })
            .collect();
        keyed.sort();
        let mut out = Vec::with_capacity(keyed.len());
        for (_, id) in keyed {
            let owner = store.owners.get(id).cloned().unwrap_or_default();
            let patched_by = store.patchers.get(id).cloned().unwrap_or_default();
            let rec = store.records.get(id).expect("key of records");
            let record = crate::maprecord::MapRecord::from_table(&owner, patched_by, &rec.borrow())
                .map_err(|e| format!("mod.content.maps record {id:?} of mod '{owner}': {e}"))?;
            out.push(record);
        }
        Ok(out)
    }

    /// Retrieves a content value or override from `mod.content.<domain>`.
    pub fn get_content_value(&self, domain: &str, key: &str) -> LuaValue {
        self.state.borrow().content.get_value(domain, key)
    }

    /// Runs a middleware hook chain in priority/topological order around `base_fn`.
    /// Each wrapper receives `(base, ctx)` and can call `base(ctx)` and transform the result.
    /// A wrapper that errors (or hits a sandbox limit) is recorded against its mod and skipped:
    /// the chain continues with the inner stage on a snapshot of the context it received.
    pub fn run_hook<F>(
        &mut self,
        hook_name: &str,
        ctx: LuaTable,
        base_fn: F,
    ) -> Result<LuaValue, String>
    where
        F: Fn(&LuaTable) -> Result<LuaValue, String> + 'static,
    {
        let mut hooks: Vec<HookRegistration> = {
            let st = self.state.borrow();
            st.hooks
                .get(hook_name)
                .map(|v| {
                    v.iter()
                        .filter(|h| !st.tripped.contains(&h.mod_id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };

        if hooks.is_empty() {
            return base_fn(&ctx);
        }
        hooks.sort_by_key(|h| (h.priority, h.load_index));

        let base_rc: BaseFn = Rc::new(base_fn);
        let stage = build_hook_stage(
            hooks.len(),
            Rc::new(hooks),
            base_rc,
            Rc::clone(&self.state),
            hook_name.to_string(),
        );

        let lua = self.sandbox.lua();
        let ctx_val =
            to_lua(lua, &LuaValue::Table(Rc::new(RefCell::new(ctx)))).map_err(|e| e.to_string())?;
        let Value::Table(ctx_tbl) = ctx_val else {
            return Err("hook context conversion failed".to_string());
        };
        let out = stage(lua, ctx_tbl)?;
        from_lua(&out).map_err(|e| format!("Hook '{}' returned non-data value: {}", hook_name, e))
    }

    /// Emits an event to all registered `mod.events:on(event_key, ...)` listeners in priority order.
    /// Each listener gets its own copy of `ctx` and runs in its own protected call.
    pub fn emit_event(&mut self, event_key: &str, ctx: LuaTable) {
        let mut listeners: Vec<EventRegistration> = {
            let st = self.state.borrow();
            st.events
                .get(event_key)
                .map(|v| {
                    v.iter()
                        .filter(|l| !st.tripped.contains(&l.mod_id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        listeners.sort_by_key(|l| (l.priority, l.load_index));

        let lua = self.sandbox.lua();
        let ctx_val = LuaValue::Table(Rc::new(RefCell::new(ctx)));
        for listener in listeners {
            if self.state.borrow().tripped.contains(&listener.mod_id) {
                continue;
            }
            let res = guarded(lua, || {
                let ctx_copy = to_lua(lua, &ctx_val)?;
                listener.callback.call::<()>(ctx_copy)
            });
            if let Err(err) = res {
                self.state.borrow_mut().record_error(
                    &listener.mod_id,
                    format!("Event '{}' error: {}", event_key, err),
                );
            }
        }
    }

    /// Returns any runtime/hook/event errors recorded against `mod_id`.
    pub fn mod_errors(&self, mod_id: &str) -> Vec<String> {
        self.state
            .borrow()
            .mod_errors
            .get(mod_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns all recorded mod errors across all mods.
    pub fn all_mod_errors(&self) -> BTreeMap<String, Vec<String>> {
        self.state.borrow().mod_errors.clone()
    }

    /// Returns captured `print(...)` logs for `mod_id`.
    pub fn mod_logs(&self, mod_id: &str) -> Vec<String> {
        self.state
            .borrow()
            .loaded_mods
            .get(mod_id)
            .map(|m| m.logs.borrow().clone())
            .unwrap_or_default()
    }

    /// Exports a mod's `mod.save` key-value store as a pretty-printed JSON string.
    pub fn export_mod_saves_json(&self, mod_id: &str) -> String {
        let st = self.state.borrow();
        let map = st.saves.get(mod_id).cloned().unwrap_or_default();
        JsonValue::Object(map).to_json_pretty()
    }

    /// Imports a mod's `mod.save` key-value store from a JSON string.
    pub fn import_mod_saves_json(&mut self, mod_id: &str, json: &str) -> Result<(), String> {
        let val = JsonValue::parse(json)?;
        let obj = val
            .as_object()
            .ok_or_else(|| "Save JSON root must be an object".to_string())?
            .clone();
        self.state
            .borrow_mut()
            .saves
            .insert(mod_id.to_string(), obj);
        Ok(())
    }

    /// MOD-24: the player's stored settings (mod id -> key -> text) for the mods loaded after this call. Each value is
    /// checked against the mod's declaration; a bad one reads as the default.
    pub fn set_setting_values(&mut self, values: BTreeMap<String, BTreeMap<String, String>>) {
        self.state.borrow_mut().setting_values = values;
    }

    /// Sets a user option value for `mod_id`.
    pub fn set_mod_option(&mut self, mod_id: &str, key: &str, val: LuaValue) {
        self.state
            .borrow_mut()
            .options_values
            .entry(mod_id.to_string())
            .or_default()
            .insert(key.to_string(), val);
    }

    /// Gets a user option value for `mod_id`.
    pub fn get_mod_option(&self, mod_id: &str, key: &str) -> LuaValue {
        self.state
            .borrow()
            .options_values
            .get(mod_id)
            .and_then(|m| m.get(key))
            .map(|v| v.deep_clone_tables())
            .unwrap_or(LuaValue::Nil)
    }

    /// Returns a loaded mod's sandbox environment.
    pub fn mod_env(&self, mod_id: &str) -> Option<&ModEnv> {
        self.envs.get(mod_id)
    }

    /// The shared sandbox (Lua state and limits) of this host.
    pub fn sandbox(&self) -> &LuaSandbox {
        &self.sandbox
    }
}

/// Shared pure base function of a hook chain.
type BaseFn = Rc<dyn Fn(&LuaTable) -> Result<LuaValue, String>>;

type Stage = Rc<dyn Fn(&Lua, Table) -> Result<Value, String>>;

fn build_hook_stage(
    stage_idx: usize,
    hooks: Rc<Vec<HookRegistration>>,
    base_fn: BaseFn,
    state: Rc<RefCell<SharedHostState>>,
    hook_name: String,
) -> Stage {
    if stage_idx == 0 {
        return Rc::new(move |lua: &Lua, ctx: Table| {
            let host_ctx = match from_lua(&Value::Table(ctx)) {
                Ok(LuaValue::Table(t)) => t,
                Ok(_) => LuaTable::new_rc(),
                Err(e) => return Err(format!("Hook '{}' context: {}", hook_name, e)),
            };
            let out = base_fn(&host_ctx.borrow())?;
            to_lua(lua, &out).map_err(|e| e.to_string())
        });
    }

    let prev = build_hook_stage(
        stage_idx - 1,
        Rc::clone(&hooks),
        base_fn,
        Rc::clone(&state),
        hook_name.clone(),
    );
    let hook = hooks[stage_idx - 1].clone();

    Rc::new(move |lua: &Lua, ctx: Table| {
        if state.borrow().tripped.contains(&hook.mod_id) {
            return prev(lua, ctx);
        }
        let snapshot = from_lua(&Value::Table(ctx.clone())).ok();

        // Errors from inner stages are not this mod's fault; remember them so they pass through.
        let inner_failure: Rc<Cell<Option<String>>> = Rc::new(Cell::new(None));
        let prev_for_lua = Rc::clone(&prev);
        let default_ctx = ctx.clone();
        let inner_flag = Rc::clone(&inner_failure);
        let base = lua
            .create_function(move |lua, args: MultiValue| {
                let passed = match args.into_iter().next() {
                    Some(Value::Table(t)) => t,
                    _ => default_ctx.clone(),
                };
                match prev_for_lua(lua, passed) {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        inner_flag.set(Some(e.clone()));
                        Err(host_err(LuaError::Runtime(e)))
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        match guarded(lua, || hook.callback.call::<Value>((base, ctx.clone()))) {
            Ok(v) => Ok(v),
            Err(err) => {
                if let Some(inner) = inner_failure.take() {
                    return Err(inner);
                }
                state.borrow_mut().record_error(
                    &hook.mod_id,
                    format!("Hook error in '{}' ({}): {}", hook.mod_id, hook_name, err),
                );
                let fallback = match snapshot {
                    Some(v) => match to_lua(lua, &v) {
                        Ok(Value::Table(t)) => t,
                        _ => ctx,
                    },
                    None => ctx,
                };
                prev(lua, fallback)
            }
        }
    })
}

fn normalize_record_id(val: &LuaValue) -> Result<String, LuaError> {
    match val {
        LuaValue::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                Ok(format!("{}", *n as i64))
            } else {
                Ok(format!("{}", n))
            }
        }
        LuaValue::String(s) if !s.is_empty() => Ok(s.clone()),
        other => Err(LuaError::Runtime(format!(
            "Record id must be a non-empty string or number, got {}",
            other.type_name()
        ))),
    }
}

pub fn validate_asset_rel_path(rel_path: &str) -> Result<String, String> {
    let trimmed = rel_path.trim();
    if trimmed.is_empty() {
        return Err("Asset path cannot be empty".to_string());
    }
    if trimmed.contains("..")
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.contains(':')
        || trimmed.contains('\0')
    {
        return Err(format!(
            "Invalid asset path '{}': path traversal or absolute path is forbidden",
            rel_path
        ));
    }
    let normalized = trimmed.replace('\\', "/");
    let p = Path::new(&normalized);
    let mut clean_parts = Vec::new();
    for comp in p.components() {
        match comp {
            Component::Normal(os) => {
                let s = os.to_string_lossy();
                if s == ".." || s.is_empty() {
                    return Err(format!(
                        "Invalid asset path '{}': path traversal is forbidden",
                        rel_path
                    ));
                }
                clean_parts.push(s.into_owned());
            }
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "Invalid asset path '{}': path traversal or root component is forbidden",
                    rel_path
                ))
            }
        }
    }
    if clean_parts.is_empty() {
        return Err(format!("Invalid asset path '{}'", rel_path));
    }
    Ok(clean_parts.join("/"))
}

// ---------------------------------------------------------------------------------------------
// `mod` capability table
// ---------------------------------------------------------------------------------------------

fn receiver_table(lua: &Lua, receiver_mt: &Table) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set_metatable(Some(receiver_mt.clone()));
    Ok(t)
}

/// Installs `name` on `tbl` as a method callable as `tbl:name(...)` or `tbl.name(...)`.
fn set_method<F>(lua: &Lua, receiver_mt: &Table, tbl: &Table, name: &str, f: F) -> mlua::Result<()>
where
    F: Fn(&Lua, Vec<Value>) -> mlua::Result<Value> + 'static,
{
    let mt = receiver_mt.clone();
    let func =
        lua.create_function(move |lua, args: MultiValue| f(lua, strip_receiver(&mt, args)))?;
    tbl.raw_set(name, func)
}

fn arg_string(args: &[Value], idx: usize, what: &str) -> mlua::Result<String> {
    match args.get(idx) {
        Some(Value::String(s)) => Ok(s.to_string_lossy()),
        _ => Err(runtime_err(format!(
            "{} expected string argument #{}",
            what,
            idx + 1
        ))),
    }
}

/// Records a capability denial against `mod_id` and builds the Lua error.
fn deny(
    st: &Rc<RefCell<SharedHostState>>,
    mod_id: &str,
    kind: ModKind,
    detail: String,
) -> mlua::Error {
    let msg = format!(
        "capability denied: mod '{}' (kind {}): {}",
        mod_id, kind, detail
    );
    let mut s = st.borrow_mut();
    s.violations
        .entry(mod_id.to_string())
        .or_default()
        .push(msg.clone());
    s.record_error(mod_id, &msg);
    host_err(LuaError::Forbidden(msg))
}

/// A load refusal that is not a capability question (a closed key vocabulary): recorded as a violation so the
/// load fails even when the mod catches the error.
fn refuse(st: &Rc<RefCell<SharedHostState>>, mod_id: &str, detail: String) -> mlua::Error {
    let msg = format!("Mod '{}': {}", mod_id, detail);
    let mut s = st.borrow_mut();
    s.violations
        .entry(mod_id.to_string())
        .or_default()
        .push(msg.clone());
    s.record_error(mod_id, &msg);
    host_err(LuaError::Runtime(msg))
}

fn deny_cap(
    st: &Rc<RefCell<SharedHostState>>,
    mod_id: &str,
    kind: ModKind,
    what: &str,
    need: &str,
) -> mlua::Error {
    deny(
        st,
        mod_id,
        kind,
        format!(
            "{} needs capability '{}', which the manifest does not declare",
            what, need
        ),
    )
}

fn arg_function(args: &[Value], idx: usize, what: &str) -> mlua::Result<Function> {
    match args.get(idx) {
        Some(Value::Function(f)) => Ok(f.clone()),
        _ => Err(runtime_err(format!("{} expected callback function", what))),
    }
}

fn arg_data(args: &[Value], idx: usize) -> mlua::Result<LuaValue> {
    from_lua(args.get(idx).unwrap_or(&Value::Nil)).map_err(host_err)
}

fn arg_table_data(args: &[Value], idx: usize, what: &str) -> mlua::Result<Rc<RefCell<LuaTable>>> {
    match args.get(idx) {
        Some(v @ Value::Table(_)) => match from_lua(v).map_err(host_err)? {
            LuaValue::Table(t) => Ok(t),
            _ => Err(runtime_err(format!("{} expected table", what))),
        },
        _ => Err(runtime_err(format!("{} expected table", what))),
    }
}

fn build_mod_capability_table(
    lua: &Lua,
    receiver_mt: &Table,
    manifest: &ModManifest,
    load_index: usize,
    state: Rc<RefCell<SharedHostState>>,
) -> mlua::Result<(Table, Table)> {
    let mod_tbl = receiver_table(lua, receiver_mt)?;
    let mod_id = manifest.id.clone();
    let priority = manifest.priority;
    let exports = lua.create_table()?;
    let caps: Rc<BTreeSet<String>> = Rc::new(manifest.capabilities.iter().cloned().collect());
    let kind = manifest.kind;

    // 1. Metadata fields
    mod_tbl.raw_set("id", manifest.id.as_str())?;
    mod_tbl.raw_set("name", manifest.name.as_str())?;
    mod_tbl.raw_set("version", manifest.version.to_string())?;
    mod_tbl.raw_set("api", manifest.api as i64)?;
    mod_tbl.raw_set("priority", manifest.priority as i64)?;
    mod_tbl.raw_set("kind", manifest.kind.as_str())?;
    mod_tbl.raw_set("exports", exports.clone())?;

    // 2. `mod.content.<domain>`
    let content_tbl = receiver_table(lua, receiver_mt)?;
    for &domain_name in CONTENT_DOMAINS {
        let dom = build_content_domain_table(
            lua,
            &mod_id,
            receiver_mt,
            domain_name,
            Rc::clone(&state),
            Rc::clone(&caps),
            kind,
        )?;
        content_tbl.raw_set(domain_name, dom)?;
    }
    mod_tbl.raw_set("content", content_tbl)?;

    // 3. `mod.hooks` (`:wrap` and `:on`)
    let hooks_tbl = receiver_table(lua, receiver_mt)?;
    for verb in ["wrap", "on"] {
        let st = Rc::clone(&state);
        let mid = mod_id.clone();
        let what = format!("mod.hooks:{}", verb);
        let caps = Rc::clone(&caps);
        set_method(lua, receiver_mt, &hooks_tbl, verb, move |_, args| {
            let hook_name = arg_string(&args, 0, &what)?;
            let callback = arg_function(&args, 1, &what)?;
            let Some(need) = hook_capability(&hook_name) else {
                return Err(deny(
                    &st,
                    &mid,
                    kind,
                    format!(
                        "{} registered hook '{}', which is not '<namespace>.<name>' with a known \
                         namespace (lobby, combat, render, hud)",
                        what, hook_name
                    ),
                ));
            };
            if !caps.contains(&need) {
                return Err(deny_cap(&st, &mid, kind, &what, &need));
            }
            st.borrow_mut()
                .hooks
                .entry(hook_name)
                .or_default()
                .push(HookRegistration {
                    mod_id: mid.clone(),
                    priority,
                    load_index,
                    callback,
                });
            Ok(Value::Nil)
        })?;
    }
    mod_tbl.raw_set("hooks", hooks_tbl)?;

    // 4. `mod.events` (`:on`)
    let events_tbl = receiver_table(lua, receiver_mt)?;
    {
        let st = Rc::clone(&state);
        let mid = mod_id.clone();
        let caps = Rc::clone(&caps);
        set_method(lua, receiver_mt, &events_tbl, "on", move |_, args| {
            let event_key = arg_string(&args, 0, "mod.events:on")?;
            let callback = arg_function(&args, 1, "mod.events:on")?;
            if !caps.contains("events") {
                return Err(deny_cap(&st, &mid, kind, "mod.events:on", "events"));
            }
            st.borrow_mut()
                .events
                .entry(event_key)
                .or_default()
                .push(EventRegistration {
                    mod_id: mid.clone(),
                    priority,
                    load_index,
                    callback,
                });
            Ok(Value::Nil)
        })?;
    }
    mod_tbl.raw_set("events", events_tbl)?;

    // 5. `mod.find(other_mod_id)`
    {
        let st = Rc::clone(&state);
        set_method(lua, receiver_mt, &mod_tbl, "find", move |lua, args| {
            let target = arg_string(&args, 0, "mod.find")?;
            // Copy out before touching Lua so no host borrow is held during allocation.
            let found = st.borrow().loaded_mods.get(&target).map(|info| {
                (
                    info.manifest.id.clone(),
                    info.manifest.name.clone(),
                    info.manifest.version.to_string(),
                    info.exports.clone(),
                )
            });
            let Some((id, name, version, exports)) = found else {
                return Ok(Value::Nil);
            };
            let res = lua.create_table()?;
            res.raw_set("id", id)?;
            res.raw_set("name", name)?;
            res.raw_set("version", version)?;
            res.raw_set("exports", exports)?;
            Ok(Value::Table(res))
        })?;
    }

    // 6. `mod.save` (`:get`, `:set`)
    let save_tbl = receiver_table(lua, receiver_mt)?;
    {
        let st = Rc::clone(&state);
        let mid = mod_id.clone();
        let caps_g = Rc::clone(&caps);
        set_method(lua, receiver_mt, &save_tbl, "get", move |lua, args| {
            let key = arg_string(&args, 0, "mod.save:get")?;
            if !caps_g.contains("save") {
                return Err(deny_cap(&st, &mid, kind, "mod.save:get", "save"));
            }
            let val = st
                .borrow()
                .saves
                .get(&mid)
                .and_then(|m| m.get(&key))
                .map(LuaValue::from_json_value)
                .unwrap_or(LuaValue::Nil);
            to_lua(lua, &val)
        })?;

        let st = Rc::clone(&state);
        let mid = mod_id.clone();
        let caps_s = Rc::clone(&caps);
        set_method(lua, receiver_mt, &save_tbl, "set", move |_, args| {
            let key = arg_string(&args, 0, "mod.save:set")?;
            if !caps_s.contains("save") {
                return Err(deny_cap(&st, &mid, kind, "mod.save:set", "save"));
            }
            let val = arg_data(&args, 1)?;
            let json = if matches!(val, LuaValue::Nil) {
                None
            } else {
                Some(val.to_json_value().map_err(runtime_err)?)
            };
            let mut s = st.borrow_mut();
            let mod_saves = s.saves.entry(mid.clone()).or_default();
            match json {
                None => {
                    mod_saves.remove(&key);
                }
                Some(j) => {
                    mod_saves.insert(key, j);
                }
            }
            Ok(Value::Nil)
        })?;
    }
    mod_tbl.raw_set("save", save_tbl)?;

    // 7. `mod.options` (`:define`, `:get`, `:set`)
    let options_tbl = receiver_table(lua, receiver_mt)?;
    {
        let st = Rc::clone(&state);
        let mid = mod_id.clone();
        set_method(lua, receiver_mt, &options_tbl, "define", move |_, args| {
            let schema = arg_table_data(&args, 0, "mod.options:define")?;
            let mut s = st.borrow_mut();
            s.options_schemas.insert(
                mid.clone(),
                Rc::new(RefCell::new(schema.borrow().deep_clone())),
            );
            let vals = s.options_values.entry(mid.clone()).or_default();
            for (k, v) in schema.borrow().entries() {
                if let LuaKey::String(opt_key) = k {
                    if !vals.contains_key(opt_key) {
                        let default_val = if let LuaValue::Table(spec) = v {
                            let d = spec.borrow().get_str("default");
                            if !matches!(d, LuaValue::Nil) {
                                d.deep_clone_tables()
                            } else {
                                v.deep_clone_tables()
                            }
                        } else {
                            v.deep_clone_tables()
                        };
                        vals.insert(opt_key.clone(), default_val);
                    }
                }
            }
            Ok(Value::Nil)
        })?;

        let st = Rc::clone(&state);
        let mid = mod_id.clone();
        set_method(lua, receiver_mt, &options_tbl, "get", move |lua, args| {
            let key = arg_string(&args, 0, "mod.options:get")?;
            let val = st
                .borrow()
                .options_values
                .get(&mid)
                .and_then(|m| m.get(&key))
                .map(|v| v.deep_clone_tables())
                .unwrap_or(LuaValue::Nil);
            to_lua(lua, &val)
        })?;

        let st = Rc::clone(&state);
        let mid = mod_id.clone();
        set_method(lua, receiver_mt, &options_tbl, "set", move |_, args| {
            let key = arg_string(&args, 0, "mod.options:set")?;
            let val = arg_data(&args, 1)?;
            st.borrow_mut()
                .options_values
                .entry(mid.clone())
                .or_default()
                .insert(key, val);
            Ok(Value::Nil)
        })?;
    }
    mod_tbl.raw_set("options", options_tbl)?;

    // 8. `mod.assets` (`:path`)
    let assets_tbl = receiver_table(lua, receiver_mt)?;
    {
        let st = Rc::clone(&state);
        let mid = mod_id.clone();
        let caps = Rc::clone(&caps);
        set_method(lua, receiver_mt, &assets_tbl, "path", move |lua, args| {
            let rel = arg_string(&args, 0, "mod.assets:path")?;
            if !caps.contains("assets") {
                return Err(deny_cap(&st, &mid, kind, "mod.assets:path", "assets"));
            }
            match validate_asset_rel_path(&rel) {
                Ok(clean) => {
                    let uri = format!("@mod:{}/{}", mid, clean);
                    st.borrow_mut()
                        .validated_assets
                        .entry(mid.clone())
                        .or_default()
                        .push(clean);
                    Ok(Value::String(lua.create_string(uri)?))
                }
                Err(err) => {
                    st.borrow_mut().record_error(&mid, &err);
                    Err(host_err(LuaError::Forbidden(err)))
                }
            }
        })?;
    }
    mod_tbl.raw_set("assets", assets_tbl)?;

    // 9. `mod.settings` (MOD-24): the player's values, read-only
    let resolved = crate::settings::resolve_settings(
        &manifest.settings,
        state.borrow().setting_values.get(&mod_id),
    );
    let values = lua.create_table()?;
    for d in &manifest.settings {
        let shown = d.lua_value(&resolved[&d.key]);
        values.raw_set(d.key.as_str(), crate::sandbox::to_lua(lua, &shown)?)?;
    }
    let view = lua.create_table()?;
    let locked = lua.create_table()?;
    locked.raw_set("__index", values.clone())?;
    locked.raw_set(
        "__newindex",
        lua.create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(runtime_err("mod.settings is read-only"))
        })?,
    )?;
    let next: Function = lua.globals().raw_get("next")?;
    locked.raw_set(
        "__pairs",
        lua.create_function(move |_, _: Value| Ok((next.clone(), values.clone(), Value::Nil)))?,
    )?;
    locked.raw_set("__metatable", "locked")?;
    view.set_metatable(Some(locked));
    mod_tbl.raw_set("settings", view)?;

    Ok((mod_tbl, exports))
}

fn build_content_domain_table(
    lua: &Lua,
    mod_id: &str,
    receiver_mt: &Table,
    domain_name: &str,
    state: Rc<RefCell<SharedHostState>>,
    caps: Rc<BTreeSet<String>>,
    kind: ModKind,
) -> mlua::Result<Table> {
    let dom_tbl = receiver_table(lua, receiver_mt)?;
    let need = content_capability(domain_name);
    let mid = mod_id.to_string();

    // :patch(id, partial_table)
    {
        let st = Rc::clone(&state);
        let dom = domain_name.to_string();
        let (caps, need, mid) = (Rc::clone(&caps), need.clone(), mid.clone());
        set_method(lua, receiver_mt, &dom_tbl, "patch", move |lua, args| {
            let what = format!("mod.content.{}:patch", dom);
            if !caps.contains(&need) {
                return Err(deny_cap(&st, &mid, kind, &what, &need));
            }
            let id_val = arg_data(&args, 0)?;
            let id_key = normalize_record_id(&id_val).map_err(host_err)?;
            let partial = arg_table_data(&args, 1, &what)?;
            // MAP-5: a map patch needs a registered map, keeps its key, and the merged record must still
            // pass the record check against the OWNER's assets (the world stays the registering mod's file)
            if dom == "maps" {
                let checked = {
                    let s = st.borrow();
                    let store = s.content.domains.get("maps");
                    let owner = store.and_then(|d| d.owners.get(&id_key).cloned());
                    let existing = store.and_then(|d| d.records.get(&id_key).cloned());
                    match (owner, existing) {
                        (Some(owner), Some(rec)) => {
                            let mut merged = rec.borrow().deep_clone();
                            merged.merge_deep(&partial.borrow());
                            // lists are values: a patch that carries one of these replaces the owner's
                            // whole list (or table), so `modes = { "FFA" }` leaves exactly FFA
                            for &key in crate::mapkeys::MAP_REPLACED_KEYS {
                                let v = partial.borrow().get_str(key);
                                if !matches!(v, LuaValue::Nil) {
                                    merged.set_str(key, v.deep_clone_tables());
                                }
                            }
                            if merged.get_str("id").as_str() != Some(id_key.as_str()) {
                                Err(format!("a patch may not change 'id' (map {id_key:?})"))
                            } else {
                                crate::mapkeys::check_map_record(&owner, &merged).map(|()| merged)
                            }
                        }
                        _ => Err(format!(
                            "patches map {id_key:?}, which no loaded mod registered; register it first"
                        )),
                    }
                };
                let merged = match checked {
                    Ok(m) => m,
                    Err(e) => return Err(refuse(&st, &mid, format!("{what}: {e}"))),
                };
                let mut s = st.borrow_mut();
                let store = s.content.domain_mut(&dom);
                let before = store
                    .records
                    .insert(id_key.clone(), Rc::new(RefCell::new(merged.deep_clone())))
                    .map(|t| t.borrow().deep_clone());
                store
                    .patch_undo
                    .entry(mid.clone())
                    .or_default()
                    .push((id_key.clone(), before));
                store.patchers.entry(id_key).or_default().push(mid.clone());
                drop(s);
                return to_lua(lua, &LuaValue::Table(Rc::new(RefCell::new(merged))));
            }
            let merged = {
                let mut s = st.borrow_mut();
                let store = s.content.domain_mut(&dom);
                let before = store.records.get(&id_key).map(|t| t.borrow().deep_clone());
                store
                    .patch_undo
                    .entry(mid.clone())
                    .or_default()
                    .push((id_key.clone(), before));
                let entry = store.records.entry(id_key).or_insert_with(|| {
                    let mut t = LuaTable::new();
                    t.set_str("id", id_val.clone());
                    Rc::new(RefCell::new(t))
                });
                entry.borrow_mut().merge_deep(&partial.borrow());
                let cloned = entry.borrow().deep_clone();
                cloned
            };
            to_lua(lua, &LuaValue::Table(Rc::new(RefCell::new(merged))))
        })?;
    }

    // :register(def_table)
    {
        let st = Rc::clone(&state);
        let dom = domain_name.to_string();
        let (caps, need, mid) = (Rc::clone(&caps), need.clone(), mid.clone());
        set_method(lua, receiver_mt, &dom_tbl, "register", move |lua, args| {
            let what = format!("mod.content.{}:register", dom);
            if !caps.contains(&need) {
                return Err(deny_cap(&st, &mid, kind, &what, &need));
            }
            let def = arg_table_data(&args, 0, &what)?;
            // MAP-4 / MAP-5: a map record is checked whole and its key is unique across the set
            if dom == "maps" {
                let cloned = def.borrow().deep_clone();
                if let Err(e) = crate::mapkeys::check_map_record(&mid, &cloned) {
                    return Err(refuse(&st, &mid, format!("{what}: {e}")));
                }
                let id_key = cloned
                    .get_str("id")
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let taken = st
                    .borrow()
                    .content
                    .domains
                    .get("maps")
                    .and_then(|d| d.owners.get(&id_key).cloned());
                if let Some(owner) = taken {
                    return Err(refuse(
                        &st,
                        &mid,
                        format!("{what}: map {id_key:?} is already registered by mod '{owner}'"),
                    ));
                }
                let mut s = st.borrow_mut();
                let store = s.content.domain_mut(&dom);
                store.registered.insert(id_key.clone());
                store.owners.insert(id_key.clone(), mid.clone());
                store
                    .records
                    .insert(id_key.clone(), Rc::new(RefCell::new(cloned)));
                drop(s);
                return to_lua(lua, &LuaValue::String(id_key));
            }
            let id_val = {
                let mut s = st.borrow_mut();
                let store = s.content.domain_mut(&dom);
                let mut cloned = def.borrow().deep_clone();
                let existing = cloned.get_str("id");
                let (id_key, id_val) = if !matches!(existing, LuaValue::Nil) {
                    (normalize_record_id(&existing).map_err(host_err)?, existing)
                } else {
                    let gen_id = store.next_generated_id;
                    store.next_generated_id += 1;
                    let lv = LuaValue::Number(gen_id as f64);
                    cloned.set_str("id", lv.clone());
                    (gen_id.to_string(), lv)
                };
                store.registered.insert(id_key.clone());
                store.owners.insert(id_key.clone(), mid.clone());
                store.records.insert(id_key, Rc::new(RefCell::new(cloned)));
                id_val
            };
            to_lua(lua, &id_val)
        })?;
    }

    // :override(key, value)
    {
        let st = Rc::clone(&state);
        set_method(lua, receiver_mt, &dom_tbl, "override", {
            let dom = domain_name.to_string();
            let (caps, need, mid) = (Rc::clone(&caps), need.clone(), mid.clone());
            move |_, args| {
                if !caps.contains(&need) {
                    let what = format!("mod.content.{}:override", dom);
                    return Err(deny_cap(&st, &mid, kind, &what, &need));
                }
                let key_val = arg_data(&args, 0)?;
                let key = normalize_record_id(&key_val).map_err(host_err)?;
                let val = arg_data(&args, 1)?;
                // MAP-4: a map record is only ever made by :register and changed by :patch (both checked)
                if dom == "maps" {
                    return Err(refuse(
                        &st,
                        &mid,
                        "mod.content.maps:override is not supported; register a map record or patch one"
                            .to_string(),
                    ));
                }
                // CLI-1613: the HUD keys are a closed vocabulary; a wrong key or value refuses the mod at load
                if dom == "hud" {
                    if let Err(e) = crate::hudkeys::check_hud_override(&key, &val) {
                        return Err(refuse(&st, &mid, format!("mod.content.hud:override: {e}")));
                    }
                }
                let mut s = st.borrow_mut();
                let store = s.content.domain_mut(&dom);
                if let LuaValue::Table(t) = &val {
                    store
                        .records
                        .insert(key.clone(), Rc::new(RefCell::new(t.borrow().deep_clone())));
                }
                store.overrides.insert(key, val.deep_clone_tables());
                Ok(Value::Nil)
            }
        })?;
    }

    // :get(id)
    {
        let st = Rc::clone(&state);
        let dom = domain_name.to_string();
        let (caps, need, mid) = (Rc::clone(&caps), need.clone(), mid.clone());
        set_method(lua, receiver_mt, &dom_tbl, "get", move |lua, args| {
            if !caps.contains(&need) {
                let what = format!("mod.content.{}:get", dom);
                return Err(deny_cap(&st, &mid, kind, &what, &need));
            }
            let id_val = arg_data(&args, 0)?;
            let id_key = normalize_record_id(&id_val).map_err(host_err)?;
            let val = st.borrow().content.get_value(&dom, &id_key);
            to_lua(lua, &val)
        })?;
    }

    Ok(dom_tbl)
}

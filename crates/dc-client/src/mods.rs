//! The client side of mod delivery: a server's mod set is fetched, cached by hash, verified, unpacked and loaded
//! into the client's sandbox before the login, and its mod-set hash is kept for the login.
//!
//! SPEC: HOSTING_AND_MODS.md §4 (joining a modded server) and §8 items 1-2 (decided 2026-10-24); docs/MODDING_GUIDE.md
//! §7 (delivery) and §10 (vanilla means no mods).
//!
//! The steps of [`ClientMods::join`]:
//! 1. `GET /server_list.json` on the lobby's web port (the auth port + 8080, [`crate::menu::clanpage::web_addr`]),
//!    at most [`SERVER_LIST_CAP`] bytes; its `mod_set` fragment is the set and its `mod_set_hash` must agree with it.
//! 2. Each mod the client needs (`requires_client`; server_rules mods run on the server only) comes from the cache
//!    `<data>/mods/by-hash/<sha256>.zip` or is downloaded from `/mods/<sha256>.zip` (at most
//!    [`DEFAULT_MAX_MOD_BYTES`]); the bytes are hashed before they are written under their name, and a mismatch
//!    refuses the join.
//! 3. A verified zip is unpacked into `<data>/mods/unpacked/<sha256>/` (stored entries only, as `dc-modkit pack`
//!    writes them; no absolute path, no `..`, no symlink).
//! 4. The mods load into a fresh `ModHost` in the server's order as `ModOrigin::Server` under the server's policy;
//!    local client-visual mods (`<data>/mods/local/*`) follow only when the policy is modded and allows them (a
//!    curated entry's fragment says so too, LG-426).
//! 5. The cache is trimmed (least recently used first) to [`DEFAULT_CACHE_BYTES`].
//!
//! Vanilla: no host at all (every mod unloaded, local ones included) and the empty-set hash.
// DC-DEFAULT: CLI-1608 (cache and download rules), CLI-1609 (vanilla unloads everything; the set hash), see
// DC_DEFAULTS.md

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use dc_mod_lua::{ModHost, ModOrigin, ModPolicy};

pub use dc_mod_lua::EMPTY_SET_HASH;

/// Largest `server_list.json` read.
pub const SERVER_LIST_CAP: u64 = 64 * 1024;
/// Largest mod package downloaded (and largest total unpacked size of one package).
use std::collections::BTreeMap;

pub const DEFAULT_MAX_MOD_BYTES: u64 = 32 * 1024 * 1024;
/// Total size of the by-hash cache after a trim.
pub const DEFAULT_CACHE_BYTES: u64 = 256 * 1024 * 1024;
/// Largest reply head read.
const MAX_HEAD: usize = 16 * 1024;
/// Most entries a package may hold.
const MAX_ZIP_ENTRIES: usize = 4096;
/// Total time one `server_list.json` request may take (the per-read timeout alone lets a dribbling server hold on).
pub const LIST_DEADLINE: Duration = Duration::from_secs(10);
/// Total time, and byte ceiling, of one package download.
pub const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(120);
pub const DOWNLOAD_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// The server's policy as its mod set states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Vanilla,
    Modded { client_visual_allowed: bool },
}

/// One mod of a server's set (the fields of the `mod_set` fragment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerMod {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub kind: Option<String>,
    pub requires_client: bool,
    pub capabilities: Vec<String>,
    pub source_repo: Option<String>,
    /// CLI-1617 / CLI-1618: what the mod-set entry says of the mod for the player (`dc-modkit modset` writes
    /// `description`, `author` and the screenshot file names; `name` and `homepage` when present).
    pub name: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub screenshots: Vec<String>,
    /// MOD-27: the manifest's command help as the set entry carries it.
    pub commands: Vec<dc_mod_lua::ModCommand>,
    /// MOD-27: SHA-256 of the lobby operator's picture (`/mods/<it>.png`).
    pub preview: Option<String>,
}

impl ServerMod {
    /// The key a player's consent is remembered under (CLI-1618): `id@version:sha256`.
    pub fn key(&self) -> String {
        mod_key(&self.id, &self.version, &self.sha256)
    }
}

/// `id@version:sha256`, the key of one exact mod package (consent memory, the trusted list).
pub fn mod_key(id: &str, version: &str, sha256: &str) -> String {
    format!("{id}@{version}:{sha256}")
}

/// MOD-27: the `commands` of a set or catalogue entry; a malformed line is dropped (it is help text).
pub fn commands_of(m: &serde_json::Value) -> Vec<dc_mod_lua::ModCommand> {
    let Some(list) = m.get("commands").and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            let line = dc_mod_lua::ModCommand {
                command: c.get("command")?.as_str()?.to_string(),
                description: c.get("description")?.as_str()?.to_string(),
            };
            line.is_valid().then_some(line)
        })
        .take(dc_mod_lua::MAX_COMMANDS)
        .collect()
}

/// MOD-27: the `preview` of a set or catalogue entry: a picture hash, else none.
pub fn preview_of(m: &serde_json::Value) -> Option<String> {
    m.get("preview")
        .and_then(|p| p.as_str())
        .filter(|h| is_sha256_hex(h))
        .map(str::to_string)
}

/// A server's mod set, in load order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerModSet {
    pub policy: Policy,
    pub mods: Vec<ServerMod>,
    /// MAP-32: the lobby's first community map index (`community_map_base` of its `server_list.json`, MAP-35);
    /// `None` from a lobby that does not publish it (an older one, or a pinned directory entry), when the menu
    /// falls back to its own ROM's map row count. Not part of the set's hash.
    pub community_map_base: Option<u32>,
    /// LG-429: the field-of-view range `[min, max]` the server lets its players choose (the 4:3-horizontal angle, 2009
    /// = 90); `None` = none published (a vanilla server, an older lobby). Not part of the set's hash.
    // DC-DEFAULT: LG-429 (proposed), see DC_DEFAULTS.md
    pub client_fov: Option<(u32, u32)>,
}

/// `<64 lower-case hex digits>`: the only form a hash may take before it names a file.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl ServerModSet {
    pub fn vanilla() -> ServerModSet {
        ServerModSet {
            policy: Policy::Vanilla,
            mods: Vec::new(),
            community_map_base: None,
            client_fov: None,
        }
    }

    /// LG-429: the field-of-view range this server publishes, for a modded or curated set only (a vanilla server keeps
    /// the 2009 field of view whatever its document says).
    // DC-DEFAULT: LG-429 (proposed)
    pub fn fov_range(&self) -> Option<(u32, u32)> {
        match self.policy {
            Policy::Vanilla => None,
            Policy::Modded { .. } => self.client_fov,
        }
    }

    /// Vanilla with no mods: what a client may assume of a server it cannot ask.
    pub fn is_plain_vanilla(&self) -> bool {
        self.policy == Policy::Vanilla && self.mods.is_empty()
    }

    /// The mod-set hash sent at login (lower-case hex SHA-256; [`EMPTY_SET_HASH`] for vanilla).
    pub fn hash(&self) -> String {
        if self.policy == Policy::Vanilla {
            return EMPTY_SET_HASH.to_string();
        }
        let text = dc_mod_lua::mod_set_hash_text(self.mods.iter().map(|m| dc_mod_lua::HashEntry {
            id: &m.id,
            version: &m.version,
            sha256: &m.sha256,
            kind: m.kind.as_deref(),
        }));
        dc_rom::sha256_hex(text.as_bytes())
    }

    /// Reads the `mod_set` fragment (`{"policy", "client_visual_allowed", "mods": [...]}`).
    pub fn from_fragment(v: &serde_json::Value) -> Result<ServerModSet, String> {
        let o = v.as_object().ok_or("the mod set is not an object")?;
        let policy = match o.get("policy").and_then(|p| p.as_str()) {
            Some("vanilla") | None => Policy::Vanilla,
            Some("modded") => Policy::Modded {
                client_visual_allowed: o
                    .get("client_visual_allowed")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false),
            },
            Some(p) => return Err(format!("unknown mod policy '{p}'")),
        };
        let mut mods = Vec::new();
        if let Some(list) = o.get("mods") {
            let list = list.as_array().ok_or("the mod list is not an array")?;
            for m in list {
                let text = |k: &str| m.get(k).and_then(|x| x.as_str()).map(str::to_string);
                let id = text("id").ok_or("a mod without an id")?;
                if !dc_mod_lua::is_valid_mod_id(&id) {
                    return Err(format!("mod id '{id}' is not valid"));
                }
                let version =
                    text("version").ok_or_else(|| format!("mod '{id}' has no version"))?;
                let sha256 = text("sha256").ok_or_else(|| format!("mod '{id}' has no sha256"))?;
                if !is_sha256_hex(&sha256) {
                    return Err(format!(
                        "mod '{id}': sha256 is not 64 lower-case hex digits"
                    ));
                }
                mods.push(ServerMod {
                    version,
                    sha256,
                    kind: text("kind"),
                    requires_client: m
                        .get("requires_client")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false),
                    capabilities: m
                        .get("capabilities")
                        .and_then(|c| c.as_array())
                        .map(|c| {
                            c.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                    source_repo: text("source_repo"),
                    name: text("name"),
                    author: text("author"),
                    description: text("description"),
                    homepage: text("homepage"),
                    screenshots: m
                        .get("screenshots")
                        .and_then(|c| c.as_array())
                        .map(|c| {
                            c.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                    commands: commands_of(m),
                    preview: preview_of(m),
                    id,
                });
            }
        }
        if policy == Policy::Vanilla && !mods.is_empty() {
            return Err("a vanilla server declares mods".into());
        }
        // LG-429: `[min, max]` of whole degrees with min <= max; anything else counts as none published
        let client_fov = o
            .get("client_fov")
            .and_then(|r| r.as_array())
            .and_then(|r| match r.as_slice() {
                [lo, hi] => Some((lo.as_u64()?, hi.as_u64()?)),
                _ => None,
            })
            .filter(|&(lo, hi)| lo >= 1 && lo <= hi && hi <= 180)
            .map(|(lo, hi)| (lo as u32, hi as u32));
        Ok(ServerModSet {
            policy,
            mods,
            community_map_base: None,
            client_fov,
        })
    }

    /// The `mod_set` fragment of this set ([`ServerModSet::from_fragment`] reads it back).
    pub fn to_fragment(&self) -> serde_json::Value {
        let mods: Vec<serde_json::Value> = self
            .mods
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id, "version": m.version, "sha256": m.sha256, "kind": m.kind,
                    "requires_client": m.requires_client, "capabilities": m.capabilities,
                    "source_repo": m.source_repo, "name": m.name, "author": m.author,
                    "description": m.description, "homepage": m.homepage, "screenshots": m.screenshots,
                    "commands": m.commands.iter()
                        .map(|c| serde_json::json!({"command": c.command, "description": c.description}))
                        .collect::<Vec<_>>(),
                    "preview": m.preview,
                })
            })
            .collect();
        match self.policy {
            Policy::Vanilla => serde_json::json!({"policy": "vanilla", "mods": mods}),
            Policy::Modded {
                client_visual_allowed,
            } => {
                let mut o = serde_json::json!({
                    "policy": "modded", "client_visual_allowed": client_visual_allowed, "mods": mods,
                });
                if let Some((lo, hi)) = self.client_fov {
                    o["client_fov"] = serde_json::json!([lo, hi]);
                }
                o
            }
        }
    }

    /// Reads the lobby's `server_list.json`: no `mod_set` = vanilla; a `mod_set_hash` must equal the set's hash;
    /// `community_map_base` (MAP-32), when the lobby publishes it, is the first community map index.
    pub fn from_server_list(body: &[u8]) -> Result<ServerModSet, String> {
        let v: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| format!("the server list is not JSON: {e}"))?;
        let mut set = match v.get("mod_set") {
            Some(f) => ServerModSet::from_fragment(f)?,
            None => ServerModSet::vanilla(),
        };
        if let Some(h) = v.get("mod_set_hash").and_then(|h| h.as_str()) {
            if h != set.hash() {
                return Err("the server's mod-set hash does not match its mod list".into());
            }
        }
        set.community_map_base = v
            .get("community_map_base")
            .and_then(|b| b.as_u64())
            .and_then(|b| u32::try_from(b).ok());
        Ok(set)
    }
}

/// Where the join stands (the later browser screen shows it).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ModState {
    #[default]
    Idle,
    FetchingSet,
    /// Mod `index` (from 1) of `count`; `done` of `total` bytes of that package (`total` 0 = not known yet).
    Downloading {
        index: usize,
        count: usize,
        done: u64,
        total: u64,
    },
    Verifying,
    Loading,
    Ready,
    Failed(String),
    /// CLI-1618: the set is read and names a mod the player has not trusted (not verified by the community list,
    /// not accepted before): nothing is downloaded until the UI answers with `LinkCmd::ModConsent`.
    NeedsConsent(ServerModSet),
}

/// What the link keeps of the last join (state, set, hash, refused local mods).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoinReport {
    pub state: ModState,
    pub set: Option<ServerModSet>,
    /// The mod-set hash of the last successful join, for the login.
    pub hash: Option<String>,
    /// Local mods that were refused (the host's messages).
    pub local_refused: Vec<String>,
    /// CLI-1611: the weapon content the loaded mods produced (empty on vanilla); every match's weapon table gets it.
    pub content: dc_bute::modpatch::WeaponContent,
    /// CLI-1619: how to rebuild the loaded sandbox on another thread (`None`: vanilla, or no successful join).
    pub recipe: Option<HostRecipe>,
    /// MAP-32: the community maps the loaded set registers (`ModHost::map_records`: mod-set load order, then map
    /// key order), the rows the menu appends after the ROM's map rows. Empty on vanilla.
    pub maps: Vec<dc_mod_lua::MapRecord>,
    /// LG-313: the set is a featured server entry's curated set (the lobby's reviewed list: Verified, no consent).
    pub curated: bool,
    /// MAP-32: the map index of `maps[0]` as the lobby published it (`community_map_base` of its server list,
    /// MAP-35); `None` when the lobby did not say, and the menu falls back to its own ROM's map row count.
    pub community_map_base: Option<u32>,
}

/// One mod of a [`HostRecipe`]: its unpacked folder, who put it there, and what it was when the link thread loaded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecipeMod {
    pub dir: PathBuf,
    pub origin: ModOrigin,
    pub id: String,
    pub version: String,
    /// The manifest kind (`shared`, `client_visual`, ...), for logs.
    pub kind: String,
}

/// CLI-1619: plain data that rebuilds the sandbox of a join on any thread (`ModHost` is not `Send`): the server's
/// policy and the mods the link thread loaded, in load order, with their origins (a local mod stays
/// [`ModOrigin::Local`]). Loading the same folders in the same order under the same policy runs the same Lua, so the
/// rebuilt host holds the same content, overrides and hooks.
// DC-DEFAULT: CLI-1619 (proposed) the match's own host from the join's recipe, see DC_DEFAULTS.md
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRecipe {
    pub policy: ModPolicy,
    pub mods: Vec<RecipeMod>,
    /// MOD-24: the player's stored mod settings (mod id -> key -> value), read when the match loads.
    pub settings: BTreeMap<String, BTreeMap<String, String>>,
}

impl HostRecipe {
    /// A new host with the recipe's mods. A server mod that no longer loads (or holds another id / version) fails the
    /// whole recipe; a local mod that no longer loads is skipped and logged, as at the join.
    pub fn load(&self) -> Result<ModHost, String> {
        let mut host = ModHost::new();
        host.set_policy(self.policy);
        host.set_setting_values(self.settings.clone());
        for m in &self.mods {
            match host.load_mod_dir_as(m.origin, &m.dir) {
                Ok(man) if man.id == m.id && man.version.to_string() == m.version => {}
                Ok(man) => {
                    return Err(format!(
                        "mod '{}': the folder now holds '{}@{}', not '{}@{}'",
                        m.id, man.id, man.version, m.id, m.version
                    ))
                }
                Err(e) if m.origin == ModOrigin::Local => eprintln!("mods: local mod skipped: {e}"),
                Err(e) => return Err(e),
            }
        }
        Ok(host)
    }

    /// The recipe of the mods `host` loaded from folders (`origin_of` names each id's origin).
    fn of(host: &ModHost, origin_of: impl Fn(&str) -> ModOrigin) -> HostRecipe {
        HostRecipe {
            policy: host.policy(),
            mods: host
                .loaded_mods()
                .into_iter()
                .filter_map(|(m, root)| {
                    root.map(|dir| RecipeMod {
                        dir,
                        origin: origin_of(&m.id),
                        version: m.version.to_string(),
                        kind: m.kind.to_string(),
                        id: m.id,
                    })
                })
                .collect(),
            settings: BTreeMap::new(),
        }
    }
}

/// CLI-1611: the weapon content of a loaded sandbox (`ModHost::weapon_patches`); no host (vanilla) = none.
pub fn set_content(host: Option<&ModHost>) -> Result<dc_bute::modpatch::WeaponContent, String> {
    match host {
        Some(h) => h.weapon_patches().map_err(|e| format!("mod content: {e}")),
        None => Ok(Default::default()),
    }
}

/// LG-271: the content hash a login carries: [`dc_bute::modpatch::WeaponContent::hash`], or `0` for no content.
pub fn content_hash(c: &dc_bute::modpatch::WeaponContent) -> String {
    if c.is_empty() {
        dc_wire::loginmods::NO_CONTENT.to_string()
    } else {
        c.hash()
    }
}

/// Caps and the data folder.
#[derive(Clone, Debug)]
pub struct ModConfig {
    /// The client's data folder (`settings::own_profile_dir`); `None`: a modded server cannot be joined.
    pub data_dir: Option<PathBuf>,
    pub max_mod_bytes: u64,
    pub cache_bytes: u64,
    /// Connect / read wait of each HTTP request.
    pub timeout: Duration,
    /// CLI-1617: the local mods (manifest ids) the player enabled on the Mods screen; `None` loads every local
    /// mod (scripted runs without a profile, the CLI-1608 behaviour).
    pub local_enabled: Option<Vec<String>>,
}

impl Default for ModConfig {
    fn default() -> ModConfig {
        ModConfig {
            data_dir: None,
            max_mod_bytes: DEFAULT_MAX_MOD_BYTES,
            cache_bytes: DEFAULT_CACHE_BYTES,
            timeout: Duration::from_secs(3),
            local_enabled: None,
        }
    }
}

/// The folders under `<data>/mods`.
#[derive(Clone, Debug)]
pub struct ModPaths {
    pub root: PathBuf,
}

impl ModPaths {
    pub fn new(data_dir: &Path) -> ModPaths {
        ModPaths {
            root: data_dir.join("mods"),
        }
    }
    pub fn by_hash(&self) -> PathBuf {
        self.root.join("by-hash")
    }
    pub fn zip(&self, sha: &str) -> PathBuf {
        self.by_hash().join(format!("{sha}.zip"))
    }
    pub fn unpacked(&self, sha: &str) -> PathBuf {
        self.root.join("unpacked").join(sha)
    }
    pub fn local(&self) -> PathBuf {
        self.root.join("local")
    }
    /// CLI-1634: a catalogue entry's picture.
    pub fn catalog_shot(&self, sha: &str) -> PathBuf {
        self.root.join("catalog").join(format!("{sha}.png"))
    }
}

/// MOD-27: fetches an operator picture into the cache unless it is there. `get` asks the lobby's web port. The file
/// is kept only when it hashes to its name and is a PNG; anything else, a dead port included, leaves no file and no
/// error: the panel then shows the package's own screenshot.
// DC-DEFAULT: MOD-27 (proposed, USER REQUEST 2026-11-01), see DC_DEFAULTS.md
pub fn store_preview(
    paths: &ModPaths,
    sha: &str,
    get: &mut dyn FnMut(&str) -> Result<HttpReply, String>,
) {
    let file = paths.catalog_shot(sha);
    if !is_sha256_hex(sha) || file.is_file() {
        return;
    }
    let Ok(r) = get(&format!("/mods/{sha}.png")) else {
        return;
    };
    if r.status != 200
        || !r.body.starts_with(b"\x89PNG\r\n\x1a\n")
        || dc_rom::sha256_hex(&r.body) != sha
    {
        return;
    }
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let part = file.with_extension("part");
    if std::fs::write(&part, &r.body).is_ok() {
        let _ = std::fs::rename(&part, &file);
    }
}

/// An HTTP reply: status and body.
#[derive(Debug)]
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// One HTTP/1.1 GET on the lobby's web port over the client's TLS connector (plain only on loopback, as for the
/// clan pages); a body longer than `cap` is refused. `progress(done, total)` follows the body bytes.
pub fn web_get(
    addr: SocketAddr,
    target: &str,
    cap: u64,
    timeout: Duration,
    total_deadline: Duration,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<HttpReply, String> {
    let trust = dc_botclient::tls::current();
    web_get_with(addr, target, cap, timeout, total_deadline, &trust, progress)
}

/// [`web_get`] with an explicit trust (a read that must not depend on the process-wide pin of the current lobby).
fn web_get_with(
    addr: SocketAddr,
    target: &str,
    cap: u64,
    timeout: Duration,
    total_deadline: Duration,
    trust: &dc_botclient::tls::Trust,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<HttpReply, String> {
    let deadline = Instant::now() + total_deadline;
    let mut c = dc_botclient::tls::connect_with(addr, timeout, trust)?;
    let _ = c.tcp().set_read_timeout(Some(timeout));
    let _ = c.tcp().set_write_timeout(Some(timeout));
    let req = format!("GET {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    c.write_all(req.as_bytes())
        .map_err(|e| format!("send to {addr}: {e}"))?;
    let r = read_reply(&mut c, cap, deadline, progress);
    c.close();
    r
}

/// Reads one reply (head, then a body of `Content-Length` bytes or up to the close) with the body capped.
pub fn read_reply(
    r: &mut dyn Read,
    cap: u64,
    deadline: Instant,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<HttpReply, String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err("the reply head is too long".into());
        }
        if Instant::now() >= deadline {
            return Err("the request took too long (deadline passed)".into());
        }
        let n = r.read(&mut chunk).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Err("the connection closed before the reply".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("not an HTTP reply")?;
    let mut length: Option<u64> = None;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let k = k.trim();
        if k.eq_ignore_ascii_case("content-length") {
            length = Some(v.trim().parse().map_err(|_| "bad Content-Length")?);
        } else if k.eq_ignore_ascii_case("transfer-encoding")
            && !v.trim().eq_ignore_ascii_case("identity")
        {
            return Err("chunked replies are not taken".into());
        }
    }
    if let Some(n) = length {
        if n > cap {
            return Err(format!(
                "the reply ({n} bytes) exceeds the cap of {cap} bytes"
            ));
        }
    }
    let mut body = buf[head_end + 4..].to_vec();
    let total = length.unwrap_or(0);
    progress(body.len() as u64, total);
    loop {
        if body.len() as u64 > cap {
            return Err(format!("the reply exceeds the cap of {cap} bytes"));
        }
        if length.is_some_and(|n| body.len() as u64 >= n) {
            break;
        }
        if Instant::now() >= deadline {
            return Err("the request took too long (deadline passed)".into());
        }
        let n = r.read(&mut chunk).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
        progress(body.len() as u64, total);
    }
    if let Some(n) = length {
        if (body.len() as u64) < n {
            return Err(format!("the reply ended after {} of {n} bytes", body.len()));
        }
        body.truncate(n as usize);
    }
    Ok(HttpReply { status, body })
}

/// One file of a package.
#[derive(Debug, PartialEq, Eq)]
pub struct ZipFile {
    /// Relative path with `/` separators; a name ending in `/` is a folder.
    pub name: String,
    pub data: Vec<u8>,
}

/// A safe relative entry name: no absolute path, drive, backslash, `..`, `.`, empty part or control character.
pub fn safe_entry_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.contains(':') {
        return false;
    }
    if name.chars().any(|c| c.is_control()) {
        return false;
    }
    let body = name.strip_suffix('/').unwrap_or(name);
    !body.is_empty()
        && body.split('/').all(|p| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && !p.ends_with('.')
                && !p.ends_with(' ')
                && !is_reserved_device(p)
        })
}

/// A Windows reserved device name (CON, PRN, AUX, NUL, COM1-9, LPT1-9), with or without an extension.
fn is_reserved_device(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).trim_end();
    let u = stem.to_ascii_uppercase();
    matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((u.starts_with("COM") || u.starts_with("LPT"))
            && u.len() == 4
            && matches!(u.as_bytes()[3], b'1'..=b'9'))
}

fn u16_le(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2)
        .map(|x| u16::from_le_bytes([x[0], x[1]]))
        .ok_or_else(|| "the package is truncated".to_string())
}

fn u32_le(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4)
        .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
        .ok_or_else(|| "the package is truncated".to_string())
}

/// IEEE CRC-32 (the zip checksum).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c: u32 = !0;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xedb8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Reads a package: stored (uncompressed) entries only, as `dc-modkit pack` writes them; every name checked by
/// [`safe_entry_name`], no symlink, no encryption, no duplicate, CRC checked, at most `cap` bytes in all.
// Public format: PKWARE APPNOTE.TXT (end of central directory 0x06054b50, central header 0x02014b50, local header
// 0x04034b50)
pub fn read_package(b: &[u8], cap: u64) -> Result<Vec<ZipFile>, String> {
    read_zip(b, cap, false)
}

/// [`read_package`], with `deflate` also deflated entries (method 8; the game's own release zips, CLI-1865).
// Public format: PKWARE APPNOTE.TXT section 4.4.5 (method 8 = deflate, RFC 1951)
pub fn read_zip(b: &[u8], cap: u64, deflate: bool) -> Result<Vec<ZipFile>, String> {
    let min = b.len().saturating_sub(22 + 0xffff);
    let eocd = (min..=b.len().saturating_sub(22))
        .rev()
        .find(|&i| b[i..].starts_with(&0x0605_4b50u32.to_le_bytes()))
        .ok_or("not a zip package")?;
    let count = u16_le(b, eocd + 10)? as usize;
    let mut at = u32_le(b, eocd + 16)? as usize;
    if count > MAX_ZIP_ENTRIES {
        return Err(format!(
            "the package holds {count} entries (at most {MAX_ZIP_ENTRIES})"
        ));
    }
    let mut out: Vec<ZipFile> = Vec::with_capacity(count);
    let mut total = 0u64;
    for _ in 0..count {
        if u32_le(b, at)? != 0x0201_4b50 {
            return Err("the package's central directory is damaged".into());
        }
        let made_by_host = u16_le(b, at + 4)? >> 8;
        let flags = u16_le(b, at + 8)?;
        let method = u16_le(b, at + 10)?;
        let crc = u32_le(b, at + 16)?;
        let csize = u32_le(b, at + 20)? as usize;
        let usize_ = u32_le(b, at + 24)? as usize;
        let nlen = u16_le(b, at + 28)? as usize;
        let elen = u16_le(b, at + 30)? as usize;
        let clen = u16_le(b, at + 32)? as usize;
        let attrs = u32_le(b, at + 38)?;
        let local = u32_le(b, at + 42)? as usize;
        let name_end = (at + 46)
            .checked_add(nlen)
            .ok_or("the package is damaged")?;
        let name_bytes = b.get(at + 46..name_end).ok_or("the package is truncated")?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| "an entry name is not UTF-8")?
            .to_string();
        at = name_end
            .checked_add(elen)
            .and_then(|x| x.checked_add(clen))
            .ok_or("the package is damaged")?;
        if !safe_entry_name(&name) {
            return Err(format!("unsafe entry name '{name}' in the package"));
        }
        // unix host (3): the file type sits in the high half of the external attributes
        if made_by_host == 3 && (attrs >> 16) & 0o170000 == 0o120000 {
            return Err(format!("the package holds a symbolic link '{name}'"));
        }
        if flags & 1 != 0 {
            return Err(format!("entry '{name}' is encrypted"));
        }
        if csize == u32::MAX as usize || usize_ == u32::MAX as usize {
            return Err(format!("entry '{name}' is too large (zip64 is not taken)"));
        }
        let stored = method == 0 && csize == usize_;
        if !stored && !(deflate && method == 8) {
            return Err(format!(
                "entry '{name}' is compressed; only dc-modkit packs (stored entries) are taken"
            ));
        }
        if out.iter().any(|f| f.name.eq_ignore_ascii_case(&name)) {
            return Err(format!("entry '{name}' appears twice in the package"));
        }
        total += usize_ as u64;
        if total > cap {
            return Err(format!("the package unpacks to more than {cap} bytes"));
        }
        if u32_le(b, local)? != 0x0403_4b50 {
            return Err(format!("entry '{name}' has no local header"));
        }
        let lnlen = u16_le(b, local + 26)? as usize;
        let lelen = u16_le(b, local + 28)? as usize;
        let start = local
            .checked_add(30 + lnlen + lelen)
            .ok_or("the package is damaged")?;
        let end = start.checked_add(csize).ok_or("the package is damaged")?;
        let raw = b.get(start..end).ok_or("the package is truncated")?;
        let data = if stored {
            raw.to_vec()
        } else {
            let mut out = Vec::with_capacity(usize_);
            flate2::read::DeflateDecoder::new(raw)
                .take(usize_ as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|e| format!("entry '{name}' does not unpack: {e}"))?;
            if out.len() != usize_ {
                return Err(format!("entry '{name}' unpacks to the wrong size"));
            }
            out
        };
        if crc32(&data) != crc {
            return Err(format!("entry '{name}' fails its checksum"));
        }
        out.push(ZipFile { name, data });
    }
    Ok(out)
}

/// Unpacks a verified package into `dest` (replaced as a whole: written next to it, then renamed).
pub fn unpack_package(bytes: &[u8], dest: &Path, cap: u64) -> Result<(), String> {
    let files = read_package(bytes, cap)?;
    let parent = dest.parent().ok_or("no folder for the unpacked mod")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let part = dest.with_extension("part");
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let write = || -> Result<(), String> {
        for f in &files {
            let path = f
                .name
                .trim_end_matches('/')
                .split('/')
                .fold(part.clone(), |p, c| p.join(c));
            if f.name.ends_with('/') {
                std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                continue;
            }
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::write(&path, &f.data).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_dir_all(&part);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&part, dest).map_err(|e| {
        let _ = std::fs::remove_dir_all(&part);
        format!("{}: {e}", dest.display())
    })
}

/// Marks a cached package as just used (its modification time is the LRU clock).
fn touch(path: &Path) {
    if let Ok(f) = std::fs::OpenOptions::new().append(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Trims the by-hash cache to `cap` bytes, oldest use first, never removing a package of `keep`; the unpacked copy
/// of a removed package goes with it. Returns the hashes removed.
pub fn trim_cache(paths: &ModPaths, cap: u64, keep: &[&str]) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(paths.by_hash()) else {
        return Vec::new();
    };
    let mut items: Vec<(SystemTime, u64, String)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let sha = name
                .strip_suffix(".zip")
                .filter(|h| is_sha256_hex(h))?
                .to_string();
            let m = e.metadata().ok()?;
            Some((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), sha))
        })
        .collect();
    let mut total: u64 = items.iter().map(|i| i.1).sum();
    items.sort();
    let mut removed = Vec::new();
    for (_, len, sha) in items {
        if total <= cap {
            break;
        }
        if keep.contains(&sha.as_str()) {
            continue;
        }
        if std::fs::remove_file(paths.zip(&sha)).is_ok() {
            let _ = std::fs::remove_dir_all(paths.unpacked(&sha));
            total -= len;
            removed.push(sha);
        }
    }
    removed
}

/// CLI-1632: why a cached package was pruned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PruneReason {
    /// The joined server declares the same mod id with this other sha256 (an update of the mod).
    Superseded(String),
    /// No known server declares the package (Clean up), or its id is declared by none and it went unused for
    /// [`UNUSED_MAX_AGE`].
    Unused,
}

/// CLI-1632: one package removed from the cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pruned {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub reason: PruneReason,
}

impl Pruned {
    /// The log line: `mods: cache: removed <id> <version> <sha8> (superseded by <sha8> | unused)`.
    pub fn log_line(&self) -> String {
        let id = if self.id.is_empty() { "?" } else { &self.id };
        let version = if self.version.is_empty() {
            "?"
        } else {
            &self.version
        };
        let why = match &self.reason {
            PruneReason::Superseded(new) => format!("superseded by {}", &new[..new.len().min(8)]),
            PruneReason::Unused => "unused".into(),
        };
        format!(
            "mods: cache: removed {id} {version} {} ({why})",
            &self.sha256[..self.sha256.len().min(8)]
        )
    }
}

/// CLI-1632: how far a prune goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PruneMode {
    /// After a join: superseded packages of the joined set's ids, and packages of retired ids (no known server
    /// declares the id) unused for [`UNUSED_MAX_AGE`].
    Auto,
    /// The Mods screen's Clean up: every package no known server declares (by sha256), whatever its age.
    CleanUp,
}

/// CLI-1632: a package of a retired id goes after this long without use (its zip's modification time, the LRU
/// clock refreshed on each cache hit).
pub const UNUSED_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// CLI-1632: prunes the by-hash cache (zip and unpacked folder). `joined` is the set of the server just joined (its
/// packages downloaded, verified and unpacked), `others` the sets of every other server the client knows (the
/// joined server's own stale entries left out); `keep` lists hashes never removed (the loaded sandbox's). A package
/// declared by `joined` or by any of `others` is never superseded; the player's local mods (`mods/local`) are not
/// in this cache and are never touched. `now` is the clock of the age rule. Returns the removed packages; the
/// caller logs them ([`Pruned::log_line`]) and drops the consent keys of their hashes.
// DC-DEFAULT: CLI-1632 (proposed, project decision) superseded and unused packages leave the cache
pub fn prune_cache(
    paths: &ModPaths,
    joined: Option<&ServerModSet>,
    others: &[&ServerModSet],
    keep: &[&str],
    mode: PruneMode,
    now: SystemTime,
) -> Vec<Pruned> {
    let Ok(rd) = std::fs::read_dir(paths.by_hash()) else {
        return Vec::new();
    };
    let known: Vec<&ServerMod> = joined
        .into_iter()
        .chain(others.iter().copied())
        .flat_map(|s| s.mods.iter())
        .collect();
    let mut items: Vec<(String, SystemTime)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let sha = name
                .strip_suffix(".zip")
                .filter(|h| is_sha256_hex(h))?
                .to_string();
            let m = e.metadata().ok().filter(|m| m.is_file())?;
            Some((sha, m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    items.sort();
    let mut removed = Vec::new();
    for (sha, modified) in items {
        if keep.contains(&sha.as_str()) || known.iter().any(|m| m.sha256 == sha) {
            continue;
        }
        let (id, version) =
            dc_mod_lua::ModManifest::from_file(&paths.unpacked(&sha).join("manifest.json"))
                .map(|m| (m.id.clone(), m.version.to_string()))
                .unwrap_or_default();
        let newer = joined.and_then(|s| {
            s.mods
                .iter()
                .find(|m| !id.is_empty() && m.id == id && m.requires_client)
        });
        let reason = if let Some(new) = newer {
            PruneReason::Superseded(new.sha256.clone())
        } else {
            let id_declared = !id.is_empty() && known.iter().any(|m| m.id == id);
            let old = now
                .duration_since(modified)
                .is_ok_and(|age| age > UNUSED_MAX_AGE);
            match mode {
                PruneMode::CleanUp => PruneReason::Unused,
                PruneMode::Auto if !id_declared && old => PruneReason::Unused,
                PruneMode::Auto => continue,
            }
        };
        if std::fs::remove_file(paths.zip(&sha)).is_ok() {
            let _ = std::fs::remove_dir_all(paths.unpacked(&sha));
            removed.push(Pruned {
                id,
                version,
                sha256: sha,
                reason,
            });
        }
    }
    removed
}

/// The package of `m`, from the cache when its bytes still hash to the name, else from the server; the bytes are
/// hashed before they are written under their name.
fn fetch_package(
    web: Option<SocketAddr>,
    paths: &ModPaths,
    m: &ServerMod,
    cfg: &ModConfig,
    index: usize,
    count: usize,
    progress: &mut dyn FnMut(ModState),
) -> Result<Vec<u8>, String> {
    let file = paths.zip(&m.sha256);
    if let Ok(b) = std::fs::read(&file) {
        if b.len() as u64 <= cfg.max_mod_bytes && dc_rom::sha256_hex(&b) == m.sha256 {
            touch(&file);
            return Ok(b);
        }
        // damaged or replaced on disk: fetched again
        let _ = std::fs::remove_file(&file);
    }
    let web = web.ok_or("the server's web port is not known")?;
    progress(ModState::Downloading {
        index,
        count,
        done: 0,
        total: 0,
    });
    let target = format!("/mods/{}.zip", m.sha256);
    let r = web_get(
        web,
        &target,
        cfg.max_mod_bytes.min(DOWNLOAD_MAX_BYTES),
        cfg.timeout,
        DOWNLOAD_DEADLINE,
        &mut |done, total| {
            progress(ModState::Downloading {
                index,
                count,
                done,
                total,
            })
        },
    )
    .map_err(|e| format!("mod '{}': download failed: {e}", m.id))?;
    if r.status != 200 {
        return Err(format!(
            "mod '{}': the server answered {} for its package",
            m.id, r.status
        ));
    }
    progress(ModState::Verifying);
    let got = dc_rom::sha256_hex(&r.body);
    if got != m.sha256 {
        return Err(format!(
            "mod '{}': the package hashes to {got}, not to the {} the server list names; join refused",
            m.id, m.sha256
        ));
    }
    std::fs::create_dir_all(paths.by_hash())
        .map_err(|e| format!("{}: {e}", paths.by_hash().display()))?;
    let part = file.with_extension("part");
    std::fs::write(&part, &r.body).map_err(|e| format!("{}: {e}", part.display()))?;
    std::fs::rename(&part, &file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(r.body)
}

/// The mod set the client asks the lobby for, and what it falls back to.
///
/// - a `server_list.json` answer: its set (a declared hash that disagrees with the list fails);
/// - `404` (a lobby that publishes no server list, e.g. one without `server.toml`): vanilla;
/// - no answer (web port closed or unreachable): vanilla only when the entry the player picked says plain vanilla,
///   or when there is no entry at all (a direct address); a modded entry fails.
///
/// With an `expected` entry (the server browser's), the lobby's set must have the same policy and hash: the
/// client pins to the entry.
pub fn fetch_set(
    web: Option<SocketAddr>,
    expected: Option<&ServerModSet>,
    timeout: Duration,
) -> Result<ServerModSet, String> {
    let fallback = |why: String| match expected {
        Some(e) if !e.is_plain_vanilla() => {
            Err(format!("cannot read the server's mod set ({why})"))
        }
        _ => Ok(ServerModSet::vanilla()),
    };
    let got = match web {
        None => return fallback("no web port".into()),
        Some(a) => match web_get(
            a,
            "/server_list.json",
            SERVER_LIST_CAP,
            timeout,
            LIST_DEADLINE,
            &mut |_, _| {},
        ) {
            Ok(r) if r.status == 200 => ServerModSet::from_server_list(&r.body)?,
            Ok(r) if r.status == 404 => ServerModSet::vanilla(),
            Ok(r) => return fallback(format!("status {}", r.status)),
            Err(e) => return fallback(e),
        },
    };
    if let Some(e) = expected {
        if e.policy != got.policy || e.hash() != got.hash() {
            return Err(
                "the server's mod set differs from its server-list entry; join refused".into(),
            );
        }
    }
    Ok(got)
}

/// MAP-86: the document of a featured entry (`/entry_mods/<entry>.json`): its set (with the lobby's
/// `community_map_base`, read by [`ServerModSet::from_server_list`]) and, when the lobby publishes them, the map
/// records of the entry's packs as the lobby computed them (`"maps"`; `None`: an older lobby, the loaded host's own
/// records stand in).
#[derive(Clone, Debug, PartialEq)]
pub struct EntryDoc {
    pub set: ServerModSet,
    pub maps: Option<Vec<dc_mod_lua::MapRecord>>,
}

impl EntryDoc {
    /// Reads the entry document. A `maps` row the client cannot turn into a record fails the document, as a
    /// malformed set does.
    // DC-DEFAULT: MAP-86 (proposed) the entry document's map records
    pub fn parse(body: &[u8]) -> Result<EntryDoc, String> {
        let set = ServerModSet::from_server_list(body)?;
        let v: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| format!("the server's set is not JSON: {e}"))?;
        let maps = match v.get("maps") {
            None | Some(serde_json::Value::Null) => None,
            Some(list) => {
                let list = list.as_array().ok_or("the map list is not an array")?;
                let mut out = Vec::with_capacity(list.len());
                for (i, m) in list.iter().enumerate() {
                    out.push(map_record_from_json(m).map_err(|e| format!("map record {i}: {e}"))?);
                }
                Some(out)
            }
        };
        Ok(EntryDoc { set, maps })
    }
}

/// MAP-86: one map record of the entry document (the lobby's JSON of `dc_mod_lua::MapRecord`: `id`, `name`,
/// `description`, `world`, `modes`, `goals` as `[[mode, goal], ...]` (an object `{mode: goal}` is taken too),
/// `round_seconds`, `picture`, `loading`, `radar` as `{picture, min: [x, z], max: [x, z]}`, `players` as
/// `{min, max}`, `mod_id`, `patched_by`). The texts are required where the record requires them; the rest is optional.
pub fn map_record_from_json(m: &serde_json::Value) -> Result<dc_mod_lua::MapRecord, String> {
    let o = m.as_object().ok_or("not an object")?;
    let text = |k: &str| o.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let opt_text = |k: &str| match o.get(k) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("'{k}' is not a string")),
    };
    let texts = |k: &str| -> Result<Vec<String>, String> {
        match o.get(k) {
            None | Some(serde_json::Value::Null) => Ok(Vec::new()),
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("'{k}' holds a value that is not a string"))
                })
                .collect(),
            Some(_) => Err(format!("'{k}' is not an array")),
        }
    };
    let u32_of = |v: &serde_json::Value, what: &str| -> Result<u32, String> {
        v.as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("'{what}' is not a whole number"))
    };
    let pair = |v: Option<&serde_json::Value>, what: &str| -> Result<[f64; 2], String> {
        let a = v
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == 2)
            .ok_or_else(|| format!("'{what}' is not a pair of numbers"))?;
        let n = |i: usize| {
            a[i].as_f64()
                .filter(|f| f.is_finite())
                .ok_or_else(|| format!("'{what}' is not a pair of numbers"))
        };
        Ok([n(0)?, n(1)?])
    };
    let id = text("id").ok_or("no id")?;
    if !dc_mod_lua::is_valid_map_id(&id) {
        return Err(format!("map key '{id}' is not valid"));
    }
    let mod_id = text("mod_id").ok_or_else(|| format!("map '{id}': no mod_id"))?;
    if !dc_mod_lua::is_valid_mod_id(&mod_id) {
        return Err(format!("map '{id}': mod id '{mod_id}' is not valid"));
    }
    let name = text("name").ok_or_else(|| format!("map '{id}': no name"))?;
    let world = text("world").ok_or_else(|| format!("map '{id}': no world"))?;
    if !world.starts_with(crate::modassets::MOD_SCHEME) {
        return Err(format!(
            "map '{id}': world '{world}' is not a {}<mod>/assets/... path",
            crate::modassets::MOD_SCHEME
        ));
    }
    let goals = match o.get("goals") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(a)) => {
            let mut g = Vec::with_capacity(a.len());
            for p in a {
                let p = p
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .ok_or_else(|| format!("map '{id}': a goal is not a [mode, goal] pair"))?;
                let mode = p[0]
                    .as_str()
                    .ok_or_else(|| format!("map '{id}': a goal's mode is not a string"))?;
                g.push((mode.to_string(), u32_of(&p[1], "goal")?));
            }
            g
        }
        Some(serde_json::Value::Object(obj)) => {
            let mut g = Vec::with_capacity(obj.len());
            for (mode, v) in obj {
                g.push((mode.clone(), u32_of(v, "goal")?));
            }
            g.sort();
            g
        }
        Some(_) => return Err(format!("map '{id}': 'goals' is not a list")),
    };
    let round_seconds = match o.get("round_seconds") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(u32_of(v, "round_seconds")?),
    };
    let radar = match o.get("radar") {
        None | Some(serde_json::Value::Null) => None,
        Some(r) => {
            let ro = r
                .as_object()
                .ok_or_else(|| format!("map '{id}': 'radar' is not an object"))?;
            Some(dc_mod_lua::RadarInfo {
                picture: ro
                    .get("picture")
                    .and_then(|p| p.as_str())
                    .ok_or_else(|| format!("map '{id}': the radar has no picture"))?
                    .to_string(),
                min: pair(ro.get("min"), "radar.min")?,
                max: pair(ro.get("max"), "radar.max")?,
            })
        }
    };
    let players = match o.get("players") {
        None | Some(serde_json::Value::Null) => None,
        Some(p) => {
            let po = p
                .as_object()
                .ok_or_else(|| format!("map '{id}': 'players' is not an object"))?;
            let n = |k: &str| {
                po.get(k)
                    .ok_or_else(|| format!("map '{id}': players has no {k}"))
                    .and_then(|v| u32_of(v, k))
            };
            Some(dc_mod_lua::PlayerRange {
                min: n("min")?,
                max: n("max")?,
            })
        }
    };
    Ok(dc_mod_lua::MapRecord {
        mod_id,
        patched_by: texts("patched_by")?,
        id: id.clone(),
        name,
        description: opt_text("description")?,
        world,
        modes: texts("modes")?,
        goals,
        round_seconds,
        picture: opt_text("picture")?,
        loading: opt_text("loading")?,
        radar,
        players,
    })
}

/// LG-313: the set document of a featured entry (`/entry_mods/<entry>.json`), which must hash to the lobby's push;
/// MAP-86: with the entry's map records when the lobby publishes them.
// DC-DEFAULT: LG-313 (proposed), MAP-86 (proposed)
pub fn fetch_entry_set(
    web: Option<SocketAddr>,
    entry: u32,
    want_hash: &str,
    timeout: Duration,
) -> Result<EntryDoc, String> {
    let web = web.ok_or("no web port")?;
    let path = format!("/entry_mods/{entry}.json");
    let r = web_get(
        web,
        &path,
        SERVER_LIST_CAP,
        timeout,
        LIST_DEADLINE,
        &mut |_, _| {},
    )?;
    if r.status != 200 {
        return Err(format!(
            "the server's set could not be read (status {})",
            r.status
        ));
    }
    let doc = EntryDoc::parse(&r.body)?;
    if doc.set.hash() != want_hash {
        return Err("the server's set differs from what the lobby announced".to_string());
    }
    Ok(doc)
}

/// How long the server list's background read of one modded row may wait, and may take in all.
pub const ROW_SET_TIMEOUT: Duration = Duration::from_secs(3);
pub const ROW_SET_DEADLINE: Duration = Duration::from_secs(5);

/// CLI-1632: the public set of another lobby of the server list, read in the background with no join. Its web port is
/// its auth port + 8080 (CLI-790; never the `DC_CLANWEB` override, which names the login lobby's). The trust rules are
/// a join's: the row's pin, else none (trust on first use). Unlike [`fetch_set`], only a real answer is a set: no
/// answer, a 404, a bad document or a refused certificate is `Err`, never "declares nothing".
// DC-DEFAULT: CLI-1632 (proposed, amended 2026-11-01)
pub fn read_row_set(auth: SocketAddr, pin: Option<&str>) -> Result<ServerModSet, String> {
    let port = auth
        .port()
        .checked_add(crate::identity::WEB_PORT_OFFSET)
        .ok_or("the auth port leaves no room for the web port")?;
    let mut trust = dc_botclient::tls::current();
    trust.pin = pin
        .map(dc_botclient::tls::normalize_fingerprint)
        .transpose()?;
    let r = web_get_with(
        SocketAddr::new(auth.ip(), port),
        "/server_list.json",
        SERVER_LIST_CAP,
        ROW_SET_TIMEOUT,
        ROW_SET_DEADLINE,
        &trust,
        &mut |_, _| {},
    )?;
    if r.status != 200 {
        return Err(format!("status {}", r.status));
    }
    ServerModSet::from_server_list(&r.body)
}

/// MOD-27: the operator pictures of a set read at a join (the login lobby's trust).
fn store_set_previews(web: Option<SocketAddr>, set: &ServerModSet, cfg: &ModConfig) {
    let (Some(web), Some(paths)) = (web, cfg.data_dir.as_deref().map(ModPaths::new)) else {
        return;
    };
    for sha in set.mods.iter().filter_map(|m| m.preview.as_deref()) {
        store_preview(&paths, sha, &mut |target| {
            web_get(
                web,
                target,
                dc_mod_lua::MAX_SCREENSHOT_BYTES,
                ROW_SET_TIMEOUT,
                ROW_SET_DEADLINE,
                &mut |_, _| {},
            )
        });
    }
}

/// MOD-27: the operator pictures of a row's set (the row's trust, [`read_row_set`]).
pub fn store_row_previews(
    auth: SocketAddr,
    pin: Option<&str>,
    set: &ServerModSet,
    paths: &ModPaths,
) {
    let Some(port) = auth.port().checked_add(crate::identity::WEB_PORT_OFFSET) else {
        return;
    };
    let mut trust = dc_botclient::tls::current();
    trust.pin = match pin
        .map(dc_botclient::tls::normalize_fingerprint)
        .transpose()
    {
        Ok(p) => p,
        Err(_) => return,
    };
    for sha in set.mods.iter().filter_map(|m| m.preview.as_deref()) {
        store_preview(paths, sha, &mut |target| {
            web_get_with(
                SocketAddr::new(auth.ip(), port),
                target,
                dc_mod_lua::MAX_SCREENSHOT_BYTES,
                ROW_SET_TIMEOUT,
                ROW_SET_DEADLINE,
                &trust,
                &mut |_, _| {},
            )
        });
    }
}

/// The client's mods: the sandbox of the server joined last (none on vanilla) and the join's status.
#[derive(Default)]
pub struct ClientMods {
    host: Option<ModHost>,
    recipe: Option<HostRecipe>,
    pub status: JoinReport,
    /// LG-313: the set being loaded is a featured entry's: the host runs under [`ModPolicy::Curated`].
    curated: bool,
}

impl ClientMods {
    pub fn new() -> ClientMods {
        ClientMods::default()
    }

    /// The loaded sandbox (`None`: vanilla, or no join yet).
    pub fn host(&self) -> Option<&ModHost> {
        self.host.as_ref()
    }

    /// Mod ids loaded, in load order.
    pub fn loaded(&self) -> Vec<String> {
        self.host
            .as_ref()
            .map(|h| h.load_order())
            .unwrap_or_default()
    }

    /// CLI-1619: how another thread rebuilds the loaded sandbox (`None`: vanilla, or nothing loaded).
    pub fn host_recipe(&self) -> Option<HostRecipe> {
        self.recipe.clone()
    }

    /// Unloads every mod (leaving a server, or vanilla).
    pub fn unload(&mut self) {
        crate::camo::set_content(None);
        crate::menu::reel::set(None);
        self.host = None;
        self.recipe = None;
    }

    /// The mod step of a join: everything is unloaded first, then the set of the lobby at `web` is fetched,
    /// verified and loaded. `Ok` = the mod-set hash for the login; `Err` = the reason (the login must not follow).
    pub fn join(
        &mut self,
        web: Option<SocketAddr>,
        expected: Option<&ServerModSet>,
        cfg: &ModConfig,
        progress: &mut dyn FnMut(ModState),
    ) -> Result<String, String> {
        self.unload();
        self.status = JoinReport::default();
        self.status.state = ModState::FetchingSet;
        progress(ModState::FetchingSet);
        let r = fetch_set(web, expected, cfg.timeout);
        if let Ok(set) = &r {
            store_set_previews(web, set, cfg);
        }
        self.finish(web, r, cfg, progress)
    }

    /// CLI-1618: the first half of a join behind the consent prompt: everything is unloaded and the set is read
    /// (nothing is downloaded). A failure is kept in the status as [`ClientMods::join`] keeps it.
    pub fn read_set(
        &mut self,
        web: Option<SocketAddr>,
        expected: Option<&ServerModSet>,
        cfg: &ModConfig,
        progress: &mut dyn FnMut(ModState),
    ) -> Result<ServerModSet, String> {
        self.unload();
        self.status = JoinReport::default();
        self.status.state = ModState::FetchingSet;
        progress(ModState::FetchingSet);
        match fetch_set(web, expected, cfg.timeout) {
            Ok(set) => {
                store_set_previews(web, &set, cfg);
                self.status.set = Some(set.clone());
                Ok(set)
            }
            Err(e) => {
                let _ = self.finish(web, Err(e.clone()), cfg, progress);
                Err(e)
            }
        }
    }

    /// CLI-1618: notes the pause for the player's answer (the status shows the set waiting for consent).
    pub fn wait_consent(&mut self, set: &ServerModSet) {
        self.status.set = Some(set.clone());
        self.status.state = ModState::NeedsConsent(set.clone());
    }

    /// CLI-1618: the player declined: nothing was downloaded; the status goes back to idle with the set kept.
    pub fn declined(&mut self) {
        self.unload();
        self.status.state = ModState::Idle;
        self.status.hash = None;
    }

    /// LG-313: picking a featured server entry: everything is unloaded, the entry's set is read from its document, must
    /// hash to what the lobby pushed, and is downloaded, verified and loaded under the curated limit. `Ok` = the set hash.
    /// MAP-86: the report's `maps` are the document's records when it has them (the lobby's table is what the indices
    /// mean: it may have left a record out), else the loaded host's; `community_map_base` is the document's.
    // DC-DEFAULT: LG-313 (proposed, project decision), MAP-86 (proposed)
    pub fn join_entry(
        &mut self,
        web: Option<SocketAddr>,
        entry: u32,
        want_hash: &str,
        cfg: &ModConfig,
        progress: &mut dyn FnMut(ModState),
    ) -> Result<String, String> {
        self.unload();
        self.status = JoinReport::default();
        self.status.state = ModState::FetchingSet;
        progress(ModState::FetchingSet);
        let (set, doc_maps) = match fetch_entry_set(web, entry, want_hash, cfg.timeout) {
            Ok(doc) => (Ok(doc.set), doc.maps),
            Err(e) => (Err(e), None),
        };
        self.curated = true;
        let r = self.finish(web, set, cfg, progress);
        self.curated = false;
        self.status.curated = r.is_ok();
        if let (Ok(_), Some(maps)) = (&r, doc_maps) {
            self.status.maps = maps;
        }
        r
    }

    /// The second half of a join: the set read before (and agreed to) is downloaded, verified and loaded.
    pub fn join_set(
        &mut self,
        web: Option<SocketAddr>,
        set: ServerModSet,
        cfg: &ModConfig,
        progress: &mut dyn FnMut(ModState),
    ) -> Result<String, String> {
        self.unload();
        self.status.curated = false;
        self.finish(web, Ok(set), cfg, progress)
    }

    fn finish(
        &mut self,
        web: Option<SocketAddr>,
        set: Result<ServerModSet, String>,
        cfg: &ModConfig,
        progress: &mut dyn FnMut(ModState),
    ) -> Result<String, String> {
        let r = set.and_then(|set| {
            self.status.set = Some(set.clone());
            let refused = self.load_set(web, &set, cfg, progress)?;
            // CLI-1611: the content the set's Lua produced (the lobby compares its hash at login, LG-271)
            let content = set_content(self.host.as_ref())?;
            if self.curated {
                if let Some(why) = dc_mod_lua::curated_content_refusal(&content) {
                    return Err(why);
                }
            }
            // MAP-32: the set's community maps as plain data (a record the host cannot turn into one fails the
            // join, as a malformed set does at the lobby)
            let maps = match self.host.as_ref() {
                Some(h) => h.map_records().map_err(|e| format!("mod maps: {e}"))?,
                None => Vec::new(),
            };
            Ok((set, refused, content, maps))
        });
        match r {
            Ok((set, refused, content, maps)) => {
                let hash = set.hash();
                self.status.hash = Some(hash.clone());
                self.status.local_refused = refused;
                crate::camo::set_content(Some(&content));
                // CRATE-5: a pack of the set may switch the case reel on (its HUD keys were checked at load)
                crate::menu::reel::set(
                    self.host
                        .as_ref()
                        .and_then(|h| crate::modassets::hud_overrides(h).ok())
                        .and_then(|o| o.reel()),
                );
                self.status.content = content;
                self.status.maps = maps;
                self.status.community_map_base = set.community_map_base;
                self.status.recipe = self.host_recipe();
                self.status.state = ModState::Ready;
                progress(ModState::Ready);
                Ok(hash)
            }
            Err(e) => {
                self.unload();
                self.status.state = ModState::Failed(e.clone());
                progress(ModState::Failed(e.clone()));
                Err(e)
            }
        }
    }

    /// Loads a fetched set; returns the refusals of local mods.
    fn load_set(
        &mut self,
        web: Option<SocketAddr>,
        set: &ServerModSet,
        cfg: &ModConfig,
        progress: &mut dyn FnMut(ModState),
    ) -> Result<Vec<String>, String> {
        let Policy::Modded {
            client_visual_allowed,
        } = set.policy
        else {
            // CLI-1609: vanilla = no host at all, local client-visual mods included
            return Ok(Vec::new());
        };
        let data = cfg
            .data_dir
            .as_deref()
            .ok_or("no data folder to keep the server's mods in")?;
        let paths = ModPaths::new(data);
        // server_rules mods run on the server only
        let needed: Vec<&ServerMod> = set.mods.iter().filter(|m| m.requires_client).collect();
        // LG-313: every pack of a curated set is on the client; nothing there runs out of the player's sight
        if self.curated && needed.len() != set.mods.len() {
            return Err("a curated set names a mod the client would not load".to_string());
        }
        let mut dirs = Vec::with_capacity(needed.len());
        for (i, m) in needed.iter().enumerate() {
            let bytes = fetch_package(web, &paths, m, cfg, i + 1, needed.len(), progress)?;
            progress(ModState::Verifying);
            let dest = paths.unpacked(&m.sha256);
            unpack_package(&bytes, &dest, cfg.max_mod_bytes)
                .map_err(|e| format!("mod '{}': {e}", m.id))?;
            dirs.push(dest);
        }
        progress(ModState::Loading);
        self.status.state = ModState::Loading;
        let mut host = ModHost::new();
        // LG-426: a curated entry's fragment may allow the player's local client_visual mods beside its packs
        host.set_policy(if self.curated {
            ModPolicy::Curated {
                client_visual_allowed,
            }
        } else {
            ModPolicy::Modded {
                client_visual_allowed,
            }
        });
        for (m, dir) in needed.iter().zip(&dirs) {
            let man = host.load_mod_dir_as(ModOrigin::Server, dir)?;
            if man.id != m.id || man.version.to_string() != m.version {
                return Err(format!(
                    "mod '{}': the package holds '{}@{}', not the '{}@{}' the server list names",
                    m.id, man.id, man.version, m.id, m.version
                ));
            }
        }
        let servers: Vec<String> = needed.iter().map(|m| m.id.clone()).collect();
        let mut refused = Vec::new();
        if client_visual_allowed {
            refused = load_local(&mut host, &paths.local(), cfg.local_enabled.as_deref());
        }
        self.recipe = Some(HostRecipe::of(&host, |id| {
            if servers.iter().any(|s| s == id) {
                ModOrigin::Server
            } else {
                ModOrigin::Local
            }
        }));
        self.host = Some(host);
        let keep: Vec<&str> = needed.iter().map(|m| m.sha256.as_str()).collect();
        trim_cache(&paths, cfg.cache_bytes, &keep);
        Ok(refused)
    }
}

/// Loads `<data>/mods/local/*` as local mods (the host refuses every kind but client_visual); returns the
/// refusals, which do not fail the join. With an `enabled` list (CLI-1617, the Mods screen's toggles) a folder
/// whose manifest id is not on it is skipped.
// DC-DEFAULT: CLI-1617 (proposed) a local mod loads only when the player enabled it
fn load_local(host: &mut ModHost, dir: &Path, enabled: Option<&[String]>) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("manifest.json").is_file())
        .filter(|p| match enabled {
            None => true,
            Some(on) => manifest_id(p).is_some_and(|id| on.iter().any(|e| *e == id)),
        })
        .collect();
    dirs.sort();
    dirs.into_iter()
        .filter_map(|d| host.load_mod_dir_as(ModOrigin::Local, &d).err())
        .collect()
}

/// The `id` of the manifest in mod folder `dir` (read as JSON; no validation).
pub fn manifest_id(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    v.get("id").and_then(|i| i.as_str()).map(str::to_string)
}

/// One local mod folder as the Mods screen lists it (CLI-1617).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMod {
    pub dir: PathBuf,
    /// The manifest's id (the folder name when the manifest does not parse).
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub screenshots: Vec<String>,
    /// MOD-27: the manifest's command help.
    pub commands: Vec<dc_mod_lua::ModCommand>,
    /// Why the manifest does not parse (the mod cannot load at all).
    pub broken: Option<String>,
    /// MOD-24: what the player can set on it.
    pub settings: Vec<dc_mod_lua::ModSetting>,
    /// CLI-1634: downloaded from a catalogue (it has a Remove button).
    pub downloaded: bool,
}

/// The local mod folders under `<data>/mods/local`, sorted by folder.
pub fn list_local(paths: &ModPaths) -> Vec<LocalMod> {
    let Ok(rd) = std::fs::read_dir(paths.local()) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("manifest.json").is_file())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|dir| {
            let folder = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match dc_mod_lua::ModManifest::from_file(&dir.join("manifest.json")) {
                Ok(m) => LocalMod {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.to_string(),
                    kind: m.kind.as_str().to_string(),
                    author: m.author.clone(),
                    description: m.description.clone(),
                    capabilities: m.capabilities.clone(),
                    screenshots: m.screenshots.clone(),
                    commands: m.commands.clone(),
                    broken: None,
                    settings: m.settings.clone(),
                    downloaded: dir.join(crate::modcatalog::DOWNLOAD_MARK).is_file(),
                    dir,
                },
                Err(e) => LocalMod {
                    id: manifest_id(&dir).unwrap_or(folder.clone()),
                    name: folder,
                    version: String::new(),
                    kind: String::new(),
                    author: String::new(),
                    description: String::new(),
                    capabilities: Vec::new(),
                    screenshots: Vec::new(),
                    commands: Vec::new(),
                    broken: Some(e),
                    settings: Vec::new(),
                    downloaded: dir.join(crate::modcatalog::DOWNLOAD_MARK).is_file(),
                    dir,
                },
            }
        })
        .collect()
}

/// Why local mod `m` did not (or will not) load at the last join, in plain words for the Mods screen; `None` =
/// nothing against it. The host's own refusal message of that join wins when there is one.
// DC-DEFAULT: CLI-1617 (proposed) the reasons: a broken manifest, a kind other than client_visual, a render / HUD
// hook, the vanilla rule, a server that allows no local mods
pub fn local_reason(m: &LocalMod, report: &JoinReport) -> Option<String> {
    if let Some(b) = &m.broken {
        return Some(format!("The manifest cannot be read: {b}"));
    }
    let tag = format!("Mod '{}' refused", m.id);
    if let Some(r) = report.local_refused.iter().find(|r| r.starts_with(&tag)) {
        return Some(r.clone());
    }
    if m.kind != "client_visual" {
        return Some(format!(
            "Only client-visual mods can be local; this one is '{}' and loads only from a server's set.",
            m.kind
        ));
    }
    if let Some(c) = dc_mod_lua::CLIENT_VIEW_CAPABILITIES
        .iter()
        .find(|c| m.capabilities.iter().any(|x| x == *c))
    {
        return Some(format!(
            "A local mod may not use '{c}' (render and HUD hooks come only from a server's set)."
        ));
    }
    match report.set.as_ref().map(|s| s.policy) {
        Some(Policy::Vanilla) => Some(
            "The last server was vanilla: vanilla means no mods at all, so it did not load.".into(),
        ),
        Some(Policy::Modded {
            client_visual_allowed: false,
        }) => Some("The last server does not allow local mods, so it did not load.".into()),
        _ => None,
    }
}

/// The plain words the player is shown for a declared capability (the Mods screen and the consent prompt).
// DC-DEFAULT: CLI-1618 (proposed) the fixed wording table; an unknown capability is shown as itself
pub fn capability_text(cap: &str) -> String {
    let t = match cap {
        "content.weapons" => "changes weapon stats",
        "content.items" => "changes items",
        "content.shop" => "changes the shop and its prices",
        "content.gameroom" => "changes room and match rules",
        "content.ai_brains" => "changes how bots behave",
        "content.missions" => "changes missions",
        "content.text" => "changes texts of the game",
        "content.music" => "changes the music",
        "content.hud" => "changes the look of the HUD",
        "content.maps" => "adds new maps",
        "hooks.lobby" => "changes rewards and lobby rules",
        "hooks.combat" => "changes damage and combat rules",
        "hooks.render" => "changes what you see in the match",
        "hooks.hud" => "draws on your HUD",
        "events" => "reacts to game events",
        "save" => "stores its own data",
        "assets" => "brings its own pictures and sounds",
        "world.full_visibility" => "everyone is visible through walls",
        "combat.no_recoil" => "lets rooms switch recoil and spread off",
        other => return other.to_string(),
    };
    t.to_string()
}

#[cfg(test)]
#[path = "mods_tests.rs"]
mod tests;

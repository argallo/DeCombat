//! The `server.toml` schema. Every struct is `deny_unknown_fields` so a typo is an error, and every
//! field has a documented default. Hosting keys: SPEC: HOSTING_AND_MODS.md §2, §8. Gameplay keys are the
//! "config key" column of DC_DEFAULTS.md (the `defaults_coverage` test keeps the two in step).
//!
//! No `Option` fields: "unset" is an empty string / empty list, so that serialising the default config
//! yields a complete key tree (used by env overrides and by the coverage test).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Root of `server.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: ServerSection,
    pub paths: PathsSection,
    pub log: LogSection,
    pub metrics: MetricsSection,
    pub directory: DirectorySection,
    pub mods: ModsSection,
    pub auth: AuthSection,
    pub names: NamesSection,
    pub lobby: LobbySection,
    pub room: RoomSection,
    pub modes: ModesSection,
    pub spy: SpySection,
    pub economy: EconomySection,
    pub inventory: InventorySection,
    pub gm: GmSection,
    pub shop: ShopSection,
    pub rewards: RewardsSection,
    pub bots: BotsSection,
    pub megaphone: MegaphoneSection,
    pub inbox: InboxSection,
    pub notice: NoticeSection,
    pub reports: ReportsSection,
    pub monitor: MonitorSection,
    pub messenger: MessengerSection,
    pub clan: ClanSection,
    pub dserver: DserverSection,
    pub combat: CombatSection,
    pub vis: VisSection,
    pub evars: EvarsSection,
    pub hisec: HisecSection,
    pub mail: MailSection,
    pub beta: BetaSection,
    pub admin_web: AdminWebSection,
    pub identity: IdentitySection,
    pub hosts: HostsSection,
    pub client: ClientSection,
    pub deploy: DeploySection,
}

/// `deploy.*`: deploys from the dashboard (dc-lobby-server deploy.rs, docs/DEPLOY_FROM_ADMIN.md).
// DC-DEFAULT: LG-392 (proposed, USER REQUEST 2026-11-03)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct DeploySection {
    /// The folder `tools/vps/deploy_setup.sh` made (`/opt/decombat/deploy`): the lobby reads `available.json`,
    /// `status.json`, `history.jsonl` and `host/`, and writes only `requests/`. Empty = no Updates tab, no host builds.
    pub dir: String,
}

/// What the lobby tells clients about versions (`GET /client_version.json` on the web port), read before their login.
// DC-DEFAULT: LG-322 (proposed, USER REQUEST 2026-11-02)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientSection {
    /// The newest release, `MAJOR.MINOR.PATCH`; an older client shows a notice and goes on. Empty = no notice.
    pub latest: String,
    /// The oldest client this lobby takes; an older one shows why and does not log in. Empty = no refusal.
    pub min_version: String,
    /// The download page the notice's button opens in the player's browser.
    pub update_url: String,
    /// Direct links to the platform zips (`[client.download]`): the client's "Update now" takes these instead of
    /// looking up the newest release.
    pub download: ClientDownloads,
}

impl Default for ClientSection {
    fn default() -> Self {
        ClientSection {
            latest: String::new(),
            min_version: String::new(),
            update_url: "https://github.com/argallo/DeCombat/releases/latest".into(),
            download: ClientDownloads::default(),
        }
    }
}

/// `[client.download]`: the release zip of each platform; empty = the client finds the newest release. Clients take
/// only an official release download (`https://github.com/argallo/DeCombat/releases/download/<tag>/<file>`, CLI-1864)
/// and ignore any other link; `dc-config check` warns about one.
// DC-DEFAULT: LG-404 (proposed, USER REQUEST 2026-11-04)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ClientDownloads {
    /// `DeCombat-win-<tag>.zip` of the release.
    pub windows: String,
    /// `DeCombat-mac-<tag>.zip` of the release.
    pub macos: String,
}

/// Network identity and ports. Loopback-safe: `bind` stays 127.0.0.1 unless `public = true`.
/// SPEC: HOSTING_AND_MODS.md §2 (network row); port defaults SPEC: LOBBY_HARNESS.md §1 (auth 10000,
/// lobby auth+2, clan lobby lobby+1, game 27888, clan web auth+8080).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSection {
    /// Display name shown in the server list and the community directory.
    pub name: String,
    /// Address to bind listeners on. Non-loopback requires `public = true`.
    pub bind: String,
    /// `--public`: bind 0.0.0.0 (overrides `bind`).
    pub public: bool,
    /// Auth listener port.
    pub auth_port: u16,
    /// Lobby listener port; 0 means `auth_port + 2`.
    pub lobby_port: u16,
    /// Clan lobby port; 0 means `lobby_port + 1`.
    pub clan_port: u16,
    /// HTTP port for clan pages and `/mods/<sha256>.zip`; 0 means `auth_port + 8080`.
    pub http_port: u16,
    /// Set false to run without the HTTP endpoint (a modded server then cannot serve its mods).
    pub http_enabled: bool,
    /// Game-server port advertised in the game-start packet (single-port mode).
    pub game_port: u16,
    /// Address advertised to clients in the server list and game-start packet.
    pub advertise_ip: String,
    /// Run lobby and dedicated server roles in one process (`dc-server --all-in-one`).
    pub all_in_one: bool,
}

impl Default for ServerSection {
    fn default() -> Self {
        ServerSection {
            name: "DeCombat Server".into(),
            bind: "127.0.0.1".into(),
            public: false,
            auth_port: 10000,
            lobby_port: 0,
            clan_port: 0,
            http_port: 0,
            http_enabled: true,
            game_port: 27888,
            advertise_ip: "127.0.0.1".into(),
            all_in_one: true,
        }
    }
}

/// File-system locations. Empty string = unset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsSection {
    /// The user's 2009 client folder (`--rom`). Required to run, optional to validate.
    pub rom: String,
    /// Directory holding mod zips served at `/mods/<sha256>.zip`.
    pub mods_dir: String,
    /// SQLite database file (WAL mode).
    pub db: String,
    /// Directory for logs; empty = log to stderr only.
    pub log_dir: String,
}

impl Default for PathsSection {
    fn default() -> Self {
        PathsSection {
            rom: String::new(),
            mods_dir: "mods".into(),
            db: "decombat.db".into(),
            log_dir: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogSection {
    /// "text" or "json" (structured logs).
    pub format: String,
    /// "error" | "warn" | "info" | "debug" | "trace".
    pub level: String,
}

impl Default for LogSection {
    fn default() -> Self {
        LogSection {
            format: "text".into(),
            level: "info".into(),
        }
    }
}

/// Prometheus text endpoint (`--metrics`). Off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsSection {
    pub enabled: bool,
    /// Bind address for the metrics listener (loopback by default, never follows `server.public`).
    pub bind: String,
    pub port: u16,
}

impl Default for MetricsSection {
    fn default() -> Self {
        MetricsSection {
            enabled: false,
            bind: "127.0.0.1".into(),
            port: 9464,
        }
    }
}

/// Self-registration with a live directory (`dc-directory`). Off by default; the community JSON on GitHub
/// Pages (listed by pull request) needs no registration. SPEC: HOSTING_AND_MODS.md §5, §8.3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DirectorySection {
    pub register: bool,
    /// Directory endpoints to heartbeat to (http/https URLs).
    pub urls: Vec<String>,
    /// `host:port` players should join; required when `register = true`.
    pub public_address: String,
    /// Region code, see [`crate::directory::REGIONS`].
    pub region: String,
    pub description: String,
    pub website: String,
    pub heartbeat_secs: u32,
}

impl Default for DirectorySection {
    fn default() -> Self {
        DirectorySection {
            register: false,
            urls: Vec::new(),
            public_address: String::new(),
            region: "lan".into(),
            description: String::new(),
            website: String::new(),
            heartbeat_secs: 60,
        }
    }
}

/// The largest mod package: what the client will download, so the default and the most a server may set (MOD-29).
pub const MAX_ZIP_BYTES: u64 = 32 * 1024 * 1024;

/// One mod in the server's declared set. SPEC: HOSTING_AND_MODS.md §3, §8.2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModRef {
    pub id: String,
    pub version: String,
    /// SHA-256 (64 lowercase hex) of the packed zip.
    pub sha256: String,
}

/// The mod-set declaration. `policy = "vanilla"` requires an empty set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModsSection {
    /// "vanilla" (no mods at all) or "modded".
    pub policy: String,
    /// Whether clients may add their own client-visual mods on this server.
    pub client_visual_allowed: bool,
    /// Per-mod download size cap served from `/mods/`, in bytes.
    pub max_zip_bytes: u64,
    /// Ordered mod set; the order is the load order the client must follow. `[[mods.set]]` in TOML.
    pub set: Vec<ModRef>,
    /// MOD-25: optional cosmetic mods this lobby offers for download (`[[mods.catalog]]`). Not part of the set or
    /// its hash, never loaded by the lobby, allowed on a vanilla lobby too.
    pub catalog: Vec<ModRef>,
}

impl Default for ModsSection {
    fn default() -> Self {
        ModsSection {
            policy: "vanilla".into(),
            client_visual_allowed: false,
            max_zip_bytes: MAX_ZIP_BYTES,
            set: Vec::new(),
            catalog: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// DC_DEFAULTS.md keys. Each default cites its row.
// ---------------------------------------------------------------------------------------------

/// `auth.*`: CD-13, CD-14, CD-19.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthSection {
    /// DC-DEFAULT: CD-13. "off" accepts any file-check data.
    pub file_check: String,
    /// DC-DEFAULT: CD-14.
    pub auto_create: bool,
    /// DC-DEFAULT: CD-19.
    pub session_ttl_secs: u32,
    /// DC-DEFAULT: AUTH-1 (proposed). Number of the first account of a never-used accounts table.
    pub first_account_no: u32,
}

impl Default for AuthSection {
    fn default() -> Self {
        AuthSection {
            file_check: "off".into(),
            auto_create: true,
            session_ttl_secs: 60,
            first_account_no: 1001,
        }
    }
}

/// `names.*`: CD-16, CD-17.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NamesSection {
    /// DC-DEFAULT: CD-16.
    pub case_insensitive: bool,
    /// DC-DEFAULT: CD-17. "optional" accepts an empty or existing referrer; "off" ignores the field.
    pub referrer: String,
}

impl Default for NamesSection {
    fn default() -> Self {
        NamesSection {
            case_insensitive: true,
            referrer: "optional".into(),
        }
    }
}

/// One advertised lobby server. DC-DEFAULT: CD-21 (empty list = the implicit single server).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LobbyServerCfg {
    pub name: String,
    /// Required (> 0) for an entry of this lobby; unused by an entry naming another lobby (`lobby`).
    #[serde(default)]
    pub capacity: u32,
    /// The game host of this entry's matches: "" = the lobby's own machine, else the name of a `[[hosts.agents]]`
    /// entry (SPEC: HOST_AGENT.md §1). DC-DEFAULT: CD-88 (proposed).
    #[serde(default)]
    pub host: String,
    /// The number of channels this entry owns (a contiguous block of channel ids). With one entry (or none) the
    /// channel count stays `lobby.channels` (CD-21). DC-DEFAULT: CD-21 (amended), CD-88 (proposed).
    #[serde(default = "default_server_channels")]
    pub channels: u32,
    /// LG-304: what this entry adds to the vanilla game ([`SERVER_FEATURES`]); same accounts and DB as its siblings.
    /// DC-DEFAULT: LG-304 (proposed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// LG-313: the curated mod set of this entry alone (zips in the lobby's mods dir); empty by default. Only on a lobby
    /// whose `mods.policy` is `vanilla`. DC-DEFAULT: LG-313 (proposed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<ModRef>,
    /// LG-426: the player's local `client_visual` mods (a crosshair, a HUD look) load on this entry, beside its curated
    /// set; the entry's counterpart of the lobby-level `mods.client_visual_allowed`. Only on an entry with `mods`.
    /// Default false. DC-DEFAULT: LG-426 (proposed).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub client_visual_allowed: bool,
    /// LG-272: `host:port` of ANOTHER lobby instance (its auth port), e.g. the modded instance on the same machine.
    /// Such an entry is no server of this lobby (no id, no channels, no host): our clients list it after this lobby's
    /// entries (push 0x3f02, `dc_wire::moddedlist`); the 2009 client never sees it. "" = an entry of this lobby.
    /// DC-DEFAULT: LG-272 (proposed).
    #[serde(default)]
    pub lobby: String,
    /// LG-272: the other lobby's certificate SHA-256 (64 hex digits, colon pairs allowed) or "".
    #[serde(default)]
    pub fingerprint: String,
    /// LG-272: `modded` or `vanilla` (default `modded`) for an entry with `lobby`.
    #[serde(default)]
    pub policy: String,
    /// LG-272: the region shown on the row (free text such as "US Central"; "" = none).
    #[serde(default)]
    pub region: String,
}

impl LobbyServerCfg {
    /// True for an entry naming another lobby instance (LG-272).
    pub fn is_remote(&self) -> bool {
        !self.lobby.trim().is_empty()
    }
}

impl LobbySection {
    /// The entries of this lobby (CD-21): every `[[lobby.servers]]` entry without `lobby`.
    pub fn local_servers(&self) -> Vec<LobbyServerCfg> {
        self.servers
            .iter()
            .filter(|e| !e.is_remote())
            .cloned()
            .collect()
    }

    /// The entries naming other lobby instances (LG-272), in config order.
    pub fn remote_servers(&self) -> Vec<LobbyServerCfg> {
        self.servers
            .iter()
            .filter(|e| e.is_remote())
            .cloned()
            .collect()
    }
}

/// The values `lobby.servers[].features` takes. DC-DEFAULT: LG-304 (proposed).
pub const SERVER_FEATURES: &[&str] = &["gun_game", "hardcore", "possession"];

fn default_server_channels() -> u32 {
    3
}

/// One channel. DC-DEFAULT: CD-21 (empty list = three channels generated by the lobby).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LobbyChannelCfg {
    pub name: String,
    pub capacity: u32,
}

/// `lobby.*`: CD-21, CD-55.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LobbySection {
    pub servers: Vec<LobbyServerCfg>,
    pub channels: Vec<LobbyChannelCfg>,
    /// DC-DEFAULT: CD-55.
    pub single_session: bool,
}

impl Default for LobbySection {
    fn default() -> Self {
        LobbySection {
            servers: Vec::new(),
            channels: Vec::new(),
            single_session: true,
        }
    }
}

/// `room.*`: CD-23, CD-24, CD-25, CD-49, CD-51, CD-56, CD-57.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoomSection {
    /// DC-DEFAULT: CD-23.
    pub countdown_ticks: u32,
    /// DC-DEFAULT: CD-23.
    pub countdown_ms: u32,
    /// DC-DEFAULT: CD-24.
    pub allow_late_join: bool,
    /// DC-DEFAULT: CD-25.
    pub max_per_channel: u32,
    /// DC-DEFAULT: CD-49 (room option 1).
    pub default_mid_join: u8,
    /// DC-DEFAULT: CD-51.
    pub entry_team_balance: bool,
    /// DC-DEFAULT: CD-56.
    pub max_players_cap: u32,
    /// DC-DEFAULT: CD-57.
    pub handover_any_state: bool,
    /// DC-DEFAULT: RM-1 (proposed). A host settings change clears every other member's ready state.
    pub settings_change_clears_ready: bool,
    /// DC-DEFAULT: DSV-165 (gameplay choice, user-tunable). Multiplier of every blast's damage to players (the ROM
    /// row's `AreaDamage` x falloff x this); 1.0 = the 2009 values. Env `DC_ROOM__BLAST_DAMAGE_SCALE`.
    pub blast_damage_scale: f64,
    /// DC-DEFAULT: GMD-2 (clarification, project decision "every room has a time limit"). The round time
    /// of a room whose mode file gives none.
    pub round_secs: RoundSecsSection,
}

/// `room.round_secs.*`: GMD-2 (project decision). `fallback` and `default` are named keys; every other
/// key is a ROM mode class (`GameType`, 0..=255) with its seconds, e.g. `1 = 900`. A class not listed keeps the
/// built-in GMD-2 table of the lobby (`dc_bute::RoundTimePolicy`); `check` refuses a key that is not a class.
// DC-DEFAULT: GMD-2 (clarification) the same keys as the lobby's lobby.json `room.round_secs.<class>`,
// `room.round_secs.default`, `room.round_secs.fallback`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoundSecsSection {
    /// false = no fallback at all: a mode file without a time limit gives `time=0` (DSV-20, no clock).
    pub fallback: bool,
    /// Seconds for a mode class neither the built-in table nor this table lists.
    pub default: u32,
    /// `<mode class> = <seconds>` overrides (0 = no clock for that class).
    #[serde(flatten)]
    pub classes: BTreeMap<String, u32>,
}

impl Default for RoundSecsSection {
    fn default() -> Self {
        RoundSecsSection {
            fallback: true,
            // DC-DEFAULT: GMD-2 10 min for an unlisted class (dc_bute::RoundTimePolicy::default)
            default: 600,
            classes: BTreeMap::new(),
        }
    }
}

impl Default for RoomSection {
    fn default() -> Self {
        RoomSection {
            countdown_ticks: 5,
            countdown_ms: 1000,
            allow_late_join: true,
            max_per_channel: 100,
            default_mid_join: 1,
            entry_team_balance: true,
            max_players_cap: 16,
            handover_any_state: true,
            settings_change_clears_ready: true,
            blast_damage_scale: 1.0,
            round_secs: RoundSecsSection::default(),
        }
    }
}

/// `spy.*`: the Spy Hunt rules `dc-dserver` takes from the operator (the rest are the ROM's GMS files and DC_DEFAULTS.md
/// section modes-spyhunt).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpySection {
    /// DC-DEFAULT: DSV-451 (proposed, amended by project decision "the super spy did not have enough extra
    /// hp"). The agent's health and its maximum while he is the agent.
    pub agent_health: f64,
    /// DC-DEFAULT: DSV-451 (proposed, amended 2026-10-30). The agent's armour and its maximum while he is the agent.
    pub agent_armor: f64,
    /// DC-DEFAULT: DSV-456 (proposed, USER REQUEST 2026-10-31 "the super spy flame thrower and rocket launch should
    /// do more damage"). Multiplier of the burn and blast damage of the agent's own rounds; 1.0 = the ROM rows.
    pub agent_damage_scale: f64,
}

impl Default for SpySection {
    fn default() -> Self {
        SpySection {
            agent_health: 1000.0,
            agent_armor: 500.0,
            agent_damage_scale: 2.0,
        }
    }
}

/// `modes.*`: which game modes the lobby offers and lets start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModesSection {
    /// DC-DEFAULT: GMD-26 (amended, project decision). The modes `dc-dserver` plays today, by our names (`TDM`,
    /// `TDM_CLAN`, `COOP`, `BOMB`, `BOMB_CLAN`, `FFA`, `FLAG`, `FLAG_CLAN`, `BOUNTY`, `INFECT`, `ELIM`,
    /// `ELIM_CLAN`) or a ROM mode class number (a mod's own row). The lobby keeps only these bits in the 0x311
    /// map / mode masks and refuses the Start of a room of any other mode. `["*"]` (or an empty list) = every
    /// ROM-enabled mode, the gate off.
    pub playable: Vec<String>,
}

impl Default for ModesSection {
    fn default() -> Self {
        ModesSection {
            // the one vocabulary (`crate::modes::MODE_NAMES`), so a map record and the gate spell modes alike
            playable: crate::modes::MODE_NAMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// Reward shape for CD-03 (`economy.exp`) and CD-04 (`economy.gp`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RewardRates {
    pub finish: u32,
    pub kill: u32,
    pub assist: u32,
    /// Winner bonus in percent.
    pub win_percent: u32,
    pub objective: u32,
    /// Penalty per team kill; -1 means "same as `kill`".
    pub team_kill_penalty: i32,
    /// DC-DEFAULT: CD-86 (proposed, USER REQUEST 2026-10-31). Flat extra of the Spy Hunt agent whose upload won
    /// the match, on top of the win bonus.
    pub agent_win: u32,
}

impl RewardRates {
    /// DC-DEFAULT: CD-03 (EXP): finish 10, kill 4, assist 0, win +25 %, objective 15; CD-86 agent win 30.
    pub fn exp_default() -> Self {
        RewardRates {
            finish: 10,
            kill: 4,
            assist: 0,
            win_percent: 25,
            objective: 15,
            team_kill_penalty: -1,
            agent_win: 30,
        }
    }
    /// DC-DEFAULT: CD-04 (GP): finish 20, kill 6, win +25 %, objective 25; CD-86 agent win 0.
    pub fn gp_default() -> Self {
        RewardRates {
            finish: 20,
            kill: 6,
            assist: 0,
            win_percent: 25,
            objective: 25,
            team_kill_penalty: -1,
            agent_win: 0,
        }
    }
}

impl Default for RewardRates {
    fn default() -> Self {
        Self::exp_default()
    }
}

/// The keys one `[economy.exp]` / `[economy.gp]` table may set; a key it leaves out keeps that table's own default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RewardPatch {
    finish: Option<u32>,
    kill: Option<u32>,
    assist: Option<u32>,
    win_percent: Option<u32>,
    objective: Option<u32>,
    team_kill_penalty: Option<i32>,
    agent_win: Option<u32>,
}

impl RewardPatch {
    fn onto(self, b: RewardRates) -> RewardRates {
        RewardRates {
            finish: self.finish.unwrap_or(b.finish),
            kill: self.kill.unwrap_or(b.kill),
            assist: self.assist.unwrap_or(b.assist),
            win_percent: self.win_percent.unwrap_or(b.win_percent),
            objective: self.objective.unwrap_or(b.objective),
            team_kill_penalty: self.team_kill_penalty.unwrap_or(b.team_kill_penalty),
            agent_win: self.agent_win.unwrap_or(b.agent_win),
        }
    }
}

// DC-DEFAULT: CD-03 / CD-04 / CD-86 each table fills its missing keys from its own defaults (GP agent_win is 0, not the EXP 30)
fn de_exp<'de, D: serde::Deserializer<'de>>(d: D) -> Result<RewardRates, D::Error> {
    Ok(RewardPatch::deserialize(d)?.onto(RewardRates::exp_default()))
}

fn de_gp<'de, D: serde::Deserializer<'de>>(d: D) -> Result<RewardRates, D::Error> {
    Ok(RewardPatch::deserialize(d)?.onto(RewardRates::gp_default()))
}

/// `economy.*`: CD-01..05, CD-41, CD-54.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EconomySection {
    /// DC-DEFAULT: CD-01. `[days, weight_percent]` pairs, 0 days = permanent.
    pub case_odds: Vec<[u32; 2]>,
    /// DC-DEFAULT: CD-02.
    pub case_split_on_purchase: bool,
    /// DC-DEFAULT: CD-03.
    #[serde(deserialize_with = "de_exp")]
    pub exp: RewardRates,
    /// DC-DEFAULT: CD-04.
    #[serde(deserialize_with = "de_gp")]
    pub gp: RewardRates,
    /// DC-DEFAULT: CD-05. Path of a `ranks.toml` override; empty = generated geometric table.
    pub ranks: String,
    /// DC-DEFAULT: CD-41. Read the rank-up bonus from the ROM rank table.
    pub rank_bonus: bool,
    /// DC-DEFAULT: CD-54.
    pub trust_server_rewards: bool,
    /// DC-DEFAULT: CD-68 (proposed). A GP supply case bought in the shop opens at once.
    pub gp_case_open_on_buy: bool,
    /// DC-DEFAULT: HS-8 (proposed). Award duration of a HiSec case, `[days, weight]`, 0 days = permanent.
    pub hisec_case_odds: Vec<[u32; 2]>,
}

impl Default for EconomySection {
    fn default() -> Self {
        EconomySection {
            case_odds: vec![[1, 60], [3, 25], [7, 10], [30, 4], [0, 1]],
            case_split_on_purchase: true,
            exp: RewardRates::exp_default(),
            gp: RewardRates::gp_default(),
            ranks: String::new(),
            rank_bonus: true,
            trust_server_rewards: false,
            gp_case_open_on_buy: true,
            hisec_case_odds: vec![[7, 50], [30, 35], [0, 15]],
        }
    }
}

/// `inventory.*`: CD-06, CD-18, CD-48, CD-52, CD-53.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InventorySection {
    /// DC-DEFAULT: CD-06.
    pub expiry_scan_secs: u32,
    /// DC-DEFAULT: CD-18.
    pub starter: bool,
    /// DC-DEFAULT: CD-48.
    pub starter_backpack: bool,
    /// DC-DEFAULT: CD-52. "weapons" | "all" | "off".
    pub default_repair: String,
    /// DC-DEFAULT: CD-53.
    pub period_activation: bool,
    /// DC-DEFAULT: CD-18. Starter loadout override, ROM item ids; empty = pick from the ROM item table.
    pub starter_items: Vec<i32>,
    /// DC-DEFAULT: CD-69 (proposed). At most N non-default items per inventory.
    pub max_items: u32,
    /// DC-DEFAULT: CD-77 (proposed). Own slots (0..3 = weapon slots) whose items may sit in backpack slots 10..13.
    pub backpack_kinds: Vec<i32>,
}

impl Default for InventorySection {
    fn default() -> Self {
        InventorySection {
            expiry_scan_secs: 60,
            starter: true,
            starter_backpack: true,
            default_repair: "weapons".into(),
            period_activation: true,
            starter_items: Vec::new(),
            max_items: 100,
            backpack_kinds: vec![0, 1, 2, 3],
        }
    }
}

/// `gm.*`: CD-07, CD-33. Operator-defined further `gm.*` keys are carried in `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GmSection {
    /// DC-DEFAULT: CD-33.
    pub mute_server_side: bool,
    /// DC-DEFAULT: OPS-1 (proposed). Who may use the test chat commands: "gm" | "all" | "loopback".
    pub test_commands: String,
    /// DC-DEFAULT: OPS-4 (proposed). Mute length in minutes when the tool carries none.
    pub default_mute_minutes: u32,
    /// DC-DEFAULT: OPS-4 (proposed). Upper bound of one mute, minutes.
    pub max_mute_minutes: u32,
    /// DC-DEFAULT: OPS-3 (proposed). Day count reported for a permanent ban.
    pub permanent_ban_days: u32,
    /// DC-DEFAULT: OPS-5 (proposed). The target lookup shows the player's address to the operator.
    pub lookup_shows_address: bool,
    /// DC-DEFAULT: CD-64. Moderators are flagged to the game server.
    pub flag_moderators_to_game: bool,
    /// DC-DEFAULT: LG-395 (proposed). The most NX one dashboard change may add or take.
    pub admin_max_nx: u32,
    /// DC-DEFAULT: LG-395 (proposed). The most GP one dashboard change may add or take.
    pub admin_max_gp: u32,
    /// DC-DEFAULT: LG-395 (proposed). The most EXP one dashboard change may add or take.
    pub admin_max_exp: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl Default for GmSection {
    fn default() -> Self {
        GmSection {
            mute_server_side: true,
            test_commands: "loopback".into(),
            default_mute_minutes: 60,
            max_mute_minutes: 60 * 24 * 365,
            permanent_ban_days: 0,
            lookup_shows_address: true,
            flag_moderators_to_game: true,
            admin_max_nx: 100_000,
            admin_max_gp: 1_000_000,
            admin_max_exp: 10_000_000,
            extra: BTreeMap::new(),
        }
    }
}

/// `shop.*`: CD-09, CD-35, CD-36, CD-39.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShopSection {
    /// DC-DEFAULT: CD-09.
    pub new_days: u32,
    /// DC-DEFAULT: CD-09.
    pub hot_percent: u32,
    /// Sells the seasonal GP products in their seasons (SHOP-2).
    pub seasonal: bool,
    /// DC-DEFAULT: CD-09. Product ids the operator puts on sale.
    pub sale_list: Vec<u32>,
    /// DC-DEFAULT: CD-35. "rom" = shown flag from the ROM row.
    pub lists: String,
    /// DC-DEFAULT: CD-36.
    pub credit_to_inbox: bool,
    /// DC-DEFAULT: CD-39. Operator-issued coupon codes, `code = product_id`.
    pub coupons: BTreeMap<String, u32>,
    /// DC-DEFAULT: CD-39. Total redemptions per coupon code over all accounts; absent = unlimited.
    pub coupon_uses: BTreeMap<String, i64>,
    /// DC-DEFAULT: CD-35. Operator replacement of one computed product list; an absent list stays computed.
    pub list_override: ShopListOverride,
    /// DC-DEFAULT: CD-70 (proposed). Price overrides for the price request, `product_id = price`.
    pub price_override: BTreeMap<String, u32>,
    /// DC-DEFAULT: CD-71 (proposed). An extension request extends the owned instance.
    pub extend: bool,
    /// DC-DEFAULT: CD-73 (proposed). An offline gift receiver gets a pending notice.
    pub notify_offline_receiver: bool,
    /// DC-DEFAULT: CD-18 refinement (project decision. ROM item number issued in weapon slot 0 to a new
    /// character; 0 = unset (the ROM balanced pick, else the first marked row).
    pub default_primary_item: u32,
    /// DC-DEFAULT: CD-18 refinement (project decision. ROM item number issued in weapon slot 1; 0 = unset.
    pub default_secondary_item: u32,
}

/// `bots.*`: the bot seats and the seat kept open for people (docs/BOTS.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BotsSection {
    /// DC-DEFAULT: LG-318 (proposed, project decision). Accounts of the bot pool; 0 = no pool.
    pub pool_size: u32,
    /// DC-DEFAULT: LG-318 (proposed). Seats one address may hold; this machine has no limit.
    pub pool_per_ip: u32,
    /// DC-DEFAULT: LG-319 (proposed, project decision). People online on this lobby from which a room with bots
    /// keeps a seat open. Bots do not count; 0 = never.
    pub keep_seat_online_players: u32,
    /// DC-DEFAULT: LG-388 (proposed, project decision). The lobby runs the bot groups of its operator dashboard as
    /// `dc-botclient` child processes (docs/BOTS.md "Bots the lobby runs"); off = no bot is started.
    pub managed: bool,
    /// DC-DEFAULT: LG-388 (proposed). Most bots of all enabled groups together (a group holds at most 7).
    pub managed_max_total: u32,
}

impl Default for BotsSection {
    fn default() -> Self {
        BotsSection {
            pool_size: 100,
            pool_per_ip: 7,
            keep_seat_online_players: 8,
            managed: false,
            managed_max_total: 21,
        }
    }
}

/// `rewards.*`: project decision (no rewards for leavers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RewardsSection {
    /// DC-DEFAULT: REW-1 (project decision. true = a player who left before the round end is paid and
    /// counted like the others.
    pub pay_leavers: bool,
    /// DC-DEFAULT: CD-89 (amended, project decision). Per-rank override of the promotion gifts: a match
    /// result that raises a character to `rank` puts the listed products into its inbox instead of the rank's
    /// unlocked products or supply case (LG-303), once per character and rank. Empty = the rule for every rank.
    pub rank_gifts: Vec<RankGift>,
    /// DC-DEFAULT: CD-89 (project decision. Days of the variant of an unlocked product a promotion gives
    /// (1..=3650); a product family without that period gives its shortest one.
    pub rank_unlock_days: u32,
    /// DC-DEFAULT: CD-89 (project decision. The NX supply case pool of a promotion that unlocks nothing:
    /// one product of it at random (`ProductID`s); empty = every NX "Supply Case ..." product of the ROM shop list.
    pub rank_case_products: Vec<u32>,
    /// DC-DEFAULT: LG-310, LG-311 (proposed). NX is earned on the server: false = no daily bonus and no clan win NX
    /// (operator events still pay).
    pub nx_enabled: bool,
    /// DC-DEFAULT: LG-310 (proposed). NX (prepaid) of the first lobby login of a UTC day (0..=1000000; 0 = none).
    pub nx_daily: u32,
    /// DC-DEFAULT: LG-311 (proposed). NX (prepaid) per member of the winning side of a clan match (0..=1000000).
    pub nx_clan_win: u32,
    /// DC-DEFAULT: LG-311 (proposed). Paid clan wins per account and UTC day (1..=100).
    pub nx_clan_win_daily_cap: u32,
    /// DC-DEFAULT: LG-311 (proposed). Clan members who played to the end on each side, bots not counted (1..=8).
    pub nx_clan_min_players: u32,
    /// DC-DEFAULT: LG-311 (proposed). Seconds the match has to last (0..=3600).
    pub nx_clan_min_seconds: u32,
}

impl Default for RewardsSection {
    fn default() -> Self {
        RewardsSection {
            pay_leavers: false,
            rank_gifts: Vec::new(),
            rank_unlock_days: 7,
            rank_case_products: Vec::new(),
            nx_enabled: true,
            nx_daily: 250,
            nx_clan_win: 200,
            nx_clan_win_daily_cap: 3,
            nx_clan_min_players: 2,
            nx_clan_min_seconds: 180,
        }
    }
}

/// One `[[rewards.rank_gifts]]` entry: the shop products a promotion to `rank` delivers to the inbox, one row per
/// product (a case pack splits as a purchase does, CD-02); each item keeps its product's period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RankGift {
    /// The rank number (the ROM level table's row, as the client counts it: 1 = the first rank above the recruit).
    pub rank: u32,
    /// `ProductID`s of the ROM shop list (`SHOP_ITEM_LIST.TXT`).
    pub products: Vec<u32>,
}

/// `shop.list_override.*`: product ids that replace a computed list (CD-35). Presence in the file (not the
/// value) decides whether a list is overridden, so an empty list can blank a section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ShopListOverride {
    pub view: Vec<u32>,
    pub hot: Vec<u32>,
    pub new: Vec<u32>,
    pub sale: Vec<u32>,
}

impl Default for ShopSection {
    fn default() -> Self {
        ShopSection {
            new_days: 30,
            hot_percent: 10,
            seasonal: false,
            sale_list: Vec::new(),
            lists: "rom".into(),
            credit_to_inbox: true,
            coupons: BTreeMap::new(),
            coupon_uses: BTreeMap::new(),
            list_override: ShopListOverride::default(),
            price_override: BTreeMap::new(),
            extend: true,
            notify_offline_receiver: true,
            // the user's choice 2026-11-03 (M16A3 + M1911 MEU, was M4A1 (Default) 145 + G23 (Default) 164),
            // resolved from the ROM weapon table by the operator
            default_primary_item: 20,
            default_secondary_item: 1,
        }
    }
}

/// `megaphone.*`: CD-40.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MegaphoneSection {
    /// DC-DEFAULT: CD-40.
    pub secs: u32,
    /// DC-DEFAULT: CD-83 (proposed). Message length limit in characters.
    pub max_chars: u32,
}

impl Default for MegaphoneSection {
    fn default() -> Self {
        MegaphoneSection {
            secs: 30,
            max_chars: 65,
        }
    }
}

/// `inbox.*`: CD-74 (proposed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InboxSection {
    /// DC-DEFAULT: CD-74 (proposed). Rows per list reply (the frame limit allows at most 141).
    pub max_rows: u32,
}

impl Default for InboxSection {
    fn default() -> Self {
        InboxSection { max_rows: 141 }
    }
}

/// `notice.*`: OPS-2, OPS-9, OPS-10, OPS-12 (proposed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NoticeSection {
    /// DC-DEFAULT: OPS-2 (proposed). Operator ticker scroll period, seconds.
    pub ticker_period_secs: u32,
    /// DC-DEFAULT: OPS-2 (proposed). Operator ticker repeat count.
    pub ticker_repeat: u32,
    /// DC-DEFAULT: OPS-9 (proposed). Supply case awards announced: "off" | "permanent" | "all".
    pub case_announce: String,
    /// DC-DEFAULT: OPS-10 (proposed). Ticker text shown at every lobby login; empty = none.
    pub login_ticker: String,
    /// DC-DEFAULT: OPS-10 (proposed). Box text shown at every lobby login; empty = none.
    pub login_box: String,
    /// DC-DEFAULT: OPS-12 (proposed). The periodic expiry scan also sends boxes.
    pub expiry_boxes: bool,
}

impl Default for NoticeSection {
    fn default() -> Self {
        NoticeSection {
            ticker_period_secs: 30,
            ticker_repeat: 3,
            case_announce: "permanent".into(),
            login_ticker: String::new(),
            login_box: String::new(),
            expiry_boxes: true,
        }
    }
}

/// `reports.*`: OPS-7 (proposed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReportsSection {
    /// DC-DEFAULT: OPS-7 (proposed). Stored reports per reporter per hour.
    pub per_hour: u32,
}

impl Default for ReportsSection {
    fn default() -> Self {
        ReportsSection { per_hour: 20 }
    }
}

/// `monitor.*`: the operator monitor of the lobby (MON-2, proposed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MonitorSection {
    /// DC-DEFAULT: MON-2 (proposed). Days the match records, link windows and account events are kept; 0 = forever.
    pub retention_days: u32,
}

impl Default for MonitorSection {
    fn default() -> Self {
        MonitorSection { retention_days: 14 }
    }
}

/// `messenger.*`: OPS-8 (proposed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MessengerSection {
    /// DC-DEFAULT: OPS-8 (proposed). Friend list size.
    pub max_friends: u32,
    /// DC-DEFAULT: OPS-8 (proposed). Whispers only between friends.
    pub whisper_friends_only: bool,
}

impl Default for MessengerSection {
    fn default() -> Self {
        MessengerSection {
            max_friends: 50,
            whisper_friends_only: false,
        }
    }
}

/// `clan.*`: CD-11, CD-47.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClanSection {
    /// DC-DEFAULT: CD-11. `clan.mode_weight.<MODE_KEY>`; absent mode = 1.0.
    pub mode_weight: BTreeMap<String, f64>,
    /// DC-DEFAULT: CD-11.
    pub basic_point_default: u32,
    /// DC-DEFAULT: CD-11.
    pub winner_point_default: u32,
    /// DC-DEFAULT: CD-47. 0 = use `server.http_port`.
    pub web_port: u16,
    /// DC-DEFAULT: CLN-1 (proposed). Only clan members may enter a channel of the clan lobby.
    pub channel_members_only: bool,
    /// DC-DEFAULT: CLN-3 (proposed). A clan match starts with two clans and at least N members per side.
    pub start_min_per_side: u32,
    /// DC-DEFAULT: CLN-5 (proposed). Cumulative clan exp where levels 1..6 start (the first must be 0).
    pub level_steps: Vec<i64>,
    /// DC-DEFAULT: CLN-6 (proposed). Member limit per clan level.
    pub member_limits: Vec<u32>,
    /// DC-DEFAULT: CLN-8 (proposed). Web page actions need the account's lobby session online.
    pub web_requires_session: bool,
    /// DC-DEFAULT: CLN-9 (proposed). `!clan ...` chat commands.
    pub chat_commands: bool,
}

impl Default for ClanSection {
    fn default() -> Self {
        ClanSection {
            mode_weight: BTreeMap::new(),
            basic_point_default: 0,
            winner_point_default: 0,
            web_port: 0,
            channel_members_only: true,
            start_min_per_side: 2,
            level_steps: vec![0, 2_000, 8_000, 25_000, 70_000, 180_000],
            member_limits: vec![20, 40, 60, 80, 100, 120],
            web_requires_session: true,
            chat_commands: true,
        }
    }
}

/// `dserver.*`: CD-10, CD-50, CD-58..65, CD-67. Per-room UDP ports come from `port_range` (CD-10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DserverSection {
    /// DC-DEFAULT: CD-10. "lo-hi", inclusive.
    pub port_range: String,
    /// DC-DEFAULT: CD-50.
    pub quit_grace_secs: u32,
    /// DC-DEFAULT: CD-58.
    pub host_rate_bps: u32,
    /// DC-DEFAULT: CD-59.
    pub file_info: String,
    /// DC-DEFAULT: CD-60.
    pub hello_match: String,
    /// DC-DEFAULT: CD-61.
    pub startup_deadline_secs: u32,
    /// DC-DEFAULT: CD-62.
    pub sound_full_desc: bool,
    /// DC-DEFAULT: CD-63.
    pub tick_hz: u32,
    /// DC-DEFAULT: CD-64.
    pub remote_console: String,
    /// DC-DEFAULT: CD-65. 0 = the transport window share per tick.
    pub send_budget_bits: u32,
    /// DC-DEFAULT: CD-67.
    pub echo_own_rotation: bool,
    /// DC-DEFAULT: LDO-1 (proposed). The lobby's `user` line carries `loadout=` and `face=` tokens.
    pub loadout_tokens: bool,
    /// DC-DEFAULT: DSV-35 (proposed), DSERVER_PIPE.md §12.B. After storing a `@result` the lobby sends
    /// `gain <uid> <exp> <gp>` lines to the game server.
    pub send_gain_lines: bool,
}

impl Default for DserverSection {
    fn default() -> Self {
        DserverSection {
            port_range: "29901-29999".into(),
            quit_grace_secs: 5,
            host_rate_bps: 5_120_000,
            file_info: "size".into(),
            hello_match: "name".into(),
            startup_deadline_secs: 120,
            sound_full_desc: true,
            tick_hz: 10,
            remote_console: "moderators".into(),
            send_budget_bits: 0,
            echo_own_rotation: false,
            loadout_tokens: true,
            send_gain_lines: true,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// [combat]: the game server's damage and tuning keys (dc_dserver::game::COMBAT_KEYS, DSV-30, DSV-34)
// ---------------------------------------------------------------------------------------------

/// A number, or the word `rom` meaning "use the ROM-derived value" (written as a number or `"rom"` in TOML).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RomOr {
    Rom,
    Value(f64),
}

impl RomOr {
    /// The text form `GameTuning::set_combat` takes.
    pub fn text(&self) -> String {
        match self {
            RomOr::Rom => "rom".into(),
            RomOr::Value(v) => format!("{v}"),
        }
    }
}

impl Serialize for RomOr {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            RomOr::Rom => s.serialize_str("rom"),
            RomOr::Value(v) => s.serialize_f64(*v),
        }
    }
}

impl<'de> Deserialize<'de> for RomOr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            F(f64),
            I(i64),
            S(String),
        }
        match Raw::deserialize(d)? {
            Raw::F(v) => Ok(RomOr::Value(v)),
            Raw::I(v) => Ok(RomOr::Value(v as f64)),
            // an environment override reaches a string slot as text
            Raw::S(t) if t.trim().eq_ignore_ascii_case("rom") => Ok(RomOr::Rom),
            Raw::S(t) => t.trim().parse::<f64>().map(RomOr::Value).map_err(|_| {
                serde::de::Error::custom(format!("expected a number or \"rom\", got {t:?}"))
            }),
        }
    }
}

/// `combat.*`: DSV-30 (damage rule), DSV-34 (shot trace), DSV-39 (blast falloff). The same 17 keys as
/// `dc-dserver -combat key=value`
/// (whose `rewind_ms` and `corpse_ms` are command-line only); the command line wins over this table. Numbers are
/// non-negative; `rom` keys may say `"rom"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CombatSection {
    /// DC-DEFAULT: DSV-30 (proposed). Multiplier of a hit in the top `head_fraction` of the victim's box.
    pub head_factor: f64,
    /// DC-DEFAULT: DSV-30 (proposed).
    pub body_factor: f64,
    /// DC-DEFAULT: DSV-30 (proposed). Fraction of the box height that counts as head (at most 1).
    pub head_fraction: f64,
    /// DC-DEFAULT: DSV-30 (proposed). `rom` = the ROM skeleton node factors, `off` = head/body factors only.
    pub zone_factors: String,
    /// DC-DEFAULT: DSV-30 (proposed). Share of damage armour absorbs; `rom` = the worn armour's share (by damage
    /// type and hit location for an armour row, the grade for armour items).
    pub armor_absorb: RomOr,
    /// project decision: calibrated to the captured 2009 per-hit values (CAPTURE_tdm6 §2.1 / §3, tdm4 §3.1).
    /// What every player wears: `"worn"` (the default, DSV-430: the armour rows of the body, head gear and goggle;
    /// a player without gear wears the calibrated row 0), a ROM armour row number (`"0"` = the calibrated row for
    /// everyone), `"items"` (the loadout's armour items) or `"off"`.
    #[serde(deserialize_with = "number_or_text")]
    pub armor_gear: String,
    /// project decision: calibrated to the captured 2009 per-hit values (CAPTURE_tdm6 §2.1 / §3, tdm4 §3.1).
    /// `base` = the armour pays its share of the ammunition's damage before the zone multiplier, `hit` = of the
    /// zoned damage.
    pub armor_cost: String,
    /// DC-DEFAULT: DSV-30 (proposed).
    pub start_armor: RomOr,
    /// DC-DEFAULT: DSV-30 (proposed).
    pub max_armor: RomOr,
    /// DC-DEFAULT: DSV-30 (proposed).
    pub start_health: f64,
    /// DC-DEFAULT: DSV-30 (proposed).
    pub max_health: f64,
    /// DC-DEFAULT: DSV-30 (proposed). Damage when the ROM has no value for the reported ammunition.
    pub default_damage: f64,
    /// DC-DEFAULT: DC-C2S-11 (proposed). Hit point tolerance around the victim's box.
    pub hit_tolerance: f64,
    /// DC-DEFAULT: DC-C2S-11 (proposed). Shot origin tolerance around the shooter.
    pub origin_tolerance: f64,
    /// DC-DEFAULT: DSV-34 (proposed). `on` = refuse hits through walls.
    pub trace: String,
    /// DC-DEFAULT: DSV-34 (proposed). Units a wall may sit before the hit point.
    pub trace_slack: f64,
    /// DC-DEFAULT: DSV-39 (amendment, proposed; PLAUSIBLE). Share of a blast's radius with full damage (0..1);
    /// linear to 0 at the radius beyond it; 0 = linear from the centre.
    pub blast_inner: f64,
}

impl Default for CombatSection {
    fn default() -> Self {
        CombatSection {
            head_factor: 2.0,
            body_factor: 1.0,
            head_fraction: 0.2,
            zone_factors: "rom".into(),
            armor_absorb: RomOr::Rom,
            // project decision: calibrated to the captured 2009 per-hit values (CAPTURE_tdm6 §2.1 / §3,
            // tdm4 §3.1)
            // DC-DEFAULT: DSV-430 (proposed) the worn rows (row 0 for a player without gear)
            armor_gear: "worn".into(),
            armor_cost: "base".into(),
            start_armor: RomOr::Rom,
            max_armor: RomOr::Rom,
            start_health: 100.0,
            max_health: 100.0,
            default_damage: 25.0,
            hit_tolerance: 48.0,
            origin_tolerance: 160.0,
            trace: "on".into(),
            trace_slack: 16.0,
            blast_inner: 0.25,
        }
    }
}

/// A text key that may also be written as a TOML integer (`armor_gear = 0` or `armor_gear = "items"`).
fn number_or_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        I(i64),
        S(String),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::I(v) => v.to_string(),
        Raw::S(t) => t,
    })
}

impl CombatSection {
    /// The (key, value text) pairs for `GameTuning::set_combat`, in `COMBAT_KEYS` order.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("head_factor", self.head_factor.to_string()),
            ("body_factor", self.body_factor.to_string()),
            ("head_fraction", self.head_fraction.to_string()),
            ("zone_factors", self.zone_factors.clone()),
            ("armor_absorb", self.armor_absorb.text()),
            ("armor_gear", self.armor_gear.clone()),
            ("armor_cost", self.armor_cost.clone()),
            ("start_armor", self.start_armor.text()),
            ("max_armor", self.max_armor.text()),
            ("start_health", self.start_health.to_string()),
            ("max_health", self.max_health.to_string()),
            ("default_damage", self.default_damage.to_string()),
            ("hit_tolerance", self.hit_tolerance.to_string()),
            ("origin_tolerance", self.origin_tolerance.to_string()),
            ("trace", self.trace.clone()),
            ("trace_slack", self.trace_slack.to_string()),
            ("blast_inner", self.blast_inner.to_string()),
        ]
    }
}

// ---------------------------------------------------------------------------------------------
// [vis]: the game server's visibility culling (dc_dserver::visibility, VIS-1, VIS-7, VIS-8, VIS-12 proposed)
// ---------------------------------------------------------------------------------------------

/// `vis.*`: server-side visibility culling. The same switches as `dc-dserver -visibility on|off`,
/// `-visibility-window <ms>`, `-visibility-radius <units>`; the command line wins over this table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VisSection {
    /// DC-DEFAULT: VIS-1 (proposed). Cull enemies a viewer cannot see (false: a wallhack / x-ray server).
    pub enabled: bool,
    /// DC-DEFAULT: VIS-7 (proposed). A pair stays visible this long after its last clear test.
    pub exposure_ms: u32,
    /// DC-DEFAULT: VIS-12 (proposed). Enemies within this distance are always visible (units).
    pub always_radius: f64,
    /// DC-DEFAULT: VIS-8 (proposed). Segment tests per tick; pairs left untested fail open (visible).
    pub max_segments_per_tick: u32,
}

impl Default for VisSection {
    fn default() -> Self {
        VisSection {
            enabled: true,
            exposure_ms: 1000,
            always_radius: 800.0,
            max_segments_per_tick: 1000,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// [evars]: the game server's server variables before the world load (dc_dserver::evars, DSV-47, DSV-60 proposed)
// ---------------------------------------------------------------------------------------------

/// `evars.*`: the same switches as `dc-dserver -evars world|off` and `-evar <name>=<value>`; the command line
/// wins (`-evars`), and `-evar` variables follow the table's, so a flag replaces a table variable of the same name.
// DC-DEFAULT: DSV-47 / DSV-60 (proposed) world variables and world north on by default; DSV-61 (proposed) any
// other variable only when the operator names it
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EvarsSection {
    /// "world" = the world-settings variables and world north; "off" = none (the step height stays).
    pub mode: String,
    /// Extra variables `name = "value"` sent after the built-in ones (a same-named variable is replaced).
    pub extra: BTreeMap<String, String>,
}

impl Default for EvarsSection {
    fn default() -> Self {
        EvarsSection {
            mode: "world".into(),
            extra: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// [hisec]: the lobby side of the HiSec case (dc-lobby-server hisec.rs, HS-6..HS-9 proposed)
// ---------------------------------------------------------------------------------------------

/// `hisec.*`: lobby keys of the HiSec case (the dserver's drop / reservation / pickup keys are its `-hisec` flag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HisecSection {
    /// DC-DEFAULT: HS-6 (proposed). Product a picked-up case becomes; 0 = from the ROM tables.
    pub case_product: u32,
    /// DC-DEFAULT: HS-9 (proposed). Who hears a HiSec opening (0x3013): "all", "channel" or "off".
    pub announce: String,
    /// DC-DEFAULT: HS-7 (proposed). 0x32f reason when no usable key is owned.
    pub no_key_reason: u32,
}

impl Default for HisecSection {
    fn default() -> Self {
        HisecSection {
            case_product: 0,
            announce: "all".into(),
            no_key_reason: 0x99,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// [mail]: the lobby's password-recovery mail (dc-lobby-server mail.rs, LG-240..LG-245 proposed)
// ---------------------------------------------------------------------------------------------

/// `mail.*`: the SMTP account the lobby sends recovery codes from. Without `smtp_host` (and without `dry_run`)
/// recovery is off. Put the password in `DC_MAIL__PASSWORD` rather than the file; it is never printed.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MailSection {
    /// DC-DEFAULT: LG-245 (proposed). SMTP server name; empty = recovery off.
    pub smtp_host: String,
    /// DC-DEFAULT: LG-245 (proposed). 587 = STARTTLS (required), 465 = TLS from the first byte.
    pub smtp_port: u16,
    /// DC-DEFAULT: LG-245 (proposed). SMTP login; empty = no login.
    pub user: String,
    /// DC-DEFAULT: LG-245 (proposed). SMTP password (prefer `DC_MAIL__PASSWORD`).
    pub password: String,
    /// DC-DEFAULT: LG-245 (proposed). Sender address of the mails.
    pub from: String,
    /// DC-DEFAULT: LG-245 (proposed). Text in front of every subject.
    pub subject_prefix: String,
    /// DC-DEFAULT: LG-245 (proposed). Write the mails to the log instead of sending them (tests, local play).
    pub dry_run: bool,
    /// DC-DEFAULT: LG-316 (proposed). The operator's address for the in-game reports; empty = reports stay in the DB only.
    pub reports_to: String,
    /// DC-DEFAULT: LG-316 (proposed). At most this many report mails per hour for the whole lobby; the rest wait in the DB.
    pub reports_per_hour: u32,
}

impl Default for MailSection {
    fn default() -> Self {
        MailSection {
            smtp_host: String::new(),
            smtp_port: 587,
            user: String::new(),
            password: String::new(),
            from: String::new(),
            subject_prefix: "[DeCombat]".into(),
            dry_run: false,
            reports_to: String::new(),
            reports_per_hour: 20,
        }
    }
}

/// The password is never shown, not even in a debug print.
impl std::fmt::Debug for MailSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailSection")
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("user", &self.user)
            .field(
                "password",
                &if self.password.is_empty() { "" } else { "***" },
            )
            .field("from", &self.from)
            .field("subject_prefix", &self.subject_prefix)
            .field("dry_run", &self.dry_run)
            .field("reports_to", &self.reports_to)
            .field("reports_per_hour", &self.reports_per_hour)
            .finish()
    }
}

// ---------------------------------------------------------------------------------------------
// [beta]: the beta signup by e-mail (dc-lobby-server beta.rs, LG-370..LG-379 proposed, project decision)
// ---------------------------------------------------------------------------------------------

/// `beta.*`: the public signup endpoint on the web port (`POST /beta/signup`, `GET /beta/confirm`, `GET /beta/unsubscribe`),
/// the invite and feedback mails sent from the operator dashboard or `dc-lobby-server beta`. Off by default; it needs
/// the `[mail]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BetaSection {
    /// DC-DEFAULT: LG-370 (proposed). The signup endpoints answer (404 when false).
    pub enabled: bool,
    /// DC-DEFAULT: LG-370 (proposed). The one web origin whose pages may call the signup (CORS), no trailing slash.
    pub site_origin: String,
    /// DC-DEFAULT: LG-370 (proposed). Where the lobby's web port is reached from outside (the links in the mails), no trailing slash.
    pub public_url: String,
    /// DC-DEFAULT: LG-374 (proposed). Where the invite mail sends people to download the client.
    pub download_url: String,
    /// DC-DEFAULT: LG-374 (proposed). Reply-To of the beta mails (one address); empty = none.
    pub reply_to: String,
    /// DC-DEFAULT: LG-373 (proposed). Beta mails in any 24 hours, all kinds together (Resend's free tier is 100 a day, shared with the other mail).
    pub max_mail_per_day: u32,
    /// DC-DEFAULT: LG-373 (proposed). Of those, kept free for the confirmation mails: an operator batch may not use them.
    pub confirm_reserve: u32,
    /// DC-DEFAULT: LG-374 (proposed). Days an invite code mailed from the beta list stays valid.
    pub invite_days: u32,
    /// DC-DEFAULT: LG-371 (proposed). Take the client address from `CF-Connecting-IP` (only behind Cloudflare's proxy).
    pub trust_cf_header: bool,
    /// DC-DEFAULT: LG-371 (proposed). Signup requests per client address per hour.
    pub ip_per_hour: u32,
    /// DC-DEFAULT: LG-371 (proposed). Signup requests per hour in all.
    pub global_per_hour: u32,
}

impl Default for BetaSection {
    fn default() -> Self {
        BetaSection {
            enabled: false,
            site_origin: "https://decombat.com".into(),
            public_url: "https://beta.decombat.com".into(),
            download_url: "https://decombat.com".into(),
            reply_to: String::new(),
            max_mail_per_day: 90,
            confirm_reserve: 10,
            invite_days: 14,
            trust_cf_header: false,
            ip_per_hour: 5,
            global_per_hour: 60,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// [admin_web]: the operator pages on a public name, hardware security keys only (dc-lobby-server adminweb.rs,
// LG-382..LG-387 proposed, USER REQUEST 2026-11-03)
// ---------------------------------------------------------------------------------------------

/// `admin_web.*`: a second loopback listener for the operator dashboard and the beta list, reached through a Cloudflare
/// Tunnel at `public_origin`. The only way in is a WebAuthn assertion of an enrolled hardware security key; a key is
/// enrolled with a one-time link from `dc-lobby-server admin-web enroll`. Off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminWebSection {
    /// DC-DEFAULT: LG-382 (proposed). Open the listener (nothing listens while false).
    pub enabled: bool,
    /// DC-DEFAULT: LG-382 (proposed). `ADDR:PORT` of the listener; a loopback address only (the tunnel connects locally).
    pub bind: String,
    /// DC-DEFAULT: LG-382 (proposed). The origin browsers see: `https://` and the host, no path, no trailing slash.
    pub public_origin: String,
    /// DC-DEFAULT: LG-383 (proposed). The WebAuthn relying-party id: the origin's host or a parent domain of it.
    pub rp_id: String,
    /// DC-DEFAULT: LG-385 (proposed). Minutes without a request that end a session.
    pub idle_minutes: u32,
    /// DC-DEFAULT: LG-385 (proposed). Hours after the sign-in that end a session in any case.
    pub session_hours: u32,
    /// DC-DEFAULT: LG-384 (proposed). Most security keys enrolled at once.
    pub max_keys: u32,
    /// DC-DEFAULT: LG-386 (proposed). Sign-in and enrolment attempts per client address in 10 minutes.
    pub attempts_per_ip: u32,
    /// DC-DEFAULT: LG-386 (proposed). Sign-in and enrolment attempts in 10 minutes in all.
    pub attempts_global: u32,
    /// DC-DEFAULT: LG-399 (proposed). The name of this lobby in the instance switcher (with `instances` only).
    pub label: String,
    /// DC-DEFAULT: LG-399 (proposed). Further lobby instances on the same box whose dashboards the admin web serves
    /// under `/i/<name>/ops`, behind the same sign-in, through their loopback admin ports.
    pub instances: Vec<AdminWebInstance>,
}

/// One further lobby instance of `admin_web.instances` (LG-399).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AdminWebInstance {
    /// The path part `/i/<name>`: 1..32 of `a-z 0-9 -`, starting with a letter or digit.
    pub name: String,
    /// What the switcher and the page's top bar call it (1..40 characters).
    pub label: String,
    /// The instance's loopback admin port (`DC_ADMIN` in its decombat.env), for example `127.0.0.1:10091`.
    pub admin: String,
    /// Optional: a `KEY=VALUE` file (the instance's decombat.env) whose `DC_ADMIN_TOKEN`, when set, is sent as the
    /// admin token. Read at each request; never shown to the browser.
    pub env_file: String,
}

impl Default for AdminWebSection {
    fn default() -> Self {
        AdminWebSection {
            enabled: false,
            bind: "127.0.0.1:10096".into(),
            public_origin: "https://admin.decombat.com".into(),
            rp_id: "admin.decombat.com".into(),
            idle_minutes: 30,
            session_hours: 12,
            max_keys: 4,
            attempts_per_ip: 10,
            attempts_global: 60,
            label: "This lobby".into(),
            instances: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// [identity]: identity tokens (dc-lobby-server identity.rs, LG-68 / LG-69 proposed, project decision)
// ---------------------------------------------------------------------------------------------

/// `identity.*`: one identity, many ledgers. An issuing lobby (`issue = true`) signs tokens with its Ed25519 key pair;
/// an accepting lobby (`issuer` + `public_key`) lets a token log in instead of a password and keeps its own local
/// account for it. All empty / false = standalone accounts only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IdentitySection {
    /// DC-DEFAULT: LG-68 (proposed). Issue tokens (`POST /identity/token` on the web port); needs `issuer`.
    pub issue: bool,
    /// DC-DEFAULT: LG-68 (proposed). Issuer id: the own one when issuing, else the trusted one (1..64 of
    /// `A-Z a-z 0-9 . _ : -`).
    pub issuer: String,
    /// DC-DEFAULT: LG-68 (proposed). The trusted issuer's public key, base64 of 32 bytes (not a secret).
    pub public_key: String,
    /// DC-DEFAULT: LG-68 (proposed). Lifetime of an issued token in hours.
    pub token_hours: u32,
    /// DC-DEFAULT: LG-68 (proposed). The issuing key pair (PKCS#8 PEM); empty = `identity_key.pem` next to the DB.
    pub key_file: String,
}

impl Default for IdentitySection {
    fn default() -> Self {
        IdentitySection {
            issue: false,
            issuer: String::new(),
            public_key: String::new(),
            token_hours: 24,
            key_file: String::new(),
        }
    }
}

/// `hosts.*`: the remote game hosts (SPEC: HOST_AGENT.md §2). DC-DEFAULT: CD-88 (proposed). No agent = no host port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct HostsSection {
    /// The TLS port the agents dial; 0 = `server.auth_port + 5`.
    pub port: u16,
    /// The agents allowed to connect. `[[hosts.agents]]` in TOML.
    pub agents: Vec<HostAgentCfg>,
}

impl HostsSection {
    /// The host port after the "0 = auth_port + 5" rule (may exceed 65535; `check` reports that).
    pub fn resolved_port(&self, auth_port: u16) -> u32 {
        if self.port == 0 {
            u32::from(auth_port) + 5
        } else {
            u32::from(self.port)
        }
    }
}

/// One remote game host (`dc-host`). The token is a shared secret: never printed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostAgentCfg {
    /// The agent's name (its hello `name=`, the `host` of a `[[lobby.servers]]` entry).
    pub name: String,
    /// The shared secret of the hello (at least 24 characters).
    pub token: String,
    /// The address advertised for its game servers; "" = the address the agent connected from.
    #[serde(default)]
    pub public_address: String,
}

/// The token is never shown, not even in a debug print.
impl std::fmt::Debug for HostAgentCfg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostAgentCfg")
            .field("name", &self.name)
            .field("token", &if self.token.is_empty() { "" } else { "<set>" })
            .field("public_address", &self.public_address)
            .finish()
    }
}

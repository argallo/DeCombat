//! `dc-dserver`: the DeCombat dedicated game server (slices 1 to 4).
//!
//! Written from `rust/specs/DSERVER_PIPE.md` (control channel), `UDP_SESSION_2009_A.md` (start-up, the "First
//! vertical slice" S1..S13), `UDP_SESSION_2009_B.md` (object records, object stream, player update),
//! `GAME_MESSAGES_2009_S2C.md` / `GAME_MESSAGES_2009_C2S.md` (game messages, the TDM walkthrough of Appendix A),
//! `MOVEMENT_2009.md` §12 (player update trailer, teleport) and `UDP_TRANSPORT_2009.md` through `dc-net-engine`.
//! One process serves one room: it binds one UDP port, loads the room's world and game tables from the user's ROM
//! at run time, takes each client from the join to its own object in the world (S1-S11), ends its loading screen
//! with the player state (S12), spawns it at a spawn point of its team and runs one Team Deathmatch round: the
//! other players' movement in the per-tick object stream, client-reported hits, kills, respawns, the round end by
//! goal or timer and the result messages, then reports `@player` / `@result` to the lobby.
//!
//! - [`session::Server`]: the sans-IO, deterministic state machine (testable with injected time).
//! - [`game`]: the TDM rules (sans-IO; called by the session).
//! - [`runner`]: socket, stdin thread and clock around it; [`cli`]: launch arguments.
//! - [`msg`], [`objects`], [`stream`], [`shell64`]: the session messages and the object stream.
//! - [`gs`] (server -> client) and [`up`] (client -> server): the game messages.
//! - [`world`]: the room's world (box, spawn points by `dc-world` role, the shot-trace geometry, objective zones,
//!   flags and brushes); [`rom_tables`]: weapon, item, team and attachment tables; [`chars`]: character rows, hit
//!   nodes and body model rigs; [`modelobj`]: model-object records; [`worldobj`]: the world-file objects.
//! - [`pipe`]: control lines in, event lines out.
//! - [`cone`]: the 2009 shot cone (random table, half-width, pellet direction) and the shot-direction check (DSV-102).
//! - [`synth_client`]: a synthetic client that follows the same specs from the client side (test tooling).
//!
//! Slice 3: every player is a character object in the create form the reference stack used (kind-1 model object with
//! the type-0x18 shell blob, body / child / face models and the weapon model attached, resolved from the user's
//! ROM at run time, [`chars`], [`modelobj`]), animated in the stream from the reported control word; the own body
//! comes with the mover update; the start-up order follows the reference stack by default ([`game::StartOrder`]); a
//! `server.toml` layer for stand-alone runs ([`cli::parse_with_env`]).
//!
//! The relays the 2009 client accepted in the two-player capture (weapon sounds and jumps as effect type 0x19,
//! shot effects as type 0x06, the hand model on a weapon change), the sprint permission echo and a view-angle
//! repair for a runaway yaw are sent.
//!
//! Slice 4 (game rules as the 2009 client and the captures require): the client asks for its respawn after its own
//! wait and the server answers (no server timer where the client asks; DSV-32), death states 3 then 2, the kill
//! feed / team kills / damage forms as observed (0x6d, 0xb5, 0x74; never 0x4a; DSV-33), the damage rule from ROM
//! values (InstDamage x the ROM skeleton node factor, armour from the loadout's armour items; DSV-30), a world trace
//! of every reported shot (DSV-34), the round end by goal, timer or an emptied team with the reference stack's end timing
//! and no server bye (DSV-31, DSV-35), start and join fidelity (0x9d, 0x45 to the round's first spawn, roster kinds;
//! DSV-36), the world file's zones and flags (DSV-37), and the lobby's `loadout=slot:item` tokens (DSV-38).
//!
//! Not yet: kill volumes and ladders (kind-9 volume records), sprites and effect objects of the world file, door
//! and breakable state, thrown projectiles, death and direction-specific animations, spectator data, rules of the
//! other modes, mods.

pub mod anims;
pub mod bomblook;
pub mod camshot;
pub mod chars;
pub mod cli;
pub mod clientvars;
pub mod cone;
pub mod coop_rom;
pub mod destructibles;
pub mod doors;
pub mod evars;
pub mod game;
pub mod gs;
pub mod killvol;
pub mod linkwatch;
pub mod modelobj;
pub mod msg;
pub mod objects;
pub mod pipe;
pub mod rom_tables;
pub mod runner;
pub mod script;
pub mod session;
pub mod shell64;
pub mod sounds;
pub mod stream;
pub mod surface;
pub mod synth_client;
pub mod up;
pub mod visibility;
pub mod world;
pub mod worldlight;
pub mod worldobj;

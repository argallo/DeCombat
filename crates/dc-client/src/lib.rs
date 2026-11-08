//! `dc-client`: the first DeCombat client slice, a first-person world walker.
//!
//! One world of the user's ROM (or a synthetic world) is loaded once: the collision set for `dc-sim`, the
//! render scene for `dc-render`, the spawn points and the standing box. Every rendered frame then
//! 1. feeds the frame's look input (mouse) into the view angles ([`look`]),
//! 2. advances `dc-sim`'s [`dc_sim::FixedStep`] by the wall time and runs the due ticks, each with the
//!    commands of that tick ([`input::InputSource`]),
//! 3. interpolates the eye between the last two simulated states ([`player::LocalPlayer`]),
//! 4. draws `dc-render`'s world passes from that eye and a small debug overlay ([`overlay`]).
//!
//! The same loop runs headless ([`headless`], scripted input, PNG frames) and in a window ([`window`],
//! feature `window`). The split between [`input::InputSource`] (who decides the commands) and
//! [`world::WorldSource`] (who decides the world, the start point and the server's corrections) is the seam
//! where the network session plugs in: [`net::NetWorld`] joins a lobby and a game server (dc-botclient as a
//! library), loads the announced world, sends the local player's 0x64 update ([`update`]) and hands back the
//! server's corrections, player state, remote players (drawn as boxes, [`debug_draw`]) and status lines.
//!
//! ROM model: worlds, textures, models and fonts are read from the user's client folder at run time; nothing
//! of it is stored here, and images this crate writes are local files that are never committed.
// SPEC: MOVEMENT_2009.md §5-§7 (control word, velocity law), §13 (camera); RENDER_2009.md §3-§5 (world passes)

#![forbid(unsafe_code)]

/// CLI-1944: the build stamp `tools/friend_pack_win.py` finds in the exe
/// (`DCSTAMP1;commit=<sha|unknown>;version=<tag|dev>;target=<triple>;profile=<debug|release>;end`), written by
/// build.rs from `DC_GIT_COMMIT` / `DC_RELEASE_VERSION`; kept in the binary and shown in the `run` log event.
#[used]
pub static BUILD_STAMP: &str = env!("DC_BUILD_STAMP");

pub mod aim;
pub mod anims;
pub mod arms;
pub mod attach;
pub mod camo;
pub mod camfx;
pub mod camshot;
pub mod characters;
pub mod cone;
pub mod deathcam;
pub mod diag;
pub mod debug_draw;
pub mod display;
pub mod drops;
pub mod error;
pub mod events;
pub mod exact;
pub mod frame;
pub mod frameclock;
pub mod frametimes;
pub mod fx;
pub mod fxkeys;
pub mod fxmodels;
pub mod gpulost;
pub mod grabkeep;
pub mod headless;
pub mod heartbeat;
pub mod hitnodes;
pub mod hud;
pub mod community;
pub mod clientver;
pub mod selfupdate;
pub mod packstart;
pub mod identity;
pub mod input;
pub mod inspect;
pub mod knownsets;
#[cfg(all(feature = "window", any(target_os = "ios", test)))]
pub mod ios;
pub mod loader;
pub mod loading;
pub mod look;
pub mod matchexit;
pub mod menu;
pub mod modassets;
pub mod modcatalog;
pub mod mods;
pub mod music;
pub mod net;
pub mod objects;
pub mod offscreen;
pub mod overlay;
pub mod pacer;
pub mod prewarm;
pub mod player;
pub mod prof;
pub mod recoil;
pub mod redrawkeep;
pub mod settings;
pub mod shots;
pub mod sndcache;
pub mod sound;
pub mod spread;
pub mod sprites;
pub mod startguard;
pub mod vklayers;
pub mod steps;
pub mod stress;
#[doc(hidden)]
pub mod testkit;
pub mod thrown;
pub mod todo;
pub mod update;
pub mod viewanim;
pub mod watchdog;
#[cfg(feature = "window")]
pub mod window;
pub mod world;
pub mod zoom;

pub use arms::{Arsenal, WeaponCommands, WeaponTable};
pub use error::ClientError;
pub use frame::{FpsMeter, ViewSettings};
pub use headless::{
    run_headless, ClockMark, FrameHash, FrameStall, HeadlessOptions, HeadlessReport, HudMark,
    TracePoint,
};
pub use input::{
    Action, AnchoredScript, DeviceInput, InputSource, KeyboardMouse, ModifierKey, MoveScript,
    ScriptedInput, ServerHold, ShootTest, WalkTest, SCRIPT_SECONDS,
};
#[cfg(all(feature = "window", target_os = "ios"))]
pub use ios::ios_main;
pub use look::{LookParams, ViewAngles};
pub use net::{Connect, NetHandle, NetOptions, NetPhase, NetStats, NetWorld, WorldFile};
pub use player::{EyeSmoother, LocalPlayer, TickRecord};
pub use update::LocalUpdate;
pub use world::{
    ArmsFrame, DeathView, FlagMarker, FrameState, LoadedWorld, ModWorld, ModelRef, MoverNote,
    PidSet, RemoteBody, RemoteLook, RemoteTracker, RomWorld, SpawnPoint, SynthSource, ViewWeapon,
    WorldSource,
};

/// DC-DEFAULT: CLI-820 (proposed) a player's `--rom` that is not a 2009 client folder is refused with a message that
/// names the folder and what it lacks, instead of the menu silently falling back to its text-only mode. The folder
/// must exist and hold the 2009 client's `Game` folder (the archives the client reads).
pub fn check_rom_dir(rom: &std::path::Path) -> Result<(), String> {
    if !rom.is_dir() {
        return Err(format!(
            "the ROM folder {} does not exist; give your 2009 Combat Arms client folder with --rom \"<folder>\"",
            rom.display()
        ));
    }
    if !rom.join("Game").is_dir() {
        return Err(format!(
            "{} is not a 2009 Combat Arms client folder (no \"Game\" folder inside); give the folder that holds Engine.exe and Game with --rom \"<folder>\"",
            rom.display()
        ));
    }
    Ok(())
}

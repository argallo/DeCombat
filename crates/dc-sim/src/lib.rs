//! `dc-sim`: the deterministic player movement and collision simulation shared by the DeCombat server and
//! client. Written from `rust/specs/MOVEMENT_2009.md` (movement law, integrator, collision, reconciliation)
//! and `rust/specs/FMT_DAT85.md` (the BSP and blocker data, through `dc-world`).
//!
//! Layout, in the order of the 2009 mover's frame:
//! - [`params`]: the tunables ([`MovementParams`], code defaults of the 2009 client, overridable by name or
//!   through serde) and the ROM-derived per-player modifiers ([`SpeedMods`]);
//! - [`player`]: [`PlayerState::step`] = command sampling (§5), velocity law (§6, §7), containers (§11),
//!   ground state (§10.2), physics step (§8.1); server corrections ([`Correction`], §12.4);
//! - [`integrate`]: the engine integrator (§8.3) with its `dt^2 / 4` displacement and exact friction;
//! - [`shape`], [`collide`], [`motion`]: cylinder shape (§9.2), BSP sweep (§9.6-§9.9), slide loop (§9.5),
//!   stair move (§9.7), move driver (§9.4), ground probe (§10.1);
//! - [`driver`]: the fixed-timestep accumulator ([`FixedStep`]) and [`Simulation`];
//! - [`stamina`]: the sprint gauge ([`Stamina`], drain / refill by elapsed time, cshell+0x95850);
//! - [`volume`]: container volumes (ladders) the body can touch ([`ContainerVolumes`], §11.1);
//! - [`todo`]: one named constant per UNPROVEN item the code depends on;
//! - `from_world` (feature `world`): [`CollisionWorld`] from a parsed `dc_world::World`.
//!
//! Determinism: all arithmetic is IEEE binary32 with a fixed operation order and no fused multiply-add;
//! square root is correctly rounded (hardware with `std`, an integer routine without, bit-identical); `exp`,
//! `sin` and `cos` come from [`fmath`] (binary64 polynomials, rounded once), so the same inputs give the same
//! state bits on every target. [`PlayerState::digest`] hashes the full state for desync checks.
//!
//! The core is `no_std` + `alloc` (build with `--no-default-features`); `std` only selects the hardware
//! square root and error impls, `world` adds the dc-world bridge.
// SPEC: MOVEMENT_2009.md §15 (determinism notes for the shared simulation crate)

#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![forbid(unsafe_code)]

pub mod collide;
pub mod driver;
pub mod fmath;
#[cfg(feature = "world")]
pub mod from_world;
pub mod integrate;
pub mod loadout;
pub mod motion;
pub mod params;
pub mod player;
pub mod shape;
pub mod stamina;
pub mod todo;
pub mod vec3;
pub mod volume;

pub use collide::{
    build_bsp, CollModel, Collider, CollisionWorld, ConvexPoly, EmptyWorld, Hit, BLOCKER_MODEL,
    SURF_NO_STEP, SURF_SOLID,
};
pub use driver::{replay, FixedStep, Simulation, DEFAULT_TICK_HZ};
pub use integrate::{integrate, Branch, IntegrateEnv, Integrated};
pub use loadout::{BodySpeed, FigureSpeed, GearSpeed, Loadout, SpeedTable};
pub use motion::{
    drop_to_floor, ground_probe, slide_move, stair_move, sweep_move, Contact, MoveOut,
};
pub use params::{MovementParams, ParamError, SpeedMods};
pub use player::{
    bits, Commands, Container, Correction, Knockback, MoveMode, PlayerState, StepEvents, TickInput,
};
pub use shape::Cylinder;
pub use stamina::{GaugeMode, Stamina, StaminaRates};
pub use vec3::{Quat, V3};
pub use volume::{ContainerVolumes, ConvexVolume};

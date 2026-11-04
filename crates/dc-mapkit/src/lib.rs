//! dc-mapkit: the DeCombat community map builder (docs/MAPKIT_PLAN.md).
//!
//! A map is a text project folder (`map.json` + `terrain.r16` + own assets under `assets/`, MAPKIT_PLAN.md 5.1)
//! that the CLI, the GUI, Lua generators and AI agents all edit through the same JSON operations ([`ops::Op`]);
//! the version-85 world file under `build/` is an output. This crate holds the data side of the tool:
//!
//! - [`project`]: the serde-typed project (`format: 1`), the heightmap file, load / save by temp file + rename;
//! - [`ops`]: the `Op` enum, all-or-nothing [`ops::apply`], seeded terrain generation and brushes;
//! - [`schema`]: the hand-maintained field documentation `dc-mapkit schema` prints (a test keeps it in step with
//!   serde);
//! - [`diag`]: diagnostics (code, severity, path, message, fix hint) and the exit-code rule;
//! - [`geom`]: the intermediate geometry (`V3`, `Plane`, `Poly`, `Brush`, `Level`) and the generators
//!   (solids, terrain, OBJ) the build pipeline of the next stage consumes;
//! - [`template`]: `flat`, `arena`, `starter` (MAPKIT_PLAN.md 5.1 template contract, own textures only);
//! - [`validate`]: the project-level checks of `dc-mapkit validate` (no `.dat` written, offline);
//! - [`history`]: the shared undo (`.mapkit/history/<n>/`);
//! - [`cli`]: the verbs (every one with `--json`; exit 0 ok, 1 diagnostics, 2 usage).
//!
//! The verbs `build` ([`build`], [`bake`], [`worldgen`], [`bsp`]), `export` ([`export`]), `render`
//! ([`render_sw`]), `rom` ([`romlist`], [`thumbs`]), `run` ([`script`]), `mcp` ([`mcp`]), `gui` ([`gui`]: the
//! eframe editor behind the `gui` feature, the second binary `dc-mapkit-gui`) and `play` ([`play`]: export, then a
//! modded lobby + game server + client on this machine, MAP-67 .. MAP-69) are all implemented.
//!
//! Determinism (MAPKIT_PLAN.md D8, section 7): every map is a `BTreeMap`, every random value comes from the
//! project's `seed` through [`ops::noise`], and the geometry path calls no libm transcendental (rotations use
//! [`geom::sincos_deg`], cylinders store their unit circle in the project), so the same project and tool version
//! produce the same bytes on every platform.
//!
//! CLEANROOM: this crate never carries a ROM name; the object vocabulary is DeCombat's own ([`vocab_names`]).

pub mod bake;
pub mod bsp;
pub mod build;
pub mod cli;
pub mod diag;
pub mod export;
pub mod geom;
pub mod glb;
pub mod gui;
pub mod history;
pub mod lightmap;
pub mod mcp;
pub mod ops;
pub mod play;
pub mod project;
pub mod render_gpu;
pub mod render_sw;
pub mod romlist;
pub mod schema;
pub mod script;
pub mod template;
pub mod thumbs;
pub mod validate;
pub mod vocab_names;
pub mod worldgen;

/// Tool version written by `info` and into scaffolded files; same version + same project = same build bytes.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

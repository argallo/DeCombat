//! `dc-gunkit`: a small camo studio. Pick a gun, spin it with the mouse, and change its skin from the window or live from an
//! agent over MCP. It draws with the game's renderer and paints with the game's recipe, so what it shows is what the game shows.
pub mod commands;
pub mod design;
#[cfg(feature = "gui")]
pub mod gui;
pub mod guns;
pub mod lens;
pub mod live;
pub mod mcp;
pub mod pack;
pub mod patterns;
pub mod rom;
pub mod shape;
pub mod studio;
pub mod whole;

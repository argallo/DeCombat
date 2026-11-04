//! iPadOS (CLI-1604): the client as an iPad app played with a Bluetooth keyboard and mouse. UIKit gives the app one
//! event loop, entered once and never left, so the front end, the loading page and every match run as stages of one
//! application handler ([`crate::menu::window`]'s iPad driver) instead of the desktop's event-loop turns; the
//! keyboard and the mouse come from GameController (dc-ios-hid) through [`bridge`]; the start settings come from
//! `Documents/decombat.toml` ([`config`]) instead of a command line (`ios_main`).
//!
//! The bridge and the config reader are plain code and are unit-tested on the Mac; the driver and the entry are
//! built for iOS only.

pub mod bridge;
pub mod config;
#[cfg(target_os = "ios")]
mod entry;

#[cfg(target_os = "ios")]
pub use entry::ios_main;

/// CLI-1604: the ROM cursor image's size, set by the iPad driver from the menu's catalog so the match can draw a
/// software cursor while the mouse is free (`None`: the ROM has none).
static CURSOR_SIZE: std::sync::OnceLock<Option<[u32; 2]>> = std::sync::OnceLock::new();

pub fn set_cursor_size(size: Option<[u32; 2]>) {
    let _ = CURSOR_SIZE.set(size);
}

pub fn cursor_size() -> Option<[u32; 2]> {
    CURSOR_SIZE.get().copied().flatten()
}

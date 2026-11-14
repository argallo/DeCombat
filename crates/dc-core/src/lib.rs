//! Engine-layer basics that every reader shares and that need no game data: FIPS 180-4 SHA-256, the read journal
//! that names an exact-lane run's inputs by hash, and the read trace that lists what a process opened. They lived in
//! dc-rom; dc-rom re-exports them, so `dc_rom::sha256_hex`, `dc_rom::journal` and `dc_rom::trace` keep working and
//! there is one journal and one trace per process either way.
// Provenance: ORIGINAL (our own code; no 2009 behaviour)

pub mod journal;
mod sha256;
pub mod trace;

pub use sha256::{sha256, sha256_hex, Sha256};

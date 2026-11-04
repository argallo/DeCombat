//! Parsers for the sprite, texture-script and effect data of the 2009 client, written from
//! `rust/specs/FMT_SPR_LTC_FX.md`. No ROM data lives here: effect type names are stored as sha-ids only.
//!
//! | file | module | tier |
//! |---|---|---|
//! | `.spr` | [`spr`] | live (engine reads it) |
//! | `.tfs` / `.tfg` | [`tfs`] / [`tfg`] | live (engine reads them) |
//! | `.fxf` | [`fxf`] + [`registry`] | live (shell reads it) |
//! | `.fcf` | [`fcf`] | DROP (documented, parsed for tools) |
//! | `.ltc` `.lta` `.wst` `.pk` | [`unsupported`] | DROP (typed error) |

pub mod error;
pub mod fcf;
pub mod fxf;
pub mod registry;
pub mod spr;
mod text;
pub mod tfg;
pub mod tfs;
pub mod unsupported;

pub use error::FxError;
pub use fcf::FcfFile;
pub use fxf::{Encoding, FieldValue, FxfFile, PropType};
pub use registry::FxType;
pub use spr::{Sprite, SpriteCursor};
pub use tfg::TextureGroup;
pub use tfs::TextureScript;
pub use unsupported::{reject_dropped, DroppedFamily};

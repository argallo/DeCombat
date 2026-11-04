//! Families the 2009 runtime never opens (tier DROP). SPEC: FMT_SPR_LTC_FX.md §8, §9.1 to §9.3.
//!
//! `.ltc` compressed sources, `.lta` text model sources, `.wst` weight sets and `.pk` peak caches have no
//! loader in the client. A reimplementation only needs to tolerate their presence in archives, so every
//! attempt to load one returns the typed [`FxError::Unsupported`].

use crate::error::FxError;

/// A dropped family, recognised by file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DroppedFamily {
    Ltc,
    Lta,
    Wst,
    Pk,
}

impl DroppedFamily {
    pub fn from_name(name: &str) -> Option<DroppedFamily> {
        let ext = name.rsplit_once('.')?.1;
        [
            ("ltc", DroppedFamily::Ltc),
            ("lta", DroppedFamily::Lta),
            ("wst", DroppedFamily::Wst),
            ("pk", DroppedFamily::Pk),
        ]
        .into_iter()
        .find(|(e, _)| ext.eq_ignore_ascii_case(e))
        .map(|(_, f)| f)
    }
    pub fn label(self) -> &'static str {
        match self {
            DroppedFamily::Ltc => "ltc compressed source",
            DroppedFamily::Lta => "lta text model source",
            DroppedFamily::Wst => "wst weight set",
            DroppedFamily::Pk => "pk peak cache",
        }
    }
}

/// Fails with [`FxError::Unsupported`] for a dropped family; `Ok(())` means "not a dropped family, use the
/// real parser for this name".
pub fn reject_dropped(name: &str, _bytes: &[u8]) -> Result<(), FxError> {
    match DroppedFamily::from_name(name) {
        Some(f) => Err(FxError::Unsupported { family: f.label() }),
        None => Ok(()),
    }
}

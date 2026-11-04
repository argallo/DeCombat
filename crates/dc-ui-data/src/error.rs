//! Typed errors of the UI data readers.

use std::fmt;

/// Everything that can go wrong while reading the UI data out of the user's client folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiDataError {
    /// The PE layer failed (malformed image, rva out of range, missing file).
    Rom(dc_rom::RomError),
    /// The archive layer failed (unreadable archive, bad key).
    Rez(dc_rez::RezError),
    /// The PE resource directory is malformed; `what` names the structure.
    BadResource { what: &'static str, offset: u64 },
    /// A literal read from the user's CShell.dll does not hash to the pinned digest (not the 2009 build).
    LiteralMismatch { what: &'static str, rva: u32 },
    /// A required table entry is absent from the archives.
    MissingEntry { what: &'static str },
    /// An input structure is malformed (message says where).
    Malformed(String),
}

impl fmt::Display for UiDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiDataError::Rom(e) => write!(f, "rom: {e}"),
            UiDataError::Rez(e) => write!(f, "archive: {e}"),
            UiDataError::BadResource { what, offset } => {
                write!(
                    f,
                    "malformed resource directory ({what}) at offset {offset:#x}"
                )
            }
            UiDataError::LiteralMismatch { what, rva } => write!(
                f,
                "literal {what} at cshell+{rva:#x} does not match the pinned 2009 digest"
            ),
            UiDataError::MissingEntry { what } => write!(f, "missing table entry: {what}"),
            UiDataError::Malformed(m) => write!(f, "malformed input: {m}"),
        }
    }
}

impl std::error::Error for UiDataError {}

impl From<dc_rom::RomError> for UiDataError {
    fn from(e: dc_rom::RomError) -> Self {
        UiDataError::Rom(e)
    }
}

impl From<dc_rez::RezError> for UiDataError {
    fn from(e: dc_rez::RezError) -> Self {
        UiDataError::Rez(e)
    }
}

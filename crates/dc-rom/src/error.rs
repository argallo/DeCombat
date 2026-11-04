//! Typed errors for ROM access, PE parsing and table extraction.

use std::fmt;
use std::path::{Path, PathBuf};

/// Everything that can go wrong while locating, identifying or decoding the user-supplied ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RomError {
    /// `CA_ROM` is unset/empty and no explicit path was supplied.
    NotConfigured,
    /// The ROM path does not exist or is not a directory.
    NotADirectory(PathBuf),
    /// An I/O operation failed.
    Io { path: PathBuf, message: String },
    /// The PE container is malformed; `what` names the structure that failed.
    MalformedPe { what: &'static str, offset: u64 },
    /// An RVA/VA range does not lie inside any section (or the file is truncated there).
    RvaOutOfRange { rva: u64, len: u64 },
    /// The code/data a table extractor decodes is not the pinned 2009 build.
    TableHashMismatch {
        table: &'static str,
        expected: &'static str,
        actual: String,
    },
    /// The table bytes hash correctly but decoding failed (should be impossible for the pinned build).
    TableDecode {
        table: &'static str,
        message: String,
    },
    /// The located secret did not match the pinned digest, or could not be found.
    KeyUnavailable(String),
    /// `identify_rom` found no known build; lists the relative paths that did not match any build.
    UnknownBuild { mismatched: Vec<String> },
    /// `guard_pristine` saw a file differ from the recorded baseline.
    RomChanged { changed: Vec<String> },
    /// Baseline/state directory problems (inside the ROM, unreadable, corrupt).
    BadStateDir(String),
}

impl RomError {
    pub(crate) fn io(path: &Path, e: &std::io::Error) -> Self {
        RomError::Io {
            path: path.to_path_buf(),
            message: e.to_string(),
        }
    }
    pub(crate) fn pe(what: &'static str, offset: u64) -> Self {
        RomError::MalformedPe { what, offset }
    }
    pub(crate) fn decode(table: &'static str, message: impl Into<String>) -> Self {
        RomError::TableDecode {
            table,
            message: message.into(),
        }
    }
}

impl fmt::Display for RomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RomError::NotConfigured => write!(f, "CA_ROM is not set and no ROM path was given"),
            RomError::NotADirectory(p) => write!(f, "ROM path {} is not a directory", p.display()),
            RomError::Io { path, message } => write!(f, "I/O error on {}: {}", path.display(), message),
            RomError::MalformedPe { what, offset } => {
                write!(f, "malformed PE ({what}) at offset {offset:#x}")
            }
            RomError::RvaOutOfRange { rva, len } => {
                write!(f, "RVA range {rva:#x}+{len:#x} is outside every PE section")
            }
            RomError::TableHashMismatch {
                table,
                expected,
                actual,
            } => write!(
                f,
                "table '{table}': code/data hash {actual} does not match pinned {expected}; this is not the supported 2009 build"
            ),
            RomError::TableDecode { table, message } => {
                write!(f, "table '{table}': decode failed: {message}")
            }
            RomError::KeyUnavailable(m) => write!(f, "crypt key unavailable: {m}"),
            RomError::UnknownBuild { mismatched } => write!(
                f,
                "ROM matches no known build; differing files: {}",
                mismatched.join(", ")
            ),
            RomError::RomChanged { changed } => write!(
                f,
                "PRISTINE INSTALL CHANGED, refusing to run: {}",
                changed.join(", ")
            ),
            RomError::BadStateDir(m) => write!(f, "bad state directory: {m}"),
        }
    }
}

impl std::error::Error for RomError {}

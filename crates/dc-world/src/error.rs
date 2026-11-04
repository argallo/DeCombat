//! Typed parse errors. Every failure carries the file position where it was detected.

use std::fmt;

/// The part of the file being read when an error occurred.
// SPEC: FMT_DAT85.md §3 (header offsets name the sections)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Header,
    WorldBody,
    Quadtree,
    Bsp,
    Objects,
    BlindData,
    LightGrid,
    Blockers,
    ParticleBlockers,
    Render,
    LightGroupGrids,
}

impl fmt::Display for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Section::Header => "header",
            Section::WorldBody => "world body",
            Section::Quadtree => "quadtree",
            Section::Bsp => "bsp model",
            Section::Objects => "object list",
            Section::BlindData => "blind data",
            Section::LightGrid => "light grid",
            Section::Blockers => "blocker polygons",
            Section::ParticleBlockers => "particle-blocker polygons",
            Section::Render => "render data",
            Section::LightGroupGrids => "light-group grids",
        };
        f.write_str(s)
    }
}

/// Everything that can go wrong while reading a world file.
// SPEC: FMT_DAT85.md §12 (engine rejection rules reproduced as errors)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldError {
    /// `need` more bytes were required at `at` but the file ends first.
    Truncated {
        section: Section,
        at: usize,
        need: usize,
    },
    /// A count times its element size does not fit the address space.
    CountOverflow {
        section: Section,
        at: usize,
        count: u64,
    },
    /// Header `version` is not 85.
    // SPEC: FMT_DAT85.md §3 (check at engine+0xce905)
    BadVersion(u32),
    /// A header offset points outside the file or into the header.
    BadOffset {
        section: Section,
        offset: u32,
        file_len: usize,
    },
    /// A BSP body has portals (the engine rejects them).
    // SPEC: FMT_DAT85.md §5.2 item 6 (check engine+0x33155)
    PortalsPresent { model: u32, count: u32 },
    /// An index field is out of its table.
    IndexOutOfRange {
        section: Section,
        what: &'static str,
        at: usize,
        index: i64,
        limit: u64,
    },
    /// The texture-name block does not split into the declared names.
    // SPEC: FMT_DAT85.md §5.2 item 18
    TextureNames { model: u32, at: usize },
    /// A BSP body ends with a nonzero trailing count.
    // SPEC: FMT_DAT85.md §5.2 item 27 (check engine+0x3369f)
    TrailingCount { model: u32, value: u32 },
    /// The quadtree shape stream does not describe `declared` nodes.
    // SPEC: FMT_DAT85.md §4.1
    QuadtreeShape { declared: u32, parsed: u64 },
    /// An object's `obj_len` disagrees with the bytes its fields used.
    // SPEC: FMT_DAT85.md §6 (obj_len, OBSERVED exact)
    ObjectLength {
        object: u32,
        declared: u16,
        actual: usize,
    },
    /// The run-length packed light grid does not fill its grid exactly.
    // SPEC: FMT_DAT85.md §8
    LightGridPacking {
        at: usize,
        expected_samples: u64,
        produced: u64,
    },
    /// Render models nest deeper than this reader allows (our safety limit, not an engine rule).
    NestingTooDeep { at: usize, depth: u32 },
    /// The sections do not tile the file (only from [`crate::World::check_layout`]).
    // SPEC: FMT_DAT85.md §3 (OBSERVED: each section ends where the next begins)
    Layout {
        section: Section,
        end: usize,
        expected: usize,
    },
}

impl fmt::Display for WorldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorldError::Truncated { section, at, need } => {
                write!(f, "{section}: truncated at {at:#x} (need {need} bytes)")
            }
            WorldError::CountOverflow { section, at, count } => {
                write!(f, "{section}: count {count} at {at:#x} overflows")
            }
            WorldError::BadVersion(v) => write!(f, "header: version {v}, expected 85"),
            WorldError::BadOffset {
                section,
                offset,
                file_len,
            } => {
                write!(
                    f,
                    "header: {section} offset {offset:#x} invalid for file of {file_len} bytes"
                )
            }
            WorldError::PortalsPresent { model, count } => {
                write!(f, "bsp model {model}: {count} portals (must be 0)")
            }
            WorldError::IndexOutOfRange {
                section,
                what,
                at,
                index,
                limit,
            } => {
                write!(
                    f,
                    "{section}: {what} {index} at {at:#x} out of range (limit {limit})"
                )
            }
            WorldError::TextureNames { model, at } => {
                write!(
                    f,
                    "bsp model {model}: texture-name block at {at:#x} malformed"
                )
            }
            WorldError::TrailingCount { model, value } => {
                write!(f, "bsp model {model}: trailing count {value} (must be 0)")
            }
            WorldError::QuadtreeShape { declared, parsed } => {
                write!(
                    f,
                    "quadtree: {declared} nodes declared, shape stream has {parsed}"
                )
            }
            WorldError::ObjectLength {
                object,
                declared,
                actual,
            } => {
                write!(
                    f,
                    "object {object}: length {declared} declared, {actual} used"
                )
            }
            WorldError::LightGridPacking {
                at,
                expected_samples,
                produced,
            } => {
                write!(f, "light grid: packing at {at:#x} gives {produced} samples, grid has {expected_samples}")
            }
            WorldError::NestingTooDeep { at, depth } => {
                write!(
                    f,
                    "render data: nesting depth {depth} at {at:#x} exceeds limit"
                )
            }
            WorldError::Layout {
                section,
                end,
                expected,
            } => {
                write!(
                    f,
                    "{section}: ends at {end:#x}, next section starts at {expected:#x}"
                )
            }
        }
    }
}

impl std::error::Error for WorldError {}

pub type Result<T> = std::result::Result<T, WorldError>;

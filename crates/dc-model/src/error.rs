//! Typed errors of the model reader and writer.

use std::fmt;

/// Everything that can go wrong while reading or writing a model file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// The input ended before a field could be read.
    Truncated { offset: usize, need: usize },
    /// `file_kind` is not 1. Kind 5 is a render-style file, kind 9 a third container type; the engine's model
    /// loader rejects both with its wrong-file-type error.
    // SPEC: FMT_LTB.md §2 (file_kind must be 1; kinds 5 and 9 occur in .ltb names)
    NotAModel { file_kind: u8 },
    /// `container_rev` is not 9.
    // SPEC: FMT_LTB.md §2 (container_rev must be 9)
    BadContainerRev { rev: u16 },
    /// `model_rev` below 23: the engine logs an unsupported-revision message and refuses the file.
    // SPEC: FMT_LTB.md §2 (model_rev must be >= 23)
    UnsupportedRevision { rev: u32 },
    /// A `u16`-prefixed string is longer than the engine's 0x1000-byte read buffer allows.
    // SPEC: FMT_LTB.md preamble (0x1000-byte buffer, engine+0x6db70)
    StringTooLong { offset: usize, len: usize },
    /// A count field is larger than the remaining input could possibly hold.
    CountTooLarge {
        what: &'static str,
        offset: usize,
        count: u64,
    },
    /// More than 64 pieces.
    // SPEC: FMT_LTB.md §4 step 6 (piece_count < 0x41)
    TooManyPieces { count: u32 },
    /// A render-object kind other than 4 (rigid), 5 (skinned) or 6 (morph).
    // SPEC: FMT_LTB.md §5 (ro_kind 4, 5 or 6), §6
    UnknownMeshKind { offset: usize, kind: u32 },
    /// A skinned body's influence count is outside the range its tail reader accepts.
    // SPEC: FMT_LTB.md §6.3 (per-vertex tail: max_influences 1..4; palette tail: palette_influences 2..4)
    BadInfluences { offset: usize, value: u32 },
    /// A skinned body's tail selector is neither 0 nor 1.
    // SPEC: FMT_LTB.md §6.3 (palette_path 0 / 1)
    BadTailSelector { offset: usize, value: u8 },
    /// A blend set does not carry one weight per node.
    // SPEC: FMT_LTB.md §8 (n must equal the node count)
    BlendSetSize {
        set: usize,
        expected: usize,
        got: u32,
    },
    /// The child-model slot count is outside 1..32.
    // SPEC: FMT_LTB.md §4 step 11, §9
    ChildCount { count: u32 },
    /// A clip track codec above 3.
    // SPEC: FMT_LTB.md §10.3 (a codec above 3 should be treated as an error)
    BadCodec { clip: usize, codec: u32 },
    /// An attach point names a node that does not exist.
    // SPEC: FMT_LTB.md §11.1 (node must be < the node count)
    SocketNode {
        socket: usize,
        node: u32,
        node_count: usize,
    },
    /// Arithmetic on a size field overflowed.
    Overflow { offset: usize },
    /// The in-memory model cannot be encoded (inconsistent counts, lengths or encodings).
    Encode { what: &'static str },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ModelError::*;
        match self {
            Truncated { offset, need } => {
                write!(f, "truncated at {offset:#x}: need {need} more bytes")
            }
            NotAModel { file_kind } => write!(f, "not a model file (file kind {file_kind})"),
            BadContainerRev { rev } => write!(f, "unsupported container revision {rev}"),
            UnsupportedRevision { rev } => {
                write!(f, "unsupported model revision {rev} (need >= 23)")
            }
            StringTooLong { offset, len } => {
                write!(f, "string of {len} bytes at {offset:#x} is too long")
            }
            CountTooLarge {
                what,
                offset,
                count,
            } => write!(f, "{what} count {count} at {offset:#x} exceeds input"),
            TooManyPieces { count } => write!(f, "{count} pieces (at most 64)"),
            UnknownMeshKind { offset, kind } => {
                write!(f, "unknown render-object kind {kind} at {offset:#x}")
            }
            BadInfluences { offset, value } => {
                write!(f, "bad influence count {value} at {offset:#x}")
            }
            BadTailSelector { offset, value } => {
                write!(f, "bad skinned tail selector {value} at {offset:#x}")
            }
            BlendSetSize { set, expected, got } => {
                write!(
                    f,
                    "blend set {set} has {got} weights, node count is {expected}"
                )
            }
            ChildCount { count } => write!(f, "child model slot count {count} outside 1..32"),
            BadCodec { clip, codec } => write!(f, "clip {clip} uses unknown track codec {codec}"),
            SocketNode {
                socket,
                node,
                node_count,
            } => {
                write!(f, "attach point {socket} names node {node} of {node_count}")
            }
            Overflow { offset } => write!(f, "size arithmetic overflow at {offset:#x}"),
            Encode { what } => write!(f, "cannot encode model: {what}"),
        }
    }
}

impl std::error::Error for ModelError {}

//! `.fcf` effect editor tree (tier DROP: the runtime never reads it; parsed so tools can show or ignore it).
//! SPEC: FMT_SPR_LTC_FX.md §7.

use crate::error::FxError;
use crate::text::Lines;

/// Our nesting bound (ROM depth is 3).
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct FcfNode {
    pub name: String,
    pub image: i32,
    pub selected_image: i32,
    pub state: i32,
    pub children: Vec<FcfNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FcfWindow {
    pub effect_name: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FcfFile {
    pub roots: Vec<FcfNode>,
    pub windows: Vec<FcfWindow>,
    /// Number of sibling blocks (the root block plus one per node with children).
    pub blocks: usize,
    pub nodes: usize,
    pub trailing_nonblank: bool,
}

impl FcfFile {
    /// Parse a whole `.fcf` entry. SPEC: FMT_SPR_LTC_FX.md §7 grammar.
    pub fn parse(data: &[u8]) -> Result<FcfFile, FxError> {
        if data.is_empty() {
            return Err(FxError::Empty);
        }
        let mut l = Lines::new(data);
        let (mut blocks, mut nodes) = (0usize, 0usize);
        let roots = siblings(&mut l, 0, &mut blocks, &mut nodes)?;
        let m = l.next_line("window count")?.unsigned();
        let mut windows = Vec::new();
        for _ in 0..m {
            let effect_name = l.next_line("window name")?.word();
            let mut v = [0i32; 4];
            for s in &mut v {
                *s = l.next_line("window rect")?.int();
            }
            windows.push(FcfWindow {
                effect_name,
                left: v[0],
                top: v[1],
                right: v[2],
                bottom: v[3],
            });
        }
        Ok(FcfFile {
            roots,
            windows,
            blocks,
            nodes,
            trailing_nonblank: !l.only_blank_left(),
        })
    }
}

fn siblings(
    l: &mut Lines,
    depth: usize,
    blocks: &mut usize,
    nodes: &mut usize,
) -> Result<Vec<FcfNode>, FxError> {
    if depth >= MAX_DEPTH {
        return Err(FxError::TooDeep);
    }
    *blocks += 1;
    let n = l.next_line("sibling count")?.unsigned();
    let mut out = Vec::new();
    for _ in 0..n {
        let name = l.next_line("node name")?.word();
        let image = l.next_line("node image")?.int();
        let selected_image = l.next_line("node selected image")?.int();
        let has_children = l.next_line("node has_children")?.int() != 0;
        let state = l.next_line("node state")?.int();
        *nodes += 1;
        let children = if has_children {
            siblings(l, depth + 1, blocks, nodes)?
        } else {
            Vec::new()
        };
        out.push(FcfNode {
            name,
            image,
            selected_image,
            state,
            children,
        });
    }
    Ok(out)
}

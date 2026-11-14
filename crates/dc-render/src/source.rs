//! The renderer's texture lookup: a [`TextureIndex`] over the caller's archive layer
//! ([`dc_vfs::ArchiveLayer::find_any`]: every entry of every archive by its full relative path). The apps hand in a
//! layer over the user's own ROM; nothing read here is stored in the tree.

use dc_vfs::{ArchiveLayer, Vfs, VfsError};
use std::sync::Arc;

/// Texture names of world sections and model files, looked up by full relative path in every archive of the layer
/// (OBSERVED over the 21 ROM worlds: 1 or 2 names per world resolve to nothing and fall back; RENDER_2009.md /
/// FMT_DTX.md §1: the engine substitutes a default texture, engine+0x19c40). The default index finds nothing.
#[derive(Debug, Default, Clone)]
pub struct TextureIndex {
    layer: Option<Arc<dyn ArchiveLayer>>,
}

impl TextureIndex {
    /// An index over `layer` (for the user's ROM: `dc_rez::RomLayer::indexed`).
    pub fn new(layer: Arc<dyn ArchiveLayer>) -> TextureIndex {
        TextureIndex { layer: Some(layer) }
    }

    /// An index over the archive layer of `vfs` (none: an empty index).
    pub fn from_vfs(vfs: &Vfs) -> TextureIndex {
        TextureIndex {
            layer: vfs.archive_layer(),
        }
    }

    /// Number of indexed entries.
    pub fn len(&self) -> usize {
        self.layer.as_ref().map_or(0, |l| l.any_count())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a texture name as stored in a world section resolves.
    pub fn contains(&self, name: &[u8]) -> bool {
        !name.is_empty() && self.layer.as_ref().is_some_and(|l| l.has_any(name))
    }

    /// Raw bytes of the entry (textures are stored plain; FMT_DTX.md §8 census reads them raw).
    // SPEC: FMT_DTX.md §8 (header-only census over raw entries)
    pub fn read(&self, name: &[u8]) -> Result<Option<Vec<u8>>, VfsError> {
        let Some(layer) = &self.layer else {
            return Ok(None);
        };
        if name.is_empty() {
            return Ok(None);
        }
        let _io = crate::perf::span(crate::perf::Counter::IoNs);
        let out = layer.find_any(name)?;
        if let Some(b) = &out {
            crate::perf::add(crate::perf::Counter::IoBytes, b.len() as u64);
        }
        Ok(out)
    }
}

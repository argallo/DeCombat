//! Multisample anti-aliasing ([`RenderSettings::msaa`](crate::RenderSettings::msaa)): the sample counts an adapter
//! offers, the multisampled colour and depth textures the 3D passes draw into, and the pipeline state they share.
//!
//! How a frame uses it (`RenderSettings::msaa` > 1): every 3D pass (sky, world with models and translucent objects,
//! the client's debug boxes and effect quads) draws into [`MsaaTargets`]' colour and depth with pipelines built for
//! that count ([`multisample_state`]); the LAST of them carries the single-sample scene target as
//! [`FrameTarget::resolve`](crate::FrameTarget::resolve), so wgpu resolves the samples there, once per frame, and the
//! multisampled colour and depth are discarded. The 2D passes after it (HUD, debug panel, gamma copy) stay
//! single-sampled and draw into the resolved target as before. With `msaa` 1 nothing here is created and every pass
//! records exactly what it recorded before (the exact lane of docs/RENDER_DETERMINISM_CONTRACT.md is untouched).
// DC-DEFAULT: CLI-1593 (proposed) multisample anti-aliasing: the 2009 client offered the device's multisample types
// 2x..16x (specs/RENDER_2009.md:68 r_list_msaa, :110 msaa_samples, :1183 console variables msaa_samples /
// accept_msaa default 0); DeCombat renders the 3D passes into a multisampled target resolved before the HUD

use crate::gpu::{adapter_specific_formats, FrameTarget, GpuContext, ZBUF_FORMAT};
use std::fmt;

/// The multisample counts a setting may ask for beyond 1 (wgpu guarantees 1 and 4 for the 8-bit colour formats and
/// Depth32Float; 2 and 8 depend on the adapter).
pub const MSAA_COUNTS: [u32; 3] = [2, 4, 8];

/// The sample counts of [`MSAA_COUNTS`] that `adapter` renders `format` AND the depth format (Depth32Float) with,
/// after 1 (always first, MSAA off). The list a settings page offers. Counts other than the WebGPU-guaranteed 4 are
/// listed only when the adapter has the adapter-specific format features (every [`GpuContext`] on such an adapter
/// requests them): without them a device refuses those counts whatever the adapter's flags say.
// SPEC: RENDER_2009.md r_list_msaa (0xf2760: the multisample types supported for both the back buffer and the depth
// format); DeCombat asks wgpu's per-format feature flags the same question
pub fn supported_msaa(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> Vec<u32> {
    let colour = adapter.get_texture_format_features(format).flags;
    let depth = adapter.get_texture_format_features(ZBUF_FORMAT).flags;
    let specific = adapter_specific_formats(adapter);
    counts_where(|n| {
        (n == 4 || specific) && colour.sample_count_supported(n) && depth.sample_count_supported(n)
    })
}

/// 1, then the counts of [`MSAA_COUNTS`] that `ok` accepts (the shape [`supported_msaa`] returns; a pure function
/// for the unit test).
fn counts_where(ok: impl Fn(u32) -> bool) -> Vec<u32> {
    std::iter::once(1)
        .chain(MSAA_COUNTS.iter().copied().filter(|&n| ok(n)))
        .collect()
}

/// The multisample state of a pipeline that draws into the frame's 3D target with `count` samples. For 1 it is
/// `wgpu::MultisampleState::default()` field for field (the exact-lane pipeline journal reads the same text).
pub fn multisample_state(count: u32) -> wgpu::MultisampleState {
    wgpu::MultisampleState {
        count: count.max(1),
        mask: !0,
        alpha_to_coverage_enabled: false,
    }
}

/// The multisampled colour and depth textures of one size and sample count the 3D passes draw into when MSAA is
/// on. Kept by the frame loop next to its single-sample targets and made again when the size or the count changes
/// ([`MsaaTargets::prepare`]); `None` while MSAA is off, so it costs nothing then.
pub struct MsaaTargets {
    _colour: wgpu::Texture,
    colour_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    size: (u32, u32),
    count: u32,
    format: wgpu::TextureFormat,
}

impl fmt::Debug for MsaaTargets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MsaaTargets")
            .field("size", &self.size)
            .field("count", &self.count)
            .field("format", &self.format)
            .finish()
    }
}

impl MsaaTargets {
    /// Multisampled colour (`format`, the scene target's) and depth (Depth32Float) textures of `size` with `count`
    /// samples; `None` for a count of 1 (nothing to create).
    pub fn new(
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        count: u32,
    ) -> Option<MsaaTargets> {
        if count <= 1 {
            return None;
        }
        let size = (size.0.max(1), size.1.max(1));
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let colour = crate::perf::texture(
            &ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("msaa colour"),
                size: extent,
                mip_level_count: 1,
                sample_count: count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        );
        let depth = crate::perf::texture(
            &ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("msaa depth"),
                size: extent,
                mip_level_count: 1,
                sample_count: count,
                dimension: wgpu::TextureDimension::D2,
                format: ZBUF_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        );
        Some(MsaaTargets {
            colour_view: colour.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            _colour: colour,
            _depth: depth,
            size,
            count,
            format,
        })
    }

    /// Keeps `slot` in step with the frame: targets of `size` / `count` / `format` (made again only when one of them
    /// changed), `None` when `count` is 1. Called once per frame or on resize; a no-op while nothing changed.
    pub fn prepare(
        slot: &mut Option<MsaaTargets>,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        count: u32,
    ) {
        let size = (size.0.max(1), size.1.max(1));
        let fits = |t: &MsaaTargets| t.size == size && t.count == count && t.format == format;
        if count <= 1 {
            *slot = None;
        } else if !slot.as_ref().is_some_and(fits) {
            *slot = MsaaTargets::new(ctx, format, size, count);
        }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The multisampled colour view (a pass's colour attachment).
    pub fn colour(&self) -> &wgpu::TextureView {
        &self.colour_view
    }

    /// The multisampled depth view (a pass's depth attachment).
    pub fn depth(&self) -> &wgpu::TextureView {
        &self.depth_view
    }

    /// A frame target over these textures. `resolve` is the single-sample view the pass resolves into: `Some` for
    /// the frame's last 3D pass only (the one resolve of the frame), `None` for every pass before it.
    pub fn target<'a>(&'a self, resolve: Option<&'a wgpu::TextureView>) -> FrameTarget<'a> {
        FrameTarget {
            colour: &self.colour_view,
            depth: &self.depth_view,
            size: self.size,
            resolve,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_list_starts_with_one_and_holds_powers_of_two() {
        let shape = |v: &[u32]| {
            assert_eq!(v.first(), Some(&1), "{v:?}");
            assert!(v.iter().all(|n| n.is_power_of_two()), "{v:?}");
            assert!(
                v.windows(2).all(|w| w[0] < w[1]),
                "ascending, unique: {v:?}"
            );
            assert!(v.iter().all(|n| *n <= 8), "{v:?}");
        };
        shape(&counts_where(|_| true));
        shape(&counts_where(|_| false));
        shape(&counts_where(|n| n == 4));
        assert_eq!(counts_where(|_| true), vec![1, 2, 4, 8]);
        assert_eq!(counts_where(|_| false), vec![1]);
        assert_eq!(counts_where(|n| n == 4), vec![1, 4]);
    }

    #[test]
    fn multisample_state_of_one_is_the_default() {
        assert_eq!(multisample_state(1), wgpu::MultisampleState::default());
        assert_eq!(multisample_state(0), wgpu::MultisampleState::default());
        assert_eq!(
            format!("{:?}", multisample_state(1)),
            format!("{:?}", wgpu::MultisampleState::default()),
            "the pipeline journal's text is unchanged with MSAA off"
        );
        assert_eq!(multisample_state(4).count, 4);
    }
}

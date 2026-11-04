//! World section kinds, the 23 shader buckets and the render settings that steer the bucket choice.

/// Number of world shader buckets; they are flushed in index order 0..=22.
// SPEC: RENDER_2009.md §5.1 step 5 (23 buckets flushed in index order)
pub const BUCKET_COUNT: usize = 23;

/// Bucket of the section lightmaps (opaque, written first).
// SPEC: RENDER_2009.md §5.2 (id 14 -> bucket 14)
pub const LIGHTMAP_BUCKET: u8 = 14;

/// Internal id a file `shader_kind` maps to at load.
// SPEC: RENDER_2009.md §5.2 (switch at engine+0x181be4, jump table VA 0x5e20e4)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionClass {
    /// Kind 1, internal id 0: vertex-lit texture.
    VertexLit,
    /// Kind 2, internal id 14: carries the lightmap payload, drawn opaque first.
    Lightmap,
    /// Kind 4, internal id 15: lightmapped texture, multiplied over the lightmap.
    LightmappedTexture,
    /// Kind 8, internal id 10: two textures blended by vertex alpha, vertex-lit.
    BlendVertexLit,
    /// Kind 9, internal id 20: two-texture blend multiplied over the lightmap.
    BlendLightmapped,
    /// Kinds 0, 3, 5, 6, 7 and any other value, internal id 24: never drawn.
    Hidden,
}

impl SectionClass {
    // SPEC: RENDER_2009.md §5.2 (file kind -> internal id table)
    pub fn of_kind(kind: u8) -> SectionClass {
        match kind {
            1 => SectionClass::VertexLit,
            2 => SectionClass::Lightmap,
            4 => SectionClass::LightmappedTexture,
            8 => SectionClass::BlendVertexLit,
            9 => SectionClass::BlendLightmapped,
            _ => SectionClass::Hidden,
        }
    }

    /// The engine's internal id.
    pub fn internal_id(self) -> u8 {
        match self {
            SectionClass::VertexLit => 0,
            SectionClass::Lightmap => 14,
            SectionClass::LightmappedTexture => 15,
            SectionClass::BlendVertexLit => 10,
            SectionClass::BlendLightmapped => 20,
            SectionClass::Hidden => 24,
        }
    }

    /// Load check: kinds 4 need the first texture, kinds 8 and 9 both.
    // SPEC: RENDER_2009.md §5.2 (load check column)
    pub fn textures_required(self) -> usize {
        match self {
            SectionClass::LightmappedTexture => 1,
            SectionClass::BlendVertexLit | SectionClass::BlendLightmapped => 2,
            _ => 0,
        }
    }
}

/// Render settings that feed the bucket choice and the pass recipes, with the shipped defaults. Names are
/// ours; the ROM names are identified only by hash in the spec.
// SPEC: RENDER_2009.md §13 (render settings table, defaults)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    pub lightmaps: bool,
    pub draw_flat: bool,
    pub lightmaps_only: bool,
    pub detail_textures: bool,
    pub env_maps: bool,
    pub env_bump: bool,
    pub dot3_bump: bool,
    pub dot3_env_bump: bool,
    /// MODULATE2X instead of MODULATE in the vertex-lit recipes and the 2x multiply blend.
    pub saturate: bool,
    pub full_bright: bool,
    /// Packed ARGB texture factor of the lightmap passes; 0 means white.
    pub lightmap_tfactor: u32,
    /// Far plane, clamped to [3, 500000] by the camera builder.
    pub far_z: f32,
    pub mip_bias: f32,
    pub bilinear: bool,
    pub trilinear: bool,
    pub anisotropy: u32,
    /// Multisample anti-aliasing: samples per pixel of the 3D passes (1 = off; 2, 4 or 8; anything else reads as 1,
    /// [`RenderSettings::msaa_count`]). Pipelines are built with it, so a change needs a new [`crate::WorldRenderer`]
    /// (and the client re-uploads its scene and models); [`crate::supported_msaa`] lists what the adapter offers.
    // DC-DEFAULT: CLI-1593 (proposed) multisample anti-aliasing: the 2009 client offered the device's multisample types
    // 2x..16x (specs/RENDER_2009.md:68 r_list_msaa, :110 msaa_samples, :1183 console variables msaa_samples /
    // accept_msaa default 0); DeCombat renders the 3D passes into a multisampled target resolved before the HUD
    pub msaa: u32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            lightmaps: true,
            draw_flat: false,
            lightmaps_only: false,
            detail_textures: true,
            env_maps: true,
            env_bump: true,
            dot3_bump: true,
            dot3_env_bump: true,
            saturate: true,
            full_bright: false,
            lightmap_tfactor: 0,
            far_z: 12000.0,
            mip_bias: -1.0,
            bilinear: true,
            trilinear: true,
            anisotropy: 0,
            msaa: 1,
        }
    }
}

impl RenderSettings {
    /// The sample count the pipelines and the frame targets use: `msaa` when it is 1, 2, 4 or 8, else 1 (off).
    pub fn msaa_count(&self) -> u32 {
        if matches!(self.msaa, 1 | 2 | 4 | 8) {
            self.msaa
        } else {
            1
        }
    }

    /// `F_lm`: `lightmap_tfactor` when non-zero, else white.
    // SPEC: RENDER_2009.md §5.2 (notation: F_lm)
    pub fn lightmap_factor(&self) -> u32 {
        if self.lightmap_tfactor != 0 {
            self.lightmap_tfactor
        } else {
            0xffff_ffff
        }
    }
}

/// What the bucket selector needs to know about a section's first texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureTraits {
    pub present: bool,
    /// DTX command mode: 0 none, 1 detail, 2 env, 3 env-alpha, 4 env-bump, 5 env-bump-no-fallback, 6 dot3-bump,
    /// 7 dot3-env-bump, 8 effect.
    pub mode: u8,
    /// DTX flag 0x1 (renderer record bit 0x01): alpha read as a self-illumination mask.
    pub hi: bool,
}

/// Bucket for a section, `None` when the section is not drawn.
// SPEC: RENDER_2009.md §5.2 (bucket selection, engine+0x136780 and the undefined code after it)
pub fn select_bucket(class: SectionClass, tex: TextureTraits, s: &RenderSettings) -> Option<u8> {
    let hi = |a: u8, b: u8| if tex.hi { a } else { b };
    match class {
        SectionClass::Hidden => None,
        SectionClass::BlendVertexLit => Some(10),
        SectionClass::BlendLightmapped => Some(20),
        SectionClass::Lightmap => {
            if s.draw_flat || !s.lightmaps {
                None
            } else {
                Some(LIGHTMAP_BUCKET)
            }
        }
        SectionClass::VertexLit | SectionClass::LightmappedTexture if s.draw_flat => Some(0),
        SectionClass::LightmappedTexture if s.lightmaps => {
            if s.lightmaps_only || !tex.present {
                return None;
            }
            Some(match tex.mode {
                1 if s.detail_textures => 16,
                2 if s.env_maps => 17,
                4 if s.env_bump => 18,
                4 if s.env_maps => 17,
                5 if s.env_bump => 19,
                6 if s.dot3_bump => 21,
                7 if s.dot3_env_bump => 22,
                7 if s.env_maps => 17,
                _ => 15,
            })
        }
        // id 0, or id 15 with lightmaps off
        SectionClass::VertexLit | SectionClass::LightmappedTexture => {
            if !tex.present {
                return Some(0);
            }
            Some(match tex.mode {
                1 if s.detail_textures => hi(8, 2),
                2 if s.env_maps => hi(9, 3),
                3 if s.env_maps => hi(9, 4),
                4 if s.env_bump => hi(9, 5),
                4 => hi(9, 3),
                5 if s.env_bump => hi(9, 6),
                6 if s.dot3_bump => hi(7, 11),
                7 if s.dot3_env_bump => hi(9, 12),
                7 => hi(9, 3),
                8 => hi(7, 13),
                _ => hi(7, 1),
            })
        }
    }
}

/// The pass families this slice implements; every bucket maps to one of them (buckets whose own recipe is
/// not implemented yet fall back to the base recipe of their group, see [`PassKind::of_bucket`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PassKind {
    /// Bucket 0: vertex colour only.
    Flat,
    /// Bucket 1 (and, until their recipes exist, 2..9, 11..13): `T0 * D` (x2 with `saturate`).
    VertexLit,
    /// Bucket 10: `lerp(T1, T0, D.a * T0.a) * D` (x2).
    BlendVertexLit,
    /// Bucket 14: `lightmap * F_lm`, opaque.
    Lightmap,
    /// Bucket 15 (and 16..19, 21, 22 for now): `T0 * F_lm`, multiplied over the frame.
    LightmappedTexture,
    /// Bucket 20: bucket 10 without the vertex colour, multiplied over the frame.
    BlendLightmapped,
}

impl PassKind {
    // SPEC: RENDER_2009.md §5.2 (pass recipes table)
    pub fn of_bucket(b: u8) -> Option<PassKind> {
        Some(match b {
            0 => PassKind::Flat,
            1..=9 | 11..=13 => PassKind::VertexLit,
            10 => PassKind::BlendVertexLit,
            14 => PassKind::Lightmap,
            15..=19 | 21 | 22 => PassKind::LightmappedTexture,
            20 => PassKind::BlendLightmapped,
            _ => return None,
        })
    }

    /// True when the bucket's own recipe is drawn, false when the base recipe stands in for it.
    pub fn exact_for(b: u8) -> bool {
        matches!(b, 0 | 1 | 10 | 14 | 15 | 20)
    }

    /// Passes that multiply the frame (`dst * src`, or `2 * dst * src` with `saturate`).
    pub fn multiplies(self) -> bool {
        matches!(
            self,
            PassKind::LightmappedTexture | PassKind::BlendLightmapped
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(mode: u8, hi: bool) -> TextureTraits {
        TextureTraits {
            present: true,
            mode,
            hi,
        }
    }

    #[test]
    fn kinds_to_ids() {
        let ids: Vec<u8> = (0..=10)
            .map(|k| SectionClass::of_kind(k).internal_id())
            .collect();
        assert_eq!(ids, vec![24, 0, 14, 24, 15, 24, 24, 24, 10, 20, 24]);
    }

    #[test]
    fn default_buckets() {
        let s = RenderSettings::default();
        use SectionClass::*;
        assert_eq!(
            select_bucket(Lightmap, TextureTraits::default(), &s),
            Some(14)
        );
        assert_eq!(select_bucket(LightmappedTexture, t(0, false), &s), Some(15));
        assert_eq!(select_bucket(LightmappedTexture, t(1, false), &s), Some(16));
        assert_eq!(select_bucket(LightmappedTexture, t(3, false), &s), Some(15));
        assert_eq!(
            select_bucket(LightmappedTexture, TextureTraits::default(), &s),
            None
        );
        assert_eq!(
            select_bucket(VertexLit, TextureTraits::default(), &s),
            Some(0)
        );
        assert_eq!(select_bucket(VertexLit, t(0, false), &s), Some(1));
        assert_eq!(select_bucket(VertexLit, t(0, true), &s), Some(7));
        assert_eq!(select_bucket(VertexLit, t(3, false), &s), Some(4));
        assert_eq!(select_bucket(VertexLit, t(8, false), &s), Some(13));
        assert_eq!(select_bucket(BlendVertexLit, t(0, false), &s), Some(10));
        assert_eq!(select_bucket(BlendLightmapped, t(0, false), &s), Some(20));
        assert_eq!(select_bucket(Hidden, t(0, false), &s), None);
    }

    #[test]
    fn msaa_count_accepts_only_the_power_of_two_counts() {
        assert_eq!(RenderSettings::default().msaa, 1, "off by default");
        for (set, want) in [(0, 1), (1, 1), (2, 2), (3, 1), (4, 4), (6, 1), (8, 8), (16, 1)] {
            let s = RenderSettings {
                msaa: set,
                ..RenderSettings::default()
            };
            assert_eq!(s.msaa_count(), want, "msaa {set}");
        }
    }

    #[test]
    fn switches() {
        use SectionClass::*;
        let flat = RenderSettings {
            draw_flat: true,
            ..RenderSettings::default()
        };
        assert_eq!(select_bucket(VertexLit, t(0, false), &flat), Some(0));
        assert_eq!(
            select_bucket(LightmappedTexture, t(0, false), &flat),
            Some(0)
        );
        assert_eq!(select_bucket(Lightmap, t(0, false), &flat), None);
        let nolm = RenderSettings {
            lightmaps: false,
            ..RenderSettings::default()
        };
        assert_eq!(
            select_bucket(LightmappedTexture, t(0, false), &nolm),
            Some(1)
        );
        assert_eq!(select_bucket(Lightmap, t(0, false), &nolm), None);
        let only = RenderSettings {
            lightmaps_only: true,
            ..RenderSettings::default()
        };
        assert_eq!(select_bucket(LightmappedTexture, t(0, false), &only), None);
        assert_eq!(select_bucket(Lightmap, t(0, false), &only), Some(14));
    }

    #[test]
    fn every_bucket_has_a_pass() {
        for b in 0..BUCKET_COUNT as u8 {
            assert!(PassKind::of_bucket(b).is_some(), "bucket {b}");
        }
        assert_eq!(PassKind::of_bucket(23), None);
    }
}

//! The surface of the world at a point (V-SURF-1): the ROM surface id of the world polygon a shot or a blast hit.
//!
//! Every world polygon names a surface record of its BSP model, and that record's 16-bit texture-flags word is the
//! row `Id` of the ROM surface table (`ATTRIBUTES\SURFACE.TXT`) the 2009 client plays the impact of. The probe holds
//! a ray world over every BSP model of the world file (the level model and the models linked to objects) and the
//! surface id of each polygon, read from the user's world file at run time.
// SPEC: FMT_DAT85.md §5.2 (surface record: flags, texture index, texture-flags word; polygon -> surface index)
// OBSERVED: capture 20261025_065016_tdm6 c6 (JUNK_FLEA: 253 world shot records, `a` 10 / 20,
// each equal to the texture-flags word of the polygon at the record's end point; the two blasts, seq 9727 / 20355,
// `a` 10 = the polygon below the blast point), 20261025_002236_tdm4 c6 and 20261025_001409_tdm2 c6 (SNOWVALLEY:
// world records 20 / 71 and the blasts seq 8065 / 8517 71, the same rule; REFERENCE, accepted by the real client)
// DC-DEFAULT: DSV-62 (proposed)

use dc_sim::{CollisionWorld, V3};
use dc_world::World;

/// How far around a reported end point the probe looks for the polygon (both directions along the shot).
// DC-DEFAULT: DSV-62 (proposed) 8 units (the reference stack's end points lie on the polygon within float noise)
pub const END_PROBE_UNITS: f32 = 8.0;
/// The downward probe of a blast point: from this far above it ...
// DC-DEFAULT: DSV-62 (proposed)
pub const BLAST_PROBE_UP: f32 = 8.0;
/// ... to this far below it.
pub const BLAST_PROBE_DOWN: f32 = 64.0;

/// Ray world over every BSP model with the surface id of each polygon.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SurfaceProbe {
    world: CollisionWorld,
    /// Per model (index-aligned with `world.models`), per polygon: the texture-flags word of its surface.
    ids: Vec<Vec<u16>>,
}

impl SurfaceProbe {
    /// From a parsed world: every BSP model, every polygon.
    pub fn from_world(w: &World<'_>) -> Option<SurfaceProbe> {
        if w.models.is_empty() {
            return None;
        }
        let models = w
            .models
            .iter()
            .map(dc_sim::from_world::coll_model)
            .collect();
        let ids = w
            .models
            .iter()
            .map(|m| {
                m.polygons
                    .iter()
                    .map(|p| m.surfaces.get(p.surface as usize).map_or(0, |s| s.texflags))
                    .collect()
            })
            .collect();
        Some(SurfaceProbe {
            world: CollisionWorld::from_parts(models, &[]),
            ids,
        })
    }

    /// The surface id of the first polygon on the segment `from` -> `to`, if one is hit and its id fits the byte.
    pub fn first_on(&self, from: [f32; 3], to: [f32; 3]) -> Option<u8> {
        let h = self.world.ray(V3::from_array(from), V3::from_array(to))?;
        let id = *self.ids.get(h.model as usize)?.get(h.poly as usize)?;
        u8::try_from(id).ok()
    }

    /// The surface at the end point `at` of a shot travelling along `dir` (unit): the first polygon within
    /// [`END_PROBE_UNITS`] before the point to as far after it.
    pub fn at_shot_end(&self, at: [f32; 3], dir: [f32; 3]) -> Option<u8> {
        let k = END_PROBE_UNITS;
        let from = [at[0] - dir[0] * k, at[1] - dir[1] * k, at[2] - dir[2] * k];
        let to = [at[0] + dir[0] * k, at[1] + dir[1] * k, at[2] + dir[2] * k];
        self.first_on(from, to)
    }

    /// The surface below a blast point (or a resting object): straight down from just above it.
    pub fn below(&self, at: [f32; 3]) -> Option<u8> {
        self.first_on(
            [at[0], at[1] + BLAST_PROBE_UP, at[2]],
            [at[0], at[1] - BLAST_PROBE_DOWN, at[2]],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_world_floor_has_its_surface_word() {
        // the synthetic world (made-up values): its single model's surfaces carry made-up texture-flags words
        let bytes = crate::world::synthetic_world_bytes();
        let w = World::parse_with(&bytes, dc_world::ParseOptions { render: false }).unwrap();
        let p = SurfaceProbe::from_world(&w).expect("probe");
        let want: Vec<u16> = w.models[0]
            .polygons
            .iter()
            .map(|q| w.models[0].surfaces[q.surface as usize].texflags)
            .collect();
        assert_eq!(p.ids[0], want);
        // nothing in empty space far outside the box
        assert_eq!(
            p.first_on([5000.0, 5000.0, 5000.0], [5001.0, 5000.0, 5000.0]),
            None
        );
    }

    /// A floor quad (made-up surface word 37) at y 0: a shot end on it, a blast above it and a miss beside it.
    #[test]
    fn floor_quad_gives_its_surface_word() {
        use dc_world::synth::{SynthModel, SynthWorld};
        let bytes = SynthWorld {
            info: b"floor".to_vec(),
            extent_min: [-512.0, -64.0, -512.0],
            extent_max: [512.0, 256.0, 512.0],
            tree_shape: vec![false],
            models: vec![SynthModel {
                name: b"main".to_vec(),
                textures: vec![b"t".to_vec()],
                planes: vec![([0.0, 1.0, 0.0], 0.0)],
                surfaces: vec![(1, 0, 37)],
                polys: vec![(0, 0, vec![0, 1, 2, 3])],
                nodes: vec![(0, 0, -1, -2)],
                points: vec![
                    [-100.0, 0.0, -100.0],
                    [-100.0, 0.0, 100.0],
                    [100.0, 0.0, 100.0],
                    [100.0, 0.0, -100.0],
                ],
                root: 0,
                box_min: [-100.0, 0.0, -100.0],
                box_max: [100.0, 0.0, 100.0],
                ..SynthModel::default()
            }],
            ..SynthWorld::default()
        }
        .build();
        let w = World::parse_with(&bytes, dc_world::ParseOptions { render: false }).unwrap();
        let p = SurfaceProbe::from_world(&w).expect("probe");
        let down = [0.0, -1.0, 0.0];
        assert_eq!(p.at_shot_end([10.0, 0.0, 10.0], down), Some(37));
        assert_eq!(p.at_shot_end([10.0, 3.0, 10.0], down), Some(37));
        assert_eq!(p.below([10.0, 20.0, 10.0]), Some(37));
        assert_eq!(p.below([300.0, 20.0, 10.0]), None);
        assert_eq!(p.at_shot_end([10.0, 50.0, 10.0], down), None);
    }
}

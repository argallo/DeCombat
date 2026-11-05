//! Where the world comes from: [`WorldSource`] loads the collision set, the render scene, the spawn points
//! and the standing box, and is the place a server's authority enters (corrections, received player
//! updates). Sources: [`RomWorld`] (a map of the user's ROM), [`ModWorld`] (a community map of the server's mod set,
//! or a world file previewed standalone) and [`SynthSource`] (made-up bytes for tests).
// SPEC: FMT_DAT85.md §5 (BSP models), §6.1-§6.2 (objects, player spawn role), §9.1 (blockers), §10 (render data);
// MOVEMENT_2009.md §9.2 (stand box), §12.4 (server corrections)

use crate::error::ClientError;
use crate::update::LocalUpdate;
use dc_render::{
    global_light, world_lights, AmbientGrid, GpuContext, GpuScene, LightSet, LightShape,
    ModelLight, ModelLightSettings, RomTextures, SceneMesh, SceneOptions, SceneStats, TextureImage,
    TextureIndex, TextureTraits, UploadStats, WorldRenderer,
};
use dc_sim::from_world::{collision_world, ladder_volumes, selected_models, WorldFilter};
use dc_sim::{CollisionWorld, ContainerVolumes, Correction, StepEvents, V3};
use dc_world::{ObjectRole, ParseOptions, World};
use glam::Vec3;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// A start point: position and facing (yaw, radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnPoint {
    pub pos: V3,
    pub yaw: f32,
}

/// The device the scene is uploaded to.
#[derive(Clone, Copy)]
pub struct GpuTarget<'a> {
    pub ctx: &'a GpuContext,
    pub renderer: &'a WorldRenderer,
}

/// A loaded world.
pub struct LoadedWorld {
    /// Display name (for ROM maps: the map name read from the user's archive).
    pub name: String,
    pub collision: CollisionWorld,
    /// The container volumes the body can touch (the world's ladders), fed to the mover every tick.
    // SPEC: MOVEMENT_2009.md §11.1 (container list from the touched containers); DC-DEFAULT: CLI-59 (proposed)
    pub containers: ContainerVolumes,
    pub spawns: Vec<SpawnPoint>,
    /// Standing half extents of the player body.
    pub stand_dims: V3,
    /// Crouched half extents (the crouch clip's box of the model carrying the stand clip); `None` keeps the
    /// standing box while crouched.
    // DC-DEFAULT: SIM-25 (proposed)
    pub crouch_dims: Option<V3>,
    /// Uploaded render scene (when a GPU target was given).
    pub scene: Option<GpuScene>,
    pub scene_stats: SceneStats,
    pub upload: Option<UploadStats>,
    pub textures_missing: u64,
    /// Lowest y of any collision model's bounds (anything far below fell out of the world).
    pub floor_limit: f32,
    /// The world file's extents (min, max): the box of the object stream's `pos48` positions.
    // SPEC: FMT_DAT85.md §4 (extents); UDP_TRANSPORT_2009_addendum_disasm.md T14 (pos48 box = the world's extents)
    pub extents: ([f32; 3], [f32; 3]),
    pub file_bytes: usize,
    pub load_ms: f64,
    /// Model lighting inputs of the world (light grid and static lights), kept when a render scene was built.
    // SPEC: RENDER_2009.md §6.3 (model ambient from the light grid at the model position plus the world's lights)
    pub model_lights: Option<WorldLights>,
    /// The surface type of every polygon of every collision model (collision model order, then polygon order):
    /// the polygon's surface record `surf_texflags`, the ROM surface id the footsteps and the 0x64 trailer use.
    // OBSERVED: capture 20261025_002236_tdm4/analysis/updates.csv (the polygon under the 2009
    // client's grounded positions carries the surface byte of its own 0x64 trailer in 674 of 681 updates; the others
    // stood on objects outside this collision set; test `sound_rom.rs`), CA_ROM worlds 2026-10-25 (floor polygons:
    // SNOWVALLEY 6,053 of 10,534 carry 71, JUNK_FLEA 1,581 of 2,626 carry 20); EVIDENCE: cshell+0x27f430 (the client
    // asks the engine for the standing polygon's value)
    // SPEC: FMT_DAT85.md §5.2 item 22 (surface record: flags, texture, texflags)
    pub surface_types: Vec<Vec<u16>>,
    /// The linked world models and the state the server's objects gave them (CLI-230, world-objects).
    pub world_models: crate::objects::WorldModels,
    /// The last sky table applied to the scene (client-hudgaps).
    pub sky_applied: Option<crate::objects::SkyTable>,
    /// The world's far plane from its world settings, when it gives one (`dc_render::world_far_z`): the camera's far
    /// plane in this world ([`crate::ViewSettings::with_world_far`]).
    // DC-DEFAULT: CLI-1851 (proposed), see DC_DEFAULTS.md
    pub far_z: Option<f32>,
}

impl LoadedWorld {
    /// Applies a server-backed source's objects to the render scene and the collision set (the 2009 rule: only the
    /// world models the server created are drawn and solid, with their flags); returns the placed effects that
    /// appeared since the last call.
    // DC-DEFAULT: CLI-230 (proposed)
    pub fn apply_objects(
        &mut self,
        objs: &crate::objects::ServerObjects,
    ) -> Vec<crate::objects::WorldEffect> {
        self.world_models
            .apply(objs, &mut self.collision, self.scene.as_mut())
    }

    /// Applies the server's sky table to the render scene when it changed (view box, sky models drawn).
    pub fn apply_sky(&mut self, table: &crate::objects::SkyTable) {
        if self.sky_applied.as_ref() == Some(table) {
            return;
        }
        if let Some(s) = self.scene.as_mut() {
            crate::objects::apply_sky(table, s);
        }
        self.sky_applied = Some(table.clone());
    }

    /// gpu-recover: takes the render scene of `fresh` (the same world loaded again onto a new device) in place of
    /// the scene of the lost device, and gives it the state the server's objects gave this world so far (world models
    /// shown / hidden and moved, the sky table). The collision set, the world-model state and everything else on the
    /// CPU side stay as they are. The old scene is put away without a drop ([`crate::gpulost::retire`]).
    // DC-DEFAULT: CLI-460 (amended by gpu-recover) the match keeps its world state across a device rebuild
    pub fn adopt_scene(&mut self, fresh: LoadedWorld) {
        let mut fresh = fresh;
        if let Some(old) = std::mem::replace(&mut self.scene, fresh.scene.take()) {
            crate::gpulost::retire(old);
        }
        self.scene_stats = fresh.scene_stats;
        self.upload = fresh.upload;
        self.textures_missing = fresh.textures_missing;
        if let Some(s) = self.scene.as_mut() {
            self.world_models.reapply_scene(s);
            if let Some(t) = &self.sky_applied {
                crate::objects::apply_sky(t, s);
            }
        }
        // the rest of `fresh` is CPU data (collision, tables): dropped normally
    }
}

/// Where an own pellet stopped in the world, resolved when it was fired (the 2009 trace hands the hit polygon to the
/// impact step at once, cshell+0x238eb0): the point, the contact normal, the polygon's surface type and whether
/// the polygon belongs to a linked world model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotImpactAt {
    pub point: [f32; 3],
    pub normal: [f32; 3],
    pub surface: u16,
    pub on_model: bool,
}

impl ShotImpact {
    pub fn at(&self) -> ShotImpactAt {
        ShotImpactAt {
            point: self.point,
            normal: self.normal,
            surface: self.surface,
            on_model: self.model.is_some(),
        }
    }
}

/// Where a shot ray of the own player stopped in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotImpact {
    pub dist: f32,
    pub point: [f32; 3],
    /// The contact normal.
    pub normal: [f32; 3],
    /// The polygon's surface type (the impact's surface id).
    pub surface: u16,
    /// The linked world model it hit (lower-case name), `None` for the level.
    pub model: Option<Vec<u8>>,
}

impl LoadedWorld {
    /// The collision set and the shot-only models as the shot trace's collider (CLI-250).
    pub fn shot_world(&self) -> crate::shots::ShotWorld<'_> {
        crate::shots::ShotWorld {
            main: &self.collision,
            extra: self.world_models.shot_world(),
        }
    }

    /// The surface type of a hit of [`LoadedWorld::shot_world`].
    pub fn shot_surface(&self, model: u32, poly: u32) -> u16 {
        let n = self.collision.models.len() as u32;
        if model < n || model == dc_sim::BLOCKER_MODEL {
            return self.surface_type(model, poly);
        }
        self.world_models
            .shot_surface((model - n) as usize, poly, &self.surface_types)
    }

    /// The first polygon of the level or of a world model the server created in a state rays stop at, along the unit
    /// ray `dir` from `origin` within `range` (player clips do not stop shots).
    // EVIDENCE: cshell+0x23a910, cshell+0x238eb0 (see crate::shots)
    // DC-DEFAULT: CLI-250 (proposed)
    pub fn shot_hit(&self, origin: [f32; 3], dir: [f32; 3], range: f32) -> Option<ShotImpact> {
        let w = self.shot_world();
        let (dist, h) = crate::shots::first_hit(&w, origin, dir, range)?;
        let n = self.collision.models.len() as u32;
        let model = if h.model < n {
            self.world_models
                .name_at_collision(h.model as usize)
                .map(<[u8]>::to_vec)
        } else {
            self.world_models
                .shot_model_name((h.model - n) as usize)
                .map(<[u8]>::to_vec)
        };
        Some(ShotImpact {
            dist,
            point: [0, 1, 2].map(|k| origin[k] + dir[k] * dist),
            normal: h.normal.to_array(),
            surface: self.shot_surface(h.model, h.poly),
            model,
        })
    }
}

/// The world's model lighting inputs: the light grid with the light groups' grids added, the static lights of the
/// world's light objects, the global (sun) light and the geometry its visibility is traced against.
// SPEC: RENDER_2009.md §6.2 (light-group grids add to the grid the models sample), §6.3 (grid + lights), §6.3.1 (the
// sun is one more record of the model light set when its colour is not black, scaled by its visibility); EVIDENCE:
// engine+0x1b21b0 (the builder adds the sun before the grid sample, with the convert-to-ambient fraction of the message)
#[derive(Debug, Clone)]
pub struct WorldLights {
    pub grid: AmbientGrid,
    /// Static lights of the world's light objects (without the sun).
    pub statics: Vec<ModelLight>,
    /// The sun as built from the world's global light object.
    pub sun: Option<ModelLight>,
    /// World geometry for the sun visibility ray (no blockers) and the ray length.
    sky: Option<Arc<CollisionWorld>>,
    sky_reach: f32,
    /// Load-time grid of the static lights (render-core): the candidates of a light-set query.
    index: StaticLightIndex,
    /// The last sun-visibility results by exact position (render-core).
    sun_memo: SunMemo,
    /// The 2009 per-object sun cache (client-fps): position of the last test and its result, by object key.
    sun_objects: SunObjects,
    /// w2-fx: the dynamic lights of the running effects this frame ([`WorldLights::set_fx_lights`]).
    dynamics: FxLightList,
}

/// The frame's dynamic lights, replaced once per frame by the effect stage and read by every light-set query of the
/// frame (a clone starts empty).
#[derive(Debug, Default)]
struct FxLightList(std::sync::RwLock<Vec<ModelLight>>);

impl Clone for FxLightList {
    fn clone(&self) -> Self {
        FxLightList::default()
    }
}

/// Uniform grid over the reach boxes of the static point and spot lights, built once per world. A query returns, in
/// list order, every light whose reach box (sphere bounds, padded) shares a cell with the query box, plus the lights
/// kept outside the grid (directional lights, non-finite values, boxes that overflow to infinity, very large
/// spheres): a superset of the lights `LightSet::build` offers, so the light set is unchanged. The grid never has
/// more than `LIGHT_GRID_MAX_CELLS` cells whatever the lights are (a mod's far-away light coarsens the cells, it
/// cannot overflow the sizing or the allocation); a union box that is not finite gets no grid (every light is
/// tested). Engineering only (no 2009 rule).
#[derive(Debug, Clone, Default)]
struct StaticLightIndex {
    origin: Vec3,
    cell: f32,
    dims: [usize; 3],
    /// Light indices per cell (x fastest), ascending.
    cells: Vec<Vec<u32>>,
    /// Lights tested on every query, ascending.
    always: Vec<u32>,
    count: usize,
}

/// Grid cell edge (world units) and the most cells one light may cover before it goes to the always list.
const LIGHT_CELL: f32 = 256.0;
const LIGHT_MAX_CELLS: usize = 4096;
const LIGHT_GRID_MAX_CELLS: usize = 1 << 18;

impl StaticLightIndex {
    fn reach(l: &ModelLight) -> Option<(Vec3, Vec3)> {
        if matches!(l.shape, LightShape::Directional { .. })
            || !l.pos.is_finite()
            || !l.radius.is_finite()
        {
            return None;
        }
        // padded so float rounding in the reach test can never fall outside the box
        let r = Vec3::splat(l.radius.abs() * 1.001 + 1.0);
        let (a, b) = (l.pos - r, l.pos + r);
        // a padding or a corner that overflows to infinity cannot be put in cells: the light is tested every query
        (a.is_finite() && b.is_finite()).then_some((a, b))
    }

    /// Cells per axis over the extent `ext` with edge `cell`, as floats: `floor(ext / cell) + 1`, the same f32
    /// arithmetic as [`StaticLightIndex::range`], so the far corner of the union box lands in the last cell.
    fn cells_per_axis(ext: Vec3, cell: f32) -> [f64; 3] {
        [0, 1, 2].map(|k| f64::from((ext[k] / cell).floor() + 1.0))
    }

    fn new(lights: &[ModelLight]) -> StaticLightIndex {
        let mut idx = StaticLightIndex {
            count: lights.len(),
            cell: LIGHT_CELL,
            ..Default::default()
        };
        let boxes: Vec<Option<(Vec3, Vec3)>> = lights.iter().map(Self::reach).collect();
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for (a, b) in boxes.iter().flatten() {
            lo = lo.min(*a);
            hi = hi.max(*b);
        }
        let ext = hi - lo;
        if lo.cmple(hi).all() && ext.is_finite() {
            // the cell count is a product of floats (no integer overflow on i686 or x86_64); doubling the edge ends
            // the loop for any finite extent (at most ~120 steps: one cell per axis once the edge passes the extent)
            let mut cell = LIGHT_CELL;
            let mut n = Self::cells_per_axis(ext, cell);
            while n.iter().product::<f64>() > LIGHT_GRID_MAX_CELLS as f64 && cell.is_finite() {
                cell *= 2.0;
                n = Self::cells_per_axis(ext, cell);
            }
            if n.iter().product::<f64>() <= LIGHT_GRID_MAX_CELLS as f64 {
                idx.origin = lo;
                idx.cell = cell;
                idx.dims = n.map(|v| v as usize);
                idx.cells = vec![Vec::new(); idx.dims[0] * idx.dims[1] * idx.dims[2]];
            }
        }
        for (i, b) in boxes.iter().enumerate() {
            let Some((a, b)) = b else {
                idx.always.push(i as u32);
                continue;
            };
            match idx.range(*a, *b) {
                Some((c0, c1))
                    if (c1[0] - c0[0] + 1) * (c1[1] - c0[1] + 1) * (c1[2] - c0[2] + 1)
                        <= LIGHT_MAX_CELLS =>
                {
                    for z in c0[2]..=c1[2] {
                        for y in c0[1]..=c1[1] {
                            for x in c0[0]..=c1[0] {
                                let k = (z * idx.dims[1] + y) * idx.dims[0] + x;
                                idx.cells[k].push(i as u32);
                            }
                        }
                    }
                }
                _ => idx.always.push(i as u32),
            }
        }
        idx
    }

    /// Cell range covered by the box `a..b`, clamped to the grid; `None` when it misses the grid.
    fn range(&self, a: Vec3, b: Vec3) -> Option<([usize; 3], [usize; 3])> {
        if self.cells.is_empty() {
            return None;
        }
        let mut c0 = [0usize; 3];
        let mut c1 = [0usize; 3];
        for k in 0..3 {
            let n = self.dims[k] as f32;
            let f0 = ((a[k] - self.origin[k]) / self.cell).floor();
            let f1 = ((b[k] - self.origin[k]) / self.cell).floor();
            if f1 < 0.0 || f0 >= n {
                return None;
            }
            c0[k] = f0.max(0.0) as usize;
            c1[k] = (f1.min(n - 1.0)) as usize;
        }
        Some((c0, c1))
    }

    /// Candidate lights of the query box `pos +- radius`, ascending, into `out`.
    fn query(&self, pos: Vec3, radius: f32, out: &mut Vec<u32>) {
        out.clear();
        if !pos.is_finite() || !radius.is_finite() || radius < 0.0 {
            out.extend(0..self.count as u32);
            return;
        }
        out.extend_from_slice(&self.always);
        let r = Vec3::splat(radius);
        if let Some((c0, c1)) = self.range(pos - r, pos + r) {
            for z in c0[2]..=c1[2] {
                for y in c0[1]..=c1[1] {
                    for x in c0[0]..=c1[0] {
                        out.extend_from_slice(
                            &self.cells[(z * self.dims[1] + y) * self.dims[0] + x],
                        );
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// Sun visibility by exact position bits: the last few results (a still object asks for the same position every
/// frame). The ray is deterministic, so a hit returns the value the ray would. Engineering only.
#[derive(Debug, Default)]
struct SunMemo(std::sync::Mutex<SunRing>);

/// Ring of (position bits, visibility): entries, next slot, entries in use.
#[derive(Debug, Default)]
struct SunRing {
    entries: [([u32; 3], f32); 8],
    next: usize,
    used: usize,
}

impl Clone for SunMemo {
    fn clone(&self) -> Self {
        SunMemo::default()
    }
}

impl SunMemo {
    fn get(&self, key: [u32; 3]) -> Option<f32> {
        let g = self.0.lock().ok()?;
        g.entries[..g.used].iter().find(|e| e.0 == key).map(|e| e.1)
    }

    fn put(&self, key: [u32; 3], v: f32) {
        if let Ok(mut g) = self.0.lock() {
            let at = g.next;
            g.entries[at] = (key, v);
            g.next = (at + 1) % 8;
            g.used = (g.used + 1).min(8);
        }
    }
}

/// The 2009 per-object sun cache (client-fps): each model object keeps the position of its last sun test and the
/// result; the test runs again only when the object has moved farther than the sun variance + 0.001 from that
/// position. Keyed by [`sun_key`]; a clone starts empty. A key's entry lives as long as the world (object ids are
/// 16-bit, so the map is bounded).
#[derive(Debug, Default)]
struct SunObjects(std::sync::Mutex<std::collections::HashMap<u32, ([f32; 3], f32)>>);

impl Clone for SunObjects {
    fn clone(&self) -> Self {
        SunObjects::default()
    }
}

/// The world-light variable `model_sun_variance` at its 2009 default (also the soft-edge step of CLI-110, which is
/// not drawn): the per-object sun cache keeps its value while the object stays within this + [`SUN_MOVE_EPSILON`].
// EVIDENCE: engine+0x1b21b0; DISASM: engine+0x1b2451..0x1b2472 (the variable's float at VA 0x6d443c, initialised to
// 0.05, plus the code constant 0.001 at VA 0x6731a0, compared as a squared distance by engine+0x135e0)
pub const SUN_VARIANCE: f32 = 0.05;
pub const SUN_MOVE_EPSILON: f32 = 0.001;

/// The object kinds of the per-object sun cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SunObject {
    /// A character (remote player or corpse) by its object id.
    Character(u16),
    /// A dropped weapon by its object id.
    Drop(u16),
    /// The own first-person weapon (lit at the eye, REN-13).
    ViewWeapon,
}

/// The kept sun result of an object (`kept`: the position of its last test and the result) when the object is still
/// within [`SUN_VARIANCE`] + [`SUN_MOVE_EPSILON`] of that position (squared distance, as engine+0x135e0 compares);
/// `None` asks for a new test.
pub fn kept_sun(kept: Option<([f32; 3], f32)>, pos: Vec3) -> Option<f32> {
    let reach = SUN_VARIANCE + SUN_MOVE_EPSILON;
    let (at, k) = kept?;
    let d = pos - Vec3::from(at);
    (d.length_squared() <= reach * reach).then_some(k)
}

/// The cache key of an object (kind in the high half, object id in the low half).
pub fn sun_key(o: SunObject) -> u32 {
    match o {
        SunObject::Character(id) => 0x1_0000 | id as u32,
        SunObject::Drop(id) => 0x2_0000 | id as u32,
        SunObject::ViewWeapon => 0x3_0000,
    }
}

/// The surface flag of the polygons the sun ray must end on (sky).
// EVIDENCE: engine+0x1b1ba0 (visible = bit 4 of the byte at surface +4 of the polygon the segment hits); OBSERVED: CA_ROM
// 2026-10-25 (an upward ray from every SNOWVALLEY spawn ends about 16,180 units up on a polygon with this flag; the
// JUNK_FLEA spawns are under ceilings without it)
pub const SKY_SURFACE_FLAG: u32 = 0x10;

impl WorldLights {
    /// The lighting inputs of a parsed world and its collision geometry: light groups at intensity 1 (REN-9) and the
    /// world's global light object as the sun (REN-11; the same seven numbers dc-dserver sends in `S2C_DIR_LIGHT`,
    /// DSV-40, which this client does not read yet).
    // DC-DEFAULT: CLI-106 (proposed), see the staged rows (the sun from the world object until the client takes
    // `S2C_DIR_LIGHT`; groups on at intensity 1 as dc-render's viewer)
    pub fn of(world: &World<'_>, collision: Option<&CollisionWorld>) -> WorldLights {
        let grid = match &world.render {
            Some(r) => AmbientGrid::with_groups(&world.light_grid, &r.grids, 1.0),
            None => AmbientGrid::of(&world.light_grid),
        };
        let statics = world_lights(world);
        let sun = global_light(world)
            .map(|g| g.light())
            .filter(|l| l.colour.max_element() > 0.0);
        let reach = (Vec3::from(world.extent_max) - Vec3::from(world.extent_min)).length();
        WorldLights {
            grid,
            index: StaticLightIndex::new(&statics),
            sun_memo: SunMemo::default(),
            sun_objects: SunObjects::default(),
            dynamics: FxLightList::default(),
            statics,
            sun,
            sky: collision.map(|c| Arc::new(CollisionWorld::from_parts(c.models.clone(), &[]))),
            sky_reach: if reach.is_finite() && reach > 1.0 {
                reach
            } else {
                1.0e5
            },
        }
    }

    /// How much of the sun reaches `pos`: 1 when a ray from `pos` toward the sun ends on a sky polygon, else 0 (also
    /// when it leaves the world without a hit); 1 without geometry.
    // EVIDENCE: engine+0x1b1ba0 (segment from the position along the sun offset, hit polygon's sky flag; no hit: not
    // visible), engine+0x1b1cc0 (one test when `model_sun_variance` is 0, a bisection between two test points
    // otherwise); DC-DEFAULT: CLI-110 (proposed) one ray from the object position, as long as the world's extents
    // diagonal, no bisection (the soft edge of `model_sun_variance` 0.05 is not drawn); supersedes REN-7's factor 1
    pub fn sun_visibility(&self, pos: Vec3) -> f32 {
        let (Some(sun), Some(sky)) = (self.sun.as_ref(), self.sky.as_ref()) else {
            return 1.0;
        };
        let LightShape::Directional { dir } = sun.shape else {
            return 1.0;
        };
        let from = V3::new(pos.x, pos.y, pos.z);
        let to = from - V3::new(dir.x, dir.y, dir.z) * self.sky_reach;
        // client-fps: the pruned walk returns the same hit as `ray` (dc-sim's equality tests), faster for this long ray
        match sky.ray_pruned(from, to) {
            Some(h) if h.surface_flags & SKY_SURFACE_FLAG != 0 => 1.0,
            _ => 0.0,
        }
    }

    /// The light set of a model at object position `pos` with bounding radius `radius`: grid sample, static lights
    /// and the sun scaled by its visibility at `pos` (left out at 0).
    // SPEC: RENDER_2009.md §6.3 (light set); EVIDENCE: engine+0x1b21b0 (sun colour x visibility, inserted with its
    // convert-to-ambient fraction; grid at the object position; static lights in the box pos +- radius)
    pub fn light_set(&self, pos: Vec3, radius: f32) -> LightSet {
        self.light_set_with(pos, radius, || self.sun_memoised(pos))
    }

    /// The sun visibility at `pos` through the exact-position memo (render-core).
    fn sun_memoised(&self, pos: Vec3) -> f32 {
        let key = pos.to_array().map(f32::to_bits);
        match self.sun_memo.get(key) {
            Some(k) => k,
            None => {
                let k = self.sun_visibility(pos);
                self.sun_memo.put(key, k);
                k
            }
        }
    }

    /// [`WorldLights::light_set`] of a model object with the 2009 per-object sun cache (client-fps): the sun test
    /// runs when the object has no result yet or has moved more than [`SUN_VARIANCE`] + [`SUN_MOVE_EPSILON`] from
    /// the position of its last test (that position and the result are then kept); otherwise the kept result is
    /// used. The first light set of an object equals [`WorldLights::light_set`] at the same position.
    // EVIDENCE: engine+0x1b21b0 (DISASM engine+0x1b243b..0x1b24c8: cached result at object +0x7750 below 0 = none
    // yet; flag bit 0x10 of object +0x88 set and the squared distance from the kept position +0x7744..+0x774c above
    // (variance + 0.001)^2 = test again, keep position and result; else the kept result; engine+0x135e0 the distance
    // test); DC-DEFAULT: REN-60 (proposed) every model object the client draws is treated as one with the move
    // flag set (the flag's owner is not traced), see DC_DEFAULTS.md
    pub fn light_set_for(&self, object: SunObject, pos: Vec3, radius: f32) -> LightSet {
        self.light_set_with(pos, radius, || self.sun_cached(sun_key(object), pos))
    }

    /// The per-object sun cache step of [`WorldLights::light_set_for`].
    fn sun_cached(&self, key: u32, pos: Vec3) -> f32 {
        let kept = self
            .sun_objects
            .0
            .lock()
            .ok()
            .and_then(|m| m.get(&key).copied());
        if let Some(k) = kept_sun(kept, pos) {
            return k;
        }
        let k = self.sun_memoised(pos);
        if let Ok(mut m) = self.sun_objects.0.lock() {
            m.insert(key, (pos.to_array(), k));
        }
        k
    }

    /// The light set with the sun visibility from `vis` (asked only when the world has a sun).
    fn light_set_with(&self, pos: Vec3, radius: f32, vis: impl FnOnce() -> f32) -> LightSet {
        // render-core: the same lights in the same order as `light_set_unindexed` (the static candidates from the
        // load-time index, then the sun), without cloning the list; the sun ray is memoised by exact position
        let mut cand = Vec::new();
        self.index.query(pos, radius, &mut cand);
        let sun = self.sun.and_then(|sun| {
            let k = vis();
            (k > 0.0).then(|| ModelLight {
                colour: sun.colour * k,
                ..sun
            })
        });
        let dynamics = self.dynamics.0.read().ok();
        LightSet::build_with(
            pos,
            radius,
            Some(&self.grid),
            cand.iter()
                .map(|&i| &self.statics[i as usize])
                .chain(sun.as_ref()),
            dynamics.as_deref().map_or(&[], |d| d.as_slice()),
            &ModelLightSettings::default(),
        )
    }

    /// Replaces the dynamic lights every later light set of this frame takes (w2-fx: the effect lights of
    /// [`crate::fx::FxRunner::lights_at`]); an empty list removes them.
    // SPEC: RENDER_2009.md §6.3 (the model light set takes the dynamic lights of the visible-light list whose sphere
    // overlaps the model's), Appendix M M3 step 3
    pub fn set_fx_lights(&self, lights: Vec<ModelLight>) {
        if let Ok(mut d) = self.dynamics.0.write() {
            *d = lights;
        }
    }

    /// The dynamic lights of the frame (w2-fx).
    pub fn fx_lights(&self) -> Vec<ModelLight> {
        self.dynamics
            .0
            .read()
            .map(|d| d.clone())
            .unwrap_or_default()
    }

    /// Lighting inputs without a world file (tests, tools): the grid, the static lights and the sun as given; no
    /// geometry, so the sun is fully visible everywhere. The static-light index is built here as in
    /// [`WorldLights::of`] (render-core; engineering, no rule of its own).
    pub fn new(
        grid: AmbientGrid,
        statics: Vec<ModelLight>,
        sun: Option<ModelLight>,
    ) -> WorldLights {
        WorldLights {
            grid,
            index: StaticLightIndex::new(&statics),
            sun_memo: SunMemo::default(),
            sun_objects: SunObjects::default(),
            dynamics: FxLightList::default(),
            statics,
            sun,
            sky: None,
            sky_reach: 1.0e5,
        }
    }

    /// The reference path of [`WorldLights::light_set`]: every static light offered in list order and a fresh sun
    /// ray (kept for the equality test of the index).
    #[doc(hidden)]
    pub fn light_set_unindexed(&self, pos: Vec3, radius: f32) -> LightSet {
        let mut statics = self.statics.clone();
        if let Some(sun) = self.sun {
            let k = self.sun_visibility(pos);
            if k > 0.0 {
                statics.push(ModelLight {
                    colour: sun.colour * k,
                    ..sun
                });
            }
        }
        LightSet::build(
            pos,
            radius,
            Some(&self.grid),
            &statics,
            &self.fx_lights(),
            &ModelLightSettings::default(),
        )
    }
}

impl LoadedWorld {
    /// The surface type of a collision hit (model and polygon of a dc-sim hit); 0 for the blocker soup and unknown
    /// polygons.
    // DC-DEFAULT: CLI-14 (amended, proposed) the trailer surface and the footstep surface are the standing polygon's
    // surface type; blockers (player clips) have no surface record and report 0
    pub fn surface_type(&self, model: u32, poly: u32) -> u16 {
        self.surface_types
            .get(model as usize)
            .and_then(|m| m.get(poly as usize))
            .copied()
            .unwrap_or(0)
    }

    /// The surface type under a simulated body: 200 on a ladder, else the standing polygon's; `None` without a
    /// standing contact (the caller holds the last value).
    // SPEC: MOVEMENT_2009.md §10.2 (mover+0x50 := the surface type of the standing polygon; 200 on a ladder)
    // OBSERVED: capture 20261025_002236_tdm4/analysis/updates.csv (the 2009 client's airborne
    // updates mostly keep the last grounded surface: 319 of 360 with on_ground 0 equal it, 348 carry a non-zero
    // value; the deviations: some rising jumps report 0 (t 72.906, 73.312-73.516, 91.234-91.500) and some airborne
    // runs switch to the next surface before landing (90.156-90.641 10 -> 20, 98.875+ 71 -> 10), so the hold is an
    // approximation, not a proven rule)
    pub fn surface_under(&self, state: &dc_sim::PlayerState) -> Option<u8> {
        if state.on_ladder {
            return Some(crate::update::SURFACE_LADDER);
        }
        state
            .contact
            .map(|c| self.surface_type(c.model, c.poly).min(255) as u8)
    }

    /// The `n`-th spawn point.
    pub fn spawn(&self, n: usize) -> Result<SpawnPoint, ClientError> {
        self.spawns.get(n).copied().ok_or_else(|| {
            ClientError::Spawn(format!(
                "world {} has {} player spawn points, #{n} requested",
                self.name,
                self.spawns.len()
            ))
        })
    }

    pub fn summary(&self) -> String {
        let s = &self.scene_stats;
        format!(
            "{}: {} bytes, {} collision models, {} blockers, {} spawns, stand box {:?}, {} sections drawn, {} triangles, \
             textures missing {}, load {:.0} ms",
            self.name,
            self.file_bytes,
            self.collision.models.len(),
            self.collision.blocker_count(),
            self.spawns.len(),
            self.stand_dims.to_array(),
            s.sections_drawn,
            s.triangles_drawn,
            self.textures_missing,
            self.load_ms
        )
    }
}

/// One remote player as the client draws it this frame (interpolated from the object stream).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteBody {
    /// The player's world object.
    pub obj: u16,
    /// Roster id (the player's engine client id).
    pub pid: u8,
    pub name: String,
    /// 1 or 2 (0 / 0xff: no team), from the roster record of 0x6b.
    pub team: u8,
    /// Body centre.
    pub pos: [f32; 3],
    /// Half extents of the box drawn for it.
    pub half: [f32; 3],
    /// Facing (radians about +Y, the stream's `yaw8`), when known.
    pub yaw: Option<f32>,
    /// What the character looks like and how it is posed (models from the server's records), when known.
    pub look: Option<RemoteLook>,
}

/// One model of a remote character as the server named it: the model file and its textures (resource paths from
/// the session's file table).
// SPEC: UDP_SESSION_2009_B.md §3.4 (model file block), §3.5 (character: body entry with its child model, textures,
// styles; face entry; weapon child object at socket 2)
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelRef {
    pub model: String,
    pub textures: Vec<String>,
}

/// One playing animation tracker of a remote character (stream `UF_ANIM`): the composed clip index, the time in
/// the clip (ms, advanced since the entry arrived) and the weight set (`None` = the main tracker).
// SPEC: UDP_SESSION_2009_B.md §4.3 (three trackers: main, tracker 1 with weight set 0, tracker 2 with weight set 1),
// §4.4 (animation block)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteTracker {
    pub clip: usize,
    pub time_ms: f32,
    pub blend_set: Option<usize>,
}

/// The models and pose of a remote character.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteLook {
    /// Model entries of the character object in block order (body first, then the face).
    pub models: Vec<ModelRef>,
    /// The render styles the server's model block names for the body entry (empty: the table row's styles). The
    /// infected look of class 11 arrives this way: the override row's styles over the normal body model.
    // SPEC: UDP_SESSION_2009_B.md §3.4 (model file block: per entry the model, textures and render-style file ids)
    pub styles: Vec<String>,
    /// The held weapon's model (the attached weapon object) and its socket on the body.
    pub weapon: Option<(ModelRef, u32)>,
    pub trackers: Vec<RemoteTracker>,
    /// False for the hidden form of a character object (flag word 0x0802 / 0x2802 before its spawn).
    // SPEC: UDP_SESSION_2009_B.md §3.5 (0x0802 hidden form before the spawn, 0x3803 spawned; bit 0x0001 "visible"
    // PLAUSIBLE)
    pub visible: bool,
    /// The drawn upper-body aim (pitch, yaw offset; radians) from the server's aim notes, when any arrived
    /// ([`crate::aim`]).
    // EVIDENCE: cshell+0x26f60 (shell 0xa6 type 0x18 subs 0x80 / 0xc0)
    pub aim: Option<[f32; 2]>,
}

/// What a server-backed source hands the frame loop besides corrections; local sources return the default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameState {
    /// The own player state of shell 0x67 (`None`: no server).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.1; MOVEMENT_2009.md §12.5
    pub life_state: Option<u8>,
    /// View angle sets of shell 0x6a in arrival order: (kind, pitch / yaw / roll).
    // SPEC: GAME_MESSAGES_2009_S2C.md §4.2 (kind 0 all three, kind 1 yaw only)
    pub view_sets: Vec<(u8, [f32; 3])>,
    /// Remote players to draw.
    pub remotes: Vec<RemoteBody>,
    /// Status lines for the overlay (health, state, score, kill feed, round end).
    pub hud: Vec<String>,
    /// The game session is over (the frame loop may keep running).
    pub session_over: bool,
    /// The own held weapon for the first-person view (`None`: none drawn).
    pub view_weapon: Option<ViewWeapon>,
    /// The scope hides the first-person weapon and its mod models ([`crate::zoom::Zoom::weapons_hidden`]); it is
    /// still placed (the muzzle socket of the flash).
    pub view_hidden: bool,
    /// HUD inputs since the last frame, with the time (ms of `clock_ms`'s clock) they arrived.
    pub hud_events: Vec<(u32, crate::hud::state::HudEvent)>,
    /// The held weapon as the HUD's ammunition block shows it.
    pub ammo: Option<crate::hud::state::AmmoView>,
    /// The source's clock (ms) for the HUD's timers (`None`: the frame loop's own time).
    pub clock_ms: Option<u32>,
    /// The step height the server set with `S2C_EVAR` (`None`: not sent, or no server).
    // SPEC: MOVEMENT_2009.md §2 (`STEP_HEIGHT`, read through the server-variable table), §8.1 step 2
    pub step_height: Option<f32>,
    /// The server's sprint permission (shell 0xca `[allowed]`). A server-backed source always gives `Some`: false
    /// before its session exists and until the first 0xca, then the last value; `None` comes only from a local
    /// source (no server), where sprinting is not gated.
    // SPEC: MOVEMENT_2009.md §7 step 2; GAME_MESSAGES_2009_S2C.md §3.2; DC-DEFAULT: CLI-58 (proposed)
    pub sprint_allowed: Option<bool>,
    /// The held weapon's aim_mark ([`crate::spread::AimMarkView`]; `None`: none drawn).
    pub aim_mark: Option<crate::spread::AimMarkView>,
    /// The mod models on the own first-person weapon ([`crate::attach`]).
    pub view_attach: Vec<crate::attach::WeaponAttach>,
    /// The third-person mod models on the remote characters' weapons, by character object (CLI-618).
    pub remote_attach: std::collections::BTreeMap<u16, Vec<crate::attach::WeaponAttach>>,
    /// The pickups attached to the characters (a carried capture flag: its model and the body socket index), by
    /// character object (CLI-1072).
    pub remote_carried: std::collections::BTreeMap<u16, Vec<(ModelRef, u32)>>,
    /// The gear each character's look record names (CLI-1231), by character object.
    pub remote_gear: std::collections::BTreeMap<u16, crate::menu::figure::GearIds>,
    /// The capture flags and the own team's capture zones for the in-world flag markers (CLI-1073).
    pub hud_flags: Vec<HudFlag>,
    /// The capture flags on the ground whose create names no model (MAP-34): drawn as a bar in the team colour.
    pub flag_markers: Vec<FlagMarker>,
    /// The held weapon hides the aim cross (`DisableCrosshair`, [`crate::arms::Arsenal::aim_cross_hidden`]).
    pub aim_cross_hidden: bool,
    /// The held weapon's aim picture (`DotSightName`, [`crate::arms::Arsenal::aim_picture`]; CLI-630).
    pub aim_picture: Option<String>,
    /// The own scope overlay slots ([`crate::hud::scope::ScopeView`]).
    pub scope: crate::hud::scope::ScopeView,
    /// Sound records, relays, shot effects, preload lists and own weapon sounds since the last frame.
    pub sound_cues: Vec<crate::sound::SoundCue>,
    /// The own character object (the server streams it to its owner, DSV-23) with its models and trackers.
    pub own_object: Option<u16>,
    pub own_look: Option<RemoteLook>,
    /// HiSec cases in the world (the HUD draws their indicators).
    pub hud_cases: Vec<HudCase>,
    /// The own death while the player is in a not-playing state after a 0x67 state 2 (`None`: alive, or no
    /// server): the death camera follows it.
    pub death: Option<DeathView>,
    /// The freeze and release blocks of the server's mover updates (0xa0 flags 0x0080 / 0x0100) since the last
    /// frame, in arrival order.
    pub mover: Vec<MoverNote>,
    /// The dropped weapons in the world (type 0x17 kind-6 pickup objects, [`crate::drops`]).
    pub drops: Vec<crate::drops::WeaponDrop>,
    /// Weapon-effect events since the last frame: the own pellets and the server's shot records ([`crate::fx`]).
    pub fx_events: Vec<crate::fx::FxEvent>,
    /// The thrown objects in the world (grenades in flight or at rest), drawn with the model pass.
    pub thrown: Vec<RemoteBody>,
    /// Corpses to draw and the play-once trackers of the drawn objects ([`crate::anims`]).
    pub anims: crate::anims::AnimFrame,
    /// The server's named brushes and placed effects (`None`: a local source, the world keeps the file's own
    /// visibility). A server-backed source always gives `Some` (empty before its object group arrived).
    // DC-DEFAULT: CLI-230 (proposed)
    pub world_objects: Option<crate::objects::ServerObjects>,
    /// The live world camera (a type-0x0d object with the live bit, [`crate::camshot`]); `None`: the own view.
    // DC-DEFAULT: CLI-240 (proposed)
    pub cam_shot: Option<crate::camshot::CamShotView>,
    /// The server's sky table with its objects resolved this frame (`None`: not received, or a local source; the
    /// sky pass then keeps the viewer's default).
    pub sky: Option<crate::objects::SkyTable>,
    /// The local player's team from the roster (`None`: unknown or a local source).
    pub own_team: Option<u8>,
    /// The weapon the own player holds now (`None`: none or a local source); the mover's speed rows follow it.
    // EVIDENCE: cshell+0x5c270 (the speed penalty reads the held weapon's row, slot +0xc0 of the object at VA
    // 0x3759e110)
    pub held_weapon: Option<u16>,
    /// The own equipment from the lobby (item number per equipment slot, -1 empty; empty: unknown or a local
    /// source); the mover's head-gear, backpack and goggle speed rows follow it (CLI-1137).
    pub equipment: Vec<i32>,
    /// w2-sprites: the billboards in the world (kind-3 sprite objects, [`crate::sprites`]); drawn with the effect
    /// quads.
    pub billboards: Vec<crate::sprites::Billboard>,
}

/// The own death as the frame loop's death camera needs it.
// SPEC: GAME_MESSAGES_2009_S2C.md §4.1 (0x67 state 2 stores the death time, state 1 clears it), §3.3.1 effect 3 (the
// last killer of the local player); GAME_MESSAGES_2009_C2S.md §3.3 (cshell+0x60ac0: killer camera when a last killer
// is known, else the follow camera)
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DeathView {
    /// Milliseconds since the 0x67 state 2.
    pub since_ms: u32,
    /// The client id of the player who killed the local player (`None`: none, the world, or himself).
    pub killer: Option<u8>,
    pub own_pid: Option<u8>,
    pub own_team: Option<u8>,
    /// The `from_dir` of the last 0x74 taken while playing (the shot that killed, as a rule): the killer camera
    /// moves and looks back along it (`None`: no damage message seen).
    pub shot_dir: Option<[f32; 3]>,
    /// The follow camera may take any player, not only team mates: game classes 6 and 9 (and a player without a
    /// team), cshell+0x60010.
    pub any_team: bool,
    /// Players this client saw die and not come back yet: the follow camera skips them (the roster search's dead
    /// flag, cshell+0x29090).
    pub dead: PidSet,
}

/// A set of roster ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PidSet([u64; 4]);

impl PidSet {
    pub fn insert(&mut self, pid: u8) {
        self.0[(pid >> 6) as usize] |= 1 << (pid & 63);
    }

    pub fn contains(&self, pid: u8) -> bool {
        self.0[(pid >> 6) as usize] & (1 << (pid & 63)) != 0
    }
}

/// One freeze / release block of a `GS_MOVER_UPDATE` (0xa0).
// SPEC: MOVEMENT_2009.md §12.4 (0x0080 `[freeze:1 bit]`, 0x0100 `[release:1 bit]`)
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MoverNote {
    pub freeze: Option<bool>,
    pub release: Option<bool>,
    /// The client's own plant / defuse hold (CLI-1250; no wire block): `None` for a server block.
    pub hold: Option<HoldNote>,
    /// DSV-483: a Possession move: the end point and its length in ms (our shell 0xf2, no 0xa0 block).
    pub glide: Option<([f32; 3], u16)>,
}

/// The own plant / defuse hold as the mover sees it (CLI-1250).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldNote {
    /// The hold was sent (phase 1) at a point the server takes: the body stops at once.
    Start,
    /// The hold ended on this client (key let go, death): the own stop ends.
    End,
    /// The round result came: the server's round-end stop takes over, the own stop is no longer ended by the client.
    HandOver,
}

/// A capture flag or capture zone for the in-world flag markers (CLI-1073).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudFlag {
    /// The flag's team (or the zone's).
    pub team: u8,
    /// A capture zone (base) rather than a flag.
    pub base: bool,
    pub pos: [f32; 3],
    /// A flag's return countdown in seconds (the last note sent to its object; 0: none).
    pub remaining: i32,
}

/// A HiSec case object as the HUD needs it: its case key, world position and the last countdown it was sent.
// SPEC: HISEC_2009.md §3.1 (type 0x17 kind 5 key, sub 2 countdown), §3.3
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudCase {
    pub key: u32,
    pub pos: [f32; 3],
    pub remaining: i32,
}

/// The weapon the first-person view draws: its first-person model, textures and the clip it plays.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewWeapon {
    pub weapon: u16,
    pub model: Option<ModelRef>,
    /// Composed clip index and time (ms) into it.
    pub clip: Option<(usize, f32)>,
    /// The inspect pose this frame (DC-DEFAULT: CLI-1845).
    pub inspect: Option<crate::inspect::ViewPose>,
    /// The weapon row's first-person model position (eye space; zero without a row).
    pub pos: [f32; 3],
}

/// What the frame loop hands the weapon logic each frame.
pub struct ArmsFrame<'a> {
    pub commands: crate::arms::WeaponCommands,
    pub aim: crate::arms::Aim,
    /// The own body position (the use key's reach is measured from it, CLI-1795).
    pub body: [f32; 3],
    /// The remote players drawn this frame (their boxes are the shot targets).
    pub remotes: &'a [RemoteBody],
    pub world: &'a CollisionWorld,
    /// The player's movement for the weapon's spread ([`crate::spread::Motion`]).
    pub motion: crate::spread::Motion,
    /// The loaded world of the frame (CLI-250): the shot trace meets its shot-only world models
    /// ([`LoadedWorld::shot_world`]) and resolves the own pellets' impacts at the moment of the shot.
    pub loaded: Option<&'a LoadedWorld>,
    /// The frame's play-once trackers (the shot test poses the characters as the character pass draws them,
    /// CLI-300; `None`: every tracker loops).
    pub anims: Option<&'a crate::anims::AnimFrame>,
    /// The normal horizontal field of view of the frame (radians; 0 = keep the last one): the zoom changes run
    /// against it ([`crate::zoom`]).
    pub normal_fov_x: f32,
}

/// The scope's part of a frame ([`crate::zoom`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ZoomFrame {
    /// The zoom level (the mouse divides by `3 z + 1`).
    pub level: u32,
    /// Level >= 1 or a change running (blocks the sprint speed, `pu_flags` 0x0800).
    pub active: bool,
    /// The horizontal field of view (radians), `None` for the normal one; the vertical one follows in proportion.
    pub fov_x: Option<f32>,
}

impl ZoomFrame {
    /// `camera` with the zoomed field: the horizontal angle := `fov_x`, the vertical one scaled by the same ratio.
    // EVIDENCE: cshell+0x1f9100 (FovY x zoom field / FovX)
    pub fn apply(&self, camera: &mut dc_render::Camera) {
        if let Some(fx) = self.fov_x.filter(|f| f.is_finite() && *f > 0.0) {
            if camera.fov_x > 0.0 {
                camera.fov_y *= fx / camera.fov_x;
                camera.fov_x = fx;
            }
        }
    }
}

/// What the weapon step hands back to the frame loop.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ArmsResult {
    /// Recoil view pitch change (radians, positive looks down) to add to the view.
    pub view_pitch: f32,
    /// Recoil view yaw change (radians) to add to the view ([`crate::recoil`]).
    pub view_yaw: f32,
    /// The camera's angle offsets of this frame (the hit jar, [`crate::camfx`]): the drawn view only, never the
    /// view the client reports.
    pub camera: crate::camfx::CamAngles,
    /// How far the recoil's kick-back moves the eye back along the view (units).
    pub eye_back: f32,
    /// The scope of the frame.
    pub zoom: ZoomFrame,
}

/// The world side of the client.
pub trait WorldSource {
    /// A chat line the player typed (`UP_CHAT`); a server-backed source sends it, local sources drop it.
    fn chat(&mut self, _line: &str, _mode: crate::hud::chat::ChatScope) {}
    /// Short description for logs.
    /// A frame for the lobby connection the session keeps during a match (the in-match report, CLI-950).
    fn lobby_frame(&mut self, _frame: dc_wire::PktWriter) {}
    /// The network overlay's numbers now (CLI-1571; `players`: with the per-player rows); `None` without a network
    /// session.
    fn net_sample(&mut self, _players: bool) -> Option<crate::hud::netview::NetSample> {
        None
    }

    fn describe(&self) -> String;
    /// Loads the world; with `gpu`, also builds and uploads the render scene.
    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError>;
    /// gpu-recover: loads the world already loaded once more onto a new device (after a GPU device loss), for its
    /// render scene only ([`LoadedWorld::adopt_scene`]); no session bookkeeping. The default loads it again.
    fn reload_scene(&mut self, gpu: GpuTarget<'_>) -> Result<LoadedWorld, ClientError> {
        self.load(Some(gpu))
    }
    /// Corrections the authority wants applied before the next tick (shell 0xa0 / 0x0020 from a server).
    /// Local sources have none.
    // SPEC: MOVEMENT_2009.md §12.4 (teleport and new box)
    fn poll_corrections(&mut self) -> Vec<Correction> {
        Vec::new()
    }
    /// The local player's update (a server-backed source sends it; local sources drop it).
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.1
    fn report(&mut self, _update: &LocalUpdate) {}
    /// The movement events of one simulation tick (jump, fall, landing); a server-backed source sends them as the
    /// movement notices 0x76 [1][e], local sources drop them.
    // SPEC: GAME_MESSAGES_2009_C2S.md §2.2
    fn moved(&mut self, _events: &StepEvents) {}
    /// Per-frame state from the authority: player state, view sets, remote players, status lines.
    fn frame_state(&mut self) -> FrameState {
        FrameState::default()
    }

    /// [`WorldSource::frame_state`] for a frame whose render time is `lead_ms` ahead of (negative: behind) the
    /// source's clock now (CLI-1576: the predicted presentation time); a source without a clock ignores it.
    fn frame_state_led(&mut self, _lead_ms: f32) -> FrameState {
        self.frame_state()
    }
    /// The frame's weapon commands and aim; a server-backed source runs the weapon and sends its messages
    /// (C2S §4), local sources ignore them.
    fn arms(&mut self, _frame: &ArmsFrame<'_>) -> ArmsResult {
        ArmsResult::default()
    }
    /// The user's ROM folder the models of remote characters and the own weapon are read from (`None`: no models,
    /// remote players are drawn as boxes).
    fn rom(&self) -> Option<PathBuf> {
        None
    }
    /// The 2009 "Reality" option of the session (the HUD's damage arcs and the camera jar); off for local sources.
    fn reality(&self) -> bool {
        false
    }
    /// Whether a drawn round plays a draw voice on the HUD (no 2009 voice exists); off for local sources.
    // DC-DEFAULT: HUD-SND-2 (staged) off
    fn draw_voice(&self) -> bool {
        false
    }
    /// The input this source drives the player with when the caller has none of its own (`None`: the caller's
    /// default). The stress scenarios hold their pinned view or walk (perf-g1).
    fn scripted_input(
        &mut self,
        _start: crate::look::ViewAngles,
    ) -> Option<Box<dyn crate::input::InputSource>> {
        None
    }
}

fn spawns_of(world: &World<'_>) -> Vec<SpawnPoint> {
    world
        .objects_with_role(ObjectRole::PlayerSpawn)
        .filter_map(|o| {
            Some(SpawnPoint {
                pos: V3::from_array(o.position()?),
                yaw: o.rotation().map_or(0.0, |r| r[1]),
            })
        })
        .collect()
}

fn floor_limit(cw: &CollisionWorld) -> f32 {
    cw.models
        .iter()
        .map(|m| m.bounds().0.y)
        .fold(f32::INFINITY, f32::min)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// What [`build`] makes a world from.
struct BuildSpec<'a> {
    name: String,
    data: &'a [u8],
    stand_dims: V3,
    crouch_dims: Option<V3>,
    extra_spawns: &'a [SpawnPoint],
    filter: &'a WorldFilter,
    scene_opts: SceneOptions,
}

/// Collision set, spawns and (optionally) the uploaded scene of one parsed world.
fn build(
    spec: BuildSpec<'_>,
    gpu: Option<GpuTarget<'_>>,
    traits: &mut dyn FnMut(&[u8]) -> TextureTraits,
    images: &mut dyn FnMut(&[u8]) -> Option<TextureImage>,
) -> Result<LoadedWorld, ClientError> {
    let BuildSpec {
        name,
        data,
        stand_dims,
        crouch_dims,
        extra_spawns,
        filter,
        scene_opts,
    } = spec;
    let t = Instant::now();
    let opts = ParseOptions {
        render: gpu.is_some(),
    };
    let parse = dc_render::perf::span(dc_render::perf::Counter::ParseNs);
    let world = World::parse_with(data, opts).map_err(|e| ClientError::World(e.to_string()))?;
    drop(parse);
    let collision = collision_world(&world, filter);
    let containers = ladder_volumes(&world);
    let surface_types: Vec<Vec<u16>> = selected_models(&world, filter)
        .into_iter()
        .map(|i| {
            let m = &world.models[i];
            m.polygons
                .iter()
                .map(|p| m.surfaces.get(p.surface as usize).map_or(0, |s| s.texflags))
                .collect()
        })
        .collect();
    let mut spawns = spawns_of(&world);
    spawns.extend_from_slice(extra_spawns);
    let model_lights = gpu
        .is_some()
        .then(|| WorldLights::of(&world, Some(&collision)));
    let (scene, scene_stats, upload): (Option<GpuScene>, SceneStats, Option<UploadStats>) =
        match gpu {
            Some(g) => {
                // perf-g1: the scene build packs the GPU vertices (its texture-header reads also count as I/O)
                let pack = dc_render::perf::span(dc_render::perf::Counter::PackNs);
                let mesh = SceneMesh::build(&world, scene_opts, traits);
                drop(pack);
                let stats = mesh.stats;
                let scene = g.renderer.upload(g.ctx, &mesh, images);
                let up = scene.stats;
                (Some(scene), stats, Some(up))
            }
            None => (None, SceneStats::default(), None),
        };
    let world_models =
        crate::objects::WorldModels::of(&world, filter, scene.as_ref().map(|s| s.model_names()));
    Ok(LoadedWorld {
        world_models,
        name,
        floor_limit: floor_limit(&collision),
        extents: (world.extent_min, world.extent_max),
        collision,
        containers,
        spawns,
        stand_dims,
        crouch_dims,
        scene,
        scene_stats,
        upload,
        textures_missing: 0,
        file_bytes: data.len(),
        load_ms: ms(t),
        model_lights,
        surface_types,
        sky_applied: None,
        far_z: dc_render::world_far_z(&world),
    })
}

/// sha256(stand animation name)[:8] and its length (MOVEMENT_2009.md §9.2 key id 7f007bf2ff2bcc78/3).
// SPEC: MOVEMENT_2009.md §9.2 (stand_dims = user dims of the model animation with that key)
const STAND_KEY: ([u8; 8], usize) = ([0x7f, 0x00, 0x7b, 0xf2, 0xff, 0x2b, 0xcc, 0x78], 3);

/// The standing half extents of the user's player models: the extent of the first clip (archives and
/// entries in name order) whose name hashes to the stand key. OBSERVED (MOVEMENT_2009 addendum A7): three
/// player models carry it and their boxes agree.
// SPEC: MOVEMENT_2009.md §9.2; FMT_LTB.md (clip records, through dc-model)
pub fn rom_stand_dims(rom: &Path) -> Result<V3, ClientError> {
    rom_stance_dims(rom).map(|d| d.0)
}

/// sha256(lower-body crouch idle clip name)[:8] and its length.
// OBSERVED: ROM player models (every crouch clip of the model carrying the stand clip has one box, lower than the
// stand clip's); DC-DEFAULT: SIM-25 (proposed)
const CROUCH_KEY: ([u8; 8], usize) = ([0x0a, 0x98, 0x0e, 0x22, 0x63, 0x73, 0x50, 0x01], 2);

/// The standing box ([`rom_stand_dims`]) and the crouch box: the crouch clip's extent of the first player model
/// (same order) that carries both the stand clip and the crouch clip (`None` when no model does). The first model
/// with the stand clip need not carry the crouch clip (the user's ROM: it does not), so the search goes on past it.
// DC-DEFAULT: SIM-25 (proposed); SIM-26 (proposed) crouch box from the first model carrying both clips
pub fn rom_stance_dims(rom: &Path) -> Result<(V3, Option<V3>), ClientError> {
    let game = rom.join("Game");
    let mut archives: Vec<PathBuf> = std::fs::read_dir(&game)
        .map_err(|e| ClientError::Rom(format!("{}: {e}", game.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rez")))
        .collect();
    archives.sort();
    let mut stand: Option<V3> = None;
    for path in archives {
        let arch = dc_rez::RezArchive::open(&path).map_err(|e| ClientError::Rom(e.to_string()))?;
        let mut entries: Vec<&(String, u64, u32)> = arch
            .entries()
            .iter()
            .filter(|(n, _, _)| n.to_ascii_lowercase().ends_with(".ltb"))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, off, size) in entries {
            let Ok(bytes) = arch.read_raw(*off, *size) else {
                continue;
            };
            let Ok(model) = dc_model::Model::parse(&bytes) else {
                continue;
            };
            let key =
                |n: &[u8], k: ([u8; 8], usize)| n.len() == k.1 && dc_rom::sha256(n)[..8] == k.0;
            let Some(s) = model.clips.iter().find(|c| key(&c.name.0, STAND_KEY)) else {
                continue;
            };
            let first = *stand.get_or_insert(V3::from_array(s.extent));
            if let Some(c) = model.clips.iter().find(|c| key(&c.name.0, CROUCH_KEY)) {
                return Ok((first, Some(V3::from_array(c.extent))));
            }
        }
    }
    stand
        .map(|s| (s, None))
        .ok_or_else(|| ClientError::Rom("no player model with the stand clip in the ROM".into()))
}

/// A map of the user's ROM.
pub struct RomWorld {
    pub rom: PathBuf,
    pub map: String,
    pub scene: SceneOptions,
    pub filter: WorldFilter,
    stand: Option<(V3, Option<V3>)>,
}

impl RomWorld {
    pub fn new(rom: &Path, map: &str) -> RomWorld {
        RomWorld {
            rom: rom.to_path_buf(),
            map: map.to_string(),
            scene: SceneOptions::default(),
            filter: WorldFilter::default(),
            stand: None,
        }
    }

    /// Map names of the user's world archive, sorted.
    pub fn map_names(rom: &Path) -> Result<Vec<String>, ClientError> {
        Ok(dc_render::WorldSource::open(rom)
            .map_err(|e| ClientError::Rom(e.to_string()))?
            .map_names())
    }

    /// The stand box (read from the ROM once, then cached).
    pub fn stand_dims(&mut self) -> Result<V3, ClientError> {
        self.stance_dims().map(|d| d.0)
    }

    /// The stand box and the crouch box (read from the ROM once, then cached).
    pub fn stance_dims(&mut self) -> Result<(V3, Option<V3>), ClientError> {
        if let Some(d) = self.stand {
            return Ok(d);
        }
        let d = rom_stance_dims(&self.rom)?;
        self.stand = Some(d);
        Ok(d)
    }
}

impl WorldSource for RomWorld {
    fn describe(&self) -> String {
        format!("ROM map {}", self.map.to_ascii_uppercase())
    }

    fn rom(&self) -> Option<PathBuf> {
        Some(self.rom.clone())
    }

    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        let t = Instant::now();
        let (stand, crouch) = self.stance_dims()?;
        let worlds =
            dc_render::WorldSource::open(&self.rom).map_err(|e| ClientError::Rom(e.to_string()))?;
        let data = worlds
            .read(&self.map)
            .map_err(|e| ClientError::Rom(e.to_string()))?
            .ok_or_else(|| ClientError::NoSuchMap(self.map.clone()))?;
        let index = match gpu {
            Some(_) => {
                TextureIndex::open(&self.rom).map_err(|e| ClientError::Rom(e.to_string()))?
            }
            None => TextureIndex::default(),
        };
        let tex = std::cell::RefCell::new(RomTextures::new(&index));
        let mut traits = |name: &[u8]| tex.borrow_mut().traits(name);
        let mut images = |name: &[u8]| {
            let mut t = tex.borrow_mut();
            let img = t.image(name);
            t.release(name);
            img
        };
        let spec = BuildSpec {
            name: self.map.to_ascii_uppercase(),
            data: &data,
            stand_dims: stand,
            crouch_dims: crouch,
            extra_spawns: &[],
            filter: &self.filter,
            scene_opts: self.scene,
        };
        let mut w = build(spec, gpu, &mut traits, &mut images)?;
        w.textures_missing = tex.borrow().missing;
        w.load_ms = ms(t);
        Ok(w)
    }
}

/// A made-up world (bytes of a version-85 file, for example from `dc_world::synth`), with explicit spawn
/// points, a made-up standing box and named solid-colour textures.
pub struct SynthSource {
    pub name: String,
    pub bytes: Vec<u8>,
    pub spawns: Vec<SpawnPoint>,
    pub stand_dims: V3,
    pub textures: Vec<(Vec<u8>, [u8; 4])>,
}

impl WorldSource for SynthSource {
    fn describe(&self) -> String {
        format!("synthetic world {}", self.name)
    }

    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        let textures = self.textures.clone();
        let mut traits = |_: &[u8]| TextureTraits {
            present: true,
            mode: 0,
            hi: false,
        };
        let mut images = |name: &[u8]| {
            textures
                .iter()
                .find(|(n, _)| n.as_slice() == name)
                .map(|(_, c)| TextureImage::rgba(2, 2, c.repeat(4)))
        };
        let filter = WorldFilter::default();
        let spec = BuildSpec {
            name: self.name.clone(),
            data: &self.bytes,
            stand_dims: self.stand_dims,
            crouch_dims: None,
            extra_spawns: &self.spawns,
            filter: &filter,
            scene_opts: SceneOptions::default(),
        };
        build(spec, gpu, &mut traits, &mut images)
    }
}

/// A community map (MAPKIT_PLAN.md 6.5, MAP-30): the bytes of a `@mod:` world with its section textures read in
/// layers, the mod set first (`@mod:<id>/assets/...` names and `overrides/` of ROM paths through
/// [`crate::modassets::ModAssets`]) and the ROM's texture index for the rest. A name no layer answers counts as
/// missing, as a ROM map's missing texture does (the section draws flat). The standing box comes from the ROM's
/// player models when a ROM is at hand, else the made-up box of CLI-9 until the server's first teleport.
///
/// The standalone preview (`dc-client --world-file <dat> [--rom DIR] [--mod-dir DIR]`, MAP-33) is the same source
/// with `any_id` (every `@mod:<id>` names the one folder given) and `fallback_colours` (a name no layer answers
/// draws a solid colour derived from the name, so the geometry is visible without a ROM).
// DC-DEFAULT: MAP-30 (proposed) the `@mod:` world source; MAP-33 (proposed) the standalone preview, see DC_DEFAULTS.md
pub struct ModWorld {
    /// The name the session reports (the announced path, or the file stem of a preview).
    pub name: String,
    pub bytes: Vec<u8>,
    /// The ROM for textures, models and fonts (`None`: none at hand).
    pub rom: Option<PathBuf>,
    /// The mod files (`None`: no mod layer; every `@mod:` name is then missing).
    pub assets: Option<Arc<crate::modassets::ModAssets>>,
    /// Preview: `@mod:<any id>/assets/<rel>` reads `<rel>` from the first mod of `assets`.
    pub any_id: bool,
    /// Preview: a texture no layer answers draws a solid colour instead of counting as missing.
    pub fallback_colours: bool,
    pub scene: SceneOptions,
    pub filter: WorldFilter,
    stand: Option<(V3, Option<V3>)>,
}

impl ModWorld {
    /// The world a server announced as `@mod:` (the match's loader).
    pub fn new(
        name: &str,
        bytes: Vec<u8>,
        rom: Option<PathBuf>,
        assets: Option<Arc<crate::modassets::ModAssets>>,
    ) -> ModWorld {
        ModWorld {
            name: name.to_string(),
            bytes,
            rom,
            assets,
            any_id: false,
            fallback_colours: false,
            scene: SceneOptions::default(),
            filter: WorldFilter::default(),
            stand: None,
        }
    }

    /// The standalone preview of a world file: textures from the ROM by name when one is given, from
    /// `<mod_dir>/assets/...` for `@mod:<any id>/assets/...` names, solid colours for the rest.
    pub fn preview(
        path: &Path,
        rom: Option<PathBuf>,
        mod_dir: Option<&Path>,
    ) -> Result<ModWorld, ClientError> {
        let bytes =
            std::fs::read(path).map_err(|e| ClientError::Io(format!("{}: {e}", path.display())))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "WORLD".into());
        let assets = mod_dir.map(|d| {
            Arc::new(crate::modassets::ModAssets::new([(
                "preview".to_string(),
                d.to_path_buf(),
            )]))
        });
        let mut w = ModWorld::new(&name, bytes, rom, assets);
        w.any_id = true;
        w.fallback_colours = true;
        Ok(w)
    }

    /// The stand box and the crouch box: the ROM's player models when a ROM is at hand (read once), else the
    /// made-up box of CLI-9 (the server's teleport box replaces it).
    // DC-DEFAULT: CLI-9 (proposed) 16 x 40 x 16 half extents for worlds not from the ROM
    pub fn stance_dims(&mut self) -> Result<(V3, Option<V3>), ClientError> {
        if let Some(d) = self.stand {
            return Ok(d);
        }
        let d = match &self.rom {
            Some(rom) => rom_stance_dims(rom)?,
            None => (V3::new(16.0, 40.0, 16.0), None),
        };
        self.stand = Some(d);
        Ok(d)
    }
}

/// The mod layer of a [`ModWorld`]'s texture names: `Some(Some(bytes))` a mod file, `Some(None)` a `@mod:` name
/// that resolves to nothing (never the ROM), `None` the ROM serves the name.
fn mod_texture_bytes(
    assets: Option<&crate::modassets::ModAssets>,
    any_id: bool,
    name: &[u8],
) -> Option<Option<Vec<u8>>> {
    let text = String::from_utf8_lossy(name);
    let text = text.trim();
    let is_mod = crate::modassets::is_mod_path(text);
    let Some(assets) = assets else {
        return is_mod.then_some(None);
    };
    let path = match (is_mod, any_id, assets.first_id()) {
        (true, true, Some(first)) => {
            // the preview folder stands for every mod id: `@mod:<x>/<rel>` -> `@mod:<first>/<rel>`
            let rest = &text[crate::modassets::MOD_SCHEME.len()..];
            match rest.split_once('/') {
                Some((_, rel)) => format!("{}{first}/{rel}", crate::modassets::MOD_SCHEME),
                None => text.to_string(),
            }
        }
        _ => text.to_string(),
    };
    match assets.read(&path) {
        Some(Ok(b)) => Some(Some(b)),
        Some(Err(e)) => {
            log_texture_refusal(&e);
            Some(None)
        }
        None => is_mod.then_some(None),
    }
}

fn log_texture_refusal(e: &str) {
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    if SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(e.to_string())
    {
        eprintln!("mods: world texture refused: {e}");
    }
}

/// A solid colour for a texture name no layer answers (the preview, MAP-33): three mid-range channels from the
/// name's bytes, so the same name draws the same colour on every run and two names rarely share one.
pub fn fallback_colour(name: &[u8]) -> [u8; 4] {
    let mut h: u32 = 0x811c_9dc5;
    for b in name {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let ch = |shift: u32| 80 + ((h >> shift) & 0x7f) as u8;
    [ch(0), ch(8), ch(16), 255]
}

/// The layered texture reader of a [`ModWorld`] load: the mod files first, the ROM index second, then (preview)
/// a solid colour; parsed files are cached by name.
struct LayeredTextures<'i> {
    assets: Option<Arc<crate::modassets::ModAssets>>,
    any_id: bool,
    fallback_colours: bool,
    rom: RomTextures<'i>,
    own: std::collections::HashMap<Vec<u8>, Option<dc_tex::DtxFile>>,
}

enum Layered<'a> {
    Own(&'a dc_tex::DtxFile),
    Rom,
    Missing,
}

impl LayeredTextures<'_> {
    fn lookup(&mut self, name: &[u8]) -> Layered<'_> {
        if !self.own.contains_key(name) {
            let parsed = match mod_texture_bytes(self.assets.as_deref(), self.any_id, name) {
                Some(Some(bytes)) => Some(dc_tex::DtxFile::parse(&bytes).ok()),
                Some(None) => Some(None),
                None => None,
            };
            match parsed {
                Some(p) => {
                    self.own.insert(name.to_vec(), p);
                }
                None => return self.rom_lookup(name),
            }
        }
        match self.own.get(name).and_then(|d| d.as_ref()) {
            Some(d) => Layered::Own(d),
            None => Layered::Missing,
        }
    }

    fn rom_lookup(&mut self, name: &[u8]) -> Layered<'_> {
        if self.rom.get(name).is_some() {
            Layered::Rom
        } else {
            Layered::Missing
        }
    }

    fn traits(&mut self, name: &[u8]) -> TextureTraits {
        let fallback = self.fallback_colours;
        match self.lookup(name) {
            Layered::Own(d) => dc_render::traits_of(d),
            Layered::Rom => self.rom.traits(name),
            Layered::Missing => TextureTraits {
                present: fallback,
                mode: 0,
                hi: false,
            },
        }
    }

    fn image(&mut self, name: &[u8]) -> Option<TextureImage> {
        let fallback = self.fallback_colours;
        match self.lookup(name) {
            Layered::Own(d) => dc_render::decode_dtx(d),
            Layered::Rom => {
                let img = self.rom.image(name);
                self.rom.release(name);
                img
            }
            Layered::Missing => {
                fallback.then(|| TextureImage::rgba(2, 2, fallback_colour(name).repeat(4)))
            }
        }
    }

    /// Names no layer answered (a solid colour stood in for them when `fallback_colours`): the mod layer's misses
    /// plus the ROM's (whose counter also holds the names it could not parse, as [`RomWorld`] reports them).
    fn missing_total(&self) -> u64 {
        self.own.values().filter(|d| d.is_none()).count() as u64
            + self.rom.missing
            + self.rom.unreadable
    }
}

impl WorldSource for ModWorld {
    fn describe(&self) -> String {
        match (&self.assets, self.any_id) {
            (Some(_), true) => format!("world file {} (preview)", self.name),
            (Some(_), false) => format!("mod world {}", self.name),
            (None, _) => format!("world file {}", self.name),
        }
    }

    fn rom(&self) -> Option<PathBuf> {
        self.rom.clone()
    }

    fn load(&mut self, gpu: Option<GpuTarget<'_>>) -> Result<LoadedWorld, ClientError> {
        let t = Instant::now();
        let (stand, crouch) = self.stance_dims()?;
        let index = match (gpu.as_ref(), &self.rom) {
            (Some(_), Some(rom)) => {
                TextureIndex::open(rom).map_err(|e| ClientError::Rom(e.to_string()))?
            }
            _ => TextureIndex::default(),
        };
        let layered = std::cell::RefCell::new(LayeredTextures {
            assets: self.assets.clone(),
            any_id: self.any_id,
            fallback_colours: self.fallback_colours,
            rom: RomTextures::new(&index),
            own: std::collections::HashMap::new(),
        });
        let mut traits = |name: &[u8]| layered.borrow_mut().traits(name);
        let mut images = |name: &[u8]| layered.borrow_mut().image(name);
        let spec = BuildSpec {
            name: self.name.clone(),
            data: &self.bytes,
            stand_dims: stand,
            crouch_dims: crouch,
            extra_spawns: &[],
            filter: &self.filter,
            scene_opts: self.scene,
        };
        let mut w = build(spec, gpu, &mut traits, &mut images)?;
        w.textures_missing = layered.borrow().missing_total();
        w.load_ms = ms(t);
        Ok(w)
    }
}

/// A capture flag without a model (MAP-34): the server sends the create with an empty model name and the client
/// draws a marker at the flag in its team's colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlagMarker {
    pub team: u8,
    pub pos: [f32; 3],
}

/// Half extents of the marker bar (world units): a thin bar a standing player's height.
// DC-DEFAULT: MAP-34 (proposed) an 8 x 80 x 8 bar standing on the flag position, in the team colour
pub const FLAG_MARKER_HALF: [f32; 3] = [4.0, 40.0, 4.0];

/// The boxes of the flag markers for the debug-box pass.
pub fn flag_marker_boxes(markers: &[FlagMarker]) -> Vec<crate::debug_draw::BoxInstance> {
    markers
        .iter()
        .filter(|m| m.pos.iter().all(|v| v.is_finite()))
        .map(|m| crate::debug_draw::BoxInstance {
            centre: [m.pos[0], m.pos[1] + FLAG_MARKER_HALF[1], m.pos[2]],
            half: FLAG_MARKER_HALF,
            colour: crate::net::team_colour(m.team),
        })
        .collect()
}

#[cfg(test)]
mod mod_world_tests {
    //! The texture layers of a mod world and the preview (no ROM content: made-up DTX files of our own writer).
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-modworld-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn dtx(colour: [u8; 4]) -> Vec<u8> {
        dc_tex::write_bgra8(2, 2, &colour.repeat(4)).unwrap()
    }

    fn layered<'i>(
        index: &'i TextureIndex,
        assets: Option<Arc<crate::modassets::ModAssets>>,
        any_id: bool,
        fallback: bool,
    ) -> LayeredTextures<'i> {
        LayeredTextures {
            assets,
            any_id,
            fallback_colours: fallback,
            rom: RomTextures::new(index),
            own: Default::default(),
        }
    }

    #[test]
    fn mod_textures_decode_and_missing_ones_count() {
        let base = temp("layers");
        let m = base.join("valley");
        put(
            &m.join("assets/textures/floor.dtx"),
            &dtx([10, 20, 30, 255]),
        );
        put(&m.join("assets/textures/bad.dtx"), b"not a texture");
        let assets = Arc::new(crate::modassets::ModAssets::new([(
            "valley".to_string(),
            m.clone(),
        )]));
        let index = TextureIndex::default();
        let mut l = layered(&index, Some(assets), false, false);
        let t = l.traits(b"@mod:valley/assets/textures/floor.dtx");
        assert!(t.present);
        let img = l.image(b"@mod:valley/assets/textures/floor.dtx").unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(&img.levels[0][..4], &[10, 20, 30, 255]);
        // a file that does not parse, a name of another mod and a ROM name without a ROM: all missing
        assert!(!l.traits(b"@mod:valley/assets/textures/bad.dtx").present);
        assert!(!l.traits(b"@mod:other/assets/textures/floor.dtx").present);
        assert!(!l.traits(b"a/rom/name.dtx").present);
        assert!(l.image(b"a/rom/name.dtx").is_none());
        assert_eq!(l.missing_total(), 3);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_preview_maps_every_mod_id_to_its_folder_and_colours_the_rest() {
        let base = temp("preview");
        let m = base.join("build");
        put(
            &m.join("assets/textures/wall.dtx"),
            &dtx([200, 100, 50, 255]),
        );
        let file = base.join("x.dat");
        put(&file, b"");
        let w = ModWorld::preview(&file, None, Some(&m)).unwrap();
        assert_eq!(w.name, "X");
        assert!(w.any_id && w.fallback_colours);
        let index = TextureIndex::default();
        let mut l = layered(&index, w.assets.clone(), true, true);
        let img = l
            .image(b"@mod:whatever_id/assets/textures/wall.dtx")
            .unwrap();
        assert_eq!(&img.levels[0][..4], &[200, 100, 50, 255]);
        // a name nobody answers: present, a solid colour of the name, counted as missing
        assert!(l.traits(b"some/rom/texture.dtx").present);
        let c = l.image(b"some/rom/texture.dtx").unwrap();
        assert_eq!(&c.levels[0][..4], &fallback_colour(b"some/rom/texture.dtx"));
        assert_ne!(
            fallback_colour(b"some/rom/texture.dtx"),
            fallback_colour(b"other/rom/texture.dtx")
        );
        assert_eq!(l.missing_total(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_mod_world_loads_the_synthetic_bytes_with_the_made_up_stand_box() {
        let bytes = crate::testkit::synth_world_bytes();
        let mut w = ModWorld::new("@mod:t/assets/maps/x.dat", bytes, None, None);
        let loaded = w.load(None).unwrap();
        assert_eq!(loaded.name, "@mod:t/assets/maps/x.dat");
        assert_eq!(loaded.stand_dims, V3::new(16.0, 40.0, 16.0));
        assert_eq!(loaded.collision.models.len(), 1);
        assert_eq!(w.describe(), "world file @mod:t/assets/maps/x.dat");
    }

    #[test]
    fn flag_markers_stand_on_the_flag_in_the_team_colour() {
        let b = flag_marker_boxes(&[
            FlagMarker {
                team: 1,
                pos: [10.0, 0.0, 20.0],
            },
            FlagMarker {
                team: 2,
                pos: [f32::NAN, 0.0, 0.0],
            },
        ]);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].centre, [10.0, FLAG_MARKER_HALF[1], 20.0]);
        assert_eq!(b[0].half, FLAG_MARKER_HALF);
        assert_eq!(b[0].colour, crate::net::team_colour(1));
    }
}

#[cfg(test)]
mod light_index_tests {
    //! ROM-free tests of the static-light index sizing and the sun memo (render-core; engineering only).
    use super::*;
    use dc_render::Falloff;

    fn point(pos: Vec3, radius: f32) -> ModelLight {
        ModelLight {
            shape: LightShape::Point,
            pos,
            radius,
            colour: Vec3::splat(200.0),
            falloff: Falloff::Linear,
            to_ambient: 0.0,
        }
    }

    #[test]
    fn far_or_overflowing_lights_keep_the_grid_bounded() {
        let o = point(Vec3::ZERO, 100.0);
        let cases: Vec<Vec<ModelLight>> = vec![
            Vec::new(),
            vec![o],
            // one far light: (5e5 / 256)^3 cells overflowed the old usize product on i686
            vec![o, point(Vec3::new(5.0e5, 0.0, 0.0), 100.0)],
            vec![
                point(Vec3::splat(-5.0e5), 100.0),
                point(Vec3::splat(5.0e5), 100.0),
            ],
            // finite but enormous union box
            vec![
                o,
                point(Vec3::splat(1.0e38), 10.0),
                point(Vec3::splat(-1.0e38), 10.0),
            ],
            // union extent overflows to infinity: no grid, every light tested
            vec![
                o,
                point(Vec3::splat(3.0e38), 10.0),
                point(Vec3::splat(-3.0e38), 10.0),
            ],
            // padding overflows to infinity, huge finite sphere, non-finite and negative radii
            vec![
                o,
                point(Vec3::ZERO, f32::MAX),
                point(Vec3::ZERO, 1.0e30),
                point(Vec3::ZERO, f32::INFINITY),
                point(Vec3::ZERO, f32::NAN),
                point(Vec3::new(300.0, 0.0, 0.0), -50.0),
            ],
        ];
        for lights in &cases {
            let idx = StaticLightIndex::new(lights);
            let cells: usize = idx.dims.iter().product();
            assert!(cells <= LIGHT_GRID_MAX_CELLS, "{cells} cells");
            assert_eq!(idx.cells.len(), cells);
            let mut out = Vec::new();
            for (i, l) in lights.iter().enumerate() {
                let i = i as u32;
                assert!(
                    idx.always.contains(&i) || idx.cells.iter().any(|c| c.contains(&i)),
                    "light {i} lost"
                );
                if l.pos.is_finite() {
                    idx.query(l.pos, 1.0, &mut out);
                    assert!(
                        out.contains(&i),
                        "light {i} not a candidate at its own position"
                    );
                }
            }
            // a non-finite or negative query box gets every light
            for (p, r) in [
                (Vec3::splat(f32::NAN), 1.0),
                (Vec3::ZERO, f32::INFINITY),
                (Vec3::ZERO, -1.0),
            ] {
                idx.query(p, r, &mut out);
                assert_eq!(out, (0..lights.len() as u32).collect::<Vec<_>>());
            }
        }
        // the extent that overflows gets no grid
        let idx = StaticLightIndex::new(&cases[5]);
        assert!(idx.cells.is_empty());
        assert_eq!(idx.always, vec![0, 1, 2]);
    }

    #[test]
    fn sun_cache_keeps_the_result_within_the_variance() {
        // no result yet: test
        assert_eq!(kept_sun(None, Vec3::ZERO), None);
        let kept = Some(([10.0, 20.0, 30.0], 0.75));
        // the same position and a move up to variance + epsilon keep the result
        assert_eq!(kept_sun(kept, Vec3::new(10.0, 20.0, 30.0)), Some(0.75));
        assert_eq!(kept_sun(kept, Vec3::new(10.05, 20.0, 30.0)), Some(0.75));
        assert_eq!(
            kept_sun(kept, Vec3::new(10.0, 20.0, 30.0 - 0.05)),
            Some(0.75)
        );
        // beyond it (on any axis, or diagonally over the squared distance) the object is tested again
        assert_eq!(kept_sun(kept, Vec3::new(10.06, 20.0, 30.0)), None);
        assert_eq!(kept_sun(kept, Vec3::new(10.04, 20.04, 30.0)), None);
        assert_eq!(kept_sun(kept, Vec3::new(10.0, 21.0, 30.0)), None);
        // the keys of the object kinds do not collide
        assert_ne!(
            sun_key(SunObject::Character(5)),
            sun_key(SunObject::Drop(5))
        );
        assert_ne!(sun_key(SunObject::Drop(0)), sun_key(SunObject::ViewWeapon));
    }

    #[test]
    fn sun_memo_keeps_the_last_eight_positions() {
        let m = SunMemo::default();
        assert_eq!(m.get([0, 0, 0]), None);
        for i in 0..9u32 {
            m.put([i, 0, 0], i as f32);
        }
        assert_eq!(m.get([0, 0, 0]), None, "the oldest entry is overwritten");
        for i in 1..9u32 {
            assert_eq!(m.get([i, 0, 0]), Some(i as f32));
        }
        // a clone starts empty (the memo is a cache, not state)
        assert_eq!(m.clone().get([8, 0, 0]), None);
    }
}

//! `dc-mapkit build`: project -> version-85 world file (MAPKIT_PLAN.md 5.3; DC-DEFAULT: MAP-43).
//!
//! Pipeline: [`level_of`] (terrain, solids, imports -> [`Level`]) -> vertex colours ([`crate::bake`]) ->
//! BSP per model ([`crate::bsp::build`]: helpers on for the level, off for every brush) -> BSP models, render
//! data and objects ([`crate::worldgen`]) -> `dc_world::synth::SynthWorld::build` -> the world-level checks
//! ([`crate::validate::check_world`]: parse back, layout, references, spawns, extents, sky).
//!
//! File layout (MAP-43): models = the level (`"level"`, flags 0: terrain + plain solids + collision imports +
//! the 0x10 sky faces, with the kd helper polygons) then one model per brush in project order (name = the solid
//! id, flags bit 1, origin = the solid's centre); objects = the project's objects, then one object per brush,
//! then `dc.world_settings` and `dc.global_light` (MAP-46); render data = one top-level block for the level
//! (every level polygon except the sky faces, which become the block's sky portals) and one nested render world
//! per drawn brush (world models, breakables; the sky model with its faces turned inside); a 1 x 1 x 1 light
//! grid of the ambient colour; no blind data, no blockers, one quadtree leaf; extents = the geometry box plus
//! 128 units. Prefabs are expanded by the prefab library of a later stage (TODO(mapkit-stage2)); `level_of`
//! ignores them today.

use crate::bake::{self, BakeInput};
use crate::bsp::{self, BspOptions};
use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::geom::{self, Brush, Level, Surface, Xform, SURF_NO_STEP, SURF_SKY, SURF_SOLID};
use crate::project::{
    BrushRole, BuildSettings, Heights, Material, Project, TextureRef, ASSETS_DIR,
};
use crate::worldgen::{self, EXTENT_MARGIN, INFO_TEXT, LEVEL_MODEL_NAME};
use dc_world::synth::{SynthRenderWorld, SynthWorld};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Deepest helper split of the level's kd tree (a guard; 16-polygon cells are reached long before).
pub const MAX_HELPER_DEPTH: usize = 48;
/// The vertex colour of every sky face: the texture as drawn: the game doubles vertex colours, so mid grey is no light and no shade (MAP-96).
const SKY_COLOUR: u32 = 0xFF80_8080;

/// Where the project's own files are read from (the project folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assets {
    pub dir: PathBuf,
}

impl Assets {
    pub fn new(dir: &Path) -> Assets {
        Assets {
            dir: dir.to_path_buf(),
        }
    }
}

/// Counts of a built world.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildStats {
    /// Polygons written (level, sky faces and every brush; helpers not counted).
    pub polys: usize,
    pub brushes: usize,
    pub sky_faces: usize,
    /// BSP nodes over every model.
    pub nodes: usize,
    /// Deepest BSP path over every model.
    pub depth: usize,
    /// Distinct texture names.
    pub textures: usize,
    /// Render vertices, triangles and sections over every render world.
    pub vertices: usize,
    pub triangles: usize,
    pub sections: usize,
    /// Lightmap pages and their packed bytes (MAP-108).
    pub lightmap_pages: usize,
    pub lightmap_bytes: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub dat: Vec<u8>,
    pub stats: BuildStats,
    pub diagnostics: Diagnostics,
}

/// The surface data of a material at index `i` (flags from the material's flags).
pub fn surface_of(m: &Material, i: usize) -> Surface {
    let mut flags = 0;
    if m.flags.solid || m.flags.sky {
        flags |= SURF_SOLID;
    }
    if m.flags.sky {
        flags |= SURF_SKY;
    }
    if m.flags.no_step {
        flags |= SURF_NO_STEP;
    }
    // a sky face always carries the client's sky surface id, whatever the material says (C4; MAP-43)
    let surface = if m.flags.sky {
        worldgen::SURFACE_SKY
    } else {
        m.surface_id as u16
    };
    Surface {
        material: i,
        flags,
        surface,
        uv_scale: m.tile,
        fit: m.fit,
    }
}

/// The polygons of `polys` whose corner count does not fit the file's one-byte count (`MAX_CORNERS`).
fn oversized(polys: &[geom::Poly]) -> Vec<usize> {
    polys
        .iter()
        .enumerate()
        .filter(|(_, p)| p.corners.len() > crate::project::MAX_CORNERS)
        .map(|(i, _)| i)
        .collect()
}

/// Pairs of drawn faces on nearly one plane that overlap: they fight over the same pixels (DC-DEFAULT: MAP-109).
fn overlapping_faces(level: &Level, baked: bool) -> Vec<(String, String)> {
    struct Face<'a> {
        source: &'a str,
        material: usize,
        n: geom::V3,
        d: f32,
        lo: [f32; 2],
        hi: [f32; 2],
    }
    let mut faces: Vec<Face<'_>> = Vec::new();
    for (p, source) in level.polys.iter().zip(&level.poly_sources) {
        if p.corner_normals.is_some() || p.corners.len() < 3 || source == "terrain" {
            continue;
        }
        let n = p.plane.n;
        let (a, b) = if n.y.abs() > 0.7 {
            (0, 2)
        } else if n.x.abs() > n.z.abs() {
            (1, 2)
        } else {
            (0, 1)
        };
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for c in &p.corners {
            let q = c.to_array();
            lo = [lo[0].min(q[a]), lo[1].min(q[b])];
            hi = [hi[0].max(q[a]), hi[1].max(q[b])];
        }
        // nobody looks at a thing from under the floor, and one material on both faces hardly shows
        if n.y < -0.5 && p.corners.iter().all(|c| c.y < 4.0) {
            continue;
        }
        // the thin edges of a strip are not seen
        if hi[0] - lo[0] < 3.0 || hi[1] - lo[1] < 3.0 {
            continue;
        }
        faces.push(Face {
            source,
            material: p.material,
            n,
            d: p.plane.d,
            lo,
            hi,
        });
    }
    let mut out = Vec::new();
    for (i, f) in faces.iter().enumerate() {
        for g in &faces[i + 1..] {
            // one material on both is the same picture, unless each has its own light (DC-DEFAULT: MAP-110)
            if f.source != g.source
                && (baked || f.material != g.material)
                && f.n.dot(g.n) > 0.999
                && (f.d - g.d).abs() < 1.0
                && f.lo[0].max(g.lo[0]) + 1.0 < f.hi[0].min(g.hi[0])
                && f.lo[1].max(g.lo[1]) + 1.0 < f.hi[1].min(g.hi[1])
            {
                out.push((f.source.to_string(), g.source.to_string()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// True for a texture with holes, which the export keeps (DC-DEFAULT: MAP-110).
fn material_has_holes(m: &Material, assets: &Assets) -> bool {
    let crate::project::TextureRef::Own(rel) = &m.texture else {
        return false;
    };
    image::open(Project::resolve(&assets.dir, rel))
        .is_ok_and(|img| img.to_rgba8().pixels().any(|p| p[3] < 128))
}

/// A material's average colour, 0 .. 1, for the light a surface bounces (grey for a texture only the user's ROM has).
fn material_albedo(m: &Material, assets: &Assets) -> [f32; 3] {
    let crate::project::TextureRef::Own(rel) = &m.texture else {
        return [0.5; 3];
    };
    let Ok(img) = image::open(Project::resolve(&assets.dir, rel)) else {
        return [0.5; 3];
    };
    let img = img.to_rgb8();
    let n = (img.width() as u64 * img.height() as u64).max(1) as f32;
    let mut sum = [0f32; 3];
    for p in img.pixels() {
        for c in 0..3 {
            sum[c] += p[c] as f32 / 255.0;
        }
    }
    sum.map(|v| v / n)
}

/// The `bsp.budget` diagnostic of a model whose tree ran past its node budget, naming the solids / imports /
/// terrain the offending cell's polygons came from (`sources` parallel to the polygons).
fn budget_diagnostic(model: &str, hit: &bsp::BudgetHit, sources: &[String]) -> Diagnostic {
    let mut names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for &i in &hit.cell_polys {
        let s = sources.get(i).map(String::as_str).unwrap_or("");
        names.insert(if s.is_empty() { "?" } else { s });
    }
    let list: Vec<&str> = names.iter().copied().take(12).collect();
    let more = names.len().saturating_sub(list.len());
    Diagnostic::new(
        Code::BspBudget,
        format!("world.models.{model}"),
        format!(
            "the collision tree ran past its node budget at {} nodes: {} polygons of one cell straddle every \
split plane and would be duplicated into every subtree; they come from {}{}",
            hit.nodes,
            hit.cell_polys.len(),
            list.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ),
    )
    .with_fix(
        "split long rotated walls that cross a common area into shorter pieces, align them to the axes, or \
make them brushes (world_model)",
    )
}

/// Assemble the geometry of a project: terrain triangles (layer 0 material), plain solids and collision imports
/// as level polygons (sky-flagged faces into `sky_faces`), solids with a brush role as brushes named by their
/// id. OBJ files are read from `assets.dir`. Errors: a missing material, a malformed solid, an unreadable OBJ.
pub fn level_of(p: &Project, h: &Heights, assets: &Assets) -> Result<Level, Diagnostics> {
    let mut diags = Diagnostics::new();
    let mut level = Level::default();
    let hidden = |id: &str| {
        p.material_index(id)
            .is_some_and(|i| p.materials[i].flags.hidden)
    };
    let surf = |id: &str| -> Option<Surface> {
        p.material_index(id).map(|i| surface_of(&p.materials[i], i))
    };
    if let Some(t) = &p.terrain {
        let layer = t.layers.first().map(|l| l.material.as_str()).unwrap_or("");
        let Some(s) = surf(layer) else {
            diags.error(
                Code::MaterialMissing,
                "terrain.layers[0]",
                format!("material `{layer}` is not defined"),
            );
            return Err(diags);
        };
        for poly in geom::terrain_polys(t, h, &|_, _| s) {
            level.push_from(poly, "terrain");
        }
    }
    for (i, s) in p.solids.iter().enumerate() {
        let missing = std::cell::RefCell::new(None);
        let polys = geom::solid_polys(s, &|m| {
            surf(m).unwrap_or_else(|| {
                *missing.borrow_mut() = Some(m.to_string());
                Surface::default()
            })
        });
        if let Some(m) = missing.into_inner() {
            diags.error(
                Code::MaterialMissing,
                format!("solids[{i}]"),
                format!("material `{m}` is not defined"),
            );
            return Err(diags);
        }
        let polys = match polys {
            Ok(v) => v,
            Err(e) => {
                diags.error(Code::SolidShape, format!("solids[{i}].kind"), e);
                return Err(diags);
            }
        };
        match s.brush {
            None if hidden(&s.material) => {
                // a hidden material: collision only, never drawn (DC-DEFAULT: MAP-104)
                for poly in polys {
                    level.hidden.push(poly);
                    level.hidden_sources.push(s.id.clone());
                }
            }
            None => {
                for poly in polys {
                    level.push_from(poly, &s.id);
                }
            }
            Some(role) => level.brushes.push(Brush {
                name: s.id.clone(),
                role,
                origin: geom::V3::from_array(s.pos),
                polys,
            }),
        }
    }
    for (i, imp) in p.imports.iter().enumerate() {
        let Some(s) = surf(&imp.material) else {
            diags.error(
                Code::MaterialMissing,
                format!("imports[{i}].material"),
                format!("material `{}` is not defined", imp.material),
            );
            return Err(diags);
        };
        let path = Project::resolve(&assets.dir, &imp.file);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                diags.error(
                    Code::ProjectIo,
                    format!("imports[{i}].file"),
                    format!("{}: {e}", imp.file),
                );
                return Err(diags);
            }
        };
        let mesh = match geom::parse_obj(&text) {
            Ok(m) => m,
            Err(e) => {
                diags.error(
                    Code::ProjectIo,
                    format!("imports[{i}].file"),
                    format!("{}: {e}", imp.file),
                );
                return Err(diags);
            }
        };
        let mut s = s;
        if !imp.collision {
            s.flags &= !SURF_SOLID;
        }
        let (polys, skipped) = geom::obj_polys(
            &mesh,
            &Xform::new(imp.pos, imp.rot_y_deg, imp.scale),
            s,
            Some(imp.id.clone()),
        );
        if skipped > 0 {
            diags.push(
                Diagnostic::new(
                    Code::SolidShape,
                    format!("imports[{i}].file"),
                    format!("{skipped} degenerate triangles skipped"),
                )
                .with_severity(crate::diag::Severity::Warning),
            );
        }
        for poly in polys {
            level.push_from(poly, &imp.id);
        }
    }
    level.update_extent();
    Ok(level)
}

/// The texture name of every material (index = material index), MAP-45; an own texture outside `assets/` is
/// an error (the client's mod asset layer resolves `@mod:` paths under `assets/` only).
pub fn texture_names(p: &Project, diags: &mut Diagnostics) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(p.materials.len());
    for (i, m) in p.materials.iter().enumerate() {
        if let TextureRef::Own(path) = &m.texture {
            if !path.starts_with(&format!("{ASSETS_DIR}/")) {
                diags.push(
                    Diagnostic::new(
                        Code::TextureAssets,
                        format!("materials[{i}].texture"),
                        format!("`{path}` is not under {ASSETS_DIR}/"),
                    )
                    .with_fix(format!(
                        "move the PNG under {ASSETS_DIR}/ and point the material there"
                    )),
                );
            }
        }
        out.push(worldgen::texture_name(m, &p.mod_meta.id));
    }
    out
}

/// Build the world file from a project (the project-level checks run first; their errors refuse the build).
pub fn build(
    p: &Project,
    h: &Heights,
    assets: &Assets,
    settings: &BuildSettings,
) -> Result<Built, Diagnostics> {
    let mut diags = crate::validate::validate(&assets.dir, p, h);
    if diags.has_errors() {
        return Err(diags);
    }
    let level = match level_of(p, h, assets) {
        Ok(l) => l,
        Err(e) => {
            diags.extend(e);
            return Err(diags);
        }
    };
    let textures = texture_names(p, &mut diags);
    if diags.has_errors() {
        return Err(diags);
    }
    let occluders: Vec<bake::Occluder> = if p.build.bake.shadow > 0.0 {
        level
            .polys
            .iter()
            .zip(&level.poly_sources)
            .filter(|(poly, source)| poly.flags & SURF_SOLID != 0 && source.as_str() != "terrain")
            .map(|(poly, _)| poly)
            .chain(level.hidden.iter())
            .filter(|poly| poly.corners.len() >= 3)
            .map(bake::Occluder::of)
            .collect()
    } else {
        Vec::new()
    };
    let bake_in = BakeInput {
        occluders: &occluders,
        settings: &p.settings,
        bake: &settings.bake,
        terrain: p.terrain.as_ref().map(|t| (t, h)),
    };
    let mut stats = BuildStats {
        brushes: level.brushes.len(),
        sky_faces: level.sky_faces.len(),
        ..Default::default()
    };

    // every polygon's corner count must fit the file's one-byte count (C14): a parse error otherwise
    {
        let mut bad: Vec<String> = Vec::new();
        for i in oversized(&level.polys) {
            bad.push(format!(
                "{} ({} corners)",
                level.poly_sources.get(i).map(String::as_str).unwrap_or("?"),
                level.polys[i].corners.len()
            ));
        }
        for i in oversized(&level.sky_faces) {
            bad.push(format!(
                "{} ({} corners)",
                level.sky_sources.get(i).map(String::as_str).unwrap_or("?"),
                level.sky_faces[i].corners.len()
            ));
        }
        for b in &level.brushes {
            for i in oversized(&b.polys) {
                bad.push(format!("{} ({} corners)", b.name, b.polys[i].corners.len()));
            }
        }
        if !bad.is_empty() {
            bad.dedup();
            diags.push(
                Diagnostic::new(
                    Code::BuildFail,
                    "solids",
                    format!(
                        "{} polygons have more than {} corners (the world file counts them in one byte): {}",
                        bad.len(),
                        crate::project::MAX_CORNERS,
                        bad.iter().take(8).cloned().collect::<Vec<_>>().join(", ")
                    ),
                )
                .with_fix("use at most 255 cylinder sides / prism points"),
            );
            return Err(diags);
        }
    }

    // the level's BSP model: what can be hit (solid level polygons, hidden cores, sky faces); render data without
    // the sky faces and the hidden cores. A see-through model has nothing the walk could sweep, so a dense mesh
    // costs neither nodes nor bytes here and cannot blow the budget (DC-DEFAULT: MAP-104).
    let mut level_polys = Vec::new();
    let mut level_sources = Vec::new();
    for (poly, source) in level.polys.iter().zip(&level.poly_sources) {
        if poly.flags & SURF_SOLID != 0 {
            level_polys.push(poly.clone());
            level_sources.push(source.clone());
        }
    }
    level_polys.extend(level.hidden.iter().cloned());
    level_sources.extend(level.hidden_sources.iter().cloned());
    level_polys.extend(level.sky_faces.iter().cloned());
    level_sources.extend(level.sky_sources.iter().cloned());
    let level_tree = bsp::build(
        &level_polys,
        &BspOptions {
            cell_polys: settings.cell_polys.max(1) as usize,
            helpers: true,
            max_depth: MAX_HELPER_DEPTH,
        },
    );
    if let Some(hit) = &level_tree.budget_exceeded {
        diags.push(budget_diagnostic(LEVEL_MODEL_NAME, hit, &level_sources));
        return Err(diags);
    }
    stats.polys += level_polys.len();
    stats.nodes += level_tree.nodes.len();
    stats.depth = stats.depth.max(bsp::depth(&level_tree));
    let mut models = vec![worldgen::bsp_model(
        LEVEL_MODEL_NAME,
        &level_polys,
        &level_tree,
        geom::V3::ZERO,
        0,
        &textures,
    )];
    for (a, b) in overlapping_faces(&level, p.build.bake.lightmap > 0.0)
        .into_iter()
        .take(40)
    {
        diags.push(Diagnostic::new(
            Code::FaceOverlap,
            format!("solids.{a}"),
            format!("{a} and {b} share a plane and overlap, so they will flicker"),
        ));
    }
    // lightmaps when the project asks for them (DC-DEFAULT: MAP-108), else light per corner
    let baked = (p.build.bake.lightmap > 0.0).then(|| {
        let means: Vec<[f32; 3]> = p
            .materials
            .iter()
            .map(|m| material_albedo(m, assets))
            .collect();
        let albedo = |i: usize| means.get(i).copied().unwrap_or([0.5; 3]);
        let holes: Vec<bool> = p
            .materials
            .iter()
            .map(|m| material_has_holes(m, assets))
            .collect();
        let paint = |i: usize| holes.get(i).copied().unwrap_or(false);
        let skin = |i: usize| p.materials.get(i).is_some_and(|m| m.flags.skin);
        let input = crate::lightmap::LightInput {
            settings: &p.settings,
            density: p.build.bake.lightmap,
            ambient: p.build.bake.ambient,
            sun: p.build.bake.sun,
            bounce: 1.0,
            lamps: p
                .objects
                .iter()
                .filter_map(crate::lightmap::Lamp::of)
                .collect(),
            albedo: &albedo,
            paint: &paint,
            skin: &skin,
        };
        crate::lightmap::bake(&level, &input)
    });
    if let Some(b) = &baked {
        let mut seen = std::collections::BTreeSet::new();
        for i in &b.paint_off_wall {
            let source = level
                .poly_sources
                .get(*i)
                .map(String::as_str)
                .unwrap_or("?");
            if seen.insert(source) {
                diags.push(
                    Diagnostic::new(
                        Code::PaintOffWall,
                        format!("imports.{source}"),
                        format!("{source} hangs off its wall or sits behind another surface"),
                    )
                    .with_fix("place it wholly on the wall, up to 12 units off it and in front of anything painted on it"),
                );
            }
        }
        stats.lightmap_pages = b.pages.len();
        stats.lightmap_bytes = b.pages.iter().map(Vec::len).sum();
    }
    let (level_render, counts) = match &baked {
        Some(b) => worldgen::render_world_lit(&level.polys, b, &textures, &level.sky_faces),
        None => {
            let colours = bake::bake_polys(&bake_in, &level.polys);
            worldgen::render_world(&level.polys, &colours, &textures, false, &level.sky_faces)
        }
    };
    stats.vertices += counts.vertices;
    stats.triangles += counts.triangles;
    stats.sections += counts.sections;
    let mut render = SynthRenderWorld {
        blocks: level_render.blocks,
        models: Vec::new(),
    };

    // brushes: one model each (naive BSP, no helpers), a nested render world when drawn, one object each
    let mut objects: Vec<_> = p.objects.iter().map(worldgen::map_object).collect();
    for b in &level.brushes {
        let tree = bsp::build(
            &b.polys,
            &BspOptions {
                cell_polys: settings.cell_polys.max(1) as usize,
                helpers: false,
                max_depth: MAX_HELPER_DEPTH,
            },
        );
        if let Some(hit) = &tree.budget_exceeded {
            let sources = vec![b.name.clone(); b.polys.len()];
            diags.push(budget_diagnostic(&b.name, hit, &sources));
            return Err(diags);
        }
        stats.polys += b.polys.len();
        stats.nodes += tree.nodes.len();
        stats.depth = stats.depth.max(bsp::depth(&tree));
        models.push(worldgen::bsp_model(
            &b.name,
            &b.polys,
            &tree,
            b.origin,
            worldgen::BSP_FLAG_BRUSH,
            &textures,
        ));
        if let Some(inside) = worldgen::brush_render(b.role) {
            // DC-DEFAULT: MAP-96 the sky is not lit: baking it shaded each side of the box by the sun, and the
            // box's edges showed as a dark rectangle in the sky
            let colours = if b.role == BrushRole::Sky {
                b.polys
                    .iter()
                    .map(|q| vec![SKY_COLOUR; q.corners.len()])
                    .collect()
            } else {
                bake::bake_polys(&bake_in, &b.polys)
            };
            let (w, counts) = worldgen::render_world(&b.polys, &colours, &textures, inside, &[]);
            stats.vertices += counts.vertices;
            stats.triangles += counts.triangles;
            stats.sections += counts.sections;
            render.models.push((b.name.as_bytes().to_vec(), w));
        }
        objects.push(worldgen::brush_object(b));
    }

    // the generated settings objects at the centre of the geometry box
    let centre = (level.extent_min + level.extent_max) * 0.5;
    let top = geom::V3::new(centre.x, level.extent_max.y, centre.z);
    if !p
        .objects
        .iter()
        .any(|o| o.class == crate::vocab_names::CLASS_END_CAMERA)
    {
        objects.push(worldgen::default_end_camera(p, level.extent_max));
    }
    objects.push(worldgen::world_settings_object(&p.settings, centre));
    objects.push(worldgen::global_light_object(&p.settings, top));

    // extents and the light grid
    let margin = geom::V3::new(EXTENT_MARGIN, EXTENT_MARGIN, EXTENT_MARGIN);
    let (extent_min, extent_max) = if level.polys.is_empty()
        && level.sky_faces.is_empty()
        && level.brushes.iter().all(|b| b.polys.is_empty())
    {
        (-margin, margin)
    } else {
        (level.extent_min - margin, level.extent_max + margin)
    };
    let span = extent_max - extent_min;
    let a = p.settings.ambient;
    let flat_grid = (
        [1u32, 1, 1],
        extent_min.to_array(),
        [span.x.max(1.0), span.y.max(1.0), span.z.max(1.0)],
        vec![a[0], a[1], a[2]],
    );
    let (grid_dims, grid_origin, grid_cell, grid_rgb) = match baked {
        Some(b) => (
            b.grid_dims,
            b.grid_origin.to_array(),
            b.grid_cell.to_array(),
            b.grid_rgb,
        ),
        None => flat_grid,
    };
    let world = SynthWorld {
        info: INFO_TEXT.as_bytes().to_vec(),
        extent_min: extent_min.to_array(),
        extent_max: extent_max.to_array(),
        world_offset: [0.0; 3],
        tree_shape: vec![false],
        models,
        objects,
        blind: Vec::new(),
        grid_origin,
        grid_cell,
        grid_dims,
        grid_rgb,
        blockers: Vec::new(),
        particle_blockers: Vec::new(),
        render,
        grids: Vec::new(),
    };
    let dat = world.build();
    stats.bytes = dat.len();
    let mut names: Vec<&[u8]> = textures.iter().map(Vec::as_slice).collect();
    names.sort_unstable();
    names.dedup();
    stats.textures = names.len();

    diags.extend(crate::validate::check_world(&dat));
    Ok(Built {
        dat,
        stats,
        diagnostics: diags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{BrushRole, MaterialFlags, Solid, Terrain, TerrainLayer, TextureRef};

    /// A hidden core stops a shot and is not drawn; a see-through model is drawn and stops nothing, and however
    /// dense it is it does not reach the collision tree (MAP-104), all read back through the world file.
    #[test]
    fn a_core_stops_shots_unseen_and_a_model_is_seen_and_stops_nothing() {
        use dc_sim::from_world::{collision_world, WorldFilter};
        let dir = std::env::temp_dir().join(format!("dc-mapkit-build-core-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        let made = crate::template::make(crate::template::Template::Flat, "Core", "tests", 1);
        for file in &made.files {
            std::fs::write(dir.join(&file.path), &file.bytes).unwrap();
        }
        let (mut p, h) = (made.project, made.heights);
        // a fan of 400 triangles through one point, the shape that used to run the tree out of its budget
        let mut obj = String::from("v 0 0 0\n");
        for i in 0..=400 {
            let a = i as f32 * 0.0157;
            obj.push_str(&format!(
                "v {} {} {}\n",
                100.0 * a.cos(),
                50.0 + 40.0 * (a * 7.0).sin(),
                100.0 * a.sin()
            ));
        }
        for i in 0..400 {
            obj.push_str(&format!("f 1 {} {}\n", i + 2, i + 3));
        }
        std::fs::write(dir.join("assets/fan.obj"), obj).unwrap();
        let fan = p.alloc_id("import");
        p.imports.push(crate::project::Import {
            id: fan,
            file: "assets/fan.obj".into(),
            pos: [600.0, 0.0, 600.0],
            rot_y_deg: 0.0,
            scale: 1.0,
            material: "wall".into(),
            collision: false,
        });
        p.materials.push(Material {
            id: "core".into(),
            texture: TextureRef::Own("assets/wall.png".into()),
            flags: MaterialFlags {
                hidden: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let id = p.alloc_id("solid");
        p.solids.push(Solid {
            id,
            pos: [-600.0, 50.0, 600.0],
            size: [100.0, 100.0, 100.0],
            material: "core".into(),
            ..Default::default()
        });
        let built =
            build(&p, &h, &Assets::new(&dir), &BuildSettings::default()).expect("it builds");
        assert!(
            built.stats.triangles >= 400,
            "the model is drawn: {:?}",
            built.stats
        );
        let w = dc_world::World::parse(&built.dat).expect("the world parses");
        let cw = collision_world(&w, &WorldFilter::default());
        let shot = |x: f32| {
            cw.ray(
                dc_sim::V3::new(x, 50.0, 300.0),
                dc_sim::V3::new(x, 50.0, 900.0),
            )
        };
        assert!(shot(-600.0).is_some(), "the core stops a shot");
        assert!(shot(600.0).is_none(), "the model does not");
        assert!(
            built.stats.triangles >= built.stats.polys + 300,
            "the model is drawn but not in the collision model: {:?}",
            built.stats
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_faces_on_one_plane_are_reported_and_a_gap_is_not() {
        let s = Surface::default();
        let quad = |x: f32, z: f32| {
            geom::Poly::new(
                vec![
                    geom::V3::new(x, 0.0, z),
                    geom::V3::new(x + 10.0, 0.0, z),
                    geom::V3::new(x + 10.0, 10.0, z),
                    geom::V3::new(x, 10.0, z),
                ],
                geom::V3::new(0.0, 0.0, -1.0),
                s,
                None,
            )
            .unwrap()
        };
        let mut l = Level::default();
        l.push_from(quad(0.0, 0.0), "a");
        let mut other = quad(5.0, 0.0);
        other.material = 1;
        l.push_from(other, "b");
        l.push_from(quad(0.0, 8.0), "c");
        assert_eq!(
            overlapping_faces(&l, false),
            vec![("a".to_string(), "b".to_string())]
        );
        // one material with its own light on each flickers too
        let mut same = Level::default();
        same.push_from(quad(0.0, 0.0), "a");
        same.push_from(quad(5.0, 0.0), "d");
        assert!(overlapping_faces(&same, false).is_empty());
        assert_eq!(
            overlapping_faces(&same, true),
            vec![("a".to_string(), "d".to_string())]
        );
    }

    /// A lit project writes what the renderer reads (RENDER_2009.md 5.2, 6.1): page sections that unpack to the full
    /// size, the same triangle count again as textured sections, and a grid of light that varies (MAP-108).
    #[test]
    fn a_lit_map_writes_lightmap_pages_and_a_varied_grid() {
        let dir = std::env::temp_dir().join(format!("dc-mapkit-build-lit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        let made = crate::template::make(crate::template::Template::Flat, "Lit", "tests", 1);
        for file in &made.files {
            std::fs::write(dir.join(&file.path), &file.bytes).unwrap();
        }
        let (mut p, h) = (made.project, made.heights);
        p.build.bake.lightmap = 32.0;
        let mut lamp = crate::project::MapObject {
            id: p.alloc_id("point_light"),
            class: crate::vocab_names::CLASS_POINT_LIGHT.to_string(),
            name: "lamp".into(),
            pos: [0.0, 200.0, 0.0],
            ..Default::default()
        };
        lamp.props.insert(
            crate::vocab_names::PROP_RADIUS.to_string(),
            crate::project::PropValue::Num(800.0),
        );
        p.objects.push(lamp);
        let settings = p.build.clone();
        let built = build(&p, &h, &Assets::new(&dir), &settings).expect("it builds");
        assert!(built.stats.lightmap_pages > 0, "{:?}", built.stats);
        let w = dc_world::World::parse(&built.dat).expect("the world parses");
        w.check_layout().expect("layout");
        let render = w.render.as_ref().expect("render data");
        let block = &render.world.blocks[0];
        let (mut kind2, mut kind4) = (0u64, 0u64);
        for s in &block.sections {
            match s.shader_kind {
                2 => {
                    kind2 += s.triangles as u64;
                    assert_eq!((s.lightmap_w, s.lightmap_h), (256, 256));
                    assert_eq!(
                        crate::lightmap::tests_unpack(s.lightmap).len(),
                        256 * 256 * 3,
                        "a page unpacks to its whole size"
                    );
                }
                4 => kind4 += s.triangles as u64,
                _ => {}
            }
        }
        assert!(kind2 > 0);
        assert_eq!(
            kind2, kind4,
            "the page sections duplicate the textured triangles"
        );
        let grid = w.light_grid.expand();
        assert!(grid.len() > 3);
        let (lo, hi) = (grid.iter().min().unwrap(), grid.iter().max().unwrap());
        assert!(hi > lo, "the grid holds the lamp's pool, not one colour");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn level_of_assembles_terrain_solids_and_brushes() {
        let mut p = Project::default();
        p.materials.push(Material {
            id: "ground".into(),
            texture: TextureRef::Own("assets/g.png".into()),
            ..Default::default()
        });
        p.materials.push(Material {
            id: "sky".into(),
            flags: MaterialFlags {
                sky: true,
                ..Default::default()
            },
            ..Default::default()
        });
        p.terrain = Some(Terrain {
            cells_x: 2,
            cells_z: 2,
            layers: vec![TerrainLayer {
                material: "ground".into(),
            }],
            ..Default::default()
        });
        let h = Heights::for_terrain(p.terrain.as_ref().unwrap(), 0);
        p.solids.push(Solid {
            id: "solid_1".into(),
            material: "ground".into(),
            ..Default::default()
        });
        p.solids.push(Solid {
            id: "solid_2".into(),
            material: "sky".into(),
            ..Default::default()
        });
        p.solids.push(Solid {
            id: "solid_3".into(),
            material: "ground".into(),
            brush: Some(BrushRole::KillVolume),
            ..Default::default()
        });
        let assets = Assets::new(Path::new("."));
        let l = level_of(&p, &h, &assets).unwrap();
        assert_eq!(l.polys.len(), 8 + 6);
        assert_eq!(l.sky_faces.len(), 6);
        assert_eq!(l.brushes.len(), 1);
        assert_eq!(l.brushes[0].name, "solid_3");
        assert!(l
            .sky_faces
            .iter()
            .all(|p| p.flags & SURF_SKY != 0 && p.flags & SURF_SOLID != 0));
        assert!(l.extent_max.x >= 2048.0 - 64.0 * 62.0);
        p.solids[0].material = "nope".into();
        let e = level_of(&p, &h, &assets).unwrap_err();
        assert_eq!(e.items[0].code, Code::MaterialMissing);
        // the build refuses a project with errors
        let e = build(&p, &h, &assets, &BuildSettings::default()).unwrap_err();
        assert!(e.has_errors());
    }

    #[test]
    fn own_textures_outside_assets_are_refused() {
        let mut p = Project::default();
        p.materials.push(Material {
            id: "x".into(),
            texture: TextureRef::Own("textures/x.png".into()),
            ..Default::default()
        });
        let mut d = Diagnostics::new();
        let names = texture_names(&p, &mut d);
        assert_eq!(names[0], b"@mod:untitled_map/textures/x.dtx");
        assert_eq!(d.items[0].code, Code::TextureAssets);
    }
}

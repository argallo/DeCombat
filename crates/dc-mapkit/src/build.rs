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
    let bake_in = BakeInput {
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

    // the level: every level polygon plus the sky faces in the BSP model; render data without the sky faces
    let mut level_polys = level.polys.clone();
    level_polys.extend(level.sky_faces.iter().cloned());
    let mut level_sources = level.poly_sources.clone();
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
    let colours = bake::bake_polys(&bake_in, &level.polys);
    let (level_render, counts) =
        worldgen::render_world(&level.polys, &colours, &textures, false, &level.sky_faces);
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
    let world = SynthWorld {
        info: INFO_TEXT.as_bytes().to_vec(),
        extent_min: extent_min.to_array(),
        extent_max: extent_max.to_array(),
        world_offset: [0.0; 3],
        tree_shape: vec![false],
        models,
        objects,
        blind: Vec::new(),
        grid_origin: extent_min.to_array(),
        grid_cell: [span.x.max(1.0), span.y.max(1.0), span.z.max(1.0)],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![a[0], a[1], a[2]],
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

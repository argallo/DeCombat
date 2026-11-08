//! The studio without a window: the guns, the gun on show, its design and the picture of it. The window and the headless tools
//! both drive this, so an agent's picture is the window's picture.
// DC-DEFAULT: VAR-6 (proposed) the studio draws one gun through the game's first-person pass, turned by the mouse, with fixed
// studio lights, on a plain background

use crate::design::Design;
use crate::guns::{read_guns, Gun, View};
use crate::lens::{self, Lens};
use crate::shape::{self, Shape};
use crate::whole;
use dc_render::{
    AdapterChoice, Camera, FrameImage, FrameOptions, GpuContext, GpuScene, LightSet, ModelFiles,
    ModelInstance, ModelLight, ModelLook, OffscreenTarget, PlayerViewSettings, RenderSettings,
    RomTextures, SceneMesh, StyleChoice, StyleLibrary, TextureImage, TextureIndex, Tracker,
    WorldRenderer, OFFSCREEN_FORMAT,
};
use dc_rez::RezVfs;
use glam::{Mat4, Quat, Vec3};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The camera's vertical field of view (degrees): long enough that a gun looks like itself.
const FOV_Y_DEG: f32 = 28.0;

/// How the gun is turned and framed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    /// Degrees about the vertical axis.
    pub yaw: f32,
    /// Degrees about the screen's horizontal axis.
    pub pitch: f32,
    /// 1 fits the gun; larger is closer.
    pub zoom: f32,
    /// Moved across the screen, in fractions of the gun's size.
    pub pan: [f32; 2],
}

impl Default for Orbit {
    fn default() -> Self {
        Orbit {
            yaw: -90.0,
            pitch: 8.0,
            zoom: 1.0,
            pan: [0.0, 0.0],
        }
    }
}

pub const ZOOM_RANGE: (f32, f32) = (0.5, 12.0);

impl Orbit {
    /// Turns by a mouse drag, in degrees.
    pub fn turn(&mut self, yaw: f32, pitch: f32) {
        self.yaw = (self.yaw + yaw + 180.0).rem_euclid(360.0) - 180.0;
        self.pitch = (self.pitch + pitch).clamp(-89.0, 89.0);
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
    }

    /// The gun's rotation: turned about its centre, the pitch last, so it tips toward the viewer whatever its yaw.
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_x(self.pitch.to_radians())
            * Quat::from_rotation_y(self.yaw.to_radians())
    }

    /// As [`Orbit::rotation`] for a model of `view`: third-person models point the other way, so both start on the same side.
    pub fn rotation_of(&self, view: View) -> Quat {
        let flip = match view {
            View::FirstPerson => 0.0,
            View::ThirdPerson => std::f32::consts::PI,
        };
        Quat::from_rotation_x(self.pitch.to_radians())
            * Quat::from_rotation_y(self.yaw.to_radians() + flip)
    }

    /// The distance of the gun's centre from the eye, for a gun of `radius`.
    pub fn distance(&self, radius: f32) -> f32 {
        radius.max(1e-3) / (FOV_Y_DEG.to_radians() * 0.5).sin() * 0.72 / self.zoom
    }
}

/// The plain colours behind the gun: a camo has to be judged on more than one.
pub const BACKGROUNDS: &[(&str, [u8; 3])] = &[
    ("dark", [28, 30, 34]),
    ("grey", [96, 98, 102]),
    ("light", [196, 198, 202]),
];

/// The gun on show.
struct Loaded {
    row: u32,
    view: View,
    /// With the hands, and without.
    files: ModelFiles,
    files_gun: ModelFiles,
    /// Which body pieces are the gun's: the hands are the others.
    has_hands: bool,
    skins: Vec<Option<TextureImage>>,
    styles: Vec<Option<Arc<dc_render::RenderStyle>>>,
    prepared: Prepared,
    instance: ModelInstance,
    /// A one-sided model's reflection, which closes it (`shape`), and the shape it was measured to have.
    mirror: Option<(ModelInstance, Shape)>,
    centre: Vec3,
    radius: f32,
    /// The texture on show in the main slot, for the texture panel.
    shown: Option<Arc<TextureImage>>,
    base: Option<Arc<TextureImage>>,
    /// The whole gun for this first-person model, when its third-person model fits it.
    whole: Option<Arc<WholeGun>>,
    /// Whether the instance is the whole gun.
    draws_whole: bool,
}

/// The third-person model, ready to be drawn with the first-person texture baked onto it (`whole`).
struct WholeGun {
    files: ModelFiles,
    mesh: dc_render::ModelMesh,
    skins: Vec<Option<TextureImage>>,
    styles: Vec<Option<Arc<dc_render::RenderStyle>>>,
    /// The skin slot of its body, which the bake replaces.
    slot: usize,
    /// Its own texture at the bake's size, and how much larger that is.
    own: TextureImage,
    scale: f32,
    lookup: whole::Lookup,
    bounds: (Vec3, Vec3),
    /// How well the two models fit, and the turn that stands this one the way the first-person one stands.
    fit: whole::Fit,
    align: Quat,
}

/// What is measured once per gun and view.
struct Prepared {
    /// The gun without its hands or loose parts.
    mesh: dc_render::ModelMesh,
    /// The triangles its reflection adds, when it needs one, and the line it is reflected across.
    mirror: Option<(dc_render::ModelMesh, Shape)>,
    shape: Option<Shape>,
    bones: Vec<Mat4>,
    bounds: (Vec3, Vec3),
    lenses: Vec<Lens>,
}

/// How the whole gun's two models fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WholeFit {
    /// How much their outlines from the side overlap.
    pub overlap: f32,
    /// How far the third-person model's nose is down against the first-person one, in degrees.
    pub tilt: f32,
    /// The share of the third-person surface that reads the first-person texture.
    pub matched: f32,
    /// How far on average those surfaces were apart, as a share of the gun's length.
    pub gap: f32,
}

/// One part of a texture: where it is on the texture and on the gun.
#[derive(Debug, Clone, PartialEq)]
pub struct TexturePart {
    /// Left, top, right, bottom, in texels.
    pub texels: [f32; 4],
    /// Its share of the gun's surface using this texture.
    pub share: f32,
    /// In words, such as "front, high, facing up".
    pub place: String,
}

/// The idle clip when a model has one, held at its start: the pose the player knows the gun in.
fn idle_pose(inst: &mut ModelInstance) {
    let clip = inst
        .anim
        .find_clip(|n| dc_render::name_key(&n.to_ascii_lowercase()) == dc_render::CLIP_VIEW_IDLE)
        .or((inst.anim.clip_count() > 0).then_some(0));
    if let Some(c) = clip {
        inst.set_pose(&[Tracker::main(c, 0.0)]);
    }
}

/// What an upload made.
struct Uploaded {
    instance: ModelInstance,
    mirror: Option<(ModelInstance, Shape)>,
    shown: Option<Arc<TextureImage>>,
    centre: Vec3,
    radius: f32,
    draws_whole: bool,
}

fn bounds_corners(lo: Vec3, hi: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|k| {
        Vec3::new(
            if k & 1 == 0 { lo.x } else { hi.x },
            if k & 2 == 0 { lo.y } else { hi.y },
            if k & 4 == 0 { lo.z } else { hi.z },
        )
    })
}

/// Whether a turn shows the side a one-sided model does not have: the viewer, at -z in the eye's space, is across the centre line from
/// the side the model has.
pub fn faces_far_side(orbit: &Orbit, view: View, shape: &Shape) -> bool {
    let toward_viewer = orbit.rotation_of(view).inverse() * Vec3::NEG_Z;
    toward_viewer.dot(shape.across) < 0.0
}

/// The studio.
pub struct Studio {
    pub ctx: GpuContext,
    renderer: WorldRenderer,
    scene: GpuScene,
    rom: PathBuf,
    vfs: RezVfs,
    textures: TextureIndex,
    styles: StyleLibrary,
    /// What the first-person placeholders stand for, as the game fills them (hands from a character, sleeves from the arm texture).
    hands_skin: Option<String>,
    arm_skin: Option<String>,
    pub guns: Vec<Gun>,
    pub design: Design,
    pub orbit: Orbit,
    pub background: [u8; 3],
    pub show_hands: bool,
    pub view: View,
    /// Draw a first-person gun as the whole gun when it can be.
    pub whole_gun: bool,
    /// The last whole gun worked out: going back and forth between the views does not work it out again.
    whole_made: Option<(u32, Option<Arc<WholeGun>>)>,
    /// How much the last gun's two models overlapped when that was too little.
    whole_refused: Option<f32>,
    loaded: Option<Loaded>,
    /// Counts every change of what is drawn: a window redraws its texture panel when this moves.
    pub revision: u64,
}

/// A style list that draws both sides of every face, for slots up to `slots`. The game's first-person models hold only the side the
/// player sees, so from the far side they would be a few floating parts. The studio's own copy: the game's styles are not touched.
// DC-DEFAULT: VAR-6 (proposed; USER REPORT 2026-11-05 "Guns are only rendering one side")
pub fn double_sided(
    styles: &[Option<Arc<dc_render::RenderStyle>>],
    slots: usize,
) -> Vec<Option<Arc<dc_render::RenderStyle>>> {
    (0..styles.len().max(slots))
        .map(|i| {
            let mut style = match styles.get(i).and_then(|s| s.as_deref()) {
                Some(s) => s.clone(),
                None => dc_render::RenderStyle::engine_default(),
            };
            for pass in &mut style.passes {
                pass.cull = dc_render::Cull::None;
            }
            Some(Arc::new(style))
        })
        .collect()
}

/// The style slots a model's pieces name.
fn style_slots(files: &ModelFiles) -> usize {
    files
        .body
        .pieces
        .iter()
        .flat_map(|p| p.lods.iter())
        .map(|l| l.style_index as usize + 1)
        .max()
        .unwrap_or(0)
}

fn neutral_lights() -> LightSet {
    // fixed in the room, so the shine moves over the gun as it turns; the key from above and the viewer's left, a fill from the right
    LightSet {
        ambient: Vec3::splat(96.0),
        lights: vec![
            ModelLight::sun(Vec3::new(0.45, -0.7, 0.55), Vec3::splat(175.0)),
            ModelLight::sun(Vec3::new(-0.7, -0.15, 0.7), Vec3::splat(70.0)),
        ],
    }
}

impl Studio {
    /// Opens a game folder on its own GPU device.
    pub fn open(rom: &Path) -> Result<Studio, String> {
        let ctx = GpuContext::headless(AdapterChoice::Auto).map_err(|e| e.to_string())?;
        Studio::with_context(rom, ctx)
    }

    /// Opens a game folder on a device the caller shares.
    pub fn with_context(rom: &Path, ctx: GpuContext) -> Result<Studio, String> {
        let guns = read_guns(rom)?;
        let textures = TextureIndex::open(rom).map_err(|e| format!("game textures: {e}"))?;
        let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
        let scene = renderer.upload(&ctx, &SceneMesh::default(), &mut |_| None);
        let vfs = RezVfs::new(rom);
        let hands_skin = dc_render::CharacterTable::from_vfs(&vfs)
            .ok()
            .and_then(|t| {
                t.bodies
                    .keys()
                    .find_map(|row| t.hands_skin(*row, 0, 0).filter(|s| !s.is_empty()))
            });
        Ok(Studio {
            ctx,
            renderer,
            scene,
            rom: rom.to_path_buf(),
            vfs,
            textures,
            styles: StyleLibrary::new(),
            hands_skin,
            arm_skin: dc_render::pv_none_texture(rom),
            guns,
            design: Design::default(),
            orbit: Orbit::default(),
            background: BACKGROUNDS[0].1,
            show_hands: false,
            view: View::FirstPerson,
            whole_gun: true,
            whole_made: None,
            whole_refused: None,
            loaded: None,
            revision: 0,
        })
    }

    pub fn rom(&self) -> &Path {
        &self.rom
    }

    pub fn renderer(&self) -> &WorldRenderer {
        &self.renderer
    }

    pub fn gun(&self) -> Option<&Gun> {
        let row = self.loaded.as_ref()?.row;
        self.guns.iter().find(|g| g.row == row)
    }

    pub fn row(&self) -> Option<u32> {
        self.loaded.as_ref().map(|l| l.row)
    }

    /// The gun of a row, or of a name (any case; a part of a name when it fits one gun best).
    pub fn find_gun(&self, what: &str) -> Option<u32> {
        let t = what.trim();
        if let Ok(n) = t.parse::<u32>() {
            return self.guns.iter().find(|g| g.row == n).map(|g| g.row);
        }
        let lower = t.to_ascii_lowercase();
        self.guns
            .iter()
            .find(|g| g.name.eq_ignore_ascii_case(t))
            .or_else(|| {
                self.guns
                    .iter()
                    .filter(|g| g.name.to_ascii_lowercase().contains(&lower))
                    .min_by_key(|g| g.name.len())
            })
            .map(|g| g.row)
    }

    /// Shows a gun: its model for the current view, or its other one when it has only that.
    pub fn select(&mut self, row: u32) -> Result<(), String> {
        let gun = self
            .guns
            .iter()
            .find(|g| g.row == row)
            .ok_or_else(|| format!("no gun in row {row}"))?;
        let view = if gun.model(self.view).is_some() {
            self.view
        } else if gun.first_person.is_some() {
            View::FirstPerson
        } else {
            View::ThirdPerson
        };
        self.load(row, view)
    }

    /// Shows the current gun's other model. The turn and zoom stay.
    pub fn set_view(&mut self, view: View) -> Result<(), String> {
        let row = self.row().ok_or("no gun is selected")?;
        self.load(row, view)
    }

    pub fn set_hands(&mut self, on: bool) -> Result<(), String> {
        self.show_hands = on;
        self.repaint()
    }

    fn load(&mut self, row: u32, view: View) -> Result<(), String> {
        let gun = self
            .guns
            .iter()
            .find(|g| g.row == row)
            .ok_or_else(|| format!("no gun in row {row}"))?;
        let model = gun
            .model(view)
            .ok_or_else(|| format!("{} has no {} model", gun.name, view.label()))?
            .clone();
        let name = gun.name.clone();
        let has_scope = gun.has_scope;
        let unreadable =
            |e: dc_render::ModelLoadError| format!("this game folder cannot show the {name}: {e}");
        let files = ModelFiles::load(&self.vfs, &model.model).map_err(unreadable)?;
        let mut files_gun = ModelFiles::load(&self.vfs, &model.model).map_err(unreadable)?;
        let main = view.main_slot() as u32;
        // the hands use the skin slots before the gun's; pieces of a model with no other slot all stay
        let is_gun = |p: &dc_model::Piece| {
            p.lods
                .first()
                .is_none_or(|l| l.slot_count == 0 || l.slots[0] >= main)
        };
        let before = files_gun.body.pieces.len();
        files_gun.body.pieces.retain(is_gun);
        let has_hands = files_gun.body.pieces.len() < before && !files_gun.body.pieces.is_empty();
        if files_gun.body.pieces.is_empty() {
            files_gun = ModelFiles::load(&self.vfs, &model.model).map_err(unreadable)?;
        }
        // the hands and sleeves are placeholders in the weapon table, which the game fills from the character
        let names = match view {
            View::FirstPerson => dc_render::resolve_view_skins(
                &model.skins,
                self.hands_skin.as_deref(),
                self.arm_skin.as_deref(),
            ),
            View::ThirdPerson => model.skins.clone(),
        };
        let mut rom = RomTextures::new(&self.textures);
        let skins: Vec<Option<TextureImage>> =
            names.iter().map(|n| rom.image(n.as_bytes())).collect();
        let styles = self.styles.list(&self.vfs, &model.styles);
        let base = skins.get(view.main_slot()).cloned().flatten().map(Arc::new);
        let size = base.as_ref().map_or((1, 1), |b| (b.width, b.height));
        let prepared = self.prepare(&files_gun, view, has_scope, size);
        if !prepared.lenses.is_empty() {
            let circles = prepared.lenses.iter().map(|l| l.circle).collect();
            self.design.keep.entry((row, view)).or_insert(circles);
        }
        let same_gun = self.loaded.as_ref().is_some_and(|l| l.row == row);
        let whole = if view == View::FirstPerson && self.whole_gun {
            self.whole_of(row, &prepared, &skins)
        } else {
            None
        };
        let up = self.upload(
            row,
            view,
            &files,
            &files_gun,
            &prepared,
            &skins,
            &styles,
            base.as_deref(),
            whole.as_deref(),
        );
        self.view = view;
        if !same_gun {
            let kept = self.orbit;
            self.orbit = Orbit {
                zoom: 1.0,
                pan: [0.0, 0.0],
                ..kept
            };
        }
        self.loaded = Some(Loaded {
            row,
            view,
            files,
            files_gun,
            has_hands,
            skins,
            styles,
            prepared,
            instance: up.instance,
            mirror: up.mirror,
            centre: up.centre,
            radius: up.radius,
            shown: up.shown,
            base,
            whole,
            draws_whole: up.draws_whole,
        });
        self.revision += 1;
        Ok(())
    }

    /// The whole gun for a first-person model, worked out once per gun. `None`: it has no third-person model, or that is not close enough to the same gun.
    fn whole_of(
        &mut self,
        row: u32,
        pv: &Prepared,
        pv_skins: &[Option<TextureImage>],
    ) -> Option<Arc<WholeGun>> {
        if let Some((made, w)) = &self.whole_made {
            if *made == row {
                return w.clone();
            }
        }
        self.whole_refused = None;
        let w = self.make_whole(row, pv, pv_skins).map(Arc::new);
        self.whole_made = Some((row, w.clone()));
        w
    }

    fn make_whole(
        &mut self,
        row: u32,
        pv: &Prepared,
        pv_skins: &[Option<TextureImage>],
    ) -> Option<WholeGun> {
        let gun = self.guns.iter().find(|g| g.row == row)?;
        let has_scope = gun.has_scope;
        let model = gun.third_person.clone()?;
        let pv_shape = pv.shape?;
        let files = ModelFiles::load(&self.vfs, &model.model).ok()?;
        let mut rom = RomTextures::new(&self.textures);
        let skins: Vec<Option<TextureImage>> = model
            .skins
            .iter()
            .map(|n| rom.image(n.as_bytes()))
            .collect();
        // the body is on the skin with the most surface: mostly the first, but some models keep an attachment there
        let probe = self.prepare(&files, View::ThirdPerson, has_scope, (1, 1));
        let area_of = |slot: usize, size: (u32, u32)| -> f32 {
            lens::skin_triangles(&probe.mesh, &probe.bones, slot as u32, size)
                .iter()
                .map(|t| t.area)
                .sum()
        };
        let slot = (0..skins.len())
            .filter(|k| skins[*k].is_some())
            .max_by(|a, b| area_of(*a, (1, 1)).total_cmp(&area_of(*b, (1, 1))))?;
        let base = skins[slot].as_ref()?;
        let size = (base.width, base.height);
        let hh = self.prepare(&files, View::ThirdPerson, has_scope, size);
        let hh_shape = hh.shape?;
        let hh_tris = lens::skin_triangles(&hh.mesh, &hh.bones, slot as u32, size);
        let sources: Vec<whole::Source> = pv_skins
            .iter()
            .enumerate()
            .map(|(k, s)| whole::Source {
                tris: s.as_ref().map_or_else(Vec::new, |s| {
                    lens::skin_triangles(&pv.mesh, &pv.bones, k as u32, (s.width, s.height))
                }),
            })
            .collect();
        let pv_all: Vec<lens::SkinTri> = sources.iter().flat_map(|s| s.tris.clone()).collect();
        let fit = whole::fit(&hh_tris, &hh_shape, &pv_all, &pv_shape)?;
        if fit.overlap < whole::MIN_OVERLAP {
            self.whole_refused = Some(fit.overlap);
            return None;
        }
        let side = whole::BAKE_SIDE;
        let lookup = whole::bake_lookup(&hh_tris, size, &fit, &pv_shape, &sources, side)?;
        if lookup.gap > whole::MAX_GAP {
            self.whole_refused = Some(fit.overlap);
            return None;
        }
        let mut lookup = lookup;
        let hh_lenses: Vec<_> = hh.lenses.iter().map(|l| l.circle).collect();
        let pv_lenses: Vec<_> = pv.lenses.iter().map(|l| l.circle).collect();
        whole::keep_lenses(
            &mut lookup,
            &hh_lenses,
            size,
            &pv_lenses,
            View::FirstPerson.main_slot() as u16,
        );
        let own = whole::enlarged(base, side);
        let scale = side as f32 / size.0.max(1) as f32;
        if !hh.lenses.is_empty() {
            let circles = hh.lenses.iter().map(|l| l.circle).collect();
            self.design
                .keep
                .entry((row, View::ThirdPerson))
                .or_insert(circles);
        }
        let styles = self.styles.list(&self.vfs, &model.styles);
        Some(WholeGun {
            files,
            mesh: hh.mesh,
            skins,
            styles,
            slot,
            own,
            scale,
            lookup,
            bounds: hh.bounds,
            align: fit.align(),
            fit,
        })
    }

    /// How well the gun on show fits, when it is drawn as the whole gun.
    pub fn whole_fit(&self) -> Option<WholeFit> {
        let l = self.loaded.as_ref().filter(|l| l.draws_whole)?;
        let w = l.whole.as_ref()?;
        Some(WholeFit {
            overlap: w.fit.overlap,
            tilt: w.fit.tilt,
            matched: w.lookup.matched,
            gap: w.lookup.gap,
        })
    }

    /// How much the gun on show's two models overlapped, when that was too little to draw it whole.
    pub fn whole_refused(&self) -> Option<f32> {
        self.whole_refused
    }

    /// Turns the whole gun on or off. The turn and zoom stay.
    pub fn set_whole_gun(&mut self, on: bool) -> Result<(), String> {
        self.whole_gun = on;
        match self.row() {
            Some(row) => self.load(row, self.view),
            None => Ok(()),
        }
    }

    /// What is measured once per gun and view: the gun without its loose parts, what its reflection adds, and its lenses.
    fn prepare(
        &self,
        files_gun: &ModelFiles,
        view: View,
        has_scope: bool,
        size: (u32, u32),
    ) -> Prepared {
        let look = ModelLook {
            textures: &[],
            styles: StyleChoice::Plain,
        };
        let mut probe = self.renderer.upload_model_look(&self.ctx, files_gun, look);
        idle_pose(&mut probe);
        let bones = probe.bones.clone();
        let whole = dc_render::ModelMesh::build(&files_gun.body);
        let mesh = match view {
            View::FirstPerson => shape::without_loose_parts(&whole, &bones),
            View::ThirdPerson => whole,
        };
        let shape = shape::measure(&shape::posed_triangles(&mesh, &bones));
        let mirror = shape
            .filter(|s| view == View::FirstPerson && s.one_sided())
            .and_then(|s| shape::reflected_mesh(&mesh, &bones, &s).map(|m| (m, s)));
        let lenses = shape.map_or_else(Vec::new, |s| {
            let long = Vec3::new(s.across.z, 0.0, -s.across.x);
            let tris = lens::skin_triangles(&mesh, &bones, view.main_slot() as u32, size);
            let mut found = lens::scope_lenses(&lens::find_lenses(&tris, long), has_scope);
            // a low model's lens is too plain to be told by its shape, and the two ends of a scope are told as a pair
            if view == View::ThirdPerson && has_scope {
                for cap in lens::scope_caps(&tris, long, size, lens::skins_used(&mesh) == 1) {
                    let known = found.iter().any(|l| {
                        (l.circle.x - cap.circle.x).hypot(l.circle.y - cap.circle.y)
                            < l.circle.radius.max(cap.circle.radius)
                    });
                    if !known {
                        found.push(cap);
                    }
                }
            }
            found
        });
        let bounds =
            shape::mesh_bounds(&mesh, &bones).unwrap_or((Vec3::splat(-1.0), Vec3::splat(1.0)));
        Prepared {
            mesh,
            mirror,
            shape,
            bones,
            bounds,
            lenses,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn upload(
        &self,
        row: u32,
        view: View,
        files: &ModelFiles,
        files_gun: &ModelFiles,
        prepared: &Prepared,
        skins: &[Option<TextureImage>],
        styles: &[Option<Arc<dc_render::RenderStyle>>],
        base: Option<&TextureImage>,
        whole: Option<&WholeGun>,
    ) -> Uploaded {
        let shown = base.and_then(|b| self.design.painted(row, view, b));
        let mut images = skins.to_vec();
        if let (Some(s), Some(slot)) = (shown.clone(), images.get_mut(view.main_slot())) {
            *slot = Some(s);
        }
        let both_sides = (view == View::FirstPerson)
            .then(|| double_sided(styles, style_slots(files).max(style_slots(files_gun))));
        let style_list = both_sides.as_deref().unwrap_or(styles);
        let look = || ModelLook {
            textures: &images,
            styles: StyleChoice::List(style_list),
        };
        let ready = |mut inst: ModelInstance| {
            inst.lod_distance = Some(0.0);
            inst.lights = neutral_lights();
            idle_pose(&mut inst);
            inst
        };
        let models = self.renderer.models();
        let shown = shown.map(Arc::new).or_else(|| base.cloned().map(Arc::new));
        // with the hands on show the model stays as the player sees it
        if self.show_hands {
            let inst = ready(self.renderer.upload_model_look(&self.ctx, files, look()));
            let (lo, hi) = inst
                .posed_bounds()
                .unwrap_or((Vec3::splat(-1.0), Vec3::splat(1.0)));
            return Uploaded {
                instance: inst,
                mirror: None,
                shown,
                centre: (lo + hi) * 0.5,
                radius: (hi - lo).length() * 0.5,
                draws_whole: false,
            };
        }
        if let Some(w) = whole {
            // the first-person texture, as painted, goes onto the third-person model; what found no place on it is that model's own, painted the same
            let painted = images.iter().map(Option::as_ref).collect::<Vec<_>>();
            let own = self
                .design
                .painted_scaled(row, View::ThirdPerson, &w.own, w.scale);
            let own = own.as_ref().unwrap_or(&w.own);
            let mut baked = whole::bake(&w.lookup, &painted, own);
            baked.fx = own.fx.clone();
            let mut hh_images = w.skins.clone();
            if let Some(slot) = hh_images.get_mut(w.slot) {
                *slot = Some(baked);
            }
            let hh_look = ModelLook {
                textures: &hh_images,
                styles: StyleChoice::List(&w.styles),
            };
            let inst = ready(models.upload_mesh_look(&self.ctx, &w.files, w.mesh.clone(), hh_look));
            let (lo, hi) = w.bounds;
            return Uploaded {
                instance: inst,
                mirror: None,
                shown,
                centre: (lo + hi) * 0.5,
                radius: (hi - lo).length() * 0.5,
                draws_whole: true,
            };
        }
        let inst =
            ready(models.upload_mesh_look(&self.ctx, files_gun, prepared.mesh.clone(), look()));
        let (lo, hi) = prepared.bounds;
        let mut centre = (lo + hi) * 0.5;
        let mut radius = (hi - lo).length() * 0.5;
        let mirror = prepared.mirror.as_ref().map(|(mesh, shape)| {
            let m = ready(models.upload_mesh_look(&self.ctx, files_gun, mesh.clone(), look()));
            // the gun turns about its centre line
            centre += shape.across * (shape.centre - centre.dot(shape.across));
            radius = bounds_corners(lo, hi)
                .iter()
                .map(|c| (*c - centre).length())
                .fold(0.0, f32::max);
            (m, *shape)
        });
        Uploaded {
            instance: inst,
            mirror,
            shown,
            centre,
            radius,
            draws_whole: false,
        }
    }

    /// The lenses found on the gun and view on show: only a gun the game gives a sight has any.
    pub fn lenses(&self) -> &[Lens] {
        self.loaded.as_ref().map_or(&[], |l| &l.prepared.lenses)
    }

    /// Goes back to the lenses found for this gun and view, instead of a list set by hand.
    pub fn use_found_lenses(&mut self) -> Result<(), String> {
        let (row, view) = (self.row().ok_or("no gun is on show")?, self.view);
        self.design.keep.remove(&(row, view));
        self.repaint()
    }

    /// The parts the texture on show is cut into, largest on the gun first: where each lies on the texture and on the gun.
    pub fn texture_parts(&self, limit: usize) -> Vec<TexturePart> {
        let Some(l) = &self.loaded else {
            return Vec::new();
        };
        let Some(base) = &l.base else {
            return Vec::new();
        };
        let tris = lens::skin_triangles(
            &l.prepared.mesh,
            &l.prepared.bones,
            l.view.main_slot() as u32,
            (base.width, base.height),
        );
        let Some(shape) = l.prepared.shape else {
            return Vec::new();
        };
        let (lo, hi) = l.prepared.bounds;
        let height = (hi.y - lo.y).max(1e-6);
        let along = |p: Vec3| p.dot(shape.toward_muzzle);
        let corners = bounds_corners(lo, hi);
        let (rear, front) = corners.iter().fold((f32::MAX, f32::MIN), |(a, b), c| {
            (a.min(along(*c)), b.max(along(*c)))
        });
        let total: f32 = tris.iter().map(|t| t.area).sum();
        let mut out: Vec<TexturePart> = lens::parts(&tris)
            .iter()
            .map(|group| {
                let (mut tmin, mut tmax) =
                    (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
                let (mut area, mut mid, mut normal) = (0.0, Vec3::ZERO, Vec3::ZERO);
                for &k in group {
                    let t = &tris[k];
                    for q in t.texels {
                        tmin = tmin.min(q);
                        tmax = tmax.max(q);
                    }
                    area += t.area;
                    mid += (t.points[0] + t.points[1] + t.points[2]) / 3.0 * t.area;
                    normal += t.normal * t.area;
                }
                let mid = mid / f32::max(area, 1e-6);
                let run = (along(mid) - rear) / (front - rear).max(1e-6);
                let up = (mid.y - lo.y) / height;
                let n = normal.normalize_or_zero();
                TexturePart {
                    texels: [tmin.x, tmin.y, tmax.x, tmax.y],
                    share: area / total.max(1e-6),
                    place: format!(
                        "{}, {}{}",
                        match run {
                            r if r < 0.34 => "rear",
                            r if r < 0.67 => "middle",
                            _ => "front",
                        },
                        match up {
                            u if u < 0.34 => "low",
                            u if u < 0.67 => "mid height",
                            _ => "high",
                        },
                        match () {
                            _ if n.y > 0.7 => ", facing up",
                            _ if n.y < -0.7 => ", facing down",
                            _ if n.dot(shape.toward_muzzle).abs() > 0.7 => ", facing along the gun",
                            _ => "",
                        },
                    ),
                }
            })
            .collect();
        out.sort_by(|a, b| b.share.total_cmp(&a.share));
        out.truncate(limit);
        out
    }

    /// Whether the side facing the viewer is a reflection of the player's side, which the game's model does not have.
    pub fn far_side_is_reflected(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| {
            l.mirror
                .as_ref()
                .is_some_and(|(_, s)| faces_far_side(&self.orbit, l.view, s))
        })
    }

    /// Whether this model was closed with its reflection.
    pub fn is_reflected(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.mirror.is_some())
    }

    /// The shape measured for the reflection, when the model has one.
    pub fn reflection_shape(&self) -> Option<Shape> {
        self.loaded.as_ref()?.mirror.as_ref().map(|(_, s)| *s)
    }

    /// Paints again after the design changed.
    pub fn repaint(&mut self) -> Result<(), String> {
        let Some(l) = self.loaded.take() else {
            return Ok(());
        };
        if !l.prepared.lenses.is_empty() {
            let circles = l.prepared.lenses.iter().map(|x| x.circle).collect();
            self.design.keep.entry((l.row, l.view)).or_insert(circles);
        }
        let up = self.upload(
            l.row,
            l.view,
            &l.files,
            &l.files_gun,
            &l.prepared,
            &l.skins,
            &l.styles,
            l.base.as_deref(),
            l.whole.as_deref(),
        );
        self.loaded = Some(Loaded {
            instance: up.instance,
            mirror: up.mirror,
            shown: up.shown,
            centre: up.centre,
            radius: up.radius,
            draws_whole: up.draws_whole,
            ..l
        });
        self.revision += 1;
        Ok(())
    }

    /// Reads again any art whose file changed, and paints when one did. Returns what went wrong, when anything did.
    pub fn watch_files(&mut self) -> Option<String> {
        let (changed, error) = self.design.reload_changed();
        if changed {
            if let Err(e) = self.repaint() {
                return Some(e);
            }
        }
        error
    }

    /// The game's texture of a view of the gun on show, read for a save's check.
    fn base_of(&self, view: View) -> Option<TextureImage> {
        let gun = self.gun()?;
        let name = gun.model(view)?.skins.get(view.main_slot())?;
        RomTextures::new(&self.textures).image(name.as_bytes())
    }

    /// The lenses of a view of the gun on show, read for a save when that view has not been on show.
    fn lenses_of(&self, view: View) -> Vec<Lens> {
        let (Some(gun), Some(base)) = (self.gun(), self.base_of(view)) else {
            return Vec::new();
        };
        let Some(model) = gun.model(view) else {
            return Vec::new();
        };
        let Ok(mut files) = ModelFiles::load(&self.vfs, &model.model) else {
            return Vec::new();
        };
        let main = view.main_slot() as u32;
        files.body.pieces.retain(|p| {
            p.lods
                .first()
                .is_none_or(|l| l.slot_count == 0 || l.slots[0] >= main)
        });
        self.prepare(&files, view, gun.has_scope, (base.width, base.height))
            .lenses
    }

    /// Saves the design for the gun on show as a pack, with the lenses of both of its models kept clean.
    pub fn save_pack(&self, opts: &crate::pack::SaveOptions) -> Result<crate::pack::Saved, String> {
        let gun = self.gun().ok_or("no gun is on show")?;
        let mut design = self.design.clone();
        for view in [View::FirstPerson, View::ThirdPerson] {
            if let std::collections::btree_map::Entry::Vacant(free) =
                design.keep.entry((gun.row, view))
            {
                let circles: Vec<_> = self.lenses_of(view).iter().map(|l| l.circle).collect();
                if !circles.is_empty() {
                    free.insert(circles);
                }
            }
        }
        crate::pack::save(&design, gun, &|v| self.base_of(v), opts)
    }

    /// The gun's main texture as the game has it, from the player's own ROM.
    pub fn base_texture(&self) -> Option<Arc<TextureImage>> {
        self.loaded.as_ref()?.base.clone()
    }

    /// The texture on show in the main slot.
    pub fn shown_texture(&self) -> Option<Arc<TextureImage>> {
        self.loaded.as_ref()?.shown.clone()
    }

    pub fn has_hands(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.has_hands)
    }

    /// The design moves over time, so the picture has to keep drawing.
    pub fn animating(&self) -> bool {
        self.design.fx.is_some() || self.design.flow.is_some()
    }

    /// Draws the gun into `target`.
    pub fn draw(&mut self, target: &OffscreenTarget) {
        let (w, h) = target.size();
        let fov_y = FOV_Y_DEG.to_radians();
        let camera = Camera::looking(
            Vec3::ZERO,
            0.0,
            0.0,
            dc_render::fov_x_for(fov_y, w, h),
            fov_y,
        );
        let [r, g, b] = self.background.map(|c| c as f64 / 255.0);
        let opts = FrameOptions {
            sky: None,
            translucent: false,
            sort_translucent: true,
            clear: Some([r, g, b, 1.0]),
        };
        let orbit = self.orbit;
        let aspect = w as f32 / h.max(1) as f32;
        let mut pv = PlayerViewSettings {
            fov_y_deg: FOV_Y_DEG,
            near: 0.01,
            far: 100.0,
        };
        if let Some(l) = self.loaded.as_mut() {
            let turn = l
                .whole
                .as_ref()
                .filter(|_| l.draws_whole)
                .map_or(Quat::IDENTITY, |w| w.align);
            let rot = orbit.rotation_of(l.view) * turn;
            // a wide window gives the gun's length more room than its height
            let fit = if aspect >= 1.4 { 1.0 } else { 1.4 / aspect };
            let dist = orbit.distance(l.radius) * fit;
            let pan = Vec3::new(orbit.pan[0], orbit.pan[1], 0.0) * l.radius;
            l.instance.rotation = rot;
            l.instance.position = Vec3::new(0.0, 0.0, dist) + pan - rot * l.centre;
            if let Some((m, shape)) = l.mirror.as_mut() {
                let (turn, scale, offset) = shape.reflection();
                m.rotation = rot * turn;
                m.scale = scale;
                m.position = l.instance.position + rot * offset;
            }
            pv.near = (dist - l.radius * 1.3).max(dist * 0.02).max(0.001);
            pv.far = (dist + l.radius * 3.0).max(pv.near * 4.0);
        }
        let list: Vec<&ModelInstance> = self
            .loaded
            .iter()
            .flat_map(|l| std::iter::once(&l.instance).chain(l.mirror.iter().map(|(m, _)| m)))
            .collect();
        self.renderer.render_frame_to_target(
            &self.ctx,
            &self.scene,
            &camera,
            &opts,
            &[],
            &list,
            &pv,
            &[],
            target,
        );
    }

    /// A picture of the gun as it is on show.
    pub fn picture(&mut self, width: u32, height: u32) -> Result<FrameImage, String> {
        let size = (width.clamp(16, 4096), height.clamp(16, 4096));
        let target = OffscreenTarget::for_renderer(&self.ctx, &self.renderer, size);
        self.draw(&target);
        target.read_back(&self.ctx).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_sided_list_covers_every_slot_and_leaves_the_games_styles_alone() {
        let culled = dc_render::RenderStyle::engine_default();
        assert_ne!(culled.passes[0].cull, dc_render::Cull::None);
        let list = vec![None, Some(Arc::new(culled.clone()))];
        let both = double_sided(&list, 4);
        assert_eq!(
            both.len(),
            4,
            "a slot past the list would fall back to a culling style"
        );
        for s in &both {
            let s = s.as_ref().expect("every slot has a style");
            assert!(s.passes.iter().all(|p| p.cull == dc_render::Cull::None));
        }
        assert_eq!(
            list[1].as_ref().unwrap().passes[0].cull,
            culled.passes[0].cull
        );
        assert_eq!(double_sided(&list, 0).len(), 2);
    }

    #[test]
    fn the_far_side_is_the_one_across_the_centre_line_from_the_models_surface() {
        // a model whose surface faces -x, which the default turn shows the viewer
        let shape = Shape {
            across: Vec3::NEG_X,
            centre: 0.7,
            balance: 0.2,
            spread: 0.02,
            toward_muzzle: Vec3::Z,
        };
        let mut o = Orbit {
            pitch: 0.0,
            ..Orbit::default()
        };
        let near = faces_far_side(&o, View::FirstPerson, &shape);
        o.yaw = 90.0;
        assert_ne!(near, faces_far_side(&o, View::FirstPerson, &shape));
    }

    #[test]
    fn a_drag_turns_and_stops_at_the_poles() {
        let mut o = Orbit::default();
        o.turn(10.0, 500.0);
        assert_eq!(o.pitch, 89.0);
        assert_eq!(o.yaw, -80.0);
        o.turn(360.0, -500.0);
        assert_eq!(o.pitch, -89.0);
        assert!((o.yaw + 80.0).abs() < 1e-3);
    }

    #[test]
    fn zoom_stays_in_range_and_closer_is_nearer() {
        let mut o = Orbit::default();
        let far = o.distance(10.0);
        o.zoom_by(2.0);
        assert!(o.distance(10.0) < far);
        o.zoom_by(1000.0);
        assert_eq!(o.zoom, ZOOM_RANGE.1);
        o.zoom_by(1e-6);
        assert_eq!(o.zoom, ZOOM_RANGE.0);
    }
}

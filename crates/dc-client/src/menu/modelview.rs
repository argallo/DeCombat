//! shop-model: the character model of the shop screen's left pane. The 2009 shop shows the player's character with
//! its gear and the held weapon as a 3D model next to the equipment rows; the menu itself is 2D, so the model is
//! drawn offscreen with dc-render on a GPU device of its own (created on first use, on a worker thread), read back
//! and handed to the menu as a runtime texture that one display-list image places in the pane.
//!
//! - The character is the in-match remote body ([`crate::characters::ModelScene`]): body + face (+ weapon at its
//!   socket) posed by the body row's shop clip (`InvenShopAni`), else the first clip, looped at the clip's own
//!   rate: the window asks for the next pose whenever the last one arrived; the headless image path draws a fixed
//!   time ([`ModelView::want`]).
//! - Only the pane is rendered: a pane-sized target with an off-centre projection (the pane's part of the
//!   interface camera's view), so a render per tick reads back the pane only.
//! - The frame is drawn twice, cleared to black and to white: the difference is the models' coverage (alpha), so
//!   translucent pieces keep their blend; the pane's rectangle is cropped from the frame.
//! - The window polls ([`ModelView::poll`]); the headless image path and the tests wait ([`ModelView::wait`]).
// EVIDENCE: cshell+0x1b7ff0 (the interface camera: FovXInterface 57.82, FovYInterface 45 degrees); cshell+0x1e4760
// (the model table's InvenShopAni clip of a body row)
// DC-DEFAULT: CLI-742 (proposed) the shop character: own headless GPU device, coverage from a black and a white clear, placement
// read off the reference stack picture (see [`PLACE`])

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use glam::Vec3;

use crate::hud::compose::{HudAssets, Tex};
use crate::hud::items::DrawItem;
use crate::world::{ModelRef, RemoteBody, RemoteLook, RemoteTracker};

/// The interface camera's field of view (degrees): horizontal, vertical.
// EVIDENCE: cshell+0x1b7ff0 (FovXInterface / FovYInterface defaults 57.82 / 45)
pub const FOV_INTERFACE_DEG: [f32; 2] = [57.82, 45.0];

/// Where the shop character stands in front of the interface camera (the camera at the origin looking along +Z):
/// the model origin (its feet) and its yaw (radians about +Y; pi faces the camera).
// EVIDENCE: cshell+0x12cc60 (the inventory character: position (-40, -15, 205), scale 1, the camera's rotation turned
// by pi, animation mode 3 = the body row's InvenShopAni clip looped, cshell+0xf96c0)
// DC-DEFAULT: CLI-742 (proposed) the frame of that position (the interface camera's place) is not traced; the origin
// is fit to the reference stack picture screens-sweep/cpp/shop/shop_bb00_shop.png (model from y 138 to
// 570, body centre x 132 of 1024 x 768): with the default body 97.5 units tall in its shop clip that is depth 209
// (the 2009 value 205 within 2 %), feet 42 below and 82 left of the eye
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub origin: [f32; 3],
    pub yaw: f32,
}

/// The inventory character's position in the 2009 code; the interface lights are placed in the same frame, so they
/// move with the fitted origin by `PLACE.origin - ORIGIN_2009`.
// EVIDENCE: cshell+0x12cc60 (position (-40, -15, 205))
pub const ORIGIN_2009: [f32; 3] = [-40.0, -15.0, 205.0];

/// The ambient light of the shop character (0..255 per channel).
// DC-DEFAULT: CLI-744 (proposed) the interface's ambient term is not traced; chosen so the lit sides match the reference stack
// picture screens-sweep/cpp/shop/shop_bb00_shop.png
pub const SHOP_AMBIENT: f32 = 60.0;

pub const PLACE: Placement = Placement {
    origin: [-82.0, -42.0, 209.0],
    yaw: std::f32::consts::PI,
};

/// The pane rectangle the model is shown in (virtual 1024 x 768 pixels: x, y, w, h).
// DC-DEFAULT: CLI-742 (proposed) the pane's free part left of the equipment rows (OBSERVED pane1/shop.png: rows from
// x 253, the nickname box from y 585)
pub const PANE_RECT: [i32; 4] = [14, 104, 238, 480];

/// What the character wears and holds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CharLook {
    /// Body first, then the face and the other pieces.
    pub models: Vec<ModelRef>,
    /// The held weapon's third-person model and the body socket it hangs on.
    pub weapon: Option<(ModelRef, u32)>,
    /// The body's render styles when they differ from the table row's (empty: the row's), as a server model block
    /// names them ([`RemoteLook::styles`]).
    pub styles: Vec<String>,
}

/// A request: the look, where it stands, the pane rectangle (virtual pixels) and the target size (pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewReq {
    pub look: Option<CharLook>,
    /// The extra pieces on the body (head gear, goggles, backpack).
    pub pieces: Vec<crate::characters::BodyPiece>,
    /// The interface lights (2009 frame, [`ORIGIN_2009`]); empty: the full-ambient default of a world without
    /// lights.
    pub lights: Vec<super::figure::ShopLight>,
    pub place: Placement,
    pub rect: [i32; 4],
    /// CLI-1629: the x-ray style previewed on the character (the shop page only; `None`: no preview).
    pub xray: Option<crate::settings::XrayStyle>,
    /// CRATE-8: draw this model on its own instead of the character.
    pub showcase: Option<Showcase>,
}

/// CRATE-8: one model shown on its own in front of the interface camera (the case reel's won gun): turned by `yaw`
/// and `pitch` (radians) about the middle of its bounds, which sits on the camera's axis at a depth where the model's
/// length fills `fill` of the request's rectangle; a key light from the front left and a rim light of `rim` from behind.
#[derive(Debug, Clone, PartialEq)]
pub struct Showcase {
    pub model: ModelRef,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fill: f32,
    pub rim: [f32; 3],
}

struct Job {
    gen: u64,
    req: ViewReq,
    size: (u32, u32),
    /// The clip time of the pose (ms; the clip wraps it).
    time_ms: f32,
}

struct Done {
    gen: u64,
    /// The request's pane rectangle (virtual pixels) and yaw.
    rect: [i32; 4],
    yaw: f32,
    tex: Option<Tex>,
}

/// The menu's side of the character view.
pub struct ModelView {
    /// The names its renders are put under (two in turn).
    prefix: String,
    rom: PathBuf,
    jobs: Option<Sender<Job>>,
    done: Option<Receiver<Done>>,
    gen: u64,
    /// The newest generation shown (or dropped by a `None` request): an older render arriving later is stale.
    shown_gen: u64,
    /// The last request sent and its target size.
    asked: Option<(ViewReq, (u32, u32))>,
    /// The clip time of the last request sent.
    asked_time: f32,
    /// The last request was posed on the live clock (so a later pose step makes it due again).
    asked_live: bool,
    /// A render is on its way.
    pending: bool,
    /// The texture shown (name, rectangle in virtual pixels, texture size in texels) for generation `gen`.
    shown: Option<(String, [i32; 4], [u32; 2])>,
    /// The worker could not render (no GPU adapter, no ROM): nothing is drawn.
    pub failed: bool,
    /// Renders received (tests).
    pub renders: u64,
    /// Jobs sent to the worker (tests: the draw path's render count).
    pub requests: u64,
    /// The yaw of the shown render's request (tests: how far the shown character trails a turn).
    pub shown_yaw: Option<f32>,
}

/// The live pose's step (ms): 30 poses a second.
pub const POSE_STEP_MS: f32 = 1000.0 / 30.0;

/// A clock reading (ms) at the start of its pose step.
pub fn pose_step(ms: f32) -> f32 {
    (ms / POSE_STEP_MS).floor() * POSE_STEP_MS
}

const TEX_PREFIX: &str = "#shop-model/";

impl ModelView {
    pub fn new(rom: PathBuf) -> ModelView {
        Self::named(rom, TEX_PREFIX)
    }

    /// A second view with textures of its own (`prefix`: the names it puts its renders under).
    pub fn named(rom: PathBuf, prefix: &str) -> ModelView {
        ModelView {
            prefix: prefix.to_string(),
            rom,
            jobs: None,
            done: None,
            gen: 0,
            shown_gen: 0,
            asked: None,
            asked_time: 0.0,
            asked_live: false,
            pending: false,
            shown: None,
            failed: false,
            renders: 0,
            requests: 0,
            shown_yaw: None,
        }
    }

    fn start(&mut self) {
        if self.jobs.is_some() {
            return;
        }
        let (tx, rx) = channel::<Job>();
        let (dtx, drx) = channel::<Done>();
        let rom = self.rom.clone();
        let spawned = std::thread::Builder::new()
            .name("shop-model".into())
            .spawn(move || worker(rom, rx, dtx));
        if spawned.is_ok() {
            self.jobs = Some(tx);
            self.done = Some(drx);
        } else {
            self.failed = true;
        }
    }

    /// Asks for `req` at target size `size` (`None`: the screen shows no model) posed at clip time `time_ms`
    /// (`None`: the live pose, `live_ms` on the caller's clock quantised to [`POSE_STEP_MS`], which loops the clip).
    /// A request equal to the last one in the same pose costs nothing; a new pose is sent once the render before it
    /// arrived (one render in flight).
    // EVIDENCE: cshell+0x12cc60 (animation mode 3: the InvenShopAni clip looped) (CLI-783)
    // DC-DEFAULT: CLI-852 the live pose steps at 30 Hz: one render per step at most, none while nothing changed
    pub fn want(
        &mut self,
        req: Option<ViewReq>,
        size: (u32, u32),
        time_ms: Option<f32>,
        live_ms: f32,
    ) {
        let Some(req) = req else {
            self.asked = None;
            self.shown = None;
            self.shown_yaw = None;
            self.shown_gen = self.gen;
            self.pending = false;
            self.asked_live = false;
            return;
        };
        if self.failed {
            return;
        }
        // a fixed time replaces a live pose in flight; the live pose waits for the render in flight
        let live = time_ms.is_none();
        let time_ms = time_ms.unwrap_or_else(|| pose_step(live_ms));
        let same = self.asked.as_ref() == Some(&(req.clone(), size));
        if same && (time_ms == self.asked_time || (live && self.pending)) {
            return;
        }
        self.start();
        self.gen += 1;
        self.asked = Some((req.clone(), size));
        self.asked_time = time_ms;
        self.asked_live = live;
        self.requests += 1;
        let sent = self.jobs.as_ref().is_some_and(|j| {
            j.send(Job {
                gen: self.gen,
                req,
                size,
                time_ms,
            })
            .is_ok()
        });
        self.pending = sent;
        if !sent {
            self.failed = true;
        }
    }

    /// The live pose stepped since the last request and no render is in flight: a frame is due.
    pub fn due(&self, live_ms: f32) -> bool {
        self.asked_live && !self.pending && !self.failed && pose_step(live_ms) != self.asked_time
    }

    /// A render is on its way.
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// Takes a finished render into `assets` (true: the shown texture changed).
    pub fn poll(&mut self, assets: &mut HudAssets) -> bool {
        let mut changed = false;
        while let Some(d) = self.done.as_ref().and_then(|r| r.try_recv().ok()) {
            changed |= self.take(d, assets);
        }
        changed
    }

    /// Waits up to `limit` for the pending render and takes it.
    pub fn wait(&mut self, assets: &mut HudAssets, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        let mut changed = false;
        while self.pending {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let Some(d) = self.done.as_ref().and_then(|r| r.recv_timeout(left).ok()) else {
                break;
            };
            changed |= self.take(d, assets);
        }
        changed
    }

    // DC-DEFAULT: CLI-1295 a render older than the newest request is still shown when it is newer than the shown one
    fn take(&mut self, d: Done, assets: &mut HudAssets) -> bool {
        // CLI-1295: a drag asks for a new yaw every frame, faster than a render; taking only the newest request's
        // render dropped every one of them while the pointer moved (the character froze until the drag paused)
        if d.gen <= self.shown_gen || self.asked.is_none() {
            return false;
        }
        self.shown_gen = d.gen;
        self.pending = d.gen != self.gen;
        self.renders += 1;
        let rect = d.rect;
        self.shown_yaw = Some(d.yaw);
        let Some(tex) = d.tex else {
            self.shown = None;
            return true;
        };
        // two names in turn: each frame changes the display list (the painter rebuilds), and the overlay atlas
        // refills a name's region in place (same size) instead of packing a new one per frame
        let old = self.shown.take().map(|(n, _, _)| n);
        let turn = u8::from(old.as_deref() != Some(&format!("{}1", self.prefix)));
        let name = format!("{}{turn}", self.prefix);
        if let Some(old) = old {
            assets.forget(&old);
        }
        let size = [tex.width, tex.height];
        assets.put(&name, tex);
        self.shown = Some((name, rect, size));
        true
    }

    /// The display-list image of the shown model: the whole render (target pixels, [`rect_px`]) over the pane
    /// rectangle (virtual pixels). CLI-970: the source is the texture's own size; the pane's virtual size took only
    /// its top-left part at any window but 1024 x 768 (at 2560 x 1440 the 238 x 480 corner of a 595 x 900 render,
    /// left of the body: no figure in the user's window).
    pub fn item(&self) -> Option<DrawItem> {
        let (name, r, size) = self.shown.as_ref()?;
        Some(DrawItem::Image {
            tex: name.clone(),
            src: [size[0] as f32, size[1] as f32],
            dst: [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32],
            tint: [255; 4],
            turn: None,
        })
    }
}

/// The models' pixels with their coverage, from the same frame drawn over black (`on_black`) and over white
/// (`on_white`, RGBA): alpha = 1 - (white - black), colour = black / alpha (written into `on_black`).
pub fn alpha_from_pair(on_black: &mut [u8], on_white: &[u8]) {
    for (b, w) in on_black
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(on_white.as_chunks::<4>().0)
    {
        let spread = (0..3)
            .map(|i| w[i].saturating_sub(b[i]) as u32)
            .max()
            .unwrap_or(255);
        let a = 255 - spread.min(255);
        if a == 0 {
            *b = [0, 0, 0, 0];
            continue;
        }
        for c in b.iter_mut().take(3) {
            *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
        }
        b[3] = a as u8;
    }
}

/// The crop `rect` (x, y, w, h in pixels, clamped to the frame) of a `w` x `h` RGBA frame.
pub fn crop(rgba: &[u8], w: u32, h: u32, rect: [u32; 4]) -> Tex {
    let x0 = rect[0].min(w);
    let y0 = rect[1].min(h);
    let cw = rect[2].min(w - x0);
    let ch = rect[3].min(h - y0);
    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for y in y0..y0 + ch {
        let s = ((y * w + x0) * 4) as usize;
        out.extend_from_slice(&rgba[s..s + (cw * 4) as usize]);
    }
    Tex {
        width: cw,
        height: ch,
        rgba: out,
    }
}

/// The pane rectangle in target pixels.
pub fn rect_px(rect: [i32; 4], size: (u32, u32)) -> [u32; 4] {
    let sx = size.0 as f32 / 1024.0;
    let sy = size.1 as f32 / 768.0;
    [
        (rect[0].max(0) as f32 * sx).round() as u32,
        (rect[1].max(0) as f32 * sy).round() as u32,
        (rect[2].max(1) as f32 * sx).round().max(1.0) as u32,
        (rect[3].max(1) as f32 * sy).round().max(1.0) as u32,
    ]
}

/// The part of the full view (normalised device units, y up: x0, y0, x1, y1) that pixel rectangle `rect` (x, y, w,
/// h) of a `size` target shows.
pub fn pane_window(rect: [u32; 4], size: (u32, u32)) -> [f32; 4] {
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let x0 = rect[0] as f32 / w * 2.0 - 1.0;
    let x1 = (rect[0] + rect[2]) as f32 / w * 2.0 - 1.0;
    let top = 1.0 - rect[1] as f32 / h * 2.0;
    let bottom = 1.0 - (rect[1] + rect[3]) as f32 / h * 2.0;
    [x0, bottom, x1, top]
}

/// Body model files parsed by the character view's workers (tests: a pose step parses none).
pub static BODY_FILE_PARSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What the view needs from a body's model file: the clip it plays and the weapon's grip socket.
#[derive(Debug, Clone, Copy)]
struct BodyFacts {
    clip: usize,
    grip: u32,
}

/// The body clip the shop character plays (the body row's `InvenShopAni` clip of the body file, else clip 0) and
/// the body's grip socket, from one parse of the body file.
fn body_facts(
    vfs: &dc_rez::RezVfs,
    table: Option<&dc_render::CharacterTable>,
    body: &str,
) -> BodyFacts {
    BODY_FILE_PARSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Ok(f) = dc_render::ModelFiles::load(vfs, body) else {
        return BodyFacts {
            clip: 0,
            grip: super::figure::GRIP_FALLBACK,
        };
    };
    let name = table
        .and_then(|t| t.row_of_model(body))
        .and_then(|(_, r)| r.shop_clip.clone());
    let clip = name
        .and_then(|n| {
            f.anim_set()
                .find_clip(|c| c.eq_ignore_ascii_case(n.as_bytes()))
        })
        .unwrap_or(0);
    BodyFacts {
        clip,
        grip: super::figure::grip_socket_index(&f.body),
    }
}

struct Gpu {
    ctx: dc_render::GpuContext,
    renderer: dc_render::WorldRenderer,
    scene: dc_render::GpuScene,
    models: crate::characters::ModelScene,
    table: Option<dc_render::CharacterTable>,
    vfs: dc_rez::RezVfs,
    target: Option<crate::offscreen::Offscreen>,
    /// CLI-1170: the body file's facts per body model (the file was parsed twice per pose: about 160 ms of a
    /// 170 ms render at 2560 x 1440, the figure ran at 5-6 poses a second)
    bodies: std::collections::HashMap<String, BodyFacts>,
}

fn open_gpu(rom: &std::path::Path) -> Result<Gpu, String> {
    use dc_render::{AdapterChoice, GpuContext, RenderSettings, WorldRenderer, OFFSCREEN_FORMAT};
    let ctx = GpuContext::headless(AdapterChoice::Auto).map_err(|e| e.to_string())?;
    let renderer = WorldRenderer::new(&ctx, OFFSCREEN_FORMAT, RenderSettings::default());
    let scene = renderer.upload(&ctx, &dc_render::SceneMesh::default(), &mut |_| None);
    let models = crate::characters::ModelScene::open(rom)?;
    let vfs = dc_rez::RezVfs::new(rom);
    let table = dc_render::CharacterTable::from_vfs(&vfs).ok();
    Ok(Gpu {
        ctx,
        renderer,
        scene,
        models,
        table,
        vfs,
        target: None,
        bodies: std::collections::HashMap::new(),
    })
}

fn worker(rom: PathBuf, jobs: Receiver<Job>, done: Sender<Done>) {
    // CLI-1095 (playtest report: "my character still doesn't appear in the shop screen"): the view's own device takes
    // no part in the frame profiler's GPU timestamps (with --profile, as the launcher runs the client, its device
    // replaced the window's timer and its passes wrote into the other device's query set: no figure)
    dc_render::perf::untimed_thread();
    let mut gpu: Option<Gpu> = None;
    let mut broken = false;
    while let Ok(mut job) = jobs.recv() {
        // only the newest request matters
        while let Ok(j) = jobs.try_recv() {
            job = j;
        }
        if gpu.is_none() && !broken {
            match open_gpu(&rom) {
                Ok(g) => gpu = Some(g),
                Err(e) => {
                    eprintln!("shop-model: no character view ({e})");
                    broken = true;
                }
            }
        }
        let tex = gpu.as_mut().and_then(|g| render(g, &job));
        let done_job = Done {
            gen: job.gen,
            rect: job.req.rect,
            yaw: job.req.place.yaw,
            tex,
        };
        if done.send(done_job).is_err() {
            return;
        }
    }
}

fn render(g: &mut Gpu, job: &Job) -> Option<Tex> {
    if let Some(s) = &job.req.showcase {
        return render_showcase(g, job, s);
    }
    g.models.clear_showcase();
    let (w, h) = (job.size.0.max(1), job.size.1.max(1));
    let look = match &job.req.look {
        Some(l) => l.clone(),
        None => CharLook {
            models: g.models.default_look(),
            weapon: None,
            styles: Vec::new(),
        },
    };
    let body = look.models.first()?.model.clone();
    // EVIDENCE: cshell+0x12cc60 (animation mode 3: the clip loops in real time)
    // DC-DEFAULT: CLI-1170 the body file is read once per body model, not per pose
    let facts = match g.bodies.get(&body) {
        Some(f) => *f,
        None => {
            let f = body_facts(&g.vfs, g.table.as_ref(), &body);
            g.bodies.insert(body.clone(), f);
            f
        }
    };
    let clip = facts.clip;
    let mut look = look;
    if let Some((_, socket)) = look.weapon.as_mut() {
        if *socket == super::figure::GRIP_UNRESOLVED {
            *socket = facts.grip;
        }
    }
    let p = job.req.place;
    let remote = RemoteBody {
        obj: 1,
        pid: 0,
        name: String::new(),
        team: 0,
        pos: p.origin,
        half: [0.0; 3],
        yaw: Some(p.yaw),
        look: Some(RemoteLook {
            styles: look.styles.clone(),
            models: look.models.clone(),
            weapon: look.weapon.clone(),
            trackers: vec![RemoteTracker {
                clip,
                time_ms: job.time_ms,
                blend_set: None,
            }],
            visible: true,
            aim: None,
        }),
    };
    g.models
        .update_remotes(&g.ctx, &g.renderer, std::slice::from_ref(&remote), None);
    if g.models.stats.characters == 0 {
        return None;
    }
    g.models
        .place_body_pieces(&g.ctx, &g.renderer, remote.obj, &job.req.pieces);
    if !job.req.lights.is_empty() {
        let shift = Vec3::from(p.origin) - Vec3::from(ORIGIN_2009);
        let set = dc_render::LightSet {
            ambient: Vec3::splat(SHOP_AMBIENT),
            lights: job
                .req
                .lights
                .iter()
                .map(|l| {
                    dc_render::ModelLight::dynamic(
                        Vec3::from(l.pos) + shift,
                        l.radius,
                        Vec3::from(l.colour),
                    )
                })
                .collect(),
        };
        g.models.set_object_lights(remote.obj, &set);
    }
    // a target of the pane's size showing the pane's part of the full view
    let rect = rect_px(job.req.rect, (w, h));
    let (pw, ph) = (rect[2].min(w).max(1), rect[3].min(h).max(1));
    if g.target.as_ref().is_none_or(|t| t.size != (pw, ph)) {
        let mut t = crate::offscreen::Offscreen::new(&g.ctx, dc_render::OFFSCREEN_FORMAT, pw, ph);
        // CLI-1095: the profiler's in-flight path (the launcher runs the client with --profile) returned an empty
        // frame for every render without DC_PROF_READBACK: the pane showed no character
        t.always_read = true;
        g.target = Some(t);
    }
    let target = g.target.as_mut()?;
    let [fx, fy] = FOV_INTERFACE_DEG;
    let mut camera =
        dc_render::Camera::looking(Vec3::ZERO, 0.0, 0.0, fx.to_radians(), fy.to_radians());
    camera.window = pane_window(rect, (w, h));
    let models = g.models.instances();
    // CLI-1629: the shop's x-ray preview: the player's style drawn on its own character. Chams and skeleton by the
    // match's x-ray pass (dc-render `encode_xray`); the box as the match's HUD box (fill = primary at a third of its
    // alpha, a border of the secondary colour) over the screen bounds of the posed model, kept inside the pane
    // DC-DEFAULT: CLI-1629 (proposed)
    let style = job.req.xray;
    let boxed = style
        .filter(|s| s.mode == dc_render::XrayMode::Box)
        .and_then(|s| {
            // the posed model: body, face, gear and weapon
            let bounds = g
                .models
                .character_instances(remote.obj)
                .iter()
                .filter_map(|i| i.bounds_in_world())
                .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))?;
            Some((s, preview_box(&camera, (pw, ph), bounds)?))
        });
    let xray: Vec<dc_render::XrayModel<'_>> = style
        .filter(|s| s.mode != dc_render::XrayMode::Box)
        .map(|s| dc_render::XrayModel {
            instances: g.models.character_instances(remote.obj),
            mode: s.mode,
            primary: s.primary,
            secondary: s.secondary,
        })
        .filter(|x| !x.instances.is_empty())
        .into_iter()
        .collect();
    let mut shot = |clear: f64| {
        target.clear_to(Some([clear, clear, clear, 1.0]));
        target
            .render_models_xray(&g.ctx, &g.renderer, &g.scene, &camera, &models, &xray)
            .ok()
            .map(|f| crop(&f.rgba, f.width, f.height, [0, 0, f.width, f.height]))
    };
    let mut tex = shot(0.0)?;
    let white = shot(1.0)?;
    alpha_from_pair(&mut tex.rgba, &white.rgba);
    if let Some((s, r)) = boxed {
        let border = (dc_render::BOX_BORDER_PX * h as f32 / 768.0).round().max(1.0);
        paint_box(&mut tex, r, border, s.primary, s.secondary);
    }
    Some(tex)
}

/// CRATE-8: the showcase model alone, posed and lit as `s` asks, over the request's rectangle (coverage as the
/// character's: drawn over black and over white).
fn render_showcase(g: &mut Gpu, job: &Job, s: &Showcase) -> Option<Tex> {
    let (w, h) = (job.size.0.max(1), job.size.1.max(1));
    let (lo, hi) = g.models.showcase_bounds(&g.ctx, &g.renderer, &s.model)?;
    let size = Vec3::from(hi) - Vec3::from(lo);
    let length = size.max_element().max(1.0);
    // the depth at which `length` spans `fill` of the rectangle's width (the rectangle's share of the view's width)
    let [fx, fy] = FOV_INTERFACE_DEG;
    let share = job.req.rect[2] as f32 / dc_ui_data::VIRTUAL_WIDTH as f32;
    let half = (fx.to_radians() / 2.0).tan() * share;
    let depth = length / (2.0 * half * s.fill.max(0.05));
    let centre = Vec3::new(0.0, 0.0, depth);
    let rotation = glam::Quat::from_rotation_y(s.yaw)
        * glam::Quat::from_rotation_x(s.pitch)
        * glam::Quat::from_rotation_z(s.roll);
    let reach = depth * 2.0;
    let lights = dc_render::LightSet {
        ambient: Vec3::splat(0.58),
        lights: vec![
            dc_render::ModelLight::dynamic(
                centre + Vec3::new(-length, length * 0.8, -depth * 0.6),
                reach,
                Vec3::splat(1.25),
            ),
            dc_render::ModelLight::dynamic(
                centre + Vec3::new(length * 0.6, length * 0.5, depth * 0.5),
                reach,
                Vec3::from(s.rim),
            ),
        ],
    };
    g.models.pose_showcase(rotation, centre, lights);
    let rect = rect_px(job.req.rect, (w, h));
    let (pw, ph) = (rect[2].min(w).max(1), rect[3].min(h).max(1));
    if g.target.as_ref().is_none_or(|t| t.size != (pw, ph)) {
        let mut t = crate::offscreen::Offscreen::new(&g.ctx, dc_render::OFFSCREEN_FORMAT, pw, ph);
        t.always_read = true;
        g.target = Some(t);
    }
    let target = g.target.as_mut()?;
    let mut camera =
        dc_render::Camera::looking(Vec3::ZERO, 0.0, 0.0, fx.to_radians(), fy.to_radians());
    camera.window = pane_window(rect, (w, h));
    let models = g.models.instances();
    let mut shot = |clear: f64| {
        target.clear_to(Some([clear, clear, clear, 1.0]));
        target
            .render_models_xray(&g.ctx, &g.renderer, &g.scene, &camera, &models, &[])
            .ok()
            .map(|f| crop(&f.rgba, f.width, f.height, [0, 0, f.width, f.height]))
    };
    let mut tex = shot(0.0)?;
    let white = shot(1.0)?;
    alpha_from_pair(&mut tex.rgba, &white.rgba);
    Some(tex)
}

/// CLI-1629: the screen rectangle (pixels of a `size` target: x, y, w, h) of the world box `bounds` seen by
/// `camera`, kept inside the target (the box of a player near the camera reaches past the pane).
pub fn preview_box(
    camera: &dc_render::Camera,
    size: (u32, u32),
    bounds: (Vec3, Vec3),
) -> Option<[f32; 4]> {
    let (lo, hi) = bounds;
    let (mut a, mut b) = ([f32::MAX; 2], [f32::MIN; 2]);
    for k in 0..8 {
        let c = Vec3::new(
            if k & 1 == 0 { lo.x } else { hi.x },
            if k & 2 == 0 { lo.y } else { hi.y },
            if k & 4 == 0 { lo.z } else { hi.z },
        );
        let p = dc_render::project_px(camera, size, c)?;
        for i in 0..2 {
            a[i] = a[i].min(p[i]);
            b[i] = b[i].max(p[i]);
        }
    }
    let (x0, y0) = (a[0].max(0.0), a[1].max(0.0));
    let (x1, y1) = (b[0].min(size.0 as f32), b[1].min(size.1 as f32));
    (x1 > x0 && y1 > y0 && [x0, y0, x1, y1].iter().all(|v| v.is_finite()))
        .then_some([x0, y0, x1 - x0, y1 - y0])
}

/// CLI-1629: blends the box of the match's x-ray (fill = `primary` at a third of its alpha, a `border` pixel frame of
/// `secondary`) over the straight-alpha picture `tex`.
pub fn paint_box(tex: &mut Tex, r: [f32; 4], border: f32, primary: [u8; 4], secondary: [u8; 4]) {
    let mut fill = primary;
    fill[3] = (fill[3] as u32 / dc_render::BOX_FILL_ALPHA_DIV) as u8;
    let [x, y, w, h] = r.map(|v| v.round() as i64);
    let t = border as i64;
    blend_rect(tex, [x, y, w, h], fill);
    for b in [
        [x, y, w, t],
        [x, y + h - t, w, t],
        [x, y + t, t, h - 2 * t],
        [x + w - t, y + t, t, h - 2 * t],
    ] {
        blend_rect(tex, b, secondary);
    }
}

fn blend_rect(tex: &mut Tex, r: [i64; 4], c: [u8; 4]) {
    let (tw, th) = (tex.width as i64, tex.height as i64);
    let (x0, y0) = (r[0].clamp(0, tw), r[1].clamp(0, th));
    let (x1, y1) = ((r[0] + r[2]).clamp(0, tw), (r[1] + r[3]).clamp(0, th));
    let sa = c[3] as f32 / 255.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * tw + x) * 4) as usize;
            let d = &mut tex.rgba[i..i + 4];
            let da = d[3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            if oa <= 0.0 {
                continue;
            }
            for k in 0..3 {
                let v = (c[k] as f32 * sa + d[k] as f32 * da * (1.0 - sa)) / oa;
                d[k] = v.round().clamp(0.0, 255.0) as u8;
            }
            d[3] = (oa * 255.0).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_comes_from_the_black_and_white_frames() {
        // cleared, opaque, half covered
        let mut b = vec![0, 0, 0, 255, 10, 20, 30, 255, 50, 50, 50, 255];
        let w = vec![255, 255, 255, 255, 10, 20, 30, 255, 177, 177, 177, 255];
        alpha_from_pair(&mut b, &w);
        assert_eq!(&b[..4], &[0, 0, 0, 0]);
        assert_eq!(&b[4..8], &[10, 20, 30, 255]);
        assert_eq!(b[11], 128);
        assert_eq!(b[8], 100);
    }

    #[test]
    fn crop_takes_the_rectangle() {
        let w = 4;
        let h = 3;
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| i as u8).collect();
        let t = crop(&rgba, w, h, [1, 1, 2, 2]);
        assert_eq!((t.width, t.height), (2, 2));
        assert_eq!(&t.rgba[..4], &rgba[20..24]);
    }

    #[test]
    fn the_pane_window_is_the_panes_part_of_the_view() {
        assert_eq!(
            pane_window([0, 0, 1024, 768], (1024, 768)),
            dc_render::FULL_WINDOW
        );
        // the left half's top quarter
        assert_eq!(
            pane_window([0, 0, 512, 192], (1024, 768)),
            [-1.0, 0.5, 0.0, 1.0]
        );
    }

    #[test]
    fn the_pane_rect_scales_with_the_target() {
        assert_eq!(
            rect_px([14, 104, 238, 480], (1024, 768)),
            [14, 104, 238, 480]
        );
        assert_eq!(
            rect_px([14, 104, 238, 480], (2048, 1536)),
            [28, 208, 476, 960]
        );
    }
}

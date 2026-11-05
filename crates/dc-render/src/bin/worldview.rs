//! `worldview`: renders one world of the user's ROM to a PNG (headless) or, with the `window` feature and
//! `--window`, briefly into a borderless window on the primary monitor.
//!
//! ```text
//! worldview --rom <CA_ROM> --map <name> [--eye x,y,z] [--look yaw,pitch] [--spawn N] --out shot.png
//! worldview --map <name> --model <rez path> [--anim N | --anim M,U,L | --pose stand|walk] [--anim-phase F]
//! worldview --map <name> --view-weapon ROW [--view-offset x,y,z] [--view-clip N] [--pv-fov DEG]
//! worldview --list            maps of the ROM
//! worldview --probe           adapter attempts (default, DX12, Vulkan, software)
//! ```
//! Images derived from the ROM are local files for the user; never commit them.

use dc_render::*;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str =
    "usage: worldview [--rom DIR] --map NAME [--eye x,y,z] [--look yaw,pitch] [--spawn N]
                 [--lift DY] [--fov DEG] [--size WxH] [--far Z] [--out FILE.png]
                 [--adapter auto|dx12|vulkan|fallback] [--lightmap-order bgr|rgb]
                 [--no-lightmaps] [--flat] [--lightmaps-only] [--no-world-models] [--gen-mips]
                 [--light-groups K | --no-light-groups] [--sky | --no-sky]
                 [--translucent | --no-translucent] [--no-sort-translucent] [--no-fog] [--gamma G]
                 [--model PATH | --character ROW] [--anim N | --anim M,U,L | --pose stand|walk] [--anim-phase F]
                  [--skin TEX ...] [--style RS ...] [--no-styles] [--model-spawn N] [--model-dist D]
                  [--no-drop] [--model-at x,y,z,yaw] [--sun dx,dy,dz,r,g,b | --no-sun] [--weapon ROW|none]
                  [--lod-dist D]]
                 [--view-weapon ROW [--view-offset x,y,z] [--view-clip N] [--view-phase F]
                  [--pv-fov DEG] [--pv-near N] [--pv-far F]]
                 [--window [--window-secs S]]
       worldview [--rom DIR] --list
       worldview --probe
  --eye and --look are world units and degrees (yaw about +Y, 0 = +Z, positive pitch looks down);
  without --eye the camera sits on player spawn N (default 0) with its stored orientation, --look
  overriding the orientation. --rom defaults to CA_ROM.
  --model places a model file of the ROM (resource path) at spawn N (default 0), feet on the floor, and
  puts the camera --model-dist (150) units in front of it unless --eye / --look are given; --anim plays one
  clip on the main tracker or three (main, upper with weight set 0, lower with weight set 1), --pose uses
  the player clips the game server streams, --anim-phase picks the point in each clip (0..1). Textures:
  --skin in material-slot order, else the ROM model table's list when the model is a character body.
  --character ROW takes the model file and textures of body row ROW of the ROM model table, with the face
  model of face row --face (default 0; --face none leaves it out) posed with the body.
  Models draw with their render styles: --style in style-slot order, else the ROM model table's list for a
  body or face row (empty slots: the engine's default style); --no-styles draws the plain state instead.
  --sun gives the global directional light the game server would send (direction it travels, colour
  0..255); by default it is the world's global light object (none on worlds without one); --no-sun leaves
  it out. --character also attaches a weapon's third-person model at the body's weapon socket: weapon row
  --weapon ROW of the ROM weapon table (default: the lowest row with a third-person model; none: no weapon).
  --lod-dist fixes the distance the model LODs are chosen for (default: the camera distance).
  The sky pass and the translucent world models are on by default; the world's linear fog comes from its
  world settings (--no-fog leaves it out). The picture goes through the 2009 gamma ramp with --gamma G, else
  the gamma of the user's profile, else the code default 1.22 (--gamma 1 leaves the picture unchanged).
  --model-at places the model at a world position (its origin) facing yaw degrees, instead of a spawn point.
  --view-weapon ROW draws the first-person model of weapon row ROW of the ROM weapon table (first hand
  identifier of the table) at the eye with the player-view camera (fov_y --pv-fov 50, near 0.1, far 7,
  depth range [0, 0.1]): origin at --view-offset (eye space: right, up, forward; default 0,-4,0), the idle
  clip (or --view-clip N) at --view-phase (default 0), lit at the eye.";

/// Where the viewer puts a first-person model's origin in eye space: 4 units below the eye.
// DC-DEFAULT: REN-15 (proposed), see DC_DEFAULTS.md (the game's own per-weapon offset is set by the game module, not
// traced; OBSERVED 2026-10-25: the ROM first-person rigs stand on their origin with the shoulders near y = 3.4 and the
// arms' bind pose reaching y = 4.0..5.0, so the eye goes 4 units above the origin)
const VIEW_OFFSET_DEFAULT: [f32; 3] = [0.0, -4.0, 0.0];

struct Args {
    rom: Option<PathBuf>,
    map: Option<String>,
    eye: Option<[f32; 3]>,
    look: Option<(f32, f32)>,
    spawn: usize,
    lift: f32,
    fov: f32,
    size: (u32, u32),
    far: Option<f32>,
    out: Option<PathBuf>,
    adapter: AdapterChoice,
    order: TexelOrder,
    lightmaps: bool,
    flat: bool,
    lightmaps_only: bool,
    world_models: bool,
    gen_mips: bool,
    light_groups: Option<f32>,
    sky: bool,
    translucent: bool,
    sort_translucent: bool,
    fog: bool,
    gamma: Option<f32>,
    model_at: Option<([f32; 3], f32)>,
    model: Option<String>,
    character: Option<u32>,
    face: Option<u32>,
    anim: Option<PoseSpec>,
    anim_phase: f32,
    skins: Vec<String>,
    model_spawn: usize,
    model_distance: f32,
    drop: bool,
    sun: Option<([f32; 3], [f32; 3])>,
    no_sun: bool,
    styles: Vec<String>,
    plain: bool,
    weapon: Option<Option<u32>>,
    lod_dist: Option<f32>,
    view_weapon: Option<u32>,
    view_offset: [f32; 3],
    view_clip: Option<usize>,
    view_phase: f32,
    pv: PlayerViewSettings,
    list: bool,
    probe: bool,
    window: bool,
    window_secs: f32,
}

fn floats<const N: usize>(s: &str, what: &str) -> Result<[f32; N], String> {
    let v: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{what}: expected {N} comma-separated numbers, got {s:?}"))?;
    v.try_into()
        .map_err(|_| format!("{what}: expected {N} comma-separated numbers, got {s:?}"))
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        rom: None,
        map: None,
        eye: None,
        look: None,
        spawn: 0,
        lift: 0.0,
        fov: 90.0,
        size: (1024, 768),
        far: None,
        out: None,
        adapter: AdapterChoice::Auto,
        order: TexelOrder::default(),
        lightmaps: true,
        flat: false,
        lightmaps_only: false,
        world_models: true,
        gen_mips: false,
        light_groups: Some(1.0),
        sky: true,
        translucent: true,
        sort_translucent: true,
        fog: true,
        gamma: None,
        model_at: None,
        model: None,
        character: None,
        face: Some(0),
        anim: None,
        anim_phase: 0.0,
        skins: Vec::new(),
        model_spawn: 0,
        model_distance: 150.0,
        drop: true,
        sun: None,
        no_sun: false,
        styles: Vec::new(),
        plain: false,
        weapon: None,
        lod_dist: None,
        view_weapon: None,
        view_offset: VIEW_OFFSET_DEFAULT,
        view_clip: None,
        view_phase: 0.0,
        pv: PLAYER_VIEW_DEFAULT,
        list: false,
        probe: false,
        window: false,
        window_secs: 3.0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut val = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match key.as_str() {
            "--rom" => a.rom = Some(PathBuf::from(val("--rom")?)),
            "--map" => a.map = Some(val("--map")?),
            "--eye" => a.eye = Some(floats::<3>(&val("--eye")?, "--eye")?),
            "--look" => {
                let [y, p] = floats::<2>(&val("--look")?, "--look")?;
                a.look = Some((y, p));
            }
            "--spawn" => a.spawn = val("--spawn")?.parse().map_err(|_| "--spawn: integer")?,
            "--lift" => a.lift = val("--lift")?.parse().map_err(|_| "--lift: number")?,
            "--fov" => a.fov = val("--fov")?.parse().map_err(|_| "--fov: number")?,
            "--far" => a.far = Some(val("--far")?.parse().map_err(|_| "--far: number")?),
            "--size" => {
                let v = val("--size")?;
                let (w, h) = v.split_once(['x', 'X']).ok_or("--size: WxH")?;
                let w: u32 = w.parse().map_err(|_| "--size: WxH")?;
                let h: u32 = h.parse().map_err(|_| "--size: WxH")?;
                if !(16..=8192).contains(&w) || !(16..=8192).contains(&h) {
                    return Err("--size: each side 16..8192".into());
                }
                a.size = (w, h);
            }
            "--out" => a.out = Some(PathBuf::from(val("--out")?)),
            "--adapter" => {
                a.adapter = match val("--adapter")?.as_str() {
                    "auto" => AdapterChoice::Auto,
                    "dx12" => AdapterChoice::Backends(wgpu::Backends::DX12),
                    "vulkan" => AdapterChoice::Backends(wgpu::Backends::VULKAN),
                    "metal" => AdapterChoice::Backends(wgpu::Backends::METAL),
                    "fallback" | "software" => AdapterChoice::Fallback,
                    other => return Err(format!("--adapter: unknown {other:?}")),
                }
            }
            "--lightmap-order" => {
                a.order = match val("--lightmap-order")?.as_str() {
                    "bgr" => TexelOrder::Bgr,
                    "rgb" => TexelOrder::Rgb,
                    other => return Err(format!("--lightmap-order: unknown {other:?}")),
                }
            }
            "--no-lightmaps" => a.lightmaps = false,
            "--flat" => a.flat = true,
            "--lightmaps-only" => a.lightmaps_only = true,
            "--no-world-models" => a.world_models = false,
            "--gen-mips" => a.gen_mips = true,
            "--no-light-groups" => a.light_groups = None,
            "--light-groups" => {
                a.light_groups = Some(
                    val("--light-groups")?
                        .parse()
                        .map_err(|_| "--light-groups: number")?,
                )
            }
            "--sky" => a.sky = true,
            "--no-sky" => a.sky = false,
            "--translucent" => a.translucent = true,
            "--no-translucent" => a.translucent = false,
            "--no-sort-translucent" => a.sort_translucent = false,
            "--no-fog" => a.fog = false,
            "--gamma" => a.gamma = Some(val("--gamma")?.parse().map_err(|_| "--gamma: number")?),
            "--model-at" => {
                let [x, y, z, yaw] = floats::<4>(&val("--model-at")?, "--model-at")?;
                a.model_at = Some(([x, y, z], yaw));
            }
            "--model" => a.model = Some(val("--model")?),
            "--face" => {
                let v = val("--face")?;
                a.face = if v == "none" {
                    None
                } else {
                    Some(v.parse().map_err(|_| "--face: row number or none")?)
                }
            }
            "--character" => {
                a.character = Some(
                    val("--character")?
                        .parse()
                        .map_err(|_| "--character: row number")?,
                )
            }
            "--anim" => {
                let v = val("--anim")?;
                let clips: Vec<usize> = v
                    .split(',')
                    .map(|p| p.trim().parse::<usize>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| "--anim: N or M,U,L (clip indices)")?;
                if clips.len() != 1 && clips.len() != 3 {
                    return Err("--anim: one index or three (main, upper, lower)".into());
                }
                a.anim = Some(PoseSpec::Clips(clips));
            }
            "--pose" => {
                a.anim = Some(match val("--pose")?.as_str() {
                    "stand" => PoseSpec::Player { walk: false },
                    "walk" => PoseSpec::Player { walk: true },
                    other => return Err(format!("--pose: stand or walk, got {other:?}")),
                })
            }
            "--anim-phase" => {
                a.anim_phase = val("--anim-phase")?
                    .parse()
                    .map_err(|_| "--anim-phase: number 0..1")?
            }
            "--skin" => a.skins.push(val("--skin")?),
            "--model-spawn" => {
                a.model_spawn = val("--model-spawn")?
                    .parse()
                    .map_err(|_| "--model-spawn: integer")?
            }
            "--model-dist" => {
                a.model_distance = val("--model-dist")?
                    .parse()
                    .map_err(|_| "--model-dist: number")?
            }
            "--no-drop" => a.drop = false,
            "--sun" => {
                let [dx, dy, dz, r, g, b] = floats::<6>(&val("--sun")?, "--sun")?;
                a.sun = Some(([dx, dy, dz], [r, g, b]));
            }
            "--no-sun" => a.no_sun = true,
            "--style" => a.styles.push(val("--style")?),
            "--no-styles" => a.plain = true,
            "--weapon" => {
                let v = val("--weapon")?;
                a.weapon = Some(if v == "none" {
                    None
                } else {
                    Some(v.parse().map_err(|_| "--weapon: row number or none")?)
                })
            }
            "--lod-dist" => {
                a.lod_dist = Some(
                    val("--lod-dist")?
                        .parse()
                        .map_err(|_| "--lod-dist: number")?,
                )
            }
            "--view-weapon" => {
                a.view_weapon = Some(
                    val("--view-weapon")?
                        .parse()
                        .map_err(|_| "--view-weapon: row number")?,
                )
            }
            "--view-offset" => {
                a.view_offset = floats::<3>(&val("--view-offset")?, "--view-offset")?
            }
            "--view-clip" => {
                a.view_clip = Some(
                    val("--view-clip")?
                        .parse()
                        .map_err(|_| "--view-clip: clip index")?,
                )
            }
            "--view-phase" => {
                a.view_phase = val("--view-phase")?
                    .parse()
                    .map_err(|_| "--view-phase: number 0..1")?
            }
            "--pv-fov" => {
                a.pv.fov_y_deg = val("--pv-fov")?.parse().map_err(|_| "--pv-fov: number")?
            }
            "--pv-near" => {
                a.pv.near = val("--pv-near")?.parse().map_err(|_| "--pv-near: number")?
            }
            "--pv-far" => a.pv.far = val("--pv-far")?.parse().map_err(|_| "--pv-far: number")?,
            "--list" => a.list = true,
            "--probe" => a.probe = true,
            "--window" => a.window = true,
            "--window-secs" => {
                a.window_secs = val("--window-secs")?
                    .parse()
                    .map_err(|_| "--window-secs: number")?
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let mut a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("worldview: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    if a.probe {
        for p in probe_adapters() {
            match p.result {
                Ok(s) => println!("{:<18} {s}", p.attempt),
                Err(e) => println!("{:<18} none ({e})", p.attempt),
            }
        }
        return ExitCode::SUCCESS;
    }
    let rom = match a.rom.clone().map(Ok).unwrap_or_else(dc_rom::resolve_rom) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("worldview: --rom not given and CA_ROM unusable: {e}");
            return ExitCode::from(2);
        }
    };
    if a.list {
        match WorldSource::open(&rom) {
            Ok(w) => {
                for m in w.map_names() {
                    println!("{m}");
                }
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("worldview: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(map) = a.map.clone() else {
        eprintln!("worldview: --map is required\n{USAGE}");
        return ExitCode::from(2);
    };
    let mut settings = RenderSettings {
        lightmaps: a.lightmaps,
        draw_flat: a.flat,
        lightmaps_only: a.lightmaps_only,
        ..RenderSettings::default()
    };
    if let Some(f) = a.far {
        settings.far_z = f;
    }
    let mut req = ViewRequest::new(&map);
    // --far replaces the world's far plane (CLI-1851)
    req.world_far = a.far.is_none();
    req.camera = match a.eye {
        Some(eye) => {
            let (yaw, pitch) = a.look.unwrap_or((0.0, 0.0));
            CameraSpec::Explicit {
                eye,
                yaw_deg: yaw,
                pitch_deg: pitch,
            }
        }
        None => CameraSpec::Spawn(a.spawn),
    };
    if a.eye.is_none() {
        req.look_override = a.look;
    }
    req.eye_lift = a.lift;
    req.fov_x_deg = a.fov;
    req.width = a.size.0;
    req.height = a.size.1;
    req.scene = SceneOptions {
        settings,
        world_models: a.world_models,
        lightmap_order: a.order,
        light_groups: a.light_groups,
        sky: a.sky,
        translucent: a.translucent,
        fog: a.fog,
    };
    req.generate_mips = a.gen_mips;
    req.frame = FrameOptions {
        sky: a.sky.then(SkySettings::default),
        translucent: a.translucent,
        sort_translucent: a.sort_translucent,
        clear: None,
    };
    let mut attach: Option<(String, Vec<String>)> = None;
    let mut weapon: Option<WeaponRequest> = None;
    if let Some(row) = a.character {
        let vfs = dc_rez::RezVfs::new(&rom);
        // the held weapon: the socket is the server's (UDP_SESSION_2009_B.md §3.5)
        // DC-DEFAULT: REN-10 (proposed), see DC_DEFAULTS.md (viewer: lowest weapon row with a third-person model)
        let want = a.weapon.unwrap_or(Some(u32::MAX));
        if let Some(w) = want {
            let hands = match HandModels::from_vfs(&vfs) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("worldview: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let pick = if w == u32::MAX {
                hands.rows.iter().next()
            } else {
                hands.rows.get_key_value(&w)
            };
            match pick {
                Some((_, h)) => {
                    weapon = Some(WeaponRequest {
                        path: h.model.clone(),
                        textures: h.skins.iter().take(1).cloned().collect(),
                        styles: h.styles.clone(),
                        socket: None,
                    })
                }
                None => {
                    eprintln!("worldview: no weapon row {w} with a third-person model");
                    return ExitCode::from(2);
                }
            }
        }
        let table = match CharacterTable::from_vfs(&vfs) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("worldview: {e}");
                return ExitCode::FAILURE;
            }
        };
        let Some(body) = table.bodies.get(&row) else {
            eprintln!("worldview: no body row {row} in the ROM model table");
            return ExitCode::from(2);
        };
        a.model = Some(body.model.clone());
        if a.skins.is_empty() {
            a.skins = table.body_textures(row, 0, 0).unwrap_or_default();
        }
        attach = a
            .face
            .and_then(|f| table.faces.get(&f))
            .map(|(m, s)| (m.clone(), s.iter().take(1).cloned().collect()));
    }
    if let Some(path) = &a.model {
        let mut m = ModelRequest::new(path);
        m.anim = a.anim.clone().map(|pose| AnimSpec {
            pose,
            phase: a.anim_phase,
        });
        if !a.skins.is_empty() {
            m.textures = Some(a.skins.clone());
        }
        m.spawn = a.model_spawn;
        m.place = a.model_at.map(|(p, yaw)| (p, yaw.to_radians()));
        m.camera_distance = a.model_distance;
        m.drop_to_floor = a.drop;
        m.sun = a.sun;
        m.world_sun = !a.no_sun;
        if !a.styles.is_empty() {
            m.styles = Some(a.styles.clone());
        }
        m.plain = a.plain;
        m.weapon = weapon;
        m.lod_distance = a.lod_dist;
        m.attachments.extend(attach);
        req.model = Some(m);
    }
    req.player_view = a.pv;
    if let Some(row) = a.view_weapon {
        // DC-DEFAULT: CLI-34 (proposed), see DC_DEFAULTS.md (first hand identifier); REN-15 (proposed) (viewer
        // placement: origin at --view-offset, rotation identity, idle clip)
        let vfs = dc_rez::RezVfs::new(&rom);
        let table = match ViewModels::from_vfs(&vfs) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("worldview: {e}");
                return ExitCode::FAILURE;
            }
        };
        let Some(v) = table.rows.get(&row) else {
            eprintln!("worldview: no weapon row {row} with a first-person model");
            return ExitCode::from(2);
        };
        let mut vr = ViewModelRequest::new(&v.model);
        vr.textures = v.skins.clone();
        vr.styles = v.styles.clone();
        vr.offset = a.view_offset;
        vr.clip = a.view_clip;
        vr.phase = a.view_phase;
        req.view_model = Some(vr);
    }

    if a.window {
        return on_screen(&rom, &req, settings, a.window_secs);
    }
    let viewer = match RomViewer::open(&rom, a.adapter, settings) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("worldview: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (img, report) = match viewer.render(&req) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("worldview: {e}");
            return ExitCode::FAILURE;
        }
    };
    // the 2009 gamma ramp of the final picture: --gamma, else the user's profile value, else the code default
    // SPEC: RENDER_2009.md §2.5 (engine+0xf6f10); DC-DEFAULT: REN-21 (proposed), see the staged rows
    let gamma = a
        .gamma
        .or_else(|| dc_render::profile_gamma(&rom))
        .unwrap_or(PROFILE_GAMMA_DEFAULT);
    let mut img = img;
    apply_ramp(&mut img.rgba, &ramp_for_gamma(gamma));
    let out = a.out.unwrap_or_else(|| {
        default_out_dir().join(format!("{}.png", report.map.to_ascii_lowercase()))
    });
    if let Err(e) = img.save_png(&out) {
        eprintln!("worldview: {e}");
        return ExitCode::FAILURE;
    }
    println!("{}", report.summary());
    println!("  gamma {gamma}");
    println!("  wrote {}", out.display());
    ExitCode::SUCCESS
}

#[cfg(feature = "window")]
fn on_screen(
    rom: &std::path::Path,
    req: &ViewRequest,
    settings: RenderSettings,
    secs: f32,
) -> ExitCode {
    match run_window(rom, req, settings, WindowOptions { seconds: secs }) {
        Ok(r) => {
            println!("{}", r.summary());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("worldview: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "window"))]
fn on_screen(
    _rom: &std::path::Path,
    _req: &ViewRequest,
    _s: RenderSettings,
    _secs: f32,
) -> ExitCode {
    eprintln!("worldview: built without the `window` feature (cargo build -p dc-render --features window)");
    ExitCode::from(2)
}

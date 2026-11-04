//! `render --textured`: the map as the game draws it, through a headless `dc-client` (MAP-95).
//!
//! The software renderer ([`crate::render_sw`]) needs no GPU, no game binaries and no ROM, and shows flat colours.
//! This path exports the project, runs the real renderer on the exported world with the project's real textures
//! (its own and any ROM ones), and keeps the last frame. A picture holding ROM textures is the user's own: it
//! stays under `build/`, which is never committed.

use crate::cli::Outcome;
use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::render_sw::View;
use crate::{build, export, play, project::Project, romlist};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Frames the client runs: the first frames can still be loading, the last one is the picture.
const FRAMES: &str = "3";
/// Half of the vertical view angle the framing of `top` and `iso` assumes, in degrees.
const FRAME_HALF_DEG: f32 = 36.0;
/// The pitch of the `iso` view (degrees, down) and its yaw.
const ISO_PITCH: f32 = 35.0;
const ISO_YAW: f32 = 45.0;
/// `top` looks almost straight down: a pitch of exactly 90 degrees leaves the camera's yaw undefined.
const TOP_PITCH: f32 = 89.9;

/// What `render --textured` was asked for.
pub struct TexturedRequest<'a> {
    pub dir: &'a Path,
    pub view: View,
    pub size: (u32, u32),
    pub out: Option<PathBuf>,
    pub binaries: Option<&'a Path>,
    pub rom: Option<&'a Path>,
}

/// The picture and the answer the CLI and the MCP tool share.
pub struct Textured {
    pub outcome: Outcome,
    pub png: Vec<u8>,
}

/// The default picture path: `build/render/<view>_textured.png`.
pub fn default_out(dir: &Path, view: &View) -> PathBuf {
    dir.join("build")
        .join("render")
        .join(format!("{}_textured.png", view.slug()))
}

/// The client's `--camera` argument for a view, given the world's box. `None`: the view is a spawn, which the
/// client's own `--spawn` places. The client's pitch looks down when positive, the mapkit's looks up.
pub fn camera_arg(view: &View, lo: [f32; 3], hi: [f32; 3]) -> Option<String> {
    let centre = [
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    ];
    let half = ((hi[0] - lo[0]).max(hi[2] - lo[2]) / 2.0).max(1.0);
    let back = half / FRAME_HALF_DEG.to_radians().tan();
    let text = |pos: [f32; 3], yaw: f32, down: f32| {
        format!("{},{},{},{yaw},{down}", pos[0], pos[1], pos[2])
    };
    match view {
        View::Spawn(_) => None,
        View::Free { pos, yaw, pitch } => Some(text(*pos, *yaw, -*pitch)),
        View::Top => Some(text([centre[0], hi[1] + back, centre[2]], 0.0, TOP_PITCH)),
        View::Iso => {
            let (yaw, down) = (ISO_YAW.to_radians(), ISO_PITCH.to_radians());
            let d = back + half;
            Some(text(
                [
                    centre[0] - yaw.sin() * down.cos() * d,
                    centre[1] + down.sin() * d,
                    centre[2] - yaw.cos() * down.cos() * d,
                ],
                ISO_YAW,
                ISO_PITCH,
            ))
        }
    }
}

fn client_error(message: impl Into<String>, fix: &str) -> Diagnostics {
    Diagnostics::one(Diagnostic::new(Code::RenderClient, "render", message.into()).with_fix(fix))
}

/// Exports the project, runs the client and keeps its last frame.
pub fn draw_textured(r: &TexturedRequest) -> Result<Textured, Diagnostics> {
    let (p, h) = Project::open(r.dir)?;
    if let View::Spawn(n) = r.view {
        let spawns = p
            .objects
            .iter()
            .filter(|o| o.class == crate::vocab_names::CLASS_PLAYER_SPAWN)
            .count();
        if n as usize >= spawns {
            return Err(Diagnostics::one(Diagnostic::new(
                Code::OpInvalid,
                "--view",
                format!("spawn {n}: the project has {spawns} player spawns"),
            )));
        }
    }
    let client = play::find_binary(r.binaries, "dc-client")?;
    let assets = build::Assets::new(r.dir);
    let level = build::level_of(&p, &h, &assets)?;
    let camera = camera_arg(
        &r.view,
        level.extent_min.to_array(),
        level.extent_max.to_array(),
    );
    let built = build::build(&p, &h, &assets, &p.build)?;
    let build_dir = r.dir.join("build");
    let exported = export::export(
        &p,
        &built,
        &build_dir,
        export::ExportOptions {
            zip: false,
            no_rom_paths: false,
            project_dir: Some(r.dir.to_path_buf()),
            force: true,
        },
    )?;
    let shots = build_dir.join("render").join(".textured");
    let io = |path: &Path, e: std::io::Error| {
        Diagnostics::one(Diagnostic::new(
            Code::ProjectIo,
            path.display().to_string(),
            e.to_string(),
        ))
    };
    // `export` writes the mod folder only: the world the client opens is this build's, never an older one
    let world = build_dir.join(format!("{}.dat", p.key));
    crate::project::write_atomic(&world, &built.dat).map_err(|e| io(&world, e))?;
    let _ = std::fs::remove_dir_all(&shots);
    std::fs::create_dir_all(&shots).map_err(|e| io(&shots, e))?;
    let mut cmd = Command::new(&client);
    cmd.arg("--world-file")
        .arg(&world)
        .arg("--mod-dir")
        .arg(&exported.dir)
        .arg("--headless-frames")
        .arg(FRAMES)
        .arg("--out")
        .arg(&shots)
        .arg("--size")
        .arg(format!("{}x{}", r.size.0, r.size.1))
        .args(["--no-sound", "--no-overlay", "--clean-frame"]);
    if let View::Spawn(n) = r.view {
        cmd.arg("--spawn").arg(n.to_string());
    }
    if let Some(c) = &camera {
        cmd.arg("--camera").arg(c);
    }
    let rom = romlist::discover_rom(r.rom).map(|(path, _)| path);
    if let Some(path) = &rom {
        cmd.arg("--rom").arg(path);
    }
    let run = cmd.output().map_err(|e| {
        client_error(
            format!("could not start {}: {e}", client.display()),
            "build it (cargo build -p dc-client) or give its folder with --binaries",
        )
    })?;
    let mut frames: Vec<PathBuf> = std::fs::read_dir(&shots)
        .map_err(|e| io(&shots, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|f| f.extension().is_some_and(|x| x == "png"))
        .collect();
    frames.sort();
    let Some(last) = frames.last() else {
        let said = String::from_utf8_lossy(&run.stderr);
        let tail: Vec<&str> = said.lines().rev().take(4).collect();
        return Err(client_error(
            format!(
                "{} drew no picture ({}): {}",
                client.display(),
                run.status,
                tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
            ),
            "an old dc-client has no --camera or --clean-frame: rebuild it. Without a GPU, use `render` without \
             --textured",
        ));
    };
    let png = std::fs::read(last).map_err(|e| io(last, e))?;
    let out = r.out.clone().unwrap_or_else(|| default_out(r.dir, &r.view));
    crate::project::write_atomic(&out, &png).map_err(|e| {
        Diagnostics::one(Diagnostic::new(
            Code::ProjectIo,
            out.display().to_string(),
            e.to_string(),
        ))
    })?;
    let _ = std::fs::remove_dir_all(&shots);
    let text = format!(
        "rendered {} with the game's renderer to {} ({}x{})",
        r.view.text(),
        out.display(),
        r.size.0,
        r.size.1
    );
    let mut outcome = Outcome::new(
        json!({
            "view": r.view.text(),
            "image": out.display().to_string(),
            "width": r.size.0,
            "height": r.size.1,
            "textured": true,
            "client": client.display().to_string(),
            "rom": rom.as_ref().map(|path| path.display().to_string()),
        }),
        text,
    );
    outcome.diagnostics = built.diagnostics;
    outcome.diagnostics.extend(exported.diagnostics);
    Ok(Textured { outcome, png })
}

/// The size of a map's preview picture: the waiting room's map pictures in the ROM are 256 x 256 (MAP-100a).
pub const PREVIEW_SIZE: u32 = 256;
/// It is drawn this many times larger, then scaled down, so it does not shimmer.
const PREVIEW_OVERSAMPLE: u32 = 3;
/// The size of a map's loading picture: the client stretches it over its 1024 x 768 page (MAP-32).
pub const LOADING_SIZE: (u32, u32) = (1024, 768);

/// The size of a map's title picture: the banner images of the ROM are 512 x 128 (MAP-102).
pub const TITLE_SIZE: (u32, u32) = (512, 128);
/// The title's font: Roboto (Apache-2.0, `assets/fonts/`), the editor's font.
const TITLE_FONT: &[u8] = include_bytes!("../../../assets/fonts/Roboto-Regular.ttf");

/// The map's name as a title picture on a transparent ground: capitals, fitted to the picture, with a soft shadow.
pub fn title_png(name: &str) -> Result<Vec<u8>, String> {
    use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
    let font = FontRef::try_from_slice(TITLE_FONT).map_err(|e| e.to_string())?;
    let text = name.trim().to_uppercase();
    let (w, h) = TITLE_SIZE;
    let width_at = |px: f32| {
        let f = font.as_scaled(PxScale::from(px));
        text.chars()
            .map(|c| f.h_advance(f.glyph_id(c)))
            .sum::<f32>()
    };
    let mut px = 72.0;
    while px > 12.0 && width_at(px) > w as f32 - 32.0 {
        px -= 1.0;
    }
    let scaled = font.as_scaled(PxScale::from(px));
    let baseline = (h as f32 + scaled.ascent() + scaled.descent()) / 2.0;
    let mut img = image::RgbaImage::new(w, h);
    let mut stamp = |dx: f32, dy: f32, rgb: [u8; 3], strength: f32| {
        let mut x = (w as f32 - width_at(px)) / 2.0 + dx;
        for c in text.chars() {
            let glyph = scaled
                .glyph_id(c)
                .with_scale_and_position(px, ab_glyph::point(x, baseline + dy));
            x += scaled.h_advance(scaled.glyph_id(c));
            let Some(outline) = font.outline_glyph(glyph) else {
                continue;
            };
            let b = outline.px_bounds();
            outline.draw(|gx, gy, cover| {
                let (px_x, px_y) = (gx as i32 + b.min.x as i32, gy as i32 + b.min.y as i32);
                if px_x < 0 || px_y < 0 || px_x >= w as i32 || px_y >= h as i32 {
                    return;
                }
                let dst = img.get_pixel_mut(px_x as u32, px_y as u32);
                let a = (cover * strength).clamp(0.0, 1.0);
                let old = dst.0[3] as f32 / 255.0;
                let out = a + old * (1.0 - a);
                if out > 0.0 {
                    for i in 0..3 {
                        dst.0[i] =
                            ((rgb[i] as f32 * a + dst.0[i] as f32 * old * (1.0 - a)) / out) as u8;
                    }
                    dst.0[3] = (out * 255.0) as u8;
                }
            });
        }
    };
    stamp(3.0, 3.0, [0, 0, 0], 0.75);
    stamp(0.0, 0.0, [255, 255, 255], 1.0);
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(png.into_inner())
}

/// A plain picture of the map's name, for a map whose renders would show ROM textures (DC-DEFAULT: MAP-103): a dark
/// steel gradient, a thin line and the title. It holds nothing of the ROM and nothing of the map.
pub fn card_png(name: &str, size: (u32, u32)) -> Result<Vec<u8>, String> {
    let (w, h) = size;
    let mut img = image::RgbaImage::from_fn(w, h, |_, y| {
        let t = y as f32 / h.max(1) as f32;
        let shade = |top: f32, bottom: f32| (top + (bottom - top) * t) as u8;
        image::Rgba([shade(58.0, 20.0), shade(64.0, 23.0), shade(70.0, 27.0), 255])
    });
    let title = image::load_from_memory(&title_png(name)?)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let title_w = w.saturating_sub(w / 16).max(1);
    let title_h = (title_w * TITLE_SIZE.1 / TITLE_SIZE.0).max(1);
    let scaled = image::imageops::resize(
        &title,
        title_w,
        title_h,
        image::imageops::FilterType::Triangle,
    );
    let (x, y) = ((w - title_w) / 2, h.saturating_sub(title_h) / 2);
    image::imageops::overlay(&mut img, &scaled, x as i64, y as i64);
    // blending the soft edges of the title rounds the ground's alpha down
    for px in img.pixels_mut() {
        px.0[3] = 255;
    }
    let line_y = (y + title_h).min(h.saturating_sub(1));
    for px in x..x + title_w {
        img.put_pixel(px, line_y, image::Rgba([200, 150, 60, 255]));
    }
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(png.into_inner())
}

/// What `pictures` was asked for.
pub struct PicturesRequest<'a> {
    pub dir: &'a Path,
    pub preview: View,
    pub loading: View,
    pub binaries: Option<&'a Path>,
    pub rom: Option<&'a Path>,
}

/// Writes the two pictures `export` packs: `assets/pictures/<key>.png` (the map's preview in the waiting room) and
/// `assets/pictures/<key>_load.png` (its loading screen), drawn with the game's renderer. DC-DEFAULT: MAP-100a
pub fn pictures(r: &PicturesRequest) -> Result<Outcome, Diagnostics> {
    let (p, _) = Project::open(r.dir)?;
    let bad = |e: String| Diagnostics::one(Diagnostic::new(Code::RenderClient, "pictures", e));
    // DC-DEFAULT: MAP-103 a render of ROM textures holds ROM pixels and may not ship (docs/MODDING_GUIDE.md section 8)
    let uses_rom = p
        .materials
        .iter()
        .any(|m| matches!(m.texture, crate::project::TextureRef::Rom(_)));
    let mut diagnostics = Diagnostics::new();
    let (preview_png, loading_png) = if uses_rom {
        (
            card_png(&p.name, (PREVIEW_SIZE, PREVIEW_SIZE)).map_err(bad)?,
            card_png(&p.name, LOADING_SIZE).map_err(bad)?,
        )
    } else {
        let scratch = r.dir.join("build").join("render").join(".pictures.png");
        let draw = |view: &View, size: (u32, u32)| {
            draw_textured(&TexturedRequest {
                dir: r.dir,
                view: view.clone(),
                size,
                out: Some(scratch.clone()),
                binaries: r.binaries,
                rom: r.rom,
            })
        };
        let drawn = draw(
            &r.preview,
            (
                PREVIEW_SIZE * PREVIEW_OVERSAMPLE,
                PREVIEW_SIZE * PREVIEW_OVERSAMPLE,
            ),
        )?;
        let big = image::load_from_memory(&drawn.png).map_err(|e| bad(e.to_string()))?;
        let small = big.resize_exact(
            PREVIEW_SIZE,
            PREVIEW_SIZE,
            image::imageops::FilterType::Triangle,
        );
        let mut preview = std::io::Cursor::new(Vec::new());
        small
            .write_to(&mut preview, image::ImageFormat::Png)
            .map_err(|e| bad(e.to_string()))?;
        let loading = draw(&r.loading, LOADING_SIZE)?;
        let _ = std::fs::remove_file(&scratch);
        diagnostics = loading.outcome.diagnostics;
        (preview.into_inner(), loading.png)
    };
    let rel = |suffix: &str| format!("assets/pictures/{}{suffix}.png", p.key);
    let (preview_rel, loading_rel, title_rel) = (rel(""), rel("_load"), rel("_title"));
    let title = title_png(&p.name).map_err(bad)?;
    for (file, bytes) in [
        (&preview_rel, preview_png.as_slice()),
        (&loading_rel, loading_png.as_slice()),
        (&title_rel, title.as_slice()),
    ] {
        let path = Project::resolve(r.dir, file);
        crate::project::write_atomic(&path, bytes).map_err(|e| {
            Diagnostics::one(Diagnostic::new(
                Code::ProjectIo,
                path.display().to_string(),
                e.to_string(),
            ))
        })?;
    }
    let how = if uses_rom {
        "plain name cards, because this map uses ROM textures"
    } else {
        "drawn with the game's renderer"
    };
    let text =
        format!("wrote {preview_rel}, {loading_rel} and {title_rel} ({how}); export packs them");
    let mut o = Outcome::new(
        json!({
            "preview": {"file": preview_rel, "view": r.preview.text(), "width": PREVIEW_SIZE, "height": PREVIEW_SIZE},
            "loading": {"file": loading_rel, "view": r.loading.text(), "width": LOADING_SIZE.0, "height": LOADING_SIZE.1},
            "title": {"file": title_rel, "width": TITLE_SIZE.0, "height": TITLE_SIZE.1},
            "kind": if uses_rom { "card" } else { "render" },
        }),
        text,
    );
    o.diagnostics = diagnostics;
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(s: &str) -> Vec<f32> {
        s.split(',').map(|t| t.parse().unwrap()).collect()
    }

    #[test]
    fn a_card_is_opaque_and_the_asked_size() {
        for size in [(PREVIEW_SIZE, PREVIEW_SIZE), LOADING_SIZE] {
            let img = image::load_from_memory(&card_png("Shipping Container", size).unwrap())
                .unwrap()
                .to_rgba8();
            assert_eq!(img.dimensions(), size);
            assert!(img.pixels().all(|p| p.0[3] == 255));
        }
    }

    #[test]
    fn a_title_picture_holds_the_name_on_a_clear_ground() {
        let img = image::load_from_memory(&title_png("Shipping Container").unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(img.dimensions(), TITLE_SIZE);
        assert_eq!(img.get_pixel(0, 0).0[3], 0, "the corner stays clear");
        assert!(
            img.pixels().any(|p| p.0 == [255, 255, 255, 255]),
            "the letters"
        );
        // a long name still fits: its letters stop short of the picture's edges
        let long = image::load_from_memory(&title_png(&"Long Name ".repeat(6)).unwrap())
            .unwrap()
            .to_rgba8();
        assert!((0..TITLE_SIZE.1).all(|y| long.get_pixel(0, y).0[3] == 0));
    }

    #[test]
    fn a_free_view_flips_the_pitch_to_the_clients_sign() {
        let v = View::parse("free:1,2,3,90,-20").unwrap();
        let c = numbers(&camera_arg(&v, [0.0; 3], [1.0; 3]).unwrap());
        assert_eq!(c, vec![1.0, 2.0, 3.0, 90.0, 20.0]);
    }

    #[test]
    fn a_spawn_view_has_no_camera_and_top_and_iso_stand_outside_the_box() {
        let (lo, hi) = ([-1000.0, -100.0, -500.0], [1000.0, 700.0, 500.0]);
        assert!(camera_arg(&View::Spawn(2), lo, hi).is_none());
        let top = numbers(&camera_arg(&View::Top, lo, hi).unwrap());
        assert!(top[1] > hi[1] && top[4] > 80.0);
        assert_eq!((top[0], top[2]), (0.0, 0.0));
        let iso = numbers(&camera_arg(&View::Iso, lo, hi).unwrap());
        assert!(iso[0] < lo[0] && iso[1] > hi[1] && iso[2] < lo[2]);
    }
}

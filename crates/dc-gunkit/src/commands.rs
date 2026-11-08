//! Everything the studio can be told to do, by name, with JSON arguments. The MCP tools, the window's own buttons where it helps,
//! and the headless server all go through [`run`], so an agent and a person change the same things.

use crate::design::{Art, Design};
use crate::guns::View;
use crate::pack::{self, SaveOptions};
use crate::patterns;
use crate::studio::{Studio, BACKGROUNDS};
use dc_render::camo::{self, CamoFlow, CamoFx, Circle};
use dc_render::TextureImage;
use serde_json::{json, Value};

/// A command's answer: a document, and a picture when it made one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub json: Value,
    pub png: Option<Vec<u8>>,
}

impl From<Value> for Reply {
    fn from(json: Value) -> Reply {
        Reply { json, png: None }
    }
}

/// One tool: its name, what it is for, and its arguments as a JSON Schema.
pub struct Tool {
    pub name: &'static str,
    pub about: &'static str,
    pub schema: fn() -> Value,
}

fn no_args() -> Value {
    json!({"type": "object", "properties": {}})
}

fn art_props() -> Value {
    json!({
        "path": {"type": "string", "description": "A PNG (or a pack's DTX) on this machine. The studio watches the file: save over it and the gun updates by itself. The easiest loop for an agent."},
        "png_base64": {"type": "string", "description": "The PNG itself, in base64."},
        "preset": {"type": "string", "description": "A generated starter pattern. See status for the names."},
        "colours": {"type": "array", "items": {"type": "string"}, "minItems": 4, "maxItems": 4, "description": "Four rrggbb colours for a preset."},
        "seed": {"type": "integer", "description": "Another picture from the same preset."}
    })
}

/// Every tool, in the order an agent is likely to want them.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "status",
        about: "What is on show: the gun, the view, the design, the turn, and the names of the presets and backgrounds.",
        schema: no_args,
    },
    Tool {
        name: "list_guns",
        about: "The guns of the player's game, those on sale first.",
        schema: || {
            json!({"type": "object", "properties": {
                "filter": {"type": "string", "description": "Part of a name."},
                "limit": {"type": "integer", "description": "At most this many (default 400)."}
            }})
        },
    },
    Tool {
        name: "select_gun",
        about: "Puts a gun on show, by row or name. The design stays, so one camo can be tried on many guns.",
        schema: || {
            json!({"type": "object", "properties": {
                "gun": {"type": ["string", "integer"], "description": "A row number or a name, such as 25 or G36E."}
            }, "required": ["gun"]})
        },
    },
    Tool {
        name: "set_view",
        about: "Shows the first-person model (1024 texture, what the player holds) or the third-person one (256 texture, what others see). Check both before calling a camo done.",
        schema: || {
            json!({"type": "object", "properties": {
                "view": {"type": "string", "enum": ["first_person", "third_person"]},
                "hands": {"type": "boolean", "description": "Show the player's hands on the first-person model."},
                "whole_gun": {"type": "boolean", "description": "On (the default): first person shows a whole gun, the third-person model's shape with the first-person texture on it, on every gun that fits. Off: the first-person model as it is, which has little or no far side."}
            }})
        },
    },
    Tool {
        name: "set_pattern",
        about: "Sets the pattern painted over the gun's texture. It repeats twice across the texture and the gun's shading shows through. Updates the gun at once. Give path, png_base64 or preset.",
        schema: || {
            let mut p = art_props();
            p["layer"] = json!({"type": "string", "enum": ["paint", "effect"], "description": "paint (default), or effect: the pattern that moves over the gun as light."});
            json!({"type": "object", "properties": p})
        },
    },
    Tool {
        name: "clear_pattern",
        about: "Takes a pattern off: the paint, the effect's own pattern, or the whole design (all).",
        schema: || {
            json!({"type": "object", "properties": {
                "layer": {"type": "string", "enum": ["paint", "effect", "full", "all"], "description": "Default paint. full removes this gun and view's full texture."}
            }})
        },
    },
    Tool {
        name: "set_full_texture",
        about: "Replaces this gun and view's whole texture with your own art, drawn to the gun's layout (see get_texture base for the layout). No shading is added, so draw it in. It may not be the game's texture or a recolour of it: saving refuses that.",
        schema: || json!({"type": "object", "properties": art_props()}),
    },
    Tool {
        name: "set_effect",
        about: "Animates the camo: a pattern drifts over the gun as light, and may pulse. Values are held to the game's limits and the answer shows what was kept. Use off to remove it.",
        schema: || {
            json!({"type": "object", "properties": {
                "off": {"type": "boolean"},
                "drift_u": {"type": "number", "description": "Repeats per second across, -1..1. A slow drift is 0.02."},
                "drift_v": {"type": "number", "description": "Repeats per second down, -1..1."},
                "gain_low": {"type": "number", "description": "Lowest brightness, 0..1."},
                "gain_high": {"type": "number", "description": "Highest brightness, 0..1."},
                "pulse_seconds": {"type": "number", "description": "Seconds per pulse, at least 1. 0 is steady at the highest brightness."}
            }})
        },
    },
    Tool {
        name: "set_flow",
        about: "Makes the paint itself move, like a lava lamp: the pattern's colours drift and are swirled by slow waves, so its blobs travel, stretch and fold over the gun. It is paint, lit like the gun, so it does not glow; it takes the place of set_effect's light. Needs a paint pattern. Values are held to the game's limits. Use off to remove it.",
        schema: || {
            json!({"type": "object", "properties": {
                "off": {"type": "boolean"},
                "drift_u": {"type": "number", "description": "Repeats per second across, -1..1. A slow drift is 0.03."},
                "drift_v": {"type": "number", "description": "Repeats per second down, -1..1."},
                "swirl": {"type": "number", "description": "How far the waves push the pattern about, in repeats, 0..0.25. 0.1 is a gentle lava lamp."},
                "swirl_seconds": {"type": "number", "description": "Seconds per turn of the waves, at least 3."}
            }})
        },
    },
    Tool {
        name: "set_keep",
        about: "The circles of this gun and view's texture left as the game drew them. A gun the game gives a sight already has its lenses kept clean, on both models, so this is for anything else, such as a sight's glass or a knob. In texels of the texture; see describe_texture. Replaces the list; an empty list paints the lens too.",
        schema: || {
            json!({"type": "object", "properties": {
                "circles": {"type": "array", "items": {"type": "object", "properties": {
                    "x": {"type": "number"}, "y": {"type": "number"}, "radius": {"type": "number"}
                }, "required": ["x", "y", "radius"]}},
                "auto": {"type": "boolean", "description": "Go back to the lenses the studio found, instead of giving circles."}
            }})
        },
    },
    Tool {
        name: "set_camera",
        about: "Turns the gun and frames it. Any value left out stays.",
        schema: || {
            json!({"type": "object", "properties": {
                "yaw": {"type": "number", "description": "Degrees. -90 shows its left side, 90 its right side, 0 looks down the barrel."},
                "pitch": {"type": "number", "description": "Degrees, -89..89."},
                "zoom": {"type": "number", "description": "1 fits the gun; larger is closer, up to 12."},
                "pan_x": {"type": "number"},
                "pan_y": {"type": "number"},
                "reset": {"type": "boolean"},
                "background": {"type": "string", "description": "dark, grey, light, or an rrggbb colour."}
            }})
        },
    },
    Tool {
        name: "screenshot",
        about: "A picture of the gun exactly as it is on show, with the current turn. Look at it after every change. An animated camo shows the moment it was taken.",
        schema: || {
            json!({"type": "object", "properties": {
                "width": {"type": "integer", "description": "Default 1280."},
                "height": {"type": "integer", "description": "Default 720."},
                "path": {"type": "string", "description": "Also write the PNG here."}
            }})
        },
    },
    Tool {
        name: "window_screenshot",
        about: "A picture of the whole studio window, as the user sees it.",
        schema: no_args,
    },
    Tool {
        name: "describe_texture",
        about: "What is where on this gun and view's texture: its size, the lenses found and kept clean, the circles kept, and the largest parts it is cut into with where each lies on the gun, such as \"front, high, facing up\". The picture is the painted texture with the parts outlined in yellow and the kept circles in cyan. Call this before painting to a layout or placing a detail.",
        schema: || {
            json!({"type": "object", "properties": {
                "parts": {"type": "integer", "description": "At most this many parts, largest first (default 24)."},
                "path": {"type": "string", "description": "Also write the PNG here."}
            }})
        },
    },
    Tool {
        name: "get_texture",
        about: "The texture itself. shown is the painted result; base is the game's own, to see the gun's layout and find a lens for set_keep. The base comes from the player's own game: never put it, or anything made from it, in a pack or a commit.",
        schema: || {
            json!({"type": "object", "properties": {
                "which": {"type": "string", "enum": ["shown", "base"], "description": "Default shown."},
                "path": {"type": "string", "description": "Also write the PNG here."}
            }})
        },
    },
    Tool {
        name: "save_pack",
        about: "Saves the camo as a Vanilla++ pack for the gun on show, covering both of its models, and checks it with the same code as dc-modkit validate and lint. Refuses art that copies the game's texture.",
        schema: || {
            json!({"type": "object", "properties": {
                "name": {"type": "string", "description": "The camo's name, such as Night Tiger."},
                "author": {"type": "string"},
                "row": {"type": "integer", "description": "The variant's row, 500..899. Default: chosen from the name."},
                "dir": {"type": "string", "description": "Default: a folder under Documents/DeCombat Camos."}
            }, "required": ["name"]})
        },
    },
    Tool {
        name: "open_pack",
        about: "Opens a saved camo or any variant pack and shows it on its gun. Give dir, or example with a name from status.",
        schema: || {
            json!({"type": "object", "properties": {
                "dir": {"type": "string"},
                "example": {"type": "string"}
            }})
        },
    },
];

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(ABC[(n >> 18) as usize & 63] as char);
        out.push(ABC[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            ABC[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            ABC[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Standard base64 back to bytes (white space ignored).
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return Err("png_base64 is not base64".into()),
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// A picture as PNG bytes.
pub fn png_of(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(width, height, rgba).ok_or("picture size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn write_if_asked(args: &Value, png: &[u8]) -> Result<Option<String>, String> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return Ok(None);
    };
    std::fs::write(path, png).map_err(|e| format!("{path}: {e}"))?;
    Ok(Some(path.to_string()))
}

fn art_of(args: &Value) -> Result<Art, String> {
    if let Some(p) = args.get("path").and_then(Value::as_str) {
        return Art::from_file(std::path::Path::new(p));
    }
    if let Some(b) = args.get("png_base64").and_then(Value::as_str) {
        return Art::from_png(&base64_decode(b)?);
    }
    if let Some(name) = args.get("preset").and_then(Value::as_str) {
        let colours = match args.get("colours").and_then(Value::as_array) {
            None => None,
            Some(list) => {
                let parsed: Option<Vec<[u8; 3]>> = list
                    .iter()
                    .map(|v| v.as_str().and_then(patterns::parse_colour))
                    .collect();
                let list: [[u8; 3]; 4] = parsed
                    .and_then(|v| v.try_into().ok())
                    .ok_or("colours must be four rrggbb values")?;
                Some(list)
            }
        };
        let seed = args.get("seed").and_then(Value::as_u64).unwrap_or(1) as u32;
        return patterns::generate(name, colours, seed);
    }
    Err("give path, png_base64 or preset".into())
}

fn number(args: &Value, key: &str) -> Option<f32> {
    args.get(key).and_then(Value::as_f64).map(|v| v as f32)
}

fn fx_json(fx: Option<CamoFx>) -> Value {
    match fx {
        None => Value::Null,
        Some(f) => json!({
            "drift_u": f.speed[0],
            "drift_v": f.speed[1],
            "gain_low": f.gain[0],
            "gain_high": f.gain[1],
            "pulse_seconds": f.seconds,
        }),
    }
}

/// The whole state, as `status` answers and as every change answers.
pub fn status(s: &Studio) -> Value {
    let (row, view) = (s.row(), s.view);
    let keep: Vec<Value> = row
        .map(|r| s.design.keep_of(r, view).to_vec())
        .unwrap_or_default()
        .iter()
        .map(|c| json!({"x": c.x, "y": c.y, "radius": c.radius}))
        .collect();
    let tex = s.base_texture();
    json!({
        "gun": s.gun().map(|g| g.summary()),
        "view": view,
        "hands": s.show_hands,
        "has_hands": s.has_hands(),
        "reflected": s.is_reflected(),
        "whole_gun": s.whole_fit().map(|f| json!({"overlap": f.overlap, "tilt": f.tilt, "matched": f.matched, "gap": f.gap})),
        "whole_gun_refused_overlap": s.whole_refused(),
        "lenses_kept_clean": s.lenses().iter().map(|l| json!({"x": l.circle.x, "y": l.circle.y, "radius": l.circle.radius})).collect::<Vec<_>>(),
        "reflection": s.reflection_shape().map(|r| json!({
            "across": [r.across.x, r.across.z],
            "centre": r.centre,
            "balance": r.balance,
            "spread": r.spread,
        })),
        "texture_size": tex.map(|t| [t.width, t.height]),
        "design": {
            "name": s.design.name,
            "layers": pack::describe(&s.design),
            "full_texture": row.is_some_and(|r| s.design.full.contains_key(&(r, view))),
            "effect": fx_json(s.design.fx),
            "flow": s.design.flow.map(|f| json!({
                "drift_u": f.speed[0],
                "drift_v": f.speed[1],
                "swirl": f.swirl,
                "swirl_seconds": f.seconds,
            })),
            "keep": keep,
            "animated": s.animating(),
        },
        "camera": {"yaw": s.orbit.yaw, "pitch": s.orbit.pitch, "zoom": s.orbit.zoom, "pan": s.orbit.pan},
        "background": s.background,
        "presets": patterns::PRESETS.iter().map(|p| p.0).collect::<Vec<_>>(),
        "backgrounds": BACKGROUNDS.iter().map(|b| b.0).collect::<Vec<_>>(),
        "examples": pack::EXAMPLES.iter().map(|e| e.0).collect::<Vec<_>>(),
    })
}

fn need_row(s: &Studio) -> Result<u32, String> {
    s.row()
        .ok_or_else(|| "no gun is on show: call select_gun first".into())
}

/// The base texture of a view of a gun from the player's own ROM, for a save's check.
fn image_png(t: &TextureImage) -> Result<Vec<u8>, String> {
    let px = t.levels.first().ok_or("the texture is empty")?;
    png_of(t.width, t.height, px.clone())
}

/// Outlines a texture's parts and kept circles, for looking at.
fn mark_texture(
    px: &mut [u8],
    width: u32,
    height: u32,
    parts: &[crate::studio::TexturePart],
    keep: &[Circle],
) {
    let (w, h) = (width as i32, height as i32);
    let mut dot = |x: i32, y: i32, c: [u8; 3]| {
        if x >= 0 && y >= 0 && x < w && y < h {
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 3].copy_from_slice(&c);
            px[i + 3] = 255;
        }
    };
    for p in parts {
        let [x0, y0, x1, y1] = p.texels.map(|v| v.round() as i32);
        for x in x0..=x1 {
            dot(x, y0, [255, 220, 40]);
            dot(x, y1, [255, 220, 40]);
        }
        for y in y0..=y1 {
            dot(x0, y, [255, 220, 40]);
            dot(x1, y, [255, 220, 40]);
        }
    }
    for c in keep {
        let steps = (c.radius * 8.0).ceil().max(16.0) as i32;
        for k in 0..steps {
            let a = k as f32 / steps as f32 * std::f32::consts::TAU;
            let (x, y) = (c.x + c.radius * a.cos(), c.y + c.radius * a.sin());
            for t in 0..2 {
                dot(x.round() as i32 + t, y.round() as i32, [60, 230, 255]);
            }
        }
    }
}

/// Opens a pack's design: its gun on show, its art on the gun.
pub fn apply_opened(s: &mut Studio, opened: pack::Opened) -> Result<Vec<String>, String> {
    s.design = opened.design;
    s.select(opened.row)?;
    // a variant is named with its gun, which a save adds again
    if let Some(gun) = s.gun().map(|g| g.name.clone()) {
        if let Some(rest) = s.design.name.strip_prefix(&format!("{gun} ")) {
            s.design.name = rest.to_string();
        }
    }
    Ok(opened.notes)
}

/// Runs one command.
pub fn run(s: &mut Studio, tool: &str, args: &Value) -> Result<Reply, String> {
    match tool {
        "status" => Ok(status(s).into()),
        "list_guns" => {
            let filter = args
                .get("filter")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(400) as usize;
            let list: Vec<Value> = s
                .guns
                .iter()
                .filter(|g| filter.is_empty() || g.name.to_ascii_lowercase().contains(&filter))
                .take(limit)
                .map(|g| g.summary())
                .collect();
            Ok(json!({"guns": list}).into())
        }
        "select_gun" => {
            let what = match args.get("gun") {
                Some(Value::String(t)) => t.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => return Err("give gun: a row number or a name".into()),
            };
            let row = s
                .find_gun(&what)
                .ok_or_else(|| format!("no gun matches {what:?}; try list_guns"))?;
            s.select(row)?;
            Ok(status(s).into())
        }
        "set_view" => {
            if let Some(on) = args.get("hands").and_then(Value::as_bool) {
                s.set_hands(on)?;
            }
            if let Some(on) = args.get("whole_gun").and_then(Value::as_bool) {
                s.set_whole_gun(on)?;
            }
            if let Some(v) = args.get("view").and_then(Value::as_str) {
                let view = View::parse(v).ok_or_else(|| format!("no view named {v:?}"))?;
                s.set_view(view)?;
            }
            Ok(status(s).into())
        }
        "set_pattern" => {
            let art = art_of(args)?;
            match args.get("layer").and_then(Value::as_str).unwrap_or("paint") {
                "paint" => s.design.paint = Some(art),
                "effect" => s.design.glow = Some(art),
                other => return Err(format!("no layer named {other:?}")),
            }
            s.repaint()?;
            Ok(status(s).into())
        }
        "clear_pattern" => {
            let row = s.row();
            let view = s.view;
            match args.get("layer").and_then(Value::as_str).unwrap_or("paint") {
                "paint" => s.design.paint = None,
                "effect" => s.design.glow = None,
                "full" => {
                    if let Some(r) = row {
                        s.design.full.remove(&(r, view));
                    }
                }
                "all" => {
                    s.design = Design {
                        name: s.design.name.clone(),
                        ..Design::default()
                    }
                }
                other => return Err(format!("no layer named {other:?}")),
            }
            s.repaint()?;
            Ok(status(s).into())
        }
        "set_full_texture" => {
            let row = need_row(s)?;
            let art = art_of(args)?;
            s.design.full.insert((row, s.view), art);
            s.repaint()?;
            Ok(status(s).into())
        }
        "set_effect" => {
            if args.get("off").and_then(Value::as_bool) == Some(true) {
                s.design.fx = None;
            } else {
                let was = s.design.fx.unwrap_or(CamoFx {
                    speed: [0.02, 0.012],
                    gain: [0.0, 0.65],
                    seconds: 0.0,
                });
                s.design.fx = Some(camo::limited_fx(CamoFx {
                    speed: [
                        number(args, "drift_u").unwrap_or(was.speed[0]),
                        number(args, "drift_v").unwrap_or(was.speed[1]),
                    ],
                    gain: [
                        number(args, "gain_low").unwrap_or(was.gain[0]),
                        number(args, "gain_high").unwrap_or(was.gain[1]),
                    ],
                    seconds: number(args, "pulse_seconds").unwrap_or(was.seconds),
                }));
            }
            s.repaint()?;
            let mut out = status(s);
            if s.design.fx.is_some()
                && s.design.paint.is_none()
                && s.design.glow.is_none()
                && !s
                    .row()
                    .is_some_and(|r| s.design.full.contains_key(&(r, s.view)))
            {
                out["note"] = json!("The effect has no pattern to move yet: call set_pattern.");
            }
            Ok(out.into())
        }
        "set_flow" => {
            if args.get("off").and_then(Value::as_bool) == Some(true) {
                s.design.flow = None;
            } else {
                let was = s.design.flow.unwrap_or(CamoFlow {
                    speed: [0.03, 0.01],
                    swirl: 0.1,
                    seconds: 8.0,
                });
                s.design.flow = Some(camo::limited_flow(CamoFlow {
                    speed: [
                        number(args, "drift_u").unwrap_or(was.speed[0]),
                        number(args, "drift_v").unwrap_or(was.speed[1]),
                    ],
                    swirl: number(args, "swirl").unwrap_or(was.swirl),
                    seconds: number(args, "swirl_seconds").unwrap_or(was.seconds),
                }));
            }
            s.repaint()?;
            let mut out = status(s);
            if s.design.flow.is_some() && s.design.paint.is_none() {
                out["note"] = json!("A flow moves the paint pattern: call set_pattern first.");
            } else if s.design.flow.is_some() && s.design.fx.is_some() {
                out["note"] = json!("The flow takes the place of the light effect, which the game will not draw with it.");
            }
            Ok(out.into())
        }
        "set_keep" => {
            let row = need_row(s)?;
            if args.get("auto").and_then(Value::as_bool) == Some(true) {
                s.use_found_lenses()?;
                return Ok(status(s).into());
            }
            let list = args
                .get("circles")
                .and_then(Value::as_array)
                .ok_or("give circles: a list of {x, y, radius}")?;
            let mut circles = Vec::new();
            for c in list {
                let (Some(x), Some(y), Some(radius)) =
                    (number(c, "x"), number(c, "y"), number(c, "radius"))
                else {
                    return Err("a circle needs x, y and radius".into());
                };
                if radius <= 0.0 || !x.is_finite() || !y.is_finite() || !radius.is_finite() {
                    return Err("a circle needs a radius above 0".into());
                }
                circles.push(Circle { x, y, radius });
            }
            s.design.keep.insert((row, s.view), circles);
            s.repaint()?;
            Ok(status(s).into())
        }
        "set_camera" => {
            if args.get("reset").and_then(Value::as_bool) == Some(true) {
                s.orbit = Default::default();
            }
            if let Some(v) = number(args, "yaw") {
                s.orbit.yaw = 0.0;
                s.orbit.turn(v, 0.0);
            }
            if let Some(v) = number(args, "pitch") {
                s.orbit.pitch = v.clamp(-89.0, 89.0);
            }
            if let Some(v) = number(args, "zoom") {
                s.orbit.zoom = 1.0;
                s.orbit.zoom_by(v);
            }
            if let Some(v) = number(args, "pan_x") {
                s.orbit.pan[0] = v;
            }
            if let Some(v) = number(args, "pan_y") {
                s.orbit.pan[1] = v;
            }
            if let Some(b) = args.get("background").and_then(Value::as_str) {
                s.background = BACKGROUNDS
                    .iter()
                    .find(|k| k.0.eq_ignore_ascii_case(b))
                    .map(|k| k.1)
                    .or_else(|| patterns::parse_colour(b))
                    .ok_or_else(|| format!("no background named {b:?}"))?;
            }
            s.revision += 1;
            let mut out = status(s);
            if s.far_side_is_reflected() {
                out["note"] = json!("This first-person model only has the side the player sees, so this side is its reflection, drawn by the studio alone. Judge the far side on third_person.");
            } else if s.whole_fit().is_some() {
                out["note"] = json!("This is the whole gun: the third-person model's shape with the first-person texture baked on. Both sides read the side the player sees, so writing on the far side is backwards. The game draws its own first-person model; set_view with whole_gun false shows that one.");
            }
            Ok(out.into())
        }
        "screenshot" => {
            need_row(s)?;
            let w = args.get("width").and_then(Value::as_u64).unwrap_or(1280) as u32;
            let h = args.get("height").and_then(Value::as_u64).unwrap_or(720) as u32;
            let img = s.picture(w, h)?;
            let png = png_of(img.width, img.height, img.rgba)?;
            let written = write_if_asked(args, &png)?;
            Ok(Reply {
                json: json!({"width": img.width, "height": img.height, "written": written, "animated": s.animating()}),
                png: Some(png),
            })
        }
        "get_texture" => {
            need_row(s)?;
            let which = args.get("which").and_then(Value::as_str).unwrap_or("shown");
            let tex = match which {
                "shown" => s.shown_texture(),
                "base" => s.base_texture(),
                other => return Err(format!("no texture named {other:?}")),
            }
            .ok_or("this gun has no main texture")?;
            let png = image_png(&tex)?;
            let written = write_if_asked(args, &png)?;
            let mut out = json!({"which": which, "width": tex.width, "height": tex.height, "written": written});
            if which == "base" {
                out["note"] = json!("From the player's own game. Reference only: never put it, or anything made from it, in a pack or a commit.");
            }
            Ok(Reply {
                json: out,
                png: Some(png),
            })
        }
        "describe_texture" => {
            need_row(s)?;
            let limit = args.get("parts").and_then(Value::as_u64).unwrap_or(24) as usize;
            let parts = s.texture_parts(limit);
            let tex = s.shown_texture().ok_or("this gun has no main texture")?;
            let keep = s
                .row()
                .map(|r| s.design.keep_of(r, s.view).to_vec())
                .unwrap_or_default();
            let mut px = tex.levels.first().ok_or("the texture is empty")?.clone();
            mark_texture(&mut px, tex.width, tex.height, &parts, &keep);
            let png = png_of(tex.width, tex.height, px)?;
            let written = write_if_asked(args, &png)?;
            let round = |v: f32| (v * 10.0).round() / 10.0;
            Ok(Reply {
                json: json!({
                    "gun": s.gun().map(|g| g.summary()),
                    "view": s.view,
                    "width": tex.width,
                    "height": tex.height,
                    "has_scope": s.gun().is_some_and(|g| g.has_scope),
                    "lenses_found": s.lenses().iter().map(|l| json!({"x": round(l.circle.x), "y": round(l.circle.y), "radius": round(l.circle.radius)})).collect::<Vec<_>>(),
                    "keep": keep.iter().map(|c| json!({"x": round(c.x), "y": round(c.y), "radius": round(c.radius)})).collect::<Vec<_>>(),
                    "parts": parts.iter().map(|p| json!({
                        "texels": p.texels.map(round),
                        "share_of_gun": (p.share * 1000.0).round() / 1000.0,
                        "place": p.place,
                    })).collect::<Vec<_>>(),
                    "written": written,
                    "note": "Coordinates are texels, x to the right and y down. A gun with no sight has no lenses found: it may still have glass worth keeping, which you can add with set_keep. Looking at the picture is how to be sure.",
                }),
                png: Some(png),
            })
        }
        "save_pack" => {
            let opts = SaveOptions {
                name: args
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&s.design.name)
                    .to_string(),
                author: args
                    .get("author")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                row: args.get("row").and_then(Value::as_u64).map(|v| v as u32),
                dir: args
                    .get("dir")
                    .and_then(Value::as_str)
                    .map(std::path::PathBuf::from),
            };
            let saved = s.save_pack(&opts)?;
            s.design.name = opts.name;
            Ok(json!({
                "dir": saved.dir,
                "id": saved.id,
                "row": saved.row,
                "warnings": saved.warnings,
            })
            .into())
        }
        "open_pack" => {
            let opened = if let Some(name) = args.get("example").and_then(Value::as_str) {
                pack::open_example(name)?
            } else if let Some(d) = args.get("dir").and_then(Value::as_str) {
                pack::open(std::path::Path::new(d))?
            } else {
                return Err("give dir or example".into());
            };
            let notes = apply_opened(s, opened)?;
            let mut out = status(s);
            out["notes"] = json!(notes);
            Ok(out.into())
        }
        other => Err(format!("no tool named {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_and_matches_the_rfc() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            assert_eq!(
                base64_decode(&base64_encode(s.as_bytes())).unwrap(),
                s.as_bytes()
            );
        }
        assert!(base64_decode("!!").is_err());
    }

    #[test]
    fn every_tool_has_an_object_schema_and_a_unique_name() {
        let mut names = std::collections::BTreeSet::new();
        for t in TOOLS {
            assert!(names.insert(t.name), "{}", t.name);
            assert_eq!((t.schema)()["type"], "object", "{}", t.name);
            assert!(!t.about.is_empty());
        }
    }

    #[test]
    fn art_needs_a_source_and_a_preset_takes_colours() {
        assert!(art_of(&json!({})).is_err());
        let a = art_of(
            &json!({"preset": "urban", "colours": ["010203", "040506", "070809", "0a0b0c"]}),
        )
        .unwrap();
        assert!(a.rgba.as_chunks::<4>().0.iter().all(|p| p[0] <= 10));
        assert!(art_of(&json!({"preset": "urban", "colours": ["nope"]})).is_err());
    }

    #[test]
    fn a_marked_texture_outlines_parts_and_circles_and_stays_inside() {
        let mut px = vec![0u8; 16 * 16 * 4];
        let part = crate::studio::TexturePart {
            texels: [2.0, 2.0, 40.0, 5.0],
            share: 1.0,
            place: String::new(),
        };
        let lens = Circle {
            x: 8.0,
            y: 10.0,
            radius: 3.0,
        };
        mark_texture(&mut px, 16, 16, &[part], &[lens]);
        assert_eq!(px[(2 * 16 + 3) * 4], 255, "the part's edge");
        assert_eq!(px[(10 * 16 + 11) * 4 + 1], 230, "the circle's edge");
        assert_eq!(px[(8 * 16 + 8) * 4], 0, "inside stays");
    }

    #[test]
    fn art_from_inline_png() {
        let png = png_of(2, 2, [9, 8, 7, 255].repeat(4)).unwrap();
        let a = art_of(&json!({"png_base64": base64_encode(&png)})).unwrap();
        assert_eq!((a.width, a.rgba[0]), (2, 9));
    }
}

//! The verbs (MAPKIT_PLAN.md 5.2). Every verb takes `--json` and exits 0 (ok), 1 (error diagnostics) or 2
//! (usage). Stdout carries only the verb's output (text or one JSON object); everything else goes to stderr, so
//! `mcp` can route the same functions. [`run_cli`] takes the argument list and the two streams, which the smoke
//! test drives in-process.
//!
//! JSON shape: `{"ok": bool, "verb": name, "result": <verb output>, "diagnostics": [Diagnostic]}`.
//!
//! Implemented: `new`, `info`, `validate`, `schema`, `get`, `apply`, `terrain`, `snapshot`, `history`, `revert`,
//! `build`, `export`, `render`, `rom`, `run`, `mcp`, `gui` (spawns the editor binary, [`crate::gui`]) and `play`
//! (vanilla++ on this machine). No verb is a stub.

use crate::diag::{Code, Diagnostic, Diagnostics, Severity, EXIT_USAGE};
use crate::ops::{self, Falloff, Op, Preset};
use crate::project::{write_atomic, Project, ASSETS_DIR, MAP_FILE, TERRAIN_FILE};
use crate::template::{self, Template};
use crate::{build, export, history, mcp, play, render_sw, romlist, script, validate};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A verb's answer: the JSON result, its text form and any non-fatal diagnostics.
pub struct Outcome {
    pub result: Value,
    pub text: String,
    pub diagnostics: Diagnostics,
}

impl Outcome {
    pub(crate) fn new(result: Value, text: impl Into<String>) -> Outcome {
        Outcome {
            result,
            text: text.into(),
            diagnostics: Diagnostics::new(),
        }
    }
}

fn dir_arg() -> Arg {
    Arg::new("dir")
        .required(true)
        .value_name("DIR")
        .help("the project folder")
}

fn opt(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).value_name(name).help(help)
}

fn f32_opt(name: &'static str, help: &'static str) -> Arg {
    opt(name, help)
        .value_parser(clap::value_parser!(f32))
        .allow_hyphen_values(true)
}

/// The clap command tree.
pub fn command() -> Command {
    Command::new("dc-mapkit")
        .about("DeCombat community map builder (docs/MAPKIT_PLAN.md)")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("print one JSON object"),
        )
        .subcommand(
            Command::new("new")
                .about("scaffold a project from a template")
                .arg(dir_arg())
                .arg(opt("template", "starter | flat | arena (default starter)"))
                .arg(opt("name", "map name (default: the folder name)"))
                .arg(opt("author", "author for the mod metadata"))
                .arg(
                    opt("seed", "terrain seed (default 1)").value_parser(clap::value_parser!(u64)),
                ),
        )
        .subcommand(
            Command::new("info")
                .about("counts, modes, spawn balance, size estimate")
                .arg(dir_arg())
                .arg(opt("rom", "ROM folder")),
        )
        .subcommand(
            Command::new("validate")
                .about("project checks (no .dat written); --world builds in memory and runs the world-level checks too")
                .arg(dir_arg())
                .arg(
                    Arg::new("world")
                        .long("world")
                        .action(ArgAction::SetTrue)
                        .help("also build the world in memory and run the world-level checks (nothing written)"),
                ),
        )
        .subcommand(
            Command::new("schema")
                .about("the project format and ops reference")
                .arg(
                    Arg::new("ops")
                        .long("ops")
                        .action(ArgAction::SetTrue)
                        .help("ops only"),
                ),
        )
        .subcommand(
            Command::new("get")
                .about("query objects")
                .arg(dir_arg())
                .arg(opt("class", "only this class"))
                .arg(opt("id", "one id (object, solid, prefab or import)"))
                .arg(opt("near", "x,z,r: within r of (x, z)")),
        )
        .subcommand(
            Command::new("apply")
                .about("apply a JSON list of ops (file or - for stdin)")
                .arg(dir_arg())
                .arg(
                    Arg::new("ops")
                        .required(true)
                        .value_name("OPS")
                        .help("ops file or -"),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("validate, write nothing"),
                ),
        )
        .subcommand(
            Command::new("terrain")
                .about("one terrain op with flags")
                .arg(dir_arg())
                .arg(
                    Arg::new("op")
                        .required(true)
                        .value_name("OP")
                        .help("gen | raise | lower | smooth | flatten"),
                )
                .arg(opt("preset", "gen: hills | valley | ridge | flat (default hills)"))
                .arg(opt("seed", "gen: seed").value_parser(clap::value_parser!(u64)))
                .arg(f32_opt("amplitude", "gen: height range"))
                .arg(f32_opt("base", "gen: mean height"))
                .arg(f32_opt("scale", "gen: feature size"))
                .arg(opt("octaves", "gen: noise layers").value_parser(clap::value_parser!(u32)))
                .arg(
                    opt("cells", "gen: cells per side for a new terrain")
                        .value_parser(clap::value_parser!(u32)),
                )
                .arg(f32_opt("cell-size", "gen: cell size for a new terrain"))
                .arg(f32_opt("x", "brush centre x"))
                .arg(f32_opt("z", "brush centre z"))
                .arg(f32_opt("radius", "brush radius"))
                .arg(f32_opt("amount", "raise / lower: world units"))
                .arg(f32_opt("height", "flatten: target height"))
                .arg(opt("falloff", "smooth | linear | flat"))
                .arg(opt("passes", "smooth: passes").value_parser(clap::value_parser!(u32)))
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("snapshot")
                .about("save a history entry")
                .arg(dir_arg())
                .arg(opt("label", "a label")),
        )
        .subcommand(
            Command::new("history")
                .about("list history entries")
                .arg(dir_arg()),
        )
        .subcommand(
            Command::new("revert")
                .about("restore a history entry")
                .arg(dir_arg())
                .arg(
                    opt("to", "entry number")
                        .required(true)
                        .value_parser(clap::value_parser!(u32)),
                ),
        )
        .subcommand(
            Command::new("build")
                .about("write build/<key>.dat, parse it back and run the world-level checks")
                .arg(dir_arg()),
        )
        .subcommand(
            Command::new("asset")
                .about("convert a glTF model (.glb) into the project: simplified meshes and small textures, placed with asset.place")
                .arg(dir_arg())
                .arg(Arg::new("file").required(true).value_name("FILE").help("the .glb file"))
                .arg(opt("name", "the name asset.place uses (letters, digits, underscore; default: the file's)"))
                .arg(opt("max-tris", "triangles for the whole model (default 800)"))
                .arg(opt("tex", "largest texture side (default 256)"))
                .arg(opt("orient", "turn it first, like x90 or z-90,y45"))
                .arg(opt("license", "the work's license: CC-BY-4.0 or CC0-1.0 (required unless --own)"))
                .arg(opt("author", "who made the work (required unless --own)"))
                .arg(opt("source", "where it came from, a URL (required unless --own)"))
                .arg(opt("title", "the work's title (default: the file's name)"))
                .arg(opt("keep", "only these material numbers, like 0,3 (default: all of them)"))
                .arg(Arg::new("strict").long("strict").action(ArgAction::SetTrue).help("allow more shape error to get near --max-tris; seams always hold"))
                .arg(opt("proxy", "how to rebuild the model: rebake (its shape, simplified, with one texture drawn from the source), box or round (a simple solid with the look projected on it)"))
                .arg(opt("width", "for a PNG: the width on the wall in metres; it becomes a flat sheet whose holes stay holes (default 3)"))
                .arg(opt("leave-out", "parts to leave out, as words of their material names with commas: for a wire mesh, which a bake turns into a solid sheet"))
                .arg(opt("relief", "draw a texture's normal map into its colour, for walls and floors: 0.3 is a good start"))
                .arg(Arg::new("textures-only").long("textures-only").action(ArgAction::SetTrue).help("take the textures as materials <name>_<n> for floors and walls, no model"))
                .arg(Arg::new("own").long("own").action(ArgAction::SetTrue).help("you made it yourself: no credit needed")),
        )
        .subcommand(
            Command::new("export")
                .about("build and write the mod folder build/mod/<id>/ (packed and hashed; --zip writes the zip)")
                .arg(dir_arg())
                .arg(
                    Arg::new("zip")
                        .long("zip")
                        .action(ArgAction::SetTrue)
                        .help("also write build/<id>.zip"),
                )
                .arg(
                    Arg::new("no-rom-paths")
                        .long("no-rom-paths")
                        .action(ArgAction::SetTrue)
                        .help("refuse a project with a ROM texture path"),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("pack the world even when the world-level checks reported errors"),
                ),
        )
        .subcommand(
            Command::new("render")
                .about("render a PNG of the project: flat colours without a GPU, or --textured with the game's renderer")
                .arg(dir_arg())
                .arg(
                    Arg::new("textured")
                        .long("textured")
                        .action(ArgAction::SetTrue)
                        .help("draw it with the game's renderer and the real textures (needs dc-client and a GPU; writes build/render/<view>_textured.png)"),
                )
                .arg(opt("rom", "--textured: ROM folder"))
                .arg(opt("binaries", "--textured: the folder holding dc-client (default: next to dc-mapkit)"))
                .arg(opt(
                    "view",
                    "top | iso | spawn:<n> | free:<x,y,z,yaw,pitch> (default top)",
                ))
                .arg(opt("size", "WxH (default 1024x768)"))
                .arg(
                    Arg::new("out")
                        .short('o')
                        .long("out")
                        .value_name("OUT")
                        .help("PNG path (default build/render/<view>.png)"),
                )
                .arg(
                    Arg::new("labels")
                        .long("labels")
                        .action(ArgAction::SetTrue)
                        .help("draw object markers (discs by class; ids and legend in --json)"),
                )
                .arg(
                    Arg::new("grid")
                        .long("grid")
                        .action(ArgAction::SetTrue)
                        .help("draw the world grid (top / iso)"),
                )
                .arg(f32_opt("grid-step", "grid spacing in world units (default 512)")),
        )
        .subcommand(
            Command::new("pictures")
                .about("draw the map's preview and loading pictures with the game's renderer (export packs them)")
                .arg(dir_arg())
                .arg(opt("preview-view", "the preview's view: spawn:<n> | free:<x,y,z,yaw,pitch> | top | iso (default spawn:0)"))
                .arg(opt("loading-view", "the loading picture's view (default: the preview's)"))
                .arg(opt("rom", "ROM folder"))
                .arg(opt("binaries", "the folder holding dc-client (default: next to dc-mapkit)")),
        )
        .subcommand(
            Command::new("rom")
                .about("ROM asset listings (run time only; --thumbs DIR writes PNG thumbnails)")
                .arg(
                    Arg::new("kind")
                        .required(true)
                        .value_name("KIND")
                        .help("textures | sounds | sprites | surfaces | worlds | modes"),
                )
                .arg(opt("filter", "substring"))
                .arg(opt("thumbs", "thumbnail folder"))
                .arg(opt("rom", "ROM folder")),
        )
        .subcommand(
            Command::new("run")
                .about("run a Lua generator in the mod sandbox (map.* API) and apply its ops as one batch")
                .arg(dir_arg())
                .arg(
                    Arg::new("script")
                        .required(true)
                        .value_name("SCRIPT")
                        .help("script file or - for stdin"),
                )
                .arg(
                    opt("budget", "Lua instruction budget (default 1e9)")
                        .value_parser(clap::value_parser!(u64)),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help("run and check, write nothing"),
                ),
        )
        .subcommand(
            Command::new("play")
                .about("Vanilla++ on this machine: export, then a vanilla lobby whose featured entry carries the map + game server + client")
                .arg(dir_arg())
                .arg(opt("rom", "ROM folder"))
                .arg(opt(
                    "binaries",
                    "folder holding dc-lobby-server, dc-dserver and dc-client (default: next to dc-mapkit, PATH, target/{debug,release})",
                ))
                .arg(
                    Arg::new("headless")
                        .long("headless")
                        .action(ArgAction::SetTrue)
                        .help("no window: the menu pilot picks the entry, creates a room on the map, starts it alone and plays --seconds"),
                )
                .arg(
                    opt("seconds", "headless: the match length (default 20); window: stop the session after this long")
                        .value_parser(clap::value_parser!(u64)),
                )
                .arg(
                    Arg::new("keep-profile")
                        .long("keep-profile")
                        .action(ArgAction::SetTrue)
                        .help("keep the previous run's accounts and client profile under <profile>/mapkit/play/"),
                )
                .arg(opt(
                    "loadout",
                    "what a new character starts with, slot:item,... (0:25 the G36E as the primary)",
                ))
                .arg(opt(
                    "inventory",
                    "items a new character owns in the inventory, not equipped: item,... (27 the L96A1)",
                ))
                .arg(
                    Arg::new("with-mod")
                        .long("with-mod")
                        .action(ArgAction::Append)
                        .help("a mod folder to play the map with, in the same set; repeat for more"),
                )
                .arg(
                    Arg::new("modded")
                        .long("modded")
                        .action(ArgAction::SetTrue)
                        .help("a modded lobby with the zip as its lobby-wide set, instead of the Vanilla++ entry"),
                ),
        )
        .subcommand(
            Command::new("mcp").about("MCP server over stdio (JSON-RPC 2.0; the verbs as tools)"),
        )
        .subcommand(
            Command::new("gui")
                .about("start the editor window (the `dc-mapkit-gui` binary next to this tool) and return")
                .arg(Arg::new("dir").value_name("DIR")),
        )
}

/// Run the CLI; returns the exit code. `out` gets the verb's output, `err` everything else. The `mcp` verb reads
/// the process's stdin (locked only for that verb, so an MCP tool running a verb in-process never re-locks it).
pub fn run_cli(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    run_cli_inner(args, None, out, err)
}

/// [`run_cli`] with the stream the `mcp` verb serves from (tests script a session without the process's stdin).
pub fn run_cli_with_stdin(
    args: &[String],
    stdin: &mut dyn std::io::BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    run_cli_inner(args, Some(stdin), out, err)
}

fn run_cli_inner(
    args: &[String],
    stdin: Option<&mut dyn std::io::BufRead>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let matches = match command().try_get_matches_from(args) {
        Ok(m) => m,
        Err(e) => {
            use clap::error::ErrorKind;
            let text = e.to_string();
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    let _ = out.write_all(text.as_bytes());
                    0
                }
                _ => {
                    let _ = err.write_all(text.as_bytes());
                    EXIT_USAGE
                }
            };
        }
    };
    let json = matches.get_flag("json");
    let (verb, sub) = matches.subcommand().expect("subcommand required");
    if verb == "mcp" {
        // DC-DEFAULT: MAP-58: the server owns stdout for the session (one JSON object per line, nothing else:
        // no envelope or text after it, `--json` or not); logging and stream failures go to stderr.
        return match stdin {
            Some(input) => mcp::serve_verb(input, out, err),
            None => {
                let stdin = std::io::stdin();
                mcp::serve_verb(&mut stdin.lock(), out, err)
            }
        };
    }
    let result = dispatch(verb, sub, err);
    emit(verb, json, result, out)
}

fn emit(verb: &str, json: bool, result: Result<Outcome, Diagnostics>, out: &mut dyn Write) -> i32 {
    match result {
        Ok(o) => {
            if json {
                let v = json!({"ok": true, "verb": verb, "result": o.result, "diagnostics": o.diagnostics});
                let _ = writeln!(out, "{}", serde_json::to_string(&v).expect("json"));
            } else {
                let _ = writeln!(out, "{}", o.text.trim_end());
                for d in o.diagnostics.iter() {
                    let _ = writeln!(out, "{d}");
                }
            }
            o.diagnostics.exit_code()
        }
        Err(d) => {
            if json {
                let v = json!({"ok": false, "verb": verb, "result": null, "diagnostics": d});
                let _ = writeln!(out, "{}", serde_json::to_string(&v).expect("json"));
            } else {
                for x in d.iter() {
                    let _ = writeln!(out, "{x}");
                }
            }
            d.exit_code().max(1)
        }
    }
}

fn dir_of(m: &ArgMatches) -> PathBuf {
    PathBuf::from(
        m.get_one::<String>("dir")
            .map(String::as_str)
            .unwrap_or("."),
    )
}

fn dispatch(verb: &str, m: &ArgMatches, err: &mut dyn Write) -> Result<Outcome, Diagnostics> {
    match verb {
        "new" => verb_new(m),
        "info" => verb_info(m),
        "validate" => verb_validate(m),
        "schema" => {
            let v = ops::schema(m.get_flag("ops"));
            let text = serde_json::to_string_pretty(&v).expect("json");
            Ok(Outcome::new(v, text))
        }
        "get" => verb_get(m),
        "apply" => verb_apply(m, err),
        "terrain" => verb_terrain(m, err),
        "asset" => {
            let num = |k: &str, d: u32| {
                m.get_one::<String>(k)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(d)
            };
            let text = |k: &str| m.get_one::<String>(k).cloned();
            import_asset(
                &dir_of(m),
                &AssetArgs {
                    file: PathBuf::from(m.get_one::<String>("file").expect("required")),
                    name: text("name"),
                    max_tris: num("max-tris", 800) as usize,
                    tex: num("tex", 256),
                    orient: text("orient").unwrap_or_default(),
                    license: text("license"),
                    author: text("author"),
                    source: text("source"),
                    title: text("title"),
                    own: m.get_flag("own"),
                    textures_only: m.get_flag("textures-only"),
                    strict: m.get_flag("strict"),
                    proxy: text("proxy"),
                    leave_out: text("leave-out"),
                    width: text("width").and_then(|v| v.parse().ok()).unwrap_or(3.0),
                    relief: text("relief").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                    keep: text("keep")
                        .map(|t| t.split(',').filter_map(|k| k.trim().parse().ok()).collect()),
                },
                err,
            )
        }
        "snapshot" => {
            let dir = dir_of(m);
            let label = m
                .get_one::<String>("label")
                .cloned()
                .unwrap_or_else(|| "snapshot".into());
            let e = history::snapshot(&dir, &label)?;
            let text = format!("snapshot {} ({})", e.n, e.label);
            Ok(Outcome::new(serde_json::to_value(e).expect("json"), text))
        }
        "history" => {
            let entries = history::list(&dir_of(m))?;
            let text = if entries.is_empty() {
                "no history".to_string()
            } else {
                entries
                    .iter()
                    .map(|e| format!("{:>4}  {}  {}", e.n, e.time, e.label))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(Outcome::new(
                serde_json::to_value(entries).expect("json"),
                text,
            ))
        }
        "revert" => {
            let n = *m.get_one::<u32>("to").expect("required");
            let e = history::revert(&dir_of(m), n)?;
            let text = format!("reverted to {} ({})", e.n, e.label);
            Ok(Outcome::new(serde_json::to_value(e).expect("json"), text))
        }
        "build" => verb_build(m),
        "export" => {
            let dir = dir_of(m);
            let (p, h) = Project::open(&dir)?;
            let b = build::build(&p, &h, &build::Assets::new(&dir), &p.build)?;
            let e = export::export(
                &p,
                &b,
                &dir.join("build"),
                export::ExportOptions {
                    zip: m.get_flag("zip"),
                    no_rom_paths: m.get_flag("no-rom-paths"),
                    project_dir: Some(dir.clone()),
                    force: m.get_flag("force"),
                },
            )?;
            let text = format!(
                "exported {} ({} files, {} KiB{}){}",
                e.dir.display(),
                e.files.len(),
                e.bytes >> 10,
                e.sha256
                    .as_deref()
                    .map(|s| format!(", sha256 {s}"))
                    .unwrap_or_default(),
                e.zip
                    .as_ref()
                    .map(|z| format!("; zip {}", z.display()))
                    .unwrap_or_default()
            );
            let mut o = Outcome::new(
                json!({
                    "dir": e.dir.display().to_string(),
                    "zip": e.zip.as_ref().map(|z| z.display().to_string()),
                    "sha256": e.sha256,
                    "files": e.files,
                    "bytes": e.bytes,
                    "stats": b.stats,
                }),
                text,
            );
            o.diagnostics = b.diagnostics;
            o.diagnostics.extend(e.diagnostics);
            Ok(o)
        }
        "render" => {
            let dir = dir_of(m);
            let (p, h) = Project::open(&dir)?;
            let view = render_sw::View::parse(
                m.get_one::<String>("view")
                    .map(String::as_str)
                    .unwrap_or("top"),
            )
            .map_err(|e| Diagnostics::one(Diagnostic::new(Code::OpInvalid, "--view", e)))?;
            let size = match m.get_one::<String>("size") {
                Some(s) => render_sw::parse_size(s)
                    .map_err(|e| Diagnostics::one(Diagnostic::new(Code::OpInvalid, "--size", e)))?,
                None => (1024, 768),
            };
            if m.get_flag("textured") {
                return crate::render_gpu::draw_textured(&crate::render_gpu::TexturedRequest {
                    dir: &dir,
                    view,
                    size,
                    out: m.get_one::<String>("out").map(PathBuf::from),
                    binaries: m.get_one::<String>("binaries").map(Path::new),
                    rom: m.get_one::<String>("rom").map(Path::new),
                })
                .map(|t| t.outcome);
            }
            let out = m
                .get_one::<String>("out")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    dir.join("build")
                        .join("render")
                        .join(format!("{}.png", view.slug()))
                });
            let opts = render_sw::RenderOptions {
                labels: m.get_flag("labels"),
                grid: m.get_flag("grid"),
                grid_step: m
                    .get_one::<f32>("grid-step")
                    .copied()
                    .unwrap_or(render_sw::GRID_STEP),
                sun_dir: crate::geom::V3::from_array(p.settings.sun_dir),
                ..Default::default()
            };
            let level = build::level_of(&p, &h, &build::Assets::new(&dir))?;
            let mut scene = render_sw::Scene::from_level(&level, &opts);
            scene.add_project_objects(&p);
            let r = render_sw::render_report(&scene, view.clone(), size, &opts)?;
            write_atomic(&out, &r.image.png()).map_err(|e| {
                Diagnostics::one(Diagnostic::new(
                    Code::ProjectIo,
                    out.display().to_string(),
                    e.to_string(),
                ))
            })?;
            let text = format!(
                "rendered {} to {} ({}x{}, {} triangles{})",
                view.text(),
                out.display(),
                r.image.width,
                r.image.height,
                r.triangles,
                if r.markers.is_empty() {
                    String::new()
                } else {
                    format!(", {} markers", r.markers.len())
                }
            );
            Ok(Outcome::new(
                json!({
                    "view": view.text(),
                    "image": out.display().to_string(),
                    "width": r.image.width,
                    "height": r.image.height,
                    "triangles": r.triangles,
                    "legend": r.legend,
                    "markers": r.markers,
                    "grid_step": r.grid_step,
                    "top_bounds": r.top_bounds,
                }),
                text,
            ))
        }
        "pictures" => {
            let dir = dir_of(m);
            let parse = |flag: &str, text: &str| {
                render_sw::View::parse(text)
                    .map_err(|e| Diagnostics::one(Diagnostic::new(Code::OpInvalid, flag, e)))
            };
            let preview_text = m
                .get_one::<String>("preview-view")
                .map(String::as_str)
                .unwrap_or("spawn:0");
            let loading_text = m
                .get_one::<String>("loading-view")
                .map(String::as_str)
                .unwrap_or(preview_text);
            crate::render_gpu::pictures(&crate::render_gpu::PicturesRequest {
                dir: &dir,
                preview: parse("--preview-view", preview_text)?,
                loading: parse("--loading-view", loading_text)?,
                binaries: m.get_one::<String>("binaries").map(Path::new),
                rom: m.get_one::<String>("rom").map(Path::new),
            })
        }
        "rom" => {
            let kind = m.get_one::<String>("kind").expect("required");
            let Some(kind) = romlist::RomKind::parse(kind) else {
                return Err(Diagnostics::one(
                    Diagnostic::new(Code::OpInvalid, "kind", format!("unknown listing `{kind}`"))
                        .with_fix("textures | sounds | sprites | surfaces | worlds | modes"),
                ));
            };
            let flag = m.get_one::<String>("rom").map(PathBuf::from);
            let found = romlist::discover_rom(flag.as_deref());
            // `modes` is DeCombat's own vocabulary and needs no ROM
            let (rom, source) = match (found, kind) {
                (Some((r, s)), _) => (r, Some(s)),
                (None, romlist::RomKind::Modes) => (PathBuf::new(), None),
                (None, _) => return Err(romlist::no_rom()),
            };
            let q = romlist::RomQuery {
                kind,
                filter: m.get_one::<String>("filter").cloned(),
                thumbs: m.get_one::<String>("thumbs").map(PathBuf::from),
            };
            let rows = romlist::list(&rom, &q)?;
            let text = if rows.is_empty() {
                "no entries".to_string()
            } else {
                rows.iter()
                    .map(|r| {
                        let mut line = r.path.clone();
                        if let Some(id) = r.id {
                            line.push_str(&format!("  id {id}"));
                        }
                        if let Some(n) = &r.name {
                            line.push_str(&format!("  {n}"));
                        }
                        if let (Some(w), Some(h)) = (r.width, r.height) {
                            line.push_str(&format!("  {w}x{h}"));
                        }
                        if let Some(t) = &r.thumb {
                            line.push_str(&format!("  {t}"));
                        }
                        line
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(Outcome::new(
                json!({
                    "kind": kind.name(),
                    "rom": source.map(|s| json!({"path": rom.display().to_string(), "source": s})),
                    "count": rows.len(),
                    "thumbs": q.thumbs.as_ref().map(|t| t.display().to_string()),
                    "rows": rows,
                }),
                text,
            ))
        }
        "run" => {
            let dir = dir_of(m);
            let (p, h) = Project::open(&dir)?;
            let path = m.get_one::<String>("script").expect("required");
            let text = if path == "-" {
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s).map_err(|e| {
                    Diagnostics::one(Diagnostic::new(Code::ProjectIo, "stdin", e.to_string()))
                })?;
                s
            } else {
                std::fs::read_to_string(path).map_err(|e| {
                    Diagnostics::one(Diagnostic::new(
                        Code::ProjectIo,
                        path.clone(),
                        e.to_string(),
                    ))
                })?
            };
            let opts = script::RunOptions {
                budget: m
                    .get_one::<u64>("budget")
                    .copied()
                    .unwrap_or(script::RunOptions::default().budget),
            };
            run_script(&dir, &p, &h, &text, &opts, m.get_flag("dry-run"), err)
        }
        "play" => {
            let dir = dir_of(m);
            let opts = play::PlayOptions {
                rom: m.get_one::<String>("rom").map(PathBuf::from),
                binaries: m.get_one::<String>("binaries").map(PathBuf::from),
                headless: m.get_flag("headless"),
                seconds: m.get_one::<u64>("seconds").copied(),
                keep_profile: m.get_flag("keep-profile"),
                modded: m.get_flag("modded"),
                loadout: m.get_one::<String>("loadout").cloned(),
                inventory: m.get_one::<String>("inventory").cloned(),
                with_mods: m
                    .get_many::<String>("with-mod")
                    .map(|v| v.map(PathBuf::from).collect())
                    .unwrap_or_default(),
            };
            let r = play::play(&dir, &opts)?;
            let on = match &r.entry_name {
                Some(name) => format!(
                    "the entry {} \"{name}\" of the vanilla lobby",
                    r.entry.unwrap_or(0)
                ),
                None => "the modded lobby".to_string(),
            };
            let mut text = format!(
                "played {} on {on} 127.0.0.1:{} (admin {}, game port {}); map index {}; client exit {:?} (stopped by {}); logs in {}",
                r.map_key,
                r.lobby_port,
                r.admin_port,
                r.game_port,
                r.map_index.map_or("none".to_string(), |i| i.to_string()),
                r.client_exit,
                r.stopped_by,
                r.logs
                    .first()
                    .and_then(|l| l.parent())
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
            );
            if let Some(l) = &r.launch_line {
                text.push_str(&format!("\nlobby: {l}"));
            }
            for l in &r.match_lines {
                text.push_str(&format!("\nclient: {l}"));
            }
            if r.headless {
                text.push_str(&format!(
                    "\nclient world {:?}, textures missing {:?}, frames {:?}, spawn {:?}, match ended {}",
                    r.client_world, r.textures_missing, r.frames, r.spawn, r.match_ended
                ));
            }
            Ok(Outcome::new(serde_json::to_value(&r).expect("json"), text))
        }
        // `mcp` is served by `run_cli` before dispatch (no envelope after the session).
        "gui" => verb_gui(m, err),
        other => Err(Diagnostics::one(Diagnostic::new(
            Code::OpUnknownOp,
            other,
            "unknown verb",
        ))),
    }
}

/// Where the editor binary is: next to this executable (`exe_dir`), then on `path` (the `PATH` list), the
/// platform's executable suffix included. `None` when nowhere.
// DC-DEFAULT: MAP-66 (the console `gui` verb spawns the sibling `dc-mapkit-gui`, then one on PATH, and exits)
pub fn gui_binary(exe_dir: Option<&Path>, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let name = format!("{}{}", crate::gui::GUI_BINARY, std::env::consts::EXE_SUFFIX);
    if let Some(d) = exe_dir {
        let p = d.join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    path.into_iter()
        .flat_map(std::env::split_paths)
        .map(|d| d.join(&name))
        .find(|p| p.is_file())
}

/// `gui [<dir>]`: start the editor window as its own process and return (the console keeps stdout for
/// `--json`; on Windows the editor is a GUI-subsystem binary).
fn verb_gui(m: &ArgMatches, err: &mut dyn Write) -> Result<Outcome, Diagnostics> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let path = std::env::var_os("PATH");
    let Some(bin) = gui_binary(exe_dir.as_deref(), path.as_deref()) else {
        return Err(Diagnostics::one(
            Diagnostic::new(
                Code::GuiLaunch,
                "gui",
                format!(
                    "`{}` is not next to this tool{} or on PATH",
                    crate::gui::GUI_BINARY,
                    exe_dir
                        .as_ref()
                        .map(|d| format!(" ({})", d.display()))
                        .unwrap_or_default()
                ),
            )
            .with_fix("install the release package whole, or `cargo build -p dc-mapkit --bin dc-mapkit-gui`"),
        ));
    };
    let mut cmd = std::process::Command::new(&bin);
    if let Some(dir) = m.get_one::<String>("dir") {
        cmd.arg(dir);
    }
    match cmd.spawn() {
        Ok(child) => {
            let _ = writeln!(err, "started {} (pid {})", bin.display(), child.id());
            Ok(Outcome::new(
                json!({"binary": bin.display().to_string(), "pid": child.id()}),
                format!("started {}", bin.display()),
            ))
        }
        Err(e) => Err(Diagnostics::one(Diagnostic::new(
            Code::GuiLaunch,
            bin.display().to_string(),
            e.to_string(),
        ))),
    }
}

/// `new`: scaffold a project folder (map.json, terrain.r16, assets/, AGENTS.md, .gitignore).
fn verb_new(m: &ArgMatches) -> Result<Outcome, Diagnostics> {
    let dir = dir_of(m);
    let starter_name = m
        .get_one::<String>("template")
        .map(String::as_str)
        .unwrap_or("starter");
    let Some(t) = Template::parse(starter_name) else {
        return Err(Diagnostics::one(
            Diagnostic::new(
                Code::OpInvalid,
                "--template",
                format!("unknown template `{starter_name}`"),
            )
            .with_fix("starter | flat | arena"),
        ));
    };
    if dir.join(MAP_FILE).exists() {
        return Err(Diagnostics::one(
            Diagnostic::new(
                Code::ProjectIo,
                dir.display().to_string(),
                "already holds a map.json",
            )
            .with_fix("choose another folder"),
        ));
    }
    let name = m
        .get_one::<String>("name")
        .cloned()
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Untitled".into());
    let author = m.get_one::<String>("author").cloned().unwrap_or_default();
    let seed = m.get_one::<u64>("seed").copied().unwrap_or(1);
    let made = template::make(t, &name, &author, seed);
    write_project(&dir, &made, t)
}

/// Write a made project into `dir`; returns the file list.
pub fn write_project(
    dir: &Path,
    made: &template::Made,
    t: Template,
) -> Result<Outcome, Diagnostics> {
    let io = |path: &Path, e: std::io::Error| {
        Diagnostics::one(Diagnostic::new(
            Code::ProjectIo,
            path.display().to_string(),
            e.to_string(),
        ))
    };
    std::fs::create_dir_all(dir.join(ASSETS_DIR)).map_err(|e| io(dir, e))?;
    let mut files = vec![MAP_FILE.to_string(), TERRAIN_FILE.to_string()];
    made.project.save_all(dir, &made.heights)?;
    for f in &made.files {
        let path = Project::resolve(dir, &f.path);
        write_atomic(&path, &f.bytes).map_err(|e| io(&path, e))?;
        files.push(f.path.clone());
    }
    let agents = dir.join("AGENTS.md");
    write_atomic(&agents, template::agents_md(&made.project, t).as_bytes())
        .map_err(|e| io(&agents, e))?;
    files.push("AGENTS.md".into());
    let gi = dir.join(".gitignore");
    write_atomic(&gi, template::GITIGNORE.as_bytes()).map_err(|e| io(&gi, e))?;
    files.push(".gitignore".into());
    let text = format!(
        "created {} ({} template, key {}): {}",
        dir.display(),
        t.name(),
        made.project.key,
        files.join(", ")
    );
    Ok(Outcome::new(
        json!({"dir": dir.display().to_string(), "template": t.name(), "key": made.project.key, "files": files}),
        text,
    ))
}

fn verb_info(m: &ArgMatches) -> Result<Outcome, Diagnostics> {
    let dir = dir_of(m);
    let (p, h) = Project::open(&dir)?;
    let flag = m.get_one::<String>("rom").map(PathBuf::from);
    let rom = romlist::discover_rom(flag.as_deref());
    let i = validate::info(&dir, &p, &h, rom.as_ref().map(|(p, s)| (p.as_path(), *s)));
    let text = format!(
        "{} (key {}, format {}, seed {})\n\
terrain: {}\n\
objects: {} ({} solids, {} brushes, {} prefabs, {} imports, {} materials, ~{} polygons)\n\
spawns: team 1 {}, team 2 {}, any {}\n\
modes: {}\n\
extent: {}\n\
size estimate: {} KiB ({} own textures, {} ROM textures)\n\
validate: {} errors, {} warnings\n\
rom: {}",
        i.name,
        i.key,
        i.format,
        i.seed,
        i.terrain.as_ref().map_or("none".to_string(), |t| format!(
            "{} x {} cells of {} (y {}..{})",
            t.cells_x, t.cells_z, t.cell_size, t.min_y, t.max_y
        )),
        i.counts.objects,
        i.counts.solids,
        i.counts.brushes,
        i.counts.prefabs,
        i.counts.imports,
        i.counts.materials,
        i.counts.polys,
        i.spawns.team1,
        i.spawns.team2,
        i.spawns.any,
        i.modes.join(", "),
        i.extent.as_ref().map_or("none".to_string(), |e| format!(
            "{:?} .. {:?}",
            e.min, e.max
        )),
        i.size_estimate_bytes >> 10,
        i.own_textures,
        i.rom_textures,
        i.errors,
        i.warnings,
        i.rom
            .as_ref()
            .map_or("none".to_string(), |r| r.path.clone()),
    );
    Ok(Outcome::new(serde_json::to_value(i).expect("json"), text))
}

/// `build`: the world file to `build/<key>.dat` (written even when the world-level checks find errors, so the
/// file can be inspected; the exit code follows the diagnostics), stats and diagnostics.
fn verb_build(m: &ArgMatches) -> Result<Outcome, Diagnostics> {
    let dir = dir_of(m);
    let (p, h) = Project::open(&dir)?;
    let b = build::build(&p, &h, &build::Assets::new(&dir), &p.build)?;
    let out = dir
        .join(crate::project::BUILD_DIR)
        .join(format!("{}.dat", p.key));
    write_atomic(&out, &b.dat).map_err(|e| {
        Diagnostics::one(Diagnostic::new(
            Code::ProjectIo,
            out.display().to_string(),
            format!("write: {e}"),
        ))
    })?;
    let s = &b.stats;
    let text = format!(
        "built {} ({} bytes): {} polygons ({} sky faces), {} brushes, {} BSP nodes (depth {}), {} textures, \
{} vertices, {} triangles, {} sections; {} errors, {} warnings",
        out.display(),
        s.bytes,
        s.polys,
        s.sky_faces,
        s.brushes,
        s.nodes,
        s.depth,
        s.textures,
        s.vertices,
        s.triangles,
        s.sections,
        b.diagnostics.count(Severity::Error),
        b.diagnostics.count(Severity::Warning),
    );
    let mut o = Outcome::new(
        json!({"file": out.display().to_string(), "bytes": s.bytes, "stats": s}),
        text,
    );
    o.diagnostics = b.diagnostics;
    Ok(o)
}

fn verb_validate(m: &ArgMatches) -> Result<Outcome, Diagnostics> {
    let dir = dir_of(m);
    let (p, h) = Project::open(&dir)?;
    let d = if m.get_flag("world") {
        // the build runs the project checks first and refuses on their errors; its diagnostics carry both
        match build::build(&p, &h, &build::Assets::new(&dir), &p.build) {
            Ok(b) => b.diagnostics,
            Err(d) => d,
        }
    } else {
        validate::validate(&dir, &p, &h)
    };
    let errors = d.count(Severity::Error);
    let warnings = d.count(Severity::Warning);
    let text = if errors == 0 {
        format!("ok: 0 errors, {warnings} warnings")
    } else {
        format!("{errors} errors, {warnings} warnings")
    };
    let mut o = Outcome::new(json!({"errors": errors, "warnings": warnings}), text);
    o.diagnostics = d;
    Ok(o)
}

fn verb_get(m: &ArgMatches) -> Result<Outcome, Diagnostics> {
    let dir = dir_of(m);
    let p = Project::load(&dir)?;
    if let Some(id) = m.get_one::<String>("id") {
        let v = if let Some(o) = p.object(id) {
            json!({"kind": "object", "item": o})
        } else if let Some(s) = p.solid(id) {
            json!({"kind": "solid", "item": s})
        } else if let Some(x) = p.prefabs.iter().find(|x| x.id == *id) {
            json!({"kind": "prefab", "item": x})
        } else if let Some(x) = p.imports.iter().find(|x| x.id == *id) {
            json!({"kind": "import", "item": x})
        } else {
            return Err(Diagnostics::one(Diagnostic::new(
                Code::IdUnknown,
                "--id",
                format!("no `{id}`"),
            )));
        };
        let text = serde_json::to_string_pretty(&v).expect("json");
        return Ok(Outcome::new(v, text));
    }
    let class = m.get_one::<String>("class");
    let near = match m.get_one::<String>("near") {
        Some(s) => {
            let v: Result<Vec<f32>, _> = s.split(',').map(|t| t.trim().parse::<f32>()).collect();
            match v {
                Ok(v) if v.len() == 3 => Some((v[0], v[1], v[2])),
                _ => {
                    return Err(Diagnostics::one(Diagnostic::new(
                        Code::OpInvalid,
                        "--near",
                        "want x,z,r",
                    )))
                }
            }
        }
        None => None,
    };
    let objects: Vec<&crate::project::MapObject> = p
        .objects
        .iter()
        .filter(|o| class.is_none_or(|c| o.class == *c))
        .filter(|o| {
            near.is_none_or(|(x, z, r)| {
                let dx = o.pos[0] - x;
                let dz = o.pos[2] - z;
                dx * dx + dz * dz <= r * r
            })
        })
        .collect();
    let text = objects
        .iter()
        .map(|o| {
            let props: Vec<String> = o
                .props
                .iter()
                .map(|(k, val)| format!("{k}={}", serde_json::to_string(val).expect("json")))
                .collect();
            format!(
                "{:<20} {:<18} {:?} {}",
                o.id,
                o.class,
                o.pos,
                props.join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Outcome::new(
        json!({"objects": objects, "count": objects.len()}),
        if text.is_empty() {
            "no objects".into()
        } else {
            text
        },
    ))
}

/// Parse an ops document: a JSON array of ops or `{"ops": [...]}`.
pub fn parse_ops(text: &str) -> Result<Vec<Op>, Diagnostics> {
    let v: Value = serde_json::from_str(text).map_err(|e| {
        Diagnostics::one(Diagnostic::new(
            Code::OpInvalid,
            "ops",
            format!("not JSON: {e}"),
        ))
    })?;
    let list = match v {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("ops") {
            Some(Value::Array(a)) => a,
            _ => {
                return Err(Diagnostics::one(Diagnostic::new(
                    Code::OpInvalid,
                    "ops",
                    "want an array of ops or {\"ops\": [...]}",
                )))
            }
        },
        _ => {
            return Err(Diagnostics::one(Diagnostic::new(
                Code::OpInvalid,
                "ops",
                "want an array of ops",
            )))
        }
    };
    let mut ops = Vec::with_capacity(list.len());
    for (i, item) in list.into_iter().enumerate() {
        let name = item
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match serde_json::from_value::<Op>(item) {
            Ok(op) => ops.push(op),
            Err(e) => {
                let code = if Op::NAMES.contains(&name.as_str()) {
                    Code::OpInvalid
                } else {
                    Code::OpUnknownOp
                };
                return Err(Diagnostics::one(
                    Diagnostic::new(code, format!("ops[{i}]"), e.to_string())
                        .with_fix("see `dc-mapkit schema --ops`"),
                ));
            }
        }
    }
    Ok(ops)
}

/// Snapshot, apply and save a batch (or dry-run). Shared by `apply` and `terrain`.
/// What `asset` converts and how (MAP-104).
#[derive(Debug, Clone, Default)]
pub struct AssetArgs {
    pub file: PathBuf,
    pub name: Option<String>,
    pub max_tris: usize,
    pub tex: u32,
    pub orient: String,
    pub license: Option<String>,
    pub author: Option<String>,
    pub source: Option<String>,
    pub title: Option<String>,
    pub own: bool,
    pub textures_only: bool,
    pub strict: bool,
    pub proxy: Option<String>,
    pub leave_out: Option<String>,
    pub width: f32,
    pub relief: f32,
    pub keep: Option<Vec<usize>>,
}

/// `asset`: converts a `.glb` into the project and records it for `asset.place`, with its credit (MAP-104, MOD-28).
pub fn import_asset(
    dir: &Path,
    a: &AssetArgs,
    err: &mut dyn Write,
) -> Result<Outcome, Diagnostics> {
    let fail = |path: &str, msg: String| {
        Diagnostics::one(Diagnostic::new(Code::OpInvalid, path.to_string(), msg))
    };
    let stem = a
        .file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();
    let name = a.name.clone().unwrap_or_else(|| stem.clone());
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(fail(
            "--name",
            format!("`{name}`: letters, digits and underscores only"),
        ));
    }
    let credit = if a.own {
        None
    } else {
        let need = |v: &Option<String>, k: &str| {
            v.clone().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
                fail(
                    &format!("--{k}"),
                    format!("somebody else's model needs --{k}, or --own if you made it yourself"),
                )
            })
        };
        let license = need(&a.license, "license")?;
        if !dc_mod_lua::CREDIT_LICENSES.contains(&license.as_str()) {
            return Err(fail(
                "--license",
                format!(
                    "`{license}` is not one of {}",
                    dc_mod_lua::CREDIT_LICENSES.join(", ")
                ),
            ));
        }
        Some(crate::project::Credit {
            title: a.title.clone().unwrap_or_else(|| stem.clone()),
            author: need(&a.author, "author")?,
            license,
            source: need(&a.source, "source")?,
            changes: "converted to the game's format; geometry simplified and textures reduced"
                .into(),
        })
    };
    let bytes =
        std::fs::read(&a.file).map_err(|e| fail("file", format!("{}: {e}", a.file.display())))?;
    let is_picture = a
        .file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    let model = if is_picture {
        crate::glb::decal(&bytes, a.width)
    } else {
        crate::glb::convert(
            &bytes,
            &crate::glb::GlbOptions {
                textures_only: a.textures_only,
                strict: a.strict,
                relief: a.relief,
                leave_out: a.leave_out.clone().unwrap_or_default(),
                proxy: crate::glb::Proxy::parse(a.proxy.as_deref().unwrap_or(""))
                    .map_err(|e| fail("--proxy", e))?,
                max_tris: a.max_tris.max(8),
                tex: a.tex.clamp(4, 1024),
                orient: a.orient.clone(),
            },
        )
    }
    .map_err(|e| fail("file", format!("{}: {e}", a.file.display())))?;
    let (mut p, h) = Project::open(dir)?;
    let snap = history::snapshot(dir, &format!("asset {name}"))?;
    let _ = writeln!(err, "snapshot {} taken", snap.n);
    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).map_err(|e| fail("assets", e.to_string()))?;
    let mut record = crate::project::AssetRecord {
        parts: Vec::new(),
        size: model.size,
        tris: 0,
        footprint: model.footprint.clone(),
        hull_boxes: model.hull_boxes.clone(),
    };
    let mut ops_list = Vec::new();
    let mut tex_bytes = 0;
    let mut written: std::collections::BTreeMap<Vec<u8>, String> =
        std::collections::BTreeMap::new();
    for (k, part) in model.parts.iter().enumerate() {
        if a.keep.as_ref().is_some_and(|keep| !keep.contains(&k)) {
            continue;
        }
        let (obj, png, material) = (
            format!("assets/{name}_{k}.obj"),
            format!("assets/{name}_{k}.png"),
            format!("{name}_{k}"),
        );
        // the same picture as one the project already holds is that file (MAP-105)
        let same = p.materials.iter().find_map(|m| match &m.texture {
            crate::project::TextureRef::Own(rel) if rel.starts_with("assets/") => {
                let on_disk = std::fs::read(dir.join(rel)).ok()?;
                (on_disk == part.png).then(|| rel.clone())
            }
            _ => None,
        });
        let png = match same.or_else(|| written.get(&part.png).cloned()) {
            Some(rel) => rel,
            None => {
                std::fs::write(dir.join(&png), &part.png).map_err(|e| fail(&png, e.to_string()))?;
                tex_bytes += part.png.len();
                written.insert(part.png.clone(), png.clone());
                png
            }
        };
        if !a.textures_only {
            std::fs::write(dir.join(&obj), crate::glb::part_obj(part))
                .map_err(|e| fail(&obj, e.to_string()))?;
            record.tris += part.tris;
            record.parts.push(crate::project::AssetPart {
                file: obj,
                material: material.clone(),
            });
        }
        ops_list.push(Op::MaterialSet {
            id: material,
            texture: Some(crate::project::TextureRef::Own(png)),
            tile: None,
            surface_id: None,
            flags: None,
            fit: None,
        });
    }
    let mut h = h;
    ops::apply(&mut p, &mut h, &ops_list)?;
    let (tris, parts, size) = (record.tris, record.parts.len(), record.size);
    if !a.textures_only {
        p.assets.insert(name.clone(), record);
    }
    if let Some(c) = credit {
        p.mod_meta.credits.retain(|o| o.title != c.title);
        p.mod_meta.credits.push(c);
    }
    p.save_all(dir, &h)?;
    Ok(Outcome::new(
        serde_json::json!({
            "name": name, "parts": parts, "tris": tris, "source_tris": model.source_tris,
            "size_m": size, "png_bytes": tex_bytes,
        }),
        format!(
            "{name}: {} -> {tris} triangles in {parts} parts, {:.2} x {:.2} x {:.2} m",
            model.source_tris, size[0], size[1], size[2]
        ),
    ))
}

pub fn apply_to_project(
    dir: &Path,
    ops: &[Op],
    dry_run: bool,
    label: &str,
    err: &mut dyn Write,
) -> Result<Outcome, Diagnostics> {
    let (mut p, mut h) = Project::open(dir)?;
    let applied = ops::apply(&mut p, &mut h, ops)?;
    if !dry_run && !applied.changed.is_empty() {
        let snap = history::snapshot(dir, label)?;
        let _ = writeln!(err, "snapshot {} taken", snap.n);
        p.save_all(dir, &h)?;
    }
    let ids: Vec<String> = applied
        .applied
        .iter()
        .flat_map(|a| a.ids.iter().cloned())
        .collect();
    let text = format!(
        "{} {} ops{}; changed: {}{}",
        if dry_run { "checked" } else { "applied" },
        applied.applied.len(),
        if ids.is_empty() {
            String::new()
        } else {
            format!(" ({})", ids.join(", "))
        },
        if applied.changed.is_empty() {
            "nothing".to_string()
        } else {
            applied.changed.join(", ")
        },
        if dry_run {
            " (dry run, nothing written)"
        } else {
            ""
        }
    );
    let mut o = Outcome::new(
        json!({"applied": applied.applied, "changed": applied.changed, "dry_run": dry_run}),
        text,
    );
    o.diagnostics = applied.diagnostics;
    Ok(o)
}

/// Run a Lua generator against the opened project and apply its op batch (or dry-run). Shared by `run` and the
/// MCP `run` tool: the script's `print` lines go to `err` as `script: ...`.
pub fn run_script(
    dir: &Path,
    p: &Project,
    h: &crate::project::Heights,
    text: &str,
    opts: &script::RunOptions,
    dry_run: bool,
    err: &mut dyn Write,
) -> Result<Outcome, Diagnostics> {
    let g = script::generate(p, h, text, opts)?;
    for line in &g.prints {
        let _ = writeln!(err, "script: {line}");
    }
    let mut o = apply_to_project(dir, &g.ops, dry_run, "run", err)?;
    if let Value::Object(map) = &mut o.result {
        map.insert("ops".into(), json!(g.ops.len()));
        map.insert("prints".into(), json!(g.prints));
    }
    o.text = format!("script queued {} ops; {}", g.ops.len(), o.text);
    Ok(o)
}

fn verb_apply(m: &ArgMatches, err: &mut dyn Write) -> Result<Outcome, Diagnostics> {
    let dir = dir_of(m);
    let src = m.get_one::<String>("ops").expect("required");
    let text = if src == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| {
            Diagnostics::one(Diagnostic::new(Code::ProjectIo, "stdin", e.to_string()))
        })?;
        s
    } else {
        std::fs::read_to_string(src).map_err(|e| {
            Diagnostics::one(Diagnostic::new(Code::ProjectIo, src.clone(), e.to_string()))
        })?
    };
    let ops = parse_ops(&text)?;
    apply_to_project(&dir, &ops, m.get_flag("dry-run"), "apply", err)
}

fn verb_terrain(m: &ArgMatches, err: &mut dyn Write) -> Result<Outcome, Diagnostics> {
    let dir = dir_of(m);
    let op_name = m.get_one::<String>("op").expect("required").as_str();
    let f = |k: &str| m.get_one::<f32>(k).copied();
    let need = |k: &str| -> Result<f32, Diagnostics> {
        f(k).ok_or_else(|| {
            Diagnostics::one(Diagnostic::new(
                Code::OpInvalid,
                format!("--{k}"),
                format!("`{op_name}` needs --{k}"),
            ))
        })
    };
    let falloff = match m.get_one::<String>("falloff").map(String::as_str) {
        None | Some("smooth") => Falloff::Smooth,
        Some("linear") => Falloff::Linear,
        Some("flat") => Falloff::Flat,
        Some(other) => {
            return Err(Diagnostics::one(Diagnostic::new(
                Code::OpInvalid,
                "--falloff",
                format!("`{other}`"),
            )))
        }
    };
    let op = match op_name {
        "gen" => {
            let preset = match m.get_one::<String>("preset").map(String::as_str) {
                None | Some("hills") => Preset::Hills,
                Some("valley") => Preset::Valley,
                Some("ridge") => Preset::Ridge,
                Some("flat") => Preset::Flat,
                Some(other) => {
                    return Err(Diagnostics::one(Diagnostic::new(
                        Code::OpInvalid,
                        "--preset",
                        format!("`{other}`"),
                    )))
                }
            };
            Op::TerrainGen {
                preset,
                seed: m.get_one::<u64>("seed").copied(),
                amplitude: f("amplitude").unwrap_or(256.0),
                base: f("base").unwrap_or(0.0),
                scale: f("scale").unwrap_or(1024.0),
                octaves: m.get_one::<u32>("octaves").copied().unwrap_or(5),
                cells_x: m.get_one::<u32>("cells").copied(),
                cells_z: m.get_one::<u32>("cells").copied(),
                cell_size: f("cell-size"),
            }
        }
        "raise" => Op::TerrainRaise {
            x: need("x")?,
            z: need("z")?,
            radius: need("radius")?,
            amount: need("amount")?,
            falloff,
        },
        "lower" => Op::TerrainLower {
            x: need("x")?,
            z: need("z")?,
            radius: need("radius")?,
            amount: need("amount")?,
            falloff,
        },
        "flatten" => Op::TerrainFlatten {
            x: need("x")?,
            z: need("z")?,
            radius: need("radius")?,
            height: need("height")?,
            falloff,
        },
        "smooth" => Op::TerrainSmooth {
            x: f("x"),
            z: f("z"),
            radius: f("radius"),
            passes: m.get_one::<u32>("passes").copied().unwrap_or(1),
        },
        other => {
            return Err(Diagnostics::one(
                Diagnostic::new(Code::OpUnknownOp, "op", format!("`{other}`"))
                    .with_fix("gen | raise | lower | smooth | flatten"),
            ))
        }
    };
    apply_to_project(
        &dir,
        &[op],
        m.get_flag("dry-run"),
        &format!("terrain.{op_name}"),
        err,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> (i32, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let argv: Vec<String> = std::iter::once("dc-mapkit".to_string())
            .chain(args.iter().map(|s| s.to_string()))
            .collect();
        let code = run_cli(&argv, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn json(args: &[&str]) -> (i32, Value) {
        let (code, out, _) = run(args);
        let v: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"));
        (code, v)
    }

    #[test]
    fn smoke_new_info_apply_validate_history_revert_through_the_lib() {
        let dir = std::env::temp_dir().join(format!("dc-mapkit-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.to_str().unwrap();

        let (code, v) = json(&[
            "new",
            d,
            "--template",
            "arena",
            "--name",
            "Smoke Test",
            "--author",
            "me",
            "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["key"], "smoke_test");
        assert!(
            dir.join("AGENTS.md").exists()
                && dir.join(".gitignore").exists()
                && dir.join("assets/ground.png").exists()
        );
        let (code, _, _) = run(&["new", d]);
        assert_eq!(code, 1, "refuses an existing project");

        let (code, v) = json(&["info", d, "--json"]);
        assert_eq!(code, 0);
        assert_eq!(v["result"]["spawns"]["team1"], 8);
        assert_eq!(v["result"]["errors"], 0);
        let (code, text, _) = run(&["info", d]);
        assert_eq!(code, 0);
        assert!(text.contains("spawns: team 1 8"));

        let (code, v) = json(&["validate", d, "--json"]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["errors"], 0);

        let ops_file = dir.join("ops.json");
        std::fs::write(
            &ops_file,
            r#"[{"op":"add","class":"dc.item_spawn","pos":[100,0,100],"ground":true},
                {"op":"terrain.raise","x":0,"z":0,"radius":300,"amount":40}]"#,
        )
        .unwrap();
        let (code, v) = json(&[
            "apply",
            d,
            ops_file.to_str().unwrap(),
            "--dry-run",
            "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["dry_run"], true);
        assert!(
            history::list(&dir).unwrap().is_empty(),
            "a dry run takes no snapshot"
        );
        let (code, v) = json(&["apply", d, ops_file.to_str().unwrap(), "--json"]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["applied"][0]["op"], "add");
        let new_id = v["result"]["applied"][0]["ids"][0]
            .as_str()
            .unwrap()
            .to_string();
        assert!(new_id.starts_with("item_spawn_"));
        assert_eq!(v["result"]["changed"], json!(["map.json", "terrain.r16"]));
        let (code, v) = json(&["get", d, "--id", &new_id, "--json"]);
        assert_eq!(code, 0);
        assert_eq!(v["result"]["kind"], "object");
        let (code, v) = json(&["get", d, "--class", "dc.bomb_site", "--json"]);
        assert_eq!(code, 0);
        assert_eq!(v["result"]["count"], 2);
        let (code, v) = json(&["get", d, "--near", "0,-1024,10", "--json"]);
        assert_eq!(code, 0);
        assert_eq!(v["result"]["count"], 1);

        // a bad batch is refused whole: exit 1, nothing changed, no second snapshot
        std::fs::write(&ops_file, r#"[{"op":"remove","id":"item_spawn_999"}]"#).unwrap();
        let (code, v) = json(&["apply", d, ops_file.to_str().unwrap(), "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["ok"], false);
        assert_eq!(v["diagnostics"][0]["code"], "id.unknown");
        std::fs::write(&ops_file, r#"[{"op":"teleport"}]"#).unwrap();
        let (code, v) = json(&["apply", d, ops_file.to_str().unwrap(), "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "op.unknown");

        let (code, v) = json(&[
            "terrain", d, "lower", "--x", "0", "--z", "0", "--radius", "200", "--amount", "10",
            "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        let (code, v) = json(&["history", d, "--json"]);
        assert_eq!(code, 0);
        assert_eq!(v["result"].as_array().unwrap().len(), 2);
        let (code, v) = json(&["revert", d, "--to", "1", "--json"]);
        assert_eq!(code, 0, "{v}");
        let (code, v) = json(&["get", d, "--class", "dc.item_spawn", "--json"]);
        assert_eq!(code, 0);
        assert_eq!(
            v["result"]["count"], 4,
            "the added item spawn is gone after the revert"
        );
        let (code, v) = json(&["revert", d, "--to", "99", "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "history.missing");

        let (code, v) = json(&["schema", "--ops", "--json"]);
        assert_eq!(code, 0);
        assert!(v["result"]["ops"]["list"].is_array());
        let (code, out, _) = run(&["schema"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"structs\""));

        // build writes build/<key>.dat, prints stats and the world-level diagnostics
        let (code, v) = json(&["build", d, "--json"]);
        assert_eq!(code, 0, "{v}");
        assert!(v["result"]["stats"]["nodes"].as_u64().unwrap() > 0);
        let dat = std::path::Path::new(v["result"]["file"].as_str().unwrap()).to_path_buf();
        assert!(
            dat.is_file() && dat.ends_with("build/smoke_test.dat"),
            "{}",
            dat.display()
        );
        let (code, v) = json(&["validate", d, "--world", "--json"]);
        assert_eq!(code, 0, "{v}");

        // render: a PNG without a GPU, markers and legend in the JSON
        let out = dir.join("top.png");
        let (code, v) = json(&[
            "render",
            d,
            "--view",
            "top",
            "--size",
            "200x150",
            "--labels",
            "--grid",
            "-o",
            out.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(code, 0, "{v}");
        assert!(out.is_file());
        assert_eq!(v["result"]["width"], 200);
        assert!(v["result"]["markers"].as_array().unwrap().len() >= 36);
        assert!(v["result"]["legend"].as_array().unwrap().len() >= 4);
        assert!(v["result"]["top_bounds"].is_array());
        let (code, v) = json(&["render", d, "--view", "spawn:0", "--json"]);
        assert_eq!(code, 0, "{v}");
        assert!(dir.join("build/render/spawn_0.png").is_file());
        let (code, v) = json(&["render", d, "--view", "spawn:99", "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "render.view");
        let (code, _, _) = run(&["render", d, "--size", "big"]);
        assert_eq!(code, 1);

        // rom modes needs no ROM; the other listings name the attempts when none is found
        let (code, v) = json(&["rom", "modes", "--filter", "tdm", "--json"]);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["count"], 2);
        let (code, _, _) = run(&["rom", "nothing"]);
        assert_eq!(code, 1);
        let (code, v) = json(&[
            "rom",
            "worlds",
            "--rom",
            dir.join("no-rom").to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "rom.read");

        // the gui verb's binary lookup (the verb itself would open a window): sibling first, then PATH, else none
        let bin_dir = dir.join("bins");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let name = format!("{}{}", crate::gui::GUI_BINARY, std::env::consts::EXE_SUFFIX);
        assert_eq!(gui_binary(Some(&bin_dir), None), None);
        std::fs::write(bin_dir.join(&name), b"").unwrap();
        assert_eq!(gui_binary(Some(&bin_dir), None), Some(bin_dir.join(&name)));
        let path_env = std::env::join_paths([dir.join("nowhere"), bin_dir.clone()]).unwrap();
        assert_eq!(
            gui_binary(Some(&dir.join("nowhere")), Some(&path_env)),
            Some(bin_dir.join(&name))
        );
        // play: an explicit binary folder without the binaries stops before anything starts (MAP-67)
        let empty = dir.join("no-binaries");
        std::fs::create_dir_all(&empty).unwrap();
        let (code, v) = json(&["play", d, "--binaries", empty.to_str().unwrap(), "--json"]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "play.binaries", "{v}");
        assert_eq!(v["diagnostics"][0]["path"], "dc-lobby-server");
        // `--modded` is the same verb on the stage-3 shape (MAP-89)
        let (code, v) = json(&[
            "play",
            d,
            "--modded",
            "--binaries",
            empty.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(code, 1);
        assert_eq!(v["diagnostics"][0]["code"], "play.binaries", "{v}");
        let (code, _, err) = run(&["nothing"]);
        assert_eq!(code, 2);
        assert!(!err.is_empty());
        let (code, _, _) = run(&["validate"]);
        assert_eq!(code, 2, "missing dir is usage");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ops_documents_take_both_forms() {
        assert_eq!(parse_ops("[]").unwrap().len(), 0);
        assert_eq!(
            parse_ops(r#"{"ops":[{"op":"remove","id":"x_1"}]}"#)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(parse_ops("{}").unwrap_err().items[0].code, Code::OpInvalid);
        assert_eq!(
            parse_ops("nope").unwrap_err().items[0].code,
            Code::OpInvalid
        );
        assert_eq!(
            parse_ops(r#"[{"op":"remove"}]"#).unwrap_err().items[0].code,
            Code::OpInvalid
        );
    }
}

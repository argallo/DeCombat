//! `dc-mapkit mcp`: an MCP server over stdio (JSON-RPC 2.0, one JSON object per line) exposing the verbs as
//! tools (MAPKIT_PLAN.md section 7, D8). DC-DEFAULT: MAP-58 (protocol minimum and transport), MAP-59 (the tool
//! list and the project-path rule).
//!
//! Implemented with serde_json only. The minimum a scripted-client conformance test pins
//! ([`tests::a_scripted_client_runs_the_handshake_and_every_tool`]):
//!
//! - `initialize` -> `{protocolVersion, capabilities: {tools: {listChanged: false}, resources: {subscribe:
//!   false, listChanged: false}}, serverInfo, instructions}`; the client's `protocolVersion` is answered when it
//!   is one of [`SUPPORTED_VERSIONS`], else [`PROTOCOL_VERSION`];
//! - `notifications/initialized` and `notifications/cancelled` (no `id`) are accepted silently; no notification
//!   is ever answered;
//! - `ping` -> `{}`;
//! - `tools/list` -> `{tools: [{name, description, inputSchema}]}` with JSON Schema objects generated from the
//!   verb arguments and, for the typed twins, from the [`Op`] fields (`schema::OPS`);
//! - `tools/call` -> `{content: [{type: "text", text}], isError}`; a verb failure (exit 1 / 2 of the CLI path, a
//!   refused batch) is `isError: true` with the same JSON envelope the CLI prints, never a JSON-RPC error; an
//!   unknown tool name or malformed arguments object is `-32602`;
//! - `resources/list` / `resources/read` -> the playbook (`mapkit://playbook`), the schema (`mapkit://schema`)
//!   and the vocabulary (`mapkit://vocab`) as `{contents: [{uri, mimeType, text}]}`; an unknown uri is `-32002`;
//! - unknown method `-32601`; a line that is not JSON (or not UTF-8) `-32700`; a JSON value that is not a 2.0
//!   request object (a batch array included) `-32600`; no `prompts` capability.
//!
//! Transport: UTF-8 lines, one object per line, no embedded newlines; a line that is not UTF-8 is read lossily
//! and answered `-32700` like any other non-JSON line, so the session never dies on bad input. ALL logging goes
//! to stderr: the verbs run in-process through [`crate::cli::run_cli`] with their stdout captured into the tool
//! result and their stderr forwarded to this process's stderr, so nothing but responses reaches the client; the
//! `mcp` verb itself prints no envelope after the session (`--json` or not) and exits 0 when stdin closes. The
//! server needs neither a ROM nor a GPU: `render` uses the software path and answers in `isError` when it cannot.
//!
//! Protocol versions: 2025-06-18 (ours) and 2024-11-05 are answered as the client's own; a 2025-03-26 client (the
//! one revision that requires JSON-RPC batch support, which this one-object-per-line server does not offer) and
//! any unknown version are answered 2025-06-18.
//!
//! Tools (every one takes `project`, the project folder, absolute or relative to the server's working
//! directory; required except on `schema` and `rom_list`): `new`, `info`, `validate`, `build`, `export`,
//! `render` (the CLI's JSON answer: `view`, `image`, `width`, `height`, `triangles`, `legend`, `markers`,
//! `grid_step`, `top_bounds`, plus `out` when given, and the PNG as image content), `apply`, `get`, `terrain`,
//! `snapshot`, `history`, `revert`, `run` (inline script text), `rom_list`, `schema`, plus the flat typed twins
//! `add_object`, `move_object`, `set_property`, `remove_object`, `terrain_noise`.

use crate::cli::{self, Outcome};
use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::ops::{self, Op};
use crate::project::Project;
use crate::{build, render_sw, schema, script};
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// The protocol version answered when the client's is unsupported.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// The protocol versions answered as the client's own. 2025-03-26 is left out on purpose: that revision requires
/// JSON-RPC batches, which the one-object-per-line transport refuses with `-32600` (DC-DEFAULT: MAP-58).
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2024-11-05"];
/// `serverInfo.name`.
pub const SERVER_NAME: &str = "dc-mapkit";

/// The playbook resource (docs/MAPKIT_AGENTS.md, embedded so the server needs no checkout).
pub const PLAYBOOK_URI: &str = "mapkit://playbook";
/// The schema resource (`dc-mapkit schema`).
pub const SCHEMA_URI: &str = "mapkit://schema";
/// The vocabulary resource (docs/MAPKIT_VOCAB.md).
pub const VOCAB_URI: &str = "mapkit://vocab";
const PLAYBOOK: &str = include_str!("../../../docs/MAPKIT_AGENTS.md");
const VOCAB: &str = include_str!("../../../docs/MAPKIT_VOCAB.md");

/// JSON-RPC error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const RESOURCE_NOT_FOUND: i64 = -32002;

/// The typed twins: (tool name, op name). Their input schema is generated from the op's fields.
pub const TYPED_TWINS: &[(&str, &str)] = &[
    ("add_object", "add"),
    ("move_object", "move"),
    ("set_property", "set"),
    ("remove_object", "remove"),
    ("terrain_noise", "terrain.gen"),
];

/// The verb tools, in listing order.
pub const VERB_TOOLS: &[&str] = &[
    "new", "info", "validate", "build", "export", "render", "apply", "get", "terrain", "snapshot",
    "history", "revert", "run", "rom_list", "schema",
];

/// Serve one session: read requests from `input` until EOF, write responses to `output`. Nothing but response
/// lines is ever written to `output`: a line that is not UTF-8 is read lossily and answered `-32700` (MAP-58);
/// `Err` is a real read or write failure of the streams, which the caller reports on stderr.
pub fn serve(input: &mut dyn BufRead, output: &mut dyn Write) -> Result<(), Diagnostics> {
    let io = |what: &str, e: std::io::Error| {
        Diagnostics::one(Diagnostic::new(Code::ProjectIo, what, e.to_string()))
    };
    eprintln!(
        "dc-mapkit mcp: serving {} tools over stdio (protocol {PROTOCOL_VERSION}; logging on stderr)",
        VERB_TOOLS.len() + TYPED_TWINS.len()
    );
    let mut server = Server::default();
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if input
            .read_until(b'\n', &mut raw)
            .map_err(|e| io("stdin", e))?
            == 0
        {
            break;
        }
        let line = String::from_utf8_lossy(&raw);
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(resp) = server.handle_line(text) {
            let s = serde_json::to_string(&resp).expect("response serialises");
            writeln!(output, "{s}").map_err(|e| io("stdout", e))?;
            output.flush().map_err(|e| io("stdout", e))?;
        }
    }
    eprintln!(
        "dc-mapkit mcp: stdin closed, {} requests served",
        server.served
    );
    Ok(())
}

/// The `mcp` verb: serve the session and answer its exit code. The verb never goes through the CLI's envelope
/// printer: `output` carries response lines only, and a stream failure is reported on `err` with exit 1.
pub fn serve_verb(input: &mut dyn BufRead, output: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match serve(input, output) {
        Ok(()) => 0,
        Err(d) => {
            for x in d.iter() {
                let _ = writeln!(err, "dc-mapkit mcp: {x}");
            }
            1
        }
    }
}

/// Session state.
#[derive(Debug, Default)]
pub struct Server {
    /// The protocol version negotiated by `initialize` (none before it).
    pub protocol: Option<String>,
    /// Requests answered.
    pub served: usize,
}

fn error_response(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

impl Server {
    /// Handle one line; `None` for a notification (never answered).
    pub fn handle_line(&mut self, text: &str) -> Option<Value> {
        let v: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_response(
                    Value::Null,
                    PARSE_ERROR,
                    format!("parse error: {e}"),
                ))
            }
        };
        let Value::Object(req) = v else {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "a request is one JSON object per line",
            ));
        };
        let id = req.get("id").cloned();
        if req.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "jsonrpc must be \"2.0\"",
            ));
        }
        let Some(method) = req.get("method").and_then(Value::as_str) else {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "method must be a string",
            ));
        };
        let params = match req.get("params") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => {
                return id.map(|id| error_response(id, INVALID_PARAMS, "params must be an object"))
            }
        };
        let Some(id) = id else {
            // SPEC: JSON-RPC 2.0: a notification is never answered, whatever its method.
            if !matches!(
                method,
                "notifications/initialized" | "notifications/cancelled"
            ) {
                eprintln!("dc-mapkit mcp: ignoring notification `{method}`");
            }
            return None;
        };
        self.served += 1;
        Some(match self.dispatch(method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => error_response(id, code, message),
        })
    }

    fn dispatch(
        &mut self,
        method: &str,
        params: &Map<String, Value>,
    ) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                // DC-DEFAULT: MAP-58: the client's version when supported, else ours.
                let version = if SUPPORTED_VERSIONS.contains(&asked) {
                    asked.to_string()
                } else {
                    PROTOCOL_VERSION.to_string()
                };
                self.protocol = Some(version.clone());
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {
                        "tools": {"listChanged": false},
                        "resources": {"subscribe": false, "listChanged": false},
                    },
                    "serverInfo": {"name": SERVER_NAME, "version": crate::TOOL_VERSION},
                    "instructions": "dc-mapkit builds DeCombat community maps from a text project folder. Read the playbook resource (mapkit://playbook) first; the loop is info -> apply / run -> validate -> build -> render. Every tool takes `project` (the folder). Tool results are the CLI's JSON envelope {ok, verb, result, diagnostics}; isError means the verb failed (read `diagnostics`).",
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools().iter().map(|t| json!({
                "name": t.name, "description": t.description, "inputSchema": t.input_schema,
            })).collect::<Vec<_>>()})),
            "tools/call" => {
                let Some(name) = params.get("name").and_then(Value::as_str) else {
                    return Err((INVALID_PARAMS, "tools/call needs `name`".into()));
                };
                let args = match params.get("arguments") {
                    None | Some(Value::Null) => Map::new(),
                    Some(Value::Object(m)) => m.clone(),
                    Some(_) => {
                        return Err((INVALID_PARAMS, "`arguments` must be an object".into()))
                    }
                };
                let Some(tool) = tools().into_iter().find(|t| t.name == name) else {
                    return Err((INVALID_PARAMS, format!("unknown tool `{name}`")));
                };
                let r = call_tool(&tool, &args);
                Ok(json!({"content": r.content, "isError": r.is_error}))
            }
            "resources/list" => Ok(json!({"resources": resources()})),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
                let (mime, text) = match uri {
                    PLAYBOOK_URI => ("text/markdown", PLAYBOOK.to_string()),
                    VOCAB_URI => ("text/markdown", VOCAB.to_string()),
                    SCHEMA_URI => (
                        "application/json",
                        serde_json::to_string_pretty(&ops::schema(false)).expect("json"),
                    ),
                    _ => return Err((RESOURCE_NOT_FOUND, format!("no resource `{uri}`"))),
                };
                Ok(json!({"contents": [{"uri": uri, "mimeType": mime, "text": text}]}))
            }
            other => Err((METHOD_NOT_FOUND, format!("unknown method `{other}`"))),
        }
    }
}

fn resources() -> Vec<Value> {
    vec![
        json!({"uri": PLAYBOOK_URI, "name": "MAPKIT_AGENTS.md", "title": "dc-mapkit agent playbook",
               "description": "how to build a map with the tools: the loop, every verb, the ops, the Lua API, limits, troubleshooting",
               "mimeType": "text/markdown"}),
        json!({"uri": SCHEMA_URI, "name": "schema.json", "title": "dc-mapkit project and ops schema",
               "description": "the project format (map.json), every op and field, the classes, the modes, the diagnostic codes",
               "mimeType": "application/json"}),
        json!({"uri": VOCAB_URI, "name": "MAPKIT_VOCAB.md", "title": "the DeCombat map vocabulary",
               "description": "the dc.* classes and their properties as the game reads them",
               "mimeType": "text/markdown"}),
    ]
}

// ---------------------------------------------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------------------------------------------

/// One tool: its name, description and generated JSON Schema.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: &'static str,
    pub description: String,
    pub input_schema: Value,
    /// The op a typed twin builds (`None` for a verb tool).
    pub op: Option<&'static str>,
}

/// The result of a tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolResult {
    fn text(text: impl Into<String>, is_error: bool) -> ToolResult {
        ToolResult {
            content: vec![json!({"type": "text", "text": text.into()})],
            is_error,
        }
    }
}

fn prop(ty: &str, doc: &str) -> Value {
    json!({"type": ty, "description": doc})
}

fn vec3(doc: &str) -> Value {
    json!({"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": doc})
}

fn enum_of(values: &[&str], doc: &str) -> Value {
    json!({"type": "string", "enum": values, "description": doc})
}

fn object_schema(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut m = Map::new();
    for (k, v) in props {
        m.insert(k.to_string(), v);
    }
    json!({"type": "object", "properties": m, "required": required, "additionalProperties": false})
}

const PROJECT_DOC: &str =
    "the project folder (absolute, or relative to the server's working directory)";

fn project_prop() -> (&'static str, Value) {
    ("project", prop("string", PROJECT_DOC))
}

fn dry_run_prop() -> (&'static str, Value) {
    (
        "dry_run",
        prop(
            "boolean",
            "check the batch and write nothing (default false)",
        ),
    )
}

/// JSON Schema of one documented op field from its type text (`schema::OPS`): `x|null` is optional.
fn field_schema(ty: &str, doc: &str) -> Value {
    let base = ty.strip_suffix("|null").unwrap_or(ty);
    match base {
        "text" | "path" => prop("string", doc),
        "num" => prop("number", doc),
        "int" => prop("integer", doc),
        "bool" => prop("boolean", doc),
        "vec3" => vec3(doc),
        "rgb" => {
            json!({"type": "array", "items": {"type": "integer"}, "minItems": 3, "maxItems": 3, "description": doc})
        }
        "value" => {
            json!({"description": format!("{doc}: text, a number, a bool, [x, y, z] or {{\"rgb\": [r, g, b]}}")})
        }
        "MaterialFlags" => object_schema(
            vec![
                ("solid", prop("boolean", "blocks movement and shots")),
                ("no_step", prop("boolean", "no footstep sounds")),
                ("sky", prop("boolean", "a sky face")),
            ],
            &[],
        ),
        "role" => enum_of(
            &["world_model", "kill_volume", "ladder", "breakable", "sky"],
            doc,
        ),
        b if b.starts_with('"') => {
            let alts: Vec<&str> = b.split('|').map(str::trim).collect();
            if alts
                .iter()
                .all(|a| a.starts_with('"') && a.ends_with('"') && a.len() >= 2)
            {
                let values: Vec<&str> = alts.iter().map(|a| &a[1..a.len() - 1]).collect();
                enum_of(&values, doc)
            } else {
                json!({"description": format!("{doc} ({ty})")})
            }
        }
        b if b.starts_with('{') => {
            json!({"type": "object", "description": format!("{doc} ({base})")})
        }
        _ => json!({"description": format!("{doc} ({ty})")}),
    }
}

/// The fields of an op without a serde default: the example op refuses to deserialise without them.
fn required_fields(op: &str) -> Vec<String> {
    let Some(example) = ops::example_ops().into_iter().find(|o| o.name() == op) else {
        return Vec::new();
    };
    let Value::Object(full) = serde_json::to_value(&example).expect("op serialises") else {
        return Vec::new();
    };
    let mut required = Vec::new();
    for key in full.keys().filter(|k| *k != "op") {
        let mut cut = full.clone();
        cut.remove(key);
        if serde_json::from_value::<Op>(Value::Object(cut)).is_err() {
            required.push(key.clone());
        }
    }
    required
}

/// The input schema of a typed twin: `project`, the op's fields, `dry_run`.
fn twin_schema(op: &str) -> Value {
    let mut props = vec![project_prop()];
    let mut required = vec!["project".to_string()];
    if let Some(fields) = schema::op_fields(op) {
        for (name, ty, doc) in fields {
            props.push((name, field_schema(ty, doc)));
        }
    }
    required.extend(required_fields(op));
    props.push(dry_run_prop());
    let req: Vec<&str> = required.iter().map(String::as_str).collect();
    object_schema(props, &req)
}

/// Every tool, in listing order.
pub fn tools() -> Vec<Tool> {
    let verb = |name: &'static str,
                description: &str,
                props: Vec<(&str, Value)>,
                required: &[&str]| Tool {
        name,
        description: description.to_string(),
        input_schema: object_schema(props, required),
        op: None,
    };
    let p = project_prop;
    let mut out = vec![
        verb(
            "new",
            "Scaffold a map project folder from a template (starter | flat | arena): map.json, terrain.r16, own PNG textures under assets/, AGENTS.md.",
            vec![
                p(),
                ("template", enum_of(&["starter", "flat", "arena"], "the template (default starter)")),
                ("name", prop("string", "the map name (default: the folder name)")),
                ("author", prop("string", "author for the mod metadata")),
                ("seed", prop("integer", "terrain seed (default 1)")),
            ],
            &["project"],
        ),
        verb(
            "info",
            "Counts, objects by class, spawns per team, modes, extent, size estimate, error / warning counts and the ROM in use.",
            vec![p(), ("rom", prop("string", "ROM folder (else CA_ROM)"))],
            &["project"],
        ),
        verb(
            "validate",
            "Project checks (offline, writes nothing): every diagnostic with code, severity, path, message and fix hint. isError when there is an error diagnostic.",
            vec![p()],
            &["project"],
        ),
        verb(
            "build",
            "Build build/<key>.dat (the version-85 world) and run the world-level sweeps.",
            vec![p()],
            &["project"],
        ),
        verb(
            "export",
            "Write the mod folder under build/ (manifest, main.lua with the map record, assets) and optionally the zip.",
            vec![
                p(),
                ("zip", prop("boolean", "also pack the zip (default false)")),
                ("no_rom_paths", prop("boolean", "refuse a project with ROM texture paths (default false)")),
            ],
            &["project"],
        ),
        verb(
            "render",
            "Render a PNG of the map without a GPU (software path; top | iso | spawn:<n> | free:<x,y,z,yaw,pitch>): the CLI's JSON answer (image path, size, triangles, markers with their pixel positions, legend, grid_step, top_bounds) plus the picture as image content.",
            vec![
                p(),
                ("view", prop("string", "top | iso | spawn:<n> | free:<x,y,z,yaw,pitch> (default top)")),
                ("width", prop("integer", "image width (default 1024)")),
                ("height", prop("integer", "image height (default 768)")),
                ("labels", prop("boolean", "draw a disc per object, coloured by class; the result's `markers` list their ids and pixel positions, `legend` the colours (default false)")),
                ("textured", prop("boolean", "draw it with the game's renderer and the real textures instead of flat colours; needs dc-client and a GPU, and writes <project>/build/render/<view>_textured.png (default false)")),
                ("grid", prop("boolean", "draw the world grid on the floor of the extent in the top and iso views (default false)")),
                ("grid_step", prop("number", "grid spacing in world units (default 512)")),
                ("out", prop("string", "write the PNG to this path (default <project>/build/render/<view>.png, as the CLI does)")),
            ],
            &["project"],
        ),
        verb(
            "apply",
            "Apply a batch of JSON ops ({\"op\": name, ...}; see the schema tool) all-or-nothing; the result echoes the new ids.",
            vec![
                p(),
                ("ops", json!({"type": "array", "items": {"type": "object"}, "description": "the ops, each {\"op\": \"<name>\", ...fields}"})),
                dry_run_prop(),
            ],
            &["project", "ops"],
        ),
        verb(
            "get",
            "Query objects: by class, one id (object, solid, prefab or import), or within a radius of (x, z).",
            vec![
                p(),
                ("class", prop("string", "only this dc.* class")),
                ("id", prop("string", "one id")),
                ("near", object_schema(
                    vec![("x", prop("number", "centre x")), ("z", prop("number", "centre z")), ("r", prop("number", "radius"))],
                    &["x", "z", "r"],
                )),
            ],
            &["project"],
        ),
        verb(
            "terrain",
            "One terrain op with flat arguments: gen (preset, seed, amplitude, base, scale, octaves, cells, cell_size), raise / lower (x, z, radius, amount, falloff), smooth (x, z, radius, passes), flatten (x, z, radius, height, falloff).",
            vec![
                p(),
                ("op", enum_of(&["gen", "raise", "lower", "smooth", "flatten"], "the terrain op")),
                ("preset", enum_of(&["hills", "valley", "ridge"], "gen: preset (default hills)")),
                ("seed", prop("integer", "gen: seed (default: the project seed)")),
                ("amplitude", prop("number", "gen: height range (default 256)")),
                ("base", prop("number", "gen: mean height (default 0)")),
                ("scale", prop("number", "gen: feature size (default 1024)")),
                ("octaves", prop("integer", "gen: noise layers (default 5)")),
                ("cells", prop("integer", "gen: cells per side for a new terrain")),
                ("cell_size", prop("number", "gen: cell size for a new terrain")),
                ("x", prop("number", "brush centre x")),
                ("z", prop("number", "brush centre z")),
                ("radius", prop("number", "brush radius")),
                ("amount", prop("number", "raise / lower: world units")),
                ("height", prop("number", "flatten: target height")),
                ("falloff", enum_of(&["smooth", "linear", "flat"], "brush profile (default smooth)")),
                ("passes", prop("integer", "smooth: passes (default 1)")),
                dry_run_prop(),
            ],
            &["project", "op"],
        ),
        verb(
            "snapshot",
            "Save a history entry (.mapkit/history/<n>/).",
            vec![p(), ("label", prop("string", "a label"))],
            &["project"],
        ),
        verb("history", "List the history entries, oldest first.", vec![p()], &["project"]),
        verb(
            "revert",
            "Restore a history entry (the current state is snapshotted first).",
            vec![p(), ("to", prop("integer", "entry number"))],
            &["project", "to"],
        ),
        verb(
            "run",
            "Run a Lua generator (inline script text) in the mod sandbox against the map.* API and apply its ops as one batch.",
            vec![
                p(),
                ("script", prop("string", "the Lua script text")),
                ("budget", prop("integer", "Lua instruction budget (default 1e9)")),
                dry_run_prop(),
            ],
            &["project", "script"],
        ),
        verb(
            "rom_list",
            "List ROM assets (textures | sounds | sprites | surfaces | worlds | modes) from the user's ROM; isError without a ROM.",
            vec![
                p(),
                ("kind", enum_of(&["textures", "sounds", "sprites", "surfaces", "worlds", "modes"], "the listing")),
                ("filter", prop("string", "substring filter")),
                ("thumbs", prop("string", "write texture thumbnails into this folder")),
                ("rom", prop("string", "ROM folder (else CA_ROM)")),
            ],
            &["kind"],
        ),
        verb(
            "schema",
            "The project format, every op and field, the classes, the modes and the diagnostic codes (JSON).",
            vec![p(), ("ops", prop("boolean", "ops only (default false)"))],
            &[],
        ),
    ];
    for (name, op) in TYPED_TWINS {
        let doc = schema::OPS
            .iter()
            .find(|(n, _, _)| n == op)
            .map(|(_, d, _)| *d)
            .unwrap_or("");
        out.push(Tool {
            name,
            description: format!(
                "{doc} (the `{op}` op with flat typed arguments; the batch is one op)"
            ),
            input_schema: twin_schema(op),
            op: Some(op),
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Calling
// ---------------------------------------------------------------------------------------------------------------

fn arg_str<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn arg_bool(args: &Map<String, Value>, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn arg_num(args: &Map<String, Value>, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

fn num_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// Run a verb through the CLI in-process: stdout becomes the text content, stderr is forwarded.
fn run_verb(argv: Vec<String>) -> ToolResult {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut full = vec!["dc-mapkit".to_string()];
    full.extend(argv);
    full.push("--json".into());
    let code = cli::run_cli(&full, &mut out, &mut err);
    forward_log(&err);
    let text = String::from_utf8_lossy(&out).trim().to_string();
    let text = if text.is_empty() {
        format!("{{\"ok\": false, \"exit\": {code}}}")
    } else {
        text
    };
    ToolResult::text(text, code != 0)
}

fn forward_log(err: &[u8]) {
    for line in String::from_utf8_lossy(err).lines() {
        if !line.trim().is_empty() {
            eprintln!("dc-mapkit mcp: {line}");
        }
    }
}

/// The CLI's JSON envelope for an in-process verb result.
fn envelope(verb: &str, r: Result<Outcome, Diagnostics>) -> ToolResult {
    match r {
        Ok(o) => {
            let is_error = o.diagnostics.has_errors();
            let v =
                json!({"ok": true, "verb": verb, "result": o.result, "diagnostics": o.diagnostics});
            ToolResult::text(serde_json::to_string(&v).expect("json"), is_error)
        }
        Err(d) => {
            let v = json!({"ok": false, "verb": verb, "result": null, "diagnostics": d});
            ToolResult::text(serde_json::to_string(&v).expect("json"), true)
        }
    }
}

fn usage(tool: &str, msg: &str) -> ToolResult {
    ToolResult::text(format!("tool `{tool}`: {msg}"), true)
}

/// Call a tool with its arguments object.
pub fn call_tool(tool: &Tool, args: &Map<String, Value>) -> ToolResult {
    let project = arg_str(args, "project").map(str::to_string);
    let needs_project = !matches!(tool.name, "schema" | "rom_list");
    let dir = match (&project, needs_project) {
        (Some(p), _) => PathBuf::from(p),
        (None, false) => PathBuf::new(),
        (None, true) => return usage(tool.name, "`project` (the project folder) is required"),
    };
    let dir_s = dir.display().to_string();
    let flag = |argv: &mut Vec<String>, name: &str, key: &str| {
        if let Some(v) = args.get(key) {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => num_text(n.as_f64().unwrap_or(0.0)),
                Value::Bool(b) => b.to_string(),
                _ => return,
            };
            argv.push(format!("--{name}"));
            argv.push(text);
        }
    };
    if let Some(op) = tool.op {
        return call_twin(tool.name, op, &dir, args);
    }
    match tool.name {
        "new" => {
            let mut argv = vec!["new".to_string(), dir_s];
            for k in ["template", "name", "author", "seed"] {
                flag(&mut argv, k, k);
            }
            run_verb(argv)
        }
        "info" => {
            let mut argv = vec!["info".to_string(), dir_s];
            flag(&mut argv, "rom", "rom");
            run_verb(argv)
        }
        "validate" | "build" | "history" => run_verb(vec![tool.name.to_string(), dir_s]),
        "export" => {
            let mut argv = vec!["export".to_string(), dir_s];
            if arg_bool(args, "zip") {
                argv.push("--zip".into());
            }
            if arg_bool(args, "no_rom_paths") {
                argv.push("--no-rom-paths".into());
            }
            run_verb(argv)
        }
        "render" => match render_tool(&dir, args) {
            Ok((o, png)) => {
                let mut r = envelope("render", Ok(o));
                r.content
                    .push(json!({"type": "image", "data": base64(&png), "mimeType": "image/png"}));
                r
            }
            Err(d) => envelope("render", Err(d)),
        },
        "apply" => {
            let Some(ops_v) = args.get("ops") else {
                return usage(tool.name, "`ops` (an array of ops) is required");
            };
            if !ops_v.is_array() {
                return usage(
                    tool.name,
                    "`ops` must be an array of {\"op\": name, ...} objects",
                );
            }
            let text = serde_json::to_string(ops_v).expect("json");
            envelope(
                "apply",
                apply_text(&dir, &text, arg_bool(args, "dry_run"), "mcp apply"),
            )
        }
        "get" => {
            let mut argv = vec!["get".to_string(), dir_s];
            flag(&mut argv, "class", "class");
            flag(&mut argv, "id", "id");
            match args.get("near") {
                None => {}
                Some(Value::Object(n)) => {
                    let (x, z, r) = (
                        n.get("x").and_then(Value::as_f64),
                        n.get("z").and_then(Value::as_f64),
                        n.get("r").and_then(Value::as_f64),
                    );
                    let (Some(x), Some(z), Some(r)) = (x, z, r) else {
                        return usage(tool.name, "`near` is {x, z, r}");
                    };
                    argv.push("--near".into());
                    argv.push(format!("{},{},{}", num_text(x), num_text(z), num_text(r)));
                }
                Some(Value::String(s)) => {
                    argv.push("--near".into());
                    argv.push(s.clone());
                }
                Some(_) => return usage(tool.name, "`near` is {x, z, r}"),
            }
            run_verb(argv)
        }
        "terrain" => {
            let Some(op) = arg_str(args, "op") else {
                return usage(
                    tool.name,
                    "`op` (gen | raise | lower | smooth | flatten) is required",
                );
            };
            let mut argv = vec!["terrain".to_string(), dir_s, op.to_string()];
            for (name, key) in [
                ("preset", "preset"),
                ("seed", "seed"),
                ("amplitude", "amplitude"),
                ("base", "base"),
                ("scale", "scale"),
                ("octaves", "octaves"),
                ("cells", "cells"),
                ("cell-size", "cell_size"),
                ("x", "x"),
                ("z", "z"),
                ("radius", "radius"),
                ("amount", "amount"),
                ("height", "height"),
                ("falloff", "falloff"),
                ("passes", "passes"),
            ] {
                flag(&mut argv, name, key);
            }
            if arg_bool(args, "dry_run") {
                argv.push("--dry-run".into());
            }
            run_verb(argv)
        }
        "snapshot" => {
            let mut argv = vec!["snapshot".to_string(), dir_s];
            flag(&mut argv, "label", "label");
            run_verb(argv)
        }
        "revert" => {
            let Some(to) = arg_num(args, "to") else {
                return usage(tool.name, "`to` (the entry number) is required");
            };
            run_verb(vec!["revert".into(), dir_s, "--to".into(), num_text(to)])
        }
        "run" => {
            let Some(text) = arg_str(args, "script") else {
                return usage(tool.name, "`script` (the Lua text) is required");
            };
            let opts = script::RunOptions {
                budget: arg_num(args, "budget")
                    .map_or(script::DEFAULT_BUDGET, |b| b.max(1.0) as u64),
            };
            let r = Project::open(&dir).and_then(|(p, h)| {
                let mut err = Vec::new();
                let r = cli::run_script(
                    &dir,
                    &p,
                    &h,
                    text,
                    &opts,
                    arg_bool(args, "dry_run"),
                    &mut err,
                );
                forward_log(&err);
                r
            });
            envelope("run", r)
        }
        "rom_list" => {
            let Some(kind) = arg_str(args, "kind") else {
                return usage(
                    tool.name,
                    "`kind` (textures | sounds | sprites | surfaces | worlds | modes) is required",
                );
            };
            let mut argv = vec!["rom".to_string(), kind.to_string()];
            for k in ["filter", "thumbs", "rom"] {
                flag(&mut argv, k, k);
            }
            run_verb(argv)
        }
        "schema" => {
            let mut argv = vec!["schema".to_string()];
            if arg_bool(args, "ops") {
                argv.push("--ops".into());
            }
            run_verb(argv)
        }
        other => usage(other, "no such tool"),
    }
}

/// Parse an ops document and apply it to the project on disk (snapshot + save, or dry run).
fn apply_text(dir: &Path, text: &str, dry_run: bool, label: &str) -> Result<Outcome, Diagnostics> {
    let ops = cli::parse_ops(text)?;
    let mut err = Vec::new();
    let r = cli::apply_to_project(dir, &ops, dry_run, label, &mut err);
    forward_log(&err);
    r
}

/// A typed twin: the arguments minus `project` / `dry_run` are the op's fields.
fn call_twin(tool: &str, op: &str, dir: &Path, args: &Map<String, Value>) -> ToolResult {
    let mut fields = args.clone();
    fields.remove("project");
    let dry_run = fields
        .remove("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    fields.insert("op".into(), Value::String(op.into()));
    let text = serde_json::to_string(&Value::Array(vec![Value::Object(fields)])).expect("json");
    envelope(
        tool,
        apply_text(dir, &text, dry_run, &format!("mcp {tool}")),
    )
}

/// `render`: the software path, mirroring the CLI arm (the level plus the project's object markers through
/// `Scene::add_project_objects`, then `render_report`): the CLI's JSON answer (`view`, `image`, `width`,
/// `height`, `triangles`, `legend`, `markers`, `grid_step`, `top_bounds`, plus `out` when given), the PNG written
/// to `out` or the CLI's default `<project>/build/render/<view>.png`, and the PNG bytes (sent as image content).
// DC-DEFAULT: MAP-59 (the verb tools answer the CLI's envelope; `render` takes `grid_step` and `out` as well)
fn render_tool(dir: &Path, args: &Map<String, Value>) -> Result<(Outcome, Vec<u8>), Diagnostics> {
    let (p, h) = Project::open(dir)?;
    let view_text = arg_str(args, "view").unwrap_or("top");
    let view = render_sw::View::parse(view_text)
        .map_err(|e| Diagnostics::one(Diagnostic::new(Code::OpInvalid, "view", e)))?;
    let size = |key: &str, default: u32| -> u32 {
        arg_num(args, key).map_or(default, |n| (n as i64).clamp(16, 4096) as u32)
    };
    let (w, hgt) = (size("width", 1024), size("height", 768));
    let out_arg = arg_str(args, "out");
    if arg_bool(args, "textured") {
        let t = crate::render_gpu::draw_textured(&crate::render_gpu::TexturedRequest {
            dir,
            view,
            size: (w, hgt),
            out: out_arg.map(PathBuf::from),
            binaries: None,
            rom: None,
        })?;
        return Ok((t.outcome, t.png));
    }
    let out = out_arg.map(PathBuf::from).unwrap_or_else(|| {
        dir.join("build")
            .join("render")
            .join(format!("{}.png", view.slug()))
    });
    let opts = render_sw::RenderOptions {
        labels: arg_bool(args, "labels"),
        grid: arg_bool(args, "grid"),
        grid_step: arg_num(args, "grid_step")
            .filter(|s| s.is_finite() && *s > 0.0)
            .map_or(render_sw::GRID_STEP, |s| s as f32),
        sun_dir: crate::geom::V3::from_array(p.settings.sun_dir),
        ..render_sw::RenderOptions::default()
    };
    let level = build::level_of(&p, &h, &build::Assets::new(dir))?;
    let mut scene = render_sw::Scene::from_level(&level, &opts);
    scene.add_project_objects(&p);
    let r = render_sw::render_report(&scene, view.clone(), (w, hgt), &opts)?;
    let png = r.image.png();
    crate::project::write_atomic(&out, &png).map_err(|e| {
        Diagnostics::one(Diagnostic::new(
            Code::ProjectIo,
            out.display().to_string(),
            e.to_string(),
        ))
    })?;
    let mut result = json!({
        "view": view.text(),
        "image": out.display().to_string(),
        "width": r.image.width,
        "height": r.image.height,
        "triangles": r.triangles,
        "legend": r.legend,
        "markers": r.markers,
        "grid_step": r.grid_step,
        "top_bounds": r.top_bounds,
    });
    if let Some(o) = out_arg {
        result["out"] = json!(o);
    }
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
    Ok((
        Outcome {
            result,
            text,
            diagnostics: Diagnostics::new(),
        },
        png,
    ))
}

/// Standard base64 with padding (RFC 4648), enough for one PNG; no dependency needed.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::{self, Template};
    use std::collections::BTreeSet;
    use std::io::Cursor;

    fn req(id: u64, method: &str, params: Value) -> String {
        serde_json::to_string(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .unwrap()
    }

    fn note(method: &str, params: Value) -> String {
        serde_json::to_string(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dc-mapkit-mcp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The tool result of a `tools/call` response: (parsed envelope or raw text, isError).
    fn tool_result(resp: &Value) -> (Value, bool) {
        let content = resp["result"]["content"].as_array().expect("content");
        let text = content
            .iter()
            .find(|c| c["type"] == "text")
            .and_then(|c| c["text"].as_str())
            .unwrap_or("");
        let parsed = serde_json::from_str(text).unwrap_or(Value::String(text.to_string()));
        (
            parsed,
            resp["result"]["isError"].as_bool().expect("isError"),
        )
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// Regression (review C23): a line that is not UTF-8 is answered `-32700` and the session goes on; the `mcp`
    /// verb writes nothing but response lines to stdout (no empty line, no envelope under `--json`, no
    /// diagnostic text), and a closed stdin ends it with exit 0.
    #[test]
    fn the_mcp_verb_writes_only_response_lines_and_survives_a_non_utf8_line() {
        let mut input = Vec::new();
        input.extend_from_slice(req(1, "ping", json!({})).as_bytes());
        input.extend_from_slice(b"\n\xff\xfe{\"jsonrpc\": \"2.0\"}\n");
        input.extend_from_slice(req(2, "ping", json!({})).as_bytes());
        input.extend_from_slice(b"\n");
        for json_flag in [false, true] {
            let mut argv = vec!["dc-mapkit".to_string(), "mcp".to_string()];
            if json_flag {
                argv.push("--json".into());
            }
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code =
                cli::run_cli_with_stdin(&argv, &mut Cursor::new(input.clone()), &mut out, &mut err);
            assert_eq!(code, 0, "stdin closed: exit 0 (json {json_flag})");
            let out = String::from_utf8(out).unwrap();
            let lines: Vec<&str> = out.split('\n').collect();
            assert_eq!(lines.last(), Some(&""), "every line ends with a newline");
            let lines = &lines[..lines.len() - 1];
            assert_eq!(
                lines.len(),
                3,
                "two pings and one parse error, nothing else: {out:?}"
            );
            for line in lines {
                assert!(!line.is_empty(), "no empty line on stdout: {out:?}");
                let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
                assert_eq!(v["jsonrpc"], "2.0");
                assert!(v.get("verb").is_none(), "no CLI envelope on stdout: {line}");
            }
            let bad: Value = serde_json::from_str(lines[1]).unwrap();
            assert_eq!(bad["error"]["code"], PARSE_ERROR);
            assert!(bad["id"].is_null());
            assert!(
                !out.contains("project.io"),
                "no diagnostic text on stdout: {out}"
            );
            let err = String::from_utf8(err).unwrap();
            assert!(
                !err.contains("project.io"),
                "a bad line is not a stream failure: {err}"
            );
        }
        // a closed stdin right away: nothing on stdout, exit 0
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let argv = vec![
            "dc-mapkit".to_string(),
            "mcp".to_string(),
            "--json".to_string(),
        ];
        let code = cli::run_cli_with_stdin(&argv, &mut Cursor::new(Vec::new()), &mut out, &mut err);
        assert_eq!(code, 0);
        assert!(out.is_empty(), "{:?}", String::from_utf8_lossy(&out));
    }

    #[test]
    fn tool_schemas_are_objects_with_a_project_argument_and_twins_follow_the_op_fields() {
        let tools = tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        let mut expected: Vec<&str> = VERB_TOOLS.to_vec();
        expected.extend(TYPED_TWINS.iter().map(|(n, _)| *n));
        assert_eq!(names, expected);
        for t in &tools {
            assert_eq!(t.input_schema["type"], "object", "{}", t.name);
            assert!(
                t.input_schema["properties"]["project"].is_object(),
                "{}",
                t.name
            );
            let required: Vec<&str> = t.input_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            if matches!(t.name, "schema" | "rom_list") {
                assert!(!required.contains(&"project"), "{}", t.name);
            } else {
                assert!(required.contains(&"project"), "{}", t.name);
            }
            assert!(!t.description.is_empty());
        }
        let add = tools.iter().find(|t| t.name == "add_object").unwrap();
        let props = add.input_schema["properties"].as_object().unwrap();
        for (f, _, _) in schema::op_fields("add").unwrap() {
            assert!(props.contains_key(*f), "add_object lacks {f}");
        }
        let required: Vec<&str> = add.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, ["project", "class", "pos"]);
        assert_eq!(props["pos"]["minItems"], 3);
        let gen = tools.iter().find(|t| t.name == "terrain_noise").unwrap();
        assert_eq!(
            gen.input_schema["properties"]["preset"]["enum"],
            json!(["hills", "valley", "ridge", "flat"])
        );
        assert_eq!(gen.input_schema["required"], json!(["project", "preset"]));
    }

    /// The conformance test: a scripted client over in-memory pipes runs the handshake, the error cases and every
    /// tool against a temp project (DC-DEFAULT: MAP-58, MAP-59).
    #[test]
    fn a_scripted_client_runs_the_handshake_and_every_tool() {
        let dir = tmp("project");
        let made = template::make(Template::Arena, "Mcp Test", "tests", 3);
        cli::write_project(&dir, &made, Template::Arena).unwrap();
        let next = made.project.next_id;
        let added = format!("item_spawn_{next}");
        let added2 = format!("item_spawn_{}", next + 1);
        let d = dir.to_str().unwrap().to_string();
        let dir2 = tmp("new");
        let d2 = dir2.to_str().unwrap().to_string();

        // (tool, arguments, expectation: Some(true) = ok, Some(false) = isError, None = either)
        let calls: Vec<(&str, Value, Option<bool>)> = vec![
            (
                "new",
                json!({"project": d2, "template": "flat", "name": "Second", "author": "tests"}),
                Some(true),
            ),
            ("info", json!({"project": d}), Some(true)),
            ("validate", json!({"project": d}), Some(true)),
            ("schema", json!({"ops": true}), Some(true)),
            (
                "get",
                json!({"project": d, "class": "dc.bomb_site"}),
                Some(true),
            ),
            (
                "get",
                json!({"project": d, "near": {"x": 0, "z": -1024, "r": 10}}),
                Some(true),
            ),
            (
                "apply",
                json!({"project": d, "ops": [{"op": "add", "class": "dc.item_spawn", "pos": [100, 0, 100], "ground": true}]}),
                Some(true),
            ),
            (
                "add_object",
                json!({"project": d, "class": "dc.item_spawn", "pos": [200, 0, 200], "props": {"modes": ""}}),
                Some(true),
            ),
            (
                "move_object",
                json!({"project": d, "id": added, "delta": [10, 0, 0], "ground": true}),
                Some(true),
            ),
            (
                "set_property",
                json!({"project": d, "id": added, "prop": "modes", "value": "TDM"}),
                Some(true),
            ),
            (
                "remove_object",
                json!({"project": d, "id": added2}),
                Some(true),
            ),
            (
                "terrain_noise",
                json!({"project": d, "preset": "valley", "amplitude": 64}),
                Some(true),
            ),
            (
                "terrain",
                json!({"project": d, "op": "raise", "x": 0, "z": 0, "radius": 300, "amount": 20}),
                Some(true),
            ),
            (
                "snapshot",
                json!({"project": d, "label": "mcp"}),
                Some(true),
            ),
            ("history", json!({"project": d}), Some(true)),
            ("revert", json!({"project": d, "to": 1}), Some(true)),
            (
                "run",
                json!({"project": d, "script": "map.add{class = 'dc.item_spawn', pos = {1, 0, 1}}\nprint('hi')", "dry_run": true}),
                Some(true),
            ),
            (
                "run",
                json!({"project": d, "script": "while true do end", "budget": 50000}),
                Some(false),
            ),
            ("build", json!({"project": d}), None),
            ("export", json!({"project": d, "zip": true}), None),
            (
                "render",
                json!({"project": d, "view": "top", "width": 64, "height": 48, "labels": true, "grid": true, "grid_step": 1024}),
                Some(true),
            ),
            ("rom_list", json!({"kind": "textures", "filter": "x"}), None),
            (
                "apply",
                json!({"project": d, "ops": [{"op": "remove", "id": "solid_999"}]}),
                Some(false),
            ),
            ("validate", json!({}), Some(false)),
            ("terrain", json!({"project": d, "op": "raise"}), Some(false)),
        ];
        let mut input = vec![
            req(
                1,
                "initialize",
                json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
            ),
            note("notifications/initialized", json!({})),
            req(2, "ping", json!({})),
            "this is not json".to_string(),
            "[1, 2, 3]".to_string(),
            serde_json::to_string(&json!({"jsonrpc": "1.0", "id": 3, "method": "ping"})).unwrap(),
            req(4, "nothing/here", json!({})),
            req(5, "tools/list", json!({})),
            req(6, "resources/list", json!({})),
            req(7, "resources/read", json!({"uri": PLAYBOOK_URI})),
            req(8, "resources/read", json!({"uri": SCHEMA_URI})),
            req(9, "resources/read", json!({"uri": "mapkit://nothing"})),
            note("notifications/cancelled", json!({"requestId": 8})),
            note("notifications/unknown", json!({})),
            req(
                10,
                "tools/call",
                json!({"name": "nothing", "arguments": {}}),
            ),
            req(11, "tools/call", json!({"arguments": {}})),
            req(12, "initialize", json!({"protocolVersion": "1999-01-01"})),
            // the batching revision is answered with ours (MAP-58): this transport refuses batch arrays
            req(13, "initialize", json!({"protocolVersion": "2025-03-26"})),
            serde_json::to_string(&json!([{"jsonrpc": "2.0", "id": 14, "method": "ping"}]))
                .unwrap(),
        ];
        let first_tool_id = 100u64;
        for (i, (name, args, _)) in calls.iter().enumerate() {
            input.push(req(
                first_tool_id + i as u64,
                "tools/call",
                json!({"name": name, "arguments": args}),
            ));
        }
        let text = input.join("\n") + "\n";
        let mut out = Vec::new();
        serve(&mut Cursor::new(text.into_bytes()), &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        let mut by_id: std::collections::BTreeMap<String, Value> = Default::default();
        let mut responses = 0;
        for line in out.lines() {
            assert!(!line.is_empty());
            let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
            assert_eq!(v["jsonrpc"], "2.0");
            responses += 1;
            by_id.insert(v["id"].to_string(), v);
        }
        // every request with an id gets one answer, the three notifications none; the parse error, the
        // non-object line and the batch array answer with id null
        assert_eq!(responses, 16 + calls.len());

        // handshake
        let init = &by_id["1"];
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(by_id["13"]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(!SUPPORTED_VERSIONS.contains(&"2025-03-26"));
        assert!(
            !by_id.contains_key("14"),
            "a batch array is never dispatched"
        );
        assert_eq!(
            out.matches("\"code\":-32600").count(),
            3,
            "the [1, 2, 3] line, the batch array and the jsonrpc 1.0 request: {out}"
        );
        assert_eq!(
            init["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
        assert_eq!(
            init["result"]["capabilities"]["resources"]["subscribe"],
            false
        );
        assert!(init["result"]["capabilities"].get("prompts").is_none());
        assert_eq!(init["result"]["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(by_id["12"]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(by_id["2"]["result"], json!({}));
        // malformed lines: parse error and invalid request with a null id
        let nulls: Vec<&Value> = by_id.values().filter(|v| v["id"].is_null()).collect();
        assert_eq!(nulls.len(), 1, "the map keeps one null-id entry: {nulls:?}");
        assert!(out.contains("\"code\":-32700"));
        assert!(out.contains("\"code\":-32600"));
        assert_eq!(by_id["3"]["error"]["code"], INVALID_REQUEST);
        assert_eq!(by_id["4"]["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(by_id["10"]["error"]["code"], INVALID_PARAMS);
        assert_eq!(by_id["11"]["error"]["code"], INVALID_PARAMS);
        assert_eq!(by_id["9"]["error"]["code"], RESOURCE_NOT_FOUND);

        // tools/list and resources
        let listed: BTreeSet<String> = by_id["5"]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                assert_eq!(t["inputSchema"]["type"], "object", "{t}");
                assert!(t["description"].is_string());
                t["name"].as_str().unwrap().to_string()
            })
            .collect();
        let called: BTreeSet<String> = calls.iter().map(|(n, _, _)| n.to_string()).collect();
        assert_eq!(listed, called, "every listed tool is called by this test");
        assert_eq!(
            by_id["6"]["result"]["resources"].as_array().unwrap().len(),
            3
        );
        let playbook = &by_id["7"]["result"]["contents"][0];
        assert_eq!(playbook["uri"], PLAYBOOK_URI);
        assert_eq!(playbook["mimeType"], "text/markdown");
        let playbook_text = playbook["text"].as_str().unwrap();
        assert!(playbook_text.contains("MAPKIT_AGENTS"));
        // the served playbook is the updated file (review C25 .. C27): no stale stub sentence, the real
        // `build` / `render` / `rom` shapes, `rom modes` without a ROM, the seeded `math.random` rule
        assert_eq!(playbook_text, PLAYBOOK);
        assert!(
            !playbook_text.contains("still answers `not_implemented`"),
            "stale status paragraph"
        );
        assert!(
            !playbook_text.contains("`not_implemented` from `gui`"),
            "stale troubleshooting row"
        );
        assert!(
            !playbook_text.contains("is not in this build yet"),
            "stale diagnostics line"
        );
        assert!(playbook_text.contains("`{file, bytes, stats: {polys, brushes, sky_faces, nodes, depth, textures, vertices, triangles, sections, bytes}}`"));
        assert!(playbook_text.contains("`{view, image, width, height, triangles, legend: [{key, color}], markers: [{id, class, key, x, y, visible}], grid_step, top_bounds}`"));
        assert!(
            playbook_text.contains("`{kind, rom: {path, source} \\| null, count, thumbs, rows:")
        );
        assert!(playbook_text.replace('\r', "").contains("except `kind:\n  \"modes\"`"));
        assert!(playbook_text.contains("`math.random` is deterministic"));
        assert!(playbook_text.contains("2025-03-26 included"));
        let schema_text = by_id["8"]["result"]["contents"][0]["text"]
            .as_str()
            .unwrap();
        assert!(serde_json::from_str::<Value>(schema_text).unwrap()["ops"].is_object());

        // tools/call
        for (i, (name, args, expect)) in calls.iter().enumerate() {
            let resp = &by_id[&(first_tool_id + i as u64).to_string()];
            assert!(resp.get("error").is_none(), "{name} {args}: {resp}");
            let (body, is_error) = tool_result(resp);
            if let Some(ok) = expect {
                assert_eq!(!is_error, *ok, "{name} {args}: {body}");
            }
            if !is_error {
                assert_eq!(body["ok"], true, "{name}: {body}");
            }
            match (*name, i) {
                ("new", _) => assert_eq!(body["result"]["template"], "flat"),
                ("info", _) => assert_eq!(body["result"]["spawns"]["team1"], 8),
                ("get", 4) => assert_eq!(body["result"]["count"], 2),
                ("get", 5) => assert_eq!(body["result"]["count"], 1),
                ("apply", 6) => assert_eq!(body["result"]["applied"][0]["ids"][0], added),
                ("add_object", _) => assert_eq!(body["result"]["applied"][0]["ids"][0], added2),
                ("history", _) => assert!(body["result"].as_array().unwrap().len() >= 6, "{body}"),
                ("run", 16) => {
                    assert_eq!(body["result"]["dry_run"], true);
                    assert_eq!(body["result"]["ops"], 1);
                    assert_eq!(body["result"]["prints"], json!(["hi"]));
                }
                ("run", 17) => assert_eq!(body["diagnostics"][0]["code"], "script.budget"),
                ("render", _) => {
                    let content = resp["result"]["content"].as_array().unwrap();
                    let img = content
                        .iter()
                        .find(|c| c["type"] == "image")
                        .expect("image content");
                    assert_eq!(img["mimeType"], "image/png");
                    assert!(img["data"].as_str().unwrap().starts_with("iVBOR"), "a PNG");
                    // the CLI's JSON answer (MAP-59): the same keys as `render --json`, markers with labels
                    let r = body["result"].as_object().expect("result object");
                    let keys: BTreeSet<&str> = r.keys().map(String::as_str).collect();
                    let expected: BTreeSet<&str> = [
                        "view",
                        "image",
                        "width",
                        "height",
                        "triangles",
                        "legend",
                        "markers",
                        "grid_step",
                        "top_bounds",
                    ]
                    .into_iter()
                    .collect();
                    assert_eq!(keys, expected, "{body}");
                    assert_eq!(r["view"], "top");
                    assert_eq!(r["width"], 64);
                    assert_eq!(r["height"], 48);
                    assert_eq!(r["grid_step"], 1024.0);
                    assert!(r["top_bounds"].as_array().is_some_and(|b| b.len() == 4));
                    let markers = r["markers"].as_array().expect("markers list");
                    assert!(
                        !markers.is_empty(),
                        "labels draw the arena's objects: {body}"
                    );
                    assert!(markers
                        .iter()
                        .all(|m| m["id"].is_string() && m["class"].is_string()));
                    assert!(!r["legend"].as_array().expect("legend").is_empty());
                    let image = PathBuf::from(r["image"].as_str().unwrap());
                    assert!(
                        image.starts_with(dir.join("build").join("render")),
                        "{image:?}"
                    );
                    assert!(image.exists(), "the PNG is written as the CLI does");
                }
                ("build", _) | ("export", _) => {
                    if is_error {
                        assert!(
                            body["diagnostics"]
                                .as_array()
                                .is_some_and(|d| !d.is_empty()),
                            "{body}"
                        );
                    }
                }
                ("apply", 22) => assert_eq!(body["diagnostics"][0]["code"], "id.unknown"),
                ("validate", 23) => assert!(body.as_str().unwrap().contains("project"), "{body}"),
                ("terrain", 24) => assert_eq!(body["ok"], false),
                _ => {}
            }
        }
        // the removed twin object is gone, the moved one carries its property
        let (p, _) = Project::open(&dir).unwrap();
        assert!(p.object(&added2).is_none());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }
}

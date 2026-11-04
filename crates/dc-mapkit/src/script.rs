//! `dc-mapkit run <dir> <script.lua|->`: Lua generators in the dc-mod-lua sandbox against a `map` API
//! (MAPKIT_PLAN.md section 7, D8). DC-DEFAULT: MAP-56 (the API and the batch semantics), MAP-57 (the budget).
//!
//! A script runs in [`dc_mod_lua::LuaSandbox`], the mod sandbox (text chunks only; `string`, `table`, `math`,
//! `utf8`; no `os`, `io`, `debug`, `package`, `load`; an instruction budget), with one extra global, `map`. Every
//! mutating `map.*` call builds one [`Op`], applies it to a WORKING COPY of the project (so an invalid op fails at
//! its line and later queries see the pending state) and queues it; when the script ends the queue is applied to
//! the real project as ONE batch through [`ops::apply`], all-or-nothing. A script that errors (syntax, runtime,
//! budget, a refused op outside `pcall`) changes nothing. Errors carry the script line (`script:<line>`).
//!
//! The `map` table (every table argument takes `{x, y, z}` or `{x = .., y = .., z = ..}` for a vec3; property
//! values are text, numbers, bools, a vec3 or `{rgb = {r, g, b}}`; a missing optional field takes the op's
//! default, `dc-mapkit schema --ops`):
//!
//! | call | op | returns |
//! |---|---|---|
//! | `map.add{class, pos, name?, rot?, props?, ground?}` | `add` | the new id |
//! | `map.remove(id)` | `remove` | the id |
//! | `map.move(id, pos)` / `map.move(id, {delta = v})` / `map.move(id, {ground = true})` | `move` | the id |
//! | `map.set(id, prop, value)` | `set` | the id |
//! | `map.rename(id, name)` | `rename` | the id |
//! | `map.solid.box{pos, size, material, rot?, faces?, group?, brush?, ground?}` | `solid.add` (box) | the new id |
//! | `map.solid.wedge{...}` / `map.solid.cylinder{sides?, ...}` / `map.solid.prism{points, ...}` | `solid.add` | the new id |
//! | `map.solid.set(id, {pos?, rot?, size?, material?, faces?, group?, brush?})` | `solid.set` | the id |
//! | `map.prefab(name, {pos, rot?, scale?, ground?})` | `prefab.add` | the new id |
//! | `map.import{file, pos, material, rot?, scale?, collision?, ground?}` | `import.add` | the new id |
//! | `map.material.set{id, texture?, tile?, surface_id?, flags?}` (`texture` text = an own PNG path) | `material.set` | the id |
//! | `map.terrain.gen{preset?, seed?, amplitude?, base?, scale?, octaves?, cells_x?, cells_z?, cell_size?}` (`map.terrain.noise` is the same) | `terrain.gen` | nothing |
//! | `map.terrain.raise{x, z, radius, amount, falloff?}` / `lower` / `flatten{x, z, radius, height, falloff?}` | `terrain.*` | nothing |
//! | `map.terrain.smooth{x?, z?, radius?, passes?}` (no table = the whole terrain) | `terrain.smooth` | nothing |
//! | `map.terrain.height(x, z)` | (query) | the terrain height or nil |
//! | `map.ground(x, z)` | (query) | the floor (terrain or solid top) or nil |
//! | `map.query{class?, id?, near = {x, z, r}?}` (`near` also as `{x = .., z = .., r = ..}`) | (query) | a list of `{id, class, name, pos, rot, props}` |
//! | `map.solids{group?, brush?}` | (query) | a list of `{id, kind, pos, rot_y_deg, size, material, faces, group, brush}` |
//! | `map.info()` | (query) | name, key, seed, counts, spawns, modes, terrain, extent, pending ops |
//! | `map.ops()` | (query) | the number of queued ops |
//!
//! Constants: `map.MODES`, `map.CLASSES` (point classes `add` accepts), `map.BRUSHES` (brush roles),
//! `map.PRESETS`, `map.FALLOFFS`, `map.seed` (the project's seed). `print(...)` lines are collected and returned
//! (`Generated::prints`); the CLI writes them to stderr as `script: ...`.
//!
//! Determinism (MAPKIT_PLAN.md section 7, D8): `math.random` in a generator is the host's own generator, a
//! splitmix64 stream seeded from the project's `seed` (the same splitmix64 step `ops::noise` uses), with
//! Lua's call forms (`math.random()` a float in [0, 1), `math.random(m)` an integer in 1..m, `math.random(m, n)`
//! in m..n; an empty interval or a non-integer bound is an error as in Lua); `math.randomseed` stays out. So the
//! same project and script queue the same ops on every run and `--dry-run` previews exactly what `run` writes;
//! a different layout wants a different project `seed`.

use crate::diag::{Code, Diagnostic, Diagnostics};
use crate::ops::{self, Applied, Op};
use crate::project::{Heights, Project, PropValue};
use crate::schema;
use crate::vocab_names as v;
use dc_mod_lua::mlua::{self, Lua, MultiValue, Table, Value};
use dc_mod_lua::{LuaError, LuaSandbox, SandboxLimits};
use serde_json::{json, Map, Value as Json};
use std::cell::RefCell;
use std::rc::Rc;

/// The default instruction budget of a generator (DC-DEFAULT: MAP-57): the mod default of 10 M stops a per-cell
/// loop over a 512 x 512 grid, so generators get 1e9.
pub const DEFAULT_BUDGET: u64 = 1_000_000_000;

/// The chunk name errors are reported under (`script:<line>: ...`).
pub const CHUNK_NAME: &str = "script";

/// Options of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    /// Lua instruction budget of the whole script (`--budget`).
    pub budget: u64,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            budget: DEFAULT_BUDGET,
        }
    }
}

/// What a generator produced, before anything is written: the op batch, the result of applying it, the project
/// and heights after it, and the script's `print` lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    /// The queued ops in script order (the CLI re-applies exactly these to the on-disk project).
    pub ops: Vec<Op>,
    /// The result of applying `ops` to the given project as one batch.
    pub applied: Applied,
    /// The project after the batch.
    pub project: Project,
    /// The heights after the batch.
    pub heights: Heights,
    /// `print` output, one entry per line.
    pub prints: Vec<String>,
}

/// Run a generator and apply its ops as one batch: the signature fixed in stage 1. See [`generate`] for the full
/// result (the ops, the project after the batch, the prints).
pub fn run(
    project: &Project,
    heights: &Heights,
    script: &str,
    opts: &RunOptions,
) -> Result<Applied, Diagnostics> {
    generate(project, heights, script, opts).map(|g| g.applied)
}

/// Run a generator in the sandbox, collect its ops and apply them as one batch to copies of `project` and
/// `heights`. `Err` when the script fails (one diagnostic with the script line) or the batch is refused.
pub fn generate(
    project: &Project,
    heights: &Heights,
    script: &str,
    opts: &RunOptions,
) -> Result<Generated, Diagnostics> {
    let state = Rc::new(RefCell::new(State {
        project: project.clone(),
        heights: heights.clone(),
        ops: Vec::new(),
        refusal: None,
    }));
    let setup = |e: LuaError| {
        Diagnostics::one(Diagnostic::new(
            Code::ScriptError,
            CHUNK_NAME,
            format!("sandbox setup failed: {e}"),
        ))
    };
    // SPEC: MAPKIT_PLAN.md section 7: the mod sandbox with its libraries, the generator budget on top (MAP-57).
    let sb = LuaSandbox::with_limits(SandboxLimits::with_instruction_limit(opts.budget.max(1)))
        .map_err(setup)?;
    let env = sb.create_env("mapkit", None).map_err(setup)?;
    let map =
        build_map_table(sb.lua(), &state).map_err(|e| setup(LuaError::Runtime(e.to_string())))?;
    env.table()
        .raw_set("map", map)
        .map_err(|e| setup(LuaError::Runtime(e.to_string())))?;
    // DC-DEFAULT: MAP-56: `math.random` is seeded from the project (the same script gives the same ops).
    seeded_random(sb.lua(), env.table(), project.seed)
        .map_err(|e| setup(LuaError::Runtime(e.to_string())))?;
    let func = sb
        .load_text(&env, CHUNK_NAME, script.as_bytes())
        .map_err(|e| failure(&state, e))?;
    let outcome = sb.call(&func, MultiValue::new());
    let prints = env.logs();
    if let Err(e) = outcome {
        return Err(failure(&state, e));
    }
    let ops = state.borrow().ops.clone();
    let mut p = project.clone();
    let mut h = heights.clone();
    // DC-DEFAULT: MAP-56: the script is one batch; nothing is kept from a script that fails.
    let applied = ops::apply(&mut p, &mut h, &ops)?;
    Ok(Generated {
        ops,
        applied,
        project: p,
        heights: h,
        prints,
    })
}

/// The working copy every `map.*` call edits, the queue, and the last refused op (to answer with its code).
struct State {
    project: Project,
    heights: Heights,
    ops: Vec<Op>,
    refusal: Option<(i32, Diagnostic)>,
}

type Shared = Rc<RefCell<State>>;

/// The script failure as one diagnostic: the budget code for an exhausted budget, a refused op's own code and
/// path when that is what stopped the script, else `script.error`; the path names the script line.
fn failure(state: &Shared, e: LuaError) -> Diagnostics {
    let d = match e {
        LuaError::InstructionLimit(m) => Diagnostic::new(Code::ScriptBudget, CHUNK_NAME, m).with_fix(
            "raise --budget, or use the bulk terrain ops (map.terrain.gen / smooth / raise) instead of a per-cell loop",
        ),
        LuaError::Syntax(m) => {
            let (path, message) = split_pos(&m);
            Diagnostic::new(Code::ScriptError, path, message)
                .with_fix("the script must be Lua 5.4 text")
        }
        LuaError::Runtime(m)
        | LuaError::MemoryLimit(m)
        | LuaError::CallDepth(m)
        | LuaError::Forbidden(m) => {
            let refusal = state.borrow().refusal.clone();
            match refusal {
                Some((line, r)) if m.contains(&r.message) => {
                    let field = r.path.strip_prefix("ops[0]").unwrap_or(&r.path);
                    Diagnostic {
                        path: format!("{CHUNK_NAME}:{line}{field}"),
                        ..r
                    }
                }
                _ => {
                    let (path, message) = split_pos(&m);
                    Diagnostic::new(Code::ScriptError, path, message)
                }
            }
        }
    };
    Diagnostics::one(d)
}

/// `(script:<n>, message)` when the message carries a `script:<n>:` position (the position moves into the path
/// and leaves the message), else `(script, message)`.
fn split_pos(message: &str) -> (String, String) {
    let prefix = format!("{CHUNK_NAME}:");
    if let Some(i) = message.find(&prefix) {
        let rest = &message[i + prefix.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            let mut cleaned = message.to_string();
            let pos = format!("{prefix}{digits}: ");
            if message[..i].trim().is_empty() {
                cleaned = message[i..]
                    .strip_prefix(&pos)
                    .unwrap_or(&message[i..])
                    .to_string();
            }
            return (format!("{CHUNK_NAME}:{digits}"), cleaned);
        }
    }
    (CHUNK_NAME.to_string(), message.to_string())
}

/// The script line of the innermost Lua frame (0 when none).
fn current_line(lua: &Lua) -> i32 {
    for level in 0..32 {
        let Some(d) = lua.inspect_stack(level) else {
            break;
        };
        if d.source().what != "C" {
            let line = d.curr_line();
            if line > 0 {
                return line;
            }
        }
    }
    0
}

// ---------------------------------------------------------------------------------------------------------------
// Lua <-> JSON (ops are built from JSON so the script sees exactly the `apply` vocabulary)
// ---------------------------------------------------------------------------------------------------------------

const MAX_DEPTH: usize = 32;

/// A Lua value as JSON: a sequence becomes an array, `{x, y, z}` keys become a vec3 array, an empty table an
/// empty object, other tables objects with text keys.
fn lua_json(v: &Value, depth: usize) -> Result<Json, String> {
    Ok(match v {
        Value::Nil => Json::Null,
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(i) => json!(*i),
        Value::Number(n) => {
            if !n.is_finite() {
                return Err("a number must be finite".into());
            }
            json!(*n)
        }
        Value::String(s) => Json::String(s.to_string_lossy()),
        Value::Table(t) => {
            if depth >= MAX_DEPTH {
                return Err("table nesting too deep".into());
            }
            let mut entries: Vec<(Value, Value)> = Vec::new();
            for pair in t.pairs::<Value, Value>() {
                entries.push(pair.map_err(|e| e.to_string())?);
            }
            if entries.is_empty() {
                return Ok(Json::Object(Map::new()));
            }
            let all_int = entries.iter().all(|(k, _)| matches!(k, Value::Integer(_)));
            if all_int {
                entries.sort_by_key(|(k, _)| match k {
                    Value::Integer(i) => *i,
                    _ => 0,
                });
                let sequential = entries
                    .iter()
                    .enumerate()
                    .all(|(i, (k, _))| matches!(k, Value::Integer(n) if *n == i as i64 + 1));
                if !sequential {
                    return Err("a list must be indexed 1..n".into());
                }
                let mut out = Vec::with_capacity(entries.len());
                for (_, val) in &entries {
                    out.push(lua_json(val, depth + 1)?);
                }
                return Ok(Json::Array(out));
            }
            let mut out = Map::new();
            for (k, val) in &entries {
                let key = match k {
                    Value::String(s) => s.to_string_lossy(),
                    other => {
                        return Err(format!(
                            "table keys must be text or 1..n, not {}",
                            other.type_name()
                        ))
                    }
                };
                out.insert(key, lua_json(val, depth + 1)?);
            }
            if out.len() == 3
                && ["x", "y", "z"]
                    .iter()
                    .all(|k| out.get(*k).is_some_and(Json::is_number))
            {
                return Ok(json!([out["x"], out["y"], out["z"]]));
            }
            Json::Object(out)
        }
        other => {
            return Err(format!(
                "a {} cannot be an argument of a map call",
                other.type_name()
            ))
        }
    })
}

/// A JSON value as a fresh Lua value (arrays as 1..n sequences, objects as text-keyed tables).
fn json_lua(lua: &Lua, j: &Json) -> mlua::Result<Value> {
    Ok(match j {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Number(n.as_f64().unwrap_or(0.0))
            }
        }
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Array(a) => {
            let t = lua.create_table()?;
            for (i, item) in a.iter().enumerate() {
                t.raw_set(i as i64 + 1, json_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        Json::Object(o) => {
            let t = lua.create_table()?;
            for (k, item) in o {
                t.raw_set(k.as_str(), json_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
    })
}

fn arg_object(args: &[Value], i: usize) -> Result<Map<String, Json>, String> {
    match args.get(i) {
        Some(v @ Value::Table(_)) => match lua_json(v, 0)? {
            Json::Object(m) => Ok(m),
            _ => Err(format!("argument #{} must be a table of fields", i + 1)),
        },
        _ => Err(format!("argument #{} must be a table of fields", i + 1)),
    }
}

fn opt_object(args: &[Value], i: usize) -> Result<Map<String, Json>, String> {
    match args.get(i) {
        None | Some(Value::Nil) => Ok(Map::new()),
        _ => arg_object(args, i),
    }
}

fn arg_text(args: &[Value], i: usize) -> Result<String, String> {
    match args.get(i) {
        Some(Value::String(s)) => Ok(s.to_string_lossy()),
        _ => Err(format!("argument #{} must be text", i + 1)),
    }
}

fn arg_num(args: &[Value], i: usize) -> Result<f32, String> {
    match args.get(i) {
        Some(Value::Integer(n)) => Ok(*n as f32),
        Some(Value::Number(n)) if n.is_finite() => Ok(*n as f32),
        _ => Err(format!("argument #{} must be a number", i + 1)),
    }
}

fn arg_json(args: &[Value], i: usize) -> Result<Json, String> {
    lua_json(args.get(i).unwrap_or(&Value::Nil), 0)
}

/// Build an op from its fields, refusing unknown field names (a typo must not pass silently).
fn op_from(name: &str, mut fields: Map<String, Json>) -> Result<Op, String> {
    let known = schema::op_fields(name).ok_or_else(|| format!("no op `{name}`"))?;
    for k in fields.keys() {
        if !known.iter().any(|(f, _, _)| f == k) {
            let list: Vec<&str> = known.iter().map(|(f, _, _)| *f).collect();
            return Err(format!(
                "unknown field `{k}` (the fields of `{name}` are {})",
                list.join(", ")
            ));
        }
    }
    fields.insert("op".into(), Json::String(name.into()));
    serde_json::from_value::<Op>(Json::Object(fields)).map_err(|e| e.to_string())
}

/// `rot` is the short name of a solid's `rot_y_deg` in scripts; a table carrying both is refused (the unknown
/// field rule: nothing is dropped silently).
fn solid_fields(mut t: Map<String, Json>) -> Result<Map<String, Json>, String> {
    if let Some(r) = t.remove("rot") {
        if t.contains_key("rot_y_deg") {
            return Err("give `rot` or `rot_y_deg`, not both (they are the same field)".into());
        }
        t.insert("rot_y_deg".into(), r);
    }
    Ok(t)
}

/// Replace the sandbox's `math.random` by a host generator over a splitmix64 stream seeded from the project
/// (DC-DEFAULT: MAP-56): Lua's call forms, Lua's refusals (an empty interval, a bound without an integer
/// representation, more than two arguments). The state lives in the closure, so every `generate` starts over.
fn seeded_random(lua: &Lua, env: &Table, seed: u64) -> mlua::Result<()> {
    let math: Table = env.raw_get("math")?;
    let state = Rc::new(RefCell::new(seed));
    let random = lua.create_function(move |_, args: MultiValue| {
        let args: Vec<Value> = args.into_iter().collect();
        let bound = |v: &Value, which: &str| -> mlua::Result<i64> {
            match v {
                Value::Integer(i) => Ok(*i),
                Value::Number(n) if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e18 => {
                    Ok(*n as i64)
                }
                _ => Err(mlua::Error::runtime(format!(
                    "bad argument #{which} to 'random' (number has no integer representation)"
                ))),
            }
        };
        let r = ops::noise::splitmix64(&mut state.borrow_mut());
        let (lo, hi) = match args.as_slice() {
            // a float in [0, 1): the top 53 bits
            [] => return Ok(Value::Number((r >> 11) as f64 / (1u64 << 53) as f64)),
            [m] => (1, bound(m, "1")?),
            [m, n] => (bound(m, "1")?, bound(n, "2")?),
            _ => {
                return Err(mlua::Error::runtime(
                    "wrong number of arguments to 'random'",
                ))
            }
        };
        if lo > hi {
            return Err(mlua::Error::runtime(format!(
                "bad argument #{} to 'random' (interval is empty)",
                if args.len() == 1 { 1 } else { 2 }
            )));
        }
        // lo + r mod (hi - lo + 1), in 128 bits so the full i64 range cannot overflow
        let span = (hi as i128 - lo as i128 + 1) as u128;
        let v = lo as i128 + (r as u128 % span) as i128;
        Ok(Value::Integer(v as i64))
    })?;
    math.raw_set("random", random)?;
    math.raw_set("randomseed", Value::Nil)
}

// ---------------------------------------------------------------------------------------------------------------
// The `map` table
// ---------------------------------------------------------------------------------------------------------------

/// Apply one op to the working copy and queue it; the ids it produced on success.
fn queue(lua: &Lua, st: &Shared, op: Op) -> Result<Vec<String>, String> {
    let mut s = st.borrow_mut();
    let State {
        project,
        heights,
        ops: queued,
        refusal,
    } = &mut *s;
    match ops::apply(project, heights, std::slice::from_ref(&op)) {
        Ok(a) => {
            queued.push(op);
            Ok(a.applied.into_iter().flat_map(|x| x.ids).collect())
        }
        Err(d) => {
            let first = d
                .items
                .into_iter()
                .next()
                .expect("a refusal has a diagnostic");
            let msg = first.message.clone();
            *refusal = Some((current_line(lua), first));
            Err(msg)
        }
    }
}

fn first_id(ids: Vec<String>) -> Json {
    ids.into_iter()
        .next()
        .map(Json::String)
        .unwrap_or(Json::Null)
}

/// Drop a leading `self` when a call used `:` on the owner table.
fn strip_owner(owner: &Table, args: MultiValue) -> Vec<Value> {
    let mut v: Vec<Value> = args.into_iter().collect();
    if let Some(Value::Table(t)) = v.first() {
        if t.to_pointer() == owner.to_pointer() {
            v.remove(0);
        }
    }
    v
}

type Host = Rc<dyn Fn(&Lua, &Shared, &[Value]) -> Result<Json, String>>;

/// Install `owner.<name>` as a host function; failures become `script:<line>: map.<what>: <message>`.
fn host(
    lua: &Lua,
    owner: &Table,
    st: &Shared,
    name: &str,
    what: &'static str,
    f: Host,
) -> mlua::Result<()> {
    let owner_c = owner.clone();
    let st = Rc::clone(st);
    let func = lua.create_function(move |lua, args: MultiValue| {
        let args = strip_owner(&owner_c, args);
        match f(lua, &st, &args) {
            Ok(j) => json_lua(lua, &j),
            Err(m) => Err(mlua::Error::runtime(format!(
                "{CHUNK_NAME}:{}: map.{what}: {m}",
                current_line(lua)
            ))),
        }
    })?;
    owner.raw_set(name, func)
}

/// `map.query`'s `near`: the positional `{x, z, r}` (three numbers) or the named `{x = .., z = .., r = ..}` (the
/// MCP `get` tool's object form); anything else, a missing or an unknown key included, is refused with both
/// spellings in the message.
fn near_of(n: &Json) -> Result<(f32, f32, f32), String> {
    const FORMS: &str = "`near` is {x, z, r} (three numbers) or {x = .., z = .., r = ..}";
    match n {
        Json::Array(a) if a.len() == 3 && a.iter().all(Json::is_number) => Ok((
            a[0].as_f64().unwrap_or(0.0) as f32,
            a[1].as_f64().unwrap_or(0.0) as f32,
            a[2].as_f64().unwrap_or(0.0) as f32,
        )),
        Json::Object(o) => {
            if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "x" | "z" | "r")) {
                return Err(format!("unknown `near` field `{k}`: {FORMS}"));
            }
            let num = |k: &str| {
                o.get(k)
                    .and_then(Json::as_f64)
                    .ok_or_else(|| format!("`near` needs the number `{k}`: {FORMS}"))
            };
            Ok((num("x")? as f32, num("z")? as f32, num("r")? as f32))
        }
        _ => Err(FORMS.into()),
    }
}

fn object_json(o: &crate::project::MapObject) -> Json {
    serde_json::to_value(o).expect("object serialises")
}

fn info_json(s: &State) -> Json {
    let p = &s.project;
    let mut spawns = (0usize, 0usize, 0usize);
    for o in p.of_class(v::CLASS_PLAYER_SPAWN) {
        match o.text(v::PROP_TEAM).unwrap_or("") {
            "1" => spawns.0 += 1,
            "2" => spawns.1 += 1,
            _ => spawns.2 += 1,
        }
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    let mut any = false;
    let mut take = |q: [f32; 3]| {
        any = true;
        for i in 0..3 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    };
    for o in &p.objects {
        take(o.pos);
    }
    for sd in &p.solids {
        let h = [sd.size[0] / 2.0, sd.size[1] / 2.0, sd.size[2] / 2.0];
        take([sd.pos[0] - h[0], sd.pos[1] - h[1], sd.pos[2] - h[2]]);
        take([sd.pos[0] + h[0], sd.pos[1] + h[1], sd.pos[2] + h[2]]);
    }
    let terrain = p.terrain.as_ref().map(|t| {
        let (x0, x1) = t.span_x();
        let (z0, z1) = t.span_z();
        let (rlo, rhi) = s.heights.raw_range().unwrap_or((0, 0));
        take([x0, t.raw_to_y(rlo), z0]);
        take([x1, t.raw_to_y(rhi), z1]);
        json!({
            "cells_x": t.cells_x, "cells_z": t.cells_z, "cell_size": t.cell_size, "origin": t.origin,
            "min_x": x0, "max_x": x1, "min_z": z0, "max_z": z1,
            "min_y": t.raw_to_y(rlo), "max_y": t.raw_to_y(rhi),
        })
    });
    json!({
        "name": p.name, "key": p.key, "seed": p.seed,
        "counts": {
            "objects": p.objects.len(),
            "solids": p.solids.iter().filter(|x| x.brush.is_none()).count(),
            "brushes": p.solids.iter().filter(|x| x.brush.is_some()).count(),
            "prefabs": p.prefabs.len(), "imports": p.imports.len(), "materials": p.materials.len(),
        },
        "spawns": {"team1": spawns.0, "team2": spawns.1, "any": spawns.2},
        "modes": p.mod_meta.modes,
        "terrain": terrain,
        "extent": if any { json!({"min": lo, "max": hi}) } else { Json::Null },
        "pending_ops": s.ops.len(),
    })
}

fn build_map_table(lua: &Lua, st: &Shared) -> mlua::Result<Table> {
    let map = lua.create_table()?;
    let solid = lua.create_table()?;
    let terrain = lua.create_table()?;
    let material = lua.create_table()?;

    // constants
    map.raw_set("MODES", json_lua(lua, &json!(v::MODE_NAMES))?)?;
    map.raw_set("CLASSES", json_lua(lua, &json!(v::POINT_CLASSES))?)?;
    map.raw_set(
        "BRUSHES",
        json_lua(
            lua,
            &json!(["world_model", "kill_volume", "ladder", "breakable", "sky"]),
        )?,
    )?;
    map.raw_set(
        "PRESETS",
        json_lua(lua, &json!(["hills", "valley", "ridge"]))?,
    )?;
    map.raw_set(
        "FALLOFFS",
        json_lua(lua, &json!(["smooth", "linear", "flat"]))?,
    )?;
    // DC-DEFAULT: MAP-56: the project's seed, the one `math.random` is seeded from
    map.raw_set("seed", st.borrow().project.seed as i64)?;

    // objects
    host(
        lua,
        &map,
        st,
        "add",
        "add",
        Rc::new(|lua, st, args| {
            let op = op_from("add", arg_object(args, 0)?)?;
            Ok(first_id(queue(lua, st, op)?))
        }),
    )?;
    host(
        lua,
        &map,
        st,
        "remove",
        "remove",
        Rc::new(|lua, st, args| {
            let op = Op::Remove {
                id: arg_text(args, 0)?,
            };
            Ok(first_id(queue(lua, st, op)?))
        }),
    )?;
    host(
        lua,
        &map,
        st,
        "move",
        "move",
        Rc::new(|lua, st, args| {
            let id = arg_text(args, 0)?;
            let mut fields = match arg_json(args, 1)? {
                Json::Array(a) if a.len() == 3 => {
                    let mut m = Map::new();
                    m.insert("pos".into(), Json::Array(a));
                    m
                }
                Json::Object(m)
                    if !m.is_empty()
                        && m.keys().all(|k| matches!(k.as_str(), "pos" | "delta" | "ground")) =>
                {
                    m
                }
                _ => {
                    return Err(
                        "argument #2 must be a position {x, y, z}, {delta = {x, y, z}} or {ground = true}"
                            .into(),
                    )
                }
            };
            fields.insert("id".into(), Json::String(id));
            let op = op_from("move", fields)?;
            Ok(first_id(queue(lua, st, op)?))
        }),
    )?;
    host(
        lua,
        &map,
        st,
        "set",
        "set",
        Rc::new(|lua, st, args| {
            let id = arg_text(args, 0)?;
            let prop = arg_text(args, 1)?;
            let value: PropValue = serde_json::from_value(arg_json(args, 2)?).map_err(|_| {
                "argument #3 must be text, a number, a bool, {x, y, z} or {rgb = {r, g, b}}"
                    .to_string()
            })?;
            Ok(first_id(queue(lua, st, Op::Set { id, prop, value })?))
        }),
    )?;
    host(
        lua,
        &map,
        st,
        "rename",
        "rename",
        Rc::new(|lua, st, args| {
            let op = Op::Rename {
                id: arg_text(args, 0)?,
                name: arg_text(args, 1)?,
            };
            Ok(first_id(queue(lua, st, op)?))
        }),
    )?;

    // solids
    for (name, what) in [
        ("box", "solid.box"),
        ("wedge", "solid.wedge"),
        ("cylinder", "solid.cylinder"),
        ("prism", "solid.prism"),
    ] {
        host(
            lua,
            &solid,
            st,
            name,
            what,
            Rc::new(move |lua, st, args| {
                let mut t = solid_fields(arg_object(args, 0)?)?;
                let kind = match name {
                    "box" => json!("box"),
                    "wedge" => json!("wedge"),
                    "cylinder" => {
                        let sides = t.remove("sides").unwrap_or(json!(8));
                        json!({"cylinder": {"sides": sides}})
                    }
                    _ => {
                        let points = t
                            .remove("points")
                            .ok_or_else(|| "a prism needs `points = {{x, z}, ...}`".to_string())?;
                        json!({"prism": {"points": points}})
                    }
                };
                t.insert("kind".into(), kind);
                let op = op_from("solid.add", t)?;
                Ok(first_id(queue(lua, st, op)?))
            }),
        )?;
    }
    host(
        lua,
        &solid,
        st,
        "set",
        "solid.set",
        Rc::new(|lua, st, args| {
            let id = arg_text(args, 0)?;
            let mut t = solid_fields(arg_object(args, 1)?)?;
            t.insert("id".into(), Json::String(id));
            let op = op_from("solid.set", t)?;
            Ok(first_id(queue(lua, st, op)?))
        }),
    )?;
    map.raw_set("solid", solid)?;

    // fog, sun, ambient and sky from a generator, as the `settings.set` op takes them (MAP-98)
    host(
        lua,
        &map,
        st,
        "settings",
        "settings.set",
        Rc::new(|lua, st, args| {
            let t = arg_object(args, 0)?;
            let op = op_from("settings.set", t)?;
            queue(lua, st, op)?;
            Ok(Json::Null)
        }),
    )?;

    // a generator's first line, so running it twice does not place everything twice (MAP-94)
    host(
        lua,
        &map,
        st,
        "clear",
        "clear",
        Rc::new(|lua, st, args| {
            let t = opt_object(args, 0)?;
            let op = op_from("clear", t)?;
            Ok(Json::from(queue(lua, st, op)?.len()))
        }),
    )?;

    // prefabs, imports, materials
    host(
        lua,
        &map,
        st,
        "prefab",
        "prefab",
        Rc::new(|lua, st, args| {
            let name = arg_text(args, 0)?;
            let mut t = solid_fields(arg_object(args, 1)?)?;
            t.insert("prefab".into(), Json::String(name));
            let op = op_from("prefab.add", t)?;
            Ok(first_id(queue(lua, st, op)?))
        }),
    )?;
    host(
        lua,
        &map,
        st,
        "import",
        "import",
        Rc::new(|lua, st, args| {
            let op = op_from("import.add", solid_fields(arg_object(args, 0)?)?)?;
            Ok(first_id(queue(lua, st, op)?))
        }),
    )?;
    host(
        lua,
        &material,
        st,
        "set",
        "material.set",
        Rc::new(|lua, st, args| {
            let mut t = arg_object(args, 0)?;
            if let Some(Json::String(path)) = t.get("texture").cloned() {
                t.insert("texture".into(), json!({"own": path}));
            }
            let op = op_from("material.set", t)?;
            Ok(first_id(queue(lua, st, op)?))
        }),
    )?;
    map.raw_set("material", material)?;

    // terrain
    for (name, op_name) in [
        ("gen", "terrain.gen"),
        ("noise", "terrain.gen"),
        ("raise", "terrain.raise"),
        ("lower", "terrain.lower"),
        ("flatten", "terrain.flatten"),
        ("smooth", "terrain.smooth"),
    ] {
        host(
            lua,
            &terrain,
            st,
            name,
            op_name,
            Rc::new(move |lua, st, args| {
                let mut t = opt_object(args, 0)?;
                if op_name == "terrain.gen" {
                    t.entry("preset").or_insert(json!("hills"));
                }
                let op = op_from(op_name, t)?;
                queue(lua, st, op)?;
                Ok(Json::Null)
            }),
        )?;
    }
    host(
        lua,
        &terrain,
        st,
        "height",
        "terrain.height",
        Rc::new(|_, st, args| {
            let (x, z) = (arg_num(args, 0)?, arg_num(args, 1)?);
            let s = st.borrow();
            Ok(s.project
                .terrain
                .as_ref()
                .and_then(|t| s.heights.sample(t, x, z))
                .map_or(Json::Null, |y| json!(y)))
        }),
    )?;
    map.raw_set("terrain", terrain)?;
    host(
        lua,
        &map,
        st,
        "ground",
        "ground",
        Rc::new(|_, st, args| {
            let (x, z) = (arg_num(args, 0)?, arg_num(args, 1)?);
            let s = st.borrow();
            Ok(ops::ground_height(&s.project, &s.heights, x, z).map_or(Json::Null, |y| json!(y)))
        }),
    )?;

    // queries
    host(
        lua,
        &map,
        st,
        "query",
        "query",
        Rc::new(|_, st, args| {
            let q = opt_object(args, 0)?;
            for k in q.keys() {
                if !matches!(k.as_str(), "class" | "id" | "near") {
                    return Err(format!("unknown query field `{k}` (class, id, near)"));
                }
            }
            let class = q.get("class").and_then(Json::as_str).map(str::to_string);
            let id = q.get("id").and_then(Json::as_str).map(str::to_string);
            let near = match q.get("near") {
                None => None,
                Some(n) => Some(near_of(n)?),
            };
            let s = st.borrow();
            let list: Vec<Json> = s
                .project
                .objects
                .iter()
                .filter(|o| class.as_deref().is_none_or(|c| o.class == c))
                .filter(|o| id.as_deref().is_none_or(|i| o.id == i))
                .filter(|o| {
                    near.is_none_or(|(x, z, r)| {
                        let dx = o.pos[0] - x;
                        let dz = o.pos[2] - z;
                        dx * dx + dz * dz <= r * r
                    })
                })
                .map(object_json)
                .collect();
            Ok(Json::Array(list))
        }),
    )?;
    host(
        lua,
        &map,
        st,
        "solids",
        "solids",
        Rc::new(|_, st, args| {
            let q = opt_object(args, 0)?;
            for k in q.keys() {
                if !matches!(k.as_str(), "group" | "brush") {
                    return Err(format!("unknown solids field `{k}` (group, brush)"));
                }
            }
            let group = q.get("group").and_then(Json::as_str).map(str::to_string);
            let brush = q.get("brush").and_then(Json::as_str).map(str::to_string);
            let s = st.borrow();
            let list: Vec<Json> = s
                .project
                .solids
                .iter()
                .filter(|x| {
                    group
                        .as_deref()
                        .is_none_or(|g| x.group.as_deref() == Some(g))
                })
                .filter(|x| {
                    brush.as_deref().is_none_or(|b| {
                        x.brush
                            .map(|r| serde_json::to_value(r).ok() == Some(json!(b)))
                            .unwrap_or(false)
                    })
                })
                .map(|x| serde_json::to_value(x).expect("solid serialises"))
                .collect();
            Ok(Json::Array(list))
        }),
    )?;
    host(
        lua,
        &map,
        st,
        "info",
        "info",
        Rc::new(|_, st, _| Ok(info_json(&st.borrow()))),
    )?;
    host(
        lua,
        &map,
        st,
        "ops",
        "ops",
        Rc::new(|_, st, _| Ok(json!(st.borrow().ops.len()))),
    )?;
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::MAP_FILE;
    use crate::template::{self, Template};
    use std::path::PathBuf;

    fn flat() -> (Project, Heights) {
        let made = template::make(Template::Flat, "Script Test", "tests", 1);
        (made.project, made.heights)
    }

    fn gen(script: &str) -> Result<Generated, Diagnostics> {
        let (p, h) = flat();
        generate(&p, &h, script, &RunOptions::default())
    }

    #[test]
    fn a_script_queues_ops_applies_one_batch_and_queries_see_pending_state() {
        let g = gen(r#"
            local before = #map.query{class = "dc.item_spawn"}
            local id = map.add{class = "dc.item_spawn", pos = {100, 0, 100}, ground = true, props = {modes = ""}}
            assert(id:match("^item_spawn_%d+$"), id)
            assert(#map.query{class = "dc.item_spawn"} == before + 1, "query sees the pending add")
            local got = map.query{id = id}[1]
            assert(math.abs(got.pos[2]) < 1, "grounded on the flat floor: " .. tostring(got.pos[2]))
            map.move(id, {x = 200, y = 0, z = 200})
            map.set(id, "modes", "TDM,FFA")
            map.rename(id, "crate_drop")
            local box = map.solid.box{pos = {200, 0, 300}, size = {64, 64, 64}, material = "block", group = "cover", ground = true}
            local cyl = map.solid.cylinder{sides = 6, pos = {-200, 0, 300}, size = {64, 128, 64}, material = "block"}
            map.solid.set(cyl, {rot = 15})
            map.material.set{id = "stone", texture = "assets/ground.png", tile = 64}
            map.terrain.raise{x = 0, z = 0, radius = 300, amount = 20}
            assert(map.terrain.height(0, 0) > 10, "terrain rose")
            assert(math.abs(map.ground(200, 300) - 64) < 1, "ground sees the box top: " .. tostring(map.ground(200, 300)))
            print("ops", map.ops())
            assert(map.info().pending_ops == 9)
            assert(#map.MODES > 0 and #map.CLASSES > 0 and #map.BRUSHES == 5)
        "#)
        .unwrap_or_else(|e| panic!("{:?}", e.items));
        assert_eq!(g.ops.len(), 9);
        assert_eq!(g.applied.applied.len(), 9);
        assert_eq!(g.prints, vec!["ops\t9".to_string()]);
        assert_eq!(g.applied.changed, vec!["map.json", "terrain.r16"]);
        let o = g
            .project
            .objects
            .iter()
            .find(|o| o.name == "crate_drop")
            .unwrap();
        assert_eq!(o.pos, [200.0, 0.0, 200.0]);
        assert_eq!(o.text("modes"), Some("TDM,FFA"));
        assert!(g.project.material("stone").is_some());
        assert_eq!(
            g.project
                .solids
                .iter()
                .filter(|s| s.group.as_deref() == Some("cover"))
                .count(),
            1
        );
    }

    #[test]
    fn a_refused_op_names_the_script_line_and_its_own_code_and_changes_nothing() {
        let e = gen("local n = 1\nmap.add{class = \"dc.nope\", pos = {0, 0, 0}}\n").unwrap_err();
        let d = &e.items[0];
        assert_eq!(d.code, Code::ObjectClass, "{d}");
        assert_eq!(d.path, "script:2.class");
        let e =
            gen("map.add{class = \"dc.item_spawn\", pos = {0, 0, 0}, postion = 1}").unwrap_err();
        assert_eq!(e.items[0].code, Code::ScriptError);
        assert!(
            e.items[0].message.contains("unknown field `postion`"),
            "{}",
            e.items[0].message
        );
        assert_eq!(e.items[0].path, "script:1");
        let e =
            gen("map.add{class = \"dc.item_spawn\", pos = {0, 0, 0}}\n\nmap.remove(\"solid_999\")")
                .unwrap_err();
        assert_eq!(e.items[0].code, Code::IdUnknown);
        assert_eq!(e.items[0].path, "script:3.id");
        let e = gen("map.add{class = \"dc.item_spawn\", pos = {0, 0, 0}}\nerror(\"stop here\")")
            .unwrap_err();
        assert_eq!(e.items[0].code, Code::ScriptError);
        assert_eq!(e.items[0].path, "script:2");
        assert!(e.items[0].message.contains("stop here"));
    }

    #[test]
    fn pcall_keeps_the_rest_of_the_script_and_a_caught_refusal_queues_nothing() {
        let g = gen(r#"
            local ok, err = pcall(function() map.remove("solid_999") end)
            assert(not ok and tostring(err):find("solid_999"), tostring(err))
            map.add{class = "dc.item_spawn", pos = {0, 0, 0}}
        "#)
        .unwrap();
        assert_eq!(g.ops.len(), 1);
        assert!(matches!(g.ops[0], Op::Add { .. }));
    }

    #[test]
    fn the_budget_stops_a_runaway_loop_and_syntax_errors_carry_the_line() {
        let (p, h) = flat();
        let e = generate(&p, &h, "while true do end", &RunOptions { budget: 100_000 }).unwrap_err();
        assert_eq!(e.items[0].code, Code::ScriptBudget);
        let e = gen("local x = 1\nlocal y = = 2").unwrap_err();
        assert_eq!(e.items[0].code, Code::ScriptError);
        assert_eq!(e.items[0].path, "script:2");
        // the sandbox keeps os / io / load out
        let e = gen("return os.time()").unwrap_err();
        assert!(e.items[0].message.contains("nil"), "{}", e.items[0].message);
        let e = gen("return io.open('x')").unwrap_err();
        assert!(e.items[0].message.contains("nil"), "{}", e.items[0].message);
        let e = gen("return load('return 1')()").unwrap_err();
        assert!(e.items[0].message.contains("nil"), "{}", e.items[0].message);
    }

    #[test]
    fn colon_calls_and_xyz_tables_are_accepted_and_run_matches_generate() {
        let g = gen(r#"
            local id = map.solid:box{pos = {x = 0, y = 0, z = 0}, size = {x = 32, y = 32, z = 32}, material = "block"}
            map:move(id, {delta = {10, 0, 0}})
            map.terrain:smooth()
        "#)
        .unwrap_or_else(|e| panic!("{:?}", e.items));
        assert_eq!(g.ops.len(), 3);
        let (p, h) = flat();
        let a = run(
            &p,
            &h,
            "map.add{class = 'dc.item_spawn', pos = {1, 2, 3}}",
            &RunOptions::default(),
        )
        .unwrap();
        assert_eq!(a.applied[0].op, "add");
    }

    /// Regression (review C22): `math.random` is the host's seeded generator: the same script queues the same ops
    /// on every run, a dry run previews exactly what the real run writes, Lua's call forms and refusals hold,
    /// `map.seed` is the project's seed and another seed gives another layout.
    #[test]
    fn math_random_is_seeded_from_the_project_and_keeps_luas_call_forms() {
        let script = r#"
            assert(math.randomseed == nil, "no reseeding")
            assert(map.seed == 1, "map.seed is the project seed: " .. tostring(map.seed))
            local f = math.random()
            assert(f >= 0 and f < 1, "a float in [0, 1): " .. tostring(f))
            assert(math.type(f) == "float")
            for _ = 1, 200 do
                local a = math.random(6)
                assert(math.type(a) == "integer" and a >= 1 and a <= 6, tostring(a))
                local b = math.random(-3, 3)
                assert(math.type(b) == "integer" and b >= -3 and b <= 3, tostring(b))
                assert(math.random(5, 5) == 5)
                assert(math.random(7.0) >= 1, "an integral float is a bound")
            end
            local lo = math.random(math.mininteger, math.maxinteger)
            assert(math.type(lo) == "integer", "the full range does not overflow")
            for i = 1, 6 do
                local a = math.random(-1500, 1500)
                local z = math.random(-1500, 1500)
                map.solid.box{pos = {a, 0, z}, size = {64, 64, 64}, material = "block", ground = true}
            end
            map.add{class = "dc.item_spawn", pos = {math.random(100), 0, math.random(10, 20)}, props = {modes = ""}}
            print(math.random(1, 1000000))
            assert(not pcall(math.random, 0), "an empty interval is refused")
            assert(not pcall(math.random, 3, 1), "an empty interval is refused")
            assert(not pcall(math.random, 1.5), "a non-integer bound is refused")
            assert(not pcall(math.random, 1, 2, 3), "three arguments are refused")
            local ok, e = pcall(math.random, 0)
            assert(tostring(e):find("interval is empty", 1, true), tostring(e))
        "#;
        let g = gen(script).unwrap_or_else(|e| panic!("{:?}", e.items));
        let g2 = gen(script).unwrap();
        assert_eq!(g.ops, g2.ops, "two runs queue the same ops");
        assert_eq!(g.prints, g2.prints);
        assert_eq!(g.ops.len(), 7);
        let (p, h) = flat();
        assert_eq!(p.seed, 1);
        // the dry run and the real run of the CLI path agree on everything but `dry_run`
        let dir = example_dir("random");
        let made = template::make(Template::Flat, "Random", "tests", 1);
        crate::cli::write_project(&dir, &made, Template::Flat).unwrap();
        let mut err = Vec::new();
        let dry =
            crate::cli::run_script(&dir, &p, &h, script, &RunOptions::default(), true, &mut err)
                .unwrap_or_else(|e| panic!("{:?}", e.items));
        let real = crate::cli::run_script(
            &dir,
            &p,
            &h,
            script,
            &RunOptions::default(),
            false,
            &mut err,
        )
        .unwrap_or_else(|e| panic!("{:?}", e.items));
        assert_eq!(dry.result["dry_run"], true);
        assert_eq!(real.result["dry_run"], false);
        assert_eq!(dry.result["applied"], real.result["applied"]);
        assert_eq!(dry.result["prints"], real.result["prints"]);
        let (p2, _) = Project::open(&dir).unwrap();
        assert_eq!(p2.solids.len(), p.solids.len() + 6);
        let _ = std::fs::remove_dir_all(&dir);
        // another project seed is another stream
        let mut p3 = p.clone();
        p3.seed = 2;
        let g3 = generate(
            &p3,
            &h,
            script.replace("map.seed == 1", "map.seed == 2").as_str(),
            &RunOptions::default(),
        )
        .unwrap_or_else(|e| panic!("{:?}", e.items));
        assert_ne!(g.ops, g3.ops, "seed 2 lays the boxes out differently");
        assert_ne!(g.prints, g3.prints);
    }

    /// Regression (review C28): `near` takes the named form too, and a solid table with both `rot` and
    /// `rot_y_deg` is refused instead of dropping `rot`.
    #[test]
    fn query_near_takes_both_forms_and_rot_next_to_rot_y_deg_is_refused() {
        let g = gen(r#"
            local a = map.query{near = {0, -1024, 10}}
            local b = map.query{near = {x = 0, z = -1024, r = 10}}
            assert(#a == 1, "the positional form finds the bomb site: " .. #a)
            assert(#b == 1, "the named form finds the same: " .. #b)
            assert(a[1].id == b[1].id)
            assert(#map.query{near = {r = 10, x = 0, z = -1024}} == 1, "key order does not matter")
            local ok, e = pcall(map.query, {near = {x = 0, z = -1024, radius = 10}})
            assert(not ok and tostring(e):find("unknown `near` field `radius`", 1, true), tostring(e))
            ok, e = pcall(map.query, {near = {x = 0, z = -1024}})
            assert(not ok and tostring(e):find("needs the number `r`", 1, true), tostring(e))
            ok, e = pcall(map.query, {near = "0,-1024,10"})
            assert(not ok and tostring(e):find("{x = .., z = .., r = ..}", 1, true), tostring(e))
            local id = map.solid.box{pos = {0, 0, 0}, size = {32, 32, 32}, material = "block", rot = 45}
            assert(map.query{id = id} ~= nil)
            ok, e = pcall(map.solid.box, {pos = {0, 0, 0}, size = {32, 32, 32}, material = "block", rot = 45, rot_y_deg = 10})
            assert(not ok and tostring(e):find("`rot` or `rot_y_deg`, not both", 1, true), tostring(e))
            ok, e = pcall(map.solid.set, id, {rot = 1, rot_y_deg = 2})
            assert(not ok and tostring(e):find("not both", 1, true), tostring(e))
            map.solid.set(id, {rot_y_deg = 30})
            print(id)
        "#)
        .unwrap_or_else(|e| panic!("{:?}", e.items));
        assert_eq!(g.ops.len(), 2);
        let id = g.prints.last().unwrap();
        let s = g.project.solids.iter().find(|s| &s.id == id).unwrap();
        assert_eq!(s.rot_y_deg, 30.0);
        let refused = gen("map.solid.box{pos = {0, 0, 0}, size = {32, 32, 32}, material = 'block', rot = 45, rot_y_deg = 10}")
            .unwrap_err();
        assert_eq!(refused.items[0].code, crate::diag::Code::ScriptError);
        assert!(
            refused.items[0].message.contains("not both"),
            "{}",
            refused.items[0].message
        );
        assert_eq!(refused.items[0].path, "script:1");
    }

    fn example_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dc-mapkit-example-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The committed generators: each runs against the flat template, applies through the CLI path and validates
    /// without errors; running twice gives the same ops (determinism).
    #[test]
    fn the_example_generators_run_against_the_flat_template_and_validate() {
        let examples: [(&str, &str); 3] = [
            (
                "arena",
                include_str!("../../../examples/mapkit/arena_symmetric.lua"),
            ),
            (
                "canyon",
                include_str!("../../../examples/mapkit/canyon_river.lua"),
            ),
            (
                "tower",
                include_str!("../../../examples/mapkit/koth_tower.lua"),
            ),
        ];
        for (name, text) in examples {
            let dir = example_dir(name);
            let made = template::make(Template::Flat, "Example", "tests", 7);
            crate::cli::write_project(&dir, &made, Template::Flat).unwrap();
            let (p, h) = Project::open(&dir).unwrap();
            let g = generate(&p, &h, text, &RunOptions::default())
                .unwrap_or_else(|e| panic!("{name}: {:?}", e.items));
            let g2 = generate(&p, &h, text, &RunOptions::default()).unwrap();
            assert_eq!(g.ops, g2.ops, "{name}: deterministic");
            assert!(g.ops.len() > 10, "{name}: {} ops", g.ops.len());
            let mut err = Vec::new();
            let o = crate::cli::apply_to_project(&dir, &g.ops, false, "run", &mut err)
                .unwrap_or_else(|e| panic!("{name}: {:?}", e.items));
            assert!(!o.diagnostics.has_errors());
            let (p2, h2) = Project::open(&dir).unwrap();
            assert_eq!(
                p2, g.project,
                "{name}: the CLI path reproduces the generator's project"
            );
            assert_eq!(h2, g.heights);
            let d = crate::validate::validate(&dir, &p2, &h2);
            assert!(!d.has_errors(), "{name}: {:?}", d.items);
            assert!(dir.join(MAP_FILE).exists());
            match name {
                "canyon" => {
                    let (lo, _) = h2.raw_range().unwrap();
                    let t = p2.terrain.as_ref().unwrap();
                    assert!(t.raw_to_y(lo) < -20.0, "the river bed is below the plain");
                    let slab = p2
                        .solids
                        .iter()
                        .find(|s| s.brush == Some(crate::project::BrushRole::KillVolume))
                        .unwrap();
                    assert!(
                        slab.pos[1] + slab.size[1] / 2.0 < t.raw_to_y(lo),
                        "the kill slab stays below the terrain"
                    );
                }
                "tower" => assert!(p2.solids.len() > p.solids.len() + 8),
                _ => assert!(p2.solids.len() > p.solids.len() + 8),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

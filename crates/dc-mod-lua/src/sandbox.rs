//! Real Lua 5.4 (via `mlua`, vendored) wrapped in the DeCombat mod sandbox.
//!
//! One `mlua::Lua` state is shared by every mod of a host so that `mod.exports` functions can be
//! called across mods. Isolation is per **environment table**: each mod gets its own `_ENV` with
//! fresh copies of the safe standard libraries, so monkey-patching `string` or writing `_G.x` in one
//! mod is invisible to the others.
//!
//! Limits enforced here:
//! - text chunks only (`ChunkMode::Text` plus an explicit signature check),
//! - no `os`, `io`, `debug`, `package`, `load`, `loadstring`, `dofile`, `loadfile`, `collectgarbage`,
//!   `string.dump`, `math.randomseed`; `require` is replaced by a resolver scoped to `mods/<id>/`,
//! - an instruction budget per host->Lua call, counted by a `set_hook` count hook; once spent the
//!   budget stays spent, and the sandbox `pcall`/`xpcall` re-raise, so mods cannot swallow the kill,
//! - a call-depth probe in the same hook (on top of Lua's own C-stack and Lua-stack limits),
//! - a heap limit (`set_memory_limit`) for the whole state,
//! - output caps on `string.rep` and `string.format`,
//! - `setmetatable` refuses metatables carrying `__gc`, so no mod code can run from the collector.
// DOC: docs/MODDING_GUIDE.md

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::{
    ChunkMode, Function, HookTriggers, Lua, LuaOptions, MultiValue, StdLib, Table, Value, VmState,
};

use crate::value::{LuaError, LuaKey, LuaTable, LuaValue};

/// Message used when a chunk starts with the Lua binary signature.
pub const BYTECODE_FORBIDDEN_MSG: &str = "Binary Lua bytecode is forbidden by sandbox policy";

/// Default instruction budget for one host->Lua call (mod init, one hook stage, one event listener).
pub const DEFAULT_INSTRUCTION_LIMIT: u64 = 10_000_000;
/// Kept for callers of the old interpreter API; same meaning as `DEFAULT_INSTRUCTION_LIMIT`.
pub const DEFAULT_FUEL_BUDGET: u64 = DEFAULT_INSTRUCTION_LIMIT;
/// The count hook fires every this many VM instructions.
pub const DEFAULT_HOOK_INTERVAL: u32 = 1_000;
/// Heap limit of the shared Lua state.
pub const DEFAULT_MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// Largest string `string.rep` / `string.format` may produce.
pub const DEFAULT_MAX_STRING_BYTES: usize = 4 * 1024 * 1024;
/// Deepest Lua call stack the hook tolerates.
pub const DEFAULT_MAX_CALL_DEPTH: usize = 200;
/// `print` lines kept per mod.
pub const DEFAULT_MAX_LOG_LINES: usize = 1_000;
/// Nesting depth allowed when copying tables across the host boundary.
pub const MAX_CONVERT_DEPTH: usize = 64;
/// Total table entries allowed in one host-boundary copy.
pub const MAX_CONVERT_NODES: usize = 200_000;

const MAX_LOG_LINE_BYTES: usize = 4_096;
const MAX_MODULE_NAME_LEN: usize = 128;

/// Sandbox resource limits. All of them are host configuration, not mod-controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxLimits {
    pub instruction_limit: u64,
    pub hook_interval: u32,
    pub memory_limit: usize,
    pub max_string_bytes: usize,
    pub max_call_depth: usize,
    pub max_log_lines: usize,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            instruction_limit: DEFAULT_INSTRUCTION_LIMIT,
            hook_interval: DEFAULT_HOOK_INTERVAL,
            memory_limit: DEFAULT_MEMORY_LIMIT,
            max_string_bytes: DEFAULT_MAX_STRING_BYTES,
            max_call_depth: DEFAULT_MAX_CALL_DEPTH,
            max_log_lines: DEFAULT_MAX_LOG_LINES,
        }
    }
}

impl SandboxLimits {
    pub fn with_instruction_limit(instruction_limit: u64) -> Self {
        Self {
            instruction_limit,
            ..Self::default()
        }
    }
}

/// Rejects precompiled chunks before they reach the loader.
pub fn check_not_bytecode(bytes: &[u8]) -> Result<(), LuaError> {
    if bytes.first() == Some(&0x1b) {
        return Err(LuaError::Forbidden(BYTECODE_FORBIDDEN_MSG.to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Instruction budget
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kill {
    Instructions,
    CallDepth,
}

#[derive(Debug, Clone)]
struct Budget {
    limit: u64,
    max_depth: usize,
    remaining: i64,
    killed: Option<Kill>,
    nesting: u32,
}

impl Budget {
    fn idle(limit: u64, max_depth: usize) -> Self {
        Self {
            limit,
            max_depth,
            remaining: limit.min(i64::MAX as u64) as i64,
            killed: None,
            nesting: 0,
        }
    }

    fn kill_message(&self, kill: Kill) -> String {
        match kill {
            Kill::Instructions => format!(
                "sandbox instruction limit exceeded ({} instructions per call)",
                self.limit
            ),
            Kill::CallDepth => format!(
                "sandbox call depth limit exceeded (more than {} frames)",
                self.max_depth
            ),
        }
    }

    fn kill_error(&self, kill: Kill) -> LuaError {
        match kill {
            Kill::Instructions => LuaError::InstructionLimit(self.kill_message(kill)),
            Kill::CallDepth => LuaError::CallDepth(self.kill_message(kill)),
        }
    }
}

#[derive(Clone)]
struct BudgetHandle(Rc<RefCell<Budget>>);

/// Runs `f` as one protected host->Lua call with its own instruction budget.
///
/// Top-level calls get a fresh budget. Calls nested inside another guarded call (a hook stage calling
/// `base`, which runs the next mod) are capped by what the outer call has left, and what they spend is
/// charged back to the outer call, so a loop around `base(ctx)` cannot multiply the budget.
pub(crate) fn guarded<R>(lua: &Lua, f: impl FnOnce() -> mlua::Result<R>) -> Result<R, LuaError> {
    let handle = lua.app_data_ref::<BudgetHandle>().map(|h| h.clone());
    let Some(BudgetHandle(budget)) = handle else {
        return f().map_err(classify_error);
    };

    let (saved, start) = {
        let mut b = budget.borrow_mut();
        let saved = b.clone();
        let full = b.limit.min(i64::MAX as u64) as i64;
        let start = if saved.nesting == 0 {
            full
        } else {
            saved.remaining.clamp(0, full)
        };
        b.remaining = start;
        b.killed = None;
        b.nesting = saved.nesting + 1;
        (saved, start)
    };

    let result = f();

    let ended = {
        let mut b = budget.borrow_mut();
        let ended = b.clone();
        *b = saved;
        if b.nesting == 0 {
            // Idle budget is refreshed after every top-level call.
            b.remaining = b.limit.min(i64::MAX as u64) as i64;
            b.killed = None;
        } else {
            let spent = start - ended.remaining.max(0);
            b.remaining -= spent;
        }
        ended
    };

    if let Some(kill) = ended.killed {
        return Err(ended.kill_error(kill));
    }
    result.map_err(classify_error)
}

fn is_killed(lua: &Lua) -> bool {
    lua.app_data_ref::<BudgetHandle>()
        .map(|h| h.0.borrow().killed.is_some())
        .unwrap_or(false)
}

/// Maps an `mlua::Error` to our error type, keeping host `LuaError`s raised inside callbacks.
pub(crate) fn classify_error(err: mlua::Error) -> LuaError {
    let mut cur = &err;
    loop {
        match cur {
            mlua::Error::CallbackError { cause, .. } => cur = cause,
            mlua::Error::WithContext { cause, .. } => cur = cause,
            _ => break,
        }
    }
    if let Some(own) = cur.downcast_ref::<LuaError>() {
        return own.clone();
    }
    match cur {
        mlua::Error::SyntaxError { message, .. } => LuaError::Syntax(message.clone()),
        mlua::Error::MemoryError(msg) => {
            LuaError::MemoryLimit(format!("sandbox memory limit exceeded: {}", msg))
        }
        mlua::Error::RuntimeError(msg) => LuaError::Runtime(msg.clone()),
        other => LuaError::Runtime(other.to_string()),
    }
}

/// Wraps a host error so it survives the trip through Lua and back out of `guarded`.
pub(crate) fn host_err(err: LuaError) -> mlua::Error {
    mlua::Error::external(err)
}

pub(crate) fn runtime_err(msg: impl Into<String>) -> mlua::Error {
    host_err(LuaError::Runtime(msg.into()))
}

// ---------------------------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------------------------

/// Copies host data into a fresh Lua value.
pub fn to_lua(lua: &Lua, val: &LuaValue) -> mlua::Result<Value> {
    to_lua_depth(lua, val, 0)
}

fn to_lua_depth(lua: &Lua, val: &LuaValue, depth: usize) -> mlua::Result<Value> {
    Ok(match val {
        LuaValue::Nil => Value::Nil,
        LuaValue::Bool(b) => Value::Boolean(*b),
        LuaValue::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
                Value::Integer(*n as i64)
            } else {
                Value::Number(*n)
            }
        }
        LuaValue::String(s) => Value::String(lua.create_string(s)?),
        LuaValue::Table(t) => {
            if depth >= MAX_CONVERT_DEPTH {
                return Err(runtime_err(
                    "table nesting too deep to cross the host boundary",
                ));
            }
            let out = lua.create_table()?;
            for (k, v) in t.borrow().entries() {
                let key = to_lua_depth(lua, &k.to_value(), depth + 1)?;
                let value = to_lua_depth(lua, v, depth + 1)?;
                out.raw_set(key, value)?;
            }
            Value::Table(out)
        }
    })
}

/// Copies a Lua value into host data. Functions, userdata and threads are rejected, as are cyclic or
/// oversized tables.
pub fn from_lua(val: &Value) -> Result<LuaValue, LuaError> {
    let mut nodes = 0usize;
    from_lua_depth(val, 0, &mut nodes)
}

fn from_lua_depth(val: &Value, depth: usize, nodes: &mut usize) -> Result<LuaValue, LuaError> {
    Ok(match val {
        Value::Nil => LuaValue::Nil,
        Value::Boolean(b) => LuaValue::Bool(*b),
        Value::Integer(i) => LuaValue::Number(*i as f64),
        Value::Number(n) => LuaValue::Number(*n),
        Value::String(s) => LuaValue::String(s.to_string_lossy()),
        Value::Table(t) => {
            if depth >= MAX_CONVERT_DEPTH {
                return Err(LuaError::Runtime(
                    "table nesting too deep (or cyclic) to cross the host boundary".to_string(),
                ));
            }
            let mut entries: Vec<(LuaKey, LuaValue)> = Vec::new();
            for pair in t.pairs::<Value, Value>() {
                let (k, v) = pair.map_err(classify_error)?;
                *nodes += 1;
                if *nodes > MAX_CONVERT_NODES {
                    return Err(LuaError::Runtime(
                        "table too large to cross the host boundary".to_string(),
                    ));
                }
                let key = match &k {
                    Value::Integer(i) => LuaKey::Int(*i),
                    Value::Number(n) => LuaKey::from_f64(*n),
                    Value::String(s) => LuaKey::String(s.to_string_lossy()),
                    Value::Boolean(b) => LuaKey::Bool(*b),
                    other => {
                        return Err(LuaError::Runtime(format!(
                            "table key of type {} cannot cross the host boundary",
                            other.type_name()
                        )))
                    }
                };
                entries.push((key, from_lua_depth(&v, depth + 1, nodes)?));
            }
            // Lua's traversal order is unspecified; sort for deterministic host data.
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = LuaTable::new();
            for (k, v) in entries {
                out.set(k, v);
            }
            LuaValue::Table(Rc::new(RefCell::new(out)))
        }
        other => {
            return Err(LuaError::Runtime(format!(
                "value of type {} cannot cross the host boundary",
                other.type_name()
            )))
        }
    })
}

// ---------------------------------------------------------------------------------------------
// The sandbox
// ---------------------------------------------------------------------------------------------

/// Trusted host prelude, run once with the real globals. Produces the sandbox versions of `pcall`,
/// `xpcall` and `setmetatable` and locks the shared string metatable.
const PRELUDE: &str = r#"
local is_killed, safe_string = ...
local native_pcall, native_xpcall, native_error = pcall, xpcall, error
local native_setmetatable, native_rawget, native_type = setmetatable, rawget, type

local function pass(ok, ...)
    if not ok and is_killed() then
        native_error((...), 0)
    end
    return ok, ...
end

local safe = {}
safe.pcall = function(f, ...)
    return pass(native_pcall(f, ...))
end
safe.xpcall = function(f, handler, ...)
    return pass(native_xpcall(f, function(e)
        if is_killed() then
            return e
        end
        return handler(e)
    end, ...))
end
safe.setmetatable = function(t, mt)
    if native_type(mt) == "table" and native_rawget(mt, "__gc") ~= nil then
        native_error("__gc metamethods are not allowed in the mod sandbox", 2)
    end
    return native_setmetatable(t, mt)
end

local smt = getmetatable("")
smt.__index = safe_string
smt.__metatable = "locked"
return safe
"#;

const BASE_FUNCTIONS: &[&str] = &[
    "assert",
    "error",
    "getmetatable",
    "ipairs",
    "next",
    "pairs",
    "rawequal",
    "rawget",
    "rawlen",
    "rawset",
    "select",
    "tonumber",
    "tostring",
    "type",
];

struct Templates {
    string: Table,
    table: Table,
    math: Table,
    utf8: Table,
    base: Vec<(&'static str, Value)>,
    pcall: Function,
    xpcall: Function,
    setmetatable: Function,
    tostring: Function,
    receiver_mt: Table,
}

/// A mod's private environment (`_ENV`) inside the shared Lua state.
#[derive(Clone)]
pub struct ModEnv {
    mod_id: String,
    root: Option<PathBuf>,
    env: Table,
    logs: Rc<RefCell<Vec<String>>>,
}

impl ModEnv {
    pub fn mod_id(&self) -> &str {
        &self.mod_id
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The environment table itself (the mod's `_G`).
    pub fn table(&self) -> &Table {
        &self.env
    }

    /// Lines written by the mod's `print`.
    pub fn logs(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }

    pub(crate) fn logs_rc(&self) -> Rc<RefCell<Vec<String>>> {
        Rc::clone(&self.logs)
    }
}

/// Shared Lua 5.4 state plus the sandbox machinery.
pub struct LuaSandbox {
    // Field order matters for drop: handles first, the state last.
    templates: Templates,
    limits: SandboxLimits,
    lua: Lua,
}

impl LuaSandbox {
    pub fn new() -> Result<Self, LuaError> {
        Self::with_limits(SandboxLimits::default())
    }

    pub fn with_limits(limits: SandboxLimits) -> Result<Self, LuaError> {
        // Only the libraries we hand out are opened at all: no io, os, package or debug exist here.
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
            LuaOptions::new(),
        )
        .map_err(classify_error)?;
        let templates = build_templates(&lua, &limits).map_err(classify_error)?;

        let budget = Rc::new(RefCell::new(Budget::idle(
            limits.instruction_limit,
            limits.max_call_depth,
        )));
        lua.set_app_data(BudgetHandle(Rc::clone(&budget)));
        let interval = limits
            .hook_interval
            .max(1)
            .min(limits.instruction_limit.clamp(1, u32::MAX as u64) as u32);
        let max_depth = limits.max_call_depth;
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(interval),
            move |lua, _debug| {
                let mut b = budget.borrow_mut();
                if let Some(kill) = b.killed {
                    return Err(mlua::Error::runtime(b.kill_message(kill)));
                }
                b.remaining -= interval as i64;
                if b.remaining < 0 {
                    b.killed = Some(Kill::Instructions);
                    return Err(mlua::Error::runtime(b.kill_message(Kill::Instructions)));
                }
                if max_depth > 0 && lua.inspect_stack(max_depth).is_some() {
                    b.killed = Some(Kill::CallDepth);
                    return Err(mlua::Error::runtime(b.kill_message(Kill::CallDepth)));
                }
                Ok(VmState::Continue)
            },
        );
        if limits.memory_limit > 0 {
            lua.set_memory_limit(limits.memory_limit)
                .map_err(classify_error)?;
        }
        Ok(Self {
            templates,
            limits,
            lua,
        })
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    pub fn limits(&self) -> SandboxLimits {
        self.limits
    }

    /// Bytes currently allocated by the Lua state.
    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }

    /// Creates a fresh, isolated environment for `mod_id`. `root` scopes `require`.
    pub fn create_env(&self, mod_id: &str, root: Option<&Path>) -> Result<ModEnv, LuaError> {
        self.create_env_inner(mod_id, root).map_err(classify_error)
    }

    fn create_env_inner(&self, mod_id: &str, root: Option<&Path>) -> mlua::Result<ModEnv> {
        let lua = &self.lua;
        let t = &self.templates;
        let env = lua.create_table()?;
        for (name, val) in &t.base {
            env.raw_set(*name, val.clone())?;
        }
        env.raw_set("pcall", t.pcall.clone())?;
        env.raw_set("xpcall", t.xpcall.clone())?;
        env.raw_set("setmetatable", t.setmetatable.clone())?;

        let string = copy_table(lua, &t.string)?;
        let table = copy_table(lua, &t.table)?;
        let math = copy_table(lua, &t.math)?;
        let utf8 = copy_table(lua, &t.utf8)?;
        let unpack: Value = table.raw_get("unpack")?;
        env.raw_set("unpack", unpack)?;
        env.raw_set("string", string)?;
        env.raw_set("table", table)?;
        env.raw_set("math", math)?;
        env.raw_set("utf8", utf8)?;
        env.raw_set("_G", env.clone())?;
        env.raw_set("_VERSION", "Lua 5.4 (DeCombat mod sandbox)")?;

        let logs = Rc::new(RefCell::new(Vec::new()));
        env.raw_set(
            "print",
            make_print(
                lua,
                Rc::clone(&logs),
                t.tostring.clone(),
                self.limits.max_log_lines,
            )?,
        )?;
        let root_buf = root.map(Path::to_path_buf);
        env.raw_set(
            "require",
            make_require(lua, mod_id.to_string(), root_buf.clone(), env.clone())?,
        )?;

        Ok(ModEnv {
            mod_id: mod_id.to_string(),
            root: root_buf,
            env,
            logs,
        })
    }

    /// Compiles a text chunk bound to `env`. Binary chunks are refused twice: by signature and by
    /// `ChunkMode::Text`.
    pub fn load_text(
        &self,
        env: &ModEnv,
        chunk_name: &str,
        source: &[u8],
    ) -> Result<Function, LuaError> {
        load_text_chunk(&self.lua, env.table(), chunk_name, source)
    }

    /// Runs a chunk in `env` under the sandbox limits and returns its first result as host data.
    pub fn eval_chunk(&self, env: &ModEnv, source: &str) -> Result<LuaValue, LuaError> {
        let func = self.load_text(env, &format!("{}/chunk", env.mod_id()), source.as_bytes())?;
        let first: Value = guarded(&self.lua, || {
            let vals: MultiValue = func.call(())?;
            Ok(vals.into_iter().next().unwrap_or(Value::Nil))
        })?;
        from_lua(&first)
    }

    /// Calls `func` under the sandbox limits.
    pub fn call(&self, func: &Function, args: MultiValue) -> Result<MultiValue, LuaError> {
        guarded(&self.lua, || func.call::<MultiValue>(args))
    }

    /// Marker metatable for capability tables whose methods accept both `t:m(...)` and `t.m(...)`.
    pub(crate) fn receiver_metatable(&self) -> Table {
        self.templates.receiver_mt.clone()
    }
}

pub(crate) fn load_text_chunk(
    lua: &Lua,
    env: &Table,
    chunk_name: &str,
    source: &[u8],
) -> Result<Function, LuaError> {
    check_not_bytecode(source)?;
    lua.load(source)
        .set_name(format!("={}", chunk_name))
        .set_mode(ChunkMode::Text)
        .set_environment(env.clone())
        .into_function()
        .map_err(classify_error)
}

fn copy_table(lua: &Lua, src: &Table) -> mlua::Result<Table> {
    let out = lua.create_table()?;
    for pair in src.pairs::<Value, Value>() {
        let (k, v) = pair?;
        out.raw_set(k, v)?;
    }
    Ok(out)
}

fn build_templates(lua: &Lua, limits: &SandboxLimits) -> mlua::Result<Templates> {
    let globals = lua.globals();
    let max_bytes = limits.max_string_bytes;

    // string: no dump (it produces bytecode), capped rep and format.
    let string = copy_table(lua, &globals.raw_get::<Table>("string")?)?;
    string.raw_set("dump", Value::Nil)?;
    let native_format: Function = string.raw_get("format")?;
    string.raw_set(
        "rep",
        lua.create_function(
            move |lua, (s, n, sep): (mlua::String, i64, Option<mlua::String>)| {
                if n <= 0 {
                    return lua.create_string("");
                }
                let s_bytes = s.as_bytes();
                let sep_bytes = sep
                    .as_ref()
                    .map(|x| x.as_bytes().to_vec())
                    .unwrap_or_default();
                let total = (s_bytes.len() as u128) * (n as u128)
                    + (sep_bytes.len() as u128) * ((n - 1) as u128);
                if total > max_bytes as u128 {
                    return Err(host_err(LuaError::Runtime(format!(
                        "string.rep result too large ({} bytes, sandbox cap is {})",
                        total, max_bytes
                    ))));
                }
                let mut out = Vec::with_capacity(total as usize);
                for i in 0..n {
                    if i > 0 {
                        out.extend_from_slice(&sep_bytes);
                    }
                    out.extend_from_slice(&s_bytes);
                }
                lua.create_string(out)
            },
        )?,
    )?;
    string.raw_set(
        "format",
        lua.create_function(move |_, args: MultiValue| {
            let out: mlua::String = native_format.call(args)?;
            if out.as_bytes().len() > max_bytes {
                return Err(host_err(LuaError::Runtime(format!(
                    "string.format result too large (sandbox cap is {} bytes)",
                    max_bytes
                ))));
            }
            Ok(out)
        })?,
    )?;

    let table = copy_table(lua, &globals.raw_get::<Table>("table")?)?;

    // math: the generator is shared by every mod, so nobody may reseed it.
    let math = copy_table(lua, &globals.raw_get::<Table>("math")?)?;
    math.raw_set("randomseed", Value::Nil)?;
    // DeCombat extension kept from the first modding API: math.clamp(x, lo, hi).
    math.raw_set(
        "clamp",
        lua.create_function(|_, (x, lo, hi): (f64, f64, f64)| {
            Ok(if x < lo {
                lo
            } else if x > hi {
                hi
            } else {
                x
            })
        })?,
    )?;

    let utf8 = copy_table(lua, &globals.raw_get::<Table>("utf8")?)?;

    let mut base = Vec::new();
    for name in BASE_FUNCTIONS {
        let v: Value = globals.raw_get(*name)?;
        if v.is_nil() {
            return Err(mlua::Error::runtime(format!(
                "base function '{}' missing",
                name
            )));
        }
        base.push((*name, v));
    }
    let tostring: Function = globals.raw_get("tostring")?;

    let killed_fn = lua.create_function(|lua, ()| Ok(is_killed(lua)))?;
    let safe: Table = lua
        .load(PRELUDE)
        .set_name("=cak_sandbox_prelude")
        .set_mode(ChunkMode::Text)
        .call((killed_fn, string.clone()))?;

    let receiver_mt = lua.create_table()?;
    receiver_mt.raw_set("__metatable", "locked")?;

    Ok(Templates {
        string,
        table,
        math,
        utf8,
        base,
        pcall: safe.raw_get("pcall")?,
        xpcall: safe.raw_get("xpcall")?,
        setmetatable: safe.raw_get("setmetatable")?,
        tostring,
        receiver_mt,
    })
}

fn make_print(
    lua: &Lua,
    logs: Rc<RefCell<Vec<String>>>,
    tostring: Function,
    max_lines: usize,
) -> mlua::Result<Function> {
    lua.create_function(move |_, args: MultiValue| {
        let mut parts = Vec::with_capacity(args.len());
        for v in args {
            let s: mlua::String = tostring.call(v)?;
            parts.push(s.to_string_lossy());
        }
        let mut line = parts.join("\t");
        if line.len() > MAX_LOG_LINE_BYTES {
            let mut cut = MAX_LOG_LINE_BYTES;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            line.truncate(cut);
            line.push_str("...");
        }
        let mut l = logs.borrow_mut();
        if l.len() < max_lines {
            l.push(line);
        } else if l.len() == max_lines {
            l.push("[log truncated by sandbox]".to_string());
        }
        Ok(())
    })
}

/// Validates a `require` module name: dot-separated segments of `[A-Za-z0-9_-]`.
pub fn validate_module_name(name: &str) -> Result<Vec<&str>, LuaError> {
    let bad = || {
        LuaError::Forbidden(format!(
            "Invalid or forbidden require path '{}': only dot-separated names inside the mod directory are allowed",
            name
        ))
    };
    if name.is_empty() || name.len() > MAX_MODULE_NAME_LEN {
        return Err(bad());
    }
    let segs: Vec<&str> = name.split('.').collect();
    for seg in &segs {
        if seg.is_empty()
            || !seg
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(bad());
        }
    }
    Ok(segs)
}

fn resolve_module_path(root: &Path, segs: &[&str], name: &str) -> Result<PathBuf, LuaError> {
    let mut base = root.to_path_buf();
    for s in segs {
        base.push(s);
    }
    let file = base.with_extension("lua");
    let init = base.join("init.lua");
    let target = if file.is_file() {
        file
    } else if init.is_file() {
        init
    } else {
        return Err(LuaError::Runtime(format!(
            "Module '{}' not found inside mod directory",
            name
        )));
    };
    // Defeat symlink / junction escapes: the canonical target must stay under the canonical root.
    let root_c = fs::canonicalize(root)
        .map_err(|e| LuaError::Runtime(format!("Cannot resolve mod root: {}", e)))?;
    let target_c = fs::canonicalize(&target)
        .map_err(|e| LuaError::Runtime(format!("Cannot resolve module '{}': {}", name, e)))?;
    if !target_c.starts_with(&root_c) {
        return Err(LuaError::Forbidden(format!(
            "Module '{}' resolves outside the mod directory",
            name
        )));
    }
    Ok(target_c)
}

fn make_require(
    lua: &Lua,
    mod_id: String,
    root: Option<PathBuf>,
    env: Table,
) -> mlua::Result<Function> {
    let cache = lua.create_table()?;
    let loading: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
    lua.create_function(move |lua, name: mlua::String| {
        let name = name.to_str()?.to_string();
        let segs = validate_module_name(&name).map_err(host_err)?;
        let cached: Value = cache.raw_get(name.as_str())?;
        if !cached.is_nil() {
            return Ok(cached);
        }
        if loading.borrow().contains(&name) {
            return Err(runtime_err(format!(
                "Circular require detected for module '{}'",
                name
            )));
        }
        let Some(root) = root.as_ref() else {
            return Err(runtime_err(format!(
                "Cannot require '{}': no mod root directory configured",
                name
            )));
        };
        let path = resolve_module_path(root, &segs, &name).map_err(host_err)?;
        let bytes = fs::read(&path)
            .map_err(|e| runtime_err(format!("Failed to read module '{}': {}", name, e)))?;
        let chunk_name = format!("{}/{}.lua", mod_id, segs.join("/"));
        let func = load_text_chunk(lua, &env, &chunk_name, &bytes).map_err(host_err)?;

        loading.borrow_mut().insert(name.clone());
        let result = func.call::<Value>(name.as_str());
        loading.borrow_mut().remove(&name);
        let mut value = result?;
        if value.is_nil() {
            value = Value::Boolean(true);
        }
        cache.raw_set(name.as_str(), value.clone())?;
        Ok(value)
    })
}

/// Drops the receiver (`self`) when a capability method is called with `:`.
pub(crate) fn strip_receiver(receiver_mt: &Table, args: MultiValue) -> Vec<Value> {
    let mut v: Vec<Value> = args.into_iter().collect();
    if let Some(Value::Table(t)) = v.first() {
        if t.metatable().as_ref() == Some(receiver_mt) {
            v.remove(0);
        }
    }
    v
}

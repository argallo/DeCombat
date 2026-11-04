//! Host-side data values that cross the boundary between Rust and the Lua sandbox.
//!
//! Lua code itself runs inside real Lua 5.4 (`mlua`). The host keeps content records, options, hook
//! contexts and saves as plain data (`LuaValue` / `LuaTable`) so it never holds live references into
//! a mod's heap. Functions, userdata and threads cannot cross this boundary.
// DOC: docs/MODDING_GUIDE.md

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use crate::manifest::JsonValue;

/// Error type produced by the sandbox and by the lint parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaError {
    /// The chunk did not compile (or the lint parser rejected it).
    Syntax(String),
    /// A runtime error raised by mod code or by a host API.
    Runtime(String),
    /// The per-call instruction budget ran out (infinite loop protection).
    InstructionLimit(String),
    /// The Lua heap exceeded the host memory limit.
    MemoryLimit(String),
    /// The Lua call stack grew past the sandbox depth limit.
    CallDepth(String),
    /// Forbidden by sandbox policy (binary chunks, path traversal, ...).
    Forbidden(String),
}

impl LuaError {
    pub fn message(&self) -> &str {
        match self {
            LuaError::Syntax(s)
            | LuaError::Runtime(s)
            | LuaError::InstructionLimit(s)
            | LuaError::MemoryLimit(s)
            | LuaError::CallDepth(s)
            | LuaError::Forbidden(s) => s.as_str(),
        }
    }
}

impl fmt::Display for LuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for LuaError {}

/// Plain data value owned by the host.
#[derive(Clone)]
pub enum LuaValue {
    Nil,
    Bool(bool),
    Number(f64),
    String(String),
    Table(Rc<RefCell<LuaTable>>),
}

impl fmt::Debug for LuaValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LuaValue::Nil => write!(f, "nil"),
            LuaValue::Bool(b) => write!(f, "{}", b),
            LuaValue::Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    write!(f, "{}", *n as i64)
                } else {
                    write!(f, "{}", n)
                }
            }
            LuaValue::String(s) => write!(f, "{:?}", s),
            LuaValue::Table(t) => write!(f, "table({:p})", t.as_ptr()),
        }
    }
}

impl PartialEq for LuaValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (LuaValue::Nil, LuaValue::Nil) => true,
            (LuaValue::Bool(a), LuaValue::Bool(b)) => a == b,
            (LuaValue::Number(a), LuaValue::Number(b)) => a == b,
            (LuaValue::String(a), LuaValue::String(b)) => a == b,
            (LuaValue::Table(a), LuaValue::Table(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl LuaValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            LuaValue::Nil => "nil",
            LuaValue::Bool(_) => "boolean",
            LuaValue::Number(_) => "number",
            LuaValue::String(_) => "string",
            LuaValue::Table(_) => "table",
        }
    }

    pub fn is_truthy(&self) -> bool {
        !matches!(self, LuaValue::Nil | LuaValue::Bool(false))
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            LuaValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            LuaValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Number, or a string that parses as a Lua numeral.
    pub fn coerce_number(&self) -> Option<f64> {
        match self {
            LuaValue::Number(n) => Some(*n),
            LuaValue::String(s) => crate::syntax::parse_lua_number_str(s.trim()),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            LuaValue::Number(n)
                if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 =>
            {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            LuaValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<Rc<RefCell<LuaTable>>> {
        match self {
            LuaValue::Table(t) => Some(Rc::clone(t)),
            _ => None,
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            LuaValue::Nil => "nil".to_string(),
            LuaValue::Bool(b) => b.to_string(),
            LuaValue::Number(n) => {
                if n.is_nan() {
                    "nan".to_string()
                } else if n.is_infinite() {
                    if *n > 0.0 {
                        "inf".to_string()
                    } else {
                        "-inf".to_string()
                    }
                } else if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            LuaValue::String(s) => s.clone(),
            LuaValue::Table(t) => format!("table: {:p}", t.as_ptr()),
        }
    }

    pub fn deep_clone_tables(&self) -> LuaValue {
        match self {
            LuaValue::Table(t) => LuaValue::Table(Rc::new(RefCell::new(t.borrow().deep_clone()))),
            other => other.clone(),
        }
    }

    pub fn to_json_value(&self) -> Result<JsonValue, String> {
        match self {
            LuaValue::Nil => Ok(JsonValue::Null),
            LuaValue::Bool(b) => Ok(JsonValue::Bool(*b)),
            LuaValue::Number(n) => {
                if !n.is_finite() {
                    Err("Cannot serialize non-finite number to JSON".to_string())
                } else {
                    Ok(JsonValue::Number(*n))
                }
            }
            LuaValue::String(s) => Ok(JsonValue::String(s.clone())),
            LuaValue::Table(t) => t.borrow().to_json_value(),
        }
    }

    pub fn from_json_value(val: &JsonValue) -> LuaValue {
        match val {
            JsonValue::Null => LuaValue::Nil,
            JsonValue::Bool(b) => LuaValue::Bool(*b),
            JsonValue::Number(n) => LuaValue::Number(*n),
            JsonValue::String(s) => LuaValue::String(s.clone()),
            JsonValue::Array(arr) => {
                let mut tbl = LuaTable::new();
                for (i, item) in arr.iter().enumerate() {
                    tbl.set_int((i + 1) as i64, LuaValue::from_json_value(item));
                }
                LuaValue::Table(Rc::new(RefCell::new(tbl)))
            }
            JsonValue::Object(obj) => {
                let mut tbl = LuaTable::new();
                for (k, v) in obj {
                    tbl.set_str(k.clone(), LuaValue::from_json_value(v));
                }
                LuaValue::Table(Rc::new(RefCell::new(tbl)))
            }
        }
    }
}

/// Normalized key for `LuaTable`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LuaKey {
    Int(i64),
    FloatBits(u64),
    String(String),
    Bool(bool),
}

impl LuaKey {
    pub fn from_value(v: &LuaValue) -> Result<Self, LuaError> {
        match v {
            LuaValue::Nil => Err(LuaError::Runtime("table index is nil".to_string())),
            LuaValue::Number(n) => {
                if n.is_nan() {
                    return Err(LuaError::Runtime("table index is NaN".to_string()));
                }
                Ok(Self::from_f64(*n))
            }
            LuaValue::String(s) => Ok(LuaKey::String(s.clone())),
            LuaValue::Bool(b) => Ok(LuaKey::Bool(*b)),
            LuaValue::Table(_) => Err(LuaError::Runtime(
                "table keys cannot cross the host boundary".to_string(),
            )),
        }
    }

    pub(crate) fn from_f64(n: f64) -> Self {
        if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
            LuaKey::Int(n as i64)
        } else {
            let normalized = if n == 0.0 { 0.0 } else { n };
            LuaKey::FloatBits(normalized.to_bits())
        }
    }

    pub fn to_value(&self) -> LuaValue {
        match self {
            LuaKey::Int(i) => LuaValue::Number(*i as f64),
            LuaKey::FloatBits(bits) => LuaValue::Number(f64::from_bits(*bits)),
            LuaKey::String(s) => LuaValue::String(s.clone()),
            LuaKey::Bool(b) => LuaValue::Bool(*b),
        }
    }
}

/// Host-side table with deterministic insertion order.
#[derive(Debug, Clone, Default)]
pub struct LuaTable {
    entries: Vec<(LuaKey, LuaValue)>,
}

impl LuaTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn new_rc() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::new()))
    }

    pub fn entries(&self) -> &[(LuaKey, LuaValue)] {
        &self.entries
    }

    pub fn get(&self, key: &LuaKey) -> LuaValue {
        self.raw_get(key)
    }

    pub fn raw_get(&self, key: &LuaKey) -> LuaValue {
        for (k, v) in &self.entries {
            if k == key {
                return v.clone();
            }
        }
        LuaValue::Nil
    }

    pub fn get_str(&self, key: &str) -> LuaValue {
        self.get(&LuaKey::String(key.to_string()))
    }

    pub fn get_int(&self, key: i64) -> LuaValue {
        self.get(&LuaKey::Int(key))
    }

    pub fn set(&mut self, key: LuaKey, val: LuaValue) {
        if matches!(val, LuaValue::Nil) {
            if let Some(pos) = self.entries.iter().position(|(k, _)| k == &key) {
                self.entries.remove(pos);
            }
            return;
        }
        for (k, v) in &mut self.entries {
            if k == &key {
                *v = val;
                return;
            }
        }
        self.entries.push((key, val));
    }

    pub fn set_str(&mut self, key: impl Into<String>, val: LuaValue) {
        self.set(LuaKey::String(key.into()), val);
    }

    pub fn set_int(&mut self, key: i64, val: LuaValue) {
        self.set(LuaKey::Int(key), val);
    }

    /// Lua `#t` sequence length (1..=n contiguous non-nil integer keys).
    pub fn seq_len(&self) -> usize {
        let mut n: i64 = 1;
        while !matches!(self.raw_get(&LuaKey::Int(n)), LuaValue::Nil) {
            n += 1;
        }
        (n - 1) as usize
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn deep_clone(&self) -> LuaTable {
        let mut out = LuaTable::new();
        for (k, v) in &self.entries {
            out.entries.push((k.clone(), v.deep_clone_tables()));
        }
        out
    }

    /// Recursive merge: nested tables merge, everything else replaces.
    pub fn merge_deep(&mut self, other: &LuaTable) {
        for (k, v) in &other.entries {
            match (self.raw_get(k), v) {
                (LuaValue::Table(existing), LuaValue::Table(incoming))
                    if !Rc::ptr_eq(&existing, incoming) =>
                {
                    existing.borrow_mut().merge_deep(&incoming.borrow());
                }
                _ => {
                    self.set(k.clone(), v.deep_clone_tables());
                }
            }
        }
    }

    pub fn to_json_value(&self) -> Result<JsonValue, String> {
        if self.entries.is_empty() {
            return Ok(JsonValue::Object(BTreeMap::new()));
        }
        let seq_len = self.seq_len();
        if seq_len == self.entries.len() && seq_len > 0 {
            let mut arr = Vec::with_capacity(seq_len);
            for i in 1..=(seq_len as i64) {
                arr.push(self.get_int(i).to_json_value()?);
            }
            return Ok(JsonValue::Array(arr));
        }
        let mut map = BTreeMap::new();
        for (k, v) in &self.entries {
            let key_str = match k {
                LuaKey::String(s) => s.clone(),
                LuaKey::Int(i) => i.to_string(),
                LuaKey::FloatBits(b) => f64::from_bits(*b).to_string(),
                LuaKey::Bool(b) => b.to_string(),
            };
            map.insert(key_str, v.to_json_value()?);
        }
        Ok(JsonValue::Object(map))
    }
}

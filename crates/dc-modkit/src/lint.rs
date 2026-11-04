//! `dc-modkit lint`: static checks that mirror the sandbox's rules.
//!
//! Layers, in order, per `.lua` file:
//! 1. precompiled chunks (`check_not_bytecode`, the same test the loader applies),
//! 2. an authoritative syntax check by compiling the file with the real Lua 5.4 inside a
//!    `LuaSandbox` (compile only, nothing runs),
//! 3. the kept `dc_mod_lua::syntax` parser for an AST walk: forbidden globals and APIs, unintended
//!    global assignments, bad `require` names and bad `mod.assets:path` literals.
//!
//! The AST parser covers the Lua 5.1-style grammar plus `//`; Lua 5.4 files that use `goto`,
//! attributes (`<const>`), or bitwise operators compile fine but cannot be walked. Those get a
//! warning ("static checks skipped"), never a silent pass.
// DOC: docs/MODDING_GUIDE.md section 9

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use dc_mod_lua::{
    check_not_bytecode, content_capability, hook_capability, parse_chunk, validate_asset_rel_path,
    validate_module_name, AssignTarget, Expr, ExprKind, LuaSandbox, ModManifest, Stmt, StmtKind,
    TableField, CONTENT_DOMAINS,
};

use crate::LintDiagnostic;

/// Globals that do not exist in the sandbox (or that the sandbox removes from a library).
const FORBIDDEN_GLOBALS: &[(&str, &str)] = &[
    ("os", "the 'os' library is not available in the mod sandbox"),
    ("io", "the 'io' library is not available in the mod sandbox"),
    (
        "debug",
        "the 'debug' library is not available in the mod sandbox",
    ),
    (
        "package",
        "the 'package' library is not available; use require() with a mod-relative name",
    ),
    ("ffi", "there is no FFI in the mod sandbox"),
    ("load", "load() is not available in the mod sandbox"),
    (
        "loadstring",
        "loadstring() is not available in the mod sandbox",
    ),
    ("dofile", "dofile() is not available in the mod sandbox"),
    ("loadfile", "loadfile() is not available in the mod sandbox"),
    (
        "collectgarbage",
        "collectgarbage() is not available in the mod sandbox",
    ),
    (
        "getfenv",
        "getfenv() does not exist in Lua 5.4 and is not available in the mod sandbox",
    ),
    (
        "setfenv",
        "setfenv() does not exist in Lua 5.4 and is not available in the mod sandbox",
    ),
];

/// `(library, member, reason)` for members the sandbox strips from an otherwise kept library.
const FORBIDDEN_MEMBERS: &[(&str, &str, &str)] = &[
    (
        "string",
        "dump",
        "string.dump produces bytecode and is removed by the sandbox",
    ),
    (
        "math",
        "randomseed",
        "the random generator is shared by all mods; math.randomseed is removed",
    ),
];

fn forbidden_global(name: &str) -> Option<&'static str> {
    FORBIDDEN_GLOBALS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, r)| *r)
}

fn forbidden_member(lib: &str, member: &str) -> Option<&'static str> {
    FORBIDDEN_MEMBERS
        .iter()
        .find(|(l, m, _)| *l == lib && *m == member)
        .map(|(_, _, r)| *r)
}

pub fn lint_mod(mod_path: &Path) -> Result<Vec<LintDiagnostic>, String> {
    if !mod_path.exists() {
        return Err(format!("Path '{}' does not exist", mod_path.display()));
    }

    let mut lua_files = Vec::new();
    if mod_path.is_file() {
        lua_files.push((
            mod_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| mod_path.display().to_string()),
            mod_path.to_path_buf(),
        ));
    } else {
        collect_lua_files(mod_path, mod_path, &mut lua_files)?;
    }
    lua_files.sort_by(|a, b| a.0.cmp(&b.0));

    let sandbox = LuaSandbox::new().map_err(|e| format!("Cannot start the Lua sandbox: {}", e))?;
    let mut diagnostics = Vec::new();
    let mut uses = Vec::new();
    let mut fully_parsed = true;
    for (rel_name, full_path) in lua_files {
        if !lint_single_file(&sandbox, &rel_name, &full_path, &mut diagnostics, &mut uses)? {
            fully_parsed = false;
        }
    }
    // Capability checks need the manifest; a missing or bad manifest is `validate`'s finding.
    if mod_path.is_dir() {
        if let Ok(manifest) = ModManifest::from_file(&mod_path.join("manifest.json")) {
            check_capabilities(&manifest, &uses, fully_parsed, &mut diagnostics);
            check_metadata_files(mod_path, &manifest, &mut diagnostics);
        }
    }
    Ok(diagnostics)
}

/// The line of `manifest.json` holding `needle` (1 when it is not found).
fn manifest_line(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|l| l.contains(needle))
        .map_or(1, |i| i + 1)
}

/// The browser metadata the manifest parser cannot see: each screenshot exists, is a PNG or JPEG by its
/// signature and is at most 1 MiB; the scaffold's placeholders are still in place (warnings).
pub(crate) fn check_metadata_files(
    mod_path: &Path,
    manifest: &ModManifest,
    out: &mut Vec<LintDiagnostic>,
) {
    let text = std::fs::read_to_string(mod_path.join("manifest.json")).unwrap_or_default();
    for shot in &manifest.screenshots {
        let line = manifest_line(&text, &format!("\"{}\"", shot));
        let path = mod_path.join(shot);
        let meta = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => m,
            _ => {
                out.push(LintDiagnostic::error(
                    "manifest.json",
                    line,
                    format!("screenshot '{}' does not exist in the mod folder", shot),
                ));
                continue;
            }
        };
        if meta.len() > dc_mod_lua::MAX_SCREENSHOT_BYTES {
            out.push(LintDiagnostic::error(
                "manifest.json",
                line,
                format!(
                    "screenshot '{}' is {} bytes; the limit is {} (1 MiB)",
                    shot,
                    meta.len(),
                    dc_mod_lua::MAX_SCREENSHOT_BYTES
                ),
            ));
            continue;
        }
        let head = std::fs::read(&path).unwrap_or_default();
        let png = head.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let jpg = head.starts_with(&[0xff, 0xd8, 0xff]);
        let lower = shot.to_ascii_lowercase();
        let want_png = lower.ends_with(".png");
        if !(png && want_png || jpg && !want_png) {
            out.push(LintDiagnostic::error(
                "manifest.json",
                line,
                format!(
                    "screenshot '{}' is not a {} image (its first bytes do not match)",
                    shot,
                    if want_png { "PNG" } else { "JPEG" }
                ),
            ));
        }
    }
    if manifest.description == crate::PLACEHOLDER_DESCRIPTION {
        out.push(LintDiagnostic::warning(
            "manifest.json",
            manifest_line(&text, "\"description\""),
            "description is still the scaffold's placeholder; say what the mod does".to_string(),
        ));
    }
    if manifest.author == crate::PLACEHOLDER_AUTHOR {
        out.push(LintDiagnostic::warning(
            "manifest.json",
            manifest_line(&text, "\"author\""),
            "author is still the scaffold's placeholder".to_string(),
        ));
    }
    if manifest
        .screenshots
        .iter()
        .any(|s| std::fs::read(mod_path.join(s)).ok() == Some(crate::placeholder_png()))
    {
        out.push(LintDiagnostic::warning(
            "manifest.json",
            manifest_line(&text, "\"screenshots\""),
            "a screenshot is still the scaffold's 1x1 placeholder image".to_string(),
        ));
    }
}

/// One use of a host service found in a mod script.
struct CapUse {
    file: String,
    line: usize,
    /// Capability the call needs; `None` for a hook whose name is not a literal.
    cap: Option<String>,
    what: String,
    /// A finding that needs no manifest: the hook name has no known namespace.
    error: Option<String>,
}

fn check_capabilities(
    manifest: &ModManifest,
    uses: &[CapUse],
    fully_parsed: bool,
    out: &mut Vec<LintDiagnostic>,
) {
    let mut used: HashSet<&str> = HashSet::new();
    let mut dynamic_hook = false;
    for u in uses {
        if let Some(msg) = &u.error {
            out.push(LintDiagnostic::error(&u.file, u.line, msg.clone()));
            continue;
        }
        match &u.cap {
            Some(cap) => {
                used.insert(cap.as_str());
                if !manifest.has_capability(cap) {
                    out.push(LintDiagnostic::error(
                        &u.file,
                        u.line,
                        format!(
                            "{} needs capability '{}', which manifest.json does not declare in \"capabilities\"",
                            u.what, cap
                        ),
                    ));
                }
            }
            None => dynamic_hook = true,
        }
    }
    // Declared but never used: advisory, and only when every file was analysed.
    if fully_parsed {
        for cap in &manifest.capabilities {
            let is_hook = cap.starts_with("hooks.");
            if !used.contains(cap.as_str()) && !(is_hook && dynamic_hook) {
                out.push(LintDiagnostic::warning(
                    "manifest.json",
                    1,
                    format!("capability '{}' is declared but no script uses it", cap),
                ));
            }
        }
    }
}

/// Finds `mod.content.<domain>:...`, `mod.hooks:wrap/on("ns.name", ...)`, `mod.events:on`,
/// `mod.save:...` and `mod.assets:path` calls.
fn collect_cap_uses(file: &str, stmts: &[Stmt], out: &mut Vec<CapUse>) {
    fn is_mod(e: &Expr) -> bool {
        matches!(&e.kind, ExprKind::Ident(n) if n == "mod")
    }
    fn mod_field(e: &Expr) -> Option<&str> {
        match &e.kind {
            ExprKind::Field { expr, field } if is_mod(expr) => Some(field.as_str()),
            _ => None,
        }
    }
    walk_exprs(stmts, &mut |e| {
        let ExprKind::MethodCall {
            receiver,
            method,
            args,
        } = &e.kind
        else {
            return;
        };
        let mut push = |cap: Option<String>, what: String| {
            out.push(CapUse {
                file: file.to_string(),
                line: e.line,
                cap,
                what,
                error: None,
            })
        };
        if let Some(svc) = mod_field(receiver) {
            match svc {
                "hooks" if method == "wrap" || method == "on" => {
                    let what = format!("mod.hooks:{}", method);
                    match args.first().map(|a| &a.kind) {
                        Some(ExprKind::String(name)) => match hook_capability(name) {
                            Some(cap) => push(Some(cap), format!("{} (\"{}\")", what, name)),
                            None => out.push(CapUse {
                                file: file.to_string(),
                                line: e.line,
                                cap: None,
                                error: Some(format!(
                                    "{} registers hook '{}', which is not '<namespace>.<name>' with a known \
                                     namespace (lobby, combat, render, hud); the host will refuse it",
                                    what, name
                                )),
                                what,
                            }),
                        },
                        _ => push(None, what),
                    }
                }
                "events" if method == "on" => push(Some("events".into()), "mod.events:on".into()),
                "save" => push(Some("save".into()), format!("mod.save:{}", method)),
                "assets" if method == "path" => {
                    push(Some("assets".into()), "mod.assets:path".into())
                }
                _ => {}
            }
        } else if let ExprKind::Field { expr, field: dom } = &receiver.kind {
            if mod_field(expr) == Some("content") && CONTENT_DOMAINS.contains(&dom.as_str()) {
                push(
                    Some(content_capability(dom)),
                    format!("mod.content.{}:{}", dom, method),
                );
            }
        }
    });
}

fn collect_lua_files(
    root: &Path,
    cur: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let entries = fs::read_dir(cur)
        .map_err(|e| format!("Failed to read directory '{}': {}", cur.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_lua_files(root, &path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("lua") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Line number out of a Lua error such as `name:12: message` (first `:<digits>:` in the text).
fn lua_error_line(msg: &str) -> Option<usize> {
    let parts: Vec<&str> = msg.split(':').collect();
    parts.iter().skip(1).find_map(|p| {
        p.trim()
            .parse::<usize>()
            .ok()
            .filter(|_| !p.trim().is_empty())
    })
}

/// Line number out of a `syntax` module error such as `line 12: message`.
fn parser_error_line(msg: &str) -> Option<usize> {
    let rest = msg.strip_prefix("line ")?;
    rest.split(':').next()?.trim().parse::<usize>().ok()
}

fn lint_single_file(
    sandbox: &LuaSandbox,
    rel_name: &str,
    full_path: &Path,
    diagnostics: &mut Vec<LintDiagnostic>,
    uses: &mut Vec<CapUse>,
) -> Result<bool, String> {
    let bytes = fs::read(full_path)
        .map_err(|e| format!("Failed to read '{}': {}", full_path.display(), e))?;
    let mut push = |line: usize, message: String| {
        diagnostics.push(LintDiagnostic::error(rel_name, line, message));
    };

    if let Err(e) = check_not_bytecode(&bytes) {
        push(1, e.to_string());
        return Ok(false);
    }
    let Ok(source) = String::from_utf8(bytes) else {
        push(1, "File is not valid UTF-8".to_string());
        return Ok(false);
    };

    // Authoritative syntax check: real Lua 5.4, compile only.
    let env = sandbox
        .create_env("lint", None)
        .map_err(|e| format!("Cannot create the lint environment: {}", e))?;
    if let Err(e) = sandbox.load_text(&env, rel_name, source.as_bytes()) {
        let msg = e.to_string();
        push(lua_error_line(&msg).unwrap_or(1), msg);
        return Ok(false);
    }

    match parse_chunk(&source) {
        Ok(stmts) => {
            let mut linter = Linter {
                file: rel_name,
                out: diagnostics,
            };
            let mut root_scope = HashSet::new();
            linter.block(&stmts, &mut root_scope);
            collect_cap_uses(rel_name, &stmts, uses);
        }
        Err(err) => {
            // Valid Lua 5.4 that the AST parser does not cover (goto, <const>, bitwise operators).
            let line = parser_error_line(err.message()).unwrap_or(1);
            diagnostics.push(LintDiagnostic::warning(
                rel_name,
                line,
                format!(
                    "static checks skipped, the lint parser does not cover this Lua 5.4 syntax: {}",
                    err.message()
                ),
            ));
            return Ok(false);
        }
    }
    Ok(true)
}

struct Linter<'a> {
    file: &'a str,
    out: &'a mut Vec<LintDiagnostic>,
}

impl Linter<'_> {
    fn err(&mut self, line: usize, message: String) {
        self.out
            .push(LintDiagnostic::error(self.file, line, message));
    }

    fn block(&mut self, stmts: &[Stmt], locals: &mut HashSet<String>) {
        for stmt in stmts {
            self.stmt(stmt, locals);
        }
    }

    fn child_block(&mut self, stmts: &[Stmt], locals: &HashSet<String>, extra: &[String]) {
        let mut child = locals.clone();
        for n in extra {
            child.insert(n.clone());
        }
        self.block(stmts, &mut child);
    }

    fn stmt(&mut self, stmt: &Stmt, locals: &mut HashSet<String>) {
        match &stmt.kind {
            StmtKind::Local { names, values } => {
                for v in values {
                    self.expr(v, locals);
                }
                for n in names {
                    locals.insert(n.clone());
                }
            }
            StmtKind::Assign { targets, values } => {
                for v in values {
                    self.expr(v, locals);
                }
                for target in targets {
                    match target {
                        AssignTarget::Ident(name) => {
                            if !locals.contains(name) {
                                self.err(
                                    stmt.line,
                                    format!(
                                        "Top-level/undeclared global assignment to '{}' outside 'local'",
                                        name
                                    ),
                                );
                            }
                        }
                        AssignTarget::Field(base, field) => {
                            if is_global_table(base, locals) {
                                self.err(
                                    stmt.line,
                                    format!("Global assignment to '_G.{}' outside 'local'", field),
                                );
                            }
                            if field == "__gc" {
                                self.err(stmt.line, GC_MSG.to_string());
                            }
                            self.expr(base, locals);
                        }
                        AssignTarget::Index(base, idx) => {
                            if is_global_table(base, locals) {
                                self.err(
                                    stmt.line,
                                    "Global assignment to '_G[...]' outside 'local'".to_string(),
                                );
                            }
                            self.expr(base, locals);
                            self.expr(idx, locals);
                        }
                    }
                }
            }
            StmtKind::FuncDecl {
                name_path,
                method,
                params,
                body,
                is_local,
                ..
            } => {
                if *is_local {
                    locals.insert(name_path[0].clone());
                } else if name_path.len() == 1
                    && method.is_none()
                    && !locals.contains(&name_path[0])
                {
                    self.err(
                        stmt.line,
                        format!(
                            "Top-level global function '{}' declared outside 'local'",
                            name_path[0]
                        ),
                    );
                } else if name_path.len() > 1 {
                    // function os.x() / function _G.x(): the first segment is read like an expression.
                    let first = &name_path[0];
                    if !locals.contains(first) {
                        if let Some(why) = forbidden_global(first) {
                            self.err(stmt.line, format!("Forbidden global '{}': {}", first, why));
                        } else if first == "_G" || first == "_ENV" {
                            self.err(
                                stmt.line,
                                format!(
                                    "Global assignment to '{}.{}' outside 'local'",
                                    first, name_path[1]
                                ),
                            );
                        }
                    }
                }
                let mut extra: Vec<String> = params.clone();
                if method.is_some() {
                    extra.push("self".to_string());
                }
                self.child_block(body, locals, &extra);
            }
            StmtKind::If {
                branches,
                else_branch,
            } => {
                for (cond, body) in branches {
                    self.expr(cond, locals);
                    self.child_block(body, locals, &[]);
                }
                if let Some(body) = else_branch {
                    self.child_block(body, locals, &[]);
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond, locals);
                self.child_block(body, locals, &[]);
            }
            StmtKind::Repeat { body, cond } => {
                let mut child = locals.clone();
                self.block(body, &mut child);
                self.expr(cond, &child);
            }
            StmtKind::ForNum {
                var,
                start,
                limit,
                step,
                body,
            } => {
                self.expr(start, locals);
                self.expr(limit, locals);
                if let Some(s) = step {
                    self.expr(s, locals);
                }
                self.child_block(body, locals, std::slice::from_ref(var));
            }
            StmtKind::ForIn {
                vars,
                iter_exprs,
                body,
            } => {
                for e in iter_exprs {
                    self.expr(e, locals);
                }
                self.child_block(body, locals, vars);
            }
            StmtKind::Do { body } => self.child_block(body, locals, &[]),
            StmtKind::Return(exprs) => {
                for e in exprs {
                    self.expr(e, locals);
                }
            }
            StmtKind::Break => {}
            StmtKind::ExprStmt(expr) => self.expr(expr, locals),
        }
    }

    /// Reports `lib.member` / `lib` uses of forbidden globals. Returns true when it handled `e`
    /// (so the caller must not report the base identifier a second time).
    fn forbidden_access(
        &mut self,
        base: &Expr,
        member: Option<&str>,
        line: usize,
        locals: &HashSet<String>,
    ) -> bool {
        // _G.os, _ENV.os
        if let (Some(m), ExprKind::Ident(g)) = (member, &base.kind) {
            if (g == "_G" || g == "_ENV") && !locals.contains(g) {
                if let Some(why) = forbidden_global(m) {
                    self.err(line, format!("Forbidden global '{}.{}': {}", g, m, why));
                    return true;
                }
            }
        }
        let ExprKind::Ident(lib) = &base.kind else {
            return false;
        };
        if locals.contains(lib) {
            return false;
        }
        if let Some(why) = forbidden_global(lib) {
            let name = match member {
                Some(m) => format!("{}.{}", lib, m),
                None => lib.clone(),
            };
            self.err(line, format!("Forbidden global '{}': {}", name, why));
            return true;
        }
        if let Some(m) = member {
            if let Some(why) = forbidden_member(lib, m) {
                self.err(line, format!("Forbidden API '{}.{}': {}", lib, m, why));
                return true;
            }
        }
        false
    }

    fn expr(&mut self, expr: &Expr, locals: &HashSet<String>) {
        match &expr.kind {
            ExprKind::Nil
            | ExprKind::Bool(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Vararg => {}
            ExprKind::Ident(name) => {
                if !locals.contains(name) {
                    if let Some(why) = forbidden_global(name) {
                        self.err(expr.line, format!("Forbidden global '{}': {}", name, why));
                    }
                }
            }
            ExprKind::Paren(inner) | ExprKind::Unary { expr: inner, .. } => {
                self.expr(inner, locals)
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs, locals);
                self.expr(rhs, locals);
            }
            ExprKind::Field { expr: obj, field } => {
                if !self.forbidden_access(obj, Some(field), expr.line, locals) {
                    self.expr(obj, locals);
                }
            }
            ExprKind::Index { expr: obj, index } => {
                let member = match &index.kind {
                    ExprKind::String(s) => Some(s.as_str()),
                    _ => None,
                };
                if !self.forbidden_access(obj, member, expr.line, locals) {
                    self.expr(obj, locals);
                }
                self.expr(index, locals);
            }
            ExprKind::TableCons(fields) => {
                for f in fields {
                    match f {
                        TableField::Index(k, v) => {
                            if matches!(&k.kind, ExprKind::String(s) if s == "__gc") {
                                self.err(k.line, GC_MSG.to_string());
                            }
                            self.expr(k, locals);
                            self.expr(v, locals);
                        }
                        TableField::Named(name, v) => {
                            if name == "__gc" {
                                self.err(v.line, GC_MSG.to_string());
                            }
                            self.expr(v, locals);
                        }
                        TableField::Positional(v) => self.expr(v, locals),
                    }
                }
            }
            ExprKind::Function { params, body, .. } => self.child_block(body, locals, params),
            ExprKind::Call { func, args } => {
                if let ExprKind::Ident(fn_name) = &func.kind {
                    if fn_name == "require" && !locals.contains(fn_name) {
                        if let Some(Expr {
                            kind: ExprKind::String(mod_str),
                            ..
                        }) = args.first()
                        {
                            if validate_module_name(mod_str).is_err() {
                                self.err(
                                    expr.line,
                                    format!(
                                        "Invalid or forbidden require path '{}' in require(): use dot-separated names inside the mod directory",
                                        mod_str
                                    ),
                                );
                            }
                        }
                    }
                }
                if let ExprKind::Field { expr: recv, field } = &func.kind {
                    if field == "path" && is_assets_expr(recv) {
                        if let Some(Expr {
                            kind: ExprKind::String(p),
                            ..
                        }) = args.first()
                        {
                            self.check_asset_path(p, expr.line, "mod.assets.path()");
                        }
                    }
                }
                self.expr(func, locals);
                for a in args {
                    self.expr(a, locals);
                }
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } => {
                if method == "path" && is_assets_expr(receiver) {
                    if let Some(Expr {
                        kind: ExprKind::String(p),
                        ..
                    }) = args.first()
                    {
                        self.check_asset_path(p, expr.line, "mod.assets:path()");
                    }
                }
                // io:write() style calls on a forbidden table.
                if !self.forbidden_access(receiver, Some(method), expr.line, locals) {
                    self.expr(receiver, locals);
                }
                for a in args {
                    self.expr(a, locals);
                }
            }
        }
    }

    fn check_asset_path(&mut self, p: &str, line: usize, what: &str) {
        if validate_asset_rel_path(p).is_err() {
            self.err(
                line,
                format!("Path traversal '..' or invalid path '{}' in {}", p, what),
            );
        }
    }
}

const GC_MSG: &str = "__gc metamethods are not allowed in the mod sandbox";

/// `_G` / `_ENV` that is not a local.
fn is_global_table(e: &Expr, locals: &HashSet<String>) -> bool {
    matches!(&e.kind, ExprKind::Ident(id) if (id == "_G" || id == "_ENV") && !locals.contains(id))
}

fn is_assets_expr(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Field { field, .. } => field == "assets",
        ExprKind::Ident(name) => name == "assets",
        _ => false,
    }
}

/// Pre-order visit of every expression in a block (used by `validate` to find content calls).
pub(crate) fn walk_exprs(stmts: &[Stmt], f: &mut dyn FnMut(&Expr)) {
    for s in stmts {
        match &s.kind {
            StmtKind::Local { values, .. } => values.iter().for_each(|e| walk_expr(e, f)),
            StmtKind::Assign { targets, values } => {
                for t in targets {
                    match t {
                        AssignTarget::Ident(_) => {}
                        AssignTarget::Field(b, _) => walk_expr(b, f),
                        AssignTarget::Index(b, i) => {
                            walk_expr(b, f);
                            walk_expr(i, f);
                        }
                    }
                }
                values.iter().for_each(|e| walk_expr(e, f));
            }
            StmtKind::FuncDecl { body, .. } | StmtKind::Do { body } => walk_exprs(body, f),
            StmtKind::If {
                branches,
                else_branch,
            } => {
                for (c, b) in branches {
                    walk_expr(c, f);
                    walk_exprs(b, f);
                }
                if let Some(b) = else_branch {
                    walk_exprs(b, f);
                }
            }
            StmtKind::While { cond, body } | StmtKind::Repeat { body, cond } => {
                walk_expr(cond, f);
                walk_exprs(body, f);
            }
            StmtKind::ForNum {
                start,
                limit,
                step,
                body,
                ..
            } => {
                walk_expr(start, f);
                walk_expr(limit, f);
                if let Some(e) = step {
                    walk_expr(e, f);
                }
                walk_exprs(body, f);
            }
            StmtKind::ForIn {
                iter_exprs, body, ..
            } => {
                iter_exprs.iter().for_each(|e| walk_expr(e, f));
                walk_exprs(body, f);
            }
            StmtKind::Return(exprs) => exprs.iter().for_each(|e| walk_expr(e, f)),
            StmtKind::Break => {}
            StmtKind::ExprStmt(e) => walk_expr(e, f),
        }
    }
}

fn walk_expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Number(_)
        | ExprKind::String(_)
        | ExprKind::Vararg
        | ExprKind::Ident(_) => {}
        ExprKind::Paren(i) | ExprKind::Unary { expr: i, .. } => walk_expr(i, f),
        ExprKind::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, f);
            walk_expr(rhs, f);
        }
        ExprKind::Field { expr, .. } => walk_expr(expr, f),
        ExprKind::Index { expr, index } => {
            walk_expr(expr, f);
            walk_expr(index, f);
        }
        ExprKind::TableCons(fields) => {
            for fl in fields {
                match fl {
                    TableField::Index(k, v) => {
                        walk_expr(k, f);
                        walk_expr(v, f);
                    }
                    TableField::Named(_, v) | TableField::Positional(v) => walk_expr(v, f),
                }
            }
        }
        ExprKind::Function { body, .. } => walk_exprs(body, f),
        ExprKind::Call { func, args } => {
            walk_expr(func, f);
            args.iter().for_each(|a| walk_expr(a, f));
        }
        ExprKind::MethodCall { receiver, args, .. } => {
            walk_expr(receiver, f);
            args.iter().for_each(|a| walk_expr(a, f));
        }
    }
}

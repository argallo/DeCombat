//! `dc-lobby-server doctor [flags]`: checks the set-up the server would start with and prints a table.
//!
//! It takes the same flags and environment as the server (`--config`, `--db`, `--port`, `DC_*`, `CA_ROM`), resolves
//! them exactly as start-up does, and checks: the config (`dc_config::check` with file-system checks, plus
//! `lobby.json` keys and rule values), the listener ports, the ROM folder (identification and table counts), the
//! database (opened read-only, nothing is created), the mods folder (validate and lint of every mod) and the
//! advertised mod set with its hash. Exit status 0 = no FAIL (WARN allowed), 1 = at least one FAIL, 2 = usage.

use crate::cli;
use crate::{config, modset, rom, serverconf};
use std::net::TcpListener;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub check: String,
    pub status: Status,
    pub detail: String,
}

fn row(check: &str, status: Status, detail: impl Into<String>) -> Row {
    Row {
        check: check.to_string(),
        status,
        detail: detail.into(),
    }
}

/// Renders the rows as an aligned table.
pub fn render(rows: &[Row]) -> String {
    let w = rows
        .iter()
        .map(|r| r.check.chars().count())
        .max()
        .unwrap_or(5)
        .max(5);
    let mut out = format!("{:<w$}  {:<4}  {}\n", "CHECK", "STAT", "DETAIL", w = w);
    out.push_str(&format!("{}  {}  {}\n", "-".repeat(w), "----", "------"));
    for r in rows {
        out.push_str(&format!(
            "{:<w$}  {:<4}  {}\n",
            r.check,
            r.status.word(),
            r.detail,
            w = w
        ));
    }
    let n = |s: Status| rows.iter().filter(|r| r.status == s).count();
    out.push_str(&format!(
        "doctor: {} ok, {} warnings, {} failures, {} skipped\n",
        n(Status::Ok),
        n(Status::Warn),
        n(Status::Fail),
        n(Status::Skip)
    ));
    out
}

/// True if any row failed.
pub fn failed(rows: &[Row]) -> bool {
    rows.iter().any(|r| r.status == Status::Fail)
}

fn db_row(db: &Path) -> Row {
    if db.is_file() {
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
        return match rusqlite::Connection::open_with_flags(db, flags) {
            Ok(c) => match c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)) {
                Ok(s) if s == "ok" => {
                    let accounts: Option<i64> = c
                        .query_row("SELECT count(*) FROM accounts", [], |r| r.get(0))
                        .ok();
                    let size = std::fs::metadata(db).map(|m| m.len()).unwrap_or(0);
                    row(
                        "database",
                        Status::Ok,
                        format!(
                            "{} opens, integrity ok, {} bytes{}",
                            db.display(),
                            size,
                            accounts
                                .map(|n| format!(", {n} accounts"))
                                .unwrap_or_default()
                        ),
                    )
                }
                Ok(s) => row(
                    "database",
                    Status::Fail,
                    format!("{}: integrity check says '{}'", db.display(), s),
                ),
                Err(e) => row(
                    "database",
                    Status::Fail,
                    format!("{}: not a usable SQLite file: {}", db.display(), e),
                ),
            },
            Err(e) => row(
                "database",
                Status::Fail,
                format!("{}: cannot open: {}", db.display(), e),
            ),
        };
    }
    if db.exists() {
        return row(
            "database",
            Status::Fail,
            format!("{} exists but is not a file", db.display()),
        );
    }
    // a new database: the nearest existing ancestor must accept a new file
    let mut dir = db.parent().map(Path::to_path_buf).unwrap_or_default();
    if dir.as_os_str().is_empty() {
        dir = std::path::PathBuf::from(".");
    }
    while !dir.exists() {
        match dir.parent() {
            Some(p) if !p.as_os_str().is_empty() => dir = p.to_path_buf(),
            _ => {
                dir = std::path::PathBuf::from(".");
                break;
            }
        }
    }
    let probe = dir.join(format!(".dc-doctor-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            row(
                "database",
                Status::Ok,
                format!(
                    "{} does not exist yet; it will be created ({} is writable)",
                    db.display(),
                    dir.display()
                ),
            )
        }
        Err(e) => row(
            "database",
            Status::Fail,
            format!(
                "{} cannot be created: {} is not writable: {}",
                db.display(),
                dir.display(),
                e
            ),
        ),
    }
}

fn mods_rows(dir: &Path, rows: &mut Vec<Row>) {
    if !dir.is_dir() {
        rows.push(row(
            "mods dir",
            Status::Fail,
            format!("{} is not a folder", dir.display()),
        ));
        return;
    }
    let mods = modset::mod_dirs(dir);
    if mods.is_empty() {
        rows.push(row(
            "mods dir",
            Status::Ok,
            format!("{}: no mod folders", dir.display()),
        ));
        return;
    }
    for m in &mods {
        let name = m
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rep = dc_modkit::validate_mod(m, None);
        let mut errors = rep.errors.clone();
        let mut warns = rep.warnings.clone();
        match dc_modkit::lint_mod(m) {
            Ok(d) => {
                for x in d {
                    if x.is_error() {
                        errors.push(x.to_string());
                    } else {
                        warns.push(x.to_string());
                    }
                }
            }
            Err(e) => errors.push(e),
        }
        let label = format!("mod {name}");
        if let Some(e) = errors.first() {
            rows.push(row(
                &label,
                Status::Fail,
                format!("{} error(s); first: {}", errors.len(), e),
            ));
        } else if let Some(w) = warns.first() {
            rows.push(row(
                &label,
                Status::Warn,
                format!("{} warning(s); first: {}", warns.len(), w),
            ));
        } else {
            rows.push(row(
                &label,
                Status::Ok,
                format!(
                    "{} validates and lints clean",
                    rep.mod_id.as_deref().unwrap_or(&name)
                ),
            ));
        }
    }
}

/// Runs every check and returns the rows.
pub fn checks(opts: &cli::Options, conf: Option<&serverconf::ServerConf>) -> Vec<Row> {
    let mut rows = Vec::new();
    // config
    match conf {
        None => rows.push(row(
            "config",
            Status::Skip,
            "no --config, DC_CONFIG or DC_* variable: built-in rules plus lobby.json",
        )),
        Some(c) => {
            rows.push(row(
                "config",
                Status::Ok,
                format!("{} loaded", c.describe()),
            ));
            // file-system facts (ROM folder, DB, mods) are checked below with the values the server really uses;
            // dc_config's own fs check expects `<sha256>.zip` files, while the lobby loads mod folders
            for i in c.check(false) {
                let st = if i.is_error() {
                    Status::Fail
                } else {
                    Status::Warn
                };
                rows.push(row(&format!("config {}", i.path), st, i.message));
            }
        }
    }
    // lobby.json and the rule values
    let cfg_dir = opts
        .db
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let json = cfg_dir.join("lobby.json");
    let (mut rules, notes) = config::Rules::load(&cfg_dir);
    if !json.is_file() {
        rows.push(row(
            "lobby.json",
            Status::Skip,
            format!("{} absent (optional)", json.display()),
        ));
    } else if notes.is_empty() {
        rows.push(row(
            "lobby.json",
            Status::Ok,
            format!("{} read, every key accepted", json.display()),
        ));
    }
    for n in notes {
        rows.push(row("lobby.json", Status::Warn, n));
    }
    if let Some(c) = conf {
        let notes = c.apply_rules(&mut rules);
        if notes.is_empty() {
            rows.push(row(
                "rules",
                Status::Ok,
                format!(
                    "{} key(s) from the config layer applied",
                    c.rule_overrides().len()
                ),
            ));
        }
        for n in notes {
            rows.push(row("rules", Status::Warn, n));
        }
    }
    // ports
    let ports = [
        ("auth", opts.port as i64),
        ("lobby", opts.lobbyport as i64),
        ("clan", opts.clanport as i64),
        ("clan web", opts.clanweb),
    ];
    let mut seen: Vec<(&str, i64)> = Vec::new();
    for (name, p) in ports {
        if p == 0 {
            rows.push(row(&format!("port {name}"), Status::Skip, "off"));
            continue;
        }
        if let Some((other, _)) = seen.iter().find(|(_, q)| *q == p) {
            rows.push(row(
                &format!("port {name}"),
                Status::Fail,
                format!("{p} is also the {other} port"),
            ));
            continue;
        }
        seen.push((name, p));
        match u16::try_from(p) {
            Err(_) => rows.push(row(
                &format!("port {name}"),
                Status::Fail,
                format!("{p} is not a port number"),
            )),
            Ok(port) => match TcpListener::bind((opts.bind.as_str(), port)) {
                Ok(l) => {
                    drop(l);
                    rows.push(row(
                        &format!("port {name}"),
                        Status::Ok,
                        format!("{}:{} is free", opts.bind, port),
                    ));
                }
                Err(e) => rows.push(row(
                    &format!("port {name}"),
                    Status::Warn,
                    format!(
                        "{}:{} cannot be bound now ({}); fine if this server is running",
                        opts.bind, port, e
                    ),
                )),
            },
        }
    }
    // ROM
    match serverconf::rom_dir(conf) {
        None => rows.push(row("rom", Status::Warn, "no ROM folder: set CA_ROM, paths.rom or DC_PATHS__ROM; the server runs with empty tables")),
        Some(dir) => {
            match dc_rom::identify_rom(&dir) {
                Ok(id) => rows.push(row("rom", Status::Ok, format!("{} is the {:?} build", dir.display(), id))),
                Err(e) => rows.push(row("rom", Status::Fail, format!("{}: {}", dir.display(), e))),
            }
            let data = rom::RomData::load(&dir);
            if data.map_count() == 0 {
                rows.push(row("rom tables", Status::Fail, "no maps found in the ROM tables"));
            } else {
                rows.push(row("rom tables", Status::Ok, format!("{} maps, {} modes, {} clan modes", data.map_count(), data.mode_count(false), data.mode_count(true))));
            }
            for n in &data.notes {
                rows.push(row("rom tables", Status::Warn, n.clone()));
            }
        }
    }
    // database
    rows.push(db_row(&opts.db));
    // mods
    let mods_dir = serverconf::mods_dir(opts, conf);
    match &mods_dir {
        Some(d) => mods_rows(d, &mut rows),
        None => rows.push(row(
            "mods dir",
            Status::Skip,
            "no --mods-dir / paths.mods_dir",
        )),
    }
    if let Some(c) = conf {
        let built = modset::build(c, mods_dir.as_deref());
        for e in &built.errors {
            rows.push(row("mod set", Status::Fail, e.clone()));
        }
        for w in &built.warnings {
            rows.push(row("mod set", Status::Warn, w.clone()));
        }
        if c.cfg.mods.policy != "modded" {
            if let Some(d) = &mods_dir {
                let n = modset::mod_dirs(d).len();
                if n > 0 {
                    rows.push(row("mod set", Status::Warn, format!("policy vanilla: the {n} mod folder(s) in {} will be refused at start-up", d.display())));
                }
            }
        }
        rows.push(row(
            "mod set",
            Status::Ok,
            format!(
                "policy {}, {} declared, hash {}",
                if built.set.modded {
                    "modded"
                } else {
                    "vanilla"
                },
                built.set.mods.len(),
                built.set.hash()
            ),
        ));
    }
    rows
}

/// Entry point: `args` start with the program name; returns the process exit status.
pub fn run<I>(args: I, env: &[(String, String)]) -> i32
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().collect();
    let m = match cli::command().try_get_matches_from(&args) {
        Ok(m) => m,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    let flag = m
        .get_one::<String>("config")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);
    let (conf, mut rows) = match serverconf::ServerConf::load_unchecked(flag.as_deref(), env) {
        Ok(c) => (c, Vec::new()),
        Err(errs) => {
            let rows = errs
                .into_iter()
                .map(|e| row("config", Status::Fail, e))
                .collect::<Vec<_>>();
            (None, rows)
        }
    };
    let opts = cli::resolve_with(&m, conf.as_ref());
    rows.extend(checks(&opts, conf.as_ref()));
    print!("{}", render(&rows));
    if failed(&rows) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_renders_and_counts() {
        let rows = vec![
            row("config", Status::Ok, "fine"),
            row("config server.bind", Status::Warn, "hm"),
            row("db", Status::Fail, "bad"),
        ];
        let t = render(&rows);
        assert!(
            t.contains("CHECK") && t.contains("config server.bind  WARN  hm"),
            "{t}"
        );
        assert!(
            t.ends_with("doctor: 1 ok, 1 warnings, 1 failures, 0 skipped\n"),
            "{t}"
        );
        assert!(failed(&rows));
    }

    #[test]
    fn missing_db_in_a_writable_dir_is_ok_and_creates_nothing() {
        let dir = std::env::temp_dir().join(format!("dc_doctor_db_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("sub").join("ca.db");
        let r = db_row(&db);
        assert_eq!(r.status, Status::Ok, "{}", r.detail);
        assert!(!db.exists() && !dir.join("sub").exists());
        std::fs::write(db.with_file_name("junk.db"), b"not sqlite").ok();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(&db, b"this is not a database at all, definitely").unwrap();
        assert_eq!(db_row(&db).status, Status::Fail);
        std::fs::remove_dir_all(&dir).ok();
    }
}

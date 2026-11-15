//! `dc-lobby-server packs sync`: installs the packs of a pack repository's TAGGED release into the lobby's mods
//! folder on any host (Windows, Linux, macOS), with the checks `tools/vps/packs_sync.sh` makes on the VPS: every zip
//! must match the release's SHA256SUMS and the hash its `packs.json` row declares; all selected zips are checked
//! before any is installed (one failure installs nothing of that release); each lands as `<sha256>.zip`, and a zip
//! already there under its hash is left alone, so a re-run changes nothing. Downloads go through the system's `curl`
//! (part of Windows 10 and later, macOS and Linux), https only. server.toml is never edited: the rows to paste are
//! printed (docs/PACKS_FROM_REPOSITORIES.md).
// DC-DEFAULT: LG-434 (proposed, OWNER DECISION 2026-11-08 "utilize the repos to load maps, camos etc.")
// Provenance: ORIGINAL (our own tool)

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const USAGE: &str = "\
usage: dc-lobby-server packs sync <owner/repo> <tag> --mods-dir DIR [--only id,id,...] [--format entry|set]
                                  [--dry-run] [--base-url https-URL-or-folder]
       dc-lobby-server packs sync --config server.toml [--mods-dir DIR] [--format entry|set] [--dry-run]

  The first form installs the packs of one release; the second those of every [[packs.sources]] row of server.toml
  into its paths.mods_dir (--mods-dir wins). <tag> is a release tag (v1, v1.2.0, 2026.11), never a branch: a branch
  moves, a pack is its hash. --base-url replaces https://github.com/<owner/repo>/releases/download/<tag> (a folder
  holding the release's files works too, for a host without internet access). Nothing is installed from a release
  unless every selected zip passes; server.toml is never edited (the rows to paste are printed).
  Exit 0: all installed or present; 1: a check failed; 2: usage.";

/// The largest pack zip taken (the lobby serves packs of at most this size).
// DC-DEFAULT: LG-434 (proposed) 64 MiB, as a host agent's pack stream (MAP-84)
pub const MAX_ZIP_BYTES: u64 = 64 * 1024 * 1024;
/// The largest `packs.json` / `SHA256SUMS` taken.
const MAX_LIST_BYTES: u64 = 4 * 1024 * 1024;
/// The most rows a release's `packs.json` may hold.
// DC-DEFAULT: LG-434 (proposed) 1024 (DeCombat-mods v1 has 64)
pub const MAX_ROWS: usize = 1024;

/// One pack of a release, checked: its id, version, asset name and hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub version: String,
    pub zip: String,
    pub sha256: String,
}

/// Where a release's files come from.
#[derive(Debug, Clone)]
pub enum Source {
    /// A folder holding the release's files (tests, a host without internet access).
    Folder(PathBuf),
    /// An https base URL; `<base>/<name>` is fetched with curl.
    Url(String),
}

/// A Windows device name (CON, PRN, AUX, NUL, COM1-9, LPT1-9, any case, with any extension): never a file name.
fn device_name(s: &str) -> bool {
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

/// A plain word: letters, digits, `.`, `_`, `-`, 1 to `max` characters, not starting with `-`, no `..`, not a Windows
/// device name. A remote row is a stranger's text, so its id, version and zip name must be plain before any of them
/// names a file.
fn plain(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && !s.starts_with('-')
        && !s.contains("..")
        && !device_name(s)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The rows of `packs_json` (an array, or `{"packs": [...]}`) whose declared hash equals the `sums` line of the same
/// zip, restricted to `only` when it is not empty; `Err` with every refusal when any row is bad or an `only` id is
/// missing (then nothing of the release may be installed).
pub fn rows(packs_json: &str, sums: &str, only: &[String]) -> Result<Vec<Row>, Vec<String>> {
    let doc: serde_json::Value = serde_json::from_str(packs_json.trim_start_matches('\u{feff}'))
        .map_err(|e| vec![format!("packs.json is not JSON: {e}")])?;
    let list = match doc.get("packs").unwrap_or(&doc).as_array() {
        Some(l) => l.clone(),
        None => return Err(vec!["packs.json holds no list of packs".to_string()]),
    };
    if list.len() > MAX_ROWS {
        return Err(vec![format!(
            "packs.json has {} rows, more than {MAX_ROWS}",
            list.len()
        )]);
    }
    let mut listed = std::collections::HashMap::new();
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(h), Some(n)) = (parts.next(), parts.next()) {
            listed.insert(
                n.trim_start_matches('*').to_string(),
                h.to_ascii_lowercase(),
            );
        }
    }
    let (mut out, mut bad, mut ids) = (Vec::new(), Vec::new(), Vec::new());
    for p in &list {
        let text = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let (id, version) = (text("id"), text("version"));
        if !plain(&id, 100) || !plain(&version, 100) {
            bad.push("a row has an id or version that is not a plain word".to_string());
            continue;
        }
        ids.push(id.clone());
        if !only.is_empty() && !only.contains(&id) {
            continue;
        }
        let zip = p
            .get("zip")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{id}-{version}.zip"));
        if !(plain(&zip, 200) && zip.len() > 4 && zip[zip.len() - 4..].eq_ignore_ascii_case(".zip"))
        {
            bad.push(format!(
                "{id} {version}: the zip name is not a plain file name"
            ));
            continue;
        }
        let declared = text("sha256").to_ascii_lowercase();
        if !is_sha(&declared) {
            bad.push(format!("{id} {version} {zip}: packs.json has no sha256"));
            continue;
        }
        match listed.get(&zip) {
            None => bad.push(format!("{id} {version} {zip}: not in SHA256SUMS")),
            Some(h) if *h != declared => bad.push(format!(
                "{id} {version} {zip}: SHA256SUMS says {}, packs.json says {}",
                &h[..h.len().min(12)],
                &declared[..12]
            )),
            Some(_) => out.push(Row {
                id,
                version,
                zip,
                sha256: declared,
            }),
        }
    }
    for o in only {
        if !ids.contains(o) {
            bad.push(format!("{o}: not in packs.json"));
        }
    }
    if bad.is_empty() {
        Ok(out)
    } else {
        Err(bad)
    }
}

/// The curl program (`DC_CURL` overrides; on Windows `curl` finds the system's curl.exe).
fn curl() -> String {
    std::env::var("DC_CURL").unwrap_or_else(|_| "curl".to_string())
}

/// The release file `name` from `src` as a local file: the folder's own file, or a download into `work`.
fn fetch(src: &Source, name: &str, work: &Path, max: u64) -> Result<PathBuf, String> {
    match src {
        Source::Folder(d) => {
            let p = d.join(name);
            p.is_file()
                .then_some(p)
                .ok_or_else(|| format!("{name} is not in {}", d.display()))
        }
        Source::Url(base) => {
            // curl writes to its stdout and the bytes are counted here, so the cap holds for a chunked answer too
            // (--max-filesize only sees a declared length); -q ignores any .curlrc, --globoff takes the URL as text
            let out = work.join(name);
            let mut child = Command::new(curl())
                .args(["-q", "-fsSL", "--globoff", "--proto", "=https", "--proto-redir", "=https"])
                .args(["--max-time", "600", "--max-filesize", &max.to_string()])
                .arg(format!("{base}/{name}"))
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| {
                    format!(
                        "cannot run curl ({e}): Windows 10 and later, macOS and Linux include it; or download the \
                         release's files into a folder and give it as --base-url"
                    )
                })?;
            let copied = (|| -> Result<u64, String> {
                let mut f = std::fs::File::create(&out).map_err(|e| format!("{name}: {e}"))?;
                let stdout = child.stdout.take().ok_or("curl gave no output")?;
                let n = std::io::copy(&mut stdout.take(max + 1), &mut f)
                    .map_err(|e| format!("{name}: {e}"))?;
                f.flush().map_err(|e| format!("{name}: {e}"))?;
                Ok(n)
            })();
            if matches!(copied, Ok(n) if n > max) || copied.is_err() {
                let _ = child.kill();
            }
            let status = child.wait().map_err(|e| format!("curl: {e}"))?;
            match copied {
                Ok(n) if n > max => {
                    let _ = std::fs::remove_file(&out);
                    Err(format!("{name} is more than {max} bytes"))
                }
                Ok(_) if status.success() => Ok(out),
                Ok(_) => Err(format!(
                    "{name} could not be downloaded from {base} (curl {status})"
                )),
                Err(e) => Err(e),
            }
        }
    }
}

fn read_limited(p: &Path, max: u64, what: &str) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(p)
        .map_err(|e| format!("{what}: {e}"))?
        .len();
    if len > max {
        return Err(format!("{what} is {len} bytes, more than {max}"));
    }
    std::fs::read(p).map_err(|e| format!("{what}: {e}"))
}

fn sha_of_file(p: &Path) -> Option<String> {
    std::fs::read(p).ok().map(|b| dc_rom::sha256_hex(&b))
}

/// True when no existing component of `p` is a symbolic link (checked right before the install writes there).
fn no_symlink(p: &Path) -> bool {
    let mut acc = PathBuf::new();
    for c in p.components() {
        acc.push(c);
        if std::fs::symlink_metadata(&acc).is_ok_and(|m| m.file_type().is_symlink()) {
            return false;
        }
    }
    true
}

/// What one sync run of a release asks for.
pub struct Request<'a> {
    pub label: &'a str,
    pub mods_dir: &'a Path,
    pub only: &'a [String],
    pub dry_run: bool,
    pub format_set: bool,
}

/// Installs the packs of one release from `src` into `req.mods_dir` (see the module docs); the report lines go to
/// `out`. Returns the exit code (0 installed or present, 1 a check failed: nothing installed).
pub fn sync_release(src: &Source, req: &Request<'_>, out: &mut Vec<String>) -> i32 {
    let work = std::env::temp_dir().join(format!(
        "dc-packs-sync-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    if let Err(e) = std::fs::create_dir_all(&work) {
        out.push(format!("error: a work folder: {e}"));
        return 1;
    }
    let code = sync_in(src, req, &work, out);
    let _ = std::fs::remove_dir_all(&work);
    code
}

fn sync_in(src: &Source, req: &Request<'_>, work: &Path, out: &mut Vec<String>) -> i32 {
    out.push(format!("release {}", req.label));
    let lists = fetch(src, "SHA256SUMS", work, MAX_LIST_BYTES)
        .and_then(|p| read_limited(&p, MAX_LIST_BYTES, "SHA256SUMS"))
        .map_err(|e| format!("{e}: no release of that tag, or it has no SHA256SUMS"))
        .and_then(|sums| {
            fetch(src, "packs.json", work, MAX_LIST_BYTES)
                .and_then(|p| read_limited(&p, MAX_LIST_BYTES, "packs.json"))
                .map_err(|e| format!("{e} (the release was not made by the packs workflow)"))
                .map(|packs| (sums, packs))
        });
    let (sums, packs) = match lists {
        Ok(l) => l,
        Err(e) => {
            out.push(format!("error: {e}: nothing installed"));
            return 1;
        }
    };
    let selected = match rows(
        &String::from_utf8_lossy(&packs),
        &String::from_utf8_lossy(&sums),
        req.only,
    ) {
        Ok(r) if r.is_empty() => {
            out.push("error: no pack selected".to_string());
            return 1;
        }
        Ok(r) => r,
        Err(bad) => {
            out.extend(bad.iter().map(|b| format!("   refused: {b}")));
            out.push(
                "error: the release's packs.json and SHA256SUMS disagree, or a pack is missing: nothing installed"
                    .to_string(),
            );
            return 1;
        }
    };
    // phase 1: every selected zip present, or fetched and checked
    let mut fail = false;
    let mut plan: Vec<(&Row, Option<PathBuf>)> = Vec::new();
    for r in &selected {
        let dest = req.mods_dir.join(format!("{}.zip", r.sha256));
        if dest.is_file() && sha_of_file(&dest).as_deref() == Some(r.sha256.as_str()) {
            out.push(format!("  present   {} {} ({})", r.id, r.version, r.sha256));
            plan.push((r, None));
            continue;
        }
        if req.dry_run {
            out.push(format!(
                "  would get {} {} -> {}",
                r.id,
                r.version,
                dest.display()
            ));
            plan.push((r, None));
            continue;
        }
        let got = fetch(src, &r.zip, work, MAX_ZIP_BYTES)
            .and_then(|p| read_limited(&p, MAX_ZIP_BYTES, &r.zip).map(|b| (p, b)));
        match got {
            Err(e) => {
                out.push(format!("  FAILED    {} {}: {e}", r.id, r.version));
                fail = true;
            }
            Ok((p, bytes)) => {
                let h = dc_rom::sha256_hex(&bytes);
                if h != r.sha256 {
                    out.push(format!(
                        "  FAILED    {} {}: {} hashes {h}, the release lists {}",
                        r.id, r.version, r.zip, r.sha256
                    ));
                    fail = true;
                } else {
                    out.push(format!(
                        "  checked   {} {} ({}, {} bytes)",
                        r.id,
                        r.version,
                        r.sha256,
                        bytes.len()
                    ));
                    plan.push((r, Some(p)));
                }
            }
        }
    }
    if fail {
        out.push("error: a pack failed its check: nothing installed".to_string());
        return 1;
    }
    // phase 2: install what was checked (a part file renamed into place, so a zip of that name is always whole)
    if !req.dry_run && plan.iter().any(|(_, p)| p.is_some()) {
        if !no_symlink(req.mods_dir) {
            out.push(format!(
                "error: {} has a symbolic link in its path: nothing installed",
                req.mods_dir.display()
            ));
            return 1;
        }
        if let Err(e) = std::fs::create_dir_all(req.mods_dir) {
            out.push(format!(
                "error: {}: {e}: nothing installed",
                req.mods_dir.display()
            ));
            return 1;
        }
        let items: Vec<(&Row, &Path)> = plan
            .iter()
            .filter_map(|(r, p)| p.as_deref().map(|p| (*r, p)))
            .collect();
        if !install(&items, req.mods_dir, out) {
            return 1;
        }
    }
    // the rows to paste (server.toml is never edited here)
    out.push(String::new());
    if req.format_set {
        out.push(format!(
            "# [[mods.set]] rows (a modded lobby) for release {}:",
            req.label
        ));
        for (r, _) in &plan {
            out.push(format!(
                "[[mods.set]]\nid = \"{}\"\nversion = \"{}\"\nsha256 = \"{}\"\n",
                r.id, r.version, r.sha256
            ));
        }
    } else {
        out.push(format!(
            "# the entry's mods = [...] rows (a curated [[lobby.servers]] entry) for release {}:",
            req.label
        ));
        for (r, _) in &plan {
            out.push(format!(
                "  {{ id = \"{}\", version = \"{}\", sha256 = \"{}\" }},",
                r.id, r.version, r.sha256
            ));
        }
    }
    if req.dry_run {
        out.push("# dry run: nothing was installed".to_string());
    }
    0
}

/// Phase 2: installs checked zips into `mods` (which exists). Every zip is copied to a part file of this run and
/// hashed again there, all before the first rename: what lands under `<sha>.zip` is the bytes that hashed to it,
/// even if the source changed after the check. `false` (with the reason in `out`) when anything failed: then no
/// part file is left and nothing of this call is installed, unless a rename itself failed midway.
fn install(items: &[(&Row, &Path)], mods: &Path, out: &mut Vec<String>) -> bool {
    let run = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let mut parts: Vec<(&Row, PathBuf)> = Vec::new();
    let mut bad = None;
    for (r, p) in items {
        let part = mods.join(format!("{}.zip.{run}.part", r.sha256));
        let res = std::fs::copy(p, &part)
            .map_err(|e| e.to_string())
            .and_then(|_| {
                match read_limited(&part, MAX_ZIP_BYTES, &r.zip).map(|b| dc_rom::sha256_hex(&b)) {
                    Ok(h) if h == r.sha256 => Ok(()),
                    Ok(h) => Err(format!("its bytes changed after the check (now {h})")),
                    Err(e) => Err(e),
                }
            });
        parts.push((r, part));
        if let Err(e) = res {
            bad = Some(format!("{} {}: {e}", r.id, r.version));
            break;
        }
    }
    if let Some(e) = bad {
        for (_, part) in &parts {
            let _ = std::fs::remove_file(part);
        }
        out.push(format!("error: installing {e}: nothing installed"));
        return false;
    }
    for (r, part) in &parts {
        let dest = mods.join(format!("{}.zip", r.sha256));
        if let Err(e) = std::fs::rename(part, &dest) {
            for (_, p) in &parts {
                let _ = std::fs::remove_file(p);
            }
            out.push(format!("error: installing {} {}: {e}", r.id, r.version));
            return false;
        }
        out.push(format!(
            "  installed {} {} -> {}",
            r.id,
            r.version,
            dest.display()
        ));
    }
    true
}

/// `Ok` for a repo and tag the sync may fetch; the usage error otherwise.
fn check_release(repo: &str, tag: &str) -> Result<(), String> {
    if !dc_config::is_github_repo(repo) {
        return Err(format!("'{repo}' is not owner/repo"));
    }
    if dc_config::BRANCH_NAMES.contains(&tag)
        || ["head", "release", "stable", "nightly"].contains(&tag)
    {
        return Err(format!(
            "'{tag}' is a branch or an alias, not a release tag: a branch moves, a pack is its hash (use the tag, e.g. v1)"
        ));
    }
    if !dc_config::is_release_tag(tag) {
        return Err(format!(
            "'{tag}' is not a release tag (v1, v1.2.0, 2026.11)"
        ));
    }
    Ok(())
}

/// `dc-lobby-server packs ...`; returns the exit code.
pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(code) => code,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}");
            }
            eprintln!("{USAGE}");
            2
        }
    }
}

fn run_inner(args: &[String]) -> Result<i32, String> {
    let mut it = args.iter();
    match it.next().map(String::as_str) {
        Some("sync") => {}
        Some("-h") | Some("--help") | None => return Err(String::new()),
        Some(other) => return Err(format!("unknown packs command '{other}'")),
    }
    let (mut pos, mut only, mut mods, mut config, mut base) =
        (Vec::new(), Vec::new(), None, None, None);
    let (mut dry, mut set) = (false, false);
    while let Some(a) = it.next() {
        let mut val = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match a.as_str() {
            "--only" => {
                only = val("--only")?
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            }
            "--mods-dir" => mods = Some(PathBuf::from(val("--mods-dir")?)),
            "--config" => config = Some(PathBuf::from(val("--config")?)),
            "--base-url" => base = Some(val("--base-url")?),
            "--format" => match val("--format")?.as_str() {
                "entry" => set = false,
                "set" => set = true,
                f => return Err(format!("--format entry|set, not '{f}'")),
            },
            "--dry-run" => dry = true,
            "-h" | "--help" => return Err(String::new()),
            s if s.starts_with('-') => return Err(format!("unknown argument {s}")),
            s => pos.push(s.to_string()),
        }
    }
    let mut sources: Vec<(String, String)> = Vec::new();
    if let Some(c) = &config {
        let text = std::fs::read_to_string(c).map_err(|e| format!("{}: {e}", c.display()))?;
        let cfg = dc_config::parse_toml(&text).map_err(|issues| {
            let msgs: Vec<String> = issues.iter().map(|i| i.to_string()).collect();
            format!("{}: {}", c.display(), msgs.join("; "))
        })?;
        if mods.is_none() && !cfg.paths.mods_dir.is_empty() {
            mods = Some(PathBuf::from(&cfg.paths.mods_dir));
        }
        if pos.is_empty() {
            sources = cfg
                .packs
                .sources
                .iter()
                .map(|s| (s.repo.clone(), s.tag.clone()))
                .collect();
            if sources.is_empty() {
                return Err(format!("{} declares no [[packs.sources]]", c.display()));
            }
        }
    }
    match pos.as_slice() {
        [] if config.is_some() => {}
        [repo, tag] => sources = vec![(repo.clone(), tag.clone())],
        _ => return Err("give <owner/repo> <tag>, or --config server.toml".to_string()),
    }
    if sources.len() > 1 && (!only.is_empty() || base.is_some()) {
        return Err(
            "--only and --base-url name one release: give <owner/repo> <tag> with them".to_string(),
        );
    }
    let mods =
        mods.ok_or("no mods folder: --mods-dir DIR, or paths.mods_dir in --config server.toml")?;
    for (repo, tag) in &sources {
        check_release(repo, tag)?;
    }
    let mut worst = 0;
    for (repo, tag) in &sources {
        let src = match &base {
            Some(b) if Path::new(b).is_dir() => Source::Folder(PathBuf::from(b)),
            Some(b) if b.starts_with("https://") => {
                Source::Url(b.trim_end_matches('/').to_string())
            }
            Some(b) => {
                return Err(format!(
                    "--base-url '{b}' is neither a folder nor an https URL"
                ))
            }
            None => Source::Url(format!("https://github.com/{repo}/releases/download/{tag}")),
        };
        let label = format!("{repo} {tag}");
        let req = Request {
            label: &label,
            mods_dir: &mods,
            only: &only,
            dry_run: dry,
            format_set: set,
        };
        let mut out = Vec::new();
        let code = sync_release(&src, &req, &mut out);
        for line in &out {
            println!("{line}");
        }
        worst = worst.max(code);
    }
    Ok(worst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake release folder: two packs, their zips, packs.json and SHA256SUMS (made-up bytes).
    fn release(tag: &str) -> (PathBuf, String, String) {
        let d = std::env::temp_dir().join(format!("dc_packsync_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (a, b) = (b"PK fake camo a".to_vec(), b"PK fake reel b".to_vec());
        std::fs::write(d.join("camo_a-1.0.0.zip"), &a).unwrap();
        std::fs::write(d.join("reel_b-2.0.0.zip"), &b).unwrap();
        let (sa, sb) = (dc_rom::sha256_hex(&a), dc_rom::sha256_hex(&b));
        std::fs::write(
            d.join("packs.json"),
            format!(
                r#"{{"packs":[{{"id":"camo_a","version":"1.0.0","sha256":"{sa}"}},{{"id":"reel_b","version":"2.0.0","sha256":"{sb}","zip":"reel_b-2.0.0.zip"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            d.join("SHA256SUMS"),
            format!("{sa}  camo_a-1.0.0.zip\n{sb} *reel_b-2.0.0.zip\n"),
        )
        .unwrap();
        (d, sa, sb)
    }

    fn mods(tag: &str) -> PathBuf {
        // macOS: the temp folder is under /var, a symbolic link to /private/var, and an install refuses a path with
        // a link in it (`no_symlink`); the tests install into the folder's real path there
        let tmp = std::env::temp_dir();
        let tmp = if cfg!(target_os = "macos") {
            tmp.canonicalize().unwrap_or(tmp)
        } else {
            tmp
        };
        let d = tmp.join(format!("dc_packsync_mods_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn sync(rel: &Path, m: &Path, only: &[String], dry: bool, set: bool) -> (i32, String) {
        let req = Request {
            label: "o/r v1",
            mods_dir: m,
            only,
            dry_run: dry,
            format_set: set,
        };
        let mut out = Vec::new();
        let c = sync_release(&Source::Folder(rel.to_path_buf()), &req, &mut out);
        (c, out.join("\n"))
    }

    fn zips(m: &Path) -> usize {
        std::fs::read_dir(m)
            .map(|r| {
                r.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "zip"))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn installs_by_hash_then_a_rerun_changes_nothing() {
        let (rel, sa, sb) = release("happy");
        let m = mods("happy");
        let (c, out) = sync(&rel, &m, &[], false, false);
        assert_eq!(c, 0, "{out}");
        assert_eq!(
            std::fs::read(m.join(format!("{sa}.zip"))).unwrap(),
            b"PK fake camo a"
        );
        assert!(m.join(format!("{sb}.zip")).is_file());
        assert_eq!(zips(&m), 2);
        assert!(
            out.contains(&format!(
                "{{ id = \"camo_a\", version = \"1.0.0\", sha256 = \"{sa}\" }},"
            )),
            "{out}"
        );
        let before = std::fs::metadata(m.join(format!("{sa}.zip")))
            .unwrap()
            .modified()
            .unwrap();
        let (c, out) = sync(&rel, &m, &[], false, false);
        assert_eq!(c, 0);
        assert!(
            out.contains("present   camo_a") && !out.contains("installed"),
            "{out}"
        );
        assert_eq!(
            std::fs::metadata(m.join(format!("{sa}.zip")))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
        assert_eq!(zips(&m), 2);
    }

    #[test]
    fn only_and_set_rows_and_an_unknown_id() {
        let (rel, _, sb) = release("only");
        let m = mods("only");
        let (c, out) = sync(&rel, &m, &["reel_b".to_string()], false, true);
        assert_eq!(c, 0, "{out}");
        assert_eq!(zips(&m), 1);
        assert!(
            out.contains(&format!(
                "[[mods.set]]\nid = \"reel_b\"\nversion = \"2.0.0\"\nsha256 = \"{sb}\""
            )),
            "{out}"
        );
        let (c, out) = sync(&rel, &mods("only2"), &["nope".to_string()], false, false);
        assert_eq!(c, 1, "{out}");
        assert!(out.contains("nope: not in packs.json"));
    }

    #[test]
    fn a_dry_run_installs_nothing() {
        let (rel, _, _) = release("dry");
        let m = mods("dry");
        let (c, out) = sync(&rel, &m, &[], true, false);
        assert_eq!(c, 0, "{out}");
        assert!(!m.exists() && out.contains("would get") && out.contains("dry run"));
    }

    #[test]
    fn a_sums_mismatch_installs_nothing_at_all() {
        let (rel, sa, _) = release("sums");
        let other = "0".repeat(64);
        std::fs::write(
            rel.join("SHA256SUMS"),
            format!("{sa}  camo_a-1.0.0.zip\n{other}  reel_b-2.0.0.zip\n"),
        )
        .unwrap();
        let m = mods("sums");
        let (c, out) = sync(&rel, &m, &[], false, false);
        assert_eq!(c, 1, "{out}");
        assert_eq!(zips(&m), 0, "camo_a was fine but nothing is installed");
    }

    #[test]
    fn a_swapped_or_missing_asset_installs_nothing() {
        let (rel, _, _) = release("swap");
        std::fs::write(rel.join("reel_b-2.0.0.zip"), b"PK something else").unwrap();
        let m = mods("swap");
        let (c, out) = sync(&rel, &m, &[], false, false);
        assert_eq!(c, 1, "{out}");
        assert!(out.contains("FAILED    reel_b"));
        assert_eq!(zips(&m), 0);
        let (rel, _, _) = release("missing");
        std::fs::remove_file(rel.join("reel_b-2.0.0.zip")).unwrap();
        let m = mods("missing");
        let (c, _) = sync(&rel, &m, &[], false, false);
        assert_eq!((c, zips(&m)), (1, 0));
    }

    #[test]
    fn bytes_changed_after_the_check_are_not_installed() {
        let (rel, sa, sb) = release("toctou");
        let m = mods("toctou");
        std::fs::create_dir_all(&m).unwrap();
        let ra = Row {
            id: "camo_a".into(),
            version: "1.0.0".into(),
            zip: "camo_a-1.0.0.zip".into(),
            sha256: sa.clone(),
        };
        let rb = Row {
            id: "reel_b".into(),
            version: "2.0.0".into(),
            zip: "reel_b-2.0.0.zip".into(),
            sha256: sb,
        };
        // reel_b passed its check, then its file was swapped before the install
        std::fs::write(rel.join("reel_b-2.0.0.zip"), b"PK planted").unwrap();
        let (pa, pb) = (rel.join("camo_a-1.0.0.zip"), rel.join("reel_b-2.0.0.zip"));
        let mut out = Vec::new();
        assert!(!install(&[(&ra, &pa), (&rb, &pb)], &m, &mut out));
        assert!(
            out.join("\n").contains("changed after the check"),
            "{out:?}"
        );
        assert_eq!(
            std::fs::read_dir(&m).unwrap().count(),
            0,
            "no zip and no part file left"
        );
        let mut out = Vec::new();
        assert!(install(&[(&ra, &pa)], &m, &mut out));
        assert!(m.join(format!("{sa}.zip")).is_file());
        assert_eq!(
            std::fs::read_dir(&m).unwrap().count(),
            1,
            "no part file left"
        );
    }

    #[test]
    fn a_release_without_lists_is_refused() {
        let (rel, _, _) = release("nolist");
        std::fs::remove_file(rel.join("SHA256SUMS")).unwrap();
        let (c, out) = sync(&rel, &mods("nolist"), &[], false, false);
        assert_eq!(c, 1);
        assert!(out.contains("SHA256SUMS"), "{out}");
    }

    #[test]
    fn rows_refuse_names_that_are_not_plain_words() {
        let h = "a".repeat(64);
        let sums = format!("{h}  x.zip\n");
        for bad in [
            format!(r#"[{{"id":"../up","version":"1","sha256":"{h}","zip":"x.zip"}}]"#),
            format!(r#"[{{"id":"a b","version":"1","sha256":"{h}","zip":"x.zip"}}]"#),
            format!(r#"[{{"id":"-x","version":"1","sha256":"{h}","zip":"x.zip"}}]"#),
            format!(r#"[{{"id":"a","version":"1","sha256":"{h}","zip":"../x.zip"}}]"#),
            format!(r#"[{{"id":"a","version":"1","sha256":"{h}","zip":"x.exe"}}]"#),
            r#"[{"id":"a","version":"1","zip":"x.zip"}]"#.to_string(),
            format!(r#"[{{"id":"con","version":"1","sha256":"{h}","zip":"x.zip"}}]"#),
            format!(r#"[{{"id":"a","version":"1","sha256":"{h}","zip":"LPT1.zip"}}]"#),
            format!(r#"[{{"id":"a","version":"1","sha256":"{h}","zip":"Nul.tar.zip"}}]"#),
        ] {
            assert!(rows(&bad, &sums, &[]).is_err(), "{bad}");
        }
        assert!(plain("console", 100) && plain("com10", 100) && plain("lpt0", 100));
        let many = format!("[{}]", vec![r#"{"id":"a"}"#; MAX_ROWS + 1].join(","));
        assert!(rows(&many, &sums, &[]).unwrap_err()[0].contains("rows"));
        let good = format!(r#"[{{"id":"a","version":"1","sha256":"{h}","zip":"x.zip"}}]"#);
        assert_eq!(rows(&good, &sums, &[]).unwrap().len(), 1);
    }

    #[test]
    fn branches_aliases_and_bad_repos_are_usage_errors() {
        for t in [
            "main", "master", "latest", "HEAD", "head", "nightly", "stable", "x1", "",
        ] {
            assert!(check_release("o/r", t).is_err(), "{t}");
        }
        for r in ["o", "o/r/x", "o/..", "o r/x"] {
            assert!(check_release(r, "v1").is_err(), "{r}");
        }
        assert!(check_release("argallo/DeCombat-mods", "v1").is_ok());
        assert!(check_release("o/r", "2026.11").is_ok());
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(run(&args(&["sync", "o/r", "main", "--mods-dir", "m"])), 2);
        assert_eq!(run(&args(&["sync", "o/r"])), 2);
        assert_eq!(run(&args(&["sync", "o/r", "v1"])), 2, "no mods folder");
        assert_eq!(run(&args(&["frob"])), 2);
    }

    #[test]
    fn config_mode_syncs_every_declared_source_into_its_mods_dir() {
        let (rel, sa, _) = release("cfg");
        let m = mods("cfg");
        let cfg = std::env::temp_dir().join(format!("dc_packsync_cfg_{}.toml", std::process::id()));
        let fwd = |p: &Path| p.display().to_string().replace('\\', "/");
        std::fs::write(
            &cfg,
            format!(
                "[paths]\nmods_dir = \"{}\"\n\n[[packs.sources]]\nrepo = \"o/r\"\ntag = \"v1\"\n",
                fwd(&m)
            ),
        )
        .unwrap();
        let a = [
            "sync".to_string(),
            "--config".to_string(),
            cfg.display().to_string(),
            "--base-url".to_string(),
            rel.display().to_string(),
        ];
        assert_eq!(run(&a), 0);
        assert!(m.join(format!("{sa}.zip")).is_file());
        let _ = std::fs::remove_file(&cfg);
    }
}

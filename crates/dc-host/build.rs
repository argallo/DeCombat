//! The git short sha for `dc-host --version` and the hello line (HOST_AGENT.md §3). `DC_GIT_COMMIT` at build time
//! wins (a build from a copy without `.git`, as tools/vps/push.sh makes); else `git rev-parse --short HEAD`; else
//! "unknown". Nothing here changes the build itself.
// Provenance: ORIGINAL

use std::path::Path;
use std::process::Command;

/// `git <args>` run in this crate's folder, trimmed; `None` when git is missing, fails or prints nothing.
fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DC_GIT_COMMIT");
    let from_env = std::env::var("DC_GIT_COMMIT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if from_env.is_none() {
        // OPS-29: watch what moves HEAD, where git says it is (a worktree's `.git` is a file naming the real folder):
        // the HEAD file, the branch's loose ref and packed-refs. Only paths that exist: cargo reruns a build script on
        // every build while one of its watched paths is missing. Not the index, which changes on nearly every git
        // command and would rebuild this crate each time.
        let mut watch = vec![git(&["rev-parse", "--git-path", "HEAD"])];
        if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
            watch.push(git(&["rev-parse", "--git-path", &branch]));
        }
        watch.push(git(&["rev-parse", "--git-path", "packed-refs"]));
        for p in watch.into_iter().flatten() {
            if Path::new(&p).exists() {
                println!("cargo:rerun-if-changed={p}");
            }
        }
    }
    let sha = from_env
        .or_else(|| git(&["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=DC_HOST_GIT_SHA={sha}");
}

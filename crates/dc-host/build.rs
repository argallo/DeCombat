//! The git short sha for `dc-host --version` and the hello line (HOST_AGENT.md §3). `DC_GIT_COMMIT` at build time
//! wins (a build from a copy without `.git`, as tools/vps/push.sh makes); else `git rev-parse --short HEAD`; else
//! "unknown". Nothing here changes the build itself.
// Provenance: ORIGINAL

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DC_GIT_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    let sha = std::env::var("DC_GIT_COMMIT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=DC_HOST_GIT_SHA={sha}");
}

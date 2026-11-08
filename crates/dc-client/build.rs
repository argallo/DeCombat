//! Build facts for the render determinism manifest (`dc_client::exact`, docs/RENDER_DETERMINISM_CONTRACT.md): the
//! compiler that built this crate, the target triple, the build profile and the wgpu version of the lock file.
//! Nothing here changes the build itself.
// Provenance: ORIGINAL

use std::process::Command;

include!("winres.rs");

fn main() {
    // CLI-1906: the file name the Windows zip gives the exe
    windows_resources("DeCombat game client", "DeCombat.exe", Some("../../assets/icon/decombat.ico"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    // LG-322: a release build knows its version from the tag (tools/ci_env.sh); no other build claims one
    println!("cargo:rerun-if-env-changed=DC_RELEASE_VERSION");
    // CLI-1840: the commit a diagnostic report names (tools/ci_env.sh sets it; a local build says "unknown")
    println!("cargo:rerun-if-env-changed=DC_GIT_COMMIT");
    if let Some(c) = std::env::var("DC_GIT_COMMIT").ok().filter(|c| !c.trim().is_empty()) {
        println!("cargo:rustc-env=DC_GIT_COMMIT={}", c.trim());
    }
    println!(
        "cargo:rustc-env=DC_RELEASE_VERSION={}",
        std::env::var("DC_RELEASE_VERSION").unwrap_or_default()
    );
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=DC_RUSTC_VERSION={version}");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=DC_TARGET_TRIPLE={target}");
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    let opt = std::env::var("OPT_LEVEL").unwrap_or_else(|_| "?".into());
    println!("cargo:rustc-env=DC_BUILD_PROFILE={profile} opt-level {opt}");
    // CLI-1944: the marker tools/friend_pack_win.py reads out of the built exe, so a pack is never named after a
    // build the exe is not (a dev build says commit unknown, version dev)
    let stamp_commit = std::env::var("DC_GIT_COMMIT")
        .ok()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let stamp_version = std::env::var("DC_RELEASE_VERSION")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "dev".into());
    println!(
        "cargo:rustc-env=DC_BUILD_STAMP=DCSTAMP1;commit={stamp_commit};version={stamp_version};target={target};profile={profile};end"
    );
    let wgpu = std::fs::read_to_string("../../Cargo.lock")
        .ok()
        .and_then(|lock| {
            let mut lines = lock.lines();
            while let Some(l) = lines.next() {
                if l.trim() == "name = \"wgpu\"" {
                    return lines.next().map(|v| {
                        v.trim()
                            .trim_start_matches("version = ")
                            .trim_matches('"')
                            .to_string()
                    });
                }
            }
            None
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=DC_WGPU_VERSION={wgpu}");
}

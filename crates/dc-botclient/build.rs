//! Windows file details for the exe (`../dc-client/winres.rs`). Nothing here changes the build itself.
// Provenance: ORIGINAL

include!("../dc-client/winres.rs");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    windows_resources("DeCombat bots", "dc-botclient.exe", Some("../../assets/icon/decombat.ico"));
    println!("cargo:rerun-if-env-changed=DC_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=DC_RELEASE_VERSION");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
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
}

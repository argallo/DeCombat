//! Windows file details for the exe (`../dc-client/winres.rs`). Nothing here changes the build itself.
// Provenance: ORIGINAL

include!("../dc-client/winres.rs");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    windows_resources("DeCombat bots", "dc-botclient.exe", Some("../../assets/icon/decombat.ico"));
}

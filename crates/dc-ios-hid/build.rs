// Compiles the Objective-C bridge (src/hid.m) for iOS targets and links the GameController and UIKit frameworks.
//
// The cc crate finds the SDK itself (`xcrun --sdk iphoneos|iphonesimulator --show-sdk-path`, from the target triple)
// and passes the deployment target; the bridge guards nothing with `@available` (which would need compiler-rt's
// `__isOSVersionAtLeast`), it only calls iOS 14 APIs and the app requires iPadOS 14 or later.
fn main() {
    println!("cargo:rerun-if-changed=src/hid.m");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("ios") {
        return;
    }
    cc::Build::new()
        .file("src/hid.m")
        .flag("-fobjc-arc")
        .flag("-fmodules")
        .flag("-Wno-unguarded-availability-new")
        .warnings(true)
        .compile("dc_ios_hid");
    for framework in ["GameController", "UIKit", "Foundation", "QuartzCore"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

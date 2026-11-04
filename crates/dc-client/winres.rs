// Windows file details and icon for an exe (included by the build scripts of dc-client and dc-botclient).
// Antivirus heuristics treat a program with no publisher, version or icon as more suspicious. A build without the
// resource compiler (`RC`, else `rc.exe` on PATH, else the newest Windows Kits copy) still succeeds, with a warning.
// Provenance: ORIGINAL

/// Embeds the version block (`description`, `file_name`) and, when given, the icon into the exe of a Windows MSVC build.
fn windows_resources(description: &str, file_name: &str, icon: Option<&str>) {
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=DC_RELEASE_VERSION");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows-msvc") {
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found: the exe gets no version details or icon");
        return;
    };
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let shown = match std::env::var("DC_RELEASE_VERSION") {
        Ok(v) if !v.trim().is_empty() => v.trim().trim_start_matches('v').to_string(),
        _ => pkg.clone(),
    };
    let nums: Vec<u32> = shown
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(3)
        .filter_map(|s| s.parse().ok())
        .collect();
    let n = |i: usize| nums.get(i).copied().unwrap_or(0);
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let esc = |s: &str| s.replace('"', "\"\"");
    let mut rc_text = String::new();
    if let Some(icon) = icon {
        let path = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join(icon);
        println!("cargo:rerun-if-changed={}", path.display());
        rc_text.push_str(&format!("1 ICON \"{}\"\n", esc(&path.display().to_string().replace(std::path::MAIN_SEPARATOR, "/"))));
    }
    rc_text.push_str(&format!(
        "1 VERSIONINFO\nFILEVERSION {a},{b},{c},0\nPRODUCTVERSION {a},{b},{c},0\nFILEOS 0x40004\nFILETYPE 0x1\n\
         BEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904B0\"\n  BEGIN\n\
         \x20  VALUE \"CompanyName\", \"DeCombat\"\n\
         \x20  VALUE \"FileDescription\", \"{d}\"\n\
         \x20  VALUE \"FileVersion\", \"{v}\"\n\
         \x20  VALUE \"InternalName\", \"{f}\"\n\
         \x20  VALUE \"LegalCopyright\", \"DeCombat contributors, MIT or Apache-2.0\"\n\
         \x20  VALUE \"OriginalFilename\", \"{f}\"\n\
         \x20  VALUE \"ProductName\", \"DeCombat\"\n\
         \x20  VALUE \"ProductVersion\", \"{v}\"\n\
         \x20 END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x409, 1200\n END\nEND\n",
        a = n(0),
        b = n(1),
        c = n(2),
        d = esc(description),
        v = esc(&shown),
        f = esc(file_name),
    ));
    let rc_file = out.join("app.rc");
    let res_file = out.join("app.res");
    std::fs::write(&rc_file, rc_text).expect("write app.rc");
    let ok = std::process::Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&res_file)
        .arg(&rc_file)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={}", res_file.display());
    } else {
        println!("cargo:warning=rc.exe failed: the exe gets no version details or icon");
    }
}

fn find_rc() -> Option<std::path::PathBuf> {
    if let Ok(rc) = std::env::var("RC") {
        return Some(rc.into());
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("rc.exe");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let base = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut kits: Vec<_> = std::fs::read_dir(base).ok()?.flatten().map(|e| e.path().join("x64").join("rc.exe")).filter(|p| p.is_file()).collect();
    kits.sort();
    kits.pop()
}

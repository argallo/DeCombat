//! CLI-820: a --rom that is not a 2009 client folder is refused with a clear message.
#[test]
fn rom_folder_is_checked() {
    let base = std::env::temp_dir().join(format!("dc_rom_check_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let missing = base.join("nope");
    let e = dc_client::check_rom_dir(&missing).unwrap_err();
    assert!(e.contains("does not exist") && e.contains("nope"), "{e}");
    std::fs::create_dir_all(&base).unwrap();
    let e = dc_client::check_rom_dir(&base).unwrap_err();
    assert!(e.contains("no \"Game\" folder"), "{e}");
    std::fs::create_dir_all(base.join("Game")).unwrap();
    assert!(dc_client::check_rom_dir(&base).is_ok());
    let _ = std::fs::remove_dir_all(&base);
}

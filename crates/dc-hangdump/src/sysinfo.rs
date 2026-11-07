//! What a diagnostic report says about the machine (dc-client CLI-1840): the Windows version and build, the CPU's
//! name and the installed memory. Read-only queries; `None` where the platform or the call has no answer.
// ORIGINAL: our own diagnostics tooling (beta reports); no 2009 behaviour
// PUBLIC: Microsoft Win32 docs (RegGetValueW, GlobalMemoryStatusEx)

/// `Windows 11 Pro 24H2 (build 26100.1)` from the registry's CurrentVersion key.
#[cfg(windows)]
pub fn os_version() -> Option<String> {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let product = reg_string(KEY, "ProductName")?;
    let display = reg_string(KEY, "DisplayVersion").unwrap_or_default();
    let build = reg_string(KEY, "CurrentBuild").unwrap_or_default();
    let ubr = reg_dword(KEY, "UBR");
    // Windows 11 still names itself "Windows 10" in ProductName; the build number tells them apart
    let product = match build.parse::<u32>() {
        Ok(b) if b >= 22000 => product.replacen("Windows 10", "Windows 11", 1),
        _ => product,
    };
    let mut s = product;
    if !display.is_empty() {
        s.push(' ');
        s.push_str(&display);
    }
    if !build.is_empty() {
        match ubr {
            Some(u) => s.push_str(&format!(" (build {build}.{u})")),
            None => s.push_str(&format!(" (build {build})")),
        }
    }
    Some(s)
}

/// The first processor's name.
#[cfg(windows)]
pub fn cpu_name() -> Option<String> {
    reg_string(
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
    .map(|s| s.trim().to_string())
}

/// Installed memory in MiB.
#[cfg(windows)]
pub fn ram_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed, sized struct with its length set, as the call asks.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut m) != 0).then_some(m.ullTotalPhys / (1024 * 1024))
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn reg_string(key: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let (k, v) = (wide(key), wide(value));
    let mut buf = [0u16; 512];
    let mut len = std::mem::size_of_val(&buf) as u32;
    // SAFETY: NUL-terminated wide strings; the buffer and its byte length are ours.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            &mut len,
        )
    };
    if r != 0 {
        return None;
    }
    let n = (len as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    let s = s.trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(windows)]
fn reg_dword(key: &str, value: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD,
    };
    let (k, v) = (wide(key), wide(value));
    let mut d = 0u32;
    let mut len = 4u32;
    // SAFETY: NUL-terminated wide strings; a u32 out value of 4 bytes.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut d as *mut u32 as *mut core::ffi::c_void,
            &mut len,
        )
    };
    (r == 0).then_some(d)
}

#[cfg(not(windows))]
pub fn os_version() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn cpu_name() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn ram_mb() -> Option<u64> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn the_machine_answers() {
        let os = super::os_version().expect("os version");
        assert!(os.starts_with("Windows"), "{os}");
        assert!(super::ram_mb().unwrap_or(0) > 256);
        assert!(super::cpu_name().is_some());
    }
}

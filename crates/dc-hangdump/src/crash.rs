//! The process's last-chance exception filter (gate T1): an unhandled SEH exception (access violation, illegal
//! instruction, ...) writes `crash.txt` and `crash.dmp` (a minidump with the exception record) into the folder given
//! at install, then lets Windows go on as before (error reporting, process end). The first line of `crash.txt` names
//! the code, the faulting address with the module that holds it (`at 0x7fff102f8a32 (nvoglv64.dll+0x1c8a32)`, the file
//! name only) and the thread; the next lines are the `run:` line (build, renderer and adapter, see [`set_context`] and
//! [`add_context`]), for an access violation the kind of access and the address it touched, and every thread's
//! breadcrumbs. Rust panics do not come here (they unwind or abort through the panic hook, which prints the same
//! `run:` line).
// ORIGINAL: our own diagnostics tooling (gate T1); no 2009 behaviour
// PUBLIC: Microsoft Win32 docs (SetUnhandledExceptionFilter, MiniDumpWriteDump, MINIDUMP_EXCEPTION_INFORMATION,
// GetModuleHandleExW with GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleFileNameW, EXCEPTION_RECORD
// ExceptionInformation of an access violation)

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// `crash_<pid>/crash.dmp`, created at install so the filter opens no file in a crashed process.
static DUMP: OnceLock<std::fs::File> = OnceLock::new();

/// The one-line description of this run (build, renderer, adapter), pieces joined by `; `. Set before any crash.
static CONTEXT: Mutex<String> = Mutex::new(String::new());

/// Replaces the run description printed as `run: ...` in `crash.txt` and the panic report (call once at start).
pub fn set_context(s: String) {
    if let Ok(mut c) = CONTEXT.lock() {
        *c = s;
    }
}

/// Appends a piece to the run description (after `; `); a piece already there is not added twice.
pub fn add_context(piece: &str) {
    let piece = piece.trim();
    if piece.is_empty() {
        return;
    }
    if let Ok(mut c) = CONTEXT.lock() {
        if c.split("; ").any(|p| p == piece) {
            return;
        }
        if !c.is_empty() {
            c.push_str("; ");
        }
        c.push_str(piece);
    }
}

/// The `run: <context>` line, none while no context is set. Never blocks: a crash inside [`add_context`] must not
/// hang the report.
pub fn context_line() -> Option<String> {
    let c = match CONTEXT.try_lock() {
        Ok(c) => c,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(_) => return None,
    };
    (!c.is_empty()).then(|| format!("run: {c}"))
}

/// `read of 0x...` / `write of 0x...` / `execute of 0x...` for an access violation (`ExceptionInformation[0]` is
/// 0 read, 1 write, 8 execute; `[1]` the address touched); none for any other exception or too few parameters.
pub fn access_violation_line(code: u32, params: &[usize]) -> Option<String> {
    if code != 0xc000_0005 || params.len() < 2 {
        return None;
    }
    let kind = match params[0] {
        0 => "read",
        1 => "write",
        8 => "execute",
        _ => "access",
    };
    Some(format!("access violation: {kind} of 0x{:x}", params[1]))
}

/// The first lines of `crash.txt`: the exception, the `run:` line, the access violation detail.
pub fn head_lines(
    code: u32,
    addr: usize,
    module: Option<(&str, usize)>,
    tid: u32,
    params: &[usize],
    context: Option<String>,
) -> Vec<String> {
    let place = match module {
        Some((name, off)) => format!("0x{addr:08x} ({name}+0x{off:x})"),
        None => format!("0x{addr:08x}"),
    };
    let mut v = vec![format!(
        "unhandled exception 0x{code:08x} at {place} on thread {tid}; minidump crash.dmp"
    )];
    v.extend(context);
    v.extend(access_violation_line(code, params));
    v
}

/// The module (file name only) that holds `addr` and the offset of `addr` in it, none when the address is in no
/// loaded module. Windows only.
#[cfg(windows)]
pub fn module_of(addr: usize) -> Option<(String, usize)> {
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    let mut module = std::ptr::null_mut();
    // SAFETY: FROM_ADDRESS makes the first argument an address, not a string; the out pointer is a local.
    let ok = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            addr as *const u16,
            &mut module,
        )
    };
    if ok == 0 || module.is_null() {
        return None;
    }
    let mut buf = [0u16; 520];
    // SAFETY: `buf` is a local array of the length passed.
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n > buf.len() {
        return None;
    }
    let full = String::from_utf16_lossy(&buf[..n]);
    let name = full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
    Some((name, addr.checked_sub(module as usize)?))
}

/// Installs the filter once per process; `dir` is where the crash folder `crash_<pid>` goes. The folder and an empty
/// `crash.dmp` are created now; [`cleanup`] removes them again at a normal end.
pub fn install(dir: PathBuf) {
    if DIR.set(dir).is_err() {
        return;
    }
    if let Some(d) = crash_dir() {
        if std::fs::create_dir_all(&d).is_ok() {
            if let Ok(f) = std::fs::File::create(d.join("crash.dmp")) {
                let _ = DUMP.set(f);
            }
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter;
        // SAFETY: a plain function pointer with the documented signature.
        unsafe {
            SetUnhandledExceptionFilter(Some(filter));
        }
    }
}

/// A normal end: removes the empty `crash.dmp` and the folder made by [`install`] (only when nothing was written).
pub fn cleanup() {
    let Some(d) = crash_dir() else {
        return;
    };
    if d.join("crash.txt").exists() {
        return;
    }
    if std::fs::metadata(d.join("crash.dmp")).is_ok_and(|m| m.len() == 0) {
        let _ = std::fs::remove_file(d.join("crash.dmp"));
    }
    let _ = std::fs::remove_dir(&d);
}

/// The folder a crash of this process is written to (`crash_<pid>` under the install folder).
pub fn crash_dir() -> Option<PathBuf> {
    DIR.get()
        .map(|d| d.join(format!("crash_{}", std::process::id())))
}

#[cfg(windows)]
unsafe extern "system" fn filter(
    info: *const windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithThreadInfo, MiniDumpWriteDump,
        MINIDUMP_EXCEPTION_INFORMATION,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};
    const CONTINUE_SEARCH: i32 = 0;
    static ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return CONTINUE_SEARCH;
    }
    let Some(dir) = crash_dir() else {
        return CONTINUE_SEARCH;
    };
    let tid = crate::current_thread_id();
    // SAFETY: the system passes valid exception pointers to the filter.
    let (code, addr, params, nparams) = unsafe {
        match info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) {
            Some(r) => {
                let n = (r.NumberParameters as usize).min(2);
                let mut p = [0usize; 2];
                for (d, s) in p.iter_mut().zip(&r.ExceptionInformation[..n]) {
                    *d = *s;
                }
                (r.ExceptionCode as u32, r.ExceptionAddress as usize, p, n)
            }
            None => (0, 0, [0; 2], 0),
        }
    };
    // the dump first (into the file opened at install), the text after: formatting allocates
    if let Some(f) = DUMP.get() {
        let ei = MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: tid,
            ExceptionPointers: info as *mut _,
            ClientPointers: 0,
        };
        // SAFETY: our own process; the file handle lives while `f` does; `ei` points into this frame.
        unsafe {
            MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                f.as_raw_handle() as HANDLE,
                MiniDumpWithThreadInfo | MiniDumpWithIndirectlyReferencedMemory,
                &ei,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }
    let now = crate::crumb::now_ms();
    let module = module_of(addr);
    let mut text = head_lines(
        code,
        addr,
        module.as_ref().map(|(n, o)| (n.as_str(), *o)),
        tid,
        &params[..nparams],
        context_line(),
    );
    text.push("breadcrumbs (newest first per thread):".into());
    text.extend(crate::crumb::lines(&crate::crumb::snapshot(), now));
    let _ = std::fs::write(dir.join("crash.txt"), text.join("\n") + "\n");
    CONTINUE_SEARCH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn an_address_in_our_own_code_resolves_to_this_executable() {
        let addr = an_address_in_our_own_code_resolves_to_this_executable as fn() as usize;
        let (name, off) = module_of(addr).expect("own code is in a module");
        let exe = std::env::current_exe().unwrap();
        assert_eq!(name, exe.file_name().unwrap().to_string_lossy());
        assert!(off > 0 && off < 1 << 30, "offset {off:#x}");
        assert!(module_of(8).is_none());
    }

    #[test]
    fn context_pieces_compose_one_run_line() {
        // the only test that touches the process-wide string
        set_context("version 1.2.3; commit abc123".into());
        add_context("adapter Test GPU (Vulkan); driver 5.0");
        add_context("driver 5.0");
        add_context("  ");
        assert_eq!(
            context_line().as_deref(),
            Some("run: version 1.2.3; commit abc123; adapter Test GPU (Vulkan); driver 5.0")
        );
    }

    #[test]
    fn the_head_names_the_module_the_run_and_the_access() {
        let v = head_lines(
            0xc000_0005,
            0x7fff_102f_8a32,
            Some(("nvoglv64.dll", 0x1c8a32)),
            44752,
            &[0, 0x10],
            Some("run: version dev; commit unknown".into()),
        );
        assert_eq!(
            v,
            [
                "unhandled exception 0xc0000005 at 0x7fff102f8a32 (nvoglv64.dll+0x1c8a32) on thread 44752; minidump crash.dmp",
                "run: version dev; commit unknown",
                "access violation: read of 0x10",
            ]
        );
        let v = head_lines(0xc000_0005, 0x1000, None, 1, &[1, 0xdead], None);
        assert_eq!(
            v[0],
            "unhandled exception 0xc0000005 at 0x00001000 on thread 1; minidump crash.dmp"
        );
        assert_eq!(v[1], "access violation: write of 0xdead");
        assert_eq!(head_lines(0xc000_001d, 0, None, 1, &[0, 1], None).len(), 1);
    }
}

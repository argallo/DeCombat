//! The process's last-chance exception filter (gate T1): an unhandled SEH exception (access violation, illegal
//! instruction, ...) writes `crash.txt` (code, address, thread, every thread's breadcrumbs) and `crash.dmp` (a minidump
//! with the exception record) into the folder given at install, then lets Windows go on as before (error reporting,
//! process end). Rust panics do not come here (they unwind or abort through the panic hook).
// ORIGINAL: our own diagnostics tooling (gate T1); no 2009 behaviour
// PUBLIC: Microsoft Win32 docs (SetUnhandledExceptionFilter, MiniDumpWriteDump, MINIDUMP_EXCEPTION_INFORMATION)

use std::path::PathBuf;
use std::sync::OnceLock;

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// `crash_<pid>/crash.dmp`, created at install so the filter opens no file in a crashed process.
static DUMP: OnceLock<std::fs::File> = OnceLock::new();

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
    let (code, addr) = unsafe {
        match info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) {
            Some(r) => (r.ExceptionCode as u32, r.ExceptionAddress as usize),
            None => (0, 0),
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
    let mut text = vec![format!(
        "unhandled exception 0x{code:08x} at 0x{addr:08x} on thread {tid}; minidump crash.dmp"
    )];
    text.push("breadcrumbs (newest first per thread):".into());
    text.extend(crate::crumb::lines(&crate::crumb::snapshot(), now));
    let _ = std::fs::write(dir.join("crash.txt"), text.join("\n") + "\n");
    CONTINUE_SEARCH
}

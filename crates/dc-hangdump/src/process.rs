//! Counters of the own process for the heartbeat ledger (gate T1): open handles, threads, private bytes.
// ORIGINAL: our own diagnostics tooling (gate T1); no 2009 behaviour
// PUBLIC: Microsoft Win32 docs (GetProcessHandleCount, K32GetProcessMemoryInfo, CreateToolhelp32Snapshot)

/// One reading of the process counters; `None` where the platform has none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub handles: Option<u32>,
    pub threads: Option<u32>,
    pub private_bytes: Option<u64>,
}

#[cfg(windows)]
pub fn counters() -> Counters {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut c = Counters::default();
    // SAFETY: the pseudo handle of the own process; out values are plain integers / a sized struct.
    unsafe {
        let p = GetCurrentProcess();
        let mut n = 0u32;
        if GetProcessHandleCount(p, &mut n) != 0 {
            c.handles = Some(n);
        }
        let mut m: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        m.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if K32GetProcessMemoryInfo(p, &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb) != 0 {
            c.private_bytes = Some(m.PrivateUsage as u64);
        }
    }
    c.threads = Some(crate::thread_count());
    c
}

#[cfg(not(windows))]
pub fn counters() -> Counters {
    Counters::default()
}

//! Hang diagnostics for our own processes (fix-hang): the stacks of every thread of the calling process, each
//! thread suspended only while its registers and stack are copied (buffers allocated before the suspension, so a
//! thread that holds the heap lock cannot stop the sampler), walked with dbghelp and symbolized from the PDB next to
//! the executable; and a minidump of the process. Windows only; elsewhere the calls answer "not supported".
//!
//! This crate holds the `unsafe` Win32 calls so that dc-client keeps `forbid(unsafe_code)`.
// ORIGINAL: our own diagnostics tooling (fix-hang); no 2009 behaviour
// PUBLIC: Microsoft Win32 docs (SuspendThread, GetThreadContext, VirtualQuery, ReadProcessMemory, StackWalk64,
// SymInitialize, SymFromAddr, SymGetLineFromAddr64, MiniDumpWriteDump, CreateToolhelp32Snapshot, GetThreadDescription)

pub mod crash;
pub mod crumb;
pub mod process;
pub mod sysinfo;

#[cfg(windows)]
mod sys {
    use std::cell::Cell;
    use std::ffi::CStr;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddrModeFlat, GetThreadContext, MiniDumpWithIndirectlyReferencedMemory,
        MiniDumpWithThreadInfo, MiniDumpWriteDump, ReadProcessMemory, StackWalk64, SymFromAddr,
        SymFunctionTableAccess64, SymGetLineFromAddr64, SymGetModuleBase64, SymInitialize,
        SymSetOptions, CONTEXT, IMAGEHLP_LINE64, STACKFRAME64, SYMBOL_INFO, SYMOPT_DEFERRED_LOADS,
        SYMOPT_LOAD_LINES, SYMOPT_UNDNAME,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription,
        OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION,
        THREAD_SUSPEND_RESUME,
    };

    /// Most bytes of a thread's stack copied for the walk.
    const STACK_COPY: usize = 1 << 20;
    /// Most frames per stack.
    const MAX_FRAMES: usize = 64;
    /// Samples of the frame thread (a loop shows different tops, a wait the same one).
    const FRAME_SAMPLES: usize = 3;

    pub fn current_thread_id() -> u32 {
        // SAFETY: no arguments, no preconditions.
        unsafe { GetCurrentThreadId() }
    }

    thread_local! {
        /// The copied stack the walk reads from: (address of the first byte, pointer, length).
        static COPY: Cell<(u64, *const u8, usize)> = const { Cell::new((0, std::ptr::null(), 0)) };
    }

    unsafe extern "system" fn read_mem(
        process: HANDLE,
        base: u64,
        buf: *mut core::ffi::c_void,
        size: u32,
        read: *mut u32,
    ) -> i32 {
        let (start, ptr, len) = COPY.with(|c| c.get());
        let Some(end) = base.checked_add(size as u64) else {
            return 0;
        };
        if u128::from(end) > 1u128 << usize::BITS {
            // beyond this process's address space (a garbage frame pointer)
            return 0;
        }
        if !ptr.is_null() && base >= start && end <= start + len as u64 {
            // SAFETY: the range lies inside the copy, which outlives the walk; `buf` holds `size` bytes (dbghelp).
            unsafe {
                std::ptr::copy_nonoverlapping(
                    ptr.add((base - start) as usize),
                    buf as *mut u8,
                    size as usize,
                );
                if !read.is_null() {
                    *read = size;
                }
            }
            return 1;
        }
        let mut got = 0usize;
        // SAFETY: plain read of our own process's memory into dbghelp's buffer of `size` bytes.
        let ok = unsafe {
            ReadProcessMemory(
                process,
                base as usize as *const _,
                buf,
                size as usize,
                &mut got,
            )
        };
        if !read.is_null() {
            // SAFETY: dbghelp's out pointer.
            unsafe { *read = got as u32 };
        }
        ok
    }

    #[repr(C, align(16))]
    struct Ctx(CONTEXT);

    fn new_ctx() -> Box<Ctx> {
        // SAFETY: CONTEXT is plain old data; all-zero is a valid value.
        let mut c: Box<Ctx> = Box::new(unsafe { std::mem::zeroed() });
        #[cfg(target_arch = "x86")]
        {
            c.0.ContextFlags = windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_FULL_X86;
        }
        #[cfg(target_arch = "x86_64")]
        {
            c.0.ContextFlags = windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_FULL_AMD64;
        }
        c
    }

    #[cfg(target_arch = "x86")]
    const MACHINE: u32 = 0x014c;
    #[cfg(target_arch = "x86_64")]
    const MACHINE: u32 = 0x8664;

    #[cfg(target_arch = "x86")]
    fn regs(c: &CONTEXT) -> (u64, u64, u64) {
        (c.Eip as u64, c.Ebp as u64, c.Esp as u64)
    }
    #[cfg(target_arch = "x86_64")]
    fn regs(c: &CONTEXT) -> (u64, u64, u64) {
        (c.Rip, c.Rbp, c.Rsp)
    }

    struct Sample {
        ctx: Box<Ctx>,
        stack_at: u64,
        stack: Vec<u8>,
    }

    /// Suspends the thread, copies its registers and stack into buffers allocated BEFORE the suspension (no heap
    /// use while it is stopped: it may hold the heap lock), resumes it.
    fn sample(thread: HANDLE) -> Option<Sample> {
        let mut ctx = new_ctx();
        let mut stack = vec![0u8; STACK_COPY];
        // SAFETY: a thread handle with suspend / get-context rights; resumed on every path below.
        unsafe {
            if SuspendThread(thread) == u32::MAX {
                return None;
            }
            let ok = GetThreadContext(thread, &mut ctx.0) != 0;
            let mut copied = 0usize;
            let mut at = 0u64;
            if ok {
                let sp = regs(&ctx.0).2;
                let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                if VirtualQuery(
                    sp as usize as *const _,
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                ) != 0
                {
                    let end = mbi.BaseAddress as u64 + mbi.RegionSize as u64;
                    let n = (end.saturating_sub(sp) as usize).min(STACK_COPY);
                    let mut got = 0usize;
                    if ReadProcessMemory(
                        GetCurrentProcess(),
                        sp as usize as *const _,
                        stack.as_mut_ptr() as *mut _,
                        n,
                        &mut got,
                    ) != 0
                    {
                        copied = got;
                        at = sp;
                    }
                }
            }
            ResumeThread(thread);
            if !ok {
                return None;
            }
            stack.truncate(copied);
            Some(Sample {
                ctx,
                stack_at: at,
                stack,
            })
        }
    }

    fn symbol(addr: u64) -> String {
        let process = unsafe { GetCurrentProcess() };
        // SYMBOL_INFO followed by room for the name
        let mut buf = vec![0u64; (std::mem::size_of::<SYMBOL_INFO>() + 1024) / 8 + 1];
        let info = buf.as_mut_ptr() as *mut SYMBOL_INFO;
        let mut text = String::new();
        // SAFETY: `buf` is large enough for SYMBOL_INFO plus MaxNameLen bytes and 8-aligned.
        unsafe {
            (*info).SizeOfStruct = std::mem::size_of::<SYMBOL_INFO>() as u32;
            (*info).MaxNameLen = 1000;
            let mut disp = 0u64;
            if SymFromAddr(process, addr, &mut disp, info) != 0 {
                let name = CStr::from_ptr((*info).Name.as_ptr()).to_string_lossy();
                text = format!("{name}+0x{disp:x}");
            }
            let mut line: IMAGEHLP_LINE64 = std::mem::zeroed();
            line.SizeOfStruct = std::mem::size_of::<IMAGEHLP_LINE64>() as u32;
            let mut ld = 0u32;
            if SymGetLineFromAddr64(process, addr, &mut ld, &mut line) != 0
                && !line.FileName.is_null()
            {
                let f = CStr::from_ptr(line.FileName as *const _).to_string_lossy();
                text = format!("{text} ({f}:{})", line.LineNumber);
            }
            if text.is_empty() {
                let base = SymGetModuleBase64(process, addr);
                if base != 0 {
                    let mut name = [0u16; 260];
                    let n = GetModuleFileNameW(base as usize as _, name.as_mut_ptr(), 260) as usize;
                    let m = String::from_utf16_lossy(&name[..n]);
                    let m = m.rsplit('\\').next().unwrap_or("").to_string();
                    text = format!("{m}+0x{:x}", addr - base);
                } else {
                    text = "?".into();
                }
            }
        }
        format!("0x{addr:08x} {text}")
    }

    fn walk(thread: HANDLE, s: &mut Sample) -> Vec<String> {
        let process = unsafe { GetCurrentProcess() };
        let (pc, fp, sp) = regs(&s.ctx.0);
        // SAFETY: STACKFRAME64 is plain old data.
        let mut f: STACKFRAME64 = unsafe { std::mem::zeroed() };
        f.AddrPC.Offset = pc;
        f.AddrPC.Mode = AddrModeFlat;
        f.AddrFrame.Offset = fp;
        f.AddrFrame.Mode = AddrModeFlat;
        f.AddrStack.Offset = sp;
        f.AddrStack.Mode = AddrModeFlat;
        COPY.with(|c| c.set((s.stack_at, s.stack.as_ptr(), s.stack.len())));
        let mut lines = Vec::new();
        for _ in 0..MAX_FRAMES {
            // SAFETY: the context and frame are ours; the callbacks are dbghelp's own or read the copy.
            let ok = unsafe {
                StackWalk64(
                    MACHINE,
                    process,
                    thread,
                    &mut f,
                    &mut s.ctx.0 as *mut CONTEXT as *mut _,
                    Some(read_mem),
                    Some(SymFunctionTableAccess64),
                    Some(SymGetModuleBase64),
                    None,
                )
            };
            if ok == 0 || f.AddrPC.Offset == 0 {
                break;
            }
            lines.push(format!("    {}", symbol(f.AddrPC.Offset)));
        }
        COPY.with(|c| c.set((0, std::ptr::null(), 0)));
        lines
    }

    /// Words on the copied stack that point into a loaded module (return-address candidates, innermost first):
    /// the fallback when the walk stops early.
    fn scan(s: &Sample) -> Vec<String> {
        let process = unsafe { GetCurrentProcess() };
        let w = std::mem::size_of::<usize>();
        let mut out = Vec::new();
        for chunk in s.stack.chunks_exact(w) {
            let mut b = [0u8; 8];
            b[..w].copy_from_slice(chunk);
            let v = u64::from_le_bytes(b);
            if v < 0x10000 {
                continue;
            }
            // SAFETY: a lookup in dbghelp's module list.
            if unsafe { SymGetModuleBase64(process, v) } == 0 {
                continue;
            }
            let t = symbol(v);
            if t.contains(" ?") {
                continue;
            }
            out.push(format!("    {t}"));
            if out.len() >= 48 {
                break;
            }
        }
        out
    }

    fn thread_label(thread: HANDLE) -> String {
        let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
        // SAFETY: the out pointer is freed with LocalFree below.
        unsafe {
            if GetThreadDescription(thread, &mut p) >= 0 && !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 && n < 256 {
                    n += 1;
                }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                windows_sys::Win32::Foundation::LocalFree(p as _);
                return s;
            }
        }
        String::new()
    }

    pub fn thread_count() -> u32 {
        thread_ids().len() as u32
    }

    pub fn thread_description(tid: u32) -> Option<String> {
        use windows_sys::Win32::System::Threading::THREAD_QUERY_LIMITED_INFORMATION;
        // SAFETY: handle closed below.
        let h = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid) };
        if h.is_null() {
            return None;
        }
        let n = thread_label(h);
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        (!n.is_empty()).then_some(n)
    }

    fn thread_ids() -> Vec<u32> {
        let pid = unsafe { GetCurrentProcessId() };
        let mut ids = Vec::new();
        // SAFETY: snapshot handle closed below; THREADENTRY32 sized as the API asks.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return ids;
            }
            let mut te: THREADENTRY32 = std::mem::zeroed();
            te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut ok = Thread32First(snap, &mut te) != 0;
            while ok {
                if te.th32OwnerProcessID == pid {
                    ids.push(te.th32ThreadID);
                }
                ok = Thread32Next(snap, &mut te) != 0;
            }
            CloseHandle(snap);
        }
        ids
    }

    fn sym_init() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.display().to_string()))
                .unwrap_or_default();
            let path = std::ffi::CString::new(dir).unwrap_or_default();
            // SAFETY: once per process, from the watchdog thread only.
            unsafe {
                SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES);
                SymInitialize(GetCurrentProcess(), path.as_ptr() as _, 1);
            }
        });
    }

    /// The stacks of every thread but the caller (the frame thread `frame_tid` first, sampled several times).
    pub fn thread_stacks(frame_tid: u32) -> Vec<String> {
        sym_init();
        let me = current_thread_id();
        let mut ids = thread_ids();
        ids.retain(|&t| t != me);
        ids.sort_by_key(|&t| t != frame_tid);
        let mut out = Vec::new();
        for tid in ids {
            // SAFETY: handle closed below.
            let h = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                    0,
                    tid,
                )
            };
            if h.is_null() {
                out.push(format!("  thread {tid}: cannot open"));
                continue;
            }
            let name = thread_label(h);
            let samples = if tid == frame_tid { FRAME_SAMPLES } else { 1 };
            for k in 0..samples {
                let tag = if tid == frame_tid {
                    " FRAME THREAD"
                } else {
                    ""
                };
                out.push(format!("  thread {tid} \"{name}\"{tag} sample {}:", k + 1));
                match sample(h) {
                    Some(mut s) => {
                        let w = walk(h, &mut s);
                        let short = w.len() < 6;
                        out.extend(w);
                        if tid == frame_tid && (k == 0 || short) {
                            out.push(
                                "    -- stack scan (return-address candidates, innermost first):"
                                    .into(),
                            );
                            out.extend(scan(&s));
                        }
                    }
                    None => out.push("    (no context)".into()),
                }
                if k + 1 < samples {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            }
            // SAFETY: our handle.
            unsafe { CloseHandle(h) };
        }
        out
    }

    pub fn write_minidump(path: &Path) -> Result<(), String> {
        let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
        // SAFETY: our own process; the file handle is valid while `f` lives.
        let ok = unsafe {
            MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                f.as_raw_handle() as HANDLE,
                MiniDumpWithThreadInfo | MiniDumpWithIndirectlyReferencedMemory,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod sys {
    use std::path::Path;

    pub fn current_thread_id() -> u32 {
        0
    }

    pub fn thread_stacks(_frame_tid: u32) -> Vec<String> {
        vec!["  (thread stacks: Windows only)".into()]
    }

    pub fn write_minidump(_path: &Path) -> Result<(), String> {
        Err("Windows only".into())
    }

    pub fn thread_count() -> u32 {
        0
    }

    pub fn thread_description(_tid: u32) -> Option<String> {
        None
    }
}

/// The calling thread's id (0 where not supported).
pub fn current_thread_id() -> u32 {
    sys::current_thread_id()
}

/// Report lines with the stacks of every thread of the process but the caller, `focus_tid` first (sampled three
/// times, 150 ms apart, with a scan of its stack for return addresses).
pub fn thread_stacks(focus_tid: u32) -> Vec<String> {
    sys::thread_stacks(focus_tid)
}

/// The description (Rust thread name) of a thread of the own process, when it has one.
pub fn thread_description(tid: u32) -> Option<String> {
    sys::thread_description(tid)
}

/// The number of threads of the own process (0 where not supported).
pub fn thread_count() -> u32 {
    sys::thread_count()
}

/// Writes a minidump of the process (thread info and indirectly referenced memory) to `path`.
pub fn write_minidump(path: &std::path::Path) -> Result<(), String> {
    sys::write_minidump(path)
}

// Every test samples Windows thread stacks; other platforms only have the no-op `sys` above.
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
    use std::time::Duration;

    #[cfg(windows)]
    #[inline(never)]
    fn spin_here(stop: &AtomicBool) {
        while !stop.load(Relaxed) {
            std::hint::spin_loop();
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_spinning_thread_is_sampled_with_a_symbolized_stack() {
        use std::sync::Arc;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let s2 = stop.clone();
        let h = std::thread::spawn(move || {
            tx.send(current_thread_id()).unwrap();
            spin_here(&s2);
        });
        let tid = rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let lines = thread_stacks(tid);
        stop.store(true, Relaxed);
        h.join().unwrap();
        let text = lines.join("\n");
        if std::env::var_os("DC_HANGDUMP_SHOW").is_some() {
            eprintln!("{text}");
        }
        assert!(text.contains("FRAME THREAD sample 1"), "{text}");
        assert!(
            text.contains("spin_here"),
            "the spinning function is named:\n{text}"
        );
    }
}

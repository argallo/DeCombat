//! A message box for a process that has no window of its own (the friend pack's double-click start, CLI-1906): a
//! double-clicked console closes before its text can be read. Windows only; elsewhere the text goes to stderr.
// ORIGINAL: our own start-up notice; no 2009 behaviour
// PUBLIC: Microsoft Win32 docs (MessageBoxW)

/// Shows `text` under `title` and waits for OK (`error`: the error icon, else the information icon).
#[cfg(windows)]
pub fn message_box(title: &str, text: &str, error: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (t, c) = (wide(text), wide(title));
    let icon = if error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive the call; no owner window.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            t.as_ptr(),
            c.as_ptr(),
            MB_OK | icon | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
pub fn message_box(title: &str, text: &str, _error: bool) {
    eprintln!("{title}: {text}");
}

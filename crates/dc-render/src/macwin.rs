//! macOS window calls winit does not expose; no-ops on the other platforms. Main thread only (the event loop's).
//!
//! - [`system_cursor_hidden`]: winit hides the cursor through the view's cursor rects, which AppKit applies on the
//!   next cursor update; a locked mouse never moves the cursor, so a window that captured it kept the arrow on
//!   screen (dc-client CLI-1284, playtest report). `NSCursor::hide` / `unhide` are a balanced counter:
//!   callers pair the two.
//! - [`window_animations_off`]: winit's macOS backend closes every window when an event-loop run exits, and macOS
//!   animates a close as a fade to the desktop and the next order-front as a fade back; a window handed from one
//!   run to the next (dc-client's menu turns, the loading page, the match; CLI-1288) blinked to the desktop at every
//!   hand-over (playtest report). With `NSWindowAnimationBehaviorNone` the close and the re-show are
//!   instantaneous.
// PUBLIC: Apple docs (NSCursor hide / unhide; NSWindow animationBehavior; NSView window), winit 0.30 view.rs
// resetCursorRects and event_loop.rs notify_windows_of_exit; raw-window-handle 0.6 AppKitWindowHandle (ns_view)

/// Hides (`true`) or shows (`false`) the system cursor; paired calls. Main thread only (the event loop's).
pub fn system_cursor_hidden(hidden: bool) {
    #[cfg(target_os = "macos")]
    // SAFETY: plain class methods without arguments; the event loop thread is the main thread.
    unsafe {
        if hidden {
            objc2_app_kit::NSCursor::hide();
        } else {
            objc2_app_kit::NSCursor::unhide();
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = hidden;
}

/// Turns the window's AppKit animations off (`NSWindowAnimationBehaviorNone`) for the window of the AppKit view
/// `ns_view` (the `ns_view` of winit's `AppKitWindowHandle`). Returns whether a window was found. The other
/// platforms return `false`.
pub fn window_animations_off(ns_view: *mut std::ffi::c_void) -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2::rc::Retained;
        use objc2_app_kit::{NSView, NSWindow, NSWindowAnimationBehavior};
        if ns_view.is_null() {
            return false;
        }
        // SAFETY: the pointer is the NSView of a live winit window (raw-window-handle contract), retained for the
        // call; main thread.
        let view: Option<Retained<NSView>> = unsafe { Retained::retain(ns_view.cast::<NSView>()) };
        let Some(view) = view else {
            return false;
        };
        let window: Option<Retained<NSWindow>> = view.window();
        match window {
            Some(w) => {
                // SAFETY: a plain setter on a live window; main thread.
                unsafe { w.setAnimationBehavior(NSWindowAnimationBehavior::None) };
                true
            }
            None => false,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ns_view;
        false
    }
}

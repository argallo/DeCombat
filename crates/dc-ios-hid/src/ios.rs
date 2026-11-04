//! The iOS side: the Objective-C bridge in hid.m (compiled by build.rs) feeds a queue this module drains.

use super::HidEvent;

/// One queued event as hid.m writes it (`struct dc_hid_event`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawEvent {
    /// One of the `KIND_*` constants.
    kind: u8,
    /// Key / button: 1 down, 0 up; presence: 1 connected, 0 disconnected.
    down: u8,
    /// Key: the usage id; button: the button index.
    code: u16,
    dx: f32,
    dy: f32,
}

const KIND_KEY: u8 = 1;
const KIND_MOVE: u8 = 2;
const KIND_BUTTON: u8 = 3;
const KIND_SCROLL: u8 = 4;
const KIND_KEYBOARD: u8 = 5;
const KIND_MOUSE: u8 = 6;

unsafe extern "C" {
    fn dc_hid_start();
    fn dc_hid_drain(out: *mut RawEvent, cap: usize) -> usize;
    fn dc_hid_set_pointer_locked(on: bool);
    fn dc_hid_set_frame_rate(max_hz: std::os::raw::c_int);
    fn dc_hid_display_max_hz() -> std::os::raw::c_int;
    fn dc_hid_link_ticks() -> u64;
    fn dc_hid_pointer_is_locked() -> bool;
    fn dc_hid_relock();
    fn dc_hid_hide_pointer(hide: bool);
}

pub struct Inner;

impl Inner {
    pub fn start() -> Inner {
        // SAFETY: no arguments; hid.m hops to the main queue itself and ignores a second start.
        unsafe { dc_hid_start() };
        Inner
    }

    pub fn drain(&mut self) -> Vec<HidEvent> {
        let mut buf = [RawEvent::default(); 256];
        let mut events = Vec::new();
        loop {
            // SAFETY: `buf` holds `buf.len()` writable events; hid.m writes at most `cap` of them.
            let n = unsafe { dc_hid_drain(buf.as_mut_ptr(), buf.len()) }.min(buf.len());
            events.extend(buf[..n].iter().filter_map(convert));
            if n < buf.len() {
                return events;
            }
        }
    }
}

fn convert(e: &RawEvent) -> Option<HidEvent> {
    let down = e.down != 0;
    Some(match e.kind {
        KIND_KEY => HidEvent::Key { usage: e.code, down },
        KIND_MOVE => HidEvent::MouseMove { dx: e.dx, dy: e.dy },
        KIND_BUTTON => HidEvent::MouseButton {
            button: e.code.min(255) as u8,
            down,
        },
        KIND_SCROLL => HidEvent::Scroll { dx: e.dx, dy: e.dy },
        KIND_KEYBOARD => HidEvent::Keyboard(down),
        KIND_MOUSE => HidEvent::Mouse(down),
        _ => return None,
    })
}

pub fn set_pointer_locked(on: bool) {
    // SAFETY: plain flag; hid.m applies it on the main queue.
    unsafe { dc_hid_set_pointer_locked(on) }
}

/// The display rate the app asks the system for (a CADisplayLink's preferred range; 0 removes the link).
pub fn set_frame_rate(max_hz: u32) {
    unsafe { dc_hid_set_frame_rate(max_hz.min(240) as std::os::raw::c_int) }
}

/// The main screen's maximum frames per second (`UIScreen.maximumFramesPerSecond`).
pub fn display_max_hz() -> u32 {
    unsafe { dc_hid_display_max_hz() }.max(0) as u32
}

/// How many times the frame-rate display link has fired (a check of the rate the system granted).
pub fn link_ticks() -> u64 {
    unsafe { dc_hid_link_ticks() }
}

pub fn pointer_is_locked() -> bool {
    unsafe { dc_hid_pointer_is_locked() }
}

pub fn relock() {
    unsafe { dc_hid_relock() }
}

pub fn hide_pointer(hide: bool) {
    unsafe { dc_hid_hide_pointer(hide) }
}

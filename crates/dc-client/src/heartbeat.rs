//! `--heartbeat FILE` (gate T1): a thread `dc-heartbeat` appends one JSON line per second with the resource ledger
//! of the process and the frame thread's progress:
//! `{"t_ms", "unix_ms", "pid", "frame", "pumps", "phase", "stalled_ms", "stalls", "handles", "threads",
//! "private_bytes", "voices_looping", "corpses", "corpses_placeholder", "crumb_threads", "crumb_no_slot"}`.
//! The external watchdog (`gate/hang.py`) reads it: a file that stops growing means the process (or this
//! thread) stopped; a frame and pump count that stop while the phase is watched mean the frame thread stopped. The
//! gate's ledger (validation plan §2.2 item 6) reads the counters.
//!
//! `voices_looping` comes from the event stream ([`crate::events::voices_looping`]; `null` without `--events`), so
//! this thread takes the stream's lock once per second: a frame thread stuck while holding it stops the heartbeat,
//! which the external watchdog reports as well.
// ORIGINAL: our own diagnostics tooling (gate T1); no 2009 behaviour
// DC-DEFAULT: CLI-GATE-T1-2 the heartbeat ledger file

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One line per this many ms.
pub const PERIOD_MS: u64 = 1000;

static ON: AtomicBool = AtomicBool::new(false);
static CORPSES: AtomicU32 = AtomicU32::new(0);
static PLACEHOLDERS: AtomicU32 = AtomicU32::new(0);

/// Starts the heartbeat thread writing to `path` (created / truncated). Once per process.
pub fn start(path: &Path) -> std::io::Result<()> {
    if ON.swap(true, Relaxed) {
        return Ok(());
    }
    if let Some(d) = path.parent() {
        if !d.as_os_str().is_empty() {
            std::fs::create_dir_all(d)?;
        }
    }
    let mut f = std::fs::File::create(path)?;
    let t0 = Instant::now();
    std::thread::Builder::new()
        .name("dc-heartbeat".into())
        .spawn(move || loop {
            let line = line(t0.elapsed().as_millis() as u64);
            dc_hangdump::crumb!("heartbeat: write line");
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
            std::thread::sleep(Duration::from_millis(PERIOD_MS));
        })?;
    Ok(())
}

/// Whether a heartbeat is written.
pub fn on() -> bool {
    ON.load(Relaxed)
}

/// The corpses of a drawn frame (`drawn`: the objects drawn with a model this frame or loading one); a corpse with
/// no model or not drawn counts as a placeholder (the gate's `corpses_placeholder`).
pub fn corpses(fs: &crate::world::FrameState, drawn: &[u16]) {
    if !on() {
        return;
    }
    let all = &fs.anims.corpses;
    let placeholders = all
        .iter()
        .filter(|c| {
            c.look.as_ref().and_then(|l| l.models.first()).is_none() || !drawn.contains(&c.obj)
        })
        .count();
    CORPSES.store(all.len() as u32, Relaxed);
    PLACEHOLDERS.store(placeholders as u32, Relaxed);
}

fn line(t_ms: u64) -> String {
    let c = dc_hangdump::process::counters();
    let crumbs = dc_hangdump::crumb::snapshot();
    let voices = crate::events::on().then(|| {
        dc_hangdump::crumb_wait!("heartbeat: voices_looping", crate::events::voices_looping())
    });
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let v = serde_json::json!({
        "t_ms": t_ms,
        "unix_ms": unix_ms,
        "pid": std::process::id(),
        "frame": crate::watchdog::frames(),
        "pumps": crate::watchdog::pumps(),
        "phase": crate::watchdog::current_phase().name(),
        "armed": crate::watchdog::armed(),
        "stalled_ms": crate::watchdog::stalled_ms(),
        "stalls": crate::watchdog::stalls(),
        "handles": c.handles,
        "threads": c.threads,
        "private_bytes": c.private_bytes,
        "voices_looping": voices,
        "corpses": CORPSES.load(Relaxed),
        "corpses_placeholder": PLACEHOLDERS.load(Relaxed),
        "crumb_threads": crumbs.len(),
        "crumb_no_slot": dc_hangdump::crumb::no_slot(),
    });
    v.to_string() + "\n"
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_line_is_one_json_object_with_the_ledger_fields() {
        let l = super::line(1234);
        assert!(l.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(l.trim()).unwrap();
        for k in [
            "t_ms",
            "pid",
            "frame",
            "pumps",
            "phase",
            "handles",
            "threads",
            "private_bytes",
            "voices_looping",
            "corpses",
            "corpses_placeholder",
        ] {
            assert!(v.get(k).is_some(), "{k} in {l}");
        }
        assert_eq!(v["t_ms"], 1234);
        if cfg!(windows) {
            assert!(v["threads"].as_u64().unwrap() >= 1);
            assert!(v["private_bytes"].as_u64().unwrap() > 0);
        }
    }
}

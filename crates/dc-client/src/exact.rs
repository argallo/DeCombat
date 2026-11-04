//! The exact lane of the render determinism contract (gate G0 of `docs/RENDER_PERF_GATES.md`; the contract is
//! `docs/RENDER_DETERMINISM_CONTRACT.md`): the run manifest that names everything an exact-lane frame hash depends
//! on, and the lane key that says which golden hashes a run may be compared with.
//!
//! A run holds a [`Recording`] for its whole length (the read journal of `dc_rom::journal` and dc-render's GPU-object
//! journal are process-global, so exact-lane runs in one process are serialised), renders its scripted scene with
//! [`crate::HeadlessOptions::hash_frames`] (or hashes its own read-backs with [`dc_render::texel_hash`]), and ends
//! with [`Recording::manifest`]:
//!
//! - `inputs.rom`: SHA-256 of every byte range read from the user's client folder, keyed by a digest of the path
//!   relative to that folder (names never leave the process, CLEANROOM.md section 4);
//! - `inputs.shaders` / `pipelines` / `samplers`: digests of every WGSL source and of the pixel-relevant settings of
//!   every pipeline and sampler created;
//! - `render_settings`: what the scene states (resolution, gamma, fog, field of view, MSAA, present mode, clocks);
//! - `software`: crate version, git commit (dirty flag), rustc, target triple, build profile, wgpu version;
//! - `adapter`: the wgpu adapter info (name, vendor, device, type, driver, driver info, backend); `os`.
//!
//! Exact-lane hashes are comparable only between runs with the same [`lane_key`] (adapter class, driver, target,
//! wgpu) and the same `inputs_digest`.
// Provenance: ORIGINAL (our own engineering contract; no 2009 behaviour claims)

use serde_json::{json, Value};
use std::path::Path;
use std::sync::MutexGuard;

/// Name and version of the contract the manifest follows.
pub const CONTRACT: &str = "dc-render-determinism/1";

/// The texel layout every exact-lane hash covers (with [`dc_render::TEXEL_CONTRACT`]).
pub const TEXEL_LAYOUT: &str = "RGBA8 unorm, rows top to bottom, no row padding, alpha forced to 255, client \
                                gamma ramp applied; SHA-256 over 'dc-texels/1 rgba8 <w>x<h>\\n' + texels";

/// The journals of one exact-lane run; dropping it stops them.
pub struct Recording {
    _lock: MutexGuard<'static, ()>,
}

/// Starts recording the run's inputs (waits for any other exact-lane run of this process to end).
pub fn record() -> Recording {
    let lock = dc_rom::journal::lock();
    dc_rom::journal::enable(true);
    dc_render::journal_enable(true);
    Recording { _lock: lock }
}

impl Drop for Recording {
    fn drop(&mut self) {
        dc_rom::journal::enable(false);
        dc_render::journal_enable(false);
    }
}

fn sha_hex(bytes: &[u8]) -> String {
    dc_render::hex(&dc_rom::sha256(bytes))
}

/// The journal key of a ROM read, relative to the ROM folder (lower case, `/` separators): the same file read from
/// two users' folders gets the same key.
fn relative_key(key: &str, rom: Option<&Path>) -> String {
    let k = key.replace('\\', "/").to_ascii_lowercase();
    let Some(rom) = rom else {
        return k;
    };
    let r = rom
        .display()
        .to_string()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let r = r.trim_end_matches('/');
    match k.find(r) {
        Some(i) => format!("{}{}", &k[..i], &k[i + r.len()..]),
        None => k,
    }
}

impl Recording {
    /// The manifest of the run so far. `rom`: the client folder the run read (its prefix is cut from the read keys);
    /// `settings`: the scene's render settings ([`render_settings`] plus anything scene-specific).
    pub fn manifest(
        &self,
        adapter: Option<&wgpu::AdapterInfo>,
        rom: Option<&Path>,
        settings: Value,
    ) -> Value {
        build_manifest(
            &dc_rom::journal::snapshot(),
            &dc_render::journal_snapshot(),
            adapter,
            rom,
            settings,
        )
    }
}

/// The manifest of these journal contents (see [`Recording::manifest`]).
fn build_manifest(
    reads: &[(String, [u8; 32])],
    gpu: &dc_render::Journal,
    adapter: Option<&wgpu::AdapterInfo>,
    rom: Option<&Path>,
    settings: Value,
) -> Value {
    let mut rom_lines = String::new();
    let mut entries = Vec::with_capacity(reads.len());
    for (key, digest) in reads {
        let k = sha_hex(relative_key(key, rom).as_bytes());
        let d = dc_render::hex(digest);
        rom_lines.push_str(&format!("{k} {d}\n"));
        entries.push(json!({ "key": &k[..16], "sha256": d }));
    }
    let list = |set: &std::collections::BTreeSet<(String, [u8; 32])>| -> Value {
        Value::Array(
            set.iter()
                .map(|(l, d)| json!({ "label": l, "sha256": dc_render::hex(d) }))
                .collect(),
        )
    };
    let shaders = list(&gpu.shaders);
    let pipelines = list(&gpu.pipelines);
    let samplers = list(&gpu.samplers);
    let rom_digest = sha_hex(rom_lines.as_bytes());
    let code_digest = sha_hex(
        json!({
            "shaders": shaders,
            "pipelines": pipelines,
            "samplers": samplers,
            "render_settings": settings,
        })
        .to_string()
        .as_bytes(),
    );
    let inputs_digest = sha_hex(format!("{rom_digest} {code_digest}").as_bytes());
    json!({
        "contract": CONTRACT,
        "texel_contract": dc_render::TEXEL_CONTRACT,
        "texel_layout": TEXEL_LAYOUT,
        "lane_key": adapter.map(lane_key),
        "inputs_digest": inputs_digest,
        "inputs": {
            "rom": { "reads": entries.len(), "digest": rom_digest, "entries": entries },
            "code_digest": code_digest,
            "shaders": shaders,
            "pipelines": pipelines,
            "samplers": samplers,
        },
        "render_settings": settings,
        "software": software(),
        "adapter": adapter.map(adapter_json),
        "os": os_version(),
    })
}

/// The render settings every scene states: target size and format, MSAA (none: RENDER_2009 section 2), present mode
/// (offscreen: none), the client gamma, fog (the world settings object's, on), the camera's fields of view, the
/// world renderer's settings and the frame clock.
pub fn render_settings(
    width: u32,
    height: u32,
    camera: &dc_render::Camera,
    gamma: f32,
    settings: &dc_render::RenderSettings,
    frame_us: u64,
    tick_hz: u32,
) -> Value {
    json!({
        "width": width,
        "height": height,
        "format": format!("{:?}", dc_render::OFFSCREEN_FORMAT),
        "msaa": 1,
        "present_mode": "offscreen (none)",
        "gamma": gamma,
        "fog": "world settings object (SceneOptions::fog on)",
        "fov_x_rad": camera.fov_x,
        "fov_y_rad": camera.fov_y,
        "renderer": format!("{settings:?}"),
        "frame_us": frame_us,
        "tick_hz": tick_hz,
    })
}

/// The key of the exact lane a run belongs to: backend, vendor and device id, driver and driver info, target
/// triple, build profile and wgpu version. Hashes are compared only within one key
/// (docs/RENDER_DETERMINISM_CONTRACT.md): the CPU side of a frame (poses, particles, matrices) is f32 arithmetic
/// whose results may differ between targets and optimisation levels (libm calls, constant folding), the GPU side
/// between adapters and drivers.
pub fn lane_key(info: &wgpu::AdapterInfo) -> String {
    format!(
        "{:?}|{:#06x}:{:#06x}|{}|{}|{}|{}|wgpu {}",
        info.backend,
        info.vendor,
        info.device,
        info.driver,
        info.driver_info,
        env!("DC_TARGET_TRIPLE"),
        env!("DC_BUILD_PROFILE"),
        env!("DC_WGPU_VERSION")
    )
}

/// The adapter facts of the manifest.
pub fn adapter_json(info: &wgpu::AdapterInfo) -> Value {
    json!({
        "name": info.name,
        "vendor": format!("{:#06x}", info.vendor),
        "device": format!("{:#06x}", info.device),
        "device_type": format!("{:?}", info.device_type),
        "driver": info.driver,
        "driver_info": info.driver_info,
        "backend": format!("{:?}", info.backend),
    })
}

/// Crate version, git commit (`DC_GIT_COMMIT` overrides; `-dirty` when tracked files differ), compiler, target,
/// build profile and wgpu version.
pub fn software() -> Value {
    json!({
        "dc_client": env!("CARGO_PKG_VERSION"),
        "git_commit": git_commit(),
        "rustc": env!("DC_RUSTC_VERSION"),
        "target": env!("DC_TARGET_TRIPLE"),
        "profile": env!("DC_BUILD_PROFILE"),
        "wgpu": env!("DC_WGPU_VERSION"),
    })
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn git_commit() -> String {
    if let Ok(c) = std::env::var("DC_GIT_COMMIT") {
        return c;
    }
    let dir = env!("CARGO_MANIFEST_DIR");
    let Some(head) = run("git", &["-C", dir, "rev-parse", "HEAD"]) else {
        return "unknown".into();
    };
    let dirty = run(
        "git",
        &["-C", dir, "status", "--porcelain", "--untracked-files=no"],
    )
    .is_some_and(|s| !s.is_empty());
    if dirty {
        format!("{head}-dirty")
    } else {
        head
    }
}

/// The operating system and its version as the system reports it.
pub fn os_version() -> String {
    let v = if cfg!(windows) {
        run("cmd", &["/C", "ver"])
    } else {
        run("uname", &["-srm"])
    };
    format!(
        "{} {}",
        std::env::consts::OS,
        v.unwrap_or_else(|| "unknown".into())
    )
}

/// One exact-lane frame hash as JSON (`frame`, `width`, `height`, `sha256`).
pub fn frame_json(h: &crate::headless::FrameHash) -> Value {
    json!({
        "frame": h.frame,
        "width": h.width,
        "height": h.height,
        "sha256": dc_render::hex(&h.sha256),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rom_keys_are_relative_and_case_folded() {
        let rom = Path::new("C:\\Games\\CA\\Client");
        assert_eq!(
            relative_key("rez:C:\\Games\\CA\\Client\\Game\\X.REZ@10+4", Some(rom)),
            "rez:/game/x.rez@10+4"
        );
        assert_eq!(
            relative_key(
                "rez:D:/Other/Client/Game/X.rez@10+4",
                Some(Path::new("d:/other/client/"))
            ),
            "rez:/game/x.rez@10+4"
        );
        assert_eq!(relative_key("file:/a/B", None), "file:/a/b");
    }

    #[test]
    fn manifest_names_inputs_by_digest() {
        let reads = vec![(
            "rez:/rom/Game/A.rez@0+3".to_string(),
            dc_rom::sha256(b"abc"),
        )];
        let gpu = dc_render::Journal::default();
        let rom = Some(Path::new("/rom"));
        let m = build_manifest(&reads, &gpu, None, rom, json!({ "width": 4 }));
        assert_eq!(m["contract"], CONTRACT);
        assert_eq!(m["inputs"]["rom"]["reads"], 1);
        let e = &m["inputs"]["rom"]["entries"][0];
        assert_eq!(e["sha256"], sha_hex(b"abc"));
        assert_eq!(
            e["key"].as_str().unwrap(),
            &sha_hex(b"rez:/game/a.rez@0+3")[..16]
        );
        assert!(!m.to_string().contains("A.rez"), "no names in the manifest");
        let again = build_manifest(&reads, &gpu, None, rom, json!({ "width": 4 }));
        assert_eq!(m["inputs_digest"], again["inputs_digest"]);
        let other = build_manifest(&reads, &gpu, None, rom, json!({ "width": 5 }));
        assert_ne!(
            m["inputs_digest"], other["inputs_digest"],
            "settings are inputs"
        );
        let mut gpu2 = gpu.clone();
        gpu2.shaders.insert(("s".into(), [1; 32]));
        let shader = build_manifest(&reads, &gpu2, None, rom, json!({ "width": 4 }));
        assert_ne!(
            m["inputs_digest"], shader["inputs_digest"],
            "shaders are inputs"
        );
        assert_eq!(
            m["inputs"]["rom"]["digest"],
            shader["inputs"]["rom"]["digest"]
        );
    }
}

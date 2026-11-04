//! A lost GPU device (driver reset, TDR) without a hang (fix-hang).
//!
//! Why: wgpu-core 25.0.2 calls `Device::lose` from `handle_hal_error` while `Queue::submit` (and `Device::maintain`,
//! the poll path) still hold the device's snatch lock for reading, and `lose` ends in `release_gpu_resources`, which
//! takes the same lock for writing: the calling thread waits for itself forever. The user's window hangs of
//! 2026-10-26 (and the soak reproduction, `reports/fix-hang.md`) are exactly this: an NVIDIA
//! driver reset (System log `nvlddmkm` 153) made `vkQueueSubmit` fail, and the frame thread never returned.
//!
//! `lose` runs the device's lost callback BEFORE the self-deadlocking step, on the thread that met the error. The
//! callback installed by [`watch`] marks the loss and unwinds that thread with a [`DeviceLostUnwind`] panic, which
//! leaves wgpu-core's call by its normal unwinding (its guards are dropped, its locks do not poison). [`guard`]
//! catches it around the window's frame. The lost device's GPU objects are never dropped (they are put away through
//! [`retire`]): dropping its queue waits on the lost fence and runs `lose` once more.
//!
//! gpu-recover: the window then REBUILDS the GPU side inside the match (`window::App::recover`): a new instance,
//! surface, adapter, device and queue, every renderer, the world's render scene from the ROM with the server's
//! state applied again, and the model / effect uploads from the CPU caches. The simulation and the network thread go
//! on the whole time; the window prints `GPU device recovered in N ms` and draws again. Only when no device comes
//! back within [`RECOVER_GIVE_UP_MS`] does the window leave the match as before (`App::on_gpu_reset`, exit reason
//! `gpu device lost`, exit code 0). `DC_GPU_RECOVER=0` restores the old behaviour (leave at once).
//!
//! If the unwinding itself never returns, the frame watchdog ([`crate::watchdog`]) sees the stall, writes a stall dir
//! (`threads.txt` with breadcrumbs and stacks, `stall.json` kind `gpu_lost_no_recovery`, no minidump), flushes the
//! profiler's flight recorder and ends the process with exit code 3, so a lost device can no longer leave a window
//! that does not answer. Exit 3 means the frame did NOT recover (the hang gate fails the run).
//!
//! Test knobs: `DC_GPU_LOSS_TEST_AT=<s>` provokes a REAL reset of our device once ([`provoke`]);
//! `DC_FAULT=gpu_lost` + `DC_FAULT_AT=<s>` (only with `DC_AGENT=1` or a debug build) runs the loss path without a
//! driver reset ([`simulate_loss`]).
// ORIGINAL: our own error handling around the wgpu crate (fix-hang); no 2009 behaviour
// DC-DEFAULT: CLI-460 a lost GPU device never hangs; amended by gpu-recover: the window rebuilds its device inside
// the match, and leaves the match (menu: the waiting room; direct join: the run ends) only when that fails

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Once;

static LOST: AtomicBool = AtomicBool::new(false);
static LOSSES: AtomicU64 = AtomicU64::new(0);
static RECOVERIES: AtomicU64 = AtomicU64::new(0);
static LEAVES: AtomicU64 = AtomicU64::new(0);
static RETIRED: AtomicU64 = AtomicU64::new(0);
/// `DC_GPU_LOSS_TEST_AT=<seconds>`: the window provokes a real device loss once at that window time ([`provoke`]).
static TEST_AT_MS: AtomicU64 = AtomicU64::new(u64::MAX);
/// `DC_FAULT=gpu_lost`: the window time (ms) of the simulated loss ([`simulate_loss`]).
static FAULT_AT_MS: AtomicU64 = AtomicU64::new(u64::MAX);
static KNOBS: Once = Once::new();

/// A device that does not come back within this long ends the recovery: the window leaves the match.
// DC-DEFAULT: CLI-460 (amended by gpu-recover) 12 s, inside the watchdog's 15 s recovery budget
pub const RECOVER_GIVE_UP_MS: u64 = 12_000;
/// The wait between two recovery attempts (the driver's reset comes as a pair of events about 2 s apart).
pub const RECOVER_RETRY_MS: u64 = 250;

/// The error of [`guard`]: the GPU device was lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuReset;

impl std::fmt::Display for GpuReset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the GPU device was lost")
    }
}

impl std::error::Error for GpuReset {}

/// The panic payload that carries a device loss out of wgpu-core (never shown: the panic hook skips it).
#[derive(Debug)]
pub struct DeviceLostUnwind;

fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().is::<DeviceLostUnwind>() {
                return;
            }
            prev(info);
        }));
    });
}

/// What the lost callback does: Unknown (the driver) marks the loss and unwinds; Destroyed (our own drop or
/// destroy) is ignored.
fn on_lost(reason: wgpu::DeviceLostReason, message: &str) {
    if reason == wgpu::DeviceLostReason::Destroyed {
        return;
    }
    LOST.store(true, Ordering::SeqCst);
    LOSSES.fetch_add(1, Ordering::Relaxed);
    eprintln!("dc-client: the GPU device was lost ({message})");
    std::panic::panic_any(DeviceLostUnwind);
}

/// TEST TOOL (`DC_FAULT=gpu_lost`): the loss path without a driver reset: marks the loss and unwinds the calling
/// thread exactly as the lost callback does. The device stays usable; the window rebuilds it all the same.
pub fn simulate_loss() {
    install_hook();
    LOST.store(true, Ordering::SeqCst);
    LOSSES.fetch_add(1, Ordering::Relaxed);
    eprintln!("dc-client: the GPU device was lost (DC_FAULT=gpu_lost: simulated)");
    std::panic::panic_any(DeviceLostUnwind);
}

/// Reads the test knobs once per process (a rebuilt device must not provoke its own loss again).
fn read_knobs() {
    KNOBS.call_once(|| {
        let secs = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
        };
        if let Some(at) = secs("DC_GPU_LOSS_TEST_AT") {
            TEST_AT_MS.store((at * 1000.0) as u64, Ordering::Relaxed);
        }
        if crate::watchdog::fault_name() == Some("gpu_lost") {
            let at = secs("DC_FAULT_AT").unwrap_or(crate::watchdog::FAULT_AT_DEFAULT_S);
            FAULT_AT_MS.store((at * 1000.0) as u64, Ordering::Relaxed);
        }
    });
}

/// Watches `device`: a driver-side loss unwinds the thread that met it (see the module doc). Clears an earlier
/// loss (a new device).
pub fn watch(device: &wgpu::Device) {
    watch_keep_lost(device);
    LOST.store(false, Ordering::SeqCst);
}

/// Watches `device` like [`watch`] but keeps an earlier loss marked: the recovery watches its new device from the
/// first call on and clears the loss ([`recovered`]) only when everything was rebuilt.
pub fn watch_keep_lost(device: &wgpu::Device) {
    install_hook();
    read_knobs();
    device.set_device_lost_callback(|reason, message| on_lost(reason, &message));
}

/// The recovery rebuilt everything on a new device: the loss is over.
pub fn recovered() {
    RECOVERIES.fetch_add(1, Ordering::Relaxed);
    LOST.store(false, Ordering::SeqCst);
}

/// Recoveries inside the match so far.
pub fn recoveries() -> u64 {
    RECOVERIES.load(Ordering::Relaxed)
}

/// The window left the match after a loss (the fix-hang rule, now the recovery's fallback).
pub fn left_match() {
    LEAVES.fetch_add(1, Ordering::Relaxed);
}

/// Times a loss made the window leave the match.
pub fn leaves() -> u64 {
    LEAVES.load(Ordering::Relaxed)
}

/// The recovery inside the match is on (`DC_GPU_RECOVER=0`: a loss leaves the match at once, the fix-hang rule).
pub fn recover_enabled() -> bool {
    std::env::var("DC_GPU_RECOVER")
        .ok()
        .as_deref()
        .map(str::trim)
        != Some("0")
}

/// Puts away a GPU object of a lost (or replaced) device without dropping it (see the module doc). Counted.
pub fn retire<T>(t: T) {
    RETIRED.fetch_add(1, Ordering::Relaxed);
    std::mem::forget(t);
}

/// GPU objects retired so far.
pub fn retired() -> u64 {
    RETIRED.load(Ordering::Relaxed)
}

/// Whether the `DC_FAULT=gpu_lost` time has come (true once).
pub fn fault_due(now_ms: u64) -> bool {
    now_ms >= FAULT_AT_MS.load(Ordering::Relaxed)
        && FAULT_AT_MS.swap(u64::MAX, Ordering::Relaxed) != u64::MAX
}

/// Whether the window's `DC_GPU_LOSS_TEST_AT` time has come (true once).
pub fn test_due(now_ms: u64) -> bool {
    now_ms >= TEST_AT_MS.load(Ordering::Relaxed)
        && TEST_AT_MS.swap(u64::MAX, Ordering::Relaxed) != u64::MAX
}

/// A device watched by [`watch`] was lost since.
pub fn is_lost() -> bool {
    LOST.load(Ordering::SeqCst)
}

/// Device losses seen by this process.
pub fn losses() -> u64 {
    LOSSES.load(Ordering::Relaxed)
}

/// Runs `f`; `Err(GpuReset)` when it was unwound by a device loss (or panicked after one: every later wgpu call on the lost
/// device reports an error that wgpu's default handler turns into a panic). Any other panic goes on.
pub fn guard<R>(f: impl FnOnce() -> R) -> Result<R, GpuReset> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => Ok(r),
        Err(p) => {
            if p.is::<DeviceLostUnwind>() || is_lost() {
                LOST.store(true, Ordering::SeqCst);
                Err(GpuReset)
            } else {
                resume_unwind(p)
            }
        }
    }
}

/// TEST TOOL (fix-hang): submits one render pass whose fragments loop about 4 billion times each, far beyond the
/// Windows GPU time-out (2 s), so the driver resets the GPU and the device is lost. Only the manual test
/// `tests/gpu_loss.rs` and the window's `DC_GPU_LOSS_TEST_AT=<seconds>` switch call it; it resets the machine's
/// display driver.
pub fn provoke(device: &wgpu::Device, queue: &wgpu::Queue) {
    use wgpu::util::DeviceExt;
    const SPIN: &str = r#"
    struct U { n: u32, _p0: u32, _p1: u32, _p2: u32 }
    @group(0) @binding(0) var<uniform> u: U;
    @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
        let x = f32((i << 1u) & 2u);
        let y = f32(i & 2u);
        return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    }
    @fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
        var a = p.x * 0.001;
        for (var k = 0u; k < u.n; k = k + 1u) {
            a = fract(a * 1.0000001 + 0.37);
        }
        return vec4<f32>(a, 0.0, 0.0, 1.0);
    }
    "#;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spin"),
        source: wgpu::ShaderSource::Wgsl(SPIN.into()),
    });
    let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("spin n"),
        contents: &[0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: ubuf.as_entire_binding(),
        }],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("spin"),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("spin target"),
        size: wgpu::Extent3d {
            width: 256,
            height: 256,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("spin"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    queue.submit([enc.finish()]);
    std::mem::forget((module, ubuf, layout, group, pl, pipeline, target, view));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loss_unwinds_to_the_guard_and_a_destroy_does_not() {
        install_hook();
        LOST.store(false, Ordering::SeqCst);
        // our own destroy: the callback returns
        on_lost(wgpu::DeviceLostReason::Destroyed, "");
        assert!(!is_lost());
        // a driver loss inside a call: the guard gets it, the code after the loss never runs
        let mut after = false;
        let r = guard(|| {
            on_lost(wgpu::DeviceLostReason::Unknown, "test");
            after = true;
        });
        assert_eq!(r, Err(GpuReset));
        assert!(!after);
        assert!(is_lost());
        // any panic after a loss counts as the loss (wgpu's errors on the lost device)
        assert_eq!(
            guard(|| panic!("validation error on a lost device")),
            Err::<(), _>(GpuReset)
        );
        LOST.store(false, Ordering::SeqCst);
        // without a loss a panic goes on
        let other = catch_unwind(|| guard(|| panic!("a bug")));
        assert!(other.is_err());
        assert_eq!(guard(|| 7), Ok(7));
    }
}

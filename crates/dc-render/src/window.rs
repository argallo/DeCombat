//! Window mode of the viewer (feature `window`): one borderless, non-activating window on the primary monitor
//! that re-draws the prepared map until a deadline, Escape or close. Never exclusive full screen and never a
//! display-mode change (RENDER_2009.md §2.1 DeCombat note: "full screen" is a borderless window).

use crate::bucket::RenderSettings;
use crate::gpu::{FrameTarget, GpuContext, RenderError};
use crate::msaa::MsaaTargets;
use crate::viewer::{PreparedView, RomViewer, ViewError, ViewReport, ViewRequest};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Window-mode options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowOptions {
    /// Close automatically after this long (the viewer is a check tool, not a game).
    pub seconds: f32,
}

struct Live {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    /// CLI-1593: the multisampled targets while the settings ask for MSAA (`None` otherwise).
    msaa: Option<MsaaTargets>,
    viewer: RomViewer,
    view: PreparedView,
    frames: u64,
    started: Instant,
}

struct App {
    rom: PathBuf,
    req: ViewRequest,
    settings: RenderSettings,
    opts: WindowOptions,
    live: Option<Live>,
    error: Option<ViewError>,
    report: Option<ViewReport>,
}

fn depth_view(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("window depth"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

impl App {
    fn start(&mut self, el: &ActiveEventLoop) -> Result<Live, ViewError> {
        let (w, h) = (self.req.width, self.req.height);
        let mut attrs = Window::default_attributes()
            .with_title("worldview")
            .with_decorations(false)
            .with_resizable(false)
            .with_active(false)
            .with_inner_size(PhysicalSize::new(w, h));
        if let Some(m) = el.primary_monitor() {
            let p = m.position();
            let s = m.size();
            let x = p.x + (s.width.saturating_sub(w) / 2) as i32;
            let y = p.y + (s.height.saturating_sub(h) / 2) as i32;
            attrs = attrs.with_position(PhysicalPosition::new(x, y));
        }
        let window = Arc::new(
            el.create_window(attrs)
                .map_err(|e| ViewError::Gpu(RenderError::Device(e.to_string())))?,
        );
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| ViewError::Gpu(RenderError::Device(e.to_string())))?;
        let ctx = GpuContext::for_surface(instance, &surface)?;
        let caps = surface.get_capabilities(&ctx.adapter);
        // 8-bit non-sRGB like the 2009 back buffer (RENDER_2009.md §2.1: Bgra8Unorm)
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Bgra8Unorm)
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| ViewError::Gpu(RenderError::Device("surface has no formats".into())))?;
        // RENDER_2009.md §2.6: the 2009 engine never waits for vertical blank; one frame in flight.
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode,
            // iPadOS / DX12: two frames in flight (crate::frame_latency_for, dx12-perf), else one
            desired_maximum_frame_latency: crate::frame_latency_for(ctx.info.backend),
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
        };
        surface.configure(&ctx.device, &config);
        let depth = depth_view(&ctx.device, w, h);
        let viewer = RomViewer::with_context(&self.rom, ctx, format, self.settings)?;
        let msaa = MsaaTargets::new(&viewer.ctx, format, (w, h), viewer.renderer.msaa());
        let view = viewer.prepare(&self.req)?;
        window.request_redraw();
        Ok(Live {
            window,
            surface,
            config,
            depth,
            msaa,
            viewer,
            view,
            frames: 0,
            started: Instant::now(),
        })
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.live.is_some() || self.error.is_some() {
            return;
        }
        match self.start(el) {
            Ok(l) => self.live = Some(l),
            Err(e) => {
                self.error = Some(e);
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape) =>
            {
                el.exit()
            }
            WindowEvent::RedrawRequested => {
                let frame = match live.surface.get_current_texture() {
                    Ok(f) => f,
                    Err(_) => {
                        live.surface
                            .configure(&live.viewer.ctx.device, &live.config);
                        return;
                    }
                };
                let target = frame.texture.create_view(&Default::default());
                let mut enc = live.viewer.ctx.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor {
                        label: Some("window frame"),
                    },
                );
                // CLI-1593: with MSAA the one 3D pass draws into the multisampled targets and resolves to the screen
                let frame_target = match &live.msaa {
                    Some(ms) => ms.target(Some(&target)),
                    None => FrameTarget::new(
                        &target,
                        &live.depth,
                        (live.config.width, live.config.height),
                    ),
                };
                let models = live.view.model_refs();
                let view_models = live.view.view_model_refs();
                live.viewer.renderer.encode_frame_with_view(
                    &live.viewer.ctx,
                    &live.view.scene,
                    &live.view.camera,
                    frame_target,
                    &live.view.frame,
                    &models,
                    &view_models,
                    &live.view.player_view,
                    &mut enc,
                );
                live.viewer.ctx.queue.submit([enc.finish()]);
                frame.present();
                live.frames += 1;
                if live.started.elapsed() >= Duration::from_secs_f32(self.opts.seconds.max(0.1)) {
                    el.exit();
                } else {
                    live.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(live) = &self.live {
            if live.started.elapsed() >= Duration::from_secs_f32(self.opts.seconds.max(0.1)) {
                el.exit();
            }
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let Some(live) = self.live.take() {
            let mut r = live.view.report.clone();
            // Window mode: wall-clock time per presented frame (present mode Immediate when offered).
            r.timings.gpu_frame_ms =
                live.started.elapsed().as_secs_f64() * 1000.0 / live.frames.max(1) as f64;
            r.adapter = format!("{} ({} frames presented)", r.adapter, live.frames);
            self.report = Some(r);
        }
    }
}

/// Opens the window, draws `req` until the deadline and returns the view report.
pub fn run_window(
    rom: &Path,
    req: &ViewRequest,
    settings: RenderSettings,
    opts: WindowOptions,
) -> Result<ViewReport, ViewError> {
    let el = EventLoop::new().map_err(|e| ViewError::Gpu(RenderError::Device(e.to_string())))?;
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        rom: rom.to_path_buf(),
        req: req.clone(),
        settings,
        opts,
        live: None,
        error: None,
        report: None,
    };
    el.run_app(&mut app)
        .map_err(|e| ViewError::Gpu(RenderError::Device(e.to_string())))?;
    if let Some(e) = app.error {
        return Err(e);
    }
    app.report.ok_or_else(|| {
        ViewError::Gpu(RenderError::Device(
            "window closed before the first frame".into(),
        ))
    })
}

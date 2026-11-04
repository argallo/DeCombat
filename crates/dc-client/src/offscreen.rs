//! Offscreen frame: dc-render's world passes, the debug boxes and the overlay pass into one colour target, then
//! read back as RGBA8. The headless runs use it so their PNGs show the same passes as the window.
//!
//! CLI-1593: with [`Offscreen::set_msaa`] above 1 the 3D passes (world, boxes, effects) draw into multisampled
//! targets of that count and the last of them resolves into the colour target, as the window does; the HUD and the
//! overlay then draw single-sampled into the resolved target. The renderer and the box pass must be built for the
//! same count.

use crate::debug_draw::{BoxInstance, DebugBoxes};
use crate::error::ClientError;
use crate::overlay::OverlayRenderer;
use dc_render::{
    Camera, FrameImage, FrameOptions, FrameTarget, GpuContext, GpuScene, ModelInstance,
    MsaaTargets, WorldRenderer, PLAYER_VIEW_DEFAULT,
};

/// Colour and depth targets of one size.
pub struct Offscreen {
    colour: wgpu::Texture,
    colour_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    pub size: (u32, u32),
    format: wgpu::TextureFormat,
    /// The 2009 gamma ramp applied to every read frame (`None`: the frame as drawn).
    ramp: Option<[u16; 256]>,
    /// Profile mode only ([`crate::prof`]): submissions still in flight, oldest first.
    inflight: std::cell::RefCell<std::collections::VecDeque<wgpu::SubmissionIndex>>,
    /// CLI-1095: every render is read back, also in profile mode (the menu's character view needs its pixels; the
    /// profiler's in-flight path hands back an empty image unless `DC_PROF_READBACK` is set).
    pub always_read: bool,
    /// Persistent read-back buffer (rows padded to the copy alignment).
    readback: wgpu::Buffer,
    padded_row: u32,
    /// The clear colour of the world pass (`None`: dc-render's default).
    clear: Option<[f64; 4]>,
    /// CLI-1593: the multisampled targets the 3D passes draw into (`None`: MSAA off, the passes draw into the
    /// colour target directly).
    msaa: Option<MsaaTargets>,
}

impl Offscreen {
    pub fn new(ctx: &GpuContext, format: wgpu::TextureFormat, w: u32, h: u32) -> Offscreen {
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let colour = dc_render::perf::texture(
            &ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("client offscreen colour"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        );
        let depth = dc_render::perf::texture(
            &ctx.device,
            &wgpu::TextureDescriptor {
                label: Some("client offscreen depth"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
        );
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = (4 * w).div_ceil(align) * align;
        let readback = dc_render::perf::buffer(
            &ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("client read-back"),
                size: padded_row as u64 * h as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
        );
        Offscreen {
            colour_view: colour.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            colour,
            size: (w, h),
            format,
            ramp: None,
            always_read: false,
            inflight: Default::default(),
            readback,
            padded_row,
            clear: None,
            msaa: None,
        }
    }

    /// The colour and depth views as a frame target (callers that record their own passes into this target).
    pub fn target(&self) -> FrameTarget<'_> {
        FrameTarget::new(&self.colour_view, &self.depth_view, self.size)
    }

    /// CLI-1593: draws the 3D passes of every later frame with `count` samples per pixel (1: off; the renderer and
    /// the box pass handed to the renders must be built for the same count, [`dc_render::RenderSettings::msaa`]).
    pub fn set_msaa(&mut self, ctx: &GpuContext, count: u32) {
        MsaaTargets::prepare(&mut self.msaa, ctx, self.format, self.size, count);
    }

    /// Samples per pixel of the 3D passes (1: off).
    pub fn msaa(&self) -> u32 {
        self.msaa.as_ref().map_or(1, MsaaTargets::count)
    }

    /// The target of one 3D pass: the multisampled textures with MSAA on (resolving into the colour target when
    /// `resolves`, the frame's last 3D pass), else the colour target itself.
    fn pass_target(&self, resolves: bool) -> FrameTarget<'_> {
        match &self.msaa {
            Some(ms) => ms.target(resolves.then_some(&self.colour_view)),
            None => self.target(),
        }
    }

    /// Clears every later frame to `c` (the menu's character view: a transparent colour it keys out).
    pub fn clear_to(&mut self, c: Option<[f64; 4]>) {
        self.clear = c;
    }

    /// Reads every later frame through the 2009 gamma ramp of `gamma` (1: unchanged), as the window shows it.
    // SPEC: RENDER_2009.md §2.5 (the ramp applies to the finished frame, HUD included)
    pub fn set_gamma(&mut self, gamma: f32) {
        self.ramp = ((gamma - 1.0).abs() > 1e-4).then(|| dc_render::ramp_for_gamma(gamma));
    }

    /// Draws the world from `camera`, then the overlay panel (when `overlay` has one), and reads the frame.
    pub fn render(
        &self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        scene: &GpuScene,
        camera: &Camera,
        overlay: Option<&OverlayRenderer>,
    ) -> Result<FrameImage, ClientError> {
        self.render_with(ctx, renderer, scene, camera, None, &[], overlay)
    }

    /// [`Offscreen::render`] with models (drawn in the world pass after the opaque world, RENDER_2009 §7.1) and the
    /// debug boxes drawn after the world and before the overlay.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with(
        &self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        scene: &GpuScene,
        camera: &Camera,
        boxes: Option<(&mut DebugBoxes, &[BoxInstance])>,
        models: &[&ModelInstance],
        overlay: Option<&OverlayRenderer>,
    ) -> Result<FrameImage, ClientError> {
        self.render_with_view(ctx, renderer, scene, camera, boxes, models, &[], overlay)
    }

    /// [`Offscreen::render_with`] plus eye-attached models (the own weapon's first-person model) drawn by
    /// dc-render's player-view pass after the world (RENDER_2009 §4.2 / §4.3).
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_view(
        &self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        scene: &GpuScene,
        camera: &Camera,
        boxes: Option<(&mut DebugBoxes, &[BoxInstance])>,
        models: &[&ModelInstance],
        view_models: &[&ModelInstance],
        overlay: Option<&OverlayRenderer>,
    ) -> Result<FrameImage, ClientError> {
        self.render_with_hud(
            ctx,
            renderer,
            scene,
            camera,
            boxes,
            models,
            view_models,
            None,
            overlay,
        )
    }

    /// As [`Offscreen::render_with_view`], with the HUD layer (a full-target image at (0, 0)) drawn after the world
    /// and before the debug panel.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_hud(
        &self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        scene: &GpuScene,
        camera: &Camera,
        boxes: Option<(&mut DebugBoxes, &[BoxInstance])>,
        models: &[&ModelInstance],
        view_models: &[&ModelInstance],
        hud: Option<&OverlayRenderer>,
        overlay: Option<&OverlayRenderer>,
    ) -> Result<FrameImage, ClientError> {
        self.render_with_fx(
            ctx,
            renderer,
            scene,
            camera,
            boxes,
            models,
            view_models,
            None,
            hud,
            overlay,
        )
    }

    /// [`Offscreen::render_with_hud`] plus the weapon effects ([`crate::fx::FxStage`] with the frame's clock and the
    /// first-person muzzle socket) after the box pass, before the HUD.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_fx(
        &self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        scene: &GpuScene,
        camera: &Camera,
        boxes: Option<(&mut DebugBoxes, &[BoxInstance])>,
        models: &[&ModelInstance],
        view_models: &[&ModelInstance],
        fx: Option<(&mut crate::fx::FxStage, u32, Option<glam::Mat4>)>,
        hud: Option<&OverlayRenderer>,
        overlay: Option<&OverlayRenderer>,
    ) -> Result<FrameImage, ClientError> {
        self.render_with_xray(ctx, renderer, scene, camera, boxes, models, view_models, &[], fx, hud, overlay)
    }

    /// [`Self::render_with_fx`] with the chams / skeleton players of a declared x-ray server (CLI-1624) after the world.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_xray(
        &self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        scene: &GpuScene,
        camera: &Camera,
        boxes: Option<(&mut DebugBoxes, &[BoxInstance])>,
        models: &[&ModelInstance],
        view_models: &[&ModelInstance],
        xray: &[dc_render::XrayModel<'_>],
        fx: Option<(&mut crate::fx::FxStage, u32, Option<glam::Mat4>)>,
        hud: Option<&OverlayRenderer>,
        overlay: Option<&OverlayRenderer>,
    ) -> Result<FrameImage, ClientError> {
        use crate::prof::Stage as PS;
        let mut laps = crate::prof::Laps::start();
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("client frame"),
            });
        // CLI-1593: the frame's last 3D pass resolves the multisampled target: the effect pass when it has quads,
        // else the box pass when it has boxes, else the world pass (nothing changes with MSAA off)
        let mut fx = fx;
        let fx_draws = fx
            .as_mut()
            .is_some_and(|(stage, now_ms, socket)| stage.prepare(camera, *now_ms, *socket));
        let boxes_draw = boxes.as_ref().is_some_and(|(_, list)| !list.is_empty());
        let report = renderer.encode_frame_with_view(
            ctx,
            scene,
            camera,
            self.pass_target(!fx_draws && !boxes_draw && xray.is_empty()),
            // SPEC: RENDER_2009.md §5.1 step 3 / §5.3 (sky pass before the world), §7.1 steps 7-9 (translucent and additive
            // world models); DC-DEFAULT: CLI-211 (proposed), see the staged rows (the client draws them like the viewer)
            &FrameOptions {
                clear: self.clear,
                ..FrameOptions::default()
            },
            models,
            view_models,
            &PLAYER_VIEW_DEFAULT,
            &mut enc,
        );
        // the frame profiler's command counters (render-core)
        crate::prof::count_frame(&report);
        if !xray.is_empty() {
            renderer.encode_xray(ctx, camera, self.pass_target(!fx_draws && !boxes_draw), xray, &mut enc);
        }
        if let Some((d, list)) = boxes {
            d.encode(
                ctx,
                &mut enc,
                self.pass_target(!fx_draws && boxes_draw),
                camera,
                list,
            );
        }
        if let Some((stage, now_ms, socket)) = fx {
            stage.encode(
                ctx,
                self.format,
                &mut enc,
                self.pass_target(fx_draws),
                camera,
                now_ms,
                socket,
            );
        }
        if let Some(h) = hud {
            h.encode(ctx, &mut enc, &self.colour_view, self.size, (0, 0));
        }
        if let Some(o) = overlay {
            o.encode(ctx, &mut enc, &self.colour_view, self.size, (8, 8));
        }
        // perf-g1: the frame's pass timestamps are resolved in its own encoder (no-op without the profiler's timer)
        dc_render::perf::gpu_frame_end(&mut enc);
        let cmd = enc.finish();
        laps.lap(PS::Encode);
        let idx = ctx.queue.submit([cmd]);
        dc_render::perf::gpu_frame_submitted();
        laps.lap(PS::Submit);
        if !crate::prof::on() || self.always_read {
            return self.read(ctx);
        }
        self.profile_wait(ctx, idx, laps)
    }

    /// CLI-1628: the world with `models`, then dc-render's x-ray pass of `xray` (the shop's preview of the player's
    /// x-ray style on its own character), read back always (no profiler in-flight path: the menu needs the pixels).
    pub fn render_models_xray(
        &self,
        ctx: &GpuContext,
        renderer: &WorldRenderer,
        scene: &GpuScene,
        camera: &Camera,
        models: &[&ModelInstance],
        xray: &[dc_render::XrayModel<'_>],
    ) -> Result<FrameImage, ClientError> {
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("client frame xray"),
            });
        let report = renderer.encode_frame_with_view(
            ctx,
            scene,
            camera,
            self.pass_target(xray.is_empty()),
            &FrameOptions {
                clear: self.clear,
                ..FrameOptions::default()
            },
            models,
            &[],
            &PLAYER_VIEW_DEFAULT,
            &mut enc,
        );
        crate::prof::count_frame(&report);
        if !xray.is_empty() {
            renderer.encode_xray(ctx, camera, self.pass_target(true), xray, &mut enc);
        }
        ctx.queue.submit([enc.finish()]);
        self.read(ctx)
    }

    /// Profile mode: keeps the configured number of frames in flight (0 = wait for this one), then reads the
    /// frame back only when asked (an empty image otherwise).
    fn profile_wait(
        &self,
        ctx: &GpuContext,
        idx: wgpu::SubmissionIndex,
        mut laps: crate::prof::Laps,
    ) -> Result<FrameImage, ClientError> {
        use crate::prof::Stage as PS;
        let cfg = crate::prof::config();
        {
            let mut q = self.inflight.borrow_mut();
            q.push_back(idx);
            while q.len() > cfg.inflight {
                if let Some(old) = q.pop_front() {
                    let _ = ctx.device.poll(wgpu::PollType::WaitForSubmissionIndex(old));
                }
            }
            let _ = ctx.device.poll(wgpu::PollType::Poll);
        }
        laps.lap(PS::GpuWait);
        if cfg.readback {
            let img = self.read(ctx);
            laps.lap(PS::Readback);
            return img;
        }
        Ok(FrameImage {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        })
    }

    fn read(&self, ctx: &GpuContext) -> Result<FrameImage, ClientError> {
        let (width, height) = self.size;
        let padded = self.padded_row;
        let buffer = &self.readback;
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("client copy"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.colour,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit([enc.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = ctx.device.poll(wgpu::PollType::Wait);
        rx.recv()
            .map_err(|e| ClientError::Gpu(e.to_string()))?
            .map_err(|e| ClientError::Gpu(e.to_string()))?;
        let data = slice.get_mapped_range();
        let bgra = matches!(
            self.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut rgba = dc_render::unpack_rows(&data, width, height, padded, bgra);
        drop(data);
        buffer.unmap();
        if let Some(r) = &self.ramp {
            dc_render::apply_ramp(&mut rgba, r);
        }
        Ok(FrameImage {
            width,
            height,
            rgba,
        })
    }
}

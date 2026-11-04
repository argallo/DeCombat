//! The editor (MAPKIT_PLAN.md P3, D7 as amended by the review, 5.2 `gui`): an eframe / egui window over the same
//! library the CLI uses. Every edit is one op batch through [`crate::ops::apply`] with a history snapshot
//! (MAP-61), the 3D viewport is dc-render on the GPU eframe shares (MAP-62), the welcome screen scaffolds a map
//! under `Documents/DeCombat Maps/<name>` or opens one and remembers the ROM folder (MAP-63), the panels are
//! tools / inspector / problems / status (MAP-64), the asset browser lists own and ROM textures with cached
//! thumbnails (MAP-65), and on Windows the editor is the second binary `dc-mapkit-gui` the console `gui` verb
//! spawns (MAP-66).
//!
//! The modules without egui types ([`camera`], [`pick`], [`commands`], [`model`], [`welcome`]) are compiled and
//! unit-tested without the `gui` feature; the window ([`app`], [`viewport`], [`assets`], [`jobs`]) needs it.

pub mod camera;
pub mod commands;
pub mod font;
pub mod model;
pub mod pick;
pub mod welcome;

#[cfg(feature = "gui")]
pub mod app;
#[cfg(feature = "gui")]
pub mod assets;
#[cfg(feature = "gui")]
pub mod jobs;
#[cfg(feature = "gui")]
pub mod viewport;

/// The editor binary's name (what the console `gui` verb spawns, MAP-66).
pub const GUI_BINARY: &str = "dc-mapkit-gui";

/// Run the editor window until it closes. `dir` opens that project straight away.
// DC-DEFAULT: MAP-62 (GpuContext::headless first, then eframe on WgpuSetup::Existing)
#[cfg(feature = "gui")]
pub fn launch(dir: Option<std::path::PathBuf>) -> Result<(), String> {
    launch_with(dir, None)
}

/// [`launch`] with the scripted smoke run writing under `smoke_out` (`dc-mapkit-gui --smoke <out> <dir>`).
#[cfg(feature = "gui")]
pub fn launch_with(
    dir: Option<std::path::PathBuf>,
    smoke_out: Option<std::path::PathBuf>,
) -> Result<(), String> {
    use dc_render::{AdapterChoice, GpuContext};
    let gpu = GpuContext::headless(AdapterChoice::Auto).map_err(|e| {
        let probes: Vec<String> = dc_render::probe_adapters()
            .into_iter()
            .map(|p| format!("{}: {:?}", p.attempt, p.result))
            .collect();
        format!("no usable GPU: {e}\n{}", probes.join("\n"))
    })?;
    let setup = egui_wgpu::WgpuSetupExisting {
        instance: gpu.instance.clone(),
        adapter: gpu.adapter.clone(),
        device: gpu.device.clone(),
        queue: gpu.queue.clone(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DeCombat map kit")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::Existing(setup),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "dc-mapkit",
        options,
        Box::new(move |cc| {
            let editor = app::Editor::new(cc, dir, gpu);
            Ok(Box::new(match smoke_out {
                Some(out) => editor.with_smoke(out),
                None => editor,
            }))
        }),
    )
    .map_err(|e| e.to_string())
}

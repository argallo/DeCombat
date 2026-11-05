//! The studio's window.
pub mod app;
pub mod viewport;

use std::path::PathBuf;

/// The name of the window's own binary, which the console `dc-gunkit` starts.
pub const GUI_BINARY: &str = "dc-gunkit-gui";

/// The window's font (Apache-2.0, `assets/fonts/`): egui's own font pack is off, it is outside CLEANROOM.md 4a.
const ROBOTO: &[u8] = include_bytes!("../../../../assets/fonts/Roboto-Regular.ttf");

fn fonts() -> egui::FontDefinitions {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut defs = FontDefinitions::empty();
    defs.font_data.insert(
        "Roboto".to_string(),
        std::sync::Arc::new(FontData::from_static(ROBOTO)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        defs.families.insert(family, vec!["Roboto".to_string()]);
    }
    defs
}

/// Runs the window until it closes. `rom` names the game folder, `gun` the first gun on show.
pub fn launch(rom: Option<PathBuf>, gun: Option<String>) -> Result<(), String> {
    use dc_render::{AdapterChoice, GpuContext};
    // the same device draws the gun and the window: the viewport's picture is sampled directly
    let gpu =
        GpuContext::headless(AdapterChoice::Auto).map_err(|e| format!("no usable GPU: {e}"))?;
    let setup = egui_wgpu::WgpuSetupExisting {
        instance: gpu.instance.clone(),
        adapter: gpu.adapter.clone(),
        device: gpu.device.clone(),
        queue: gpu.queue.clone(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DeCombat GunKit")
            .with_inner_size([1360.0, 820.0])
            .with_min_inner_size([980.0, 600.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::Existing(setup),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "dc-gunkit",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, gpu, rom, gun)))),
    )
    .map_err(|e| e.to_string())
}

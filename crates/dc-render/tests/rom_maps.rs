//! ROM-gated: renders every world of the user's ROM from its first player spawn point to a PNG and reports
//! the frame build times. Skipped when CA_ROM is unset or no graphics adapter exists. Map names come from the
//! user's archive at run time; the images are ROM-derived local files (folder from `DC_RENDER_OUT`, else the
//! system temp folder), never part of the tree.

use dc_render::*;

/// FMT_DAT85.md §11: the ROM carries 21 worlds.
const ROM_WORLDS: usize = 21;

#[test]
fn every_rom_world_renders_from_its_first_spawn() {
    let rom = dc_rom::rom_or_skip!();
    let viewer = match RomViewer::open(&rom, AdapterChoice::Auto, RenderSettings::default()) {
        Ok(v) => v,
        Err(ViewError::Gpu(e)) => {
            println!("skipped: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let out = default_out_dir();
    let maps = viewer.map_names();
    assert_eq!(maps.len(), ROM_WORLDS);
    let clear = CLEAR_COLOUR.map(|c| (c * 255.0).round() as u8);
    let mut total_ms = 0.0;
    for map in &maps {
        let mut req = ViewRequest::new(map);
        req.width = 1024;
        req.height = 768;
        let (img, report) = viewer.render(&req).unwrap_or_else(|e| panic!("{map}: {e}"));
        let path = out.join(format!("{}.png", map.to_ascii_lowercase()));
        img.save_png(&path).expect("png");
        let t = report.timings;
        let build = t.parse_ms + t.scene_build_ms + t.texture_decode_ms + t.upload_ms;
        total_ms += build + t.gpu_frame_ms;
        println!("{}", report.summary());
        println!(
            "  frame build total {build:.1} ms + gpu {:.2} ms -> {}",
            t.gpu_frame_ms,
            path.display()
        );

        // Every frame shows world geometry: most pixels differ from the clear colour.
        let covered = img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] != clear[0] || p[1] != clear[1] || p[2] != clear[2])
            .count();
        let frac = covered as f64 / (img.width * img.height) as f64;
        println!("  covered {:.1} %", frac * 100.0);
        assert!(
            frac > 0.25,
            "{map}: only {:.1} % of the frame shows geometry",
            frac * 100.0
        );
        assert_eq!(report.scene.lightmaps_bad, 0, "{map}: undecodable lightmap");
        assert_eq!(
            report.scene.bad_ranges, 0,
            "{map}: section triangle range past the block"
        );
        assert!(report.scene.sections_drawn > 0);
    }
    println!("all {} worlds: {:.0} ms total", maps.len(), total_ms);
}

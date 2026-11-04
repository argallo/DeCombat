//! The editor's UI font: Roboto (Apache-2.0, `assets/fonts/`), compiled in with `include_bytes!` and bound to
//! BOTH egui font families, because the editor is built with egui's `default_fonts` feature off (its own pack is
//! OFL / Ubuntu Font Licence, outside CLEANROOM.md 4a) and egui panics when a family has no font.
// DC-DEFAULT: MAP-64

/// The TrueType bytes of Roboto Regular.
pub const ROBOTO: &[u8] = include_bytes!("../../../../assets/fonts/Roboto-Regular.ttf");
/// The name the font is registered under.
pub const FONT_NAME: &str = "roboto";

/// A quick shape check of a TrueType / OpenType file: the `sfnt` version tag.
pub fn looks_like_truetype(bytes: &[u8]) -> bool {
    bytes.len() > 12
        && (bytes.starts_with(&[0x00, 0x01, 0x00, 0x00])
            || bytes.starts_with(b"true")
            || bytes.starts_with(b"OTTO"))
}

/// The font definitions the editor installs: Roboto for the proportional AND the monospace family.
#[cfg(feature = "gui")]
pub fn definitions() -> egui::FontDefinitions {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut defs = FontDefinitions::empty();
    defs.font_data.insert(
        FONT_NAME.to_string(),
        std::sync::Arc::new(FontData::from_static(ROBOTO)),
    );
    defs.families
        .insert(FontFamily::Proportional, vec![FONT_NAME.to_string()]);
    defs.families
        .insert(FontFamily::Monospace, vec![FONT_NAME.to_string()]);
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_font_is_a_truetype_file() {
        assert!(looks_like_truetype(ROBOTO));
        assert!(ROBOTO.len() > 100_000);
        assert!(!looks_like_truetype(b"not a font"));
    }

    /// The bytes parse: egui lays text out with them in a headless context (no window, no GPU).
    #[cfg(feature = "gui")]
    #[test]
    fn egui_accepts_the_font_for_both_families() {
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions());
        let mut proportional = 0.0;
        let mut mono = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                proportional = ui.label("DeCombat map kit").rect.width();
                mono = ui.monospace("DeCombat map kit").rect.width();
            });
        });
        assert!(proportional > 10.0 && mono > 10.0, "{proportional} {mono}");
    }
}

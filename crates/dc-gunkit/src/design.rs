//! What a camo is, with no window and no GPU: the art, its effect, and the circles it leaves unpainted. It paints through
//! `dc_render::camo`, the same code as the game (VAR-3, VAR-4).

use crate::guns::View;
use dc_render::camo::{self, CamoFlow, CamoFx, Circle};
use dc_render::TextureImage;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// The largest side of art the studio takes (the game's largest gun texture is 1024).
pub const MAX_ART_SIDE: u32 = 2048;

/// A picture of the author's own, and where it came from when it is a file (a changed file is read again).
#[derive(Debug, Clone, PartialEq)]
pub struct Art {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    pub source: Option<PathBuf>,
    pub stamp: Option<SystemTime>,
}

impl Art {
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Art, String> {
        if width == 0 || height == 0 || width > MAX_ART_SIDE || height > MAX_ART_SIDE {
            return Err(format!(
                "art must be 1 to {MAX_ART_SIDE} pixels on a side, this is {width} x {height}"
            ));
        }
        if rgba.len() != (width * height * 4) as usize {
            return Err("art data does not match its size".into());
        }
        Ok(Art {
            width,
            height,
            rgba: Arc::new(rgba),
            source: None,
            stamp: None,
        })
    }

    /// Art from PNG bytes.
    pub fn from_png(bytes: &[u8]) -> Result<Art, String> {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .map_err(|e| format!("not a readable PNG: {e}"))?
            .to_rgba8();
        let (w, h) = img.dimensions();
        Art::from_rgba(w, h, img.into_raw())
    }

    /// Art from a PNG file, or a DTX file such as a pack's asset.
    pub fn from_file(path: &Path) -> Result<Art, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let is_dtx = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dtx"));
        let mut art = if is_dtx {
            Art::from_dtx(&bytes).map_err(|e| format!("{}: {e}", path.display()))?
        } else {
            Art::from_png(&bytes).map_err(|e| format!("{}: {e}", path.display()))?
        };
        art.source = Some(path.to_path_buf());
        art.stamp = file_stamp(path);
        Ok(art)
    }

    /// Art from a DTX file's bytes.
    pub fn from_dtx(bytes: &[u8]) -> Result<Art, String> {
        let dtx = dc_tex::DtxFile::parse(bytes).map_err(|e| format!("not a readable DTX: {e}"))?;
        let img = dc_render::decode_dtx(&dtx).ok_or("this DTX cannot be decoded")?;
        let rgba = img.levels.into_iter().next().ok_or("this DTX is empty")?;
        Art::from_rgba(img.width, img.height, rgba)
    }

    /// The art again when its file changed since it was read: an editor or an agent saved it.
    pub fn reloaded(&self) -> Option<Result<Art, String>> {
        let path = self.source.as_ref()?;
        let now = file_stamp(path)?;
        (Some(now) != self.stamp).then(|| Art::from_file(path))
    }

    pub fn image(&self) -> TextureImage {
        TextureImage::rgba(self.width, self.height, self.rgba.as_ref().clone())
    }
}

fn file_stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The effect's limits, for sliders (VAR-4 holds any value to them on read).
pub const FX_DRIFT_MAX: f32 = 1.0;
pub const FX_PULSE_MIN: f32 = 1.0;

/// A camo being designed. One design paints every gun; what depends on the gun's texture is kept per gun and view.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Design {
    pub name: String,
    /// The pattern painted over the gun's own texture, keeping its shading.
    pub paint: Option<Art>,
    /// The pattern the effect moves over the gun as light (`None`: the painted pattern).
    pub glow: Option<Art>,
    pub fx: Option<CamoFx>,
    /// The paint's colour itself moving, like a lava lamp (VAR-12). It takes the place of the light effect.
    pub flow: Option<CamoFlow>,
    /// Texture painted to the gun's own layout, which replaces the texture, per gun and view.
    pub full: BTreeMap<(u32, View), Art>,
    /// The circles left as the gun drew them (a scope's lens), in texels of the texture, per gun and view.
    pub keep: BTreeMap<(u32, View), Vec<Circle>>,
}

impl Design {
    pub fn keep_of(&self, row: u32, view: View) -> &[Circle] {
        self.keep
            .get(&(row, view))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Whether this gun and view show anything but the game's own texture.
    pub fn changes(&self, row: u32, view: View) -> bool {
        self.paint.is_some()
            || self.fx.is_some()
            || self.flow.is_some()
            || self.full.contains_key(&(row, view))
    }

    /// The texture of a gun's main slot under this design. `None`: the gun's own texture stays.
    pub fn painted(&self, row: u32, view: View, base: &TextureImage) -> Option<TextureImage> {
        self.painted_scaled(row, view, base, 1.0)
    }

    /// As [`Design::painted`], for a `base` larger than the game's texture by `scale`.
    pub fn painted_scaled(
        &self,
        row: u32,
        view: View,
        base: &TextureImage,
        scale: f32,
    ) -> Option<TextureImage> {
        let scaled: Vec<Circle> = self
            .keep_of(row, view)
            .iter()
            .map(|c| Circle {
                x: c.x * scale,
                y: c.y * scale,
                radius: c.radius * scale,
            })
            .collect();
        let keep = scaled.as_slice();
        if let (None, Some(p), Some(flow)) = (self.full.get(&(row, view)), &self.paint, self.flow) {
            return camo::flowing(base, &p.image(), keep, flow);
        }
        let mut out = match (self.full.get(&(row, view)), &self.paint) {
            (Some(full), _) => camo::replace(base, &full.image(), keep)?,
            (None, Some(p)) => camo::paint(base, &p.image(), keep)?,
            (None, None) => {
                self.fx?;
                let mut i =
                    TextureImage::rgba(base.width, base.height, base.levels.first()?.clone());
                i.alpha_cutoff = base.alpha_cutoff;
                i.mip_bias = base.mip_bias;
                i
            }
        };
        if let Some(fx) = self.fx {
            let moved = self
                .glow
                .as_ref()
                .or(self.paint.as_ref())
                .or(self.full.get(&(row, view)))?;
            out.fx = camo::fx_of(base, &moved.image(), keep, fx).map(Arc::new);
        }
        Some(out)
    }

    /// Reads again any art whose file changed. Returns whether anything did, and the first error.
    pub fn reload_changed(&mut self) -> (bool, Option<String>) {
        let mut changed = false;
        let mut error = None;
        let mut check = |art: &mut Art| match art.reloaded() {
            Some(Ok(new)) => {
                *art = new;
                changed = true;
            }
            Some(Err(e)) => {
                // a file being written reads as broken for a moment: try again on the next look, once it has settled
                art.stamp = file_stamp(art.source.as_deref().unwrap_or(Path::new("")));
                error.get_or_insert(e);
            }
            None => {}
        };
        if let Some(a) = self.paint.as_mut() {
            check(a);
        }
        if let Some(a) = self.glow.as_mut() {
            check(a);
        }
        for a in self.full.values_mut() {
            check(a);
        }
        (changed, error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        c.repeat((w * h) as usize)
    }

    fn base() -> TextureImage {
        TextureImage::rgba(8, 8, flat(8, 8, [100, 100, 100, 255]))
    }

    #[test]
    fn with_nothing_chosen_the_gun_keeps_its_texture() {
        assert!(Design::default()
            .painted(25, View::FirstPerson, &base())
            .is_none());
        assert!(!Design::default().changes(25, View::FirstPerson));
    }

    #[test]
    fn a_pattern_paints_as_the_game_does() {
        let mut d = Design::default();
        let art = Art::from_rgba(4, 4, flat(4, 4, [200, 10, 10, 255])).unwrap();
        let want = camo::paint(&base(), &art.image(), &[]).unwrap();
        d.paint = Some(art);
        assert_eq!(d.painted(25, View::FirstPerson, &base()), Some(want));
    }

    #[test]
    fn a_kept_circle_belongs_to_one_gun_and_view() {
        let mut d = Design {
            paint: Some(Art::from_rgba(4, 4, flat(4, 4, [200, 10, 10, 255])).unwrap()),
            ..Design::default()
        };
        let lens = Circle {
            x: 4.0,
            y: 4.0,
            radius: 3.0,
        };
        d.keep.insert((25, View::FirstPerson), vec![lens]);
        let kept = d.painted(25, View::FirstPerson, &base()).unwrap();
        assert_eq!(kept.levels[0][(4 * 8 + 4) * 4], 100);
        let other = d.painted(27, View::FirstPerson, &base()).unwrap();
        assert_ne!(other.levels[0][(4 * 8 + 4) * 4], 100);
        let third = d.painted(25, View::ThirdPerson, &base()).unwrap();
        assert_ne!(third.levels[0][(4 * 8 + 4) * 4], 100);
    }

    #[test]
    fn a_full_texture_replaces_one_gun_and_view_only() {
        let mut d = Design::default();
        let full = Art::from_rgba(2, 2, flat(2, 2, [1, 2, 3, 255])).unwrap();
        d.full.insert((25, View::FirstPerson), full);
        let t = d.painted(25, View::FirstPerson, &base()).unwrap();
        assert_eq!((t.width, t.levels[0][0]), (8, 1), "at the base's size");
        assert!(d.painted(27, View::FirstPerson, &base()).is_none());
    }

    #[test]
    fn an_effect_needs_something_to_move() {
        let fx = CamoFx {
            speed: [0.02, 0.0],
            gain: [0.0, 0.5],
            seconds: 0.0,
        };
        let mut d = Design {
            fx: Some(fx),
            ..Design::default()
        };
        assert!(d.painted(25, View::FirstPerson, &base()).is_none());
        d.glow = Some(Art::from_rgba(4, 4, flat(4, 4, [9, 9, 200, 255])).unwrap());
        let t = d.painted(25, View::FirstPerson, &base()).unwrap();
        assert_eq!(t.levels[0], base().levels[0], "only light is added");
        assert!(t.fx.is_some());
    }

    #[test]
    fn art_of_the_wrong_size_is_refused_in_words() {
        assert!(Art::from_rgba(0, 4, Vec::new()).is_err());
        assert!(Art::from_rgba(4, 4, vec![0; 3]).is_err());
        assert!(Art::from_rgba(4096, 4, vec![0; 4096 * 4 * 4]).is_err());
        assert!(Art::from_png(b"not a png").is_err());
    }

    #[test]
    fn a_saved_file_is_read_again() {
        let dir = std::env::temp_dir().join(format!("dc-gunkit-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        let save = |c: [u8; 4]| {
            image::RgbaImage::from_raw(2, 2, flat(2, 2, c))
                .unwrap()
                .save(&path)
                .unwrap()
        };
        save([1, 1, 1, 255]);
        let mut d = Design {
            paint: Some(Art::from_file(&path).unwrap()),
            ..Design::default()
        };
        assert_eq!(d.reload_changed(), (false, None));
        save([9, 9, 9, 255]);
        d.paint.as_mut().unwrap().stamp = Some(SystemTime::UNIX_EPOCH);
        assert_eq!(d.reload_changed(), (true, None));
        assert_eq!(d.paint.as_ref().unwrap().rgba[0], 9);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

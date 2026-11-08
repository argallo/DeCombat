//! Virtual-screen scaler: every layout coordinate of the 2009 UI is authored for a 1024 x 768 screen and
//! scaled per axis (`width / 1024`, `height / 768`). EVIDENCE: cshell+0x1bef80 (SPEC FMT_CRES_UI.md §5 step 5)
//!
//! A surface wider than 16:9 keeps the 16:9 stretch: the virtual screen maps to a band `height x 16 / 9` pixels
//! wide, centred on the surface (`sx = sy x 4 / 3`, origin `ox = (width - band) / 2`); the world fills the whole
//! surface behind it. A surface at 16:9 or narrower is scaled as before (the band is the whole surface).
// DC-DEFAULT: CLI-1915 (proposed, OWNER DIRECTION 2026-11-07 "supporting ultra wide should be across the board")

/// Width of the virtual screen the layout data is authored for.
pub const VIRTUAL_WIDTH: u32 = 1024;
/// Height of the virtual screen the layout data is authored for.
pub const VIRTUAL_HEIGHT: u32 = 768;
/// The widest aspect the 2D layer is stretched to (width : height); a wider surface centres a band of it.
pub const BAND_ASPECT: (u32, u32) = (16, 9);

/// Width in pixels of the band the virtual screen maps to on a `width` x `height` surface: the whole width at
/// 16:9 or narrower; else `height x 16 / 9` rounded, one pixel wider when needed so that the two side bands are
/// the same whole number of pixels (the band's centre is the surface's centre).
// DC-DEFAULT: CLI-1915 (proposed)
pub fn band_width(width: u32, height: u32) -> u32 {
    let (aw, ah) = (BAND_ASPECT.0 as u64, BAND_ASPECT.1 as u64);
    if width as u64 * ah <= height as u64 * aw {
        return width;
    }
    let mut band = ((height as u64 * aw + ah / 2) / ah) as u32;
    if (width - band.min(width)) % 2 == 1 {
        band += 1;
    }
    band.min(width)
}

/// Per-axis scale from virtual 1024 x 768 coordinates to a real surface, with the band's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualScaler {
    pub sx: f32,
    pub sy: f32,
    /// Left edge of the band in surface pixels (0 at 16:9 or narrower).
    pub ox: f32,
    /// Top edge of the band in surface pixels (always 0: the band spans the whole height).
    pub oy: f32,
    /// The surface's size in pixels.
    pub width: u32,
    pub height: u32,
}

impl VirtualScaler {
    /// Scaler for a `width` x `height` surface (the band rule of [`band_width`]).
    pub fn new(width: u32, height: u32) -> Self {
        let band = band_width(width, height);
        VirtualScaler {
            sx: band as f32 / VIRTUAL_WIDTH as f32,
            sy: height as f32 / VIRTUAL_HEIGHT as f32,
            ox: ((width - band) / 2) as f32,
            oy: 0.0,
            width,
            height,
        }
    }

    /// The band the virtual screen maps to, in surface pixels: `[x, y, width, height]`.
    pub fn band(&self) -> [u32; 4] {
        let w = band_width(self.width, self.height);
        [self.ox as u32, self.oy as u32, w, self.height]
    }

    /// The band's size in pixels (a surface of this size is drawn like the band, without side bands).
    pub fn band_size(&self) -> (u32, u32) {
        let b = self.band();
        (b[2], b[3])
    }

    /// True when the band is narrower than the surface (side bands to the left and right).
    pub fn banded(&self) -> bool {
        self.ox > 0.0
    }

    /// Scales a virtual point onto the surface (the band's origin added).
    pub fn point(&self, p: [i32; 2]) -> [f32; 2] {
        [
            p[0] as f32 * self.sx + self.ox,
            p[1] as f32 * self.sy + self.oy,
        ]
    }

    /// Scales a virtual size (no origin).
    pub fn size(&self, s: [i32; 2]) -> [f32; 2] {
        [s[0] as f32 * self.sx, s[1] as f32 * self.sy]
    }

    /// Scales a virtual rectangle `(left, top, right, bottom)` onto the surface.
    pub fn rect(&self, r: [i32; 4]) -> [f32; 4] {
        let a = self.point([r[0], r[1]]);
        let b = self.point([r[2], r[3]]);
        [a[0], a[1], b[0], b[1]]
    }

    /// Maps a surface point back to virtual coordinates (input hit-testing); a point in a side band lies left of
    /// 0 or right of 1024.
    pub fn to_virtual(&self, p: [f32; 2]) -> [f32; 2] {
        let inv = |v: f32, s: f32| if s == 0.0 { 0.0 } else { v / s };
        [inv(p[0] - self.ox, self.sx), inv(p[1] - self.oy, self.sy)]
    }

    /// The whole surface in virtual coordinates `(left, top, right, bottom)`: `(0, 0, 1024, 768)` without side
    /// bands, wider than the virtual screen with them (world-anchored marks may sit in a side band).
    pub fn surface_virtual(&self) -> [f32; 4] {
        if !self.banded() {
            return [0.0, 0.0, VIRTUAL_WIDTH as f32, VIRTUAL_HEIGHT as f32];
        }
        let l = self.to_virtual([0.0, 0.0]);
        let r = self.to_virtual([self.width as f32, self.height as f32]);
        [l[0], 0.0, r[0], VIRTUAL_HEIGHT as f32]
    }

    /// A point given as a share of the whole surface (0..1 on each axis, y down: a projected world point) in
    /// virtual coordinates. Without side bands this is `u x (1024, 768)`.
    pub fn from_surface_share(&self, u: [f32; 2]) -> [f32; 2] {
        let y = u[1] * VIRTUAL_HEIGHT as f32;
        if !self.banded() {
            return [u[0] * VIRTUAL_WIDTH as f32, y];
        }
        [self.to_virtual([u[0] * self.width as f32, 0.0])[0], y]
    }

    /// Scales a point and rounds to whole pixels.
    pub fn point_px(&self, p: [i32; 2]) -> [i32; 2] {
        let q = self.point(p);
        [q[0].round() as i32, q[1].round() as i32]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_per_axis() {
        let s = VirtualScaler::new(1024, 768);
        assert_eq!(s.point([10, 20]), [10.0, 20.0]);
        let s = VirtualScaler::new(2048, 1152);
        assert_eq!(s.point([10, 20]), [20.0, 30.0]);
        assert_eq!(s.rect([0, 0, 512, 384]), [0.0, 0.0, 1024.0, 576.0]);
        assert_eq!(s.to_virtual([20.0, 30.0]), [10.0, 20.0]);
        assert_eq!(
            VirtualScaler::new(800, 600).point_px([1024, 768]),
            [800, 600]
        );
        assert_eq!(VirtualScaler::new(0, 0).to_virtual([5.0, 5.0]), [0.0, 0.0]);
    }

    #[test]
    fn a_surface_wider_than_16_9_centres_a_16_9_band() {
        // 16:9 and narrower: the whole surface, sx = width / 1024, no origin
        for (w, h) in [
            (2560, 1440),
            (1920, 1080),
            (1024, 768),
            (1680, 1050),
            (1366, 768),
        ] {
            let s = VirtualScaler::new(w, h);
            assert_eq!(s.sx, w as f32 / 1024.0, "{w}x{h}");
            assert_eq!(s.sy, h as f32 / 768.0, "{w}x{h}");
            assert_eq!((s.ox, s.oy), (0.0, 0.0), "{w}x{h}");
            assert_eq!(s.band(), [0, 0, w, h], "{w}x{h}");
            assert!(!s.banded());
            assert_eq!(s.surface_virtual(), [0.0, 0.0, 1024.0, 768.0]);
        }
        // 4:3 is unchanged
        let s = VirtualScaler::new(1600, 1200);
        assert_eq!((s.sx, s.sy, s.ox), (1600.0 / 1024.0, 1200.0 / 768.0, 0.0));
        // 21:9 and 32:9: sx = sy x 4 / 3, the band centred
        for (w, h, band) in [
            (3440, 1440, 2560),
            (5120, 1440, 2560),
            (2560, 1080, 1920),
            (3840, 1080, 1920),
        ] {
            let s = VirtualScaler::new(w, h);
            assert_eq!(s.sx, s.sy * 4.0 / 3.0, "{w}x{h}");
            assert_eq!(s.ox, ((w - band) / 2) as f32, "{w}x{h}");
            assert_eq!(s.band(), [(w - band) / 2, 0, band, h], "{w}x{h}");
            assert_eq!(s.band_size(), (band, h));
            assert!(s.banded());
            // the band is centred: as much room left as right
            assert_eq!(s.ox * 2.0 + band as f32, w as f32);
            assert_eq!(s.point([0, 0]), [s.ox, 0.0]);
            assert_eq!(s.point([1024, 768]), [s.ox + band as f32, h as f32]);
            assert_eq!(s.size([1024, 768]), [band as f32, h as f32]);
            assert_eq!(s.to_virtual(s.point([300, 200])), [300.0, 200.0]);
            assert_eq!(s.from_surface_share([0.5, 0.5]), [512.0, 384.0]);
            let v = s.surface_virtual();
            assert!(v[0] < 0.0 && v[2] > 1024.0 && (v[0] + v[2] - 1024.0).abs() < 1e-3);
        }
        // the side bands are whole pixels and equal on an odd width
        let s = VirtualScaler::new(3441, 1440);
        assert_eq!(s.band(), [440, 0, 2561, 1440]);
        // a band is drawn like a surface of its own size
        let b = VirtualScaler::new(3440, 1440).band_size();
        let t = VirtualScaler::new(b.0, b.1);
        assert_eq!((t.sx, t.sy, t.ox), (2.5, 1440.0 / 768.0, 0.0));
    }
}

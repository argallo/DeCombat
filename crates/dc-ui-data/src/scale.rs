//! Virtual-screen scaler: every layout coordinate of the 2009 UI is authored for a 1024 x 768 screen and
//! scaled per axis (`width / 1024`, `height / 768`). EVIDENCE: cshell+0x1bef80 (SPEC FMT_CRES_UI.md §5 step 5)

/// Width of the virtual screen the layout data is authored for.
pub const VIRTUAL_WIDTH: u32 = 1024;
/// Height of the virtual screen the layout data is authored for.
pub const VIRTUAL_HEIGHT: u32 = 768;

/// Per-axis scale from virtual 1024 x 768 coordinates to a real surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualScaler {
    pub sx: f32,
    pub sy: f32,
}

impl VirtualScaler {
    /// Scaler for a `width` x `height` surface.
    pub fn new(width: u32, height: u32) -> Self {
        VirtualScaler {
            sx: width as f32 / VIRTUAL_WIDTH as f32,
            sy: height as f32 / VIRTUAL_HEIGHT as f32,
        }
    }

    /// Scales a virtual point.
    pub fn point(&self, p: [i32; 2]) -> [f32; 2] {
        [p[0] as f32 * self.sx, p[1] as f32 * self.sy]
    }

    /// Scales a virtual size.
    pub fn size(&self, s: [i32; 2]) -> [f32; 2] {
        self.point(s)
    }

    /// Scales a virtual rectangle `(left, top, right, bottom)`.
    pub fn rect(&self, r: [i32; 4]) -> [f32; 4] {
        [
            r[0] as f32 * self.sx,
            r[1] as f32 * self.sy,
            r[2] as f32 * self.sx,
            r[3] as f32 * self.sy,
        ]
    }

    /// Maps a surface point back to virtual coordinates (input hit-testing).
    pub fn to_virtual(&self, p: [f32; 2]) -> [f32; 2] {
        let inv = |v: f32, s: f32| if s == 0.0 { 0.0 } else { v / s };
        [inv(p[0], self.sx), inv(p[1], self.sy)]
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
}

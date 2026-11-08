//! The scope overlay slots and their picture.
//!
//! The 2009 interface manager keeps eight overlay slots. Each holds a sprite object the engine draws in front of the
//! camera, made from the held weapon's sight mod (`ZoomSpriteName`, scaled by `ZoomSpriteScale`): slot 0 while the
//! zoom level is at least 1, slot 6 during a change in, slot 7 during a change out (both go when the change ends),
//! and any slot the server switches with `GS_OVERLAY` 0x77. [`crate::zoom::Zoom`] keeps the table; the HUD reads it
//! through [`ScopeView`] and draws it with [`push_items`] under every other HUD element.
//!
//! The sprite object (cshell+0x1b88f0): object type 3 (a sprite), flags 0x2d1 (visible and the eye-attached bit
//! 0x40), position = the forward axis of the identity rotation times 1.0, i.e. one unit straight ahead in eye space,
//! scale = the sight's `ZoomSpriteScale` (cshell+0x1b5d50 sets it every frame, times 1.0 on each axis). The engine
//! draws a sprite with bit 0x40 with the player-view camera (engine+0x1b1210 switches to engine+0x1b4870: vertical
//! field 50 degrees, the target's aspect) on identity axes, and its half extents are the frame texture's width and
//! height times the scale (engine+0x1af730), so a texel spans twice the scale. On the target the picture is centred,
//! and one texel covers `2 x scale / tan(25 degrees) x height / 2` pixels on both axes, whatever the camera's zoomed
//! field is.
// EVIDENCE: cshell+0x1b88f0 (create: free slot, the sight via slot +0x11c(1), +0x50 sprite name, +0x19c scale stored at
// +0x5360 + 4 x slot, object type 3, flags 0x2d1, position = cshell+0x1b8a0 forward of the identity rotation x 1.0
// (the f32 at VA 0x3755447c)), cshell+0x1b5d50 (per frame: of slots 0..2 only the lowest filled one is shown, slots
// 3..7 always, while the interface state is 1; scale = stored scale x 1.0 (VA 0x375545c8..d0)), cshell+0x1b37b0 /
// cshell+0x1b3890 / cshell+0x1b37f0 (removal), cshell+0x1b8bc0 (a change in = slot 6, out = slot 7), cshell+0x1b8b90
// (slot 0, and 3 / 4 by flags); engine+0x1b1210 (flag 0x40: player-view camera engine+0x1b4870), engine+0x1af730
// (flag 0x40: identity axes, no near cut; half extents = texture width / height x scale)
// SPEC: GAME_MESSAGES_2009_S2C.md 0x77 row (cshell+0x1b90e0); RENDER_2009.md §4.2 (player-view camera)
// DC-DEFAULT: CLI-615 the sprite is drawn as the first HUD image (the 2009 HUD is drawn after the scene, so the
// order is the same); a sprite file shows its first frame; alpha blended (flag word 2 is 0)

/// The overlay slots of the interface manager.
// EVIDENCE: cshell+0x1b88f0 (slot table +0x5340, 4 bytes per slot), cshell+0x1b90e0 (0x77: slots >= 8 ignored)
pub const OVERLAY_SLOTS: usize = 8;
/// The scope picture while the zoom level is at least 1.
pub const OVERLAY_SCOPE: usize = 0;
/// The picture of a running change in / out.
pub const OVERLAY_CHANGE_IN: usize = 6;
pub const OVERLAY_CHANGE_OUT: usize = 7;
/// The slots removed when a zoom-out ends at level 0.
// EVIDENCE: cshell+0x1b37f0 (slots 0, 3, 4, 5)
pub const OVERLAY_LEVEL_SLOTS: [usize; 4] = [0, 3, 4, 5];

/// One overlay slot: the sight's sprite and scale.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlaySprite {
    pub sprite: String,
    pub scale: f32,
}

/// The eight slots.
pub type OverlayTable = [Option<OverlaySprite>; OVERLAY_SLOTS];

/// What the HUD may show of the scope this frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScopeView {
    /// The zoom level (0 = no zoom).
    pub level: u32,
    /// The zoom state (level >= 1 or a change running).
    pub active: bool,
    /// The filled slots, lowest first: `(slot, sprite)`.
    pub overlays: Vec<(usize, OverlaySprite)>,
}

impl ScopeView {
    /// The view of a zoom state.
    pub fn of(zoom: &crate::zoom::Zoom) -> ScopeView {
        ScopeView {
            level: zoom.level(),
            active: zoom.active(),
            overlays: zoom
                .overlays()
                .iter()
                .enumerate()
                .filter_map(|(i, o)| o.clone().map(|o| (i, o)))
                .collect(),
        }
    }

    /// True while the scope picture (slot 0) is up.
    pub fn scope_up(&self) -> bool {
        self.overlays.iter().any(|(s, _)| *s == OVERLAY_SCOPE)
    }

    /// The pictures drawn this frame, in slot order: of the slots below [`FIRST_ALWAYS_SHOWN`] only the lowest
    /// filled one, every filled slot from there on.
    // EVIDENCE: cshell+0x1b5d50 (the `shown` flag set by the first slot < 3, later slots < 3 hidden)
    pub fn drawn(&self) -> Vec<&OverlaySprite> {
        let mut low = false;
        let mut out = Vec::new();
        for (slot, o) in &self.overlays {
            if *slot < FIRST_ALWAYS_SHOWN {
                if low {
                    continue;
                }
                low = true;
            }
            out.push(o);
        }
        out
    }
}

/// Slots from this one on are shown whatever the lower slots hold.
// EVIDENCE: cshell+0x1b5d50 (`iVar4 < 3`)
pub const FIRST_ALWAYS_SHOWN: usize = 3;
/// The sprite's distance ahead of the eye (eye-space units).
// EVIDENCE: cshell+0x1b88f0 (forward axis x the f32 1.0 at VA 0x3755447c)
pub const SPRITE_DEPTH: f32 = 1.0;

/// Virtual pixels (1024 x 768 space, x then y) one texel of a scope picture covers on a `width` x `height` target.
pub fn texel_size(scale: f32, width: u32, height: u32) -> [f32; 2] {
    let fy = dc_render::PLAYER_VIEW_DEFAULT.fov_y_deg.to_radians();
    // eye units per texel (the half extent is texels x scale) / eye units per half screen height at the depth
    let k = 2.0 * scale / (SPRITE_DEPTH * (fy * 0.5).tan());
    // CLI-1915: the virtual width is the 16:9 band's on a wider target
    let band = dc_ui_data::VirtualScaler::new(width.max(1), height.max(1)).band_size().0;
    let (w, h) = (band.max(1) as f32, height.max(1) as f32);
    let vh = dc_ui_data::VIRTUAL_HEIGHT as f32;
    let vw = dc_ui_data::VIRTUAL_WIDTH as f32;
    // target pixels per texel: k x height / 2 on both axes (the player-view projection divides x by the aspect)
    let px = k * h * 0.5;
    [px * vw / w, px * vh / h]
}

/// Pushes the scope pictures of `view` (centred, sized by [`texel_size`]); `tex_size` gives a picture's texel size,
/// a picture without one is skipped.
pub fn push_items(
    items: &mut Vec<super::DrawItem>,
    view: &ScopeView,
    target: (u32, u32),
    tex_size: &dyn Fn(&str) -> Option<[f32; 2]>,
) {
    for o in view.drawn() {
        let Some([tw, th]) = tex_size(&o.sprite) else {
            continue;
        };
        let [ux, uy] = texel_size(o.scale, target.0, target.1);
        let (w, h) = (tw * ux, th * uy);
        let c = super::items::VIRTUAL_CENTRE;
        items.push(super::DrawItem::Image {
            tex: o.sprite.clone(),
            src: [tw, th],
            dst: [c[0] - w * 0.5, c[1] - h * 0.5, w, h],
            tint: [255; 4],
            turn: None,
        });
    }
}

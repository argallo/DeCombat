//! Weapon variants on the client (VAR-1 .. VAR-3): which rows are variants of which gun, and the camo a variant's skin
//! names.
//!
//! A pack may not carry anything taken from the game (docs/MODDING_GUIDE.md section 8), so a camo ships as its own pattern
//! and a skin path that says how to use it: `@camo:<mode>:<base row>:<skin key>:[keep=x,y,radius[/x,y,radius...]:]<pattern
//! path>`. The texture is painted here, in memory, from the player's own ROM texture and the pack's pattern, and no ROM path or
//! pixel is in the pack. `keep` names circles of the base texture that stay unpainted, such as a scope's lens.
// DC-DEFAULT: VAR-3 (proposed; USER DECISION 2026-11-02, gen1recomp's rule)

use crate::hud::compose::Tex;
use dc_bute::modpatch::{VariantInfo, WeaponContent};
use dc_render::{HandModel, HandModels, TextureFx, TextureImage, ViewModel, ViewModels};
use std::collections::BTreeMap;
use std::sync::RwLock;

/// The scheme of a painted skin.
pub const SCHEME: &str = "@camo:";

/// What comes before the circles a camo leaves unpainted.
const KEEP_PREFIX: &str = "keep=";

/// How many texels the edge of a kept circle takes to fade into the paint.
const KEEP_EDGE: f32 = 2.0;

/// What comes before a camo's effect: `fx=<drift u>,<drift v>,<lowest gain>,<highest gain>,<seconds per pulse>`.
const FX_PREFIX: &str = "fx=";

/// The shortest pulse an effect may have, so that nothing strobes (VAR-4).
const FX_MIN_PULSE_SECONDS: f32 = 1.0;

/// The fastest an effect's pattern may drift, in repeats per second (VAR-4).
const FX_MAX_DRIFT: f32 = 1.0;

/// The largest side of an effect's mask: it only has to follow the texture's parts, so it stays small.
const FX_MASK_SIZE: usize = 256;

/// A camo's effect: its pattern drifts and pulses over the gun as light (VAR-4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CamoFx {
    pub speed: [f32; 2],
    pub gain: [f32; 2],
    pub seconds: f32,
}

/// A round part of the base texture that keeps its own look, in texels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}

/// The scheme of a shop picture painted for a variant: `@camoart:<pattern path>|<the base gun's picture>`. The client makes these
/// itself from what a variant's skins name, and a pack never writes one.
const ART_SCHEME: &str = "@camoart:";

/// The painted form of `picture`, with the pattern of variant `v` (`None`: its skins name no pattern).
pub fn art_path(v: &VariantInfo, picture: &str) -> Option<String> {
    let skin = v.pv_skins.values().chain(v.hh_skins.values());
    let c = skin.filter_map(|s| parse(s)).next()?;
    // a still picture shows what the camo is known by: the effect's pattern, when it has its own
    let pattern = c.fx_pattern.unwrap_or(c.pattern);
    Some(format!("{ART_SCHEME}{pattern}|{picture}"))
}

/// A painted picture's pattern path and base picture.
pub fn parse_art(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix(ART_SCHEME)?.split_once('|')
}

/// Paints a menu picture: the same recipe as a gun's texture, over the picture's own pixels and alpha.
pub fn paint_picture(base: &Tex, pattern: &Tex) -> Option<Tex> {
    let image = |t: &Tex| TextureImage::rgba(t.width, t.height, t.rgba.clone());
    let painted = paint(&image(base), &image(pattern), &[])?;
    Some(Tex {
        width: painted.width,
        height: painted.height,
        rgba: painted.levels.into_iter().next()?,
    })
}

/// A parsed `@camo:` path.
#[derive(Debug, Clone, PartialEq)]
pub struct CamoPath<'a> {
    pub mode: &'a str,
    pub base_row: i32,
    pub base_key: &'a str,
    pub keep: Vec<Circle>,
    pub fx: Option<CamoFx>,
    pub pattern: &'a str,
    /// The pattern the effect moves, when it is not the painted one (`<pattern>|<effect pattern>`).
    pub fx_pattern: Option<&'a str>,
}

/// Parses a `@camo:` path (`None`: not one, or malformed).
pub fn parse(name: &str) -> Option<CamoPath<'_>> {
    let mut parts = name.strip_prefix(SCHEME)?.splitn(4, ':');
    let mode = parts.next()?;
    let base_row = parts.next()?.parse().ok()?;
    let base_key = parts.next()?;
    let mut pattern = parts.next()?;
    let mut keep = Vec::new();
    if let Some(rest) = pattern.strip_prefix(KEEP_PREFIX) {
        let (circles, after) = rest.split_once(':')?;
        keep = circles
            .split('/')
            .map(parse_circle)
            .collect::<Option<_>>()?;
        pattern = after;
    }
    let mut fx = None;
    if let Some(rest) = pattern.strip_prefix(FX_PREFIX) {
        let (text, after) = rest.split_once(':')?;
        fx = Some(parse_fx(text)?);
        pattern = after;
    }
    let (pattern, fx_pattern) = match pattern.split_once('|') {
        Some((painted, moved)) if !moved.is_empty() => (painted, Some(moved)),
        Some(_) => return None,
        None => (pattern, None),
    };
    (dc_bute::modpatch::is_cosmetic_key(base_key) && !pattern.is_empty()).then_some(CamoPath {
        mode,
        base_row,
        base_key,
        keep,
        fx,
        pattern,
        fx_pattern,
    })
}

/// `drift u,drift v,lowest gain,highest gain,seconds`, held to the limits of VAR-4. A pulse needs its seconds; 0 is a steady gain.
fn parse_fx(text: &str) -> Option<CamoFx> {
    let v = text
        .split(',')
        .map(|t| t.parse::<f32>().ok().filter(|v| v.is_finite()))
        .collect::<Option<Vec<f32>>>()?;
    let [su, sv, low, high, seconds] = v.as_slice() else {
        return None;
    };
    let high = high.clamp(0.0, 1.0);
    Some(CamoFx {
        speed: [su, sv].map(|s| s.clamp(-FX_MAX_DRIFT, FX_MAX_DRIFT)),
        gain: [low.clamp(0.0, high), high],
        seconds: match *seconds {
            s if s > 0.0 => s.max(FX_MIN_PULSE_SECONDS),
            _ => 0.0,
        },
    })
}

/// The effect of a painted texture: the pattern with its smaller sizes, and a mask that leaves out the base's unused space and
/// its kept circles, so that no light lands on a scope's lens.
pub fn fx_of(
    base: &TextureImage,
    pattern: &TextureImage,
    keep: &[Circle],
    fx: CamoFx,
) -> Option<TextureFx> {
    let b = base.levels.first()?;
    let (w, h) = (base.width as usize, base.height as usize);
    let (mw, mh) = (w.min(FX_MASK_SIZE).max(1), h.min(FX_MASK_SIZE).max(1));
    let mut mask = vec![255u8; mw * mh * 4];
    for my in 0..mh {
        for mx in 0..mw {
            let x = ((mx * w + w / 2) / mw).min(w.saturating_sub(1));
            let y = ((my * h + h / 2) / mh).min(h.saturating_sub(1));
            let i = (y * w + x) * 4;
            let drawn = b.get(i..i + 4).is_some_and(|px| luma(px) > UNUSED_LUMA);
            let amount = paint_amount(keep, x as f32 + 0.5, y as f32 + 0.5);
            let v = if drawn {
                (amount * 255.0).round() as u8
            } else {
                0
            };
            mask[(my * mw + mx) * 4..][..3].fill(v);
        }
    }
    Some(TextureFx {
        tile: with_smaller_sizes(pattern)?,
        mask: TextureImage::rgba(mw as u32, mh as u32, mask),
        tiles: PATTERN_TILES,
        speed: fx.speed,
        gain: fx.gain,
        seconds: fx.seconds,
    })
}

/// A copy of `image` with its halved sizes down to a few texels, so a far gun's effect does not shimmer.
fn with_smaller_sizes(image: &TextureImage) -> Option<TextureImage> {
    let mut levels = vec![image.levels.first()?.clone()];
    let (mut w, mut h) = (image.width as usize, image.height as usize);
    while w > 4 && h > 4 {
        let last = levels.last()?;
        let (nw, nh) = (w / 2, h / 2);
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at =
                        |dx: usize, dy: usize| last[((y * 2 + dy) * w + x * 2 + dx) * 4 + c] as u32;
                    next[(y * nw + x) * 4 + c] =
                        ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                }
            }
        }
        levels.push(next);
        (w, h) = (nw, nh);
    }
    let mut out = TextureImage::rgba(image.width, image.height, Vec::new());
    out.levels = levels;
    Some(out)
}

/// `x,y,radius` in texels of the base texture.
fn parse_circle(text: &str) -> Option<Circle> {
    let mut n = text
        .split(',')
        .map(|v| v.parse::<f32>().ok().filter(|v| v.is_finite()));
    let circle = Circle {
        x: n.next()??,
        y: n.next()??,
        radius: n.next()??,
    };
    (n.next().is_none() && circle.radius > 0.0).then_some(circle)
}

/// How bright a texel is, 0..1.
fn luma(px: &[u8]) -> f32 {
    (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32) / 255.0
}

/// The darkest and lightest the base's shading may make a camo: a shaded part stays readable, a scratch stays bright.
const SHADE_RANGE: (f32, f32) = (0.3, 1.5);

/// How many times the pattern repeats across the base's width, whatever its size: the 1024 first-person texture and the 256
/// third-person one show the same blobs on the gun.
const PATTERN_TILES: f32 = 2.0;

/// The most pattern samples per axis when the base is much smaller than the pattern.
const MAX_PATTERN_SAMPLES: usize = 8;

/// A texel this dark is the texture's unused space, not part of the gun, so it stays out of the mean the shading compares with.
const UNUSED_LUMA: f32 = 0.04;

/// Paints `pattern`, repeated over `base`, keeping the base's shading and alpha: each texel is the pattern's colour scaled
/// by how bright the base is there against its own average, so seams, screws and wear still show through a bright colour
/// however dark the gun's own paint is. The result is one level.
pub fn paint(base: &TextureImage, pattern: &TextureImage, keep: &[Circle]) -> Option<TextureImage> {
    let (b, p) = (base.levels.first()?, pattern.levels.first()?);
    let (w, h, pw, ph) = (
        base.width as usize,
        base.height as usize,
        pattern.width as usize,
        pattern.height as usize,
    );
    if w == 0 || h == 0 || pw == 0 || ph == 0 || b.len() < w * h * 4 || p.len() < pw * ph * 4 {
        return None;
    }
    let lit = b.chunks_exact(4).map(luma).filter(|l| *l > UNUSED_LUMA);
    let (sum, count) = lit.fold((0.0_f32, 0_u32), |(sum, count), l| (sum + l, count + 1));
    let mean = (sum / count.max(1) as f32).max(0.02);
    let step = pw as f32 * PATTERN_TILES / w as f32;
    let samples = (step.ceil() as usize).clamp(1, MAX_PATTERN_SAMPLES);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let colour = pattern_colour(p, (pw, ph), (x, y), step, samples);
            let shade = (luma(&b[i..i + 4]) / mean).clamp(SHADE_RANGE.0, SHADE_RANGE.1);
            let painted = paint_amount(keep, x as f32 + 0.5, y as f32 + 0.5);
            for c in 0..3 {
                let camo = (colour[c] * shade).min(255.0);
                out[i + c] = (b[i + c] as f32 + (camo - b[i + c] as f32) * painted).round() as u8;
            }
            out[i + 3] = b[i + 3];
        }
    }
    let mut img = TextureImage::rgba(base.width, base.height, out);
    img.alpha_cutoff = base.alpha_cutoff;
    img.mip_bias = base.mip_bias;
    Some(img)
}

/// The pattern's average colour over one texel of the base, `step` pattern texels wide.
fn pattern_colour(
    p: &[u8],
    (pw, ph): (usize, usize),
    (x, y): (usize, usize),
    step: f32,
    samples: usize,
) -> [f32; 3] {
    let at = |texel: usize, k: usize| {
        ((texel as f32 + (k as f32 + 0.5) / samples as f32) * step) as usize
    };
    let mut sum = [0.0_f32; 3];
    for sy in 0..samples {
        for sx in 0..samples {
            let j = (at(y, sy) % ph * pw + at(x, sx) % pw) * 4;
            for c in 0..3 {
                sum[c] += p[j + c] as f32;
            }
        }
    }
    sum.map(|v| v / (samples * samples) as f32)
}

/// How much paint a texel takes: none inside a kept circle, all of it outside, with a soft edge between.
fn paint_amount(keep: &[Circle], x: f32, y: f32) -> f32 {
    keep.iter().fold(1.0_f32, |amount, c| {
        let distance = (x - c.x).hypot(y - c.y) - c.radius;
        amount.min((distance / KEEP_EDGE).clamp(0.0, 1.0))
    })
}

static VARIANTS: RwLock<BTreeMap<i32, VariantInfo>> = RwLock::new(BTreeMap::new());

/// The variants of the loaded mod set (none: `None` or a document without any).
pub fn set_content(content: Option<&WeaponContent>) {
    let map = content
        .map(|c| c.variants().into_iter().map(|v| (v.row, v)).collect())
        .unwrap_or_default();
    *VARIANTS.write().unwrap_or_else(|e| e.into_inner()) = map;
}

/// What another player's gun of row `n` draws (`table`: the ROM's rows, which hold no variant): a variant is its base gun
/// with the skins it sets.
pub fn hand_model(table: &HandModels, n: u32) -> Option<HandModel> {
    hand_model_of(table, n, variant(n as i32).as_ref())
}

fn hand_model_of(table: &HandModels, n: u32, variant: Option<&VariantInfo>) -> Option<HandModel> {
    let Some(v) = variant else {
        return table.rows.get(&n).cloned();
    };
    let worn = v.look_of.unwrap_or(v.base);
    let mut model = table.rows.get(&u32::try_from(worn).ok()?)?.clone();
    set_skins(&mut model.skins, &v.hh_skins);
    Some(model)
}

/// What a player's own gun of row `n` draws in first person (`table`: the ROM's rows, which hold no variant): a variant is its base
/// gun's model for this hand identifier, so its clips and hands stay right, with the skins it sets.
pub fn view_model(table: &ViewModels, n: u32, ident: Option<&str>) -> Option<ViewModel> {
    let Some(v) = variant(n as i32) else {
        return table.pick(n, ident);
    };
    let worn = v.look_of.unwrap_or(v.base);
    let mut model = table.pick(u32::try_from(worn).ok()?, ident)?;
    set_skins(&mut model.skins, &v.pv_skins);
    Some(model)
}

/// The row a ROM-only table (sounds, muzzle effects) knows weapon `n` by: its base gun when it is a variant.
pub fn base_weapon(n: u16) -> u16 {
    variant(i32::from(n)).map_or(n, |v| v.base as u16)
}

fn set_skins(skins: &mut Vec<String>, set: &BTreeMap<usize, String>) {
    for (&k, path) in set {
        if skins.len() <= k {
            skins.resize(k + 1, String::new());
        }
        skins[k] = path.clone();
    }
}

/// The loaded variant of row `n`.
pub fn variant(n: i32) -> Option<VariantInfo> {
    VARIANTS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(&n)
        .cloned()
}

/// The row whose name, icon and type item `n` uses: its base gun when it is a variant, else itself.
pub fn art_row(n: i32) -> i32 {
    variant(n).map_or(n, |v| v.base)
}

/// Every loaded variant.
pub fn variants() -> Vec<VariantInfo> {
    VARIANTS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_players_variant_draws_its_base_gun_with_its_own_skin() {
        let mut table = HandModels::default();
        let base = HandModel {
            model: "gun.ltb".into(),
            skins: vec!["base.dtx".into()],
            styles: vec!["style".into()],
        };
        table.rows.insert(25, base.clone());
        let variant = VariantInfo {
            row: 500,
            base: 25,
            name: None,
            icon: None,
            hh_skins: [(0, "@camo:x".to_string())].into_iter().collect(),
            pv_skins: Default::default(),
            look_of: None,
        };
        let drawn = hand_model_of(&table, 500, Some(&variant)).unwrap();
        assert_eq!(drawn.model, "gun.ltb");
        assert_eq!(drawn.styles, base.styles);
        assert_eq!(drawn.skins, vec!["@camo:x".to_string()]);
        assert_eq!(hand_model_of(&table, 25, None), Some(base));
        assert_eq!(hand_model_of(&table, 500, None), None);
    }

    #[test]
    fn an_effect_is_held_to_its_limits() {
        let c =
            parse("@camo:paint:25:PVSkin1:keep=1,2,3:fx=0.03,-0.01,0.2,0.8,2:@mod:p/assets/c.dtx")
                .unwrap();
        assert_eq!(c.keep.len(), 1);
        assert_eq!(c.pattern, "@mod:p/assets/c.dtx");
        assert_eq!(
            c.fx,
            Some(CamoFx {
                speed: [0.03, -0.01],
                gain: [0.2, 0.8],
                seconds: 2.0
            })
        );
        let wild = parse("@camo:paint:25:PVSkin1:fx=9,-9,5,3,0.01:a.dtx")
            .unwrap()
            .fx
            .unwrap();
        assert_eq!(
            wild.speed,
            [1.0, -1.0],
            "it may not drift faster than the limit"
        );
        assert_eq!(wild.gain, [1.0, 1.0], "nor be brighter");
        assert_eq!(wild.seconds, 1.0, "nor strobe");
        let steady = parse("@camo:paint:25:PVSkin1:fx=0,0,0,0.5,0:a.dtx")
            .unwrap()
            .fx
            .unwrap();
        assert_eq!(steady.seconds, 0.0);
        assert!(parse("@camo:paint:25:PVSkin1:fx=1,2,3:a.dtx").is_none());
        assert!(parse("@camo:paint:25:PVSkin1:fx=1,2,3,x,5:a.dtx").is_none());
        assert!(parse("@camo:paint:25:PVSkin1:a.dtx").unwrap().fx.is_none());
        let two =
            parse("@camo:paint:25:PVSkin1:fx=0,0,0,1,0:@mod:p/assets/a.dtx|@mod:p/assets/b.dtx")
                .unwrap();
        assert_eq!(two.pattern, "@mod:p/assets/a.dtx");
        assert_eq!(two.fx_pattern, Some("@mod:p/assets/b.dtx"));
        assert!(parse("@camo:paint:25:PVSkin1:a.dtx|").is_none());
    }

    #[test]
    fn an_effects_mask_leaves_out_unused_space_and_kept_circles() {
        let mut b = [[100, 100, 100, 255]; 4].concat();
        b[8..12].copy_from_slice(&[0, 0, 0, 255]);
        let base = TextureImage::rgba(4, 1, b);
        let pattern = TextureImage::rgba(16, 16, vec![9; 16 * 16 * 4]);
        let keep = [Circle {
            x: 0.5,
            y: 0.5,
            radius: 0.5,
        }];
        let fx = CamoFx {
            speed: [0.0, 0.0],
            gain: [0.0, 1.0],
            seconds: 0.0,
        };
        let out = fx_of(&base, &pattern, &keep, fx).unwrap();
        let red: Vec<u8> = out.mask.levels[0].chunks_exact(4).map(|p| p[0]).collect();
        assert_eq!(
            red,
            vec![0, 64, 0, 255],
            "kept, its soft edge, unused, painted"
        );
        let sizes: Vec<usize> = out.tile.levels.iter().map(|l| l.len()).collect();
        assert_eq!(sizes, vec![1024, 256, 64]);
    }

    #[test]
    fn a_shop_picture_is_painted_from_the_variants_pattern() {
        let variant = VariantInfo {
            row: 500,
            base: 25,
            name: None,
            icon: None,
            hh_skins: Default::default(),
            pv_skins: [(
                1,
                "@camo:paint:25:PVSkin1:keep=1,2,3:@mod:p/assets/c.dtx".to_string(),
            )]
            .into_iter()
            .collect(),
            look_of: None,
        };
        let path = art_path(&variant, "UI\\gun.dtx").unwrap();
        assert_eq!(
            parse_art(&path),
            Some(("@mod:p/assets/c.dtx", "UI\\gun.dtx"))
        );
        assert_eq!(parse_art("UI\\gun.dtx"), None);

        let base = Tex {
            width: 2,
            height: 1,
            rgba: vec![90, 90, 90, 255, 90, 90, 90, 0],
        };
        let pattern = Tex {
            width: 1,
            height: 1,
            rgba: vec![200, 0, 0, 255],
        };
        let out = paint_picture(&base, &pattern).unwrap();
        assert_eq!(
            out.rgba,
            vec![200, 0, 0, 255, 200, 0, 0, 0],
            "colour from the pattern, alpha from the picture"
        );
    }

    #[test]
    fn a_camo_path_may_name_circles_to_keep() {
        let c = parse("@camo:paint:25:PVSkin1:keep=928,928,90/10,20,5:@mod:pack/assets/candy.dtx")
            .unwrap();
        assert_eq!(c.pattern, "@mod:pack/assets/candy.dtx");
        assert_eq!(
            c.keep,
            vec![
                Circle {
                    x: 928.0,
                    y: 928.0,
                    radius: 90.0
                },
                Circle {
                    x: 10.0,
                    y: 20.0,
                    radius: 5.0
                },
            ]
        );
        for bad in [
            "@camo:paint:25:PVSkin1:keep=1,2:a.dtx",
            "@camo:paint:25:PVSkin1:keep=1,2,3,4:a.dtx",
            "@camo:paint:25:PVSkin1:keep=1,2,0:a.dtx",
            "@camo:paint:25:PVSkin1:keep=1,2,x:a.dtx",
            "@camo:paint:25:PVSkin1:keep=1,2,3",
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_kept_circle_stays_as_the_base_drew_it() {
        let base = TextureImage::rgba(9, 1, [[40, 50, 60, 200]; 9].concat());
        let pattern = TextureImage::rgba(1, 1, vec![200, 0, 0, 255]);
        let keep = [Circle {
            x: 0.5,
            y: 0.5,
            radius: 2.0,
        }];
        let out = paint(&base, &pattern, &keep).unwrap();
        let px = &out.levels[0];
        assert_eq!(&px[0..4], &[40, 50, 60, 200], "inside the circle");
        assert_eq!(&px[8 * 4..8 * 4 + 4], &[200, 0, 0, 200], "outside it");
    }

    #[test]
    fn a_camo_path_names_its_parts() {
        let c = parse("@camo:paint:25:PVSkin1:@mod:pack/assets/candy.dtx").unwrap();
        assert_eq!(
            (c.mode, c.base_row, c.base_key, c.pattern),
            ("paint", 25, "PVSkin1", "@mod:pack/assets/candy.dtx")
        );
        for bad in [
            "PV_G36E.dtx",
            "@camo:paint:x:PVSkin1:a.dtx",
            "@camo:paint:25:InstDamage:a.dtx",
            "@camo:paint:25:PVSkin1:",
            "@camo:paint:25",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn paint_keeps_the_bases_shading_and_alpha_and_repeats_the_pattern() {
        // a base half dark, half light, with a clear corner
        let mut b = Vec::new();
        for i in 0..4u8 {
            let v = if i < 2 { 40 } else { 120 };
            b.extend_from_slice(&[v, v, v, if i == 3 { 0 } else { 255 }]);
        }
        let base = TextureImage::rgba(4, 1, b);
        let pattern = TextureImage::rgba(2, 1, vec![200, 0, 0, 255, 0, 200, 0, 255]);
        let out = paint(&base, &pattern, &[]).unwrap();
        assert_eq!((out.width, out.height), (4, 1));
        let px = &out.levels[0];
        assert!(px[0] > 0 && px[1] == 0, "the first colour");
        assert!(
            px[5] > 0 && px[4] == 0,
            "the second colour, repeated from here on"
        );
        assert!(px[8] > px[0], "a lighter part of the base is lighter");
        assert_eq!((px[3], px[15]), (255, 0), "alpha comes from the base");
    }

    #[test]
    fn a_small_base_shows_the_same_tiles_as_a_large_one() {
        let pattern = TextureImage::rgba(
            4,
            1,
            [
                [255, 0, 0, 255],
                [255, 0, 0, 255],
                [0, 0, 255, 255],
                [0, 0, 255, 255],
            ]
            .concat(),
        );
        // eight pattern texels across a two-texel base: each texel averages half a tile's width
        let base = TextureImage::rgba(2, 1, [[50, 50, 50, 255]; 2].concat());
        let px = paint(&base, &pattern, &[]).unwrap().levels.remove(0);
        assert_eq!((px[0], px[2]), (128, 128));
        assert_eq!((px[4], px[6]), (128, 128));
    }

    #[test]
    fn unused_black_space_does_not_brighten_the_gun() {
        let mut b = [[0, 0, 0, 255]; 7].concat();
        b.extend([100, 100, 100, 255]);
        let base = TextureImage::rgba(8, 1, b);
        let pattern = TextureImage::rgba(1, 1, vec![100, 100, 100, 255]);
        let px = paint(&base, &pattern, &[]).unwrap().levels.remove(0);
        assert_eq!(
            px[7 * 4],
            100,
            "the only drawn texel is at the mean, so it keeps the pattern's colour"
        );
    }

    #[test]
    fn nothing_paints_from_an_empty_image() {
        let ok = TextureImage::rgba(1, 1, vec![1, 2, 3, 4]);
        assert!(paint(&TextureImage::rgba(0, 0, Vec::new()), &ok, &[]).is_none());
        assert!(paint(&ok, &TextureImage::rgba(0, 0, Vec::new()), &[]).is_none());
    }
}

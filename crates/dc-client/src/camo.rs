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
pub use dc_render::camo::{fx_of, paint, parse, CamoFx, CamoPath, Circle, SCHEME};
use dc_render::{HandModel, HandModels, TextureImage, ViewModel, ViewModels};
use std::collections::BTreeMap;
use std::sync::RwLock;

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
}

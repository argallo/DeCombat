//! The Camo Crate (CRATE-1..CRATE-4): a DeCombat supply case that holds the community camos. It drops in matches on an
//! entry whose curated set has weapon variants, lands in the inbox like a picked-up HiSec case and opens without a key into one
//! of that entry's variants.
//!
//! Like the seasonal rows ([`crate::seasonal`]), its catalogue row and product live here, not in the ROM, and both the lobby
//! and the client add them to the catalogue they read. The product is hidden (`Shopview` 0) and never listed, so nobody can
//! buy one. The crate shows the pictures of a ROM supply case ([`LOOK_OF`], by item number, read at run time); its name is
//! our own. Nothing here is text or data from the game.
// DC-DEFAULT: CRATE-1 (proposed; USER REQUEST 2026-11-05 "another supply crate type that can drop that gives you one of the
// community mod camo guns")

use crate::modpatch::Rarity;
use crate::{ShopItemRow, ShopProduct};

/// The crate's item number: a supply case (8000..8999, client type 0x14) above every case of the ROM (8000..8058 in the EU
/// ROM).
pub const ITEM: i32 = 8900;

/// The crate's product id, above every id of the ROM's catalogue and apart from the seasonal ids (60,000..).
pub const PRODUCT: i32 = 61_000;

/// The crate's catalogue row index.
pub const ROW: i32 = 61_000;

/// The crate's name in the menus (our own words).
pub const NAME: &str = "Camo Crate";

/// The ROM supply case whose slot pictures the crate shows: a plain event case (no key class), so the crate looks like a
/// case but no shop case of the ROM.
pub const LOOK_OF: i32 = 8037;

/// The catalogue item type of a supply case.
const CLASS_CASE: i32 = 20;

/// The product's consume type: a supply case (CD-72).
const CONSUME_CASE: i32 = 2;

/// The bit a Camo Crate's case key carries in a match (CRATE-2): the game server sets it, so a client tells a crate from a
/// HiSec case by the key alone (the key counter stays far below it).
pub const CAMO_KEY_BIT: u32 = 0x4000_0000;

/// True for a case key of a Camo Crate.
pub fn is_camo_key(key: u32) -> bool {
    key & CAMO_KEY_BIT != 0
}

/// True for the crate's item number.
pub fn is_camo_crate(item_no: i64) -> bool {
    item_no == i64::from(ITEM)
}

/// The crate's catalogue row and product.
pub(crate) fn row() -> (ShopItemRow, ShopProduct) {
    let row = ShopItemRow {
        index: ROW,
        itemid: ITEM,
        class_code: CLASS_CASE,
        new_item: 0,
        name: NAME.to_string(),
        gp_display: 0,
        nx_display: 0,
        gp_unlock: 0,
        nx_unlock: 0,
    };
    let product = ShopProduct {
        id: PRODUCT,
        sale_type: 0,
        view: 0,
        price: 0,
        sale_price: 0,
        bonus: 0,
        name: NAME.to_string(),
        items: vec![(ROW, CONSUME_CASE, 1)],
        period: 1,
        consume: CONSUME_CASE,
    };
    (row, product)
}

/// A tier of what a Camo Crate holds, lowest first (CRATE-10): the regular guns (the plain base gun of each camo), then
/// the camos by their rarity (CRATE-6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Regular,
    Camo(Rarity),
}

impl Tier {
    /// Every tier, lowest first.
    pub const ALL: [Tier; 5] = [
        Tier::Regular,
        Tier::Camo(Rarity::Common),
        Tier::Camo(Rarity::Rare),
        Tier::Camo(Rarity::Epic),
        Tier::Camo(Rarity::Legendary),
    ];

    /// 0 for the regular guns up to 4 for the Legendary camos.
    pub fn index(self) -> usize {
        match self {
            Tier::Regular => 0,
            Tier::Camo(r) => 1 + r.index(),
        }
    }

    /// The name players see (our own words).
    pub fn name(self) -> &'static str {
        match self {
            Tier::Regular => "Regular gun",
            Tier::Camo(Rarity::Common) => "Common camo",
            Tier::Camo(Rarity::Rare) => "Rare camo",
            Tier::Camo(Rarity::Epic) => "Epic camo",
            Tier::Camo(Rarity::Legendary) => "Legendary camo",
        }
    }
}

/// Each tier's share of the openings in percent, by [`Tier::index`]. A tier the crate holds nothing of gives its share to
/// the others in proportion (see [`tier_odds`]).
// DC-DEFAULT: CRATE-10 (proposed; USER DECISION 2026-11-06 "70 / 18 / 8 / 3 / 1") regular 70, Common 18, Rare 8, Epic 3,
// Legendary 1
pub const TIER_PERCENT: [u32; 5] = [70, 18, 8, 3, 1];

/// One thing a Camo Crate can give: an item number (a weapon row) and its tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Content {
    pub item: i32,
    pub tier: Tier,
}

/// What a Camo Crate holds, from the weapon variants of an entry as `(variant row, base row, rarity)`: every camo, and the
/// base gun of each camo once as a regular gun (a base that is itself one of the camos stays only a camo). Sorted by
/// tier, then item. A new camo pack on the entry adds its camo and its plain gun, with nothing else to edit.
// DC-DEFAULT: CRATE-10 (proposed; USER DECISION 2026-11-06 "base guns of the camos")
pub fn contents(variants: impl IntoIterator<Item = (i32, i32, Rarity)>) -> Vec<Content> {
    let variants: Vec<(i32, i32, Rarity)> = variants.into_iter().collect();
    let mut out: Vec<Content> = variants
        .iter()
        .map(|&(row, _, r)| Content {
            item: row,
            tier: Tier::Camo(r),
        })
        .collect();
    for &(_, base, _) in &variants {
        if !variants.iter().any(|v| v.0 == base) {
            out.push(Content {
                item: base,
                tier: Tier::Regular,
            });
        }
    }
    out.sort_by_key(|c| (c.tier, c.item));
    out.dedup_by_key(|c| c.item);
    out
}

/// How many items of each tier `contents` holds, by [`Tier::index`].
fn counts(contents: &[Content]) -> [u32; 5] {
    let mut n = [0u32; 5];
    for c in contents {
        n[c.tier.index()] += 1;
    }
    n
}

/// The [`TIER_PERCENT`] weights of the tiers `contents` holds something of (0 for an empty tier).
fn live_weights(contents: &[Content]) -> [u32; 5] {
    let n = counts(contents);
    std::array::from_fn(|t| if n[t] > 0 { TIER_PERCENT[t] } else { 0 })
}

/// Each tier's chance in percent, by [`Tier::index`]: the shares of the tiers that hold something, scaled to 100 (an
/// empty tier 0). All zero for an empty crate.
// DC-DEFAULT: CRATE-10 (proposed) an empty tier's share goes to the others in proportion
pub fn tier_odds(contents: &[Content]) -> [f64; 5] {
    let w = live_weights(contents);
    let total: u32 = w.iter().sum();
    std::array::from_fn(|t| {
        if total == 0 {
            0.0
        } else {
            f64::from(w[t]) * 100.0 / f64::from(total)
        }
    })
}

/// The chance in percent of item `item`: its tier's chance shared evenly among the tier's items (0 when the crate does
/// not hold it).
pub fn item_odds(contents: &[Content], item: i32) -> f64 {
    let Some(c) = contents.iter().find(|c| c.item == item) else {
        return 0.0;
    };
    let t = c.tier.index();
    tier_odds(contents)[t] / f64::from(counts(contents)[t].max(1))
}

/// Draws an award from `contents`: `tier_roll` picks the tier by [`tier_odds`], `item_roll` one of its items, each as
/// likely. Every player has the same odds (no boost for a camo one does not own, CRATE-6 until 2026-11-06).
// DC-DEFAULT: CRATE-10 (proposed; USER DECISION 2026-11-06 "drop the boost")
pub fn draw(contents: &[Content], tier_roll: u32, item_roll: u32) -> Option<Content> {
    let w = live_weights(contents);
    let total: u32 = w.iter().sum();
    if total == 0 {
        return None;
    }
    let mut at = tier_roll % total;
    let tier = Tier::ALL.into_iter().find(|t| {
        let wt = w[t.index()];
        if at < wt {
            true
        } else {
            at -= wt;
            false
        }
    })?;
    let of_tier: Vec<&Content> = contents.iter().filter(|c| c.tier == tier).collect();
    of_tier.get(item_roll as usize % of_tier.len()).map(|c| **c)
}

/// The cells a case reel fills its strip from for a Camo Crate (CRATE-5): each item repeated by its chance in steps of
/// half a percent (at least once), so the strip shows mostly regular guns and a Legendary only now and then, as the
/// odds are.
// DC-DEFAULT: CRATE-10 (proposed) the strip follows the odds
pub fn reel_mix(contents: &[Content]) -> Vec<i32> {
    let mut out = Vec::new();
    for c in contents {
        let n = (item_odds(contents, c.item) * 2.0).round().max(1.0) as usize;
        out.extend(std::iter::repeat_n(c.item, n));
    }
    out
}

/// `p` percent as players read it: whole numbers bare ("70%"), else one decimal ("4.1%"), below 1 two ("0.45%").
pub fn percent_text(p: f64) -> String {
    if (p - p.round()).abs() < 0.05 {
        format!("{:.0}%", p.round())
    } else if p >= 1.0 {
        format!("{p:.1}%")
    } else {
        format!("{p:.2}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crate_is_a_hidden_free_supply_case_of_its_own() {
        let (row, p) = row();
        assert_eq!((row.itemid, row.class_code), (ITEM, CLASS_CASE));
        assert_eq!(
            (p.id, p.view, p.price, p.consume),
            (PRODUCT, 0, 0, CONSUME_CASE)
        );
        assert_eq!(p.items, vec![(ROW, CONSUME_CASE, 1)]);
        assert!((8000..9000).contains(&ITEM));
        // apart from every seasonal product and row
        for (r, products) in crate::seasonal::rows() {
            assert_ne!(r.index, ROW);
            assert!(products.iter().all(|s| s.id < PRODUCT));
        }
        assert!(is_camo_crate(8900) && !is_camo_crate(8037));
    }
    fn content(item: i32, tier: Tier) -> Content {
        Content { item, tier }
    }

    #[test]
    fn the_crate_holds_each_camo_and_its_plain_gun_once() {
        use Rarity::*;
        // two camos of row 25, one of row 27, and a camo of a camo (row 504 of 502): 502 is a camo only
        let c = contents([
            (502, 25, Epic),
            (505, 25, Common),
            (503, 27, Legendary),
            (504, 502, Rare),
        ]);
        assert_eq!(
            c,
            vec![
                content(25, Tier::Regular),
                content(27, Tier::Regular),
                content(505, Tier::Camo(Common)),
                content(504, Tier::Camo(Rare)),
                content(502, Tier::Camo(Epic)),
                content(503, Tier::Camo(Legendary)),
            ]
        );
        assert!(contents([]).is_empty());
    }

    #[test]
    fn the_odds_are_the_tier_shares_and_an_empty_tier_gives_its_share_away() {
        use Rarity::*;
        let full = contents([
            (501, 25, Common),
            (502, 25, Rare),
            (503, 27, Epic),
            (504, 27, Legendary),
        ]);
        assert_eq!(tier_odds(&full), [70.0, 18.0, 8.0, 3.0, 1.0]);
        // two regular guns share 70
        assert_eq!(item_odds(&full, 25), 35.0);
        assert_eq!(item_odds(&full, 504), 1.0);
        assert_eq!(item_odds(&full, 999), 0.0);
        // only Epic and Legendary camos (the two example packs): 70 : 3 : 1 of 74
        let two = contents([(502, 25, Epic), (503, 27, Legendary)]);
        let o = tier_odds(&two);
        assert_eq!((o[1], o[2]), (0.0, 0.0));
        assert!((o.iter().sum::<f64>() - 100.0).abs() < 1e-9);
        assert!((o[0] - 7000.0 / 74.0).abs() < 1e-9);
        assert_eq!(tier_odds(&[]), [0.0; 5]);
    }

    #[test]
    fn the_draw_follows_the_odds_exactly() {
        use Rarity::*;
        let c = contents([
            (501, 25, Common),
            (502, 25, Rare),
            (503, 27, Epic),
            (504, 27, Legendary),
        ]);
        let mut n = std::collections::BTreeMap::new();
        for tier_roll in 0..100 {
            for item_roll in 0..2 {
                *n.entry(draw(&c, tier_roll, item_roll).unwrap().item)
                    .or_insert(0) += 1;
            }
        }
        // of 200 draws: each regular gun 70, then 36 / 16 / 6 / 2
        assert_eq!(
            n.into_iter().collect::<Vec<_>>(),
            vec![(25, 70), (27, 70), (501, 36), (502, 16), (503, 6), (504, 2)]
        );
        assert!(draw(&c, u32::MAX, u32::MAX).is_some());
        assert_eq!(draw(&[], 1, 1), None);
    }

    #[test]
    fn the_reel_mix_follows_the_odds_and_keeps_every_item() {
        use Rarity::*;
        let c = contents([(501, 25, Common), (504, 27, Legendary)]);
        let mix = reel_mix(&c);
        let of = |n: i32| mix.iter().filter(|&&m| m == n).count();
        // regular 70 of 89 (78.7 % each 39.3): 79 cells; Common 18 of 89 (20.2 %): 40; Legendary 1 of 89 (1.1 %): 2
        assert_eq!((of(25), of(27), of(501), of(504)), (79, 79, 40, 2));
        assert_eq!(percent_text(70.0), "70%");
        assert_eq!(percent_text(7000.0 / 74.0), "94.6%");
        assert_eq!(percent_text(0.45), "0.45%");
        assert_eq!(percent_text(1.0), "1%");
    }
}

//! Seasonal GP products (SHOP-2): items the 2009 catalogue hides or sells for NX only, on sale for GP in a season of the year.
//!
//! The product rows live here, not in the ROM, and both the lobby and the client add them to the catalogue they read
//! ([`add_to`]). They are hidden rows (`Shopview` 0) like the ROM's: only the lobby's list says one is on sale, so an operator
//! who leaves `shop.seasonal` off sells nothing new, and an older client drops an id it does not know. This table holds item
//! numbers and our own prices, and no text or data from the game.
// DC-DEFAULT: SHOP-2 (proposed; USER DECISION 2026-11-03) the seasons, items and prices are ours; nothing here is 2009 behaviour

use crate::{ShopItemRow, ShopProduct};

/// The first product id of the table, above every id of the ROM's catalogue.
pub const FIRST_PRODUCT: i32 = 60_000;

/// The first catalogue row index of the table.
const FIRST_ROW: i32 = 60_000;

/// Days on sale per product of an item, and the price for that time in thousandths of the one-day price: the factors of the
/// ROM's own GP products.
const PERIODS: [(i32, i32); 4] = [(1, 1_000), (7, 4_900), (30, 19_500), (90, 54_000)];

/// A time of year, both ends included: (month, day) to (month, day), which may run over the new year.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Season {
    pub from: (u32, u32),
    pub to: (u32, u32),
}

impl Season {
    /// Whether the date falls in this season.
    pub fn contains(&self, month: u32, day: u32) -> bool {
        let at = (month, day);
        if self.from <= self.to {
            self.from <= at && at <= self.to
        } else {
            at >= self.from || at <= self.to
        }
    }
}

pub const HALLOWEEN: Season = Season {
    from: (10, 1),
    to: (11, 30),
};
pub const THANKSGIVING: Season = Season {
    from: (11, 1),
    to: (12, 31),
};
pub const CHRISTMAS: Season = Season {
    from: (12, 1),
    to: (1, 31),
};

/// All year: items in the shop whatever the date.
pub const ALL_YEAR: Season = Season {
    from: (1, 1),
    to: (12, 31),
};

/// One item of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeasonalItem {
    pub item_no: i32,
    /// The catalogue row's item type.
    pub class_code: i32,
    /// The GP price of one day.
    pub day_price: i32,
    pub season: Season,
}

const fn item(item_no: i32, class_code: i32, day_price: i32, season: Season) -> SeasonalItem {
    SeasonalItem {
        item_no,
        class_code,
        day_price,
        season,
    }
}

// Item types: 7 melee, 8 an event weapon (the ROM's item table has them in the primary slot), 11 head, 12 face, 16 backpack.
// Prices follow the shop's own: head and face items as its dearer ones, a two-slot backpack 500 and a three-slot one 900; the
// all-year items are cheaper, as its everyday caps and scarves are, and a one-slot backpack 300.
// Melee at 700, the top of its melee prices, because these two hit harder than the shop's.
pub const ITEMS: &[SeasonalItem] = &[
    item(237, 7, 700, HALLOWEEN),
    item(4020, 11, 600, HALLOWEEN),
    item(7017, 16, 500, HALLOWEEN),
    item(7018, 16, 900, HALLOWEEN),
    item(270, 7, 700, THANKSGIVING),
    item(4052, 11, 600, THANKSGIVING),
    item(7037, 16, 500, THANKSGIVING),
    item(100, 8, 200, CHRISTMAS),
    item(101, 8, 200, CHRISTMAS),
    item(102, 8, 100, CHRISTMAS),
    item(103, 8, 200, CHRISTMAS),
    item(4006, 11, 600, CHRISTMAS),
    item(4025, 11, 600, CHRISTMAS),
    item(4058, 11, 900, CHRISTMAS),
    item(4026, 11, 600, CHRISTMAS),
    item(4059, 11, 900, CHRISTMAS),
    item(5013, 12, 300, CHRISTMAS),
    item(5019, 12, 450, CHRISTMAS),
    item(5014, 12, 400, CHRISTMAS),
    item(5020, 12, 600, CHRISTMAS),
    item(7007, 16, 500, CHRISTMAS),
    item(7008, 16, 900, CHRISTMAS),
    // new rows go last: a product's id follows its place in this table
    item(4046, 11, 300, ALL_YEAR),
    item(4060, 11, 450, ALL_YEAR),
    item(5017, 12, 300, ALL_YEAR),
    item(5021, 12, 450, ALL_YEAR),
    item(7031, 16, 300, ALL_YEAR),
    item(7033, 16, 900, ALL_YEAR),
];

/// The shop list an item shows on, by the thousands of its display number (`display_shelf` in the client): primary, melee, head,
/// face, backpack. Its place within the list is after the ROM's items.
fn display_of(class_code: i32, index: usize) -> i32 {
    let thousands = match class_code {
        8 => 0,
        7 => 2,
        11 => 5,
        12 => 6,
        _ => 9,
    };
    thousands * 1000 + 900 + index as i32
}

/// A name for an item the ROM gives no shop text: our own words, which the menus fall back on.
fn own_name(item_no: i32) -> &'static str {
    match item_no {
        100 => "Snowball Launcher",
        101 => "Ice Launcher",
        102 => "Snow Shovel",
        103 => "Snow Grenade",
        _ => "",
    }
}

/// The product ids of an item, one per period.
fn products_of(index: usize) -> impl Iterator<Item = i32> {
    let first = FIRST_PRODUCT + (index * PERIODS.len()) as i32;
    (0..PERIODS.len() as i32).map(move |k| first + k)
}

/// The product ids on sale on this date.
pub fn on_sale(month: u32, day: u32) -> Vec<i32> {
    ITEMS
        .iter()
        .enumerate()
        .filter(|(_, it)| it.season.contains(month, day))
        .flat_map(|(i, _)| products_of(i))
        .collect()
}

/// The table's rows and products, for the catalogue.
pub(crate) fn rows() -> Vec<(ShopItemRow, Vec<ShopProduct>)> {
    ITEMS
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let index = FIRST_ROW + i as i32;
            let row = ShopItemRow {
                index,
                itemid: it.item_no,
                class_code: it.class_code,
                new_item: 0,
                name: String::new(),
                gp_display: display_of(it.class_code, i),
                nx_display: 0,
                gp_unlock: 0,
                nx_unlock: 0,
            };
            let products = products_of(i)
                .zip(PERIODS)
                .map(|(id, (days, factor))| {
                    let price = it.day_price * factor / 1000;
                    ShopProduct {
                        id,
                        sale_type: 0,
                        view: 0,
                        price,
                        sale_price: price,
                        bonus: 0,
                        name: own_name(it.item_no).to_string(),
                        items: vec![(index, 0, days)],
                        period: days,
                        consume: 0,
                    }
                })
                .collect();
            (row, products)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seasons_end_on_their_last_day_and_christmas_runs_over_the_new_year() {
        assert!(HALLOWEEN.contains(11, 30));
        assert!(!HALLOWEEN.contains(12, 1));
        assert!(THANKSGIVING.contains(11, 3));
        assert!(THANKSGIVING.contains(12, 31));
        assert!(!THANKSGIVING.contains(1, 1));
        assert!(!CHRISTMAS.contains(11, 30));
        assert!(CHRISTMAS.contains(12, 1));
        assert!(CHRISTMAS.contains(1, 31));
        assert!(!CHRISTMAS.contains(2, 1));
        assert!(ALL_YEAR.contains(1, 1) && ALL_YEAR.contains(7, 15) && ALL_YEAR.contains(12, 31));
    }

    #[test]
    fn what_is_on_sale_follows_the_date() {
        let items_on = |month, day| -> Vec<i32> {
            let rows = rows();
            on_sale(month, day)
                .iter()
                .map(|pid| {
                    rows[((pid - FIRST_PRODUCT) / PERIODS.len() as i32) as usize]
                        .0
                        .itemid
                })
                .collect()
        };
        let november = items_on(11, 3);
        assert!(november.contains(&237), "Halloween");
        assert!(november.contains(&270), "Thanksgiving");
        assert!(!november.contains(&4026), "not yet Christmas");
        let december = items_on(12, 15);
        assert!(!december.contains(&237), "Halloween is over");
        assert!(december.contains(&270));
        assert!(december.contains(&4026));
        let january = items_on(1, 10);
        assert!(!january.contains(&270), "Thanksgiving is over");
        assert!(january.contains(&4026));
        let march = items_on(3, 1);
        assert_eq!(march.len(), 6 * PERIODS.len(), "only the all-year items");
        assert!(march.contains(&4046));
        assert!(november.contains(&4046) && december.contains(&4046) && january.contains(&4046));
    }

    #[test]
    fn every_product_has_a_gp_price_and_a_unique_id_on_the_right_shelf() {
        let rows = rows();
        assert_eq!(rows.len(), ITEMS.len());
        let mut ids = std::collections::BTreeSet::new();
        for (row, products) in &rows {
            assert_eq!(products.len(), PERIODS.len());
            assert!(row.gp_display > 0);
            for p in products {
                assert!(ids.insert(p.id), "{}", p.id);
                assert_eq!(p.sale_type, 0, "GP");
                assert_eq!(p.view, 0, "hidden unless the lobby lists it");
                assert!(p.sale_price > 0);
                assert!((0..0x10000).contains(&p.id));
            }
        }
        let shelf = |item: i32| {
            rows.iter()
                .find(|r| r.0.itemid == item)
                .unwrap()
                .0
                .gp_display
                / 1000
        };
        assert_eq!(
            [
                shelf(100),
                shelf(237),
                shelf(4020),
                shelf(5013),
                shelf(7017)
            ],
            [0, 2, 5, 6, 9]
        );
    }

    #[test]
    fn the_price_of_a_day_scales_like_the_roms_own_products() {
        let rows = rows();
        let (_, products) = rows.iter().find(|r| r.0.itemid == 4020).unwrap();
        let prices: Vec<i32> = products.iter().map(|p| p.sale_price).collect();
        assert_eq!(
            prices,
            vec![600, 2940, 11700, 32400],
            "as its ROM GP product"
        );
    }
}

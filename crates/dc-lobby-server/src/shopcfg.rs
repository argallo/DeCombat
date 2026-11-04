//! Operator rules of the shop, inventory, inbox, coupon and megaphone paths. Every value is a DeCombat default
//! (`DC_DEFAULTS.md`), never a 2009 server value. Rows CD-68.. are proposals (staged in
//! `DC_DEFAULTS.md`) and are cited "(proposed)".
//! Overrides use the same flat `lobby.json` as `config.rs` (keys below); `Rules::apply` forwards them here.

use dc_mod_lua::JsonValue;

/// One coupon issued by the operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CouponSpec {
    pub code: String,
    pub product: i32,
    /// Total redemptions allowed over all accounts; -1 = unlimited (each account still once, CD-39).
    pub uses: i64,
}

#[derive(Clone, Debug)]
pub struct ShopRules {
    // DC-DEFAULT: CD-01 supply case award duration: weighted draw [days, weight], 0 days = permanent
    pub case_odds: Vec<(u32, u32)>,
    // DC-DEFAULT: CD-02 a pack of N cases becomes N single-case inbox rows
    pub case_split_on_purchase: bool,
    // DC-DEFAULT: CD-09 new = first seen within N days up to the ROM build day
    pub new_days: u32,
    // DC-DEFAULT: CD-09 hot = top N % of visible products by sales
    pub hot_percent: u32,
    // DC-DEFAULT: CD-09 sale = operator list
    pub sale_list: Vec<i32>,
    // DC-DEFAULT: CD-35 operator replacement of a computed list (view, hot, new, sale); None = computed
    pub list_override: [Option<Vec<i32>>; 4],
    // DC-DEFAULT: CD-36 credit products go to the inbox
    pub credit_to_inbox: bool,
    // DC-DEFAULT: SHOP-1 (proposed, rust3) false = only products with the ROM display flag are listed and
    // purchasable; true also lists the products the ROM hides (`Shopview` 0, e.g. the shotguns) for the test phase
    pub show_hidden_products: bool,
    // DC-DEFAULT: SHOP-2 (proposed; USER DECISION 2026-11-03) true also sells the products of `dc_bute::seasonal` in their seasons
    pub seasonal: bool,
    /// The (month, day) the seasons read instead of today's date (tests).
    pub seasonal_today: Option<(u32, u32)>,
    // DC-DEFAULT: CD-70 (proposed) 0x359 price overrides {product: price}
    pub price_override: Vec<(i32, u32)>,
    // DC-DEFAULT: CD-71 (proposed) an extension uid in 0x328 extends the owned instance
    pub extend: bool,
    // DC-DEFAULT: CD-39 operator coupons
    pub coupons: Vec<CouponSpec>,
    // DC-DEFAULT: CD-40 seconds a megaphone message stays on air
    pub megaphone_secs: u32,
    // DC-DEFAULT: CD-83 (proposed) megaphone text limit (the 2009 input box allows 65 characters)
    pub megaphone_max_chars: usize,
    // DC-DEFAULT: CD-68 (proposed) a GP supply case bought in the shop opens at once
    pub gp_case_open_on_buy: bool,
    // DC-DEFAULT: CD-69 (proposed) at most N non-default items per inventory (res 0x503 says 100)
    pub max_items: u32,
    // DC-DEFAULT: CD-77 (proposed) own slots (0..3 = weapon slots) whose items may sit in backpack slots 10..13
    pub backpack_kinds: Vec<i64>,
    // DC-DEFAULT: CD-74 (proposed) inbox rows per 0x32d reply (0x8000 frame limit: at most 141)
    pub inbox_max_rows: u32,
    // DC-DEFAULT: CD-73 (proposed) an offline gift receiver gets a pending notice (0x303 +0x104 -> 0x366)
    pub notify_offline_receiver: bool,
    // DC-DEFAULT: CD-18 refinement (project decision ROM item number issued in weapon slot 0 to a new
    // character (0 = unset: the ROM balanced pick); the user's choice, resolved from the ROM weapon table by the operator
    pub default_primary_item: u32,
    // DC-DEFAULT: CD-18 refinement (project decision the same for weapon slot 1
    pub default_secondary_item: u32,
    /// An operator set either key (lobby.json / server.toml): the numbers apply even when no ROM tables are loaded.
    pub default_items_explicit: bool,
}

/// Rows per 0x32d reply that fit the 0x8000-byte body (0x16 + 0xe8 n).
// SPEC: PROTOCOL_2009.md §1.2 (max body 0x8000), §4.3 0x32d (row 0xe8)
pub const INBOX_ROWS_FIT: u32 = (0x8000 - 0x16) / 0xe8;

impl Default for ShopRules {
    fn default() -> Self {
        Self {
            case_odds: vec![(1, 60), (3, 25), (7, 10), (30, 4), (0, 1)],
            case_split_on_purchase: true,
            new_days: 30,
            hot_percent: 10,
            sale_list: Vec::new(),
            list_override: [None, None, None, None],
            credit_to_inbox: true,
            show_hidden_products: false,
            seasonal: false,
            seasonal_today: None,
            price_override: Vec::new(),
            extend: true,
            coupons: Vec::new(),
            megaphone_secs: 30,
            megaphone_max_chars: 65,
            gp_case_open_on_buy: true,
            max_items: 100,
            backpack_kinds: vec![0, 1, 2, 3],
            inbox_max_rows: INBOX_ROWS_FIT,
            notify_offline_receiver: true,
            // the user's choice 2026-11-03: M16A3 (20) + M1911 MEU (1); see DC_DEFAULTS.md CD-18 refinement
            default_primary_item: 20,
            default_secondary_item: 1,
            default_items_explicit: false,
        }
    }
}

fn num(v: &JsonValue) -> Option<f64> {
    match v {
        JsonValue::Number(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

fn int_list(v: &JsonValue) -> Option<Vec<i32>> {
    match v {
        JsonValue::Array(a) => a.iter().map(|x| num(x).map(|f| f as i32)).collect(),
        _ => None,
    }
}

fn coupon_spec(e: &JsonValue) -> Option<CouponSpec> {
    let spec = match e {
        JsonValue::Array(p) if p.len() >= 2 => match (&p[0], num(&p[1])) {
            (JsonValue::String(c), Some(pid)) => CouponSpec {
                code: c.clone(),
                product: pid as i32,
                uses: p.get(2).and_then(num).map(|u| u as i64).unwrap_or(-1),
            },
            _ => return None,
        },
        JsonValue::Object(m) => match (m.get("code"), m.get("product").and_then(num)) {
            (Some(JsonValue::String(c)), Some(pid)) => CouponSpec {
                code: c.clone(),
                product: pid as i32,
                uses: m.get("uses").and_then(num).map(|u| u as i64).unwrap_or(-1),
            },
            _ => return None,
        },
        _ => return None,
    };
    (!crate::coupons::normalize(&spec.code).is_empty()).then_some(spec)
}

impl ShopRules {
    /// Keys this struct owns.
    pub fn owns(key: &str) -> bool {
        matches!(
            key,
            "economy.case_odds"
                | "economy.case_split_on_purchase"
                | "economy.gp_case_open_on_buy"
                | "shop.new_days"
                | "shop.hot_percent"
                | "shop.sale_list"
                | "shop.lists"
                | "shop.credit_to_inbox"
                | "shop.show_hidden_products"
                | "shop.seasonal"
                | "shop.price_override"
                | "shop.extend"
                | "shop.coupons"
                | "shop.notify_offline_receiver"
                | "shop.default_primary_item"
                | "shop.default_secondary_item"
                | "megaphone.secs"
                | "megaphone.max_chars"
                | "inventory.max_items"
                | "inventory.backpack_kinds"
                | "inbox.max_rows"
        )
    }

    /// Applies one override; false for an unknown key or a value of the wrong shape.
    pub fn apply(&mut self, key: &str, v: &JsonValue) -> bool {
        let n = num(v);
        let b = match v {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        };
        match key {
            "economy.case_odds" => {
                let JsonValue::Array(a) = v else { return false };
                let mut out = Vec::new();
                for e in a {
                    match e {
                        JsonValue::Array(p) if p.len() == 2 => match (num(&p[0]), num(&p[1])) {
                            (Some(d), Some(w)) if d >= 0.0 && w >= 0.0 => {
                                out.push((d as u32, w as u32))
                            }
                            _ => return false,
                        },
                        _ => return false,
                    }
                }
                if out.iter().all(|&(_, w)| w == 0) {
                    return false;
                }
                self.case_odds = out;
                true
            }
            "economy.case_split_on_purchase" => {
                b.map(|x| self.case_split_on_purchase = x).is_some()
            }
            "economy.gp_case_open_on_buy" => b.map(|x| self.gp_case_open_on_buy = x).is_some(),
            "shop.new_days" => n
                .map(|x| self.new_days = x.clamp(0.0, 36500.0) as u32)
                .is_some(),
            "shop.hot_percent" => n
                .map(|x| self.hot_percent = x.clamp(0.0, 100.0) as u32)
                .is_some(),
            "shop.sale_list" => int_list(v).map(|l| self.sale_list = l).is_some(),
            "shop.lists" => {
                let JsonValue::Object(m) = v else {
                    return false;
                };
                for (k, list) in m {
                    let i = match k.as_str() {
                        "view" => 0,
                        "hot" => 1,
                        "new" => 2,
                        "sale" => 3,
                        _ => return false,
                    };
                    match int_list(list) {
                        Some(l) => self.list_override[i] = Some(l),
                        None => return false,
                    }
                }
                true
            }
            "shop.credit_to_inbox" => b.map(|x| self.credit_to_inbox = x).is_some(),
            "shop.show_hidden_products" => b.map(|x| self.show_hidden_products = x).is_some(),
            "shop.seasonal" => b.map(|x| self.seasonal = x).is_some(),
            "shop.price_override" => {
                let JsonValue::Object(m) = v else {
                    return false;
                };
                let mut out = Vec::new();
                for (k, p) in m {
                    match (k.trim().parse::<i32>().ok(), num(p)) {
                        (Some(pid), Some(price)) if price >= 0.0 => {
                            out.push((pid, price.min(u32::MAX as f64) as u32))
                        }
                        _ => return false,
                    }
                }
                out.sort_unstable();
                self.price_override = out;
                true
            }
            "shop.extend" => b.map(|x| self.extend = x).is_some(),
            "shop.coupons" => {
                let JsonValue::Array(a) = v else { return false };
                let specs: Option<Vec<CouponSpec>> = a.iter().map(coupon_spec).collect();
                specs.map(|s| self.coupons = s).is_some()
            }
            "shop.notify_offline_receiver" => b.map(|x| self.notify_offline_receiver = x).is_some(),
            "shop.default_primary_item" => n
                .map(|x| {
                    self.default_primary_item = x.clamp(0.0, 6999.0) as u32;
                    self.default_items_explicit = true;
                })
                .is_some(),
            "shop.default_secondary_item" => n
                .map(|x| {
                    self.default_secondary_item = x.clamp(0.0, 6999.0) as u32;
                    self.default_items_explicit = true;
                })
                .is_some(),
            "megaphone.secs" => n
                .map(|x| self.megaphone_secs = x.clamp(1.0, 3600.0) as u32)
                .is_some(),
            "megaphone.max_chars" => n
                .map(|x| self.megaphone_max_chars = x.clamp(1.0, 100.0) as usize)
                .is_some(),
            "inventory.max_items" => n
                .map(|x| self.max_items = x.clamp(1.0, 100_000.0) as u32)
                .is_some(),
            "inventory.backpack_kinds" => match int_list(v) {
                Some(l) if l.iter().all(|&k| (0..15).contains(&k)) => {
                    self.backpack_kinds = l.into_iter().map(i64::from).collect();
                    true
                }
                _ => false,
            },
            "inbox.max_rows" => n
                .map(|x| self.inbox_max_rows = x.clamp(1.0, INBOX_ROWS_FIT as f64) as u32)
                .is_some(),
            _ => false,
        }
    }

    /// The 0x359 override price of a product, if the operator set one.
    pub fn price_for(&self, product: i32) -> Option<u32> {
        self.price_override
            .iter()
            .find(|(p, _)| *p == product)
            .map(|(_, v)| *v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(t: &str) -> JsonValue {
        JsonValue::parse(t).unwrap()
    }

    #[test]
    fn defaults_follow_dc_defaults() {
        let r = ShopRules::default();
        // CD-01 odds, CD-02 split, CD-09 30 days / 10 %, CD-40 30 s
        assert_eq!(
            r.case_odds,
            vec![(1, 60), (3, 25), (7, 10), (30, 4), (0, 1)]
        );
        assert!(r.case_split_on_purchase && r.credit_to_inbox);
        assert_eq!((r.new_days, r.hot_percent, r.megaphone_secs), (30, 10, 30));
        assert_eq!((r.max_items, r.inbox_max_rows), (100, 141));
    }

    #[test]
    fn overrides_parse_and_bad_shapes_are_refused() {
        let mut r = ShopRules::default();
        assert!(r.apply("economy.case_odds", &j("[[2, 1], [0, 3]]")));
        assert_eq!(r.case_odds, vec![(2, 1), (0, 3)]);
        assert!(
            !r.apply("economy.case_odds", &j("[[2, 0]]")),
            "all-zero weights"
        );
        assert!(r.apply("shop.price_override", &j(r#"{"9": 100, "12": 0}"#)));
        assert_eq!(r.price_for(9), Some(100));
        assert_eq!(r.price_for(12), Some(0));
        assert_eq!(r.price_for(13), None);
        assert!(r.apply(
            "shop.coupons",
            &j(r#"[["ab-cd", 7], {"code": "x1", "product": 8, "uses": 2}]"#)
        ));
        assert_eq!(
            r.coupons[0],
            CouponSpec {
                code: "ab-cd".into(),
                product: 7,
                uses: -1
            }
        );
        assert_eq!(r.coupons[1].uses, 2);
        assert!(!r.apply("shop.coupons", &j(r#"[["", 7]]"#)));
        assert!(r.apply("shop.lists", &j(r#"{"hot": [1, 2], "view": [3]}"#)));
        assert_eq!(r.list_override[1], Some(vec![1, 2]));
        assert_eq!(r.list_override[0], Some(vec![3]));
        assert!(!r.apply("shop.lists", &j(r#"{"bogus": [1]}"#)));
        assert!(r.apply("inbox.max_rows", &j("1000")));
        assert_eq!(r.inbox_max_rows, INBOX_ROWS_FIT);
        assert!(!r.apply("inventory.backpack_kinds", &j("[20]")));
        assert!(ShopRules::owns("megaphone.secs") && !ShopRules::owns("room.countdown_ticks"));
    }
}

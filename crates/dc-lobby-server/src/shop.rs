//! The shop: catalogue lists and their versions (0x350..0x35d), purchases (0x328 / 0x329), gifts (0x32a / 0x32b),
//! the recipient check (0x346 / 0x347) and the dispatcher of every shop, inventory, inbox, coupon, megaphone and
//! item-window message. The catalogue is the user's ROM shop list (`SHOP_ITEM_LIST.TXT`), read at run time.
// SPEC: PROTOCOL_2009.md §4

use crate::delivery::{self, Consume, Delivered};
use crate::shopmsg::*;
use crate::state::{Core, Fault, HResult, Sid};
use crate::{clock, coupons, db, inbox, itemuse, loadout, megaphone};
use dc_bute::{ButeDoc, RezVfs, ShopCatalog, ShopFlagInputs, ShopFlagsPolicy, ShopProduct};
use dc_wire::{PktReader, PktWriter};
use rusqlite::{params, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Refusal reasons the 2009 client turns into its own message boxes.
// EVIDENCE: cshell+0x150c40 / cshell+0x1588a0 (0x329: 0x3b not enough money, 0x3c), cshell+0x393f0 (reason ->
// box table: 0x3c inventory full res 0x503, 0x56 / 0x6b no such character res 0x1805, 0x60 not for sale
// res 0x17f9, 0x62 not giftable res 0x17fa, 0x65 shop data updating res 0x18bf, 0x67 not enough credit
// res 0x1873, 0x68 receiver is no clan leader res 0x17fb)
pub mod reason {
    pub const NOT_ENOUGH_MONEY: u32 = 0x3b;
    pub const INVENTORY_FULL: u32 = 0x3c;
    pub const NO_SUCH_CHARACTER: u32 = 0x56;
    pub const NOT_FOR_SALE: u32 = 0x60;
    pub const NOT_GIFTABLE: u32 = 0x62;
    pub const SHOP_UNAVAILABLE: u32 = 0x65;
    pub const NOT_ENOUGH_CREDIT: u32 = 0x67;
    pub const RECEIVER_NOT_LEADER: u32 = 0x68;
    /// A purchase below the cell's rank requirement: the 2009 reason table (cshell+0x393f0) has no rank row, so the
    /// refusal takes the closest one, 0x60 "This item is not available for purchase." (0x17f9).
    // DC-DEFAULT: LG-302 (proposed) a rank refusal answers reason 0x60
    pub const RANK_TOO_LOW: u32 = NOT_FOR_SALE;
    /// 0x347: the name exists (gift window: receiver ok; rename window: name taken).
    pub const NAME_EXISTS: u32 = 0x20;
}

/// Run-time tables of the shop, from the user's ROM (`RomData::shop`). Nothing here is compiled in.
#[derive(Default)]
pub struct ShopTables {
    pub catalog: Option<ShopCatalog>,
    /// Every product id of the catalogue (shown or not), ascending; for `shop.show_hidden_products`.
    pub all_products: Vec<i32>,
    /// `[NameColorTable] ColorMax` of the ROM colour table (0 = unknown).
    pub name_colours: u32,
    /// `[CrosshairColorTable] ColorMax` (0 = unknown).
    pub reticle_colours: u32,
    /// Build day of the ROM (days since 1970-01-01, from the PE time stamp of `Game/CShell.dll`).
    pub rom_build_day: u32,
}

/// Days since 1970-01-01 of a PE image's header time stamp (public PE/COFF format: `e_lfanew` at 0x3c,
/// `TimeDateStamp` 8 bytes after the signature).
pub fn pe_build_day(bytes: &[u8]) -> Option<u32> {
    let rd = |o: usize| {
        bytes
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let pe = rd(0x3c)? as usize;
    if bytes.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    Some(rd(pe + 8)? / 86_400)
}

impl ShopTables {
    pub fn load(vfs: &RezVfs, dir: &Path) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let catalog = match ShopCatalog::from_vfs(vfs) {
            Ok(c) => Some(c),
            Err(e) => {
                notes.push(format!("shop list unavailable: {}", e));
                None
            }
        };
        let all_products = catalog
            .as_ref()
            // the seasonal rows are not the ROM's: rewards and "show everything" stay as they were
            .map(|c| {
                (0..dc_bute::seasonal::FIRST_PRODUCT)
                    .filter(|&p| c.product(p).is_some())
                    .collect()
            })
            .unwrap_or_default();
        let (name_colours, reticle_colours) =
            match ButeDoc::from_vfs(vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\COLORTABLE.TXT") {
                Some(d) => (
                    d.get_int("NameColorTable", "ColorMax", 0).max(0) as u32,
                    d.get_int("CrosshairColorTable", "ColorMax", 0).max(0) as u32,
                ),
                None => (0, 0),
            };
        let head = dc_rom::server_rom::shell_image(dir);
        let rom_build_day = head
            .as_deref()
            .and_then(|b| pe_build_day(&b[..b.len().min(4096)]))
            .unwrap_or(0);
        (
            Self {
                catalog,
                all_products,
                name_colours,
                reticle_colours,
                rom_build_day,
            },
            notes,
        )
    }
}

/// One item of a product: catalogue number, type code, consume kind, period / count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductItem {
    pub item_no: i64,
    pub class: i32,
    pub consume: Consume,
    pub period: i64,
}

/// The items of a product (`Itemindex<k>` -> `[ItemInfo]` `Itemid` / `Itemtype`).
pub fn product_items(cat: &ShopCatalog, p: &ShopProduct) -> Vec<ProductItem> {
    p.items
        .iter()
        .filter_map(|&(ix, consume, period)| {
            let info = cat.info(ix)?;
            // a GP grant item (type 0x19) never becomes a row, whatever its consume code says
            let consume = if info.class_code == delivery::CLASS_GP_GRANT {
                Consume::GpGrant
            } else {
                Consume::from_code(consume)
            };
            Some(ProductItem {
                item_no: info.itemid as i64,
                class: info.class_code,
                consume,
                period: period as i64,
            })
        })
        .collect()
}

/// The experience a product's shop cell asks of the buyer: its first item's catalogue row `GP_Unlock` (GP products)
/// or `NX_Unlock` (credit products); 0 = none.
// EVIDENCE: cshell+0x255860 (cell +0x10 = the item row's requirement of the cell's sale type), cshell+0x24fdb0
// (locked: the character's experience below +0x10)
pub fn unlock_exp(cat: &ShopCatalog, p: &ShopProduct) -> i64 {
    p.items
        .first()
        .and_then(|&(ix, _, _)| cat.info(ix))
        .map_or(0, |r| {
            if p.sale_type == 1 {
                r.nx_unlock
            } else {
                r.gp_unlock
            }
        })
        .max(0) as i64
}

/// True for a supply case product.
pub fn is_case(items: &[ProductItem]) -> bool {
    items
        .first()
        .map(|i| i.consume == Consume::Case || i.class == delivery::CLASS_CASE)
        .unwrap_or(false)
}

pub fn account_of(st: &Core, sid: Sid) -> Option<u32> {
    st.sessions
        .get(&sid)
        .filter(|s| s.in_lobby && !s.closing)
        .and_then(|s| s.account)
}

fn clamp_u32(v: i64) -> u32 {
    v.clamp(0, u32::MAX as i64) as u32
}

// ---------------------------------------------------------------------------------------------------------------
// catalogue lists and versions (§4.1)
// ---------------------------------------------------------------------------------------------------------------

/// Stored list keys (also the argument of [`raise_version`] / [`version_push`]).
pub const KEY_VIEW: i64 = 0;
pub const KEY_HOT: i64 = 1;
pub const KEY_NEW: i64 = 2;
pub const KEY_SALE: i64 = 3;
pub const KEY_LIST3: i64 = 4;
pub const KEY_LIST4: i64 = 5;
pub const KEY_PRICES: i64 = 6;

/// LG-314: a product that holds a pack item. No such product is on sale yet: it is off the shop on every entry.
// DC-DEFAULT: LG-314 (proposed, project decision "no shop sale of new guns yet")
pub fn holds_pack_item(st: &Core, pid: i32) -> bool {
    st.rom
        .shop
        .catalog
        .as_ref()
        .and_then(|c| c.product(pid).map(|p| product_items(c, p)))
        .is_some_and(|items| {
            items
                .iter()
                .any(|i| dc_bute::modpatch::is_pack_item(i.item_no))
        })
}

/// Product ids shown in the shop (0x351): the ROM display flag, or the operator's list.
// DC-DEFAULT: CD-35 shown = ROM display flag (`Shopview`); config shop.lists.view replaces it;
// SHOP-1 (proposed) `shop.show_hidden_products` lists the whole catalogue
pub fn view_list(st: &Core) -> Vec<i32> {
    let mut list = rom_or_operator_view(st);
    if st.rules.shop.list_override[0].is_none() {
        let seasonal = seasonal_on_sale(st);
        list.retain(|p| !seasonal.contains(p));
        list.extend(seasonal);
    }
    list.retain(|&p| !holds_pack_item(st, p));
    list
}

/// SHOP-2: the seasonal products on sale today (none with `shop.seasonal` off). An operator's own list stays as written.
fn seasonal_on_sale(st: &Core) -> Vec<i32> {
    if !st.rules.shop.seasonal {
        return Vec::new();
    }
    let (month, day) = st.rules.shop.seasonal_today.unwrap_or_else(|| {
        let (now, _) = clock::local_now();
        (u32::from(now.month), u32::from(now.day))
    });
    dc_bute::seasonal::on_sale(month, day)
}

fn rom_or_operator_view(st: &Core) -> Vec<i32> {
    if let Some(l) = &st.rules.shop.list_override[0] {
        return l
            .iter()
            .copied()
            .filter(|p| (0..0x10000).contains(p))
            .collect();
    }
    if st.rules.shop.show_hidden_products {
        return st.rom.shop.all_products.clone();
    }
    st.rom
        .shop
        .catalog
        .as_ref()
        .map(|c| c.view_list())
        .unwrap_or_default()
}

/// True when the product is shown, i.e. purchasable.
// DC-DEFAULT: CD-70 (proposed) only products in the 0x351 list can be bought or gifted
pub fn is_shown(st: &Core, pid: i32) -> bool {
    if holds_pack_item(st, pid) {
        return false;
    }
    if let Some(l) = &st.rules.shop.list_override[0] {
        return l.contains(&pid);
    }
    if seasonal_on_sale(st).contains(&pid) {
        return true;
    }
    st.rom
        .shop
        .catalog
        .as_ref()
        .and_then(|c| c.product(pid))
        .map(|p| (p.view != 0 || st.rules.shop.show_hidden_products) && (0..0x10000).contains(&pid))
        .unwrap_or(false)
}

fn sales(st: &Core) -> rusqlite::Result<HashMap<i32, u64>> {
    let mut q = st
        .db
        .prepare("SELECT product_id, COUNT(*) FROM shop_log GROUP BY product_id")?;
    let rows = q.query_map([], |r| {
        Ok((r.get::<_, i64>(0)? as i32, r.get::<_, i64>(1)? as u64))
    })?;
    rows.collect()
}

/// Hot / new / sale lists (0x353 / 0x355 / 0x357).
// DC-DEFAULT: CD-09 new / hot / sale via dc_bute::ShopFlagsPolicy; CD-85 (proposed) a ROM product whose
// first ItemInfo carries NewItem=1 counts as first seen on the ROM build day
pub fn flag_lists(st: &Core) -> rusqlite::Result<[Vec<i32>; 3]> {
    let r = &st.rules.shop;
    let computed = match st.rom.shop.catalog.as_ref() {
        Some(cat) => {
            let build = st.rom.shop.rom_build_day;
            let mut first_seen = HashMap::new();
            for pid in cat.view_list() {
                let marked = cat
                    .product(pid)
                    .and_then(|p| p.items.first())
                    .and_then(|&(ix, _, _)| cat.info(ix))
                    .map(|i| i.new_item == 1)
                    .unwrap_or(false);
                if marked {
                    first_seen.insert(pid, build);
                }
            }
            let sold = sales(st)?;
            let sale: HashSet<i32> = r.sale_list.iter().copied().collect();
            let policy = ShopFlagsPolicy {
                new_window_days: r.new_days,
                hot_percent: r.hot_percent,
            };
            let f = policy.evaluate(&ShopFlagInputs {
                catalogue: cat,
                rom_build_day: build,
                first_seen_day: &first_seen,
                sales: &sold,
                operator_sale: &sale,
            });
            [f.hot, f.new, f.sale]
        }
        None => [Vec::new(), Vec::new(), r.sale_list.clone()],
    };
    let [mut hot, new, sale] = computed;
    if r.list_override[1].is_none() {
        pad_hot_credit(st, &mut hot);
    }
    Ok([
        r.list_override[1].clone().unwrap_or(hot),
        r.list_override[2].clone().unwrap_or(new),
        r.list_override[3].clone().unwrap_or(sale),
    ])
}

/// Credit products the Hot list is filled up to when the sales rank leaves fewer.
pub const HOT_CREDIT_MIN: usize = 12;

/// LG-50: pads the Hot list (0x353) with shown credit products (catalogue order, one per item) up to
/// [`HOT_CREDIT_MIN`] credit entries. A fresh server has no sales, so the sales rank (CD-09) leaves the Black Market's
/// Hot tab, which lists the credit products of this list, empty on the first visit.
// DC-DEFAULT: LG-50 (playtest report: "black market is empty still") the 2009 ranking source is unknown; our rank
// alone never marks an unsold product hot
fn pad_hot_credit(st: &Core, hot: &mut Vec<i32>) {
    let Some(cat) = st.rom.shop.catalog.as_ref() else {
        return;
    };
    let credit = |pid: i32| cat.product(pid).is_some_and(|p| p.sale_type == 1);
    let mut have = hot.iter().filter(|p| credit(**p)).count();
    let mut items: HashSet<i32> = hot
        .iter()
        .filter_map(|p| cat.product(*p))
        .filter_map(|p| p.items.first().and_then(|&(ix, _, _)| cat.info(ix)))
        .map(|i| i.itemid)
        .collect();
    for pid in view_list(st) {
        if have >= HOT_CREDIT_MIN {
            break;
        }
        let Some(p) = cat.product(pid).filter(|p| p.sale_type == 1) else {
            continue;
        };
        let Some(item) = p.items.first().and_then(|&(ix, _, _)| cat.info(ix)) else {
            continue;
        };
        if hot.contains(&pid) || !items.insert(item.itemid) {
            continue;
        }
        hot.push(pid);
        have += 1;
    }
}

fn fnv(data: &[u8]) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h as i64
}

fn ids_bytes(ids: &[i32]) -> Vec<u8> {
    ids.iter().flat_map(|i| i.to_le_bytes()).collect()
}

/// Version of one stored list: 1 for a new list, +1 whenever its content changes.
// DC-DEFAULT: CD-35 versions start at 1 and grow on change (the client fetches only when the version grows)
fn version_of(st: &Core, key: i64, content: &[u8]) -> rusqlite::Result<u32> {
    let h = fnv(content);
    let cur: Option<(i64, i64)> = st
        .db
        .query_row(
            "SELECT hash, version FROM shop_lists WHERE list = ?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match cur {
        Some((hash, v)) if hash == h => Ok(v.max(1) as u32),
        Some((_, v)) => {
            st.db.execute(
                "UPDATE shop_lists SET hash = ?2, version = ?3 WHERE list = ?1",
                params![key, h, v + 1],
            )?;
            Ok((v + 1) as u32)
        }
        None => {
            st.db.execute(
                "INSERT INTO shop_lists (list, hash, version) VALUES (?1, ?2, 1)",
                params![key, h],
            )?;
            Ok(1)
        }
    }
}

/// Raises one list's version (an operator's "fetch again"; the content check keeps the raised number).
// DC-DEFAULT: CD-35 versions only grow
#[allow(dead_code)] // API for the operator command task (shop version pushes)
pub fn raise_version(st: &Core, key: i64) -> rusqlite::Result<u32> {
    let _ = versions(st).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
    st.db.execute(
        "UPDATE shop_lists SET version = version + 1 WHERE list = ?1",
        [key],
    )?;
    st.db
        .query_row(
            "SELECT version FROM shop_lists WHERE list = ?1",
            [key],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v as u32)
}

/// The version push of one list: 0x3054 view, 0x3056 / 0x3057 / 0x3058 lists 0..2, 0x305a / 0x305b lists 3 / 4,
/// 0x3059 price counter; each `+6 u32 version` (0xa bytes). The client fetches when the version grows.
// SPEC: PROTOCOL_2009.md §3.5 (EVIDENCE: cshell+0x36420, cshell+0x36450, cshell+0x42df0 inline cases)
#[allow(dead_code)] // API for the operator command task (shop version pushes)
pub fn version_push(key: i64, version: u32) -> Option<Vec<u8>> {
    let id = match key {
        KEY_VIEW => 0x3054,
        KEY_HOT => 0x3056,
        KEY_NEW => 0x3057,
        KEY_SALE => 0x3058,
        KEY_LIST3 => 0x305a,
        KEY_LIST4 => 0x305b,
        KEY_PRICES => 0x3059,
        _ => return None,
    };
    let mut w = PktWriter::new(id);
    w.u32(version);
    Some(w.into_frame())
}

/// Versions announced in 0x301: shop view (+0x2a), lists 0..4 (+0x2e / +0x32 / +0x36 / +0x42 / +0x46) and the
/// price counter (+0x26).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Versions {
    pub view: u32,
    pub lists: [u32; 5],
    pub prices: u32,
}

// SPEC: PROTOCOL_2009.md §3.3 0x301 (+0x26 counter, +0x2a view, +0x2e..+0x36 lists 0..2, +0x42 / +0x46 lists 3 / 4)
pub fn versions(st: &Core) -> Result<Versions, Fault> {
    let view = version_of(st, KEY_VIEW, &ids_bytes(&view_list(st)))?;
    let [hot, new, sale] = flag_lists(st)?;
    let l0 = version_of(st, KEY_HOT, &ids_bytes(&hot))?;
    let l1 = version_of(st, KEY_NEW, &ids_bytes(&new))?;
    let l2 = version_of(st, KEY_SALE, &ids_bytes(&sale))?;
    let l3 = version_of(st, KEY_LIST3, &[])?;
    let l4 = version_of(st, KEY_LIST4, &[])?;
    let prices: Vec<i32> = st
        .rules
        .shop
        .price_override
        .iter()
        .flat_map(|&(p, v)| [p, v as i32])
        .collect();
    let pv = version_of(st, KEY_PRICES, &ids_bytes(&prices))?;
    Ok(Versions {
        view,
        lists: [l0, l1, l2, l3, l4],
        prices: pv,
    })
}

/// 0x351 / 0x353 / 0x355 / 0x357: +6 result; +0xa reason; +0xe version; +0x12 count; +0x16 u16[count] ids.
// SPEC: PROTOCOL_2009.md §4.1 (EVIDENCE: cshell+0x42ca0, cshell+0x2581c0 / cshell+0x258240 / cshell+0x2582c0)
pub fn id_list_reply(id: u16, version: u32, ids: &[i32]) -> Vec<u8> {
    let ids: Vec<u16> = ids
        .iter()
        .filter(|&&p| (0..0x10000).contains(&p))
        .map(|&p| p as u16)
        .collect();
    let n = ids.len().min((0x8000 - 0x16) / 2);
    let mut w = PktWriter::new(id);
    w.u32(1).u32(0).u32(version).u32(n as u32);
    for i in &ids[..n] {
        w.u16(*i);
    }
    w.into_frame()
}

/// 0x35b (u32 values) / 0x35d ({u32 item number, u32 exp}) / 0x359 ({u32 product, u32 price}): +6 result;
/// +0xa reason; +0xe version; +0x12 count; +0x16 records.
// SPEC: PROTOCOL_2009.md §4.1 0x35b / 0x35d (cshell+0x42df0 cases), 0x359 (EVIDENCE: cshell+0x41350 -> cshell+0x251590)
pub fn word_list_reply(id: u16, version: u32, words_per_record: usize, words: &[u32]) -> Vec<u8> {
    let per = words_per_record.max(1);
    let n = (words.len() / per).min((0x8000 - 0x16) / (4 * per));
    let mut w = PktWriter::new(id);
    w.u32(1).u32(0).u32(version).u32(n as u32);
    for v in &words[..n * per] {
        w.u32(*v);
    }
    w.into_frame()
}

fn on_list_fetch(st: &mut Core, sid: Sid, id: u16) -> HResult {
    let v = versions(st)?;
    let frame = match id {
        SHOP_VIEW_FETCH => id_list_reply(SHOP_VIEW_REPLY, v.view, &view_list(st)),
        LIST_FETCH_0 | LIST_FETCH_1 | LIST_FETCH_2 => {
            let lists = flag_lists(st)?;
            let k = ((id - LIST_FETCH_0) / 2) as usize;
            id_list_reply(id + 1, v.lists[k], &lists[k])
        }
        // DC-DEFAULT: CD-85 (proposed) list 3 (no 2009 reader) and list 4 (credit-cell rank requirements) are empty
        LIST_FETCH_3 => word_list_reply(LIST_REPLY_3, v.lists[3], 1, &[]),
        LIST_FETCH_4 => word_list_reply(LIST_REPLY_4, v.lists[4], 2, &[]),
        _ => {
            // DC-DEFAULT: CD-70 (proposed) 0x359 carries the operator price overrides
            let words: Vec<u32> = st
                .rules
                .shop
                .price_override
                .iter()
                .flat_map(|&(p, price)| [p as u32, price])
                .collect();
            word_list_reply(PRICE_REPLY, v.prices, 2, &words)
        }
    };
    st.send(sid, frame);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// purchase (§4.2 0x328 / 0x329)
// ---------------------------------------------------------------------------------------------------------------

/// Parsed 0x328: product id, payment option, owned unique ids to extend (0 = new).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuyRequest {
    pub product: i32,
    pub option: u32,
    pub extend: [u32; 5],
}

// SPEC: PROTOCOL_2009.md §4.2 0x328 (EVIDENCE: cshell+0x18cf50; the option word is the dialog's payment mode - 1,
// so 0 = credit balance, otherwise prepaid: cshell+0x145b60)
pub fn parse_buy(p: &[u8]) -> BuyRequest {
    let mut r = PktReader::new(p);
    let product = r.at(6).i32();
    let option = r.u32();
    let mut extend = [0u32; 5];
    for e in &mut extend {
        *e = r.u32();
    }
    BuyRequest {
        product,
        option,
        extend,
    }
}

/// 0x329 (0x22): +6 ok; +0xa u16 reason; +0xe unique id; +0x12 FILETIME end; +0x1a GP after; +0x1e i32 award
/// item number of a supply case opened at purchase (-1 = none).
// SPEC: PROTOCOL_2009.md §4.2 0x329 (EVIDENCE: cshell+0x336a0 -> cshell+0x150c40 / cshell+0x1588a0)
pub fn buy_reply(ok: bool, reason: u32, uid: u32, end: u64, gp_after: u32, award: i32) -> Vec<u8> {
    let mut w = PktWriter::new(SHOP_BUY_REPLY);
    w.u32(ok as u32)
        .u16(reason as u16)
        .at(0xe)
        .u32(uid)
        .u64(end)
        .u32(gp_after)
        .i32(award);
    w.into_frame()
}

fn refuse_purchase(st: &mut Core, sid: Sid, reason: u32, gp: i64) {
    st.send(sid, buy_reply(false, reason, 0, 0, clamp_u32(gp), -1));
}

/// Records a sale (hot list input, audit). currency: 0 GP, 1 credit balance, 2 prepaid balance.
fn log_sale(
    st: &Core,
    account: u32,
    pid: i32,
    price: u32,
    currency: i64,
    receiver: u32,
) -> rusqlite::Result<()> {
    st.db.execute(
        "INSERT INTO shop_log (user_no, product_id, price, currency, receiver, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![account as i64, pid, price as i64, currency, receiver as i64, clock::unix_now()],
    )?;
    Ok(())
}

/// Price of a product: the 0x359 override or the ROM sale price (what the client checks and deducts).
// DC-DEFAULT: CD-36 price from the ROM product row or the 0x359 override; CD-70 (proposed) the sale price is used
// for both currencies (the client deducts SalePrice: cshell+0x150c40, cshell+0x1d6cb0)
pub fn price_of(st: &Core, p: &ShopProduct) -> u32 {
    st.rules
        .shop
        .price_for(p.id)
        .unwrap_or(p.sale_price.max(0) as u32)
}

/// True when every item of the purchase extends an instance the buyer owns (the request's unique ids, CD-71).
fn extends_owned(
    st: &Core,
    account: u32,
    items: &[ProductItem],
    req: &BuyRequest,
) -> rusqlite::Result<bool> {
    for (k, it) in items.iter().enumerate() {
        if it.consume == Consume::GpGrant {
            continue;
        }
        let uid = req.extend[k.min(4)] as i64;
        if uid == 0 || delivery::extendable(st, account, uid, it.item_no)?.is_none() {
            return Ok(false);
        }
    }
    Ok(!items.is_empty())
}

// DC-DEFAULT: CD-36 purchase rule (GP products to the inventory, credit products to the inbox); CD-37 the credit
// balance; CD-68 (proposed) a GP supply case opens at purchase; CD-69 (proposed) inventory capacity; CD-71
// (proposed) extensions
fn on_buy(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let req = parse_buy(p);
    let Some(account) = account_of(st, sid) else {
        refuse_purchase(st, sid, reason::NOT_FOR_SALE, 0);
        return Ok(());
    };
    let ch = st.character(account).unwrap_or_default();
    let Some(cat) = st.rom.shop.catalog.as_ref() else {
        refuse_purchase(st, sid, reason::SHOP_UNAVAILABLE, ch.gp);
        return Ok(());
    };
    let Some(prod) = cat.product(req.product).cloned() else {
        refuse_purchase(st, sid, reason::NOT_FOR_SALE, ch.gp);
        return Ok(());
    };
    let items = product_items(cat, &prod);
    if items.is_empty() || !is_shown(st, req.product) {
        refuse_purchase(st, sid, reason::NOT_FOR_SALE, ch.gp);
        return Ok(());
    }
    let price = price_of(st, &prod);
    let credit = prod.sale_type != 0;
    // DC-DEFAULT: LG-302 (proposed) a purchase below the cell's rank requirement is refused (reason 0x60), except a
    // prepaid credit purchase that goes to the inbox (it waits there until the rank, 0x1824) and an extension of an
    // owned instance; the 2009 client's own gates are the credit form (cshell+0x1da4a0) and the locked cell
    if ch.exp < unlock_exp(cat, &prod)
        && !(credit && req.option != 0 && st.rules.shop.credit_to_inbox)
        && !extends_owned(st, account, &items, &req)?
    {
        refuse_purchase(st, sid, reason::RANK_TOO_LOW, ch.gp);
        return Ok(());
    }
    // CD-37: payment option 0 draws on the credit balance, anything else on the prepaid balance
    let (balance, column, currency) = if !credit {
        (ch.gp, "gp", 0)
    } else if req.option == 0 {
        (ch.nx_cr, "nx_cr", 1)
    } else {
        (ch.nx, "nx", 2)
    };
    if balance < price as i64 {
        refuse_purchase(st, sid, reason::NOT_ENOUGH_MONEY, ch.gp);
        return Ok(());
    }
    let case = is_case(&items);
    let to_inbox = (credit && st.rules.shop.credit_to_inbox)
        || (case && !credit && !st.rules.shop.gp_case_open_on_buy);
    if case && !to_inbox {
        let pool_ok = st
            .rom
            .items
            .as_ref()
            .map(|t| {
                items
                    .iter()
                    .all(|i| !t.supply_pool(i.item_no as i32).is_empty())
            })
            .unwrap_or(false);
        if !pool_ok {
            refuse_purchase(st, sid, reason::SHOP_UNAVAILABLE, ch.gp);
            return Ok(());
        }
    }
    if !to_inbox {
        let new_rows = items
            .iter()
            .enumerate()
            .filter(|(k, i)| match i.consume {
                Consume::GpGrant => false,
                Consume::Case => true,
                _ => req.extend[(*k).min(4)] == 0,
            })
            .map(|(_, i)| {
                if i.consume == Consume::Case {
                    i.period.max(1)
                } else {
                    1
                }
            })
            .sum::<i64>();
        if !delivery::room_for(st, account, new_rows)? {
            refuse_purchase(st, sid, reason::INVENTORY_FULL, ch.gp);
            return Ok(());
        }
    }
    st.db.execute(
        &format!("UPDATE characters SET {column} = {column} - ?2 WHERE user_no = ?1"),
        params![account as i64, price as i64],
    )?;
    log_sale(st, account, req.product, price, currency, account)?;
    if to_inbox {
        inbox::add_product_rows(st, account, req.product, 0, "", "")?;
        let gp = st.character(account).map(|c| c.gp).unwrap_or(0);
        st.send(sid, buy_reply(true, 0, 0, 0, clamp_u32(gp), -1));
        // DC-DEFAULT: LG-125 (proposed) an inbox purchase also marks the buyer's inbox stale (0x3014), so every screen
        // that bought (the modification page too) fetches the new rows; the 2009 shop page's own reply handler marks
        // it itself (cshell+0x150c40: shell +0x1bc +0x1a = 1 on a credit purchase)
        inbox::push_stale(st, account);
        st.note(&format!(
            "shop: account {} bought product {} for {} ({}) -> inbox",
            account, req.product, price, column
        ));
        return Ok(());
    }
    let mut first: Option<Delivered> = None;
    let mut award = -1i32;
    for (k, it) in items.iter().enumerate() {
        match it.consume {
            Consume::GpGrant => {}
            Consume::Case => {
                for _ in 0..it.period.max(1) {
                    if let Some(d) = delivery::open_case(st, account, it.item_no)? {
                        award = d.item_no as i32;
                        crate::notices::case_award(st, account, &d)?;
                        first.get_or_insert(d);
                    }
                }
            }
            _ => {
                let ext = req.extend.get(k).copied().unwrap_or(0) as i64;
                if let Some(d) =
                    delivery::deliver(st, account, it.item_no, it.consume, it.period, Some(ext))?
                {
                    first.get_or_insert(d);
                }
            }
        }
    }
    // the product's bonus GP (the client shows the new total from +0x1a)
    if prod.bonus > 0 {
        st.db.execute(
            "UPDATE characters SET gp = gp + ?2 WHERE user_no = ?1",
            params![account as i64, prod.bonus as i64],
        )?;
    }
    let gp = st.character(account).map(|c| c.gp).unwrap_or(0);
    let d = first.unwrap_or_default();
    st.send(
        sid,
        buy_reply(
            true,
            0,
            d.uid as u32,
            d.end.max(0) as u64,
            clamp_u32(gp),
            award,
        ),
    );
    st.note(&format!(
        "shop: account {} bought product {} for {} ({}) -> uid {}{}",
        account,
        req.product,
        price,
        column,
        d.uid,
        if d.extended { " (extended)" } else { "" }
    ));
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// recipient check and gifts (§4.3 0x346 / 0x347, 0x32a / 0x32b)
// ---------------------------------------------------------------------------------------------------------------

/// 0x347 (>= 0xe): +6 ok; +0xa reason. An existing name answers {0, 0x20}: the gift window takes that as "the
/// receiver exists" (cshell+0x1d55e0), the rename window as "name taken" (cshell+0x2426d0); a free name {1, 0}.
// SPEC: PROTOCOL_2009.md §4.3 0x347 (EVIDENCE: dispatcher cshell+0x42df0 -> cshell+0x33890)
pub fn recipient_reply(exists: bool) -> Vec<u8> {
    let mut w = PktWriter::new(RECIPIENT_CHECK_REPLY);
    if exists {
        w.u32(0).u32(reason::NAME_EXISTS);
    } else {
        w.u32(1).u32(0);
    }
    w.into_frame()
}

fn on_recipient_check(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let name = PktReader::new(p).at(6).wstr(17);
    let exists = !name.trim().is_empty()
        && db::character_by_nick(&st.db, name.trim(), st.rules.names_case_insensitive)?.is_some();
    st.send(sid, recipient_reply(exists));
    Ok(())
}

/// Parsed 0x32a: product, receiver name, message.
// SPEC: PROTOCOL_2009.md §4.3 0x32a (EVIDENCE: cshell+0x18e0c0)
pub fn parse_gift(p: &[u8]) -> (i32, String, String) {
    let mut r = PktReader::new(p);
    let pid = r.at(6).i32();
    let to = r.at(0xa).wstr(17);
    let msg = r.at(0x2c).wstr(83);
    (pid, to, msg)
}

/// 0x32b (>= 0xa): +6 ok; +0xa u32 reason (failure box via the client's reason table, cshell+0x1d6cb0).
// SPEC: PROTOCOL_2009.md §4.3 0x32b (LIVE; EVIDENCE: cshell+0x1d6cb0 reads +6 and, on failure, +0xa)
pub fn gift_reply(ok: bool, reason: u32) -> Vec<u8> {
    let mut w = PktWriter::new(GIFT_SEND_REPLY);
    w.u32(ok as u32).u32(reason);
    w.into_frame()
}

/// True when a product holds a clan item (ROM UseItem SubType 1, 2, 3 or 9).
fn has_clan_item(st: &Core, items: &[ProductItem]) -> bool {
    items
        .iter()
        .any(|i| matches!(itemuse::subtype(st, i.item_no), Some(1 | 2 | 3 | 9)))
}

// DC-DEFAULT: CD-38 gifts need an existing receiver and land in the receiver's inbox with the message;
// CD-73 (proposed) credit products only, the sender pays from the prepaid balance (the client deducts +0x11c,
// cshell+0x1d6cb0), clan items need a clan-leader receiver, online receivers get 0x3014, offline receivers a
// pending notice
fn on_gift(st: &mut Core, sid: Sid, p: &[u8]) -> HResult {
    let (pid, to, message) = parse_gift(p);
    let Some(account) = account_of(st, sid) else {
        st.send(sid, gift_reply(false, reason::NOT_FOR_SALE));
        return Ok(());
    };
    let Some(cat) = st.rom.shop.catalog.as_ref() else {
        st.send(sid, gift_reply(false, reason::SHOP_UNAVAILABLE));
        return Ok(());
    };
    let Some(prod) = cat.product(pid).cloned() else {
        st.send(sid, gift_reply(false, reason::NOT_FOR_SALE));
        return Ok(());
    };
    let items = product_items(cat, &prod);
    if items.is_empty() || !is_shown(st, pid) {
        st.send(sid, gift_reply(false, reason::NOT_FOR_SALE));
        return Ok(());
    }
    if prod.sale_type == 0 {
        st.send(sid, gift_reply(false, reason::NOT_GIFTABLE));
        return Ok(());
    }
    let sender = st.character(account).unwrap_or_default();
    let receiver = db::character_by_nick(&st.db, to.trim(), st.rules.names_case_insensitive)?;
    let Some(receiver) = receiver.filter(|r| r.user_no != account) else {
        st.send(sid, gift_reply(false, reason::NO_SUCH_CHARACTER));
        return Ok(());
    };
    if has_clan_item(st, &items) && receiver.clan_role != 5 {
        st.send(sid, gift_reply(false, reason::RECEIVER_NOT_LEADER));
        return Ok(());
    }
    let price = price_of(st, &prod);
    if sender.nx < price as i64 {
        st.send(sid, gift_reply(false, reason::NOT_ENOUGH_CREDIT));
        return Ok(());
    }
    st.db.execute(
        "UPDATE characters SET nx = nx - ?2 WHERE user_no = ?1",
        params![account as i64, price as i64],
    )?;
    log_sale(st, account, pid, price, 2, receiver.user_no)?;
    inbox::add_product_rows(st, receiver.user_no, pid, 1, &sender.nick, &message)?;
    let online = st.sids_of(receiver.user_no);
    if !online.is_empty() {
        inbox::push_stale(st, receiver.user_no);
    } else if st.rules.shop.notify_offline_receiver {
        inbox::queue_notice(
            st,
            receiver.user_no,
            &format!("{} sent you a gift", sender.nick),
        )?;
    }
    st.send(sid, gift_reply(true, 0));
    st.note(&format!(
        "shop: {} gifted product {} to {} ({} prepaid)",
        sender.nick, pid, receiver.nick, price
    ));
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// dispatcher
// ---------------------------------------------------------------------------------------------------------------

/// Shop, inventory, inbox, coupon, megaphone and item-window messages; false when `id` is not one of them.
/// Routed before the core lobby dispatcher, so it owns 0x330 (item windows) and 0x366 (pending notices).
pub fn dispatch(st: &mut Core, sid: Sid, id: u16, p: &[u8]) -> Result<bool, Fault> {
    match id {
        SHOP_VIEW_FETCH | LIST_FETCH_0 | LIST_FETCH_1 | LIST_FETCH_2 | LIST_FETCH_3
        | LIST_FETCH_4 | PRICE_FETCH => on_list_fetch(st, sid, id)?,
        SHOP_BUY => on_buy(st, sid, p)?,
        GIFT_SEND => on_gift(st, sid, p)?,
        RECIPIENT_CHECK => on_recipient_check(st, sid, p)?,
        INBOX_PAGE => inbox::on_page(st, sid, p)?,
        INBOX_OPEN => inbox::on_open(st, sid, p)?,
        crate::msg::NOTICE_FETCH => inbox::on_notice(st, sid)?,
        COUPON_REDEEM => coupons::on_redeem(st, sid, p)?,
        MEGAPHONE_QUEUE => megaphone::on_queue(st, sid)?,
        MEGAPHONE_SEND => megaphone::on_send(st, sid, p)?,
        crate::msg::ITEM_USE => itemuse::on_use(st, sid, p)?,
        EQUIP => loadout::on_equip(st, sid, p)?,
        UNEQUIP => loadout::on_unequip(st, sid, p)?,
        FUNCTION_EQUIP | FUNCTION_UNEQUIP => loadout::on_function_slot(st, sid, id, p)?,
        ITEM_DISCARD => loadout::on_discard(st, sid, p)?,
        ATTACH => loadout::on_attach(st, sid, p)?,
        DETACH => loadout::on_detach(st, sid, p)?,
        TUNING_SYNC => loadout::on_tuning(st, sid, p)?,
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::u32_at;

    fn split(f: &[u8]) -> (i32, Vec<u8>) {
        let (frames, used) = dc_wire::parse_tcp_frames(f).unwrap();
        assert_eq!(used, f.len());
        let (id, p) = frames.into_iter().next().unwrap();
        assert_eq!(
            u16::from_le_bytes([p[0], p[1]]) as i32,
            id,
            "inner id = frame id"
        );
        assert_eq!(
            u32::from_le_bytes([p[2], p[3], p[4], p[5]]) as usize,
            p.len(),
            "inner length"
        );
        (id, p)
    }

    fn u16_at(p: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([p[o], p[o + 1]])
    }

    #[test]
    fn id_list_layout_matches_the_spec() {
        // SPEC: PROTOCOL_2009.md §4.1 0x351: +6 result, +0xa reason, +0xe version, +0x12 count, +0x16 u16 ids
        let (id, p) = split(&id_list_reply(SHOP_VIEW_REPLY, 7, &[3, 0x1234, 70000, -1]));
        assert_eq!(id, 0x351);
        assert_eq!(
            (
                u32_at(&p, 6),
                u32_at(&p, 0xa),
                u32_at(&p, 0xe),
                u32_at(&p, 0x12)
            ),
            (1, 0, 7, 2)
        );
        assert_eq!(
            (u16_at(&p, 0x16), u16_at(&p, 0x18)),
            (3, 0x1234),
            "ids outside u16 are dropped"
        );
        assert_eq!(p.len(), 0x16 + 4);
        let (id, _) = split(&id_list_reply(LIST_FETCH_1 + 1, 1, &[]));
        assert_eq!(id, 0x355);
    }

    #[test]
    fn word_list_layouts_match_the_spec() {
        // 0x359 {product, price}, 0x35d {item, exp}, 0x35b u32: count at +0x12, records at +0x16
        let (id, p) = split(&word_list_reply(PRICE_REPLY, 2, 2, &[9, 100, 12, 0]));
        assert_eq!(
            (id, u32_at(&p, 6), u32_at(&p, 0xe), u32_at(&p, 0x12)),
            (0x359, 1, 2, 2)
        );
        assert_eq!(
            (
                u32_at(&p, 0x16),
                u32_at(&p, 0x1a),
                u32_at(&p, 0x1e),
                u32_at(&p, 0x22)
            ),
            (9, 100, 12, 0)
        );
        assert_eq!(p.len(), 0x16 + 2 * 8, "harness: count * 8 + 0x16 == len");
        let (_, p) = split(&word_list_reply(LIST_REPLY_3, 1, 1, &[]));
        assert_eq!((p.len(), u32_at(&p, 0x12)), (0x16, 0));
        let (_, p) = split(&word_list_reply(LIST_REPLY_4, 1, 2, &[5]));
        assert_eq!(u32_at(&p, 0x12), 0, "half a record is not sent");
    }

    #[test]
    fn buy_layouts_match_the_spec() {
        // SPEC: PROTOCOL_2009.md §4.2 0x328: +6 product, +0xa option, +0xe u32[5] extend
        let mut w = PktWriter::new(SHOP_BUY);
        w.i32(9).u32(1).u32(5).u32(0).u32(0).u32(0).u32(7);
        let p = w.into_payload();
        assert_eq!(p.len(), 0x22);
        assert_eq!(
            parse_buy(&p),
            BuyRequest {
                product: 9,
                option: 1,
                extend: [5, 0, 0, 0, 7]
            }
        );
        // 0x329: +6 ok, +0xa u16 reason, +0xe uid, +0x12 FILETIME end, +0x1a GP after, +0x1e award (-1)
        let (id, p) = split(&buy_reply(true, 0, 42, 0x0102_0304_0506_0708, 6080, -1));
        assert_eq!((id, p.len()), (0x329, 0x22));
        assert_eq!(
            (
                u32_at(&p, 6),
                u32_at(&p, 0xe),
                u32_at(&p, 0x1a),
                u32_at(&p, 0x1e)
            ),
            (1, 42, 6080, u32::MAX)
        );
        assert_eq!(
            u64::from_le_bytes(p[0x12..0x1a].try_into().unwrap()),
            0x0102_0304_0506_0708
        );
        let (_, p) = split(&buy_reply(false, reason::NOT_ENOUGH_MONEY, 0, 0, 10, -1));
        assert_eq!((u32_at(&p, 6), u16_at(&p, 0xa)), (0, 0x3b));
    }

    #[test]
    fn recipient_and_gift_layouts() {
        let (id, p) = split(&recipient_reply(true));
        assert_eq!(
            (id, u32_at(&p, 6), u32_at(&p, 0xa), p.len()),
            (0x347, 0, 0x20, 0xe)
        );
        let (_, p) = split(&recipient_reply(false));
        assert_eq!((u32_at(&p, 6), u32_at(&p, 0xa)), (1, 0));
        let mut w = PktWriter::new(GIFT_SEND);
        w.i32(1475).wstr("Bob", 17).wstr("hi bob", 83);
        let p = w.into_payload();
        assert_eq!(p.len(), 0xd2);
        assert_eq!(
            parse_gift(&p),
            (1475, "Bob".to_string(), "hi bob".to_string())
        );
        let (id, p) = split(&gift_reply(false, reason::NO_SUCH_CHARACTER));
        assert_eq!((id, u32_at(&p, 6), u32_at(&p, 0xa)), (0x32b, 0, 0x56));
    }

    #[test]
    fn pe_build_day_reads_the_header_time_stamp() {
        let mut b = vec![0u8; 0x200];
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        b[0x88..0x8c].copy_from_slice(&(14_600u32 * 86_400 + 5).to_le_bytes());
        assert_eq!(pe_build_day(&b), Some(14_600));
        b[0x80] = b'X';
        assert_eq!(pe_build_day(&b), None);
        assert_eq!(pe_build_day(&[0u8; 4]), None);
    }

    /// LG-12: the 0x351 list (the ROM display flag, default rules) holds every item the 2009 shop lists, per
    /// category (item rows by the thousands of their display number for the product's sale type): the cell counts
    /// the 2009 client showed against this list (OBSERVED shop-guns/cpp/).
    #[test]
    fn rom_view_list_holds_every_shop_item_per_category() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = crate::rom::RomData::load(&rom);
        let cat = r.shop.catalog.as_ref().expect("shop list in the ROM");
        let mut cells: std::collections::BTreeMap<(i32, i32), std::collections::BTreeSet<i32>> =
            Default::default();
        for pid in cat.view_list() {
            let p = cat.product(pid).unwrap();
            let Some(row) = p.items.first().and_then(|(ix, _, _)| cat.info(*ix)) else {
                continue;
            };
            let display = if p.sale_type == 1 {
                row.nx_display
            } else {
                row.gp_display
            };
            if display > 0 {
                cells
                    .entry((p.sale_type, display / 1000))
                    .or_default()
                    .insert(row.itemid);
            }
        }
        let n = |sale: i32, g: i32| cells.get(&(sale, g)).map_or(0, |s| s.len());
        let gp: Vec<usize> = [0, 1, 2, 3, 4, 31, 5, 6, 7, 8, 9]
            .iter()
            .map(|g| n(0, *g))
            .collect();
        assert_eq!(gp, [47, 9, 5, 9, 7, 13, 12, 9, 4, 3, 2]);
        let mods: Vec<usize> = (0..6)
            .map(|k| (0..3).map(|j| n(0, 13 + 3 * k + j)).sum())
            .collect();
        assert_eq!(mods, [9, 6, 3, 9, 3, 9]);
        let credit: Vec<usize> = [10, 11, 12, 31].iter().map(|g| n(1, *g)).collect();
        assert_eq!(credit, [27, 8, 105, 13]);
    }

    #[test]
    fn rom_shop_tables_load_when_available() {
        let Some(rom) = dc_rom::test_rom() else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let r = crate::rom::RomData::load(&rom);
        let cat = r.shop.catalog.as_ref().expect("shop list in the ROM");
        let view = cat.view_list();
        assert!(view.len() > 100, "{}", view.len());
        assert!(r.shop.name_colours > 0 && r.shop.reticle_colours > 0);
        let y2005 = dc_bute::day_number(2005, 1, 1).unwrap();
        let y2030 = dc_bute::day_number(2030, 1, 1).unwrap();
        assert!(
            (y2005..y2030).contains(&r.shop.rom_build_day),
            "{}",
            r.shop.rom_build_day
        );
        // every shown product resolves to items; every shown case product has a ROM award pool
        let items = r.items.as_ref().expect("item table");
        let mut cases = 0;
        for pid in &view {
            let its = product_items(cat, cat.product(*pid).unwrap());
            assert!(!its.is_empty(), "product {pid}");
            if is_case(&its) {
                cases += 1;
                assert!(
                    !items.supply_pool(its[0].item_no as i32).is_empty(),
                    "case product {pid}"
                );
            }
        }
        assert!(cases > 0);
    }

    #[test]
    fn versions_start_at_one_and_grow_on_change() {
        let mut st = crate::state::testing::state();
        let v1 = versions(&st).unwrap();
        assert_eq!(v1.view, 1);
        assert!(v1.lists.iter().all(|&v| v == 1) && v1.prices == 1);
        st.rules.shop.list_override[0] = Some(vec![1, 2, 3]);
        st.rules.shop.price_override = vec![(1, 5)];
        let v2 = versions(&st).unwrap();
        assert_eq!((v2.view, v2.prices, v2.lists[0]), (2, 2, 1));
        assert_eq!(versions(&st).unwrap(), v2, "stable while unchanged");
        assert_eq!(raise_version(&st, KEY_SALE).unwrap(), 2);
        assert_eq!(
            versions(&st).unwrap().lists[2],
            2,
            "a raised version is kept"
        );
        let (id, p) = split(&version_push(KEY_SALE, 2).unwrap());
        assert_eq!((id, p.len(), u32_at(&p, 6)), (0x3058, 0xa, 2));
        assert_eq!(split(&version_push(KEY_PRICES, 1).unwrap()).0, 0x3059);
        assert!(version_push(99, 1).is_none());
    }
}

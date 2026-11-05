//! w3-shop: the shop screen's inbox page: the gifts and purchases the server holds for the character (0x32c / 0x32d),
//! four rows per page in the page's inbox list (window 5, list 1), each with the product's catalogue name, the
//! sender of a gift and its message.
//!
//! Open items (2009 evidence noted in DC_DEFAULTS.md CLI-549): Activate (1027) opening a row (0x32e / 0x32f with the
//! "use it now?" box and item use 0x330), the HiSec case box (window 10), coupons (Enter Code, 0x340 / 0x341) and the
//! megaphone composer (0x36c .. 0x36f).
// SPEC: PROTOCOL_2009.md §4.3 (0x32c / 0x32d rows; the inbox shows 4 rows per page)

use super::catalog::Catalog;
use super::link::InboxRow;
use super::lobby::OWN_COLOUR;
use super::pane;
use super::sheet::{image_fit, list_hit, list_row, place_text, Sheet, ALIGN_LEFT};
use super::storage::Wares;
use super::MenuData;
use crate::hud::items::DrawItem;
use dc_ui_data::pages::Align;
use dc_ui_data::WidgetKind;

/// The inbox page's list: window 5, list 1.
pub const INBOX_WINDOW: i32 = 5;
pub const INBOX_LIST: i32 = 1;
/// Rows per page.
// SPEC: PROTOCOL_2009.md §4.3 (inbox paging: 4 rows per page)
// EVIDENCE: cshell+0x148980 (rows 0..3 of list (5, 1) show entries i + page x 4)
pub const ROWS: usize = 4;
/// A row: a 0x205 x 0x47 button, Slot_Off normal, Slot_On pressed, GiftBox_Empty disabled (an empty row).
// DISASM: cshell+0x692e2..0x6938a (GiftBox_Slot_Off / _On, Hisec forms, GiftBox_Empty; rectangle 0 0 0x205 0x47)
// EVIDENCE: cshell+0x68cd0 (an empty row hides its parts and disables itself), cshell+0x69ff0 (a filled row enables)
pub const ROW_SIZE: [i32; 2] = [0x205, 0x47];
pub const SLOT_OFF: &str = "UI_SCR_Page_Shop\\Gift\\GiftBox_Slot_Off.dtx";
pub const SLOT_ON: &str = "UI_SCR_Page_Shop\\Gift\\GiftBox_Slot_On.dtx";
pub const SLOT_EMPTY: &str = "UI_SCR_Page_Shop\\Gift\\GiftBox_Empty.dtx";
/// The row forms of an entry with an end time (a temporary item): the HiSec slot art.
// DISASM: cshell+0x6930e / 0x69324 (GiftBox_Hisec_Slot_Off / _On into +0x240 / +0x244)
// EVIDENCE: cshell+0x69ff0 (the row's forms +0x17c / +0x180 from +0x240 / +0x244 when cshell+0x225400 finds the entry's end FILETIME +0xe0 a
// real date, else from the plain +0x238 / +0x23c)
// DC-DEFAULT: CLI-763 an end of 0 reads "no end" (the plain forms, OBSERVED shop-polish2/cpp_inbox:
// the reference stack shows plain rows for the server's end 0; cshell+0x225400's result for 0 depends on the server clock
// cshell+0x2252c0), a real date is one less than 10 years ahead
pub const HISEC_OFF: &str = "UI_SCR_Page_Shop\\Gift\\GiftBox_Hisec_Slot_Off.dtx";
pub const HISEC_ON: &str = "UI_SCR_Page_Shop\\Gift\\GiftBox_Hisec_Slot_On.dtx";
/// A row's parts (row offsets): item image 0xfe x 0x56 at (0, 0); name font 0xe size 0xe at (4, 5); the period mark
/// 0x32 x 0xd 5 px after the name; the date line at (0xed, 10); the message (0xffaaaaaa) at (0xed, 0x20); the class
/// tag at (4, 0x11); LOCKED at (4, 0x1a) and the type tag after it; the rank badge 0x1e x 0x16 at (8, 0x2a); under
/// the tags PERMANENT 0x41 x 10 and TEMPORARY 0x96 x 7 (HiSec 0x41 x 7 not drawn), each 1 px under the one before.
// EVIDENCE: cshell+0x692b0 (the row build: texts font 0xe align 0x11, the date line 0xfffead00, the message
// 0xffaaaaaa 0x113 wide; images M16A3_slot 0xfe x 0x56, GiftBox_1Day_Mark 0x32 x 0xd, Locked 0x2b, Rank 0x1e x 0x16,
// HiSec 0x41, TEMPORARY 0x96, Permenant 0x41 x 0xa), cshell+0x69ca0 (the places), cshell+0x69b00 (the mark stack),
// cshell+0x68df0 (the period mark 5 px after the name), cshell+0x69110 (LOCKED and the black picture for a locked
// item, the rank badge of its level), cshell+0x68f50 (the date line: string 0x18e6 "FROM : <sender> <date>" for a
// gift, else 0x374d3ec0 "^0xFFAAAAAA^<date>^"; MM/dd/yyyy HH:mm:ss of the received FILETIME)
// DC-DEFAULT: CLI-763 the name in 0xfffead00 (the build sets no colour on it; OBSERVED cpp_inbox/shop_bb00_inbox_p1.png
// names in (254, 173, 0))
pub const ROW_IMAGE_SIZE: [i32; 2] = [0xfe, 0x56];
pub const NAME_AT: [i32; 2] = [4, 5];
pub const NAME_COLOUR: u32 = 0xfffe_ad00;
pub const MARK_SIZE: [i32; 2] = [0x32, 0xd];
pub const DATE_AT: [i32; 2] = [0xed, 10];
pub const MESSAGE_AT: [i32; 2] = [0xed, 0x20];
pub const MESSAGE_COLOUR: u32 = 0xffaa_aaaa;
pub const ROW_FONT: i32 = 0xe;
pub const CLASS_TAG_AT: [i32; 2] = [4, 0x11];
pub const LOCKED_AT: [i32; 2] = [4, 0x1a];
pub const BADGE_AT: [i32; 2] = [8, 0x2a];
pub const HISEC_MARK: (&str, [i32; 2]) = (
    "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_HiSec.dtx",
    [0x41, 7],
);
pub const TEMPORARY_MARK: (&str, [i32; 2]) = (
    "UI_SCR_Page_Shop\\shop\\Item_Info_Mark_TEMPORARY.dtx",
    [0x96, 7],
);
/// The gift line's form and the plain date form.
pub const FROM_TEXT: i32 = 0x18e6;
pub const DATE_FORM: &str = "^0xFFAAAAAA^%s^";
/// The left pane's list of the inbox page (window 2, list 6): four MyItemSlot rows holding the character's
/// HiSec keys (item SubType 0x13); string 0x18be on the first row when there is none.
// EVIDENCE: cshell+0x1425d0 (list (2, 6): four MyItemSlot_Off / _On rows 0xc2 x 0x47, string 0x18be), cshell+0x149a20
// (inbox page: list 6 shown, rows filled with the items whose type is 0x13, the rest hidden, row 0 shows 0x18be
// when empty; lists 2 .. 5 hidden)
pub const KEY_ROWS_LIST: i32 = 6;
/// The record field the filter reads is +0x14, the item table's `SubType`: 0x13 is the HiSec key.
// EVIDENCE: cshell+0x149a20 (`record +0x14 == 0x13` on the record found by cshell+0x35c480(item number, type));
// HISEC_2009.md 4.1 (a HiSec key is a type 0x15 item of SubType 0x13). CLI-901: the earlier reading took +0x14 for the
// item type, which listed every type 0x13 item (the clip options, e.g. an extended magazine) in the key list.
pub const KEY_SUB_TYPE: i32 = 0x13;
pub const NO_ITEM_TEXT: i32 = 0x18be;
/// The info box under the pager (the shop's detail panel; its parts hidden while no row is selected).
// EVIDENCE: cshell+0x140d80 (row 0 selected on open, else the info box's parts hidden through cshell+0xd5e00)
pub const INFO_FRAME: (&str, [i32; 2]) = super::shop::DETAIL_FRAME;
/// A gift's sender line colour, an own purchase's.
pub const GIFT_COLOUR: u32 = OWN_COLOUR;

const ALIGN_TL: Align = Align {
    first: ALIGN_LEFT,
    second: 0,
};

/// The second line of a row: `<sender> : <message>` for a gift, the message alone for an own purchase.
pub fn sender_line(r: &InboxRow) -> String {
    if r.gift_kind != 0 && !r.sender.is_empty() {
        format!("{} : {}", r.sender, r.message)
    } else {
        r.message.clone()
    }
}

/// A FILETIME as `MM/dd/yyyy HH:mm:ss` (UTC, as FileTimeToSystemTime reads it).
pub fn filetime_text(ft: u64) -> String {
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil date of a day count since 1970-01-01
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{m:02}/{d:02}/{y:04} {:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Whether an entry's end FILETIME is a real date (a temporary item): not 0 and less than 10 years after `now`
/// (a FILETIME too).
pub fn has_end(end: u64, now: u64) -> bool {
    const TEN_YEARS: u64 = 10 * 365 * 86_400 * 10_000_000;
    end != 0 && end < now.saturating_add(TEN_YEARS)
}

/// The period mark of a product (consume type, period): single use and counts `<n>X`, hours `<n>h`, days `<n>Day`;
/// a product without a period shows `1X` and PERMANENT.
// EVIDENCE: cshell+0x69ff0 (consume 2 / 4 / 6 -> one use, 3 -> a count of the period, 5 -> hours, else days),
// cshell+0x68df0 (GiftBox_%dX_Mark / GiftBox_%dh_Mark / GiftBox_%dDay_Mark; days 0 or 999: GiftBox_1X_Mark and the
// PERMANENT mark shown)
// The reference client draws "7X" for a consume-2 product of period 7 (cpp_inbox/shop_bb00_inbox_p1.png, Supply Case
// AK-N); DeCombat follows the 2009 table (one use: 1X).
pub fn period_mark(consume: i32, period: i32) -> (String, bool) {
    let (days, count, hours) = match consume {
        2 | 4 | 6 => (0, 1, 0),
        3 => (0, period, 0),
        5 => (0, 0, period),
        _ => (period, 0, 0),
    };
    let base = "UI_SCR_Page_Shop\\Gift\\GiftBox_";
    if count >= 1 {
        (format!("{base}{count}X_Mark.dtx"), false)
    } else if hours >= 1 {
        (format!("{base}{hours}h_Mark.dtx"), false)
    } else if days == 0 || days == 999 {
        (format!("{base}1X_Mark.dtx"), true)
    } else {
        (format!("{base}{days}Day_Mark.dtx"), false)
    }
}

/// The current time as a FILETIME.
pub fn now_filetime() -> u64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    (unix + 11_644_473_600) * 10_000_000
}

/// Pages of the inbox by the server's total (0 for an empty inbox: every page button disabled).
// EVIDENCE: cshell+0x1482b0 (inbox page: page button k enabled while k x 4 < the inbox total)
pub fn page_count(data: &MenuData) -> usize {
    let n = (data.store.inbox_total as usize).max(data.store.inbox.len());
    n.div_ceil(ROWS)
}

/// The rows of page `page` held.
pub fn rows_on(data: &MenuData, page: usize) -> &[InboxRow] {
    let inbox = &data.store.inbox;
    let a = (page * ROWS).min(inbox.len());
    &inbox[a..(a + ROWS).min(inbox.len())]
}

/// The inbox row of page `page` under `p` (of the rows held).
pub fn row_hit(sheet: &Sheet, data: &MenuData, page: usize, p: [f32; 2]) -> Option<usize> {
    let list = sheet.part(INBOX_WINDOW, INBOX_LIST, WidgetKind::List)?;
    list_hit(
        list,
        ROW_SIZE[0],
        ROW_SIZE[1],
        rows_on(data, page).len(),
        0,
        p,
    )
}

/// Draws the four rows of page `page` (`selected` pressed; a row without an entry is the empty slot) and the info
/// box.
pub fn draw(
    cat: &mut Catalog,
    sheet: &Sheet,
    wares: &Wares,
    data: &MenuData,
    page: usize,
    selected: Option<usize>,
    out: &mut Vec<DrawItem>,
) {
    let Some(list) = sheet.part(INBOX_WINDOW, INBOX_LIST, WidgetKind::List) else {
        return;
    };
    let rows = rows_on(data, page);
    let now = now_filetime();
    let art = cat.rom.as_deref().and_then(super::lobby::PanelArt::cached);
    for i in 0..ROWS {
        let Some(at) = list_row(list, ROW_SIZE[1], i, 0) else {
            break;
        };
        let Some(r) = rows.get(i) else {
            out.push(image_fit(cat, SLOT_EMPTY, at, ROW_SIZE));
            continue;
        };
        let lit = selected == Some(i);
        let forms = if has_end(r.end, now) {
            [HISEC_OFF, HISEC_ON]
        } else {
            [SLOT_OFF, SLOT_ON]
        };
        out.push(image_fit(cat, forms[lit as usize], at, ROW_SIZE));
        draw_row(
            cat,
            wares,
            data,
            r,
            at,
            art.as_ref().map(|a| &a.levels),
            out,
        );
    }
    out.push(image_fit(
        cat,
        INFO_FRAME.0,
        super::shop::DETAIL_AT,
        INFO_FRAME.1,
    ));
    let ware = selected
        .and_then(|i| rows.get(i))
        .and_then(|r| wares.ware(r.product, &data.store.prices));
    if let Some(w) = ware {
        let marks = super::shop::DetailMarks {
            locked: super::shop::locked(w.unlock_exp, super::shop::lock_exp(data)),
            temporary: selected
                .and_then(|i| rows.get(i))
                .is_some_and(|r| has_end(r.end, now)),
        };
        super::shop::draw_detail(cat, &w, wares.art_of(w.item_no), marks, out);
        if let Some(end) = selected
            .and_then(|i| rows.get(i))
            .filter(|r| has_end(r.end, now))
            .map(|r| r.end)
        {
            let line = format!("{}{}", cat.text(0x18e8), super::shop::filetime_text(end));
            super::shop::draw_period_line(&line, out);
        }
    }
}

/// Draws the parts of an inbox row at `at`.
fn draw_row(
    cat: &mut Catalog,
    wares: &Wares,
    data: &MenuData,
    r: &InboxRow,
    at: [i32; 2],
    levels: Option<&crate::hud::board::LevelTable>,
    out: &mut Vec<DrawItem>,
) {
    let p = |o: [i32; 2]| [at[0] + o[0], at[1] + o[1]];
    let ware = wares.ware(r.product, &data.store.prices);
    let locked = ware
        .as_ref()
        .is_some_and(|w| super::shop::locked(w.unlock_exp, super::shop::lock_exp(data)));
    if let Some(icon) = ware
        .as_ref()
        .and_then(|w| w.icon.clone())
        .filter(|t| cat.tex_size(t).is_some())
    {
        // the slot image in the 0xfe x 0x56 control, as the shop cells draw theirs (CLI-697)
        // EVIDENCE: cshell+0x692b0 (the image control is set with the source rectangle (0, 0, 0xfe, 0x56) and the
        // same size: the texture's top-left 0xfe x 0x56 at texel scale), cshell+0x69eb0 / 0x343680 (a row's picture
        // only swaps the texture, the rectangle stays). The reference client draws these images about 0.85 x smaller
        // (cpp_inbox/shop_bb00_inbox_p1.png); the 2009 code is followed (CLI-834).
        let mut img = image_fit(cat, &icon, at, ROW_IMAGE_SIZE);
        if locked {
            if let DrawItem::Image { tint, .. } = &mut img {
                *tint = super::shop::LOCKED_TINT;
            }
        }
        out.push(img);
    }
    let name = ware
        .as_ref()
        .map_or_else(|| format!("#{}", r.product), |w| w.name.clone());
    out.push(place_text(
        &name,
        p(NAME_AT),
        [0, 0],
        ALIGN_TL,
        ROW_FONT,
        0xe,
        NAME_COLOUR,
    ));
    let mut permanent = false;
    if let Some(w) = &ware {
        let (mark, perm) = period_mark(w.consume, w.period);
        permanent = perm;
        let wn = cat
            .measure_text(ROW_FONT, 0xe, &name)
            .map_or(name.chars().count() as f32 * 7.0, |m| m.0)
            .round() as i32;
        out.push(image_fit(
            cat,
            &mark,
            p([NAME_AT[0] + wn + 5, NAME_AT[1]]),
            MARK_SIZE,
        ));
    }
    // the date line and the message
    let date = filetime_text(r.received);
    let line = if r.gift_kind != 0 {
        cat.text(FROM_TEXT)
            .replacen("%s", &r.sender, 1)
            .replacen("%s", &date, 1)
    } else {
        DATE_FORM.replacen("%s", &date, 1)
    };
    let mut x = at[0] + DATE_AT[0];
    for (text, colour) in super::shop::colour_runs(&line, NAME_COLOUR) {
        out.push(place_text(
            &text,
            [x, at[1] + DATE_AT[1]],
            [0, 0],
            ALIGN_TL,
            ROW_FONT,
            0xe,
            colour,
        ));
        x += cat
            .measure_text(ROW_FONT, 0xe, &text)
            .map_or(text.chars().count() as f32 * 7.0, |m| m.0)
            .round() as i32;
    }
    if !r.message.is_empty() {
        out.push(place_text(
            &r.message,
            p(MESSAGE_AT),
            [0, 0],
            ALIGN_TL,
            ROW_FONT,
            0xe,
            MESSAGE_COLOUR,
        ));
    }
    // tags, LOCKED, badge and the mark stack under them
    let Some(w) = ware else {
        return;
    };
    let class = w.kind_of.and_then(super::shop::class_tag);
    let mut y = CLASS_TAG_AT[1] + 1;
    if let Some((t, size)) = class {
        out.push(image_fit(cat, t, p(CLASS_TAG_AT), size));
        y += size[1];
    }
    let mut x = LOCKED_AT[0];
    if locked {
        out.push(image_fit(
            cat,
            super::shop::LOCKED_TAG,
            p(LOCKED_AT),
            super::shop::LOCKED_SIZE,
        ));
        x += super::shop::LOCKED_SIZE[0] + 2;
    }
    if let Some((t, size)) = w.tag_kind.and_then(super::shop::type_tag) {
        out.push(image_fit(cat, t, p([x, LOCKED_AT[1]]), size));
        y = LOCKED_AT[1] + size[1] + 1;
    }
    if let Some(b) = levels.and_then(|l| super::shop::badge_of(l, w.unlock_exp).map(str::to_string))
    {
        out.push(image_fit(cat, &b, p(BADGE_AT), [0x1e, 0x16]));
    }
    // the HiSec mark (+0x218) between PERMANENT (+0x220) and TEMPORARY (+0x21c) in the stack
    // EVIDENCE: cshell+0x69b00 (the stack order +0x220, +0x218, +0x21c), cshell+0x69ff0 (HiSec shown for a type 0x14
    // item whose record's first value is not -1: cshell+0x69e80)
    let hisec = w.kind_of == Some(0x14) && wares.art_of(w.item_no).is_some_and(|a| a.keyed_case);
    let marks = [
        (permanent, super::shop::PERMANENT_MARK),
        (hisec, HISEC_MARK),
        (has_end(r.end, now_filetime()), TEMPORARY_MARK),
    ];
    for (on, (t, size)) in marks {
        if on {
            out.push(image_fit(cat, t, p([CLASS_TAG_AT[0], y]), size));
            y += size[1] + 1;
        }
    }
}

/// Draws the left pane's function item rows of the inbox page.
pub fn draw_items(
    cat: &mut Catalog,
    sheet: &Sheet,
    wares: &Wares,
    data: &MenuData,
    out: &mut Vec<DrawItem>,
) {
    let Some(list) = sheet.part(super::shop::LEFT_WINDOW, KEY_ROWS_LIST, WidgetKind::List) else {
        return;
    };
    let items: Vec<i32> = data
        .store
        .items
        .iter()
        .map(|r| r.item_no as i32)
        .filter(|n| wares.art_of(*n).is_some_and(|a| a.sub_type == KEY_SUB_TYPE))
        .collect();
    // EVIDENCE: cshell+0x149a20 (the items of type 0x13 sorted through cshell+0x146ef0), DISASM cshell+0x145a2e ..
    // 0x145a6d (the order: item record type +0x14, then ItemID +0), before the first four are shown
    let mut items = items;
    items.sort();
    items.truncate(ROWS);
    let rows: Vec<Option<i32>> = if items.is_empty() {
        vec![None]
    } else {
        items.into_iter().map(Some).collect()
    };
    for (i, item) in rows.iter().enumerate() {
        let Some(at) = list_row(list, pane::ROW_SIZE[1], i, 0) else {
            break;
        };
        out.push(image_fit(cat, pane::BODY_ROW_IMAGE, at, pane::ROW_SIZE));
        let name = match item {
            Some(n) => {
                if let Some(icon) = wares
                    .art_of(*n)
                    .map(|a| a.icon.clone())
                    .filter(|t| cat.tex_size(t).is_some())
                {
                    out.push(image_fit(
                        cat,
                        &icon,
                        [at[0] + pane::ROW_IMAGE_DX, at[1]],
                        pane::ROW_IMAGE_SIZE,
                    ));
                }
                wares.label_of(*n)
            }
            None => cat.text(NO_ITEM_TEXT),
        };
        out.push(place_text(
            &name,
            [at[0] + pane::ROW_TEXT_AT[0], at[1] + pane::ROW_TEXT_AT[1]],
            [0, 0],
            ALIGN_TL,
            pane::ROW_FONT,
            pane::ROW_FONT_SIZE,
            pane::ROW_COLOUR,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gifts_name_their_sender() {
        let mut r = InboxRow {
            sender: "Ann".into(),
            message: "hi".into(),
            gift_kind: 1,
            ..InboxRow::default()
        };
        assert_eq!(sender_line(&r), "Ann : hi");
        r.gift_kind = 0;
        assert_eq!(sender_line(&r), "hi");
    }
}

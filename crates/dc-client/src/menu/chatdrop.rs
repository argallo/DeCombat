//! The chat page's two combo lists (lobby and waiting room): the chat filter (commands 0x274 / 0x275) and the chat
//! target (0x278 / 0x279). A press on the combo (caption or arrow) shows its list, or hides it when shown; a row sets
//! the filter or the target and the combo's caption.

use super::dropdown::{self, Dropdown};
use super::sheet::Sheet;
use super::MenuData;
use crate::menu::catalog::Catalog;
use dc_ui_data::WidgetKind;

/// The combos' commands (their arrow buttons and captions carry them in the chat page).
// EVIDENCE: cshell+0x1bf20 (0x274 shows the filter list, 0x278 toggles the target list)
pub const CMD_FILTER: i32 = 0x274;
pub const CMD_TARGET: i32 = 0x278;
pub const LIST_FILTER: &str = "chat_filter";
pub const LIST_TARGET: &str = "chat_target";

/// The chat target values: the channel chat and the clan chat.
// EVIDENCE: cshell+0x1bf20 (0x279: row 0 sets the mode 1, row 1 sets 0x1000), cshell+0x1e880
pub const TARGET_CHANNEL: i32 = 1;
pub const TARGET_CLAN: i32 = 0x1000;
/// The chat filter values: all lines (0), the channel lines, the clan lines.
// EVIDENCE: cshell+0x1bf20 (0x275: row 1 sets the filter 1, row 2 sets 0x1000, row 0 keeps the default)
pub const FILTER_ALL: i32 = 0;
pub const FILTER_CHANNEL: i32 = 1;
pub const FILTER_CLAN: i32 = 0x1000;

/// Row strings: all / channel only / clan only; channel chat (F2) / clan chat (F3).
// EVIDENCE: cshell+0x1e880 (the lists' rows in that order: 0x189b, 0x189d, 0x189c and 0x18b7, 0x18b6, size 0xe)
const FILTER_ROWS: [(i32, i32); 3] = [
    (0x189b, FILTER_ALL),
    (0x189d, FILTER_CHANNEL),
    (0x189c, FILTER_CLAN),
];
const TARGET_ROWS: [(i32, i32); 2] = [(0x18b7, TARGET_CHANNEL), (0x18b6, TARGET_CLAN)];
/// The lists' widths (the frame texture's hand-over in the build: 0x9d and 0x95).
// EVIDENCE: cshell+0x1e880 (FUN_3743c750(0x9d) for the filter list, (0x95) for the target list)
const FILTER_WIDTH: i32 = 0x9d;
const TARGET_WIDTH: i32 = 0x95;
/// The combo frame's height and its top above the arrow button.
// DC-DEFAULT: CLI-870 (proposed) the combo box frame is 21 px high and 3 px above its arrow button (the reference stack's
// picture); the list hangs under it, right-aligned with the arrow
const COMBO_HEIGHT: i32 = 21;
const COMBO_ABOVE: i32 = 3;

/// The combo (its command) under `p`: the arrow button or the caption's frame.
pub fn combo_at(sheet: &Sheet, p: [f32; 2]) -> Option<i32> {
    [CMD_FILTER, CMD_TARGET].into_iter().find(|&c| {
        anchor(sheet, c).is_some_and(|a| {
            p[0] >= a[0] as f32
                && p[0] < (a[0] + a[2]) as f32
                && p[1] >= a[1] as f32
                && p[1] < (a[1] + a[3]) as f32
        })
    })
}

fn anchor(sheet: &Sheet, cmd: i32) -> Option<[i32; 4]> {
    let arrow = sheet
        .parts
        .iter()
        .find(|p| p.kind == WidgetKind::Button && p.command == cmd)?;
    let w = if cmd == CMD_FILTER {
        FILTER_WIDTH
    } else {
        TARGET_WIDTH
    };
    Some([
        arrow.pos[0] + arrow.size[0] - w,
        arrow.pos[1] - COMBO_ABOVE,
        w,
        COMBO_HEIGHT,
    ])
}

/// The list of combo `cmd`.
pub fn open(sheet: &Sheet, cat: &Catalog, cmd: i32) -> Option<Dropdown> {
    let (list, rows): (&'static str, &[(i32, i32)]) = if cmd == CMD_FILTER {
        (LIST_FILTER, &FILTER_ROWS)
    } else {
        (LIST_TARGET, &TARGET_ROWS)
    };
    let rows = rows.iter().map(|(t, v)| (cat.text(*t), *v)).collect();
    let mut d = Dropdown::new(
        list,
        anchor(sheet, cmd)?,
        rows,
        dropdown::ROOM_ROW_SIZE,
        false,
    );
    d.left = true;
    Some(d)
}

/// A row of a chat list was picked: the value and the combo's caption.
pub fn pick(sheet: &mut Sheet, cat: &Catalog, data: &mut MenuData, list: &str, value: i32) {
    let (cmd, rows) = match list {
        LIST_FILTER => {
            data.chat_filter = value;
            (CMD_FILTER, &FILTER_ROWS[..])
        }
        LIST_TARGET => {
            data.chat_target = value;
            (CMD_TARGET, &TARGET_ROWS[..])
        }
        _ => return,
    };
    if let Some((t, _)) = rows.iter().find(|(_, v)| *v == value) {
        let text = cat.text(*t);
        for p in sheet
            .parts
            .iter_mut()
            .filter(|p| p.kind == WidgetKind::Text && p.command == cmd)
        {
            p.text = text.clone();
        }
    }
}

/// CLI-1173 (playtest report: "switching to clan or channel chat does not change"): F2 / F3 pick the target list's
/// first / second row (channel chat, clan chat), as the combo's rows do, while the chat filter shows every line; the
/// combo captions name the keys. `false`: the key does nothing here.
// DISASM: cshell+0x205e0 (the chat page's key handler: key 0x71 -> command 0x279 with row 0, key 0x72 -> 0x279 with
// row 1, both only while the global at 0x37541720 is 0x1111)
// DC-DEFAULT: CLI-1173 (proposed) that global is read as the chat filter's every-line state (it is handed to the chat
// list with the filter calls, cshell+0x1bb70 / cshell+0x198d0)
pub fn key_target(
    sheet: &mut Sheet,
    cat: &Catalog,
    data: &mut MenuData,
    key: super::MenuKey,
) -> bool {
    let row = match key {
        super::MenuKey::F2 => 0,
        super::MenuKey::F3 => 1,
        _ => return false,
    };
    if data.chat_filter != FILTER_ALL {
        return false;
    }
    pick(sheet, cat, data, LIST_TARGET, TARGET_ROWS[row].1);
    true
}

/// Whether `list` is one of the chat lists.
pub fn is_chat_list(list: &str) -> bool {
    list == LIST_FILTER || list == LIST_TARGET
}

//! What the menu screens draw with, read from the user's ROM at run time: the front-end layout pages, the string
//! table, the layout table's per-screen sections, the room table (map and mode names), the texture names the
//! 2009 shell has in its code, the fonts and the textures.
//!
//! Nothing of it is in this tree: code literals are read from the user's `CShell.dll` by rva and accepted only
//! when length and SHA-256 prefix match the pin ([`dc_ui_data::names::read_pinned`]); keys of the layout and room
//! tables are found by the SHA-256 prefix and length of the key text ([`KeyPin`]).
// SPEC: FMT_CRES_UI.md §1 (where layout lives), §3.2 (string table), §6.2 (layout sections), §6.3 (pages), §7.1
// EVIDENCE: cshell+0x1d2670 (pages), cshell+0x64770 (screen table = section names)

use crate::hud::compose::{compose, HudAssets, HudFonts, NoTextures, RomTextureSource};
use crate::hud::items::DrawItem;
use crate::overlay::text::RgbaImage;
use dc_rom::PeFile;
use dc_ui_data::names::{matches_pin, read_pinned, Pin, CSHELL_REL_PATH};
use dc_ui_data::{
    FontResolver, LayoutTable, ScreenPages, SystemFonts, UiNames, UiScreen, UiStrings,
    VirtualScaler,
};
use std::path::{Path, PathBuf};

/// A key of a ROM table addressed by the SHA-256 prefix (16 hex digits) and the length of its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPin {
    pub sha16: &'static str,
    pub len: usize,
}

pub const fn key(sha16: &'static str, len: usize) -> KeyPin {
    KeyPin { sha16, len }
}

/// The value of the first key of `entries` that matches `k`.
pub fn find_key<'a, K, V, I>(entries: I, k: &KeyPin) -> Option<&'a str>
where
    I: IntoIterator<Item = (&'a K, &'a V)>,
    K: AsRef<str> + ?Sized + 'a,
    V: AsRef<str> + ?Sized + 'a,
{
    entries
        .into_iter()
        .find(|(name, _)| matches_pin(name.as_ref(), k.sha16, k.len))
        .map(|(_, v)| v.as_ref())
}

/// `0xAARRGGBB` to straight RGBA bytes.
pub fn argb(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
}

/// Opaque `0xFFRRGGBB` from a layout colour `<r,g,b>`.
pub fn rgb_word(v: [f64; 3]) -> u32 {
    let c = |x: f64| x.clamp(0.0, 255.0) as u32;
    0xff00_0000 | (c(v[0]) << 16) | (c(v[1]) << 8) | c(v[2])
}

/// Integers of a layout value: `n`, `(a,b)`, `(a,b,c,d)` or `<a,b,c>`.
pub fn ints(v: &str) -> Vec<i32> {
    v.split(|c: char| !(c.is_ascii_digit() || c == '-' || c == '.'))
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .map(|f| f as i32)
        .collect()
}

pub fn pair(v: Option<&str>) -> Option<[i32; 2]> {
    let n = ints(v?);
    (n.len() >= 2).then(|| [n[0], n[1]])
}

pub fn int(v: Option<&str>) -> Option<i32> {
    ints(v?).first().copied()
}

pub fn colour(v: Option<&str>) -> Option<u32> {
    let n = ints(v?);
    (n.len() >= 3).then(|| rgb_word([n[0] as f64, n[1] as f64, n[2] as f64]))
}

/// Texture names in the 2009 shell's code (rva in `CShell.dll`, SHA-256 prefix, length).
pub mod pins {
    use super::Pin;

    const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
        Pin {
            what,
            rva,
            sha16,
            len,
        }
    }

    /// Tile prefix of the login screen's background (3 rows, 4 columns).
    // DISASM: cshell+0x11ce43 (push of this rva before the tiler call cshell+0x11ce58 -> cshell+0x1dbe80)
    pub const LOGIN_BACK: Pin = pin("login background tiles", 0x3e1264, "73e306b53aa07648", 33);
    /// Tile prefix of the server and channel select screens' background (3 rows, 4 columns).
    // EVIDENCE: cshell+0x13a500 (tiler call with 0, 0, 3, 4 and this literal)
    pub const SERVER_BACK: Pin = pin(
        "server select background tiles",
        0x3e3754,
        "78652e3f46a21a46",
        33,
    );
    /// Button prefix of the server group buttons (state letter + extension appended).
    // EVIDENCE: cshell+0x13b520
    pub const GROUP_BUTTON: Pin = pin(
        "server group button prefix",
        0x3e3874,
        "04193c6628fdf48c",
        43,
    );
    /// Load images of a server row: low, moderate, busy, full; then the "checking" image.
    // EVIDENCE: cshell+0x13b750 (thresholds 31 / 81 / 96 per cent), cshell+0xfa860 (channel rows)
    pub const STATE_IMAGES: [Pin; 5] = [
        pin("server load image low", 0x3dde38, "3096490ac137b028", 45),
        pin(
            "server load image moderate",
            0x3dde04,
            "32528b08cb663de0",
            46,
        ),
        pin("server load image busy", 0x3dddd0, "c924942623b21510", 44),
        pin("server load image full", 0x3ddda0, "e620efff0b474643", 44),
        pin("server checking image", 0x3ddd6c, "1f122e3bc05967fb", 48),
    ];
    /// Row icons: connectable, not connectable.
    // EVIDENCE: cshell+0x13b750 (the second one above 100 per cent load)
    pub const STATE_OK: Pin = pin("server ok icon", 0x3dde6c, "105b6b5e3f2319da", 42);
    pub const STATE_NO: Pin = pin("server refused icon", 0x3ddd40, "96126a40681d5e08", 42);
    /// The message box window (472 x 256) and its button textures (normal, over, down).
    // EVIDENCE: cshell+0x26f3c0 (box build: window 0x1d8 x 0x100, button textures, warning icon)
    pub const POPUP: Pin = pin("message box window", 0x3ed944, "0d104f95ef3a7368", 35);
    pub const POPUP_BUTTONS: [Pin; 3] = [
        pin(
            "message box button normal",
            0x3eda10,
            "6ac67d1865cd5011",
            45,
        ),
        pin("message box button over", 0x3ed9e0, "415f167cb1c8f81b", 45),
        pin("message box button down", 0x3ed9b0, "4484fc2f2abb5c40", 45),
    ];
    pub const WARNING: Pin = pin("message box warning icon", 0x3f4f1c, "a0a6b5dcfc19ff5a", 35);
    /// The OK icon the box builder loads beside the warning icon (the box's second icon control, +0x38).
    // EVIDENCE: cshell+0x26f3c0 (`interface\screen\ICON\ok_icon.dtx` for the control at +0x38)
    pub const OK_ICON: Pin = pin("message box ok icon", 0x3f4ef8, "cda5bf5840ecabbd", 33);
    /// The result screen's page name and its background tile prefix (3 rows, 4 columns).
    // EVIDENCE: cshell+0x132350 (the result screen's build: page by this name, then the tiler with 0, 0, 3, 4 and
    // the prefix)
    pub const RESULT_PAGE: Pin = pin("result page name", 0x3e2eb0, "4d02631f90178714", 11);
    pub const RESULT_BACK: Pin = pin("result background tiles", 0x3e2e7c, "b59d2d49b0d87ef4", 48);
    /// The room list screen's own textures for the chat page's Enter button, in the order its build hands them to
    /// the button's texture setter (state letters A, B, D, C of one prefix).
    // EVIDENCE: cshell+0x1211b0 (room list screen build: chat window 2 control 0 -> vtable +0x164 with these four
    // literals, VA 0x374e1930 / 0x374e18fc / 0x374e18c8 / 0x374e1894)
    pub const LOBBY_ENTER: [Pin; 4] = [
        pin("lobby chat enter normal", 0x3e1930, "2c77eff2e7b27ebf", 49),
        pin("lobby chat enter over", 0x3e18fc, "3a1e05a950649826", 49),
        pin("lobby chat enter third", 0x3e18c8, "b62df972895c061d", 49),
        pin("lobby chat enter fourth", 0x3e1894, "19b2d4275eba608b", 49),
    ];
    /// The waiting room's goal form of mode row 4 (a wide printf form: the goal, then a points word).
    // EVIDENCE: cshell+0x2623f0 (row 4: swprintf with the wide literal at cshell+0x3f4334 and the goal)
    pub const GOAL_POINTS_FORM: Pin =
        pin("goal points form (wide)", 0x3f4334, "0111ecb9257ec5bc", 9);
    /// w2-lobbychat: the chat window's panel per screen: the room list's narrow one, the waiting room's wide one.
    // EVIDENCE: cshell+0x1bb70 (chat page shown: screen 0xf -> the first literal, screen 9 -> the second, handed to
    // window 2's texture setter cshell+0x2f8270)
    pub const CHAT_PANELS: [Pin; 2] = [
        pin("room list chat panel", 0x3cd900, "9e5a7598df5bb14a", 48),
        pin("waiting room chat panel", 0x3cd8d4, "729659a5f77a753e", 42),
    ];
}

/// w3-options: literals of the options screen, the report box and the help box (textures, the option pages'
/// numbered layout keys, the report box's own texts).
pub mod overlay_pins {
    use super::Pin;

    const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
        Pin {
            what,
            rva,
            sha16,
            len,
        }
    }

    /// The option rows' control textures: on / off button, slider line, slider knob, minus, plus (state letter A
    /// of the knob, minus and plus: their prefix is the literal without `A.dtx`), dropdown frame and arrow, key box.
    // EVIDENCE: cshell+0xf1e10 (row element build by type: 1 dropdown 0x73 x 0x17, 2 / 3 / 9 on-off buttons, 4
    // slider 200 x 8 with a knob and 0x11 x 0x11 minus / plus), cshell+0xf25f0 (key boxes 0x41 x 0x17)
    pub const RADIO_ON: Pin = pin("option on button", 0x3dd768, "208e3e2a9134738c", 34);
    pub const RADIO_OFF: Pin = pin("option off button", 0x3dd744, "f822b3acb166a756", 35);
    pub const LINE: Pin = pin("option slider line", 0x3dd6d0, "55d11b4f6e1d9fd8", 27);
    pub const KNOB: Pin = pin("option slider knob", 0x3dd6a8, "6ddc90898584019d", 38);
    pub const MINUS: Pin = pin("option minus button", 0x3dd634, "c0bb3fc95c60922a", 32);
    pub const PLUS: Pin = pin("option plus button", 0x3dd5cc, "ca0d2a6978f77181", 31);
    pub const DROPDOWN: Pin = pin("option dropdown frame", 0x3dd564, "5ff11b8d620f30ca", 38);
    pub const DROPDOWN_ARROW: Pin = pin("option dropdown arrow", 0x3dd540, "21fb0af32743a6a9", 35);
    pub const KEY_BOX: Pin = pin("option key box", 0x3dd78c, "4d095946b9458f36", 40);
    /// The option pages' numbered layout keys (`<literal><n>`) and their count key.
    // EVIDENCE: cshell+0x112200, cshell+0x151510, cshell+0x117ad0 (`%s%d` of these literals, looked up in the page's
    // layout section)
    pub const ELEMENT_COUNT: Pin =
        pin("option element count key", 0x3dfe44, "a7553d059c4ff199", 19);
    /// The graphic page's second count key: the rows of its full list (the first count gives its short list).
    // EVIDENCE: cshell+0x117ad0 (two passes over the rows: the first count's rows into list 1 of window 2, this
    // count's rows into list 2)
    pub const ELEMENT_ALL_COUNT: Pin = pin(
        "option full element count key",
        0x3e08cc,
        "3f08d9360d6f752d",
        15,
    );
    pub const ELEMENT_KEYS: [Pin; 6] = [
        pin("option element type key", 0x3dfe38, "f190dcf039c46a14", 11),
        pin(
            "option element height key",
            0x3dfe28,
            "f16444f3442aad0f",
            13,
        ),
        pin("option element index key", 0x3dfe18, "a1dbe4f98eefeec8", 12),
        pin("option command index key", 0x3dfde0, "5c762d2ea3a109a3", 12),
        pin("option element blank key", 0x3dfdbc, "70384883517f7e92", 12),
        pin(
            "option element offset key",
            0x3dfdcc,
            "7970c0de73878c34",
            16,
        ),
    ];
    /// The full-screen blind under the report box and the report box's window, dropdown button prefix, find button
    /// prefix and send / cancel button prefix.
    // EVIDENCE: cshell+0x274160 (report box build: blind 0x400 x 0x300, window texture, the buttons' prefixes)
    pub const BLIND: Pin = pin("screen blind", 0x3de1c8, "345a622c5aaec3e0", 27);
    pub const REPORT_WINDOW: Pin = pin("report window", 0x3f5694, "e58d05c565af6313", 39);
    pub const REPORT_DROPDOWN: Pin =
        pin("report dropdown button", 0x3f55f8, "a128ca26062fe4bb", 37);
    pub const REPORT_FIND: Pin = pin("report find button", 0x3f5564, "39fe82e3a3ca5d96", 37);
    /// w4-charinfo (CLI-931): the Character Info box's textures (panel of the default page, panel of the fireteam
    /// page, the first rank icon, the close button's first state, the tab buttons' selected and normal textures) and
    /// its two tab captions (wide literals).
    // EVIDENCE: cshell+0x240040 (CCharacterInfoBox::Init: every texture below is loaded there with the size named
    // in the call; the tab captions are handed to the button caption setter)
    pub const CI_PANEL: Pin = pin("char info panel", 0x3f26d0, "5bdd6dd61c2a3e57", 52);
    pub const CI_PANEL2: Pin = pin("char info fireteam panel", 0x3f2508, "f617ec399cd06dd8", 55);
    pub const CI_RANK: Pin = pin(
        "char info first rank icon",
        0x3ed920,
        "40eca4ad9009ba25",
        34,
    );
    pub const CI_CLOSE: Pin = pin("char info close button", 0x3f2680, "92963410755c4a42", 39);
    pub const CI_TAB_SELECTED: Pin =
        pin("char info tab selected", 0x3f2400, "8aed1b27838a82dc", 41);
    pub const CI_TAB: Pin = pin("char info tab", 0x3f242c, "bf88cea5667f0cc7", 41);
    pub const CI_TAB_NAMES: [Pin; 2] = [
        pin(
            "char info default tab caption",
            0x3f23d4,
            "265ac1eee26d154c",
            12,
        ),
        pin(
            "char info fireteam tab caption",
            0x3f23a8,
            "3dbeb9bd4cf59e62",
            13,
        ),
    ];
    pub const SOLID_BUTTON: Pin = pin("solid button prefix", 0x3f54cc, "0f333c7f9f970b4a", 40);
    /// The report box's wide texts: the send-again notice, the two list placeholders, the two refusals (no target,
    /// no text), the send and cancel captions.
    // EVIDENCE: cshell+0x273b70 (the notice), cshell+0x273630 (the placeholders), cshell+0x273d50 (the refusals),
    // cshell+0x274160 (the captions handed to the two buttons)
    pub const REPORT_TEXTS: [Pin; 7] = [
        pin("report resend notice", 0x3f5280, "08df6cb3bb92cd15", 64),
        pin(
            "report general placeholder",
            0x3f5248,
            "52efbc6b9e7a6a19",
            25,
        ),
        pin(
            "report detail placeholder",
            0x3f5214,
            "3a047a4473dffe18",
            24,
        ),
        pin("report no target", 0x3f5308, "25afcc3df6bccc78", 65),
        pin("report no text", 0x3f5390, "8349e434f32e5f04", 76),
        pin("report send caption", 0x3f54c0, "f6f4688ff23d50c6", 4),
        pin("report cancel caption", 0x3f54a0, "19766ed6ccb2f4a3", 6),
    ];
    /// The report box's category table, first entry: its general and its detail name (the lists' item 0, which the
    /// box selects on open).
    // EVIDENCE: cshell+0x273080 (the table: entry 0 = general 0 / detail 0 with these two names), cshell+0x273b70
    // (open: placeholders, then command 0x186a5 with item 0), cshell+0x2727f0 (0x186a5: the general list's item text on
    // the general box, then 0x186a6 with item 0), cshell+0x272a10 (0x186a6: the detail list's item text on the detail
    // box)
    // DISASM: cshell+0x27316a / 0x27319d (the two literals of entry 0)
    pub const REPORT_FIRST_ENTRY: [Pin; 2] = [
        pin("report first general", 0x3f51d8, "b998239b5689442e", 19),
        pin("report first detail", 0x3f51c0, "98a7655d026affd9", 10),
    ];
    /// The report box's category names: the general kinds and the detail kinds of the table (the names are literals of
    /// the table build, in table order).
    // EVIDENCE: cshell+0x273080 (the 14 entries: general kind, detail index, flags, the two names)
    pub const REPORT_GENERAL: [Pin; 3] = [
        pin("report general 0", 0x3f51d8, "b998239b5689442e", 19),
        pin("report general 1", 0x3f50c0, "7c29386e24ebe7df", 12),
        pin("report general 2", 0x3f5024, "355f79f29d7d3c2d", 8),
    ];
    pub const REPORT_DETAIL: [Pin; 14] = [
        pin("report detail 0", 0x3f51c0, "98a7655d026affd9", 10),
        pin("report detail 1", 0x3f5188, "745cbc2bdee37418", 27),
        pin("report detail 2", 0x3f5164, "dc3e698db1e85324", 16),
        pin("report detail 3", 0x3f5120, "b508918017ac8f1e", 32),
        pin("report detail 4", 0x3f5104, "099f26a7016cb2ad", 12),
        pin("report detail 5", 0x3f50f0, "3a46ee96d1499f64", 8),
        pin("report detail 6", 0x3f50dc, "2d240807f3718430", 8),
        pin("report detail 7", 0x3f5098, "aa6f01d26d819c6f", 18),
        pin("report detail 8", 0x3f508c, "119e66d117ae8f0c", 4),
        pin("report detail 9", 0x3f506c, "d7c6e3efb5b5a907", 14),
        pin("report detail 10", 0x3f5058, "31bcb8940fffb7d1", 8),
        pin("report detail 11", 0x3f5044, "9a523eddafc67664", 9),
        pin("report detail 12", 0x3f5038, "f97e9da0e3b879f0", 5),
        pin("report detail 13", 0x3f5024, "355f79f29d7d3c2d", 8),
    ];
}

/// The literals of [`overlay_pins`] as read from the user's binary (`None`: absent or not matching).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayPaths {
    pub radio_on: Option<String>,
    pub radio_off: Option<String>,
    pub line: Option<String>,
    pub knob: Option<String>,
    pub minus: Option<String>,
    pub plus: Option<String>,
    pub dropdown: Option<String>,
    pub dropdown_arrow: Option<String>,
    pub key_box: Option<String>,
    pub element_count: Option<String>,
    pub element_all_count: Option<String>,
    pub element_keys: [Option<String>; 6],
    pub blind: Option<String>,
    pub report_window: Option<String>,
    pub report_dropdown: Option<String>,
    pub report_find: Option<String>,
    pub solid_button: Option<String>,
    pub ci_panel: Option<String>,
    pub ci_panel2: Option<String>,
    pub ci_rank: Option<String>,
    pub ci_close: Option<String>,
    pub ci_tab_selected: Option<String>,
    pub ci_tab: Option<String>,
    pub ci_tab_names: [Option<String>; 2],
    pub report_texts: [Option<String>; 7],
    pub report_first_entry: [Option<String>; 2],
    pub report_general: [Option<String>; 3],
    pub report_detail: [Option<String>; 14],
}

impl OverlayPaths {
    pub fn from_cshell(pe: &PeFile) -> OverlayPaths {
        use overlay_pins as o;
        let r = |p: &Pin| read_pinned(pe, p).ok();
        let w = |p: &Pin| {
            pe.wstr(p.rva)
                .ok()
                .filter(|s| dc_ui_data::names::matches_pin(s, p.sha16, p.len))
        };
        OverlayPaths {
            radio_on: r(&o::RADIO_ON),
            radio_off: r(&o::RADIO_OFF),
            line: r(&o::LINE),
            knob: r(&o::KNOB),
            minus: r(&o::MINUS),
            plus: r(&o::PLUS),
            dropdown: r(&o::DROPDOWN),
            dropdown_arrow: r(&o::DROPDOWN_ARROW),
            key_box: r(&o::KEY_BOX),
            element_count: r(&o::ELEMENT_COUNT),
            element_all_count: r(&o::ELEMENT_ALL_COUNT),
            element_keys: o::ELEMENT_KEYS.map(|p| r(&p)),
            blind: r(&o::BLIND),
            report_window: r(&o::REPORT_WINDOW),
            report_dropdown: r(&o::REPORT_DROPDOWN),
            report_find: r(&o::REPORT_FIND),
            solid_button: r(&o::SOLID_BUTTON),
            ci_panel: r(&o::CI_PANEL),
            ci_panel2: r(&o::CI_PANEL2),
            ci_rank: r(&o::CI_RANK),
            ci_close: r(&o::CI_CLOSE),
            ci_tab_selected: r(&o::CI_TAB_SELECTED),
            ci_tab: r(&o::CI_TAB),
            ci_tab_names: o::CI_TAB_NAMES.map(|p| w(&p)),
            report_texts: o::REPORT_TEXTS.map(|p| w(&p)),
            report_first_entry: o::REPORT_FIRST_ENTRY.map(|p| w(&p)),
            report_general: o::REPORT_GENERAL.map(|p| w(&p)),
            report_detail: o::REPORT_DETAIL.map(|p| w(&p)),
        }
    }
}

/// A button prefix from the literal of its first state texture (`...A.dtx`).
pub fn prefix_of_a(tex: Option<&str>) -> Option<String> {
    let t = tex?;
    let cut = t.len().checked_sub(5)?;
    t.get(cut..)
        .filter(|s| s.eq_ignore_ascii_case("A.dtx"))
        .map(|_| t[..cut].to_string())
}

/// The code literals of [`pins`] as read from the user's binary (`None`: absent or not matching).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodePaths {
    pub login_back: Option<String>,
    pub server_back: Option<String>,
    pub group_button: Option<String>,
    pub state_images: [Option<String>; 5],
    pub state_ok: Option<String>,
    pub state_no: Option<String>,
    pub popup: Option<String>,
    pub popup_buttons: [Option<String>; 3],
    pub warning: Option<String>,
    pub ok_icon: Option<String>,
    pub result_page: Option<String>,
    pub result_back: Option<String>,
    pub lobby_enter: [Option<String>; 4],
    /// w3-options: the options / report / help literals (not counted by [`CodePaths::resolved`]).
    pub overlay: OverlayPaths,
    /// [`pins::GOAL_POINTS_FORM`] (a UTF-16 literal).
    pub goal_points: Option<String>,
    /// The chat window's panel on the room list and in the waiting room.
    pub chat_panels: [Option<String>; 2],
    /// gm-client: the operator command words and confirmations (not counted by [`CodePaths::resolved`]).
    pub gm: super::gm::GmLiterals,
    /// crate-result-window: the supply-case award window's literals (not counted by [`CodePaths::resolved`]).
    pub award: super::award::AwardLiterals,
}

impl CodePaths {
    pub fn from_cshell(pe: &PeFile) -> CodePaths {
        let r = |p: &Pin| read_pinned(pe, p).ok();
        CodePaths {
            login_back: r(&pins::LOGIN_BACK),
            server_back: r(&pins::SERVER_BACK),
            group_button: r(&pins::GROUP_BUTTON),
            state_images: pins::STATE_IMAGES.map(|p| r(&p)),
            state_ok: r(&pins::STATE_OK),
            state_no: r(&pins::STATE_NO),
            popup: r(&pins::POPUP),
            popup_buttons: pins::POPUP_BUTTONS.map(|p| r(&p)),
            warning: r(&pins::WARNING),
            ok_icon: r(&pins::OK_ICON),
            result_page: r(&pins::RESULT_PAGE),
            result_back: r(&pins::RESULT_BACK),
            lobby_enter: pins::LOBBY_ENTER.map(|p| r(&p)),
            overlay: OverlayPaths::from_cshell(pe),
            goal_points: read_pinned_wide(pe, &pins::GOAL_POINTS_FORM),
            chat_panels: pins::CHAT_PANELS.map(|p| r(&p)),
            gm: super::gm::GmLiterals::from_cshell(pe),
            award: super::award::AwardLiterals::from_cshell(pe),
        }
    }

    pub fn from_rom(rom: &Path) -> CodePaths {
        let p = rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]);
        PeFile::from_file(&p)
            .map(|pe| CodePaths::from_cshell(&pe))
            .unwrap_or_default()
    }

    /// How many literals resolved (for logs).
    pub fn resolved(&self) -> usize {
        let one = |o: &Option<String>| o.is_some() as usize;
        one(&self.login_back)
            + one(&self.server_back)
            + one(&self.group_button)
            + self.state_images.iter().map(one).sum::<usize>()
            + one(&self.state_ok)
            + one(&self.state_no)
            + one(&self.popup)
            + self.popup_buttons.iter().map(one).sum::<usize>()
            + one(&self.warning)
            + one(&self.ok_icon)
            + one(&self.result_page)
            + one(&self.result_back)
            + self.lobby_enter.iter().map(one).sum::<usize>()
            + one(&self.goal_points)
            + self.chat_panels.iter().map(one).sum::<usize>()
    }

    /// The common button prefix of [`pins::LOBBY_ENTER`] (`<prefix><letter>.dtx`, letters A, B, D, C in that order);
    /// `None` unless all four resolve to one prefix.
    pub fn lobby_enter_prefix(&self) -> Option<String> {
        let first = self.lobby_enter[0].as_deref()?;
        let prefix = first.get(..first.len().checked_sub(5)?)?;
        let same = self
            .lobby_enter
            .iter()
            .zip(["A", "B", "D", "C"])
            .all(|(s, l)| s.as_deref() == Some(format!("{prefix}{l}.dtx").as_str()));
        same.then(|| prefix.to_string())
    }
}

/// Reads the NUL-terminated UTF-16 literal at `p.rva` and accepts it when it matches the pin (length and SHA-256
/// prefix of its Latin-1 bytes, as [`read_pinned`] checks narrow literals).
pub fn read_pinned_wide(pe: &PeFile, p: &Pin) -> Option<String> {
    let s = pe.wstr(p.rva).ok()?;
    matches_pin(&s, p.sha16, p.len).then_some(s)
}

/// Keys of the room table's map and mode rows.
pub mod room_keys {
    use super::{key, KeyPin};
    // OBSERVED: ROM room table rows read at run time (map rows: display name, small image; mode rows: short name,
    // player limits)
    // EVIDENCE: cshell+0x11ed10 (room row: map name and mode name drawn per row)
    pub const MAP_LABEL: KeyPin = key("dcd1d5223f73b3a9", 4);
    pub const MAP_PICTURE: KeyPin = key("fb28c96d40662122", 13);
    pub const MODE_NAME: KeyPin = key("106329c237658438", 14);
    /// The mode row's full name (room creation box, waiting room).
    // EVIDENCE: cshell+0x265780 (mode row: short name at +0x10c, full name at +0x128), cshell+0x263910 (the full
    // name getter used by cshell+0x2666d0 and cshell+0x167e90)
    pub const MODE_FULL_NAME: KeyPin = key("a34e59def3dd7b92", 12);
    /// The map row's description (the waiting room's map text).
    // EVIDENCE: cshell+0x265780 (map row: description at +0x208), cshell+0x263a80 (the map section's
    // `<description key>_<mode key>` entry when it exists, else the row's description)
    pub const MAP_DESCRIPTION: KeyPin = key("526e0087cc3f254d", 11);
    pub const MODE_MAX_PLAYERS: KeyPin = key("68ab2c146f47799d", 10);
    pub const MODE_MIN_PLAYERS: KeyPin = key("cf5c15506263718c", 10);
    pub const MODE_PLAYERS_STEP: KeyPin = key("29530865b879e019", 10);
    /// The mode row's place in the room creation box's mode list and its recommended player count.
    // EVIDENCE: cshell+0x265780 (mode row keys: sorting order -> +0x108, recommended player count -> +0x154),
    // cshell+0x266940 (the mode list keyed by +0x108), cshell+0x2651b0 (+0x154 for a mode without a map)
    pub const MODE_SORTING_ORDER: KeyPin = key("b30540c9208cb7b0", 12);
    pub const MODE_RECOMMENDED_PLAYERS: KeyPin = key("9fe5285051e1d2ba", 19);
    /// The row's short key (map rows and mode rows; the scripted player's `--pilot-map` / `--mode` accept it).
    // OBSERVED: ROM room table rows read at run time (dc-bute world_for_room and the lobby read the same key)
    pub const ROW_KEY: KeyPin = key("99a52df3ff3d4994", 3);
    /// The mode row's mode class (GAME_MODES_2009.md section 2).
    // SPEC: GAME_MODES_2009.md section 2 (the class of each mode row)
    pub const MODE_CLASS: KeyPin = key("e5792f5953852926", 8);
    /// The mode row's four room option flags (CLI-1897).
    // EVIDENCE: cshell+0x265780 (mode row bytes +0x168 .. +0x16b read from these keys)
    pub const MODE_WEAPONS_OPEN: KeyPin = key("72435634f14713e7", 22);
    pub const MODE_MID_JOIN_OPEN: KeyPin = key("e04bb3a649087c2c", 23);
    pub const MODE_FRIENDLY_FIRE_OPEN: KeyPin = key("e43ea74ee821e6ca", 21);
    pub const MODE_KILL_CAM_OPEN: KeyPin = key("ef03c829d033e311", 28);
}

/// A room option a mode row may lock (CLI-1897).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomOption {
    /// The weapon limit list.
    Weapons,
    MidJoin,
    FriendlyFire,
    KillCam,
}

impl RoomOption {
    fn key(self) -> KeyPin {
        match self {
            RoomOption::Weapons => room_keys::MODE_WEAPONS_OPEN,
            RoomOption::MidJoin => room_keys::MODE_MID_JOIN_OPEN,
            RoomOption::FriendlyFire => room_keys::MODE_FRIENDLY_FIRE_OPEN,
            RoomOption::KillCam => room_keys::MODE_KILL_CAM_OPEN,
        }
    }
}

/// Font cell height used when a font record gives none.
// DC-DEFAULT: CLI-161 (proposed) menu fallback font cell 14 virtual pixels (the layout's common font size)
pub const FALLBACK_CELL: f32 = 14.0;

/// The menu's ROM data and drawing resources.
pub struct Catalog {
    pub rom: Option<PathBuf>,
    pub pages: ScreenPages,
    pub names: Option<UiNames>,
    pub strings: Option<UiStrings>,
    pub layout: Option<LayoutTable>,
    pub rooms: Option<dc_bute::GameRoomData>,
    /// The room table's map and mode rows, read once (the screens look names up on every frame they draw).
    map_rows: Vec<dc_bute::SectionMap>,
    mode_rows: Vec<dc_bute::SectionMap>,
    /// CLI-1897: the clan mode rows (`[ClanMode<n>]`) and whether the mode lookups read them (the joined server
    /// entry is a clan-only one).
    clan_mode_rows: Vec<dc_bute::SectionMap>,
    clan_table: bool,
    /// MAP-32: the community maps of the joined server's mod set, the rows after the ROM's map rows (map index
    /// `first_community_index() + i`), in mod-set load order then map key order (`JoinReport::maps`).
    community: Vec<dc_mod_lua::MapRecord>,
    /// MAP-32: the lobby's published first community index (`JoinReport::community_map_base`); `None`: the
    /// ROM's map row count stands in (a lobby that does not publish it).
    community_base: Option<u32>,
    pub paths: CodePaths,
    pub assets: HudAssets,
    pub source: String,
    size: (u32, u32),
    /// The installed face (name, bold) the menu's default font was loaded from, when the ROM's font records name
    /// one without a file ([`super::sysfont`]); `None`: the default font comes from the ROM.
    pub system_face: Option<(String, bool)>,
}

impl Catalog {
    /// Everything from the client folder `rom`, for a `width` x `height` target.
    pub fn from_rom(rom: &Path, width: u32, height: u32) -> Result<Catalog, String> {
        let names = UiNames::from_rom(rom).map_err(|e| e.to_string())?;
        let vfs = dc_rez::RezVfs::new(rom);
        let layout = LayoutTable::load(&vfs, names.clone()).map_err(|e| e.to_string())?;
        let pages = ScreenPages::load(&vfs, &names).map_err(|e| e.to_string())?;
        let strings = UiStrings::from_rom(rom).ok();
        let rooms = dc_bute::GameRoomData::from_rom(rom).ok();
        let paths = CodePaths::from_rom(rom);
        let fonts = SystemFonts::from_layout(&layout);
        let resolver = FontResolver::open(rom).map_err(|e| e.to_string())?;
        let textures = RomTextureSource::open(rom)?;
        let (system_face, default) = match system_default(&fonts, &resolver) {
            Some((face, bytes)) => (Some(face), Some(bytes)),
            None => (None, crate::overlay::load_rom_font_bytes(rom)),
        };
        let scale = VirtualScaler::new(width, height).sy;
        let source = format!(
            "ROM menu ({} pages, {} strings, {} code literals)",
            pages.pages.len(),
            strings.as_ref().map_or(0, |s| s.len()),
            paths.resolved()
        );
        let (map_rows, mode_rows) = rooms
            .as_ref()
            .map(|g| (g.maps(), g.modes(false)))
            .unwrap_or_default();
        let clan_mode_rows = rooms.as_ref().map(|g| g.modes(true)).unwrap_or_default();
        let mut cat = Catalog {
            rom: Some(rom.to_path_buf()),
            pages,
            names: Some(names),
            strings,
            layout: Some(layout),
            rooms,
            map_rows,
            mode_rows,
            clan_mode_rows,
            clan_table: false,
            community_base: None,
            community: Vec::new(),
            paths,
            assets: HudAssets::new(
                Box::new(textures),
                HudFonts::new(default, FALLBACK_CELL, Some((fonts, resolver)), scale),
            ),
            source,
            size: (width, height),
            system_face,
        };
        // L1: the front end's cursor image, read once at menu start (CLI-480)
        cat.load_cursor(&vfs);
        // CLI-1893: our own flag pictures, in the shape of the ROM's flags
        cat.load_own_flags();
        Ok(cat)
    }

    /// No ROM: no pages or textures; text with `font` (tests).
    pub fn fallback(font: Option<(Vec<u8>, usize)>, width: u32, height: u32) -> Catalog {
        let scale = VirtualScaler::new(width, height).sy;
        Catalog {
            rom: None,
            pages: ScreenPages::default(),
            names: None,
            strings: None,
            layout: None,
            rooms: None,
            map_rows: Vec::new(),
            mode_rows: Vec::new(),
            clan_mode_rows: Vec::new(),
            clan_table: false,
            community_base: None,
            community: Vec::new(),
            paths: CodePaths::default(),
            assets: HudAssets::new(
                Box::new(NoTextures),
                HudFonts::new(font, FALLBACK_CELL, None, scale),
            ),
            source: "no ROM: text only".into(),
            size: (width, height),
            system_face: None,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn set_size(&mut self, width: u32, height: u32) {
        self.size = (width, height);
        self.assets
            .fonts
            .set_scale(VirtualScaler::new(width, height).sy);
    }

    /// String `id` of the string table (`#id` without a ROM, so ROM-less screens stay readable in tests).
    pub fn text(&self, id: i32) -> String {
        if id <= 0 {
            return String::new();
        }
        match &self.strings {
            Some(s) => s.get(id as u32).unwrap_or_default().to_string(),
            None => format!("#{id}"),
        }
    }

    /// The layout section whose name matches `sec` (w3-options: the report and help boxes' sections).
    pub fn section_pinned(&self, sec: &KeyPin) -> Option<dc_ui_data::layout::SectionView<'_>> {
        self.layout
            .as_ref()?
            .sections()
            .find(|s| matches_pin(s.name, sec.sha16, sec.len))
    }

    /// A value of the layout section `sec` by key pin.
    pub fn section_value(&self, sec: &KeyPin, k: &KeyPin) -> Option<&str> {
        let s = self.section_pinned(sec)?;
        find_key(s.map.iter(), k)
    }

    /// A value of `screen`'s layout section by key pin.
    pub fn style(&self, screen: UiScreen, k: &KeyPin) -> Option<&str> {
        let sec = self.layout.as_ref()?.screen_section(screen)?;
        find_key(sec.iter(), k)
    }

    fn room_row(&self, mode: bool, index: i32) -> Option<&dc_bute::SectionMap> {
        let rows = if mode {
            self.active_mode_rows()
        } else {
            &self.map_rows[..]
        };
        rows.get(usize::try_from(index).ok()?)
    }

    /// The mode rows the lookups read: the clan rows on a clan-only server entry, else the plain rows.
    fn active_mode_rows(&self) -> &[dc_bute::SectionMap] {
        if self.clan_table {
            &self.clan_mode_rows
        } else {
            &self.mode_rows
        }
    }

    /// CLI-1897: every mode lookup reads the clan mode rows (`on`, a clan-only server entry) or the plain ones.
    // EVIDENCE: cshell+0x265780 (the plain and the clan mode lists, table +0x15e0 / +0x15f0), cshell+0x1630a0 and
    // cshell+0x264d90 (the list picked by the session's clan flag +0x12c, copied from the server entry +0xbc at
    // cshell+0x13dff0), cshell+0x138e90 / cshell+0x1391e0 (room creation: players and names of the clan list),
    // cshell+0x163550 / cshell+0x1637a0 (the waiting room's goal and player lists of the clan list)
    pub fn set_clan_table(&mut self, on: bool) {
        self.clan_table = on;
    }

    /// True when the mode lookups read the clan mode rows.
    pub fn clan_table(&self) -> bool {
        self.clan_table
    }

    /// Map row `index` of the room table.
    pub fn map_row(&self, index: i32) -> Option<&dc_bute::SectionMap> {
        self.room_row(false, index)
    }

    /// Mode row `index` of the room table (the clan rows on a clan-only server entry, CLI-1897).
    pub fn mode_row(&self, index: i32) -> Option<&dc_bute::SectionMap> {
        self.room_row(true, index)
    }

    /// CLI-1897: whether mode row `index` lets the host change room option `opt` (the row's flag; a row without
    /// the key, or no row at all, lets it).
    // EVIDENCE: cshell+0x265780 (the four flags of a mode row: weapon limit +0x168, mid-match join +0x169, friendly
    // fire +0x16a, kill cam +0x16b), cshell+0x264ad0 / cshell+0x264c30 / cshell+0x264d90 / cshell+0x264ef0 (their
    // getters)
    // DC-DEFAULT: CLI-1897 (proposed) a row without the key keeps the option open
    pub fn mode_option_open(&self, index: i32, opt: RoomOption) -> bool {
        self.room_row(true, index)
            .and_then(|r| find_key(r.iter(), &opt.key()))
            .is_none_or(|v| {
                let v = v.trim().trim_matches('"').trim();
                v.eq_ignore_ascii_case("true") || v == "1"
            })
    }

    /// The mode class of mode row `index` (None without a ROM or a class key).
    // SPEC: GAME_MODES_2009.md section 2 (`GameType` of the row = mode class)
    pub fn mode_class(&self, index: i32) -> Option<u8> {
        self.room_row(true, index)
            .and_then(|r| find_key(r.iter(), &room_keys::MODE_CLASS))
            .and_then(|v| v.trim().parse().ok())
    }

    /// MAP-32: the community maps of the joined server (the rows after the ROM's map rows; empty on vanilla) and
    /// the lobby's published index of the first one (`None`: this ROM's map row count stands in).
    pub fn set_community_maps(&mut self, base: Option<u32>, maps: Vec<dc_mod_lua::MapRecord>) {
        self.community_base = base;
        self.community = maps;
    }

    /// The community map rows, in map index order.
    pub fn community_maps(&self) -> &[dc_mod_lua::MapRecord] {
        &self.community
    }

    /// The number of ROM map rows (the first community map's index when the lobby published none).
    pub fn rom_map_count(&self) -> usize {
        self.map_rows.len()
    }

    /// The map index of the first community map: the lobby's `community_map_base` (MAP-35: the ROM map count
    /// of the lobby's own table, 12 on a lobby without a ROM), else this ROM's map row count.
    // DC-DEFAULT: MAP-32 (proposed) the base comes from the lobby, the ROM row count is the fallback
    pub fn first_community_index(&self) -> usize {
        self.community_base
            .map_or(self.map_rows.len(), |b| b as usize)
    }

    /// The community map at map index `index` (`None`: a ROM row or none at all).
    // DC-DEFAULT: MAP-32 (proposed) map index >= the first community index = community map (index - base)
    pub fn community_map(&self, index: i32) -> Option<&dc_mod_lua::MapRecord> {
        let i = usize::try_from(index)
            .ok()?
            .checked_sub(self.first_community_index())?;
        self.community.get(i)
    }

    /// Display name of map row `index` (`Map n` without a ROM); a community map's record name.
    pub fn map_label(&self, index: i32) -> String {
        if let Some(m) = self.community_map(index) {
            return m.name.clone();
        }
        self.room_row(false, index)
            .and_then(|r| find_key(r.iter(), &room_keys::MAP_LABEL).map(str::to_string))
            .unwrap_or_else(|| format!("Map {index}"))
    }

    /// Small image of map row `index`; a community map's record picture (a `@mod:` path the installed mod layer
    /// serves; `None` without one, so the panel keeps its last picture, as a ROM row without the key does).
    pub fn map_picture(&self, index: i32) -> Option<String> {
        if let Some(m) = self.community_map(index) {
            return m.picture.clone();
        }
        self.room_row(false, index)
            .and_then(|r| find_key(r.iter(), &room_keys::MAP_PICTURE).map(str::to_string))
    }

    /// Short name of mode row `index` (`Mode n` without a ROM).
    pub fn mode_name(&self, index: i32) -> String {
        self.room_row(true, index)
            .and_then(|r| find_key(r.iter(), &room_keys::MODE_NAME).map(str::to_string))
            .unwrap_or_else(|| format!("Mode {index}"))
    }

    /// Full name of mode row `index` (the short name when the row has none, `Mode n` without a ROM).
    // DC-DEFAULT: CLI-476 (proposed) a mode row without a full name shows its short name
    pub fn mode_full_name(&self, index: i32) -> String {
        self.room_row(true, index)
            .and_then(|r| find_key(r.iter(), &room_keys::MODE_FULL_NAME))
            .map(|v| v.trim().trim_matches('"').trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| self.mode_name(index))
    }

    /// Description of map row `map` for mode row `mode`: the map section's mode-specific entry (the description
    /// key, `_`, the mode row's key) when it exists, else the map's description; empty without either.
    // EVIDENCE: cshell+0x263a80 (`Description_%s` with the mode row's first string, the row key; the section's
    // entry when present, else the row's +0x208)
    pub fn map_description(&self, map: i32, mode: i32) -> String {
        // MAP-32: a community map's description is the record's, the same for every mode
        if let Some(m) = self.community_map(map) {
            return m.description.clone().unwrap_or_default();
        }
        let Some(row) = self.room_row(false, map) else {
            return String::new();
        };
        let Some((name, plain)) = row.iter().find(|(k, _)| {
            matches_pin(
                k,
                room_keys::MAP_DESCRIPTION.sha16,
                room_keys::MAP_DESCRIPTION.len,
            )
        }) else {
            return String::new();
        };
        let special = self
            .room_row(true, mode)
            .and_then(|m| m.get("Key"))
            .map(|k| format!("{name}_{}", k.trim().trim_matches('"').trim()))
            .and_then(|k| row.get(&k));
        special
            .unwrap_or(plain)
            .trim()
            .trim_matches('"')
            .trim()
            .to_string()
    }

    /// The width of `text` in virtual pixels as the font draws it at 1024 x 768, so a width compares the same at
    /// every target size: what the word wrap measures with.
    // DC-DEFAULT: CLI-540 (proposed) the wrap breaks a text where it breaks at 1024 x 768 at every target size
    // (CLI-1267: a larger target draws the font at its own em, whose widths are not the 1024 x 768 ones scaled)
    pub fn measure_text_font_units(&mut self, font: i32, size: i32, text: &str) -> Option<f32> {
        let a = self
            .assets
            .fonts
            .atlas_virtual(font, (size > 0).then_some(size))?;
        Some(a.measure(text))
    }

    /// Width and line height of `text` in virtual pixels with font record `font` at cell `size` (0 = the record's).
    pub fn measure_text(&mut self, font: i32, size: i32, text: &str) -> Option<(f32, f32)> {
        let (w, h) = self.size;
        let sx = w as f32 / 1024.0;
        let sy = h as f32 / 768.0;
        let a = self
            .assets
            .fonts
            .atlas_sized(font, (size > 0).then_some(size))?;
        Some((a.measure(text) / sx, a.line_height / sy))
    }

    /// Player limits of mode row `index`: (min, max, step).
    pub fn mode_players(&self, index: i32) -> Option<(i32, i32, i32)> {
        let r = self.room_row(true, index)?;
        let get = |k: &KeyPin| find_key(r.iter(), k).and_then(|v| v.trim().parse::<i32>().ok());
        let max = get(&room_keys::MODE_MAX_PLAYERS)?;
        let min = get(&room_keys::MODE_MIN_PLAYERS).unwrap_or(2);
        let step = get(&room_keys::MODE_PLAYERS_STEP).unwrap_or(2).max(1);
        Some((min, max, step))
    }

    /// The sorting order value of mode row `index` (its place in the room creation box's mode list).
    pub fn mode_sorting_order(&self, index: i32) -> Option<i32> {
        let r = self.room_row(true, index)?;
        find_key(r.iter(), &room_keys::MODE_SORTING_ORDER).and_then(|v| v.trim().parse().ok())
    }

    /// The recommended player count of mode row `index` (the room creation box's player count for the mode).
    pub fn mode_recommended_players(&self, index: i32) -> Option<i32> {
        let r = self.room_row(true, index)?;
        find_key(r.iter(), &room_keys::MODE_RECOMMENDED_PLAYERS).and_then(|v| v.trim().parse().ok())
    }

    /// The room table's map rows (empty without a ROM).
    pub fn map_rows(&self) -> &[dc_bute::SectionMap] {
        &self.map_rows
    }

    /// The room table's mode rows (the clan rows on a clan-only server entry, CLI-1897; empty without a ROM).
    pub fn mode_rows(&self) -> &[dc_bute::SectionMap] {
        self.active_mode_rows()
    }

    /// Number of mode rows (0 without a ROM).
    pub fn mode_count(&self) -> usize {
        self.active_mode_rows().len()
    }

    /// The screen's real size in pixels.
    pub fn screen_size(&self) -> (u32, u32) {
        self.size
    }

    /// The texture's size, when it resolves.
    pub fn tex_size(&mut self, path: &str) -> Option<(u32, u32)> {
        self.assets.texture(path).map(|t| (t.width, t.height))
    }

    /// The display list drawn over an opaque black target.
    pub fn compose(&mut self, items: &[DrawItem]) -> RgbaImage {
        let (w, h) = self.size;
        compose(&with_backdrop(items), &mut self.assets, w, h)
    }
}

/// A face name and its bold flag.
type FaceKey = (String, bool);
/// Font file bytes and the face index inside them.
type FontBytes = (Vec<u8>, usize);

/// The installed font of the face most font records name without a file of their own (the menu's text of
/// those records is drawn with it), with the face's name and weight; the host's font folder is scanned once per
/// process.
// SPEC: FMT_CRES_UI.md §7.1 (16 of the 27 records name a system face only)
// DC-DEFAULT: CLI-462 (proposed) see [`super::sysfont`]
pub(crate) fn system_default(
    fonts: &SystemFonts,
    resolver: &FontResolver,
) -> Option<(FaceKey, FontBytes)> {
    use std::sync::OnceLock;
    static FACES: OnceLock<Vec<super::sysfont::InstalledFace>> = OnceLock::new();
    let mut counts: Vec<((String, bool), usize)> = Vec::new();
    for spec in fonts.iter().flatten() {
        if resolver.resolve(spec).is_some() || spec.face.is_empty() {
            continue;
        }
        let key = (spec.face.clone(), spec.bold);
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => counts.push((key, 1)),
        }
    }
    let (key, _) = counts.into_iter().max_by_key(|(_, n)| *n)?;
    let faces = FACES.get_or_init(|| {
        super::sysfont::font_dirs()
            .iter()
            .flat_map(|d| super::sysfont::scan(d))
            .collect()
    });
    // the weight actually loaded (only the other weight may be installed)
    let picked = super::sysfont::pick(faces, &key.0, key.1)?;
    let bytes = (std::fs::read(&picked.path).ok()?, picked.index);
    Some(((key.0, picked.bold), bytes))
}

/// `items` after the opaque black backdrop over the whole virtual screen (what the menu draws first).
pub fn with_backdrop(items: &[DrawItem]) -> Vec<DrawItem> {
    let mut all = Vec::with_capacity(items.len() + 1);
    all.push(DrawItem::Fill {
        rect: [0.0, 0.0, 1024.0, 768.0],
        colour: [0, 0, 0, 255],
    });
    all.extend_from_slice(items);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_colours_parse_like_the_layout_table() {
        assert_eq!(pair(Some("(28,15)")), Some([28, 15]));
        assert_eq!(int(Some("303")), Some(303));
        assert_eq!(colour(Some("<32,116,132>")), Some(0xff20_7484));
        assert_eq!(argb(0xff20_7484), [0x20, 0x74, 0x84, 0xff]);
        assert_eq!(ints("(0,0,15,15)"), vec![0, 0, 15, 15]);
    }

    #[test]
    fn keys_are_found_by_hash_only() {
        let rows = [("Name".to_string(), "x".to_string())];
        let it = rows.iter().map(|(a, b)| (a, b));
        assert_eq!(find_key(it, &room_keys::MAP_LABEL), Some("x"));
    }
}

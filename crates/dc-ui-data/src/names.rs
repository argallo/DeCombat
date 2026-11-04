//! The names the 2009 UI data is addressed by (table paths, section names, keys, page and widget tags).
//!
//! None of them is in this tree: each is read at run time from the user's own `Game/CShell.dll` by rva and
//! accepted only when its length and SHA-256 prefix match the pin below (the same model as
//! `dc_rom::crypt_key`). Tests and tools that have no ROM build a [`UiNames`] of their own
//! ([`UiNames::placeholder`]).
// SPEC: FMT_CRES_UI.md §3-§7 (literal rvas and sha ids); FMT_CRES_UI_keys.tsv

use crate::error::UiDataError;
use dc_rom::{sha256_hex, PeFile};
use std::path::Path;

/// A pinned literal of the 2009 CShell.dll: rva, first 16 hex digits of the SHA-256 of its bytes, length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub what: &'static str,
    pub rva: u32,
    pub sha16: &'static str,
    pub len: usize,
}

const fn pin(what: &'static str, rva: u32, sha16: &'static str, len: usize) -> Pin {
    Pin {
        what,
        rva,
        sha16,
        len,
    }
}

/// Relative path of the shell image inside the client folder.
pub const CSHELL_REL_PATH: [&str; 2] = ["Game", "CShell.dll"];

// ---- tables and grammar words ------------------------------------------------------------------------------

/// Path of the layout table (opened by cshell+0x1b85a6). SPEC: FMT_CRES_UI.md §4
pub const PIN_LAYOUT_PATH: Pin = pin("layout_table path", 0x3ec030, "35ccd09143915838", 21);
/// Path of the screen pages table (opened inside cshell+0x1d2670). SPEC: FMT_CRES_UI.md §4
pub const PIN_PAGES_PATH: Pin = pin("screen_pages path", 0x3ed854, "6424c856bc5d4754", 28);
/// Page-list end tag. SPEC: FMT_CRES_UI.md §6.3 step 4
pub const PIN_PAGE_LIST_END: Pin = pin("page list end tag", 0x3ed5b0, "db717ba76600179a", 21);
/// Page-name key inside a page body. SPEC: FMT_CRES_UI.md §6.3 step 5
pub const PIN_PAGE_NAME_KEY: Pin = pin("page name key", 0x3ed59c, "72926e1ab2dca5d3", 11);
/// The text-style word that marks a framed button. SPEC: FMT_CRES_UI.md §6.3 (EVIDENCE: cshell+0x1c9020)
pub const PIN_FRAME_WORD: Pin = pin("framed button word", 0x3ed2f4, "c0fcf1e75f3a5dd0", 5);

// ---- layout table sections ---------------------------------------------------------------------------------

/// Base constants section. SPEC: FMT_CRES_UI.md §6.2 family 0
pub const PIN_BASE_SECTION: Pin = pin("base section", 0x3ecf68, "d310cd72eb5bdab5", 11);
/// HUD set prefix (`prefix + n`; the prefix alone is the base HUD section). SPEC: §5 step 3.2, families 52/57
pub const PIN_HUD_PREFIX: Pin = pin("HUD set prefix", 0x3d0f60, "9b19ccce0890983f", 9);
/// HUD set 0 section name (fallback set). SPEC: FMT_CRES_UI.md §6.2
pub const PIN_HUD_SET0: Pin = pin("HUD set 0", 0x3d3478, "089cb83a2538a401", 10);
/// System font record prefix (`prefix + n`). SPEC: FMT_CRES_UI.md §7.1
pub const PIN_FONT_PREFIX: Pin = pin("system font prefix", 0x3ece6c, "523251e8d40f73d7", 7);
/// Scene light record prefix. SPEC: FMT_CRES_UI.md §5 step 3.5
pub const PIN_LIGHT_PREFIX: Pin = pin("light prefix", 0x3ed8e4, "dbcd5e7bb7a0f538", 5);
/// Interface FX record prefix. SPEC: FMT_CRES_UI.md §5 step 3.6
pub const PIN_FX_PREFIX: Pin = pin("interface FX prefix", 0x3ed8e0, "adf02dd669bdb324", 2);
/// Screen transition record prefix (0 sections in the 2009 ROM). SPEC: FMT_CRES_UI.md §5 step 3.7
pub const PIN_TRANSITION_PREFIX: Pin = pin("transition prefix", 0x3ed8d4, "2fe70464aadf5873", 10);
/// Vector-font list section (counted only; unused by the 2009 client). SPEC: FMT_CRES_UI.md §7.2
pub const PIN_VECTOR_FONT_SECTION: Pin =
    pin("vector font section", 0x3ed06c, "3680fcad01f4bfc6", 5);

// ---- keys ----------------------------------------------------------------------------------------------------

/// Background colour key of the base section (EVIDENCE: cshell+0x1c7f00).
pub const PIN_KEY_BACKGROUND: Pin = pin("background colour key", 0x3ed190, "c86f21073a9da674", 9);
/// Base section integer keys read by cshell+0x1c5c90 (help font), +0x1c5cb0, +0x1c5cd0 (HUD font), and the
/// two following integer keys of the base section. SPEC: FMT_CRES_UI.md §6.2 family 0, §7.1
pub const PIN_KEY_BASE_INTS: [Pin; 5] = [
    pin("base int key 0", 0x3ecf74, "14ab9730bf072b14", 8),
    pin("base int key 1", 0x3ecf80, "86acdf8a86fa9198", 8),
    pin("base int key 2", 0x3ecf8c, "31ba5653ab0ea916", 7),
    pin("base int key 3", 0x3ecf94, "20e312bdffd1e94d", 8),
    pin("base int key 4", 0x3ecfa0, "a45a22647beb182a", 8),
];
/// Per-screen keys with fallback to screen 0, in [`ScreenKey`](crate::layout::ScreenKey) order: title font
/// (cshell+0x1c5e70), title size (+0x1c5ed0), font index (+0x1c5f30), font size (+0x1c5f90), item spacing
/// (+0x1c5ff0), item alignment (+0x1c6050), music intensity (+0x1c60b0). SPEC: FMT_CRES_UI.md §6.2
pub const PIN_KEY_SCREEN: [Pin; 7] = [
    pin("screen key title font", 0x3ed014, "53d0f790b93d9369", 9),
    pin("screen key title size", 0x3ed020, "730ac99c06b7c5cd", 9),
    pin("screen key font index", 0x3ed02c, "17138f30610a6cba", 8),
    pin("screen key font size", 0x3d1e84, "8fab546981e2c7a0", 8),
    pin("screen key item spacing", 0x3ed038, "b922d5b66fa079ae", 9),
    pin("screen key item alignment", 0x3ed044, "bdcc470d0491d082", 9),
    pin(
        "screen key music intensity",
        0x3ed050,
        "0ca3199bc9354bdb",
        14,
    ),
];
/// Record keys shared by font, light and FX records: the record name (EVIDENCE: cshell+0x1c35c0, +0x1c7be0).
pub const PIN_KEY_NAME: Pin = pin("record name key", 0x3cd3c0, "dcd1d5223f73b3a9", 4);
/// System font request keys: file, face, width, height, bold, italic, underline. SPEC: FMT_CRES_UI.md §7.1
pub const PIN_KEY_FONT: [Pin; 7] = [
    pin("font key file", 0x3dc18c, "e316d2ad49b2586a", 8),
    pin("font key face", 0x3ed0c4, "2cfc59e319387a98", 8),
    pin("font key width", 0x3ed0bc, "3019947b6daf18b0", 5),
    pin("font key height", 0x3ed0b4, "bfe2eca401797325", 6),
    pin("font key bold", 0x3ed0ac, "94fee62e68e257c0", 4),
    pin("font key italic", 0x3ed0a4, "9bf37cb58a694456", 6),
    pin("font key underline", 0x3ed098, "02f843261112c2d8", 9),
];
/// Light record keys: position, colour (0..255), radius. EVIDENCE: cshell+0x1c7be0
pub const PIN_KEY_LIGHT: [Pin; 3] = [
    pin("light key position", 0x3e707c, "704e75860aa0d98d", 3),
    pin("light key colour", 0x3ed16c, "6b73191a0a4b6742", 5),
    pin("light key radius", 0x3ed160, "6fe0661c82d10d34", 6),
];
/// Interface FX record keys: effect name, integer parameter (position shares the light key).
/// EVIDENCE: cshell+0x1c7d50
pub const PIN_KEY_FX: [Pin; 2] = [
    pin("fx key effect", 0x3ed174, "708bf5e80c1cf5de", 6),
    pin("fx key parameter", 0x3e6498, "f2f6a01889951edc", 4),
];
/// The four position keys of the base HUD section (EVIDENCE: cshell+0x1c8680, +0x1c86a0 and the two
/// following accessors). SPEC: FMT_CRES_UI.md §6.2 family 57
pub const PIN_KEY_HUD_BASE: [Pin; 4] = [
    pin("HUD base key 0", 0x3ed214, "5445b246f9acfcd7", 11),
    pin("HUD base key 1", 0x3ed220, "bad1e2fe7844de88", 12),
    pin("HUD base key 2", 0x3ed230, "6d574e9ef04f870b", 14),
    pin("HUD base key 3", 0x3ed240, "1cf3bc4303aeadf8", 15),
];

// ---- fixed tables ------------------------------------------------------------------------------------------------

/// Screen id table: 30 entries of 32 bytes at this rva (EVIDENCE: cshell+0x64770). SPEC: FMT_CRES_UI.md §6.2
pub const SCREEN_TABLE_RVA: u32 = 0x442f68;
pub const SCREEN_TABLE_STRIDE: u32 = 32;
/// SHA-256 prefixes and lengths of the 30 screen table names (index = screen id).
pub const SCREEN_TABLE_PINS: [(&str, usize); 30] = [
    ("8ec627f6f21b861c", 13),
    ("b20bb9ebbc4caca4", 10),
    ("b7c8f034dc68afc1", 11),
    ("c77c603ff367377f", 21),
    ("f50f8d423fc6dded", 10),
    ("8a9c16309b9d0edb", 13),
    ("f656af49b705ccbd", 16),
    ("324790af55a2d74b", 12),
    ("f97e66145f1240ca", 18),
    ("3c0ad4cb5a1effef", 17),
    ("2de448611be11ba9", 10),
    ("3e2f8145f846781a", 18),
    ("37b55c70730c67d1", 19),
    ("a400bdbf9c60c2d7", 21),
    ("7291910a1fcdefb8", 14),
    ("a1799a6170ec341d", 13),
    ("a2f9bf3ceb7488b0", 10),
    ("175c1029213919c9", 11),
    ("7e2548fca2de8cca", 15),
    ("591e89e041b972b3", 15),
    ("ee4f87c9f55aed3c", 16),
    ("7956ffa01e0f471c", 18),
    ("5e4372553fd07050", 15),
    ("956e4a7a5a692656", 16),
    ("f77f3158568fa22f", 19),
    ("c6fe847978fa5539", 17),
    ("7b7599998195c1cb", 19),
    ("47fbe6d8674a4ee4", 16),
    ("f65b77a26e607dd4", 10),
    ("80cb93b1f9060935", 10),
];

/// Page tag table: 20 pointers at this rva (`.data`). SPEC: FMT_CRES_UI.md §6.3 step 4
pub const PAGE_TAG_TABLE_RVA: u32 = 0x455a38;
/// SHA-256 prefixes and lengths of the 20 page tags (index = page tag index). SPEC: FMT_CRES_UI.md §6.3
pub const PAGE_TAG_PINS: [(&str, usize); 20] = [
    ("4274fce183dbf6c8", 20),
    ("94170ebe2332b474", 26),
    ("15b085646977901f", 14),
    ("42e4bd3cc9098771", 22),
    ("e4b9c4d102018232", 22),
    ("ae38a7afbc72a372", 20),
    ("8665d52af5af373b", 21),
    ("02ed63cbe133b3ad", 15),
    ("46a564f423d0ca37", 17),
    ("c7d8cd696aad8b8c", 22),
    ("25113fe1becf22e2", 20),
    ("2daf1b681bdf392a", 19),
    ("2bdc08cf02dfa70b", 17),
    ("810cca5c6ef7d30d", 15),
    ("5dcf5c2fed008c69", 23),
    ("7c7d42695a89ac85", 21),
    ("76926b64d8d2076c", 23),
    ("7f21e2c08ab73481", 19),
    ("fd7dae74e16aa437", 13),
    ("a17fce143e8cfa16", 13),
];

/// Widget open and end tags in [`WidgetKind`](crate::pages::WidgetKind) order. SPEC: FMT_CRES_UI.md §6.3
pub const WIDGET_TAG_PINS: [(Pin, Pin); 10] = [
    (
        pin("window tag", 0x3ed590, "266d000b5e44037b", 8),
        pin("window end tag", 0x3ed354, "d7b4c0f1b505a38f", 12),
    ),
    (
        pin("image tag", 0x3ed588, "0f3020c809abc316", 7),
        pin("image end tag", 0x3ed328, "8a432c285fa607c7", 11),
    ),
    (
        pin("button tag", 0x3ed57c, "2de51feae727b659", 8),
        pin("button end tag", 0x3ed334, "90d87beb843a6597", 12),
    ),
    (
        pin("text tag", 0x3ed570, "89829726781b01aa", 8),
        pin("text end tag", 0x3ed344, "19757100041ebca4", 12),
    ),
    (
        pin("list tag", 0x3ed568, "bddefcf5d1788ac4", 6),
        pin("list end tag", 0x3ed364, "f17ac65037c227bc", 10),
    ),
    (
        pin("inventory tag", 0x3ed55c, "9e5ddb265bb2e013", 11),
        pin("inventory end tag", 0x3ed370, "adc66646522303ca", 15),
    ),
    (
        pin("edit box tag", 0x3ed550, "51d67b6fb5ec083d", 8),
        pin("edit box end tag", 0x3ed380, "aa0d95d995d6ff24", 12),
    ),
    (
        pin("check button tag", 0x3ed540, "73dda1ffb676db8e", 13),
        pin("check button end tag", 0x3ed390, "5e700814644bcb29", 17),
    ),
    (
        pin("lattice tag", 0x3ed530, "045299f77ce5883b", 14),
        pin("lattice end tag", 0x3ed3a4, "0c6b00189bf1e7c4", 17),
    ),
    (
        pin("remote xml tag", 0x3ed520, "96790dc5eb42369e", 12),
        pin("remote xml end tag", 0x3ed4e4, "15f457f9a0c72e43", 16),
    ),
];

/// Named single sections of the layout table (popup boxes, mode HUDs, miscellaneous constants) that the
/// client opens by literal. Names ours. SPEC: FMT_CRES_UI.md §6.2 families 50-102
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UiSection {
    RevengeHud,
    CoopHud,
    QuarantineHud,
    GoldItemHud,
    HighSecurityHud,
    BountyHud,
    ObserverHud,
    VoiceChatHud,
    InternetCafeMark,
    MiscConstants,
    Overlay,
    HelpBox,
    ClanMarkEditor,
    CharacterInfoBox,
    RenameBox,
    ClanNameStyle,
    UserNameStyle,
    StatReset,
    UserRecordReset,
    ClanRecordReset,
    FaceChange,
    FaceChangeValues,
    GenderChangeValues,
    ColourChange,
    NameColourValues,
    ClanNameColourValues,
    CrosshairColourValues,
    ExitGame,
    UserReport,
    Coupon,
    CountryChange,
    BuyItem,
    Megaphone,
}

impl UiSection {
    pub const ALL: [UiSection; 33] = [
        UiSection::RevengeHud,
        UiSection::CoopHud,
        UiSection::QuarantineHud,
        UiSection::GoldItemHud,
        UiSection::HighSecurityHud,
        UiSection::BountyHud,
        UiSection::ObserverHud,
        UiSection::VoiceChatHud,
        UiSection::InternetCafeMark,
        UiSection::MiscConstants,
        UiSection::Overlay,
        UiSection::HelpBox,
        UiSection::ClanMarkEditor,
        UiSection::CharacterInfoBox,
        UiSection::RenameBox,
        UiSection::ClanNameStyle,
        UiSection::UserNameStyle,
        UiSection::StatReset,
        UiSection::UserRecordReset,
        UiSection::ClanRecordReset,
        UiSection::FaceChange,
        UiSection::FaceChangeValues,
        UiSection::GenderChangeValues,
        UiSection::ColourChange,
        UiSection::NameColourValues,
        UiSection::ClanNameColourValues,
        UiSection::CrosshairColourValues,
        UiSection::ExitGame,
        UiSection::UserReport,
        UiSection::Coupon,
        UiSection::CountryChange,
        UiSection::BuyItem,
        UiSection::Megaphone,
    ];

    /// The literal pin of the section name.
    pub const fn pin(self) -> Pin {
        match self {
            UiSection::RevengeHud => pin("revenge HUD", 0x3d93a8, "4fe531937d864661", 13),
            UiSection::CoopHud => pin("co-op HUD", 0x3d2978, "28d2565664e36a53", 15),
            UiSection::QuarantineHud => pin("quarantine HUD", 0x3d8a1c, "f24a1d904fd1b482", 10),
            UiSection::GoldItemHud => pin("gold item HUD", 0x3d6d40, "af3b781677f811ae", 17),
            UiSection::HighSecurityHud => pin("high-security HUD", 0x3d72f0, "7e4aa3c2bf2bd36b", 8),
            UiSection::BountyHud => pin("bounty HUD", 0x3d50dc, "ca06af9676e22991", 15),
            UiSection::ObserverHud => pin("observer HUD", 0x3d3118, "50e9a8927fdedb83", 12),
            UiSection::VoiceChatHud => pin("voice chat HUD", 0x3d3100, "178f866e5ed5473f", 9),
            UiSection::InternetCafeMark => {
                pin("internet-cafe mark", 0x3d5bd0, "d50811027dcce733", 10)
            }
            UiSection::MiscConstants => pin("misc constants", 0x3ece74, "5cb0b42660589925", 13),
            UiSection::Overlay => pin("overlay table", 0x3ecf08, "2af93aeb435dff63", 7),
            UiSection::HelpBox => pin("help box", 0x3f4d78, "5da2ee9e5d5203da", 7),
            UiSection::ClanMarkEditor => pin("clan mark editor", 0x3f9274, "61ae1a37629e8e1f", 8),
            UiSection::CharacterInfoBox => {
                pin("character info box", 0x3f26a8, "c8c3a41e3d4d319f", 16)
            }
            UiSection::RenameBox => pin("name change", 0x3f29dc, "6402057f76b1c0b6", 10),
            UiSection::ClanNameStyle => pin("clan name style", 0x3f2a1c, "e2177cb33af22da1", 11),
            UiSection::UserNameStyle => pin("user name style", 0x3f2a28, "4f22c07401aed87f", 11),
            UiSection::StatReset => pin("stat reset", 0x3f2aa8, "1af961a4e824b7af", 12),
            UiSection::UserRecordReset => {
                pin("user record reset", 0x3f2ac8, "a68fe90ffecc8b4f", 14)
            }
            UiSection::ClanRecordReset => {
                pin("clan record reset", 0x3f2ab8, "7eb805710c741098", 14)
            }
            UiSection::FaceChange => pin("face change", 0x3f3050, "985d5107fe14095f", 7),
            UiSection::FaceChangeValues => {
                pin("face change values", 0x3f3058, "eca22d2aeebaeced", 10)
            }
            UiSection::GenderChangeValues => {
                pin("gender change values", 0x3f3064, "ce0ac6b6e0fe8024", 12)
            }
            UiSection::ColourChange => pin("colour change", 0x3f32f8, "2d4e4540eb8fb1e5", 14),
            UiSection::NameColourValues => {
                pin("name colour values", 0x3f3308, "a1aec0be291b0775", 19)
            }
            UiSection::ClanNameColourValues => {
                pin("clan name colour values", 0x3f331c, "233af8f4e9f44056", 19)
            }
            UiSection::CrosshairColourValues => {
                pin("aim mark colour values", 0x3f3330, "8a51e0afb98c1f4d", 20)
            }
            UiSection::ExitGame => pin("exit game", 0x3f58f4, "3e535cf5cca9bfde", 13),
            UiSection::UserReport => pin("user report", 0x3f5684, "9d4168409680c845", 13),
            UiSection::Coupon => pin("coupon", 0x3f4c78, "40ef230dab59bdd1", 9),
            UiSection::CountryChange => pin("country change", 0x3d5be8, "b2cd7d4e0574ab2a", 16),
            UiSection::BuyItem => pin("buy item", 0x3eddac, "e24a5a3143d3333f", 10),
            UiSection::Megaphone => pin("megaphone", 0x3f8c28, "f000c11afa184952", 12),
        }
    }
}

/// All names the readers need. Every field is owned text read from the user's binary (or supplied by a test).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiNames {
    pub layout_path: String,
    pub pages_path: String,
    pub page_list_end: String,
    pub page_name_key: String,
    pub frame_word: String,
    /// `(open tag, end tag)` per widget kind.
    pub widget_tags: [(String, String); 10],
    pub page_tags: [String; 20],
    pub screens: [String; 30],
    pub base_section: String,
    pub hud_prefix: String,
    pub hud_set0: String,
    pub font_prefix: String,
    pub light_prefix: String,
    pub fx_prefix: String,
    pub transition_prefix: String,
    pub vector_font_section: String,
    pub key_background: String,
    pub key_base_ints: [String; 5],
    pub key_screen: [String; 7],
    pub key_name: String,
    /// file, face, width, height, bold, italic, underline
    pub key_font: [String; 7],
    /// position, colour, radius
    pub key_light: [String; 3],
    /// effect, parameter
    pub key_fx: [String; 2],
    pub key_hud_base: [String; 4],
    /// Section names of [`UiSection::ALL`], same order.
    pub sections: Vec<String>,
}

fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

/// `true` when `text` has the pinned length and SHA-256 prefix.
pub fn matches_pin(text: &str, sha16: &str, len: usize) -> bool {
    let b = latin1_bytes(text);
    b.len() == len && sha256_hex(&b)[..16] == *sha16
}

/// Reads the NUL-terminated literal at `p.rva` and checks it against the pin.
pub fn read_pinned(pe: &PeFile, p: &Pin) -> Result<String, UiDataError> {
    read_checked(pe, p.what, p.rva, p.sha16, p.len)
}

fn read_checked(
    pe: &PeFile,
    what: &'static str,
    rva: u32,
    sha16: &str,
    len: usize,
) -> Result<String, UiDataError> {
    let s = pe
        .cstr(rva)
        .map_err(|_| UiDataError::LiteralMismatch { what, rva })?;
    if matches_pin(&s, sha16, len) {
        Ok(s)
    } else {
        Err(UiDataError::LiteralMismatch { what, rva })
    }
}

fn arr<const N: usize>(pe: &PeFile, pins: &[Pin; N]) -> Result<[String; N], UiDataError> {
    let v = pins
        .iter()
        .map(|p| read_pinned(pe, p))
        .collect::<Result<Vec<_>, _>>()?;
    v.try_into()
        .map_err(|_| UiDataError::Malformed("pin array length".into()))
}

impl UiNames {
    /// Reads every name from `Game/CShell.dll` of the client folder `rom`.
    pub fn from_rom(rom: &Path) -> Result<Self, UiDataError> {
        let p = rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]);
        Self::from_cshell(&PeFile::from_file(&p)?)
    }

    /// Reads every name from a loaded CShell.dll image; any mismatch means "not the 2009 build".
    pub fn from_cshell(pe: &PeFile) -> Result<Self, UiDataError> {
        let mut screens = Vec::with_capacity(30);
        for (i, (sha, len)) in SCREEN_TABLE_PINS.iter().enumerate() {
            let rva = SCREEN_TABLE_RVA + SCREEN_TABLE_STRIDE * i as u32;
            screens.push(read_checked(pe, "screen table entry", rva, sha, *len)?);
        }
        let mut page_tags = Vec::with_capacity(20);
        for (i, (sha, len)) in PAGE_TAG_PINS.iter().enumerate() {
            let at = PAGE_TAG_TABLE_RVA + 4 * i as u32;
            let ptr = pe.rva(at, 4)?;
            let va = u32::from_le_bytes([ptr[0], ptr[1], ptr[2], ptr[3]]);
            let rva = va
                .checked_sub(pe.base)
                .ok_or(UiDataError::LiteralMismatch {
                    what: "page tag pointer",
                    rva: at,
                })?;
            page_tags.push(read_checked(pe, "page tag", rva, sha, *len)?);
        }
        let mut widget_tags = Vec::with_capacity(10);
        for (o, e) in WIDGET_TAG_PINS.iter() {
            widget_tags.push((read_pinned(pe, o)?, read_pinned(pe, e)?));
        }
        let sections = UiSection::ALL
            .iter()
            .map(|s| read_pinned(pe, &s.pin()))
            .collect::<Result<Vec<_>, _>>()?;
        let screens: [String; 30] = screens
            .try_into()
            .map_err(|_| UiDataError::Malformed("screen table".into()))?;
        let page_tags: [String; 20] = page_tags
            .try_into()
            .map_err(|_| UiDataError::Malformed("page tag table".into()))?;
        let widget_tags: [(String, String); 10] = widget_tags
            .try_into()
            .map_err(|_| UiDataError::Malformed("widget tags".into()))?;
        Ok(UiNames {
            layout_path: read_pinned(pe, &PIN_LAYOUT_PATH)?,
            pages_path: read_pinned(pe, &PIN_PAGES_PATH)?,
            page_list_end: read_pinned(pe, &PIN_PAGE_LIST_END)?,
            page_name_key: read_pinned(pe, &PIN_PAGE_NAME_KEY)?,
            frame_word: read_pinned(pe, &PIN_FRAME_WORD)?,
            widget_tags,
            page_tags,
            screens,
            base_section: read_pinned(pe, &PIN_BASE_SECTION)?,
            hud_prefix: read_pinned(pe, &PIN_HUD_PREFIX)?,
            hud_set0: read_pinned(pe, &PIN_HUD_SET0)?,
            font_prefix: read_pinned(pe, &PIN_FONT_PREFIX)?,
            light_prefix: read_pinned(pe, &PIN_LIGHT_PREFIX)?,
            fx_prefix: read_pinned(pe, &PIN_FX_PREFIX)?,
            transition_prefix: read_pinned(pe, &PIN_TRANSITION_PREFIX)?,
            vector_font_section: read_pinned(pe, &PIN_VECTOR_FONT_SECTION)?,
            key_background: read_pinned(pe, &PIN_KEY_BACKGROUND)?,
            key_base_ints: arr(pe, &PIN_KEY_BASE_INTS)?,
            key_screen: arr(pe, &PIN_KEY_SCREEN)?,
            key_name: read_pinned(pe, &PIN_KEY_NAME)?,
            key_font: arr(pe, &PIN_KEY_FONT)?,
            key_light: arr(pe, &PIN_KEY_LIGHT)?,
            key_fx: arr(pe, &PIN_KEY_FX)?,
            key_hud_base: arr(pe, &PIN_KEY_HUD_BASE)?,
            sections,
        })
    }

    /// A self-consistent set of invented names for tests and tools without a ROM. None of them is a 2009
    /// name; fixtures written with these names exercise the same parsers.
    pub fn placeholder() -> Self {
        let s = |x: &str| x.to_string();
        let kinds = [
            "win", "img", "btn", "txt", "lst", "inv", "edt", "chk", "lat", "xml",
        ];
        UiNames {
            layout_path: s("ui\\look.txt"),
            pages_path: s("ui\\pages.txt"),
            page_list_end: s("[[all_pages_done]]"),
            page_name_key: s("page_title"),
            frame_word: s("FRM"),
            widget_tags: std::array::from_fn(|i| {
                (format!("<{}>", kinds[i]), format!("<{}_end>", kinds[i]))
            }),
            page_tags: std::array::from_fn(|i| format!("[[pg{i}]]")),
            screens: std::array::from_fn(|i| format!("Scr{i}")),
            base_section: s("Base"),
            hud_prefix: s("Hud"),
            hud_set0: s("Hud0"),
            font_prefix: s("Fnt"),
            light_prefix: s("Lamp"),
            fx_prefix: s("Ef"),
            transition_prefix: s("Trans"),
            vector_font_section: s("VecFonts"),
            key_background: s("Bg"),
            key_base_ints: std::array::from_fn(|i| format!("BaseInt{i}")),
            key_screen: std::array::from_fn(|i| format!("ScrKey{i}")),
            key_name: s("Nm"),
            key_font: [s("File"), s("Face"), s("W"), s("H"), s("B"), s("I"), s("U")],
            key_light: [s("P"), s("Col"), s("R")],
            key_fx: [s("Eff"), s("Par")],
            key_hud_base: std::array::from_fn(|i| format!("HudBase{i}")),
            sections: UiSection::ALL.iter().map(|x| format!("{x:?}")).collect(),
        }
    }

    /// Name of a [`UiSection`].
    pub fn section(&self, s: UiSection) -> &str {
        let i = UiSection::ALL.iter().position(|x| *x == s).unwrap_or(0);
        self.sections.get(i).map(|x| x.as_str()).unwrap_or("")
    }

    /// Splits a table path into `(archive file name, entry name)`: the folder is the archive's file stem
    /// (SPEC: FMT_CRES_UI.md §6.3 "folder = rez file stem").
    pub fn split_table_path(path: &str) -> Option<(String, String)> {
        let norm = path.replace('/', "\\");
        let (folder, entry) = norm.rsplit_once('\\')?;
        let stem = folder.rsplit('\\').next()?;
        if stem.is_empty() || entry.is_empty() {
            return None;
        }
        // The archive stores the entry under that folder, so the lookup name is the full relative path
        // (dc-rez resolves by full path only).
        Some((format!("{stem}.rez"), format!("{stem}\\{entry}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_matching() {
        let h = sha256_hex(b"abc");
        assert!(matches_pin("abc", &h[..16], 3));
        assert!(!matches_pin("abd", &h[..16], 3));
        assert!(!matches_pin("abc", &h[..16], 4));
    }

    #[test]
    fn split_paths() {
        assert_eq!(
            UiNames::split_table_path("Dir\\Entry.TXT"),
            Some(("Dir.rez".to_string(), "Dir\\Entry.TXT".to_string()))
        );
        assert_eq!(
            UiNames::split_table_path("a/b/c.txt"),
            Some(("b.rez".to_string(), "b\\c.txt".to_string()))
        );
        assert_eq!(UiNames::split_table_path("plain.txt"), None);
    }

    #[test]
    fn placeholder_is_complete() {
        let n = UiNames::placeholder();
        assert_eq!(n.sections.len(), UiSection::ALL.len());
        assert_eq!(n.section(UiSection::Megaphone), "Megaphone");
        assert!(n
            .widget_tags
            .iter()
            .all(|(o, e)| !o.is_empty() && e.starts_with(&o[..o.len() - 1])));
    }

    #[test]
    fn mismatched_cshell_is_refused() {
        // An image without the literals: every read is a typed mismatch, never a panic.
        let pe = PeFile::from_bytes(crate::pe_res::synth_pe_with_rsrc(0x4000, &[0u8; 64])).unwrap();
        assert!(matches!(
            UiNames::from_cshell(&pe),
            Err(UiDataError::LiteralMismatch { .. }) | Err(UiDataError::Rom(_))
        ));
    }
}

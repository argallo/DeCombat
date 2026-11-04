//! The 30-entry screen id table and the 20-entry page tag table, by the client's indices. Variant names are
//! ours and carry only the index (the 2009 names are read from the user's binary into
//! [`UiNames`](crate::names::UiNames)); index 0 of the screen table is the generic fallback screen.
// SPEC: FMT_CRES_UI.md §6.2 (screen id table, cshell+0x64770; name -> id cshell+0x64780), §6.3 (page tags)

use crate::names::UiNames;

macro_rules! index_enum {
    ($(#[$m:meta])* $name:ident, $count:expr, [$($v:ident = $i:expr),* $(,)?]) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u8)]
        pub enum $name { $($v = $i),* }

        impl $name {
            /// Number of entries in the client's table.
            pub const COUNT: usize = $count;
            /// Every entry in index order.
            pub const ALL: [$name; $count] = [$($name::$v),*];
            /// The client's table index.
            pub const fn index(self) -> usize { self as usize }
            /// The entry with table index `i`.
            pub fn from_index(i: usize) -> Option<Self> { Self::ALL.get(i).copied() }
        }
    };
}

index_enum!(
    /// Screen ids of the 2009 screen table (`id * 32 + table base`).
    UiScreen, 30, [
        Generic = 0, Screen1 = 1, Screen2 = 2, Screen3 = 3, Screen4 = 4, Screen5 = 5, Screen6 = 6,
        Screen7 = 7, Screen8 = 8, Screen9 = 9, Screen10 = 10, Screen11 = 11, Screen12 = 12, Screen13 = 13,
        Screen14 = 14, Screen15 = 15, Screen16 = 16, Screen17 = 17, Screen18 = 18, Screen19 = 19,
        Screen20 = 20, Screen21 = 21, Screen22 = 22, Screen23 = 23, Screen24 = 24, Screen25 = 25,
        Screen26 = 26, Screen27 = 27, Screen28 = 28, Screen29 = 29,
    ]
);

index_enum!(
    /// Page tags of the 2009 page tag table (front-end pages of the screen pages table).
    PageTag, 20, [
        Page0 = 0, Page1 = 1, Page2 = 2, Page3 = 3, Page4 = 4, Page5 = 5, Page6 = 6, Page7 = 7,
        Page8 = 8, Page9 = 9, Page10 = 10, Page11 = 11, Page12 = 12, Page13 = 13, Page14 = 14,
        Page15 = 15, Page16 = 16, Page17 = 17, Page18 = 18, Page19 = 19,
    ]
);

/// The id the client stores when a name is not in the screen table. EVIDENCE: cshell+0x64780
pub const UNKNOWN_SCREEN: u32 = 999;

impl UiScreen {
    /// The section name of this screen in the layout table.
    pub fn name(self, names: &UiNames) -> &str {
        &names.screens[self.index()]
    }

    /// Case-insensitive name lookup; `None` where the client yields 999. EVIDENCE: cshell+0x64780
    pub fn from_name(names: &UiNames, name: &str) -> Option<Self> {
        names
            .screens
            .iter()
            .position(|s| s.eq_ignore_ascii_case(name))
            .and_then(Self::from_index)
    }

    /// The raw id the client uses: the table index, or [`UNKNOWN_SCREEN`].
    pub fn raw_id(names: &UiNames, name: &str) -> u32 {
        Self::from_name(names, name)
            .map(|s| s.index() as u32)
            .unwrap_or(UNKNOWN_SCREEN)
    }
}

impl PageTag {
    /// The tag text (`[[...]]`).
    pub fn tag(self, names: &UiNames) -> &str {
        &names.page_tags[self.index()]
    }

    /// The end tag: the tag with `_end` inserted before the closing `]]`; `None` when the tag does not end
    /// in `]]` (the client then parses no page). EVIDENCE: cshell+0x1d2260
    pub fn end_tag_of(tag: &str) -> Option<String> {
        let body = tag.strip_suffix("]]")?;
        Some(format!("{body}_end]]"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_and_lookup() {
        let n = UiNames::placeholder();
        assert_eq!(UiScreen::ALL.len(), 30);
        assert_eq!(PageTag::ALL.len(), 20);
        assert_eq!(UiScreen::Screen29.index(), 29);
        assert_eq!(UiScreen::from_index(30), None);
        assert_eq!(UiScreen::from_name(&n, "scr7"), Some(UiScreen::Screen7));
        assert_eq!(UiScreen::raw_id(&n, "nope"), UNKNOWN_SCREEN);
        assert_eq!(UiScreen::Generic.name(&n), "Scr0");
        assert_eq!(PageTag::Page3.tag(&n), "[[pg3]]");
        assert_eq!(
            PageTag::end_tag_of("[[pg3]]").as_deref(),
            Some("[[pg3_end]]")
        );
        assert_eq!(PageTag::end_tag_of("[pg3]"), None);
    }
}

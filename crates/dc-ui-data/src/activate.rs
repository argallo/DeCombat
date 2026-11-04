//! The activate-type table of the 2009 client: per row, the four text ids a use target shows for its state (on, off,
//! and the two sliding-door states). The target code reads the row of the target's activate entry and the field its
//! state picks, and keeps that text id as the target text (doors: "you can open / close the door").
//!
//! ROM model, as in [`crate::names`]: the table path, the section prefix and the four field keys are read from the
//! user's own `Game/CShell.dll` at the rvas of the literals the loader pushes, and accepted only when their length
//! and SHA-256 prefix match the pins. The table itself is read from the user's archive at run time.
// DISASM: cshell+0x2e5ca0 (loader: sections `<prefix><i>` for i = 0, 1, ... while the section exists; row id = i),
// cshell+0x2e5a00 (row reader: the four fields with the default -1 into row +0x08 .. +0x14), cshell+0x2e5850 (row by
// id), cshell+0x29e828 (the table path literal pushed with the manager), cshell+0x2b3176..0x2b3193 (target text id =
// row field [state], -1 = none)

use crate::error::UiDataError;
use crate::names::{read_pinned, Pin, CSHELL_REL_PATH};
use dc_rez::RezVfs;
use dc_rom::PeFile;
use std::collections::BTreeMap;
use std::path::Path;

/// Pins of the loader's literals: the table path, the section prefix, the four state fields in row order.
// DISASM: cshell+0x29e828 (path VA 0x374f8634), cshell+0x2e5cd0 / 0x2e5d55 (prefix VA 0x374fb780, form `%s%i`),
// cshell+0x2e5a30 / 0x2e5a42 / 0x2e5a54 / 0x2e5a66 (field keys VA 0x374fb778 / 0x374fb76c / 0x374fb758 / 0x374fb744)
const PIN_PATH: Pin = Pin {
    what: "activate table path",
    rva: 0x3f8634,
    sha16: "6a2762a001da820b",
    len: 28,
};
const PIN_PREFIX: Pin = Pin {
    what: "activate section prefix",
    rva: 0x3fb780,
    sha16: "d4be27d425733154",
    len: 12,
};
const PIN_FIELDS: [Pin; 4] = [
    Pin {
        what: "activate field",
        rva: 0x3fb778,
        sha16: "c8563621b92418d0",
        len: 7,
    },
    Pin {
        what: "activate field",
        rva: 0x3fb76c,
        sha16: "3382256c2be89e4a",
        len: 8,
    },
    Pin {
        what: "activate field",
        rva: 0x3fb758,
        sha16: "fe481d86baad6257",
        len: 18,
    },
    Pin {
        what: "activate field",
        rva: 0x3fb744,
        sha16: "606729e4137c04d4",
        len: 19,
    },
];

/// The states an activate entry can be in (the field index of a row).
pub const STATE_ON: u8 = 0;
pub const STATE_OFF: u8 = 1;

/// Most rows the loader reads (a safety bound; the loop ends at the first missing section).
// DC-DEFAULT: CLI-1230 (proposed) 256 rows (the entry's row is a byte)
pub const MAX_ROWS: usize = 256;

/// The activate-type table: row id -> the four text ids (-1 = none).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivateTypes {
    pub rows: BTreeMap<u8, [i32; 4]>,
}

impl ActivateTypes {
    /// Reads the table of the client folder `rom`.
    pub fn from_rom(rom: &Path) -> Result<Self, UiDataError> {
        let pe = PeFile::from_file(&rom.join(CSHELL_REL_PATH[0]).join(CSHELL_REL_PATH[1]))?;
        let path = read_pinned(&pe, &PIN_PATH)?;
        let prefix = read_pinned(&pe, &PIN_PREFIX)?;
        let mut keys: Vec<String> = Vec::with_capacity(4);
        for p in &PIN_FIELDS {
            keys.push(read_pinned(&pe, p)?);
        }
        let vfs = RezVfs::new(rom);
        let text = crate::read_table_text(&vfs, &path, "activate table")?;
        let keys: [&str; 4] = [&keys[0], &keys[1], &keys[2], &keys[3]];
        Ok(Self::parse(&text, &prefix, keys))
    }

    /// The rows of a table text: sections `<prefix>0`, `<prefix>1`, ... up to the first missing one; a field that is
    /// missing reads -1.
    pub fn parse(text: &str, prefix: &str, keys: [&str; 4]) -> Self {
        let doc = dc_bute::ButeDoc::parse(text);
        let mut rows = BTreeMap::new();
        for i in 0..MAX_ROWS {
            let sec = format!("{prefix}{i}");
            if !doc.contains_key(&sec) {
                break;
            }
            rows.insert(i as u8, keys.map(|k| doc.get_int(&sec, k, -1)));
        }
        ActivateTypes { rows }
    }

    /// The target text id of an entry of row `row` in state `state`: `None` when the row is unknown, the state is
    /// outside the four fields or the field is -1.
    pub fn text_id(&self, row: u8, state: u8) -> Option<u32> {
        let v = *self.rows.get(&row)?.get(state as usize)?;
        (v >= 0).then_some(v as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [&str; 4] = ["A", "B", "C", "D"];

    #[test]
    fn rows_until_the_first_missing_section() {
        let t = ActivateTypes::parse(
            "[Row0]\nA = 7\nB = 8\nC = -1\nD = 9\n[Row1]\nA = 3\n[Row3]\nA = 4\n",
            "Row",
            KEYS,
        );
        assert_eq!(t.rows.len(), 2, "Row3 is behind the missing Row2");
        assert_eq!(t.text_id(0, STATE_ON), Some(7));
        assert_eq!(t.text_id(0, STATE_OFF), Some(8));
        assert_eq!(t.text_id(0, 2), None, "-1 = none");
        assert_eq!(t.text_id(0, 3), Some(9));
        assert_eq!(t.text_id(1, STATE_OFF), None, "missing field reads -1");
        assert_eq!(t.text_id(0, 4), None);
        assert_eq!(t.text_id(5, 0), None);
    }
}

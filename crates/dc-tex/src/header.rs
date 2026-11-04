//! The 0xA4-byte container header. SPEC: FMT_DTX.md §2.

use crate::DtxError;

/// Header length in bytes. SPEC: FMT_DTX.md §2 (EVIDENCE: engine+0x3e0e0, read length 0xa4).
pub const HEADER_LEN: usize = 0xA4;
const COMMAND_OFF: usize = 0x24;
const COMMAND_LEN: usize = 128;

/// Decoded header fields. Names are ours. SPEC: FMT_DTX.md §2.
#[derive(Debug, Clone, PartialEq)]
pub struct DtxHeader {
    /// 0x00, must be 0.
    pub kind: u32,
    /// 0x04, top-level width in texels.
    pub width: u16,
    /// 0x06, top-level height.
    pub height: u16,
    /// 0x08, must be -5.
    pub version: i32,
    /// 0x0C, stored mip levels, 1..=8.
    pub level_count: u16,
    /// 0x0E, not consumed by the loader.
    pub section_count: u16,
    /// 0x10, see SPEC §5.
    pub flags: u32,
    /// 0x14, copied only.
    pub user_word: u32,
    /// 0x18.
    pub lod_group: u8,
    /// 0x19, 0 means 8.
    pub upload_levels: u8,
    /// 0x1A, 0 means 3.
    pub format: u8,
    /// 0x1B.
    pub upload_skip: u8,
    /// 0x1C, no known consumer.
    pub unnamed: u8,
    /// 0x1D.
    pub device_priority: u8,
    /// 0x1E, renderer stores value + 1.0.
    pub scale_minus_one: f32,
    /// 0x22, degrees.
    pub angle_deg: i16,
    /// 0x24, NUL-terminated statement list.
    pub command: [u8; COMMAND_LEN],
}

impl Default for DtxHeader {
    fn default() -> Self {
        DtxHeader {
            kind: 0,
            width: 0,
            height: 0,
            version: -5,
            level_count: 1,
            section_count: 0,
            flags: 0,
            user_word: 0,
            lod_group: 0,
            upload_levels: 0,
            format: 3,
            upload_skip: 0,
            unnamed: 0,
            device_priority: 0,
            scale_minus_one: 0.0,
            angle_deg: 0,
            command: [0; COMMAND_LEN],
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl DtxHeader {
    /// Parses and validates the header (kind 0, version -5, level count 1..=8). SPEC: FMT_DTX.md §2.
    pub fn parse(bytes: &[u8]) -> Result<DtxHeader, DtxError> {
        if bytes.len() < HEADER_LEN {
            return Err(DtxError::ShortHeader { len: bytes.len() });
        }
        let b = &bytes[..HEADER_LEN];
        let kind = u32_at(b, 0);
        if kind != 0 {
            return Err(DtxError::BadKind(kind));
        }
        let version = u32_at(b, 8) as i32;
        if version != -5 {
            return Err(DtxError::BadVersion(version));
        }
        let level_count = u16_at(b, 0x0C);
        if !(1..=8).contains(&level_count) {
            return Err(DtxError::BadLevelCount(level_count));
        }
        let mut command = [0u8; COMMAND_LEN];
        command.copy_from_slice(&b[COMMAND_OFF..COMMAND_OFF + COMMAND_LEN]);
        Ok(DtxHeader {
            kind,
            width: u16_at(b, 4),
            height: u16_at(b, 6),
            version,
            level_count,
            section_count: u16_at(b, 0x0E),
            flags: u32_at(b, 0x10),
            user_word: u32_at(b, 0x14),
            lod_group: b[0x18],
            upload_levels: b[0x19],
            format: b[0x1A],
            upload_skip: b[0x1B],
            unnamed: b[0x1C],
            device_priority: b[0x1D],
            scale_minus_one: f32::from_bits(u32_at(b, 0x1E)),
            angle_deg: u16_at(b, 0x22) as i16,
            command,
        })
    }

    /// Serialises the header to its 0xA4-byte layout (no validation).
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&self.kind.to_le_bytes());
        b[4..6].copy_from_slice(&self.width.to_le_bytes());
        b[6..8].copy_from_slice(&self.height.to_le_bytes());
        b[8..12].copy_from_slice(&self.version.to_le_bytes());
        b[0x0C..0x0E].copy_from_slice(&self.level_count.to_le_bytes());
        b[0x0E..0x10].copy_from_slice(&self.section_count.to_le_bytes());
        b[0x10..0x14].copy_from_slice(&self.flags.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&self.user_word.to_le_bytes());
        b[0x18] = self.lod_group;
        b[0x19] = self.upload_levels;
        b[0x1A] = self.format;
        b[0x1B] = self.upload_skip;
        b[0x1C] = self.unnamed;
        b[0x1D] = self.device_priority;
        b[0x1E..0x22].copy_from_slice(&self.scale_minus_one.to_bits().to_le_bytes());
        b[0x22..0x24].copy_from_slice(&self.angle_deg.to_le_bytes());
        b[COMMAND_OFF..COMMAND_OFF + COMMAND_LEN].copy_from_slice(&self.command);
        b
    }

    /// Sets the command string (truncated to 127 bytes, NUL-terminated).
    pub fn set_command(&mut self, s: &str) {
        self.command = [0; COMMAND_LEN];
        let n = s.len().min(COMMAND_LEN - 1);
        self.command[..n].copy_from_slice(&s.as_bytes()[..n]);
    }

    /// The command string up to the first NUL, bytes mapped as Latin-1.
    pub fn command_str(&self) -> String {
        let end = self
            .command
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(COMMAND_LEN);
        self.command[..end].iter().map(|&c| c as char).collect()
    }

    /// Effective device level count: `upload_levels`, 0 meaning 8. SPEC: FMT_DTX.md §6.
    pub fn effective_upload_levels(&self) -> u8 {
        if self.upload_levels == 0 {
            8
        } else {
            self.upload_levels
        }
    }
}

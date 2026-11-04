//! The player report 0x312 as the client sends it (kind 0). In the wire crate so a lobby test can drive its parser
//! with the client's own frame.
// SPEC: PROTOCOL_2009.md §3.4 (0xe4c bytes: +6 kind, +0xa / +0xe the player keys, wstr[17] name at +0x12, wstr[601]
// text at +0x34; no reply)
// EVIDENCE: cshell+0x273790 (+0xa is the picked general kind and +0xe the detail index, not player keys)

use crate::msg_ids::PLAYER_REPORT;
use crate::tcp::PktWriter;

/// Kind 0: no statistics block, no captured text. `category` / `detail` are the box's two lists.
pub fn packet(name: &str, text: &str, category: u32, detail: u32) -> PktWriter {
    let mut w = PktWriter::new(PLAYER_REPORT);
    w.u32(0)
        .u32(category)
        .u32(detail)
        .wstr(name, 17)
        .at(0x34)
        .wstr(text, 601)
        .at(0xe4c);
    w
}

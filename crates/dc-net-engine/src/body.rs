//! Link datagram bodies as the delivery machine sees them: a receive-side parser that follows the 2009 abandonment
//! rules block by block (so a broken reliable block does not hide the control block in front of it), and the exact
//! bit counts the sender's budget arithmetic needs. Encoding itself is `dc_wire::udp::LinkBody`.

use dc_wire::linkmove::{self, Secret, TRAILER_LEN};
use dc_wire::udp::{
    check8, vlen_bits, BitReader, BitWriter, CtlBlock, LinkBody, RelBlock, CTL_ACK, CTL_BYE,
    CTL_BYE_CODE, CTL_OP_BITS, SACK_BITS, SEQ_BITS,
};

/// A parsed link datagram body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RxBody {
    pub ctl: Option<CtlBlock>,
    pub rel: Option<RelBlock>,
    /// The reliable block had an entry longer than the bits that remain: it is abandoned and nothing after it
    /// (the unreliable block) is parsed.
    pub rel_abandoned: bool,
    pub unrel: Vec<BitWriter>,
    /// An unreliable entry was longer than the bits that remain; entries before it are kept.
    pub unrel_abandoned: bool,
}

/// Parses the body bits that follow `check8`. Missing trailing bits read as 0 (no block).
// SPEC: UDP_TRANSPORT_2009.md §6, §6.2, §7, §8, §9 (engine+0xb4a70, engine+0xb2500, engine+0xb2690, engine+0xb2c20)
pub fn parse_body(body: &[u8]) -> RxBody {
    parse_body_end(body).0
}

/// [`parse_body`] and the bit position where the parse stopped (an abandoned reliable block stops it early).
// SPEC: UDP_TRANSPORT_2009.md §6.2 (same parse)
pub fn parse_body_end(body: &[u8]) -> (RxBody, usize) {
    let mut r = BitReader::new(body);
    let out = parse_into(&mut r);
    (out, r.bit_pos())
}

fn parse_into(r: &mut BitReader<'_>) -> RxBody {
    let mut out = RxBody::default();
    if r.read_bit() {
        out.ctl = Some(read_ctl(r));
    }
    if r.read_bit() {
        // SPEC: UDP_TRANSPORT_2009.md §8 (an entry claiming more bits than remain abandons the reliable block)
        let rel_seq = r.read_bits(SEQ_BITS as usize) as u16;
        let tail_cont = r.read_bit();
        let mut entries = Vec::new();
        loop {
            let len = r.read_vlen() as usize;
            if len > r.remaining_bits() {
                out.rel_abandoned = true;
                return out;
            }
            entries.push(r.read_bit_string(len));
            if !r.read_bit() {
                break;
            }
        }
        out.rel = Some(RelBlock {
            rel_seq,
            tail_cont,
            entries,
        });
    }
    if r.read_bit() {
        // SPEC: UDP_TRANSPORT_2009.md §9 (entries until vlen 0 or the end; an overlong entry abandons the block)
        while !r.at_end() {
            let len = r.read_vlen() as usize;
            if len == 0 {
                break;
            }
            if len > r.remaining_bits() {
                out.unrel_abandoned = true;
                break;
            }
            out.unrel.push(r.read_bit_string(len));
        }
    }
    out
}

/// The link move secret a datagram (check byte included) carries behind its body (NET-24, `dc_wire::linkmove`):
/// a good check byte over body and trailer, a body that parses to its end, and exactly the trailer after the byte
/// where the body stops. `None` for every 2009 datagram.
// DC-DEFAULT: NET-24 (proposed, project decision) the trailer of the link move secret
pub fn move_trailer(datagram: &[u8]) -> Option<Secret> {
    let (&got, body) = datagram.split_first()?;
    if body.len() < TRAILER_LEN || check8(body) != got {
        return None;
    }
    let (rx, end) = parse_body_end(body);
    if rx.rel_abandoned || rx.unrel_abandoned {
        return None;
    }
    linkmove::from_trailer(body.get(end.div_ceil(8)..)?)
}

/// What a body still needs before a trailer can follow it: the 2009 framing ends a body where its bits end, so a
/// body that stops before its unreliable block, or one whose unreliable block runs to the end, gets its framing
/// closed explicitly (NET-24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenEnd {
    /// `[has_ctl][ctl]` only (a bye): `has_rel = 0`, `has_unrel = 0` follow.
    AfterCtl,
    /// Through the reliable block, no unreliable block: `has_unrel = 0` follows.
    AfterRel,
    /// Ends inside the unreliable block: a zero-length entry (`vlen 0`) follows.
    InUnrel,
}

/// The sealed datagram of `body` with the trailer of `secret` (NET-24): the framing closed as `end` says, the bits
/// zero-padded to a byte, the trailer, the check byte over all of it.
// DC-DEFAULT: NET-24 (proposed, project decision)
pub fn seal_with_trailer(body: &BitWriter, end: OpenEnd, secret: &Secret) -> Vec<u8> {
    let mut w = body.clone();
    match end {
        OpenEnd::AfterCtl => {
            w.write_bit(false).write_bit(false);
        }
        OpenEnd::AfterRel => {
            w.write_bit(false);
        }
        OpenEnd::InUnrel => {
            w.write_vlen(0);
        }
    }
    let mut bytes = w.as_bytes().to_vec();
    bytes.extend_from_slice(&linkmove::trailer(secret));
    let mut out = Vec::with_capacity(bytes.len() + 1);
    out.push(check8(&bytes));
    out.extend_from_slice(&bytes);
    out
}

/// Most bytes [`seal_with_trailer`] adds to a body: one byte of closed framing and padding, and the trailer.
pub const TRAILER_EXTRA_BYTES: usize = 1 + TRAILER_LEN;

/// Control block reader. Ops other than 0, 1, 3 read no body (the 2009 receiver then continues from that
/// position, which is why a sender must never produce them).
// SPEC: UDP_TRANSPORT_2009.md §7 (engine+0xb2500)
fn read_ctl(r: &mut BitReader<'_>) -> CtlBlock {
    match r.read_bits(CTL_OP_BITS as usize) as u8 {
        CTL_ACK => {
            let ack_seq = r.read_bits(SEQ_BITS as usize) as u16;
            let sack_mask = if r.read_bit() {
                Some(r.read_bits(SACK_BITS as usize) as u8)
            } else {
                None
            };
            CtlBlock::Ack { ack_seq, sack_mask }
        }
        CTL_BYE => CtlBlock::Bye,
        CTL_BYE_CODE => CtlBlock::ByeCode(r.read_u32()),
        op => CtlBlock::Unknown(op),
    }
}

/// Bits of a control block (op included).
// SPEC: UDP_TRANSPORT_2009.md §7
pub fn ctl_bits(c: &CtlBlock) -> u32 {
    CTL_OP_BITS
        + match c {
            CtlBlock::Ack { sack_mask, .. } => {
                SEQ_BITS + 1 + if sack_mask.is_some() { SACK_BITS } else { 0 }
            }
            CtlBlock::Bye | CtlBlock::Unknown(_) => 0,
            CtlBlock::ByeCode(_) => 32,
        }
}

/// Bits of one block entry `[vlen L][L bits]`.
pub fn entry_bits(len: u32) -> u32 {
    vlen_bits(len) + len
}

/// Bits of a reliable block: `rel_seq` + `tail_cont`, the entries with a `more` bit between them, and the closing
/// `more = 0`. No entries = the keep-alive form (one zero-length entry).
// SPEC: UDP_TRANSPORT_2009.md §8 (engine+0xb2e20)
pub fn rel_block_bits(entries: &[BitWriter]) -> u32 {
    let head = SEQ_BITS + 1;
    if entries.is_empty() {
        return head + entry_bits(0) + 1;
    }
    let body: u32 = entries.iter().map(|e| entry_bits(e.bit_len() as u32)).sum();
    head + body + (entries.len() as u32 - 1) + 1
}

/// Bits of a whole body as `LinkBody::encode_bits` writes it.
// SPEC: UDP_TRANSPORT_2009.md §6.2
pub fn body_bits(body: &LinkBody) -> u32 {
    let mut n = 1 + body.ctl.as_ref().map_or(0, ctl_bits);
    n += 1 + body.rel.as_ref().map_or(0, |r| rel_block_bits(&r.entries));
    let unrel: Vec<&BitWriter> = body.unrel.iter().filter(|e| !e.is_empty()).collect();
    if !unrel.is_empty() {
        n += 1 + unrel
            .iter()
            .map(|e| entry_bits(e.bit_len() as u32))
            .sum::<u32>();
    }
    n
}

/// Bits `[start, start + len)` of `msg` as a new bit string.
pub fn slice_bits(msg: &BitWriter, start: usize, len: usize) -> BitWriter {
    let mut r = msg.reader();
    r.seek_bits(start);
    r.read_bit_string(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_wire::udp::{decode_link_datagram, encode_link_datagram, seal_link_body};

    fn bits(n: usize, seed: u8) -> BitWriter {
        let bytes: Vec<u8> = (0..n.div_ceil(8))
            .map(|i| seed.wrapping_mul(31).wrapping_add(i as u8))
            .collect();
        BitWriter::from_bits(&bytes, n)
    }

    #[test]
    fn sizes_match_the_encoder() {
        let cases = vec![
            LinkBody::default(),
            LinkBody {
                ctl: Some(CtlBlock::Ack {
                    ack_seq: 77,
                    sack_mask: Some(5),
                }),
                rel: Some(RelBlock {
                    rel_seq: 9,
                    tail_cont: false,
                    entries: vec![],
                }),
                unrel: vec![],
            },
            LinkBody {
                ctl: Some(CtlBlock::Ack {
                    ack_seq: 8191,
                    sack_mask: None,
                }),
                rel: Some(RelBlock {
                    rel_seq: 1,
                    tail_cont: true,
                    entries: vec![bits(3, 1), bits(200, 2), bits(1500, 3), bits(0, 4)],
                }),
                unrel: vec![bits(127, 5), bits(128, 6), bits(1023, 7), bits(5000, 8)],
            },
            LinkBody {
                ctl: Some(CtlBlock::ByeCode(6)),
                rel: None,
                unrel: vec![],
            },
        ];
        for b in cases {
            assert_eq!(body_bits(&b) as usize, b.encode_bits().bit_len(), "{b:?}");
        }
    }

    #[test]
    fn parser_agrees_with_the_wire_decoder() {
        let b = LinkBody {
            ctl: Some(CtlBlock::Ack {
                ack_seq: 4000,
                sack_mask: Some(0x81),
            }),
            rel: Some(RelBlock {
                rel_seq: 4001,
                tail_cont: false,
                entries: vec![bits(40, 1), bits(700, 2)],
            }),
            unrel: vec![bits(64, 3), bits(9, 4)],
        };
        let d = encode_link_datagram(&b);
        let wire = decode_link_datagram(&d).unwrap();
        let ours = parse_body(&d[1..]);
        assert_eq!(ours.ctl, wire.ctl);
        assert_eq!(ours.rel, wire.rel);
        assert_eq!(ours.unrel, wire.unrel);
        assert!(!ours.rel_abandoned && !ours.unrel_abandoned);
    }

    #[test]
    fn overlong_reliable_entry_keeps_the_control_block() {
        // [has_ctl=1][ack 5, no sack][has_rel=1][seq 2][tail 0][vlen 100] and then only a few bits
        let mut w = BitWriter::new();
        w.write_bit(true)
            .write_bits(CTL_ACK as u32, 4)
            .write_bits(5, 13)
            .write_bit(false)
            .write_bit(true)
            .write_bits(2, 13)
            .write_bit(false)
            .write_vlen(100)
            .write_bits(0x3, 2);
        let d = seal_link_body(&w);
        let rx = parse_body(&d[1..]);
        assert_eq!(
            rx.ctl,
            Some(CtlBlock::Ack {
                ack_seq: 5,
                sack_mask: None
            })
        );
        assert!(rx.rel.is_none() && rx.rel_abandoned);
    }

    #[test]
    fn slicing() {
        let m = bits(300, 9);
        let mut joined = slice_bits(&m, 0, 123);
        joined.append(&slice_bits(&m, 123, 177));
        assert_eq!(joined, m);
    }
}

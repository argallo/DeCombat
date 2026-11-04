//! The client side of the join: `OOB_JOIN_ASK` every 300 ms until an `OOB_JOIN_REPLY` arrives or 10 s passed.

use crate::link::{Link, LinkConfig};
use crate::time::{since, Millis};
use dc_wire::udp::{
    OobDatagram, JOIN_REFUSED, JOIN_RETRY_MS, JOIN_TIMED_OUT, JOIN_TIMEOUT_MS, JOIN_WRONG_APP,
};

/// State of a join attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinStatus {
    Pending,
    /// `accepted = 1`: create the link for the reply's source address.
    Accepted,
    /// `accepted = 0, same_app = 1` (result 0x51).
    Refused,
    /// `accepted = 0, same_app = 0` (result 0x52).
    WrongApp,
    /// No reply within 10 s of the first ask (result 0x4F).
    TimedOut,
}

impl JoinStatus {
    /// The engine start result code of a failed join.
    // SPEC: UDP_TRANSPORT_2009.md §5.2 (0x4F timeout, 0x51 refused, 0x52 wrong app)
    pub fn result_code(self) -> Option<u32> {
        match self {
            JoinStatus::Refused => Some(JOIN_REFUSED),
            JoinStatus::WrongApp => Some(JOIN_WRONG_APP),
            JoinStatus::TimedOut => Some(JOIN_TIMED_OUT),
            JoinStatus::Pending | JoinStatus::Accepted => None,
        }
    }
}

/// One join attempt. The ask is built once; every datagram received while pending is consumed, and only an
/// out-of-band opcode 3 counts (its source is not checked: the link peer is whoever answered).
// SPEC: UDP_TRANSPORT_2009.md §5.2 (engine+0xb6850)
#[derive(Debug, Clone)]
pub struct Joiner {
    ask: Vec<u8>,
    first_send: Option<Millis>,
    last_send: Millis,
    status: JoinStatus,
    timeout_ms: u32,
}

impl Joiner {
    pub fn new(app_id: [u8; 16], rate_bps: u32) -> Self {
        Joiner {
            ask: OobDatagram::JoinAsk { app_id, rate_bps }.encode(),
            first_send: None,
            last_send: 0,
            status: JoinStatus::Pending,
            timeout_ms: JOIN_TIMEOUT_MS,
        }
    }

    /// The same join with another give-up time. The 2009 client's 10 s is the default; a test or load tool whose
    /// server may be starved of CPU passes a longer one (the asks every 300 ms continue until then).
    // DC-DEFAULT: BOT-50 (proposed) the bot's join limit is configurable, see DC_DEFAULTS.md
    pub fn with_timeout(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = timeout_ms.max(1);
        self
    }

    pub fn status(&self) -> JoinStatus {
        self.status
    }

    /// The ask to send now, if one is due: at once, then whenever more than 299 ms passed since the last one.
    /// Also moves to `TimedOut` when more than 9999 ms passed since the first ask (the configured limit minus one).
    pub fn poll(&mut self, now: Millis) -> Option<Vec<u8>> {
        if self.status != JoinStatus::Pending {
            return None;
        }
        match self.first_send {
            None => {
                self.first_send = Some(now);
                self.last_send = now;
                Some(self.ask.clone())
            }
            Some(first) => {
                if since(now, first) >= self.timeout_ms {
                    self.status = JoinStatus::TimedOut;
                    return None;
                }
                if since(now, self.last_send) >= JOIN_RETRY_MS {
                    self.last_send = now;
                    return Some(self.ask.clone());
                }
                None
            }
        }
    }

    /// Consumes a datagram received while joining.
    pub fn handle_datagram(&mut self, bytes: &[u8]) -> JoinStatus {
        if self.status != JoinStatus::Pending {
            return self.status;
        }
        if let Ok(OobDatagram::JoinReply { accepted, same_app }) = OobDatagram::decode(bytes) {
            self.status = match (accepted, same_app) {
                (true, _) => JoinStatus::Accepted,
                (false, true) => JoinStatus::Refused,
                (false, false) => JoinStatus::WrongApp,
            };
        }
        self.status
    }

    /// The client link after an accepted join (rate `0x7FFFFFFF` until the session layer lowers it).
    pub fn into_link(self, now: Millis) -> Option<Link> {
        (self.status == JoinStatus::Accepted).then(|| Link::new(now, LinkConfig::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_every_300ms_and_time_out_after_10s() {
        let mut j = Joiner::new([1; 16], 35_840);
        let mut sends = Vec::new();
        for t in 0..11_000u32 {
            if j.poll(t).is_some() {
                sends.push(t);
            }
        }
        assert_eq!(sends[0], 0);
        assert_eq!(sends[1], 300);
        assert!(sends.windows(2).all(|w| w[1] - w[0] == 300));
        assert_eq!(*sends.last().unwrap(), 9900);
        assert_eq!(j.status(), JoinStatus::TimedOut);
        assert_eq!(j.status().result_code(), Some(0x4F));
    }

    #[test]
    fn a_longer_limit_keeps_asking() {
        let mut j = Joiner::new([1; 16], 35_840).with_timeout(60_000);
        let mut asks = 0;
        for t in 0..59_999u32 {
            if j.poll(t).is_some() {
                asks += 1;
            }
        }
        assert_eq!(j.status(), JoinStatus::Pending);
        assert_eq!(asks, 200, "one ask every 300 ms");
        j.poll(60_000);
        assert_eq!(j.status(), JoinStatus::TimedOut);
    }

    #[test]
    fn reply_outcomes() {
        for (acc, same, want) in [
            (true, false, JoinStatus::Accepted),
            (false, true, JoinStatus::Refused),
            (false, false, JoinStatus::WrongApp),
        ] {
            let mut j = Joiner::new([1; 16], 35_840);
            j.poll(0);
            assert_eq!(j.handle_datagram(&[0xff, 0x00]), JoinStatus::Pending);
            let r = OobDatagram::JoinReply {
                accepted: acc,
                same_app: same,
            }
            .encode();
            assert_eq!(j.handle_datagram(&r), want);
        }
    }
}

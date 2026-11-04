//! The network overlay (`/network` in the chat line; our own feature, CLI-1570..1574): the link numbers of the
//! running session in small HUD text at the top left, refreshed every 250 ms, and with `/network 2` one row per
//! remote player. Everything comes from data the client already has ([`crate::net::meter`], the playout tracks,
//! the 0xa2 ping table in the roster); nothing is sent and nothing goes to the event log.
// DC-DEFAULT: CLI-1570 (proposed) the command, the modes and where the choice is kept
// DC-DEFAULT: CLI-1573 (proposed) the lines and their order
// DC-DEFAULT: CLI-1574 (proposed) placement and font

use super::items::DrawItem;
use super::layout::{HudLayout, TextStyle};
use super::state::HudState;
use crate::net::meter::MeterView;

/// How often the overlay's numbers are refreshed (ms).
pub const REFRESH_MS: u32 = 250;
/// Top-left corner of the panel, its padding, the text cell and the line step (virtual 1024 x 768 pixels).
pub const ORIGIN: [f32; 2] = [6.0, 6.0];
pub const PAD: f32 = 4.0;
pub const CELL: i32 = 12;
pub const LINE_STEP: f32 = 13.0;
/// Panel background (black, half transparent) and text colour.
pub const BACKGROUND: [u8; 4] = [0, 0, 0, 128];
pub const TEXT_COLOUR: [u8; 4] = [235, 235, 235, 255];
/// Width of the panel when no text width is known (no font), virtual pixels.
pub const FALLBACK_WIDTH: f32 = 300.0;

/// The modes: off, the link block, the link block and the players block.
pub const OFF: u8 = 0;
pub const LINK: u8 = 1;
pub const PLAYERS: u8 = 2;

/// What `/network` was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetArg {
    /// `/network`, `/net`: on (link block) or off.
    Toggle,
    /// `/network 2`: the players block on, or back to the link block.
    Players,
    /// `/network off`, `/network 0`.
    Off,
    /// Any other argument: nothing changes (the line is not sent either).
    Unknown,
}

/// The mode after `arg` in mode `cur`.
pub fn next_mode(cur: u8, arg: NetArg) -> u8 {
    match arg {
        NetArg::Toggle => {
            if cur == OFF {
                LINK
            } else {
                OFF
            }
        }
        NetArg::Players => {
            if cur == PLAYERS {
                LINK
            } else {
                PLAYERS
            }
        }
        NetArg::Off => OFF,
        NetArg::Unknown => cur.min(PLAYERS),
    }
}

/// How a remote player is drawn right now (CLI-1330 / CLI-1331).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extrap {
    /// Between two samples.
    Interp,
    /// Past the newest sample, carried along the stream velocity.
    Extrap,
    /// Past the extrapolation bound: eased back to the newest sample.
    Held,
    /// No sample yet.
    None,
}

impl Extrap {
    pub fn label(self) -> &'static str {
        match self {
            Extrap::Interp => "interp",
            Extrap::Extrap => "extrap",
            Extrap::Held => "held",
            Extrap::None => "-",
        }
    }
}

/// One remote player's row.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerLink {
    pub pid: u8,
    pub name: String,
    /// Since his newest position sample arrived (ms).
    pub age_ms: Option<u32>,
    pub state: Extrap,
}

/// What a network source hands the overlay at one refresh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetSample {
    pub meter: MeterView,
    /// The playout delay of the remote players (mean over the tracks, ms) and the buffer depth (samples not yet
    /// played, mean over the tracks).
    pub delay_ms: Option<f32>,
    pub depth: Option<f32>,
    pub lobby: Option<String>,
    pub server: Option<String>,
    pub players: Vec<PlayerLink>,
}

fn ms(v: Option<f32>) -> String {
    v.map_or_else(|| "-".to_string(), |v| format!("{:.0}", v))
}

fn pct(v: Option<f32>) -> String {
    v.map_or_else(|| "-".to_string(), |v| format!("{v:.1} %"))
}

/// The "felt" delay of another player's movement on this screen: ping + the server update gap + the playout delay
/// (docs/NET_LATENCY.md's budget, stages 2..5 with the path counted as one round trip). `ping` is the measured
/// round trip ([`ping_ms`]).
pub fn felt_ms(s: &NetSample, ping: Option<f32>) -> Option<f32> {
    Some(ping? + s.meter.tick_gap_mean? + s.delay_ms?)
}

/// The ping shown: the mean of the own send-to-ack round trips of the last 5 s, else the server's 0xa2 value for
/// the own player (the same measurement taken on the server's side of the link, DSV-410).
pub fn ping_ms(m: &MeterView, table: Option<u16>) -> Option<f32> {
    m.rtt_mean.or(table.map(f32::from))
}

/// The overlay's lines: `state` gives the 0xa2 pings (roster) and the own id; `fps` the frame rate the loop
/// measured; `mouse_hz` the mouse's motion reports per second over the last second (`None`: no mouse motion for
/// two seconds); `mode` [`LINK`] or [`PLAYERS`]. `sample` is `None` without a network session.
// DC-DEFAULT: CLI-1573 (amended 2026-10-31, user request) a "mouse N reports/s" line after the fps line: the rate
// the mouse's link delivers (Bluetooth about 90..133, USB / 2.4 GHz up to 1000), not something the client can raise
pub fn lines(
    sample: Option<&NetSample>,
    state: &HudState,
    fps: f32,
    mouse_hz: Option<f32>,
    mode: u8,
) -> Vec<String> {
    let frame = if fps > 0.0 { 1000.0 / fps } else { 0.0 };
    let fps_line = format!("fps {fps:.0}  frame {frame:.2} ms");
    let mouse_line = match mouse_hz {
        Some(hz) => format!("mouse {hz:.0} reports/s"),
        None => "mouse -  (no motion)".to_string(),
    };
    let Some(s) = sample else {
        return vec!["no network session".to_string(), fps_line, mouse_line];
    };
    let m = &s.meter;
    let table = state
        .own_pid
        .and_then(|p| state.roster.get(&p))
        .and_then(|r| r.ping);
    let mut out = Vec::with_capacity(12 + s.players.len());
    let ping = ping_ms(m, table);
    out.push(format!(
        "ping {} ms  min {}  max {}  (table {})",
        ms(ping),
        ms(m.rtt_min),
        ms(m.rtt_max),
        table.map_or_else(|| "-".to_string(), |p| p.to_string())
    ));
    out.push(format!(
        "felt {} ms  (ping {} + update gap {} + playout {})",
        ms(felt_ms(s, ping)),
        ms(ping),
        ms(m.tick_gap_mean),
        ms(s.delay_ms)
    ));
    out.push(format!(
        "jitter {} ms",
        m.jitter
            .map_or_else(|| "-".to_string(), |j| format!("{j:.1}"))
    ));
    out.push(format!("loss in {}  out {}", pct(m.loss_in), pct(m.loss_out)));
    out.push(format!(
        "in {:.0} pkt/s {:.1} kbit/s  out {:.0} pkt/s {:.1} kbit/s",
        m.pps_in, m.kbps_in, m.pps_out, m.kbps_out
    ));
    let rate = m
        .tick_gap_mean
        .filter(|g| *g > 0.0)
        .map_or_else(|| "-".to_string(), |g| format!("{:.1}", 1000.0 / g));
    out.push(format!(
        "server {rate} /s  gap {} ms  max {}",
        ms(m.tick_gap_mean),
        m.tick_gap_max
            .map_or_else(|| "-".to_string(), |g| g.to_string())
    ));
    out.push(format!(
        "playout {} ms  buffer {}",
        ms(s.delay_ms),
        s.depth
            .map_or_else(|| "-".to_string(), |d| format!("{d:.1}"))
    ));
    out.push(format!(
        "late {}  held {}  (frames, 5 s)",
        m.late_frames, m.held_frames
    ));
    out.push(fps_line);
    out.push(mouse_line);
    out.push(format!(
        "last update {} ms ago",
        m.since_update
            .map_or_else(|| "-".to_string(), |t| t.to_string())
    ));
    out.push(format!(
        "lobby {}  game {}",
        s.lobby.as_deref().unwrap_or("-"),
        s.server.as_deref().unwrap_or("-")
    ));
    if mode >= PLAYERS {
        out.push("player  ping  age  state".to_string());
        for p in &s.players {
            let ping = state
                .roster
                .get(&p.pid)
                .and_then(|r| r.ping)
                .map_or_else(|| "-".to_string(), |v| format!("{v} ms"));
            out.push(format!(
                "{}  {ping}  {}  {}",
                p.name,
                p.age_ms
                    .map_or_else(|| "-".to_string(), |a| format!("{a} ms")),
                p.state.label()
            ));
        }
    }
    out
}

/// The text style of the overlay: the chat's font (the HUD font) at [`CELL`].
pub fn style(layout: &HudLayout) -> TextStyle {
    TextStyle {
        font: layout.chat.font,
        colour: TEXT_COLOUR,
        size: Some(CELL),
    }
}

/// The panel's draw items: one background fill sized to the widest line (`widest`, virtual pixels; 0 = unknown)
/// and one text item per line, top left.
pub fn push_items(layout: &HudLayout, state: &HudState, widest: f32, out: &mut Vec<DrawItem>) {
    if state.net_mode == OFF || state.net_lines.is_empty() {
        return;
    }
    let w = if widest > 0.0 { widest } else { FALLBACK_WIDTH };
    let h = state.net_lines.len() as f32 * LINE_STEP;
    out.push(DrawItem::Fill {
        rect: [ORIGIN[0], ORIGIN[1], w + 2.0 * PAD, h + 2.0 * PAD],
        colour: BACKGROUND,
    });
    let st = style(layout);
    for (i, l) in state.net_lines.iter().enumerate() {
        out.push(DrawItem::Text {
            style: st,
            pos: [ORIGIN[0] + PAD, ORIGIN[1] + PAD + i as f32 * LINE_STEP],
            text: l.clone(),
            right: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::state::BoardRow;

    #[test]
    fn modes_toggle_and_ignore_unknown_arguments() {
        assert_eq!(next_mode(OFF, NetArg::Toggle), LINK);
        assert_eq!(next_mode(LINK, NetArg::Toggle), OFF);
        assert_eq!(next_mode(PLAYERS, NetArg::Toggle), OFF);
        assert_eq!(next_mode(OFF, NetArg::Players), PLAYERS);
        assert_eq!(next_mode(LINK, NetArg::Players), PLAYERS);
        assert_eq!(next_mode(PLAYERS, NetArg::Players), LINK);
        assert_eq!(next_mode(PLAYERS, NetArg::Off), OFF);
        assert_eq!(next_mode(LINK, NetArg::Unknown), LINK);
        assert_eq!(next_mode(OFF, NetArg::Unknown), OFF);
    }

    fn sample() -> NetSample {
        NetSample {
            meter: MeterView {
                rtt_mean: Some(61.4),
                rtt_min: Some(57.0),
                rtt_max: Some(68.0),
                jitter: Some(3.24),
                rtt_samples: 40,
                loss_in: Some(0.0),
                loss_out: Some(1.3),
                pps_in: 52.0,
                pps_out: 30.0,
                kbps_in: 41.26,
                kbps_out: 12.0,
                tick_gap_mean: Some(33.3),
                tick_gap_max: Some(48),
                since_update: Some(12),
                late_frames: 3,
                held_frames: 0,
            },
            delay_ms: Some(64.0),
            depth: Some(2.0),
            lobby: Some("10.0.0.1:5001".into()),
            server: Some("10.0.0.1:27015".into()),
            players: vec![PlayerLink {
                pid: 2,
                name: "Robin".into(),
                age_ms: Some(20),
                state: Extrap::Interp,
            }],
        }
    }

    #[test]
    fn lines_are_in_the_briefed_order_with_formatted_values() {
        let mut st = HudState {
            own_pid: Some(1),
            ..HudState::default()
        };
        st.roster.insert(
            1,
            BoardRow {
                pid: 1,
                ping: Some(65),
                ..BoardRow::default()
            },
        );
        st.roster.insert(
            2,
            BoardRow {
                pid: 2,
                ping: Some(80),
                ..BoardRow::default()
            },
        );
        let s = sample();
        let l = lines(Some(&s), &st, 250.0, Some(998.6), LINK);
        assert_eq!(l[0], "ping 61 ms  min 57  max 68  (table 65)");
        assert_eq!(l[1], "felt 159 ms  (ping 61 + update gap 33 + playout 64)");
        assert_eq!(l[2], "jitter 3.2 ms");
        assert_eq!(l[3], "loss in 0.0 %  out 1.3 %");
        assert_eq!(l[4], "in 52 pkt/s 41.3 kbit/s  out 30 pkt/s 12.0 kbit/s");
        assert_eq!(l[5], "server 30.0 /s  gap 33 ms  max 48");
        assert_eq!(l[6], "playout 64 ms  buffer 2.0");
        assert_eq!(l[7], "late 3  held 0  (frames, 5 s)");
        assert_eq!(l[8], "fps 250  frame 4.00 ms");
        assert_eq!(l[9], "mouse 999 reports/s");
        assert_eq!(l[10], "last update 12 ms ago");
        assert_eq!(l[11], "lobby 10.0.0.1:5001  game 10.0.0.1:27015");
        assert_eq!(l.len(), 12);
        let l2 = lines(Some(&s), &st, 250.0, None, PLAYERS);
        assert_eq!(l2.len(), 14);
        assert_eq!(l2[13], "Robin  80 ms  20 ms  interp");
        // no session: a note, the frame rate and the mouse line only
        let none = lines(None, &st, 0.0, None, LINK);
        assert_eq!(none[0], "no network session");
        assert_eq!(none[2], "mouse -  (no motion)");
    }
}

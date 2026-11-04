//! Pass / fail of each bot and scenario from what the server and the bot saw, and the report (Markdown + JSON).
//!
//! The thresholds scale with the bot's path ([`Expect::of`]): a line with 300 ms of round trip is not judged by
//! the round trip of a wired one, a line with scheduled blackouts may go quiet for their length, and so on. What
//! never depends on the path: the session must end the way the bot ended it (its own quit, the result screen, the
//! server's normal close), reliable messages are never given up, and the match registers hits.
// DC-DEFAULT: NET-22 (proposed) the network lab's thresholds, see DC_DEFAULTS.md

use std::fmt::Write as _;

use dc_net_engine::impair::{DirSpec, PathSpec};
use serde_json::{json, Value};

use crate::collect::{quantile, BotView, ServerView};

/// The limits one bot is held to.
#[derive(Debug, Clone, PartialEq)]
pub struct Expect {
    /// From the bot's socket opening to its spawn.
    pub join_s: f64,
    /// Median of the server's per-window round trip.
    pub rtt_p50_ms: f64,
    /// 95th percentile of the server's per-window round trip. Looser than the median: the 2009 transport takes a
    /// round-trip sample from a block's FIRST send, so a block resent after a loss burst or a blackout gives a sample
    /// of the whole wait.
    pub rtt_p95_ms: f64,
    /// Share of the player updates the bot sent that the server counted (the path's upstream delivery, with margin).
    pub input_delivery: f64,
    /// Datagrams per second from the client in every 10 s window (acks included: a live client acks at least every
    /// second, NET-12, dead or alive in the game; a dead player sends no updates, so update gaps are no stall test).
    pub min_in_per_s: f64,
    /// Server resends per reliable block sent, percent.
    pub resend_pct: f64,
    /// 5 s windows in the world with nothing received at the bot.
    pub dead_windows: u32,
}

/// The share of datagrams a direction loses on average (random + the bad state's share of the burst chain).
fn loss_share(d: &DirSpec) -> f64 {
    let p = d.loss_permille as f64 / 1000.0;
    match d.burst {
        Some(b) => {
            let bad = b.enter_permille as f64 / (b.enter_permille + b.leave_permille).max(1) as f64;
            bad * b.loss_bad_permille as f64 / 1000.0 + (1.0 - bad) * p
        }
        None => p,
    }
}

impl Expect {
    pub fn of(s: &PathSpec) -> Expect {
        let base_rtt = (s.up.latency_ms + s.down.latency_ms) as f64;
        let jitter = (s.up.jitter_ms + s.down.jitter_ms) as f64;
        let queues = [s.up, s.down]
            .iter()
            .filter(|d| d.rate_kbps.is_some())
            .map(|d| d.queue_ms as f64)
            .sum::<f64>();
        let (bo_max, bo_share) = s
            .blackouts
            .map(|b| {
                (
                    b.max_ms as f64,
                    (b.min_ms + b.max_ms) as f64 / 2.0 / b.every_ms.max(1) as f64,
                )
            })
            .unwrap_or((0.0, 0.0));
        let blackouts = s.blackouts.is_some();
        let bursty = s.up.burst.is_some() || s.down.burst.is_some();
        let limited = [s.up, s.down]
            .iter()
            .any(|d| d.rate_kbps.is_some() || d.pps_cap.is_some() || d.max_datagram.is_some());
        let loss_up = loss_share(&s.up);
        let loss_down = loss_share(&s.down);
        let impaired = !s.is_clean() && (base_rtt > 40.0 || loss_up + loss_down > 0.0 || blackouts);
        let rtt_p50 = 150.0 + base_rtt * 1.5 + jitter * 2.0 + queues;
        Expect {
            join_s: if impaired { 45.0 } else { 20.0 } + bo_max / 1000.0,
            rtt_p50_ms: rtt_p50 + if bursty { 200.0 } else { 0.0 },
            rtt_p95_ms: rtt_p50
                + if bursty { 1500.0 } else { 0.0 }
                + if blackouts { bo_max } else { 0.0 },
            input_delivery: ((1.0 - loss_up) * (1.0 - bo_share) - 0.05).max(0.0),
            min_in_per_s: if blackouts { 0.0 } else { 0.5 },
            resend_pct: 5.0
                + 400.0 * loss_down
                + if bursty { 10.0 } else { 0.0 }
                + 200.0 * bo_share
                + if limited { 5.0 } else { 0.0 },
            dead_windows: if blackouts {
                2 + (bo_max / 5000.0) as u32 * 2
            } else {
                0
            },
        }
    }
}

/// The verdict of one bot.
#[derive(Debug, Clone, PartialEq)]
pub struct BotResult {
    pub nick: String,
    pub net: String,
    pub profile: String,
    pub verdict: &'static str,
    pub reasons: Vec<String>,
    pub json: Value,
}

/// How the session ended, judged: `None` = a normal end.
pub fn unexpected_end(b: &BotView, server: Option<&ServerView>) -> Option<String> {
    let end = b.end.as_str();
    let normal = end.starts_with("quit")
        || end.starts_with("server bye (code 5)")
        || end.starts_with("server gone: lobby")
        || (end.is_empty() && b.stage == "Closed");
    if !normal {
        return Some(if end.is_empty() {
            format!("session ended in stage {:?}", b.stage)
        } else {
            format!("session end: {end}")
        });
    }
    if let Some(s) = server {
        if s.silent_drop {
            return Some("server dropped the client as silent (DSV-490)".into());
        }
        if s.startup_drop {
            return Some("server dropped the client at its start-up deadline".into());
        }
        if let Some((cause, code)) = &s.gone {
            if *code == 3 || cause.contains("Idle") {
                return Some(format!("server closed the link: {cause}, code {code}"));
            }
        }
    }
    None
}

/// Judges one bot.
/// The room's input rate baseline: the best median per-window rate of any of its bots (`None` without windows).
pub fn baseline_rate(servers: &[Option<&ServerView>]) -> Option<f64> {
    servers
        .iter()
        .flatten()
        .filter_map(|s| {
            let r: Vec<f64> = middle(&s.windows).iter().map(|w| w.input_rate).collect();
            quantile(&r, 0.5)
        })
        .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))))
}

/// The windows that cover play: the first holds the join and the world load, the last may be cut short.
fn middle(wins: &[crate::collect::Window]) -> Vec<crate::collect::Window> {
    if wins.len() > 2 {
        wins[1..wins.len() - 1].to_vec()
    } else {
        wins.to_vec()
    }
}

/// Judges one bot (`baseline`: [`baseline_rate`] of its room, reported next to the bot's own rate).
pub fn judge(
    nick: &str,
    net: &str,
    b: &BotView,
    server: Option<&ServerView>,
    baseline: Option<f64>,
    xfail: bool,
) -> BotResult {
    let spec = PathSpec::parse(net).unwrap_or_default();
    let ex = Expect::of(&spec);
    let mut why: Vec<String> = Vec::new();
    if let Some(u) = unexpected_end(b, server) {
        why.push(u);
    }
    match b.spawned_ms {
        None => why.push("never spawned".into()),
        Some(t) if t as f64 / 1000.0 > ex.join_s => why.push(format!(
            "join took {:.1} s (limit {:.0} s)",
            t as f64 / 1000.0,
            ex.join_s
        )),
        _ => {}
    }
    if b.rel_abandoned > 0 {
        why.push(format!("{} reliable blocks given up", b.rel_abandoned));
    }
    let empty = Vec::new();
    let wins = server.map(|s| &s.windows).unwrap_or(&empty);
    let mid = middle(wins);
    let rtts: Vec<f64> = mid.iter().map(|w| w.rtt_ms).collect();
    let rates: Vec<f64> = mid.iter().map(|w| w.input_rate).collect();
    let rtt_p50 = quantile(&rtts, 0.5);
    let rtt_p95 = quantile(&rtts, 0.95);
    let rate_p50 = quantile(&rates, 0.5);
    let rate_min = rates
        .iter()
        .cloned()
        .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x))));
    let gap = mid.iter().map(|w| w.longest_gap_ms).max();
    let largest = wins.iter().map(|w| w.largest_b).max().unwrap_or(0);
    let (s_resends, s_blocks) = server
        .and_then(|s| s.summary_resends)
        .unwrap_or_else(|| (wins.iter().map(|w| w.resends).sum(), 0));
    let resend_pct = (s_blocks > 0).then(|| 100.0 * s_resends as f64 / s_blocks as f64);
    if server.is_none() {
        why.push("no server lines for this client".into());
    }
    if let Some(p) = rtt_p50 {
        if p > ex.rtt_p50_ms {
            why.push(format!("rtt p50 {p:.0} ms (limit {:.0})", ex.rtt_p50_ms));
        }
    }
    if let Some(p) = rtt_p95 {
        if p > ex.rtt_p95_ms {
            why.push(format!("rtt p95 {p:.0} ms (limit {:.0})", ex.rtt_p95_ms));
        }
    }
    let delivery = match (server.and_then(|s| s.summary_updates), b.updates_sent) {
        (Some(got), sent) if sent > 0 => Some(got as f64 / sent as f64),
        _ => None,
    };
    if let Some(d) = delivery {
        if d < ex.input_delivery {
            why.push(format!(
                "server counted {:.0} % of the player updates sent (limit {:.0} %)",
                d * 100.0,
                ex.input_delivery * 100.0
            ));
        }
    }
    let in_min = mid
        .iter()
        .map(|w| w.datagrams_in as f64 / 10.0)
        .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x))));
    if let Some(m) = in_min {
        if m < ex.min_in_per_s {
            why.push(format!(
                "a 10 s window with {m:.1} datagrams/s from the client (limit {:.1})",
                ex.min_in_per_s
            ));
        }
    }
    if let Some(p) = resend_pct {
        if p > ex.resend_pct {
            why.push(format!(
                "server resends {p:.1} % (limit {:.1})",
                ex.resend_pct
            ));
        }
    }
    if b.dead_windows > ex.dead_windows {
        why.push(format!(
            "{} silent 5 s windows at the bot, longest {} s (limit {})",
            b.dead_windows, b.longest_dead_s, ex.dead_windows
        ));
    }
    if largest > 548 {
        why.push(format!("server datagram of {largest} B (NET-10 cap 548)"));
    }
    let verdict = match (why.is_empty(), xfail) {
        (true, false) => "pass",
        (true, true) => "xpass",
        (false, false) => "FAIL",
        (false, true) => "xfail",
    };
    let imp = &b.impairment;
    let json = json!({
        "nick": nick, "net": net, "profile": spec.name, "verdict": verdict, "reasons": why,
        "exit": b.exit_code, "end": b.end, "stage": b.stage,
        "join_s": b.spawned_ms.map(|t| t as f64 / 1000.0),
        "server": {
            "windows": wins.len(), "rtt_p50_ms": rtt_p50, "rtt_p95_ms": rtt_p95,
            "input_rate_p50": rate_p50, "input_rate_min": rate_min, "room_input_rate": baseline,
            "longest_gap_ms": gap, "datagrams_in_min_per_s": in_min, "input_delivery": delivery,
            "resends": s_resends, "reliable_blocks": s_blocks, "resend_pct": resend_pct,
            "largest_datagram": largest, "gone": server.and_then(|s| s.gone.clone()).map(|(c, n)| format!("{c} code {n}")),
            "addresses": server.map(|s| s.linked_from.clone()),
        },
        "bot": {
            "resends": b.resends, "rel_blocks_out": b.rel_blocks_out, "rel_abandoned": b.rel_abandoned,
            "delivered_rel": b.delivered_rel, "datagrams_in": b.datagrams_in, "datagrams_out": b.datagrams_out,
            "l_ms_max": b.l_ms_max, "updates_sent": b.updates_sent, "dead_windows": b.dead_windows, "longest_dead_s": b.longest_dead_s,
            "kills": b.kills, "kills_by_me": b.kills_by_me, "hit_claims": b.hit_claims, "shots": b.shots,
        },
        "path": {
            "dropped_out": imp["dropped_out"], "dropped_in": imp["dropped_in"], "rebinds": imp["rebinds"],
            "flow_kills": imp["flow_kills"], "blackouts": imp["blackouts"], "ports": imp["ports"],
        },
        "limits": {
            "join_s": ex.join_s, "rtt_p50_ms": ex.rtt_p50_ms, "rtt_p95_ms": ex.rtt_p95_ms,
            "input_delivery": ex.input_delivery, "min_in_per_s": ex.min_in_per_s,
            "resend_pct": ex.resend_pct, "dead_windows": ex.dead_windows,
        },
    });
    BotResult {
        nick: nick.to_string(),
        net: net.to_string(),
        profile: spec.name,
        verdict,
        reasons: why,
        json,
    }
}

/// The match-level check: kills happened (hit registration works end to end) when bots were shooting for a while.
pub fn hit_sanity(bots: &[(&BotView, bool)], minutes: f64) -> Option<String> {
    let kills: u64 = bots.iter().map(|(b, _)| b.kills_by_me).sum();
    let claims: u64 = bots.iter().map(|(b, _)| b.hit_claims).sum();
    if minutes < 1.5 {
        return None;
    }
    // no shot at all is no proof against hit registration (a co-op room has no enemy players, a large map may keep
    // the bots apart): the report shows the shots, only claimed hits without a kill fail
    if claims > 20 && kills == 0 {
        return Some(format!("{claims} hits claimed and no kill registered"));
    }
    None
}

/// The Markdown report of a run.
pub fn markdown(run: &Value) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Network lab report\n");
    let _ = writeln!(
        s,
        "- matrix `{}`, {} run, started {}, {} scenarios, verdict **{}**",
        run["matrix"].as_str().unwrap_or(""),
        run["kind"].as_str().unwrap_or(""),
        run["started"].as_str().unwrap_or(""),
        run["scenarios"].as_array().map_or(0, |a| a.len()),
        run["verdict"].as_str().unwrap_or("")
    );
    let _ = writeln!(s, "- command: `{}`", run["command"].as_str().unwrap_or(""));
    let _ = writeln!(
        s,
        "- bots: {} pass, {} FAIL, {} xfail (known open issue), {} xpass\n",
        run["totals"]["pass"],
        run["totals"]["FAIL"],
        run["totals"]["xfail"],
        run["totals"]["xpass"]
    );
    let _ = writeln!(s, "| scenario | mode | map | bots | min | pass | FAIL | xfail | shots | hits claimed | kills | verdict |");
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|---|");
    for sc in run["scenarios"].as_array().into_iter().flatten() {
        let count = |v: &str| {
            sc["bots"]
                .as_array()
                .map_or(0, |a| a.iter().filter(|b| b["verdict"] == v).count())
        };
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            sc["name"].as_str().unwrap_or(""),
            sc["mode"].as_str().unwrap_or(""),
            sc["map"].as_str().unwrap_or(""),
            sc["bots"].as_array().map_or(0, |a| a.len()),
            sc["minutes"],
            count("pass") + count("xpass"),
            count("FAIL"),
            count("xfail"),
            sc["shots"],
            sc["hit_claims"],
            sc["kills"],
            sc["verdict"].as_str().unwrap_or("")
        );
    }
    for sc in run["scenarios"].as_array().into_iter().flatten() {
        let _ = writeln!(
            s,
            "\n## {} ({} on {}, {} min)\n",
            sc["name"].as_str().unwrap_or(""),
            sc["mode"].as_str().unwrap_or(""),
            sc["map"].as_str().unwrap_or(""),
            sc["minutes"]
        );
        if let Some(e) = sc["error"].as_str() {
            let _ = writeln!(s, "**error:** {e}\n");
        }
        if let Some(h) = sc["hit_sanity"].as_str() {
            let _ = writeln!(s, "**hit registration:** {h}\n");
        }
        let _ = writeln!(
            s,
            "| bot | path | join s | rtt p50/p95 | input/s p50 (min) | gap ms | srv resend % | bot resends | dead 5s | drops up/down | rebinds | kills | end | verdict |"
        );
        let _ = writeln!(
            s,
            "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
        );
        let f = |v: &Value, d: usize| v.as_f64().map_or("-".to_string(), |x| format!("{x:.d$}"));
        let n = |v: &Value| {
            if v.is_null() {
                "-".to_string()
            } else {
                v.to_string()
            }
        };
        for b in sc["bots"].as_array().into_iter().flatten() {
            let sv = &b["server"];
            let bv = &b["bot"];
            let pv = &b["path"];
            let _ = writeln!(
                s,
                "| {} | {} | {} | {}/{} | {} ({}) | {} | {} | {} | {} | {}/{} | {} | {} | {} | {} |",
                b["nick"].as_str().unwrap_or(""),
                b["net"].as_str().unwrap_or(""),
                f(&b["join_s"], 1),
                f(&sv["rtt_p50_ms"], 0),
                f(&sv["rtt_p95_ms"], 0),
                f(&sv["input_rate_p50"], 1),
                f(&sv["input_rate_min"], 1),
                n(&sv["longest_gap_ms"]),
                f(&sv["resend_pct"], 1),
                n(&bv["resends"]),
                n(&bv["dead_windows"]),
                n(&pv["dropped_out"]),
                n(&pv["dropped_in"]),
                n(&pv["rebinds"]),
                n(&bv["kills_by_me"]),
                b["end"].as_str().unwrap_or("").replace('|', "/"),
                b["verdict"].as_str().unwrap_or("")
            );
        }
        let failing: Vec<&Value> = sc["bots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["verdict"] == "FAIL" || b["verdict"] == "xfail")
            .collect();
        if !failing.is_empty() {
            let _ = writeln!(s, "\nWhy:\n");
            for b in failing {
                let reasons: Vec<&str> = b["reasons"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                let _ = writeln!(
                    s,
                    "- {} ({}, {}): {}",
                    b["nick"].as_str().unwrap_or(""),
                    b["net"].as_str().unwrap_or(""),
                    b["verdict"].as_str().unwrap_or(""),
                    reasons.join("; ")
                );
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_follow_the_path() {
        let good = Expect::of(&PathSpec::parse("good").unwrap());
        let aus = Expect::of(&PathSpec::parse("australia").unwrap());
        let bo = Expect::of(&PathSpec::parse("blackout").unwrap());
        assert!(aus.rtt_p50_ms > good.rtt_p50_ms + 400.0);
        assert!(bo.rtt_p95_ms >= good.rtt_p95_ms + 10_000.0 && bo.dead_windows > 0);
        assert_eq!(good.dead_windows, 0);
        assert!(good.join_s <= 20.0);
        let burst = Expect::of(&PathSpec::parse("lossy_burst").unwrap());
        assert!(burst.resend_pct > 20.0 && burst.input_delivery < good.input_delivery);
        assert!(good.input_delivery > 0.9);
    }

    #[test]
    fn a_normal_end_passes_and_a_drop_fails() {
        let mut b = BotView {
            end: "quit (Local, code 0)".into(),
            stage: "Closed".into(),
            spawned_ms: Some(3000),
            updates_sent: 1000,
            ..BotView::default()
        };
        let sv = ServerView {
            windows: vec![Default::default(); 3],
            gone: Some(("PeerBye".into(), 0)),
            summary_updates: Some(990),
            ..ServerView::default()
        };
        let mut sv_ok = sv.clone();
        for w in sv_ok.windows.iter_mut() {
            w.input_rate = 28.0;
            w.rtt_ms = 30.0;
            w.datagrams_in = 300;
        }
        let r = judge("A", "good", &b, Some(&sv_ok), Some(28.0), false);
        assert_eq!(r.verdict, "pass", "{:?}", r.reasons);
        let mut few = sv_ok.clone();
        few.summary_updates = Some(500);
        let r = judge("A", "good", &b, Some(&few), Some(28.0), false);
        assert_eq!(r.verdict, "FAIL", "half the updates lost on a good path");
        b.end = "link closed: IdleTimeout, code 3".into();
        let r = judge("A", "good", &b, Some(&sv_ok), Some(28.0), false);
        assert_eq!(r.verdict, "FAIL");
        let r = judge("A", "cgnat_rebind", &b, Some(&sv_ok), Some(28.0), true);
        assert_eq!(r.verdict, "xfail");
    }
}

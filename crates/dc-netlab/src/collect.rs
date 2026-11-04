//! Reading a run's evidence: the game servers' per-client link lines (CLI-1331 / DSV-412 format, the same lines
//! `tools/net_report.py` reads) and each bot's report and event log.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

/// One 10 s window of the server's link line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Window {
    /// The window's raw round-trip samples' mean (else the transport estimate).
    pub rtt_ms: f64,
    pub rtt_max_ms: f64,
    pub input_rate: f64,
    pub jitter_ms: f64,
    pub longest_gap_ms: u64,
    pub resends: u64,
    pub flow_drops: u64,
    pub datagrams_in: u64,
    pub datagrams_out: u64,
    pub largest_b: u64,
}

/// What one game server logged about one client.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerView {
    pub log: String,
    pub client: u32,
    pub windows: Vec<Window>,
    pub linked_from: Vec<String>,
    /// The `gone (<cause>, code <n>)` part.
    pub gone: Option<(String, u32)>,
    pub silent_drop: bool,
    pub startup_drop: bool,
    pub summary_resends: Option<(u64, u64)>,
    pub summary_updates: Option<u64>,
    pub summary_secs: Option<u64>,
    pub in_world: bool,
}

/// The number right after `key` in `s` (digits, `.`).
pub fn num_after(s: &str, key: &str) -> Option<f64> {
    let i = s.find(key)? + key.len();
    let rest = s[i..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn parse_window(rest: &str) -> Option<Window> {
    let rtt = num_after(rest, "(window mean ").or_else(|| num_after(rest, "rtt "))?;
    Some(Window {
        rtt_ms: rtt,
        rtt_max_ms: num_after(rest, ", max ").unwrap_or(rtt),
        input_rate: num_after(rest, "input ")?,
        jitter_ms: num_after(rest, "(jitter ").unwrap_or(0.0),
        longest_gap_ms: num_after(rest, "longest gap ").unwrap_or(0.0) as u64,
        resends: num_after(rest, "resends ").unwrap_or(0.0) as u64,
        flow_drops: num_after(rest, "flow drops ").unwrap_or(0.0) as u64,
        datagrams_in: num_after(rest, "datagrams in ").unwrap_or(0.0) as u64,
        datagrams_out: num_after(rest, " out ").unwrap_or(0.0) as u64,
        largest_b: num_after(rest, "(largest ").unwrap_or(0.0) as u64,
    })
}

/// Reads one game server log: per hello name, what the server saw. A name that joined twice (a rejoin) keeps the
/// later client's lines and the earlier one's windows.
pub fn read_server_log(path: &Path, out: &mut BTreeMap<String, ServerView>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut names: BTreeMap<u32, String> = BTreeMap::new();
    let mut from: BTreeMap<u32, String> = BTreeMap::new();
    for line in text.lines() {
        let Some(i) = line.find("dc-dserver: client ") else {
            continue;
        };
        let rest = &line[i + "dc-dserver: client ".len()..];
        let id_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let Ok(id) = rest[..id_end].parse::<u32>() else {
            continue;
        };
        let after = &rest[id_end..];
        if let Some(a) = after.strip_prefix(" linked from ") {
            from.insert(id, a.split_whitespace().next().unwrap_or("").to_string());
            continue;
        }
        if let Some(g) = after.strip_prefix(" gone (") {
            if let Some(v) = names.get(&id).and_then(|n| out.get_mut(n)) {
                let cause = g.split(',').next().unwrap_or("").to_string();
                let code = num_after(g, "code ").unwrap_or(0.0) as u32;
                v.gone = Some((cause, code));
            }
            continue;
        }
        let Some(msg) = after.strip_prefix(": ") else {
            continue;
        };
        if let Some(h) = msg.strip_prefix("hello name \"") {
            let name = h.split('"').next().unwrap_or("").to_string();
            names.insert(id, name.clone());
            let v = out.entry(name).or_default();
            v.log = file.clone();
            v.client = id;
            if let Some(a) = from.get(&id) {
                v.linked_from.push(a.clone());
            }
            continue;
        }
        let Some(v) = names.get(&id).and_then(|n| out.get_mut(n)) else {
            continue;
        };
        if msg.starts_with("link summary ") {
            v.summary_secs = num_after(msg, "link summary ").map(|x| x as u64);
            v.summary_updates = num_after(msg, "input ").map(|x| x as u64);
            if let (Some(r), Some(of)) = (num_after(msg, "resends "), num_after(msg, " of ")) {
                v.summary_resends = Some((r as u64, of as u64));
            }
        } else if msg.starts_with("link ") {
            if let Some(w) = parse_window(msg) {
                v.windows.push(w);
            }
        } else if msg.starts_with("nothing received for ") {
            v.silent_drop = true;
        } else if msg.starts_with("in world") {
            v.in_world = true;
        }
    }
    // start-up deadline lines name the address, not the client number
    for line in text.lines() {
        if line.contains("start-up deadline passed") {
            for (id, a) in &from {
                if line.contains(a.as_str()) {
                    if let Some(v) = names.get(id).and_then(|n| out.get_mut(n)) {
                        v.startup_drop = true;
                    }
                }
            }
        }
    }
}

/// Every `*.log` under `dir` (one level of sub-folders too).
pub fn read_server_logs(dir: &Path) -> BTreeMap<String, ServerView> {
    let mut out = BTreeMap::new();
    fn walk(d: &Path, files: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                files.push(e.path());
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    let subs: Vec<_> = files.iter().filter(|p| p.is_dir()).cloned().collect();
    for s in subs {
        walk(&s, &mut files);
    }
    files.sort();
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if name.starts_with("dserver") && name.ends_with(".log") {
            read_server_log(&f, &mut out);
        }
    }
    out
}

/// What one bot's files say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BotView {
    pub report: Value,
    pub exit_code: Option<i32>,
    /// Session ms of the spawn (the match is playable) and of the transport join.
    pub spawned_ms: Option<u64>,
    pub joined_ms: Option<u64>,
    /// Session ms when the link came up (`udp_open` is 0).
    pub end: String,
    pub stage: String,
    /// 5 s link windows in which nothing arrived although the session was in the world.
    pub dead_windows: u32,
    /// The longest run of such windows (s).
    pub longest_dead_s: u32,
    pub resends: u64,
    pub rel_blocks_out: u64,
    pub rel_abandoned: u64,
    pub delivered_rel: u64,
    pub datagrams_in: u64,
    pub datagrams_out: u64,
    pub l_ms_max: f64,
    pub kills: u64,
    pub kills_by_me: u64,
    pub hit_claims: u64,
    pub shots: u64,
    pub impairment: Value,
    pub matches_played: u64,
    /// Player updates the bot sent (all its matches).
    pub updates_sent: u64,
    pub server_gone: bool,
    pub link_closed: Vec<String>,
}

/// Reads a bot's report (`--report`, else its stdout) and event log (`--log`).
pub fn read_bot(report: &Path, stdout: &Path, events: &Path) -> BotView {
    let mut b = BotView::default();
    let rtext = std::fs::read_to_string(report)
        .ok()
        .or_else(|| std::fs::read_to_string(stdout).ok())
        .unwrap_or_default();
    b.report = serde_json::from_str(rtext.trim()).unwrap_or(Value::Null);
    let r = &b.report;
    b.end = r["end"]
        .as_str()
        .map(str::to_string)
        .or_else(|| r["error"].as_str().map(|e| format!("error: {e}")))
        .unwrap_or_default();
    b.stage = r["stage"].as_str().unwrap_or("").to_string();
    for m in r["milestones"].as_array().into_iter().flatten() {
        match (m[0].as_str(), m[1].as_u64()) {
            (Some("spawned"), Some(t)) => b.spawned_ms = b.spawned_ms.or(Some(t)),
            (Some("joined"), Some(t)) => b.joined_ms = b.joined_ms.or(Some(t)),
            _ => {}
        }
    }
    let ls = &r["link_stats"];
    b.resends = ls["resends"].as_u64().unwrap_or(0);
    b.rel_blocks_out = ls["rel_blocks_out"].as_u64().unwrap_or(0);
    b.rel_abandoned = ls["rel_abandoned"].as_u64().unwrap_or(0);
    b.delivered_rel = ls["delivered_rel"].as_u64().unwrap_or(0);
    b.datagrams_in = ls["datagrams_in"].as_u64().unwrap_or(0);
    b.datagrams_out = ls["datagrams_out"].as_u64().unwrap_or(0);
    b.kills = r["kills"]
        .as_array()
        .map(|a| a.len() as u64)
        .or_else(|| r["kills"].as_u64())
        .unwrap_or(0);
    let sh = &r["shooter"];
    b.kills_by_me = sh["kills_by_me"].as_u64().unwrap_or(0);
    b.hit_claims = sh["hit_claims"].as_u64().unwrap_or(0);
    b.shots = sh["shots"].as_u64().unwrap_or(0);
    b.matches_played = r["matches_played"].as_u64().unwrap_or(0);
    b.updates_sent = r["updates_sent"].as_u64().unwrap_or(0);
    let Ok(text) = std::fs::read_to_string(events) else {
        return b;
    };
    let mut last_in: Option<u64> = None;
    let mut run = 0u32;
    let mut in_world = false;
    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match e["ev"].as_str().unwrap_or("") {
            "milestone" => {
                let step = e["step"].as_str().unwrap_or("");
                if step == "spawned" || step == "engine_in_world" {
                    in_world = true;
                }
                if matches!(step, "result_screen" | "leave_world" | "level_exit") {
                    in_world = false;
                }
            }
            "link_stats" => {
                let got = e["in"].as_u64().unwrap_or(0);
                if let Some(l) = e["l_ms"].as_f64() {
                    b.l_ms_max = b.l_ms_max.max(l);
                }
                if in_world && last_in == Some(got) {
                    b.dead_windows += 1;
                    run += 1;
                    b.longest_dead_s = b.longest_dead_s.max(run * 5);
                } else {
                    run = 0;
                }
                last_in = Some(got);
            }
            "impairment" => b.impairment = e.clone(),
            "server_gone" => b.server_gone = true,
            "link_closed" => b.link_closed.push(format!(
                "{} code {}",
                e["cause"].as_str().unwrap_or("?"),
                e["code"]
            )),
            _ => {}
        }
    }
    b
}

/// Nearest-rank quantile (`None` for no values).
pub fn quantile(xs: &[f64], q: f64) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let i = ((q * (s.len() - 1) as f64) + 0.5) as usize;
    Some(s[i.min(s.len() - 1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_lines_are_read_per_hello_name() {
        let dir = std::env::temp_dir().join(format!("dc-netlab-collect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("dserver_c1_r0_g1.log");
        std::fs::write(
            &p,
            "      260 dc-dserver: client 1 linked from 127.0.0.1:59843 (link rate 5120000, app id layout 0)\n\
                   261 dc-dserver: client 1: hello name \"NL01\" (second string 9 bytes)\n\
                   637 dc-dserver: client 1: in world, object 1 (character) at (0.0, 150.0, 0.0), 0 other player(s)\n\
                 10671 dc-dserver: client 1: link 10 s: rtt 80 ms (window mean 82, min 60, max 140, 56 samples), input 27.9/s (jitter 3.2 ms, longest gap 470 ms), resends 3, flow drops 0, datagrams in 399 out 339 (largest 547 B)\n\
                 20671 dc-dserver: client 1: link 10 s: rtt 90 ms, input 25.0/s (jitter 1.0 ms, longest gap 90 ms), resends 0, flow drops 0, datagrams in 300 out 300\n\
                 21000 dc-dserver: client 1: nothing received for 20001 ms, dropping (bye code 3)\n\
                 21001 dc-dserver: client 1 gone (Local, code 3) after 530 player updates\n\
                 21001 dc-dserver: client 1: link summary 21 s: rtt mean 80 ms (min 60, max 140), input 530 updates (25.2/s, jitter 0.3 ms, longest gap 470 ms), resends 3 of 118 reliable blocks, flow drops 0, datagrams in 730 out 603 (largest 547 B)\n",
        )
        .unwrap();
        let v = read_server_logs(&dir);
        let c = &v["NL01"];
        assert_eq!(c.windows.len(), 2);
        assert_eq!(c.windows[0].rtt_ms, 82.0);
        assert_eq!(c.windows[0].rtt_max_ms, 140.0);
        assert_eq!(c.windows[0].longest_gap_ms, 470);
        assert_eq!(c.windows[0].largest_b, 547);
        assert_eq!(c.windows[1].rtt_ms, 90.0);
        assert_eq!(c.windows[1].datagrams_out, 300);
        assert!(c.silent_drop && c.in_world);
        assert_eq!(c.gone, Some(("Local".to_string(), 3)));
        assert_eq!(c.summary_resends, Some((3, 118)));
        assert_eq!(c.linked_from, ["127.0.0.1:59843"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quantiles() {
        assert_eq!(quantile(&[], 0.5), None);
        assert_eq!(quantile(&[3.0, 1.0, 2.0], 0.5), Some(2.0));
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0], 0.95), Some(4.0));
    }
}

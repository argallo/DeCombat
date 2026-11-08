//! `dc-netlab`: the network lab (docs/NETLAB.md). Runs rooms of headless `dc-botclient` players, each on its own
//! impaired path (`dc_net_engine::impair` profiles: Wi-Fi, a 4G hotspot, Australia, a campus firewall, a carrier
//! NAT that rebinds, loss bursts, blackouts), against a local lobby and its game servers (with the user's ROM) or a
//! game server on a synthetic world (no ROM, CI), and judges every player from the server's link lines and the
//! bot's own log: no unexpected drops or stalls, input rate, round trip, resends, time to join, reliable delivery,
//! hits registered. Writes `report.md` and `report.json`.
//!
//! Our own test tooling: nothing of the 2009 game or wire is in here.
// DC-DEFAULT: NET-22 (proposed) network lab runner, see DC_DEFAULTS.md

pub mod collect;
pub mod matrix;
pub mod report;
pub mod stack;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::matrix::Scenario;
use crate::stack::{Bins, BotProc, Direct, Lobby};

/// Profiles whose failures are known open issues today (reported as `xfail`, the run still passes).
// DC-DEFAULT: NET-22 (proposed) known open issues of the lab, see docs/NETLAB.md "Known failures"
pub const DEFAULT_XFAIL: [&str; 0] = [];

/// Everything a run needs.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub matrix: String,
    pub bins: Bins,
    pub rom: Option<PathBuf>,
    pub out: PathBuf,
    pub parallel: usize,
    pub seed: u64,
    pub dserver_args: String,
    pub xfail: Vec<String>,
    /// `--skills <a,b,...>`: the skill level of each bot of a ROM room in join order (the list repeats when the
    /// room has more bots), in place of the balanced 30..65 draw; the lobby deals the joiners to the two teams in
    /// turn, so `85,30` gives one team 85 and the other 30. Empty: the balanced draw.
    pub skills: Vec<u8>,
    pub command: String,
}

/// What a scenario's bots share.
struct Ctx<'a> {
    cfg: &'a RunConfig,
    lobby: Option<&'a Lobby>,
}

/// `n` skill levels in `lo..=hi`, drawn from `seed` and dealt in a snake over the join order (0, 1, 1, 0, 0, 1, ...)
/// so the two teams the lobby fills alternately get sums within one level of each other.
pub fn balanced_levels(seed: u64, n: usize, lo: u8, hi: u8) -> Vec<u8> {
    let mut x = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut draw = || {
        // splitmix64
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        lo + (z % (hi as u64 - lo as u64 + 1)) as u8
    };
    let mut sorted: Vec<u8> = (0..n).map(|_| draw()).collect();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let mut out = vec![lo; n];
    for (rank, level) in sorted.into_iter().enumerate() {
        // ranks 0,3,4,7,.. go to even slots, 1,2,5,6,.. to odd slots: the snake
        let pair = rank / 2;
        let slot = if (rank % 4 == 0) || (rank % 4 == 3) { pair * 2 } else { pair * 2 + 1 };
        if slot < n {
            out[slot] = level;
        }
    }
    out
}

/// `--skills`: `n` levels in join order from `list`, repeated (`85,30` over 8 bots: 85, 30, 85, 30, ...).
pub fn listed_levels(list: &[u8], n: usize) -> Vec<u8> {
    list.iter().copied().cycle().take(if list.is_empty() { 0 } else { n }).collect()
}

/// `--skills <a,b,...>`: levels 0..=100.
pub fn parse_skills(text: &str) -> Result<Vec<u8>, String> {
    text.split(',')
        .map(|s| match s.trim().parse::<u8>() {
            Ok(v) if v <= 100 => Ok(v),
            _ => Err(format!("--skills: {s:?} is not a level 0..100")),
        })
        .collect()
}

fn nick(sc: usize, k: usize) -> String {
    format!("n{sc:02}b{k:02}")
}

fn base_args(ctx: &Ctx<'_>, sc: &Scenario, secs: u64) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--shooter".into(),
        "--duration".into(),
        secs.to_string(),
        "--join-timeout".into(),
        "30".into(),
        "--matches".into(),
        "1".into(),
    ];
    match (&ctx.cfg.rom, ctx.lobby) {
        (Some(rom), Some(_)) => {
            a.extend(
                [
                    "--rom",
                    &rom.to_string_lossy(),
                    "--hunt",
                    "--skill",
                    "30-65",
                    "--match-wait",
                    "240",
                ]
                .iter()
                .map(|s| s.to_string()),
            );
        }
        _ => {
            a.extend(
                ["--no-rom", "--walk", "--skill", "off"]
                    .iter()
                    .map(|s| s.to_string()),
            );
        }
    }
    let _ = sc;
    a
}

/// Runs one scenario: starts its bots (and, in a direct run, its game server), waits for them, judges them.
fn run_scenario(ctx: &Ctx<'_>, idx: usize, sc: &Scenario) -> Value {
    let dir = ctx.cfg.out.join(format!("{idx:02}-{}", sc.name));
    let _ = std::fs::create_dir_all(&dir);
    let secs = (sc.minutes * 60.0).round().max(20.0) as u64;
    // the roster: (nick, net, join_after_s)
    let mut roster: Vec<(String, String, u32)> = Vec::new();
    for g in &sc.groups {
        for _ in 0..g.count {
            let k = roster.len();
            roster.push((nick(idx, k), g.net.clone(), g.join_after_s));
        }
    }
    let start_after = roster.iter().filter(|r| r.2 == 0).count().max(1);
    let title = format!("netlab{idx:02}");
    let mut direct: Option<Direct> = None;
    let mut common = base_args(ctx, sc, secs);
    let mut host_extra: Vec<String> = Vec::new();
    let mut member_extra: Vec<String> = Vec::new();
    match ctx.lobby {
        Some(l) => {
            common.extend(["--lobby".to_string(), format!("127.0.0.1:{}", l.auth_port)]);
            let tl = dir.join("host.timeline");
            let map_line = sc
                .map
                .map(|m| format!("setting 3 {m}\n"))
                .unwrap_or_default();
            let _ = std::fs::write(&tl, map_line);
            host_extra.extend(
                [
                    "--room",
                    "new",
                    "--mode",
                    &sc.mode.unwrap_or(0).to_string(),
                    "--max-players",
                    "16",
                    "--start-after",
                    &start_after.to_string(),
                    "--title",
                    &title,
                    "--timeline",
                    &tl.to_string_lossy(),
                ]
                .iter()
                .map(|s| s.to_string()),
            );
            member_extra.extend(
                ["--room", "any", "--room-title", &title]
                    .iter()
                    .map(|s| s.to_string()),
            );
        }
        None => {
            let users: Vec<(String, u8)> = roster
                .iter()
                .enumerate()
                .map(|(k, r)| (r.0.clone(), (k % 2) as u8))
                .collect();
            match Direct::start(&ctx.cfg.bins, &dir, &users, &[]) {
                Ok(d) => {
                    common.extend(["--udp".to_string(), format!("127.0.0.1:{}", d.port)]);
                    direct = Some(d);
                }
                Err(e) => {
                    return json!({"name": sc.name, "mode": sc.mode_key, "map": sc.map_name, "minutes": sc.minutes,
                                  "verdict": "FAIL", "error": e, "bots": [], "kills": 0});
                }
            }
        }
    }
    let t0 = Instant::now();
    let mut procs: Vec<BotProc> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut pending: Vec<(usize, Instant)> = Vec::new();
    for (k, r) in roster.iter().enumerate() {
        // the host first, the others a little later; late groups at their time
        let at = if k == 0 {
            t0
        } else {
            t0 + Duration::from_millis(1500 + 150 * k as u64) + Duration::from_secs(r.2 as u64)
        };
        pending.push((k, at));
    }
    let max_join = roster.iter().map(|r| r.2).max().unwrap_or(0) as u64;
    let deadline = t0 + Duration::from_secs(secs + max_join + 330);
    let mut slots: Vec<Option<usize>> = vec![None; roster.len()];
    // ROM rooms: one skill level per bot, drawn from the range once per room and dealt so that the two lobby
    // teams (the bots join in order and alternate) get the same strength; before, a level came from the nick alone,
    // so team 1 was always the stronger side (45/37/51/55 against 48/42/31/46) and won 13 of 14 round matches.
    let levels = (ctx.cfg.rom.is_some() && ctx.lobby.is_some()).then(|| {
        if ctx.cfg.skills.is_empty() {
            balanced_levels(ctx.cfg.seed ^ (idx as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15), roster.len(), 30, 65)
        } else {
            listed_levels(&ctx.cfg.skills, roster.len())
        }
    });
    loop {
        let now = Instant::now();
        pending.retain(|&(k, at)| {
            if at > now {
                return true;
            }
            let r = &roster[k];
            let mut args = common.clone();
            args.extend(if k == 0 && ctx.lobby.is_some() {
                host_extra.clone()
            } else {
                member_extra.clone()
            });
            if let Some(l) = levels.as_ref().and_then(|l| l.get(k)) {
                args.push("--skill".into());
                args.push(l.to_string());
            }
            let seed = ctx
                .cfg
                .seed
                .wrapping_mul(1000)
                .wrapping_add((idx * 100 + k) as u64);
            match BotProc::spawn(&ctx.cfg.bins, &dir, &r.0, &r.1, seed, &args) {
                Ok(p) => {
                    slots[k] = Some(procs.len());
                    procs.push(p);
                }
                Err(e) => errors.push(format!("{}: {e}", r.0)),
            }
            false
        });
        let mut running = 0;
        for p in procs.iter_mut() {
            if !p.done() {
                running += 1;
            }
        }
        if pending.is_empty() && running == 0 {
            break;
        }
        if now > deadline {
            for p in procs.iter_mut() {
                if !p.done() {
                    errors.push(format!("{}: still running at the deadline, killed", p.nick));
                    p.kill();
                }
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // the game server writes its links' last lines when they close
    match direct.take() {
        Some(d) => drop(d),
        None => std::thread::sleep(Duration::from_secs(3)),
    }
    let server_dir: PathBuf = match ctx.lobby {
        Some(l) => l.dserver_dir.clone(),
        None => dir.clone(),
    };
    let bots: Vec<BotRef> = roster
        .iter()
        .enumerate()
        .map(|(k, r)| BotRef {
            nick: r.0.clone(),
            net: r.1.clone(),
            exit: slots[k].and_then(|i| procs[i].exit),
            started: slots[k].is_some(),
        })
        .collect();
    let meta = ScenarioMeta {
        name: sc.name.clone(),
        mode: sc.mode_key.clone(),
        map: sc.map_name.clone(),
        minutes: sc.minutes,
        xfail: sc.xfail.clone(),
    };
    let mut v = judge_scenario(
        &meta,
        &dir,
        &server_dir,
        &bots,
        &ctx.cfg.xfail,
        ctx.lobby.is_some(),
        &errors,
    );
    v["wall_s"] = json!(t0.elapsed().as_secs());
    v
}

/// One bot of a scenario as the judge needs it.
#[derive(Debug, Clone)]
pub struct BotRef {
    pub nick: String,
    pub net: String,
    pub exit: Option<i32>,
    /// False when its process never started (then it has no files).
    pub started: bool,
}

/// What the judge needs of a scenario besides its bots.
#[derive(Debug, Clone)]
pub struct ScenarioMeta {
    pub name: String,
    pub mode: String,
    pub map: String,
    pub minutes: f64,
    pub xfail: Vec<String>,
}

/// Judges a scenario from its files: the bots' files in `dir`, the game server logs in `server_dir`.
pub fn judge_scenario(
    sc: &ScenarioMeta,
    dir: &Path,
    server_dir: &Path,
    bots: &[BotRef],
    xfail_all: &[String],
    rom_run: bool,
    errors: &[String],
) -> Value {
    let servers = collect::read_server_logs(server_dir);
    let views_of: Vec<Option<&collect::ServerView>> =
        bots.iter().map(|r| servers.get(&r.nick)).collect();
    let baseline = report::baseline_rate(&views_of);
    let mut bots_json = Vec::new();
    let mut views = Vec::new();
    let mut any_fail = false;
    for r in bots {
        let mut view = if r.started {
            collect::read_bot(
                &dir.join(format!("{}.report.json", r.nick)),
                &dir.join(format!("{}.out", r.nick)),
                &dir.join(format!("{}.events.jsonl", r.nick)),
            )
        } else {
            collect::BotView::default()
        };
        view.exit_code = r.exit;
        let spec_name = dc_net_engine::impair::PathSpec::parse(&r.net)
            .map(|s| s.name.trim_end_matches('+').to_string())
            .unwrap_or_default();
        let xfail = xfail_all.iter().any(|x| *x == spec_name || *x == r.net)
            || sc.xfail.iter().any(|x| *x == spec_name || *x == r.net);
        let res = report::judge(
            &r.nick,
            &r.net,
            &view,
            servers.get(&r.nick),
            baseline,
            xfail,
        );
        any_fail |= res.verdict == "FAIL";
        bots_json.push(res.json);
        views.push((view, xfail));
    }
    let refs: Vec<(&collect::BotView, bool)> = views.iter().map(|(v, x)| (v, *x)).collect();
    // without the ROM the bots cannot read the stream positions of the synthetic world, so they hold fire there
    let hit = if rom_run {
        report::hit_sanity(&refs, sc.minutes)
    } else {
        None
    };
    let kills: u64 = views.iter().map(|(v, _)| v.kills_by_me).sum();
    let shots: u64 = views.iter().map(|(v, _)| v.shots).sum();
    let claims: u64 = views.iter().map(|(v, _)| v.hit_claims).sum();
    let verdict = if any_fail || hit.is_some() || !errors.is_empty() {
        "FAIL"
    } else {
        "pass"
    };
    json!({
        "name": sc.name, "mode": sc.mode, "map": sc.map, "minutes": sc.minutes,
        "players": bots.len(), "dir": dir.to_string_lossy(), "verdict": verdict,
        "error": (!errors.is_empty()).then(|| errors.join("; ")), "hit_sanity": hit, "kills": kills,
        "shots": shots, "hit_claims": claims, "input_baseline": baseline, "bots": bots_json,
    })
}

/// Judges an earlier run again from its files (after a threshold change): reads `out/report.json`, rewrites
/// `report.md` / `report.json`.
pub fn rejudge(out: &Path, xfail_all: &[String]) -> Result<Value, String> {
    let text = std::fs::read_to_string(out.join("report.json"))
        .map_err(|e| format!("{}: {e}", out.display()))?;
    let mut run: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let rom_run = run["kind"].as_str().is_some_and(|k| k.starts_with("ROM"));
    let mut scen = Vec::new();
    for sc in run["scenarios"].as_array().cloned().unwrap_or_default() {
        let dir = PathBuf::from(sc["dir"].as_str().unwrap_or(""));
        let server_dir = if rom_run {
            out.join("dserver")
        } else {
            dir.clone()
        };
        let bots: Vec<BotRef> = sc["bots"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| BotRef {
                nick: b["nick"].as_str().unwrap_or("").to_string(),
                net: b["net"].as_str().unwrap_or("").to_string(),
                exit: b["exit"].as_i64().map(|x| x as i32),
                started: true,
            })
            .collect();
        let meta = ScenarioMeta {
            name: sc["name"].as_str().unwrap_or("").to_string(),
            mode: sc["mode"].as_str().unwrap_or("").to_string(),
            map: sc["map"].as_str().unwrap_or("").to_string(),
            minutes: sc["minutes"].as_f64().unwrap_or(0.0),
            xfail: Vec::new(),
        };
        let errors: Vec<String> = sc["error"]
            .as_str()
            .map(|e| vec![e.to_string()])
            .unwrap_or_default();
        let mut v = judge_scenario(&meta, &dir, &server_dir, &bots, xfail_all, rom_run, &errors);
        v["wall_s"] = sc["wall_s"].clone();
        scen.push(v);
    }
    run["scenarios"] = Value::Array(scen);
    finish(out, &mut run)?;
    Ok(run)
}

/// Totals, verdict, and the two report files.
fn finish(out: &Path, run: &mut Value) -> Result<(), String> {
    let mut totals = json!({"pass": 0, "FAIL": 0, "xfail": 0, "xpass": 0});
    let scen = run["scenarios"].as_array().cloned().unwrap_or_default();
    for sc in &scen {
        for b in sc["bots"].as_array().into_iter().flatten() {
            if let Some(v) = b["verdict"].as_str() {
                totals[v] = json!(totals[v].as_u64().unwrap_or(0) + 1);
            }
        }
    }
    run["totals"] = totals;
    run["verdict"] = json!(if scen.iter().all(|s| s["verdict"] == "pass") {
        "pass"
    } else {
        "FAIL"
    });
    let md = report::markdown(run);
    std::fs::write(out.join("report.md"), &md).map_err(|e| e.to_string())?;
    std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(run).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
}

/// Runs `scenarios` and writes the report; returns the run's JSON.
pub fn run(cfg: &RunConfig, scenarios: &[Scenario]) -> Result<Value, String> {
    std::fs::create_dir_all(&cfg.out).map_err(|e| format!("{}: {e}", cfg.out.display()))?;
    let started = now_text();
    let t0 = Instant::now();
    let lobby = match &cfg.rom {
        Some(rom) => {
            let base = stack::free_ports(24_000 + (std::process::id() % 500) as u16 * 20, 12)
                .ok_or("no free loopback ports")?;
            Some(Lobby::start(
                &cfg.bins,
                rom,
                &cfg.out,
                base,
                &cfg.dserver_args,
            )?)
        }
        None => None,
    };
    let ctx = Ctx {
        cfg,
        lobby: lobby.as_ref(),
    };
    let results: Arc<Mutex<Vec<Option<Value>>>> = Arc::new(Mutex::new(vec![None; scenarios.len()]));
    let next = Arc::new(Mutex::new(0usize));
    std::thread::scope(|s| {
        for _ in 0..cfg.parallel.max(1) {
            let (results, next, ctx) = (results.clone(), next.clone(), &ctx);
            s.spawn(move || loop {
                let i = {
                    let mut n = next.lock().expect("lock");
                    let i = *n;
                    *n += 1;
                    i
                };
                let Some(sc) = scenarios.get(i) else {
                    return;
                };
                eprintln!(
                    "dc-netlab: [{}/{}] {} ({} on {}, {} bots, {} min)",
                    i + 1,
                    scenarios.len(),
                    sc.name,
                    sc.mode_key,
                    sc.map_name,
                    sc.players(),
                    sc.minutes
                );
                let v = run_scenario(ctx, i, sc);
                eprintln!(
                    "dc-netlab: [{}/{}] {}: {}",
                    i + 1,
                    scenarios.len(),
                    sc.name,
                    v["verdict"].as_str().unwrap_or("?")
                );
                results.lock().expect("lock")[i] = Some(v);
            });
        }
    });
    drop(lobby);
    let scen: Vec<Value> = results
        .lock()
        .expect("lock")
        .iter()
        .map(|v| v.clone().unwrap_or(Value::Null))
        .collect();
    let mut run = json!({
        "matrix": cfg.matrix, "kind": if cfg.rom.is_some() { "ROM (lobby + game server)" } else { "direct (synthetic world, no ROM)" },
        "started": started, "wall_s": t0.elapsed().as_secs(), "command": cfg.command,
        "scenarios": scen,
    });
    finish(&cfg.out, &mut run)?;
    Ok(run)
}

/// `YYYY-MM-DD hh:mm:ss` UTC (days from the civil-from-days algorithm, public domain, H. Hinnant).
pub fn now_text() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The default output folder: `netlab-out/<stamp>` under the current folder.
pub fn default_out() -> PathBuf {
    let stamp: String = now_text()
        .chars()
        .map(|c| if c.is_ascii_digit() { c } else { '-' })
        .collect();
    Path::new("netlab-out").join(stamp)
}

#[cfg(test)]
mod level_tests {
    #[test]
    fn balanced_levels_give_both_teams_the_same_strength() {
        for seed in 0..200u64 {
            let l = super::balanced_levels(seed, 8, 30, 65);
            assert!(l.iter().all(|&x| (30..=65).contains(&x)));
            let even: u32 = l.iter().step_by(2).map(|&x| x as u32).sum();
            let odd: u32 = l.iter().skip(1).step_by(2).map(|&x| x as u32).sum();
            assert!(even.abs_diff(odd) <= 35, "seed {seed}: {l:?}");
        }
        assert_ne!(super::balanced_levels(1, 8, 30, 65), super::balanced_levels(2, 8, 30, 65));
    }

    // --skills: the list in join order, repeated; the alternate deal puts one level on each team
    #[test]
    fn listed_levels_repeat_in_join_order() {
        let l = super::parse_skills("85, 30").unwrap();
        assert_eq!(super::listed_levels(&l, 5), vec![85, 30, 85, 30, 85]);
        assert!(super::parse_skills("85,101").is_err());
        assert!(super::parse_skills("x").is_err());
        assert!(super::listed_levels(&[], 4).is_empty());
    }
}

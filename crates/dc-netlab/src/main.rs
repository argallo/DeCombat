//! `dc-netlab`: the network lab's command (docs/NETLAB.md).
//!
//! ```text
//! dc-netlab quick|soak|ci|campus [--rom <dir> | --direct] [--minutes <m>] [--players <n>] [--parallel <n>]
//!           [--only <name,...>] [--out <dir>] [--bin-dir <dir>] [--seed <n>] [--dserver-args <text>]
//!           [--xfail <profile,...>] [--strict]
//! dc-netlab run --mix good:6,wifi:2,... [--mode <key>] [--map <n>] [same options]
//! dc-netlab list [--rom <dir>]
//! dc-netlab judge --out <run folder>       (judges an earlier run again from its files)
//! ```
//! Exit status: 0 when every scenario passed (known open issues, `xfail`, allowed unless `--strict`), 1 when one
//! failed, 2 on a usage error.

use std::path::PathBuf;

use dc_netlab::matrix::{self, Scenario};
use dc_netlab::stack::Bins;
use dc_netlab::RunConfig;

const USAGE: &str = "usage: dc-netlab quick|soak|ci|campus [--rom <dir> | --direct] [--minutes <m>] [--players <n>]
                 [--parallel <n>] [--only <name,...>] [--out <dir>] [--bin-dir <dir>] [--seed <n>]
                 [--dserver-args <text>] [--xfail <profile,...>] [--strict]
       dc-netlab run --mix good:6,wifi:2,... [--mode <key>] [--map <n>] [same options]
       dc-netlab list [--rom <dir>]
       dc-netlab judge --out <run folder> [--xfail <profile,...>]   (judge an earlier run again from its files)
The ROM comes from --rom or CA_ROM; without one (or with --direct) the bots join a game server on a synthetic world.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(match real_main(&args) {
        Ok(code) => code,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("dc-netlab: {e}");
            }
            eprintln!("{USAGE}");
            2
        }
    });
}

fn real_main(args: &[String]) -> Result<i32, String> {
    let Some(cmd) = args.first().cloned() else {
        return Err(String::new());
    };
    let mut rom: Option<PathBuf> = std::env::var_os("CA_ROM")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let mut direct = false;
    let mut minutes: Option<f64> = None;
    let mut players = 16usize;
    let mut parallel = 1usize;
    let mut only: Vec<String> = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut bin_dir: Option<PathBuf> = None;
    let mut seed = 1u64;
    let mut dserver_args = String::new();
    let mut xfail: Vec<String> = dc_netlab::DEFAULT_XFAIL
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut strict = false;
    let mut mix: Option<String> = None;
    let mut mode_key: Option<String> = None;
    let mut map: Option<usize> = None;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut val = |f: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{f} needs a value"))
        };
        match a.as_str() {
            "--rom" => rom = Some(PathBuf::from(val("--rom")?)),
            "--direct" => direct = true,
            "--minutes" => {
                minutes = Some(
                    val("--minutes")?
                        .parse()
                        .map_err(|_| "--minutes: bad number")?,
                )
            }
            "--players" => {
                players = val("--players")?
                    .parse::<usize>()
                    .map_err(|_| "--players: bad number")?
                    .clamp(1, 16)
            }
            "--parallel" => {
                parallel = val("--parallel")?
                    .parse::<usize>()
                    .map_err(|_| "--parallel: bad number")?
                    .clamp(1, 4)
            }
            "--only" => only.extend(val("--only")?.split(',').map(str::to_string)),
            "--out" => out = Some(PathBuf::from(val("--out")?)),
            "--bin-dir" => bin_dir = Some(PathBuf::from(val("--bin-dir")?)),
            "--seed" => seed = val("--seed")?.parse().map_err(|_| "--seed: bad number")?,
            "--dserver-args" => dserver_args = val("--dserver-args")?,
            "--xfail" => {
                let v = val("--xfail")?;
                xfail = if v == "none" {
                    Vec::new()
                } else {
                    v.split(',').map(str::to_string).collect()
                }
            }
            "--strict" => strict = true,
            "--mix" => mix = Some(val("--mix")?),
            "--mode" => mode_key = Some(val("--mode")?),
            "--map" => map = Some(val("--map")?.parse().map_err(|_| "--map: bad number")?),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    if direct {
        rom = None;
    }
    let modes = match &rom {
        Some(r) => matrix::rom_modes(r)?,
        None => Vec::new(),
    };
    if cmd == "judge" {
        let dir = out.ok_or("judge needs --out <the run's folder>")?;
        let run = dc_netlab::rejudge(&dir, &xfail)?;
        eprintln!(
            "dc-netlab: {} ({} pass, {} FAIL, {} xfail); report {}",
            run["verdict"].as_str().unwrap_or("?"),
            run["totals"]["pass"],
            run["totals"]["FAIL"],
            run["totals"]["xfail"],
            dir.join("report.md").display()
        );
        return Ok(if run["verdict"] == "pass" { 0 } else { 1 });
    }
    if cmd == "list" {
        println!(
            "network profiles: {}",
            dc_net_engine::impair::PROFILES.join(", ")
        );
        for p in dc_net_engine::impair::PROFILES {
            println!(
                "  {p}: {:?}",
                dc_net_engine::impair::PathSpec::profile(p).expect("profile")
            );
        }
        println!("matrices: {}", matrix::MATRICES.join(", "));
        for m in &modes {
            let maps: Vec<String> = m.maps.iter().map(|(i, n)| format!("{i}:{n}")).collect();
            println!(
                "mode {} {} ({}): maps {}",
                m.index,
                m.key,
                m.name,
                maps.join(", ")
            );
        }
        if rom.is_none() {
            println!("(no ROM: --rom <dir> or CA_ROM lists the modes and maps)");
        }
        return Ok(0);
    }
    let mut scenarios: Vec<Scenario> = if cmd == "run" {
        let groups = matrix::parse_mix(mix.as_deref().ok_or("run needs --mix")?)?;
        let m = match &mode_key {
            Some(k) => Some(
                modes
                    .iter()
                    .find(|m| m.key.eq_ignore_ascii_case(k) || m.index.to_string() == *k)
                    .ok_or_else(|| format!("--mode {k}: not a playable mode of the ROM"))?,
            ),
            None => modes.first(),
        };
        let (map_i, map_name) = match (m, map) {
            (Some(m), Some(i)) => (
                Some(i),
                m.maps
                    .iter()
                    .find(|x| x.0 == i)
                    .map(|x| x.1.clone())
                    .unwrap_or(format!("map{i}")),
            ),
            (Some(m), None) => (Some(m.maps[0].0), m.maps[0].1.clone()),
            (None, _) => (None, "synthetic".into()),
        };
        vec![Scenario {
            name: "custom".into(),
            mode: m.map(|m| m.index),
            mode_key: m.map(|m| m.key.clone()).unwrap_or_else(|| "TDM".into()),
            map: map_i,
            map_name,
            groups,
            minutes: minutes.unwrap_or(3.0),
            xfail: Vec::new(),
        }]
    } else {
        matrix::build(&cmd, &modes, players, minutes)?
    };
    if !only.is_empty() {
        scenarios.retain(|s| {
            only.iter()
                .any(|o| s.name == *o || s.name.starts_with(o.as_str()))
        });
    }
    if scenarios.is_empty() {
        return Err("no scenario to run".into());
    }
    let bins = Bins::find(bin_dir.as_deref())?;
    let cfg = RunConfig {
        matrix: cmd.clone(),
        bins,
        rom,
        // The lobby starts each game server in the per-room `dserver` folder and hands it this path for its log,
        // so a relative --out would be resolved against that folder and the log would go missing: make it absolute.
        out: {
            let o = out.unwrap_or_else(dc_netlab::default_out);
            std::path::absolute(&o).unwrap_or(o)
        },
        parallel,
        seed,
        dserver_args,
        xfail,
        command: std::iter::once("dc-netlab".to_string())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
    };
    let total_min: f64 = scenarios.iter().map(|s| s.minutes + 0.8).sum::<f64>() / parallel as f64;
    eprintln!(
        "dc-netlab: {} scenarios, about {:.0} min, output {}",
        scenarios.len(),
        total_min,
        cfg.out.display()
    );
    let run = dc_netlab::run(&cfg, &scenarios)?;
    eprintln!(
        "dc-netlab: {} ({} pass, {} FAIL, {} xfail); report {}",
        run["verdict"].as_str().unwrap_or("?"),
        run["totals"]["pass"],
        run["totals"]["FAIL"],
        run["totals"]["xfail"],
        cfg.out.join("report.md").display()
    );
    let xfails = run["totals"]["xfail"].as_u64().unwrap_or(0);
    Ok(if run["verdict"] == "pass" && !(strict && xfails > 0) {
        0
    } else {
        1
    })
}

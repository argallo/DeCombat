//! `dc-host [--config FILE]`: the remote game host agent (specs/HOST_AGENT.md). `--version` prints the git short sha.
// Provenance: ORIGINAL

use dc_host::config::HostConfig;
use dc_host::games::{Games, Hub};
use dc_host::{link, log};
use std::path::PathBuf;
use std::sync::Arc;

const USAGE: &str = "usage: dc-host [--config FILE] [--version]
  --config FILE   host.toml (default: host.toml next to the dc-host executable); specs/HOST_AGENT.md section 2
  --version       print the git commit this agent was built from";

fn default_config() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("host.toml")))
        .unwrap_or_else(|| PathBuf::from("host.toml"))
}

fn main() {
    let mut config = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--version" | "-V" => {
                println!("dc-host {}", dc_host::version());
                return;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            "--config" => match args.next() {
                Some(v) => config = Some(PathBuf::from(v)),
                None => usage_error("--config needs a file"),
            },
            o => match o.strip_prefix("--config=") {
                Some(v) => config = Some(PathBuf::from(v)),
                None => usage_error(&format!("unknown argument {o:?}")),
            },
        }
    }
    let path = config.unwrap_or_else(default_config);
    let cfg = match HostConfig::load(&path) {
        Ok(c) => c,
        Err(e) => {
            log::line(&format!("config refused: {e}"));
            std::process::exit(2);
        }
    };
    log::line(&format!(
        "dc-host {} as {:?}: lobby {} (pinned {}), ports {}-{} on {}, game server {}, logs {}",
        dc_host::version(),
        cfg.name,
        cfg.lobby,
        cfg.fingerprint,
        cfg.port_lo,
        cfg.port_hi,
        cfg.bind_ip,
        cfg.dserver_exe.display(),
        cfg.log_dir.display()
    ));
    if !cfg.dserver_exe.is_file() {
        log::line(&format!(
            "warning: {} does not exist; every start will fail until it does",
            cfg.dserver_exe.display()
        ));
    }
    match &cfg.rom {
        Some(r) if !r.is_dir() => log::line(&format!("warning: rom {} is not a folder", r.display())),
        Some(_) => {}
        None if std::env::var_os("CA_ROM").is_none() => {
            log::line("warning: no rom in the config and no CA_ROM: the game servers will not find their worlds")
        }
        None => {}
    }
    let hub = Arc::new(Hub::default());
    let games = Games::new(cfg.clone(), Arc::clone(&hub));
    link::run(&cfg, &games, &hub)
}

fn usage_error(msg: &str) -> ! {
    eprintln!("dc-host: {msg}\n{USAGE}");
    std::process::exit(2)
}

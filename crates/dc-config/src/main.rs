//! `dc-config`: validate `server.toml` (with DC_* env and CLI overrides applied) and community directory files.
//!
//! ```text
//! dc-config check [server.toml] [--fs] [--rom DIR] [--public] [--bind IP] [--metrics]
//! dc-config dump  [server.toml] [same overrides]       print the effective config as TOML
//! dc-config example                                     print a commented example server.toml
//! dc-config check-directory community.json             validate a community directory file
//! dc-config check-vanilla-plus vanilla_plus.json        validate the review list of curated packs (LG-315)
//! dc-config vanilla-plus-lines vanilla_plus.json        print the `mods = [...]` lines of that list for an entry
//! ```
//! Exit status: 0 = no errors (warnings allowed), 1 = errors, 2 = usage or unreadable file.

use dc_config::{
    apply_cli, apply_env, check, directory, has_errors, parse_toml, CheckOptions, CliOverrides,
    Config, Issue,
};
use std::process::ExitCode;

fn print_issues(issues: &[Issue]) {
    for i in issues {
        eprintln!("{i}");
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  dc-config check [server.toml] [--fs] [--rom DIR] [--public] [--bind IP] [--metrics]\n  \
         dc-config dump [server.toml] [overrides]\n  dc-config example\n  dc-config check-directory community.json\n  dc-config check-vanilla-plus vanilla_plus.json\n  dc-config vanilla-plus-lines vanilla_plus.json"
    );
    ExitCode::from(2)
}

fn build(args: &[String]) -> Result<(Config, bool), ExitCode> {
    let mut cli = CliOverrides::default();
    let mut file: Option<&str> = None;
    let mut fs = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--fs" => fs = true,
            "--public" => cli.public = true,
            "--metrics" => cli.metrics = true,
            "--rom" => cli.rom = it.next().cloned(),
            "--bind" => cli.bind = it.next().cloned(),
            s if s.starts_with("--") => {
                eprintln!("unknown option {s}");
                return Err(usage());
            }
            s => file = Some(s),
        }
    }
    let base = match file {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(t) => match parse_toml(&t) {
                Ok(c) => c,
                Err(e) => {
                    print_issues(&e);
                    return Err(ExitCode::from(1));
                }
            },
            Err(e) => {
                eprintln!("cannot read {p}: {e}");
                return Err(ExitCode::from(2));
            }
        },
        None => Config::default(),
    };
    let mut cfg = match apply_env(&base, std::env::vars()) {
        Ok(c) => c,
        Err(e) => {
            print_issues(&e);
            return Err(ExitCode::from(1));
        }
    };
    apply_cli(&mut cfg, &cli);
    Ok((cfg, fs))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        return usage();
    };
    match cmd.as_str() {
        "check" => {
            let (cfg, fs) = match build(rest) {
                Ok(v) => v,
                Err(c) => return c,
            };
            let issues = check(&cfg, CheckOptions { fs });
            print_issues(&issues);
            if has_errors(&issues) {
                eprintln!("config has errors");
                ExitCode::from(1)
            } else {
                let p = cfg.resolved_ports();
                println!(
                    "config ok: bind {} auth {} lobby {} clan {} http {} policy {} ({} mods)",
                    cfg.effective_bind(),
                    p.auth,
                    p.lobby,
                    p.clan,
                    p.http,
                    cfg.mods.policy,
                    cfg.mods.set.len()
                );
                ExitCode::SUCCESS
            }
        }
        "dump" => match build(rest) {
            Ok((mut cfg, _)) => {
                // the SMTP password is never printed (LG-245)
                if !cfg.mail.password.is_empty() {
                    cfg.mail.password = "***".into();
                }
                print!("{}", cfg.to_toml());
                ExitCode::SUCCESS
            }
            Err(c) => c,
        },
        "example" => {
            print!("{}", dc_config::EXAMPLE_SERVER_TOML);
            ExitCode::SUCCESS
        }
        // DC-DEFAULT: LG-315 (proposed) the review list of curated packs and its config lines
        "check-vanilla-plus" | "vanilla-plus-lines" => {
            let Some(path) = rest.first() else {
                return usage();
            };
            let text = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("cannot read {path}: {e}");
                    return ExitCode::from(2);
                }
            };
            match dc_config::vanilla_plus::load(&text) {
                Ok(list) => {
                    if cmd_lines(cmd) {
                        print!("{}", dc_config::vanilla_plus::config_lines(&list));
                    } else {
                        println!("review list ok: {} packs", list.packs.len());
                    }
                    ExitCode::SUCCESS
                }
                Err(issues) => {
                    print_issues(&issues);
                    ExitCode::from(1)
                }
            }
        }
        "check-directory" => {
            let Some(path) = rest.first() else {
                return usage();
            };
            let text = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("cannot read {path}: {e}");
                    return ExitCode::from(2);
                }
            };
            let issues = match directory::load_community(&text) {
                Ok(doc) => {
                    let issues = directory::validate_community(&doc);
                    if !has_errors(&issues) {
                        println!(
                            "directory ok: {} servers, {} known mods",
                            doc.servers.len(),
                            doc.known_mods.len()
                        );
                    }
                    issues
                }
                Err(e) => e,
            };
            print_issues(&issues);
            if has_errors(&issues) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        _ => usage(),
    }
}

fn cmd_lines(cmd: &str) -> bool {
    cmd == "vanilla-plus-lines"
}

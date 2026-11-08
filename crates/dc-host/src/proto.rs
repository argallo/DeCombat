//! The control lines of HOST_AGENT.md §3: UTF-8, one per `\n` (a `\r` before it is dropped), at most
//! [`MAX_LINE`] bytes; a longer line is dropped whole with a log line, in both directions.

use serde::Deserialize;
use std::ffi::OsString;
use std::path::Path;

/// The longest line, without its `\n`.
// DC-DEFAULT: HA-1 16 KiB
pub const MAX_LINE: usize = 16 * 1024;

/// One line from the lobby.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromLobby {
    /// `ok name=<name> lobby=<version>`: the rest of the line, verbatim.
    Ok(String),
    /// `refuse <reason>`
    Refuse(String),
    /// `start <gid> <json>`
    Start { gid: u64, json: String },
    /// `pack <sha> <base64>`: one piece of a map pack (MAP-84).
    Pack { sha: String, data: String },
    /// `content <hash> <base64>`: one piece of a featured entry's weapon content document (LG-431).
    Content { hash: String, data: String },
    /// `line <gid> <text>`: the text after the one space following the gid, verbatim (may be empty).
    Line { gid: u64, text: String },
    /// `quit <gid>`
    Quit { gid: u64 },
    /// `ping <n>`
    Ping(String),
    /// `pong <n>`
    Pong(String),
    /// HA-2 `update <commit> <sha256> <size> <tarball>`: the words after `update`, checked by `update::Offer`.
    Update(String),
    /// HA-2 `updpart <commit> <base64>`: one piece of the offered tarball.
    UpdPart { commit: String, data: String },
    /// HA-2 `updend <commit>`: the last piece went.
    UpdEnd(String),
    /// HA-2 `updateno <commit> <reason>`: the lobby will not send it now.
    UpdNo { commit: String, reason: String },
    /// anything else (logged, otherwise ignored; a newer lobby may send more)
    Unknown(String),
    /// a known verb with a bad gid / missing argument
    Bad(String),
}

/// Splits `s` at its first space: (`word`, rest without that space).
fn word(s: &str) -> (&str, &str) {
    match s.split_once(' ') {
        Some((a, b)) => (a, b),
        None => (s, ""),
    }
}

fn gid(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Parses one line of the lobby (without its newline).
pub fn parse(line: &str) -> FromLobby {
    let (verb, rest) = word(line);
    let bad = || FromLobby::Bad(line.to_string());
    match verb {
        "ok" => FromLobby::Ok(rest.to_string()),
        "refuse" => FromLobby::Refuse(rest.to_string()),
        "start" => {
            let (g, json) = word(rest);
            match gid(g) {
                Some(gid) if !json.trim().is_empty() => FromLobby::Start {
                    gid,
                    json: json.to_string(),
                },
                _ => bad(),
            }
        }
        "pack" => {
            let (sha, data) = word(rest);
            if crate::packs::sha_ok(sha) && !data.is_empty() {
                FromLobby::Pack {
                    sha: sha.to_string(),
                    data: data.to_string(),
                }
            } else {
                bad()
            }
        }
        "content" => {
            let (hash, data) = word(rest);
            if crate::packs::sha_ok(hash) && !data.is_empty() {
                FromLobby::Content {
                    hash: hash.to_string(),
                    data: data.to_string(),
                }
            } else {
                bad()
            }
        }
        "line" => {
            let (g, text) = word(rest);
            match gid(g) {
                Some(gid) => FromLobby::Line {
                    gid,
                    text: text.to_string(),
                },
                None => bad(),
            }
        }
        "quit" => match gid(rest.trim()) {
            Some(gid) => FromLobby::Quit { gid },
            None => bad(),
        },
        "ping" => FromLobby::Ping(rest.trim().to_string()),
        "pong" => FromLobby::Pong(rest.trim().to_string()),
        "update" if !rest.trim().is_empty() => FromLobby::Update(rest.trim().to_string()),
        "updpart" => {
            let (c, data) = word(rest);
            if !c.is_empty() && !data.is_empty() {
                FromLobby::UpdPart {
                    commit: c.to_string(),
                    data: data.to_string(),
                }
            } else {
                bad()
            }
        }
        "updend" if !rest.trim().is_empty() => FromLobby::UpdEnd(rest.trim().to_string()),
        "updateno" => {
            let (c, reason) = word(rest);
            FromLobby::UpdNo {
                commit: c.to_string(),
                reason: one_line(reason),
            }
        }
        "update" | "updend" => bad(),
        _ => FromLobby::Unknown(line.to_string()),
    }
}

/// One line of free text made safe for a field at the end of a line (no newline, no control characters).
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// The lines the agent sends (each without its newline).
pub mod out {
    use super::one_line;

    pub fn hello(
        name: &str,
        token: &str,
        version: &str,
        lo: u16,
        hi: u16,
        running: usize,
    ) -> String {
        // `content=1`: this agent takes the `start` request's weapon content (LG-313); `contentlines=1`: it takes
        // that content as `content` lines before the `start` (LG-431); `packs=1`: its map packs (MAP-83);
        // `update=1`: the lobby's server build offers (HA-2)
        format!(
            "hello name={name} token={token} version={version} ports={lo}-{hi} running={running} content=1 contentlines=1 packs=1 update=1"
        )
    }

    pub fn started(gid: u64, port: u16, pid: u32) -> String {
        format!("started {gid} port={port} pid={pid}")
    }

    pub fn failed(gid: u64, reason: &str) -> String {
        let r = one_line(reason);
        format!("failed {gid} {}", if r.is_empty() { "error" } else { &r })
    }

    /// One stdout line of a game server, verbatim (DSERVER_PIPE.md: the line itself has no newline).
    pub fn event(gid: u64, text: &str) -> String {
        format!("event {gid} {text}")
    }

    /// `code` None: killed by a signal / no code (-1).
    pub fn exited(gid: u64, code: Option<i32>) -> String {
        format!("exited {gid} code={}", code.unwrap_or(-1))
    }

    pub fn running(gid: u64, port: u16, pid: u32) -> String {
        format!("running {gid} port={port} pid={pid}")
    }

    /// A map pack (MAP-84) or a content document (LG-431) this agent keeps from before; the lobby does not send it again.
    pub fn have(sha: &str) -> String {
        format!("have {sha}")
    }

    pub fn ping(n: u64) -> String {
        format!("ping {n}")
    }

    pub fn pong(n: &str) -> String {
        format!("pong {}", one_line(n))
    }
}

/// What [`LineBuf::push`] yields.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    Line(String),
    /// A line over [`MAX_LINE`] was dropped; its length (at least `MAX_LINE + 1`).
    TooLong(usize),
}

/// Splits a byte stream into lines, dropping the ones over [`MAX_LINE`] without keeping more than that in memory.
#[derive(Default)]
pub struct LineBuf {
    buf: Vec<u8>,
    /// Some(n): inside an over-long line, n bytes of it seen so far.
    skipping: Option<usize>,
}

impl LineBuf {
    pub fn push(&mut self, data: &[u8]) -> Vec<Piece> {
        let mut out = Vec::new();
        for chunk in data.split_inclusive(|&b| b == b'\n') {
            let ends = chunk.last() == Some(&b'\n');
            let body = if ends {
                &chunk[..chunk.len() - 1]
            } else {
                chunk
            };
            if let Some(n) = self.skipping.as_mut() {
                *n += body.len();
                if ends {
                    out.push(Piece::TooLong(*n));
                    self.skipping = None;
                }
                continue;
            }
            self.buf.extend_from_slice(body);
            // a `\r` before the newline is not part of the line and does not count against the limit
            let len = self.buf.len() - usize::from(ends && self.buf.last() == Some(&b'\r'));
            if len > MAX_LINE + usize::from(!ends) {
                // without its newline yet, one byte over may still be the `\r` of a line of exactly MAX_LINE
                if ends {
                    out.push(Piece::TooLong(len));
                } else {
                    self.skipping = Some(self.buf.len());
                }
                self.buf.clear();
                continue;
            }
            if ends {
                self.buf.truncate(len);
                out.push(Piece::Line(String::from_utf8_lossy(&self.buf).into_owned()));
                self.buf.clear();
            }
        }
        out
    }
}

/// The `start` request's JSON (HOST_AGENT.md §3).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StartReq {
    #[serde(default)]
    pub port: u16,
    pub world: String,
    #[serde(default = "default_guid")]
    pub guid: String,
    #[serde(default = "default_idle")]
    pub idle: i64,
    #[serde(default)]
    pub log: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// LG-313: a featured entry's weapon content document (MOD-21) and its hash: written to `content-<hash>.json` in the
    /// log folder and passed as `-content`. Empty: none. LG-431: a hash without a document names one that came as
    /// `content` lines (or stays from before): checked and passed from the agent's `content/` folder.
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub content_hash: String,
    /// MAP-83: a featured entry's community map: the pack holding its world, the world's path inside the pack, and
    /// the record's `world` to announce. All three or none.
    #[serde(default)]
    pub world_pack: Option<WorldPack>,
    #[serde(default)]
    pub world_file: String,
    #[serde(default)]
    pub announce: String,
}

/// The `world_pack` of a `start` request. `url` is the pack's path on the lobby's web port (for an operator and for a
/// later agent with an HTTP client); this agent takes the bytes from the `pack` lines.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WorldPack {
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub url: String,
}

fn default_guid() -> String {
    "ca".into()
}

fn default_idle() -> i64 {
    60
}

/// The flags the agent itself sets; the lobby's `args` cannot repeat them (a second `-log` could name any file the
/// agent's user may write, a second `-bindip` / `-port` would leave the range the router forwards).
// DC-DEFAULT: HA-1
pub const OWN_FLAGS: [&str; 10] = [
    "-port",
    "-world",
    "-guid",
    "-idle",
    "-log",
    "-bindip",
    "-content",
    "-worldfile",
    "-announce",
    "-worldobjects",
];

impl StartReq {
    pub fn parse(json: &str) -> Result<StartReq, String> {
        let r: StartReq =
            serde_json::from_str(json).map_err(|e| format!("bad start request: {e}"))?;
        if r.world.trim().is_empty() {
            return Err("bad start request: no world".into());
        }
        if r.guid.is_empty() || !r.guid.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(format!("bad start request: guid {:?}", r.guid));
        }
        if !r.log.is_empty() && !log_name_ok(&r.log) {
            return Err(format!(
                "bad start request: log {:?} is not a plain file name",
                r.log
            ));
        }
        match &r.world_pack {
            Some(p) => {
                if !crate::packs::sha_ok(&p.sha256) {
                    return Err(format!("bad start request: pack {:?}", p.sha256));
                }
                if !world_file_ok(&r.world_file) {
                    return Err(format!("bad start request: world_file {:?}", r.world_file));
                }
                if !announce_ok(&r.announce) {
                    return Err(format!("bad start request: announce {:?}", r.announce));
                }
            }
            None if !r.world_file.is_empty() || !r.announce.is_empty() => {
                return Err("bad start request: world_file or announce without world_pack".into());
            }
            None => {}
        }
        Ok(r)
    }

    /// The log file name: the lobby's, else `dserver_g<gid>.log`.
    pub fn log_name(&self, gid: u64) -> String {
        if self.log.is_empty() {
            format!("dserver_g{gid}.log")
        } else {
            self.log.clone()
        }
    }
}

/// A plain file name inside `log_dir`: no separator, no `..`, no leading dot, visible ASCII.
pub fn log_name_ok(s: &str) -> bool {
    s.len() <= 128
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// A relative path inside a pack: `/`-separated plain names, nothing that could leave the pack's folder.
pub fn world_file_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 512
        && s.split('/').all(|c| {
            !c.is_empty()
                && c != "."
                && c != ".."
                && !c
                    .chars()
                    .any(|ch| ch.is_control() || ch == '\\' || ch == ':')
        })
}

/// The record's world as the game server announces it: an `@mod:` path, one line.
pub fn announce_ok(s: &str) -> bool {
    s.len() <= 512 && s.starts_with("@mod:") && !s.chars().any(char::is_control)
}

/// The lobby's `args` filtered (HOST_AGENT.md §5): a token starting with `-` is a flag and is kept, with the one
/// token after it when that one does not start with `-` (its value, e.g. `-tickhz 30`); a token that is neither is
/// dropped, and so is a flag of [`OWN_FLAGS`] with its value.
pub fn filter_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if !a.starts_with('-') || a.len() < 2 {
            continue;
        }
        let value = args.get(i).filter(|v| !v.starts_with('-')).cloned();
        if value.is_some() {
            i += 1;
        }
        if OWN_FLAGS.iter().any(|f| f.eq_ignore_ascii_case(a)) {
            continue;
        }
        out.push(a.clone());
        out.extend(value);
    }
    out
}

/// The game server's argv after the program (DSERVER_PIPE.md §2): `-port -world -guid -idle -log -bindip`, the
/// configured extra arguments, then the lobby's filtered `args`. Each entry is one argv entry: no shell.
///
/// MAP-83: `pack_world` = the absolute path of a community map's world in its unpacked pack: `-world` names that file
/// (as a local launch does, MAP-36), and `-worldfile <it> -announce <announce> -worldobjects world` come last, so they
/// win over the configured arguments (dc-dserver takes the last value of a flag).
pub fn dserver_argv(
    req: &StartReq,
    port: u16,
    log_file: &Path,
    bind_ip: &str,
    extra: &[String],
    pack_world: Option<&Path>,
) -> Vec<OsString> {
    let mut v: Vec<OsString> = vec![
        "-port".into(),
        port.to_string().into(),
        "-world".into(),
        pack_world.map_or_else(|| req.world.clone().into(), |p| p.as_os_str().to_owned()),
        "-guid".into(),
        req.guid.clone().into(),
        "-idle".into(),
        req.idle.max(0).to_string().into(),
        "-log".into(),
        log_file.as_os_str().to_owned(),
        "-bindip".into(),
        bind_ip.into(),
    ];
    v.extend(extra.iter().map(OsString::from));
    v.extend(filter_args(&req.args).into_iter().map(OsString::from));
    if let Some(p) = pack_world {
        v.push("-worldfile".into());
        v.push(p.as_os_str().to_owned());
        v.push("-announce".into());
        v.push(req.announce.clone().into());
        v.push("-worldobjects".into());
        v.push("world".into());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_may_carry_weapon_content_and_args_cannot_name_it() {
        let r = StartReq::parse(r#"{"world":"W","content":"{}","content_hash":"ab"}"#).unwrap();
        assert_eq!((r.content.as_str(), r.content_hash.as_str()), ("{}", "ab"));
        assert_eq!(StartReq::parse(r#"{"world":"W"}"#).unwrap().content, "");
        assert!(OWN_FLAGS.contains(&"-content"));
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn content_lines_parse_and_a_bad_name_is_bad() {
        assert_eq!(
            parse(&format!("content {SHA} Zm9v")),
            FromLobby::Content {
                hash: SHA.into(),
                data: "Zm9v".into()
            }
        );
        assert!(matches!(parse("content ../x Zm9v"), FromLobby::Bad(_)));
        assert!(matches!(
            parse(&format!("content {}", SHA.to_uppercase())),
            FromLobby::Bad(_)
        ));
        assert!(matches!(
            parse(&format!("content {SHA}")),
            FromLobby::Bad(_)
        ));
    }

    #[test]
    fn pack_lines_parse_and_a_bad_name_is_bad() {
        assert_eq!(
            parse(&format!("pack {SHA} Zm9v")),
            FromLobby::Pack {
                sha: SHA.into(),
                data: "Zm9v".into()
            }
        );
        assert!(matches!(parse("pack ../x Zm9v"), FromLobby::Bad(_)));
        assert!(matches!(
            parse(&format!("pack {}", SHA.to_uppercase())),
            FromLobby::Bad(_)
        ));
        assert!(matches!(parse(&format!("pack {SHA}")), FromLobby::Bad(_)));
        assert_eq!(out::have(SHA), format!("have {SHA}"));
    }

    fn pack_request(world_file: &str, announce: &str) -> String {
        format!(
            r#"{{"world":"@mod:m/assets/maps/m.dat","world_pack":{{"sha256":"{SHA}","size":9,"url":"/mods/{SHA}.zip"}},"world_file":"{world_file}","announce":"{announce}"}}"#
        )
    }

    #[test]
    fn a_world_pack_request_is_checked() {
        let good = "@mod:m/assets/maps/m.dat";
        let r = StartReq::parse(&pack_request("assets/maps/m.dat", good)).unwrap();
        let p = r.world_pack.as_ref().unwrap();
        assert_eq!((p.sha256.as_str(), p.size), (SHA, 9));
        assert_eq!(p.url, format!("/mods/{SHA}.zip"));
        for bad in [
            "../x.dat",
            "/etc/passwd",
            "a//b",
            "a/./b",
            "a\\\\b",
            "c:/x",
            "",
        ] {
            assert!(StartReq::parse(&pack_request(bad, good)).is_err(), "{bad}");
        }
        for bad in ["-log", "x", ""] {
            assert!(
                StartReq::parse(&pack_request("a.dat", bad)).is_err(),
                "{bad}"
            );
        }
        // the name is checked; the fields do not come without a pack
        let upper = pack_request("a.dat", good).replace(SHA, &SHA.to_uppercase());
        assert!(StartReq::parse(&upper).is_err());
        assert!(StartReq::parse(r#"{"world":"W","world_file":"a.dat"}"#).is_err());
        assert!(StartReq::parse(r#"{"world":"W","announce":"@mod:a"}"#).is_err());
        let plain = StartReq::parse(r#"{"world":"W"}"#).unwrap();
        assert!(
            plain.world_pack.is_none() && plain.world_file.is_empty() && plain.announce.is_empty()
        );
    }

    #[test]
    fn the_argv_of_a_pack_world_ends_with_the_three_flags_and_args_cannot_name_them() {
        let json = pack_request("assets/maps/m.dat", "@mod:m/assets/maps/m.dat").replacen(
            "{",
            r#"{"args":["-worldfile","/etc/x","-announce","y","-WORLDOBJECTS","off","-tickhz","30"],"#,
            1,
        );
        let r = StartReq::parse(&json).unwrap();
        let world = Path::new("/opt/decombat/packs/abc/assets/maps/m.dat");
        let v = dserver_argv(
            &r,
            27890,
            Path::new("/l/g.log"),
            "0.0.0.0",
            &["-worldobjects".into(), "off".into()],
            Some(world),
        );
        let v: Vec<String> = v.into_iter().map(|s| s.into_string().unwrap()).collect();
        assert_eq!(
            v[2..4],
            ["-world", "/opt/decombat/packs/abc/assets/maps/m.dat"]
        );
        assert_eq!(
            v[12..],
            [
                "-worldobjects",
                "off",
                "-tickhz",
                "30",
                "-worldfile",
                "/opt/decombat/packs/abc/assets/maps/m.dat",
                "-announce",
                "@mod:m/assets/maps/m.dat",
                "-worldobjects",
                "world"
            ]
        );
    }

    #[test]
    fn lobby_lines_parse() {
        assert_eq!(
            parse("ok name=us-west lobby=abc"),
            FromLobby::Ok("name=us-west lobby=abc".into())
        );
        assert_eq!(
            parse("refuse bad token"),
            FromLobby::Refuse("bad token".into())
        );
        assert_eq!(
            parse("start 9 {\"world\":\"W\"}"),
            FromLobby::Start {
                gid: 9,
                json: "{\"world\":\"W\"}".into()
            }
        );
        // the text is verbatim, inner and trailing spaces included
        assert_eq!(
            parse("line 9 kick  3 "),
            FromLobby::Line {
                gid: 9,
                text: "kick  3 ".into()
            }
        );
        assert_eq!(
            parse("line 9"),
            FromLobby::Line {
                gid: 9,
                text: String::new()
            }
        );
        assert_eq!(parse("quit 12"), FromLobby::Quit { gid: 12 });
        assert_eq!(parse("ping 4"), FromLobby::Ping("4".into()));
        assert_eq!(parse("pong 4"), FromLobby::Pong("4".into()));
        assert!(matches!(parse("quit x"), FromLobby::Bad(_)));
        assert!(matches!(parse("quit -1"), FromLobby::Bad(_)));
        assert!(matches!(parse("start 9"), FromLobby::Bad(_)));
        assert!(matches!(parse("line"), FromLobby::Bad(_)));
        assert!(matches!(parse("reboot now"), FromLobby::Unknown(_)));
        // HA-2: the build offer and its transfer
        assert_eq!(parse("update a b 3 t"), FromLobby::Update("a b 3 t".into()));
        assert_eq!(
            parse("updpart abc Zm9v"),
            FromLobby::UpdPart {
                commit: "abc".into(),
                data: "Zm9v".into()
            }
        );
        assert_eq!(parse("updend abc"), FromLobby::UpdEnd("abc".into()));
        assert_eq!(
            parse("updateno abc busy\tnow"),
            FromLobby::UpdNo {
                commit: "abc".into(),
                reason: "busy now".into()
            }
        );
        assert!(matches!(parse("update"), FromLobby::Bad(_)));
        assert!(matches!(parse("updpart abc"), FromLobby::Bad(_)));
    }

    #[test]
    fn agent_lines_are_written_as_the_spec_shows() {
        assert_eq!(
            out::hello("us-west", "T", "abc1234", 27888, 27899, 2),
            "hello name=us-west token=T version=abc1234 ports=27888-27899 running=2 content=1 contentlines=1 packs=1 update=1"
        );
        assert_eq!(
            out::started(7, 27888, 4242),
            "started 7 port=27888 pid=4242"
        );
        assert_eq!(out::failed(7, "no free\nport"), "failed 7 no free port");
        assert_eq!(out::failed(7, ""), "failed 7 error");
        assert_eq!(
            out::event(7, "@ready port=1 world=W"),
            "event 7 @ready port=1 world=W"
        );
        assert_eq!(out::exited(7, Some(0)), "exited 7 code=0");
        assert_eq!(out::exited(7, None), "exited 7 code=-1");
        assert_eq!(out::running(7, 27890, 9), "running 7 port=27890 pid=9");
        assert_eq!(out::ping(3), "ping 3");
        assert_eq!(out::pong("3"), "pong 3");
    }

    #[test]
    fn lines_split_across_reads_and_crlf() {
        let mut b = LineBuf::default();
        assert_eq!(b.push(b"ab"), vec![]);
        assert_eq!(
            b.push(b"c\r\nde\n\nf"),
            vec![
                Piece::Line("abc".into()),
                Piece::Line("de".into()),
                Piece::Line(String::new())
            ]
        );
        assert_eq!(b.push(b"\n"), vec![Piece::Line("f".into())]);
    }

    #[test]
    fn a_line_over_16_kib_is_dropped_and_the_next_one_kept() {
        let mut b = LineBuf::default();
        // exactly the limit (with a CR) passes
        let ok = vec![b'x'; MAX_LINE];
        let mut data = ok.clone();
        data.extend_from_slice(b"\r\n");
        assert_eq!(b.push(&data), vec![Piece::Line("x".repeat(MAX_LINE))]);
        // one over, fed in pieces, is dropped; the following line survives
        let long = vec![b'y'; MAX_LINE + 1];
        assert_eq!(b.push(&long[..100]), vec![]);
        assert_eq!(b.push(&long[100..]), vec![]);
        assert_eq!(b.push(b"zz"), vec![]);
        assert_eq!(
            b.push(b"z\nnext\n"),
            vec![Piece::TooLong(MAX_LINE + 4), Piece::Line("next".into())]
        );
        // over the limit in one read
        let mut data = vec![b'q'; MAX_LINE + 1];
        data.extend_from_slice(b"\nafter\n");
        assert_eq!(
            b.push(&data),
            vec![Piece::TooLong(MAX_LINE + 1), Piece::Line("after".into())]
        );
    }

    #[test]
    fn start_requests_are_checked() {
        let r = StartReq::parse(
            r#"{"port":0,"world":"Worlds/X","guid":"ca","idle":30,"log":"dserver_c1_r3_g9.log","args":["-tickhz","30"]}"#,
        )
        .unwrap();
        assert_eq!(r.idle, 30);
        assert_eq!(r.log_name(9), "dserver_c1_r3_g9.log");
        let r = StartReq::parse(r#"{"world":"W"}"#).unwrap();
        assert_eq!((r.port, r.guid.as_str(), r.idle), (0, "ca", 60));
        assert_eq!(r.log_name(4), "dserver_g4.log");
        assert!(StartReq::parse(r#"{"world":""}"#).is_err());
        assert!(StartReq::parse(r#"{"world":"W","log":"../x.log"}"#).is_err());
        assert!(StartReq::parse(r#"{"world":"W","log":"/etc/passwd"}"#).is_err());
        assert!(StartReq::parse(r#"{"world":"W","log":"a\\b.log"}"#).is_err());
        assert!(StartReq::parse(r#"{"world":"W","guid":"c a"}"#).is_err());
        assert!(StartReq::parse("not json").is_err());
    }

    #[test]
    fn the_lobby_args_keep_flags_and_their_values_only() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            filter_args(&a(&["-tickhz", "30", "-fps", "60"])),
            a(&["-tickhz", "30", "-fps", "60"])
        );
        // a bare token not following a flag is dropped; a flag after a flag has no value
        assert_eq!(
            filter_args(&a(&["rm", "-rf", "-x", "/", "y"])),
            a(&["-rf", "-x", "/"])
        );
        // the agent's own flags cannot be given again
        assert_eq!(
            filter_args(&a(&[
                "-log", "/etc/x", "-BINDIP", "1.2.3.4", "-port", "-n", "2"
            ])),
            a(&["-n", "2"])
        );
        assert_eq!(filter_args(&a(&["-", "x"])), Vec::<String>::new());
    }

    #[test]
    fn the_argv_is_one_entry_per_argument() {
        let r =
            StartReq::parse(r#"{"world":"Worlds/A B","idle":-5,"args":["-tickhz","30"]}"#).unwrap();
        let v = dserver_argv(
            &r,
            27890,
            Path::new("/l/g.log"),
            "0.0.0.0",
            &["-maxplayers".into(), "16".into()],
            None,
        );
        let v: Vec<String> = v.into_iter().map(|s| s.into_string().unwrap()).collect();
        assert_eq!(
            v,
            [
                "-port",
                "27890",
                "-world",
                "Worlds/A B",
                "-guid",
                "ca",
                "-idle",
                "0",
                "-log",
                "/l/g.log",
                "-bindip",
                "0.0.0.0",
                "-maxplayers",
                "16",
                "-tickhz",
                "30"
            ]
        );
    }
}

//! Deploys from the dashboard (LG-392 .. LG-394) and the build offer to the host agents (HA-2).
// Provenance: ORIGINAL

use super::*;
use crate::clanweb::parse_request;
use crate::state::testing::state;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const NOW: i64 = 1_790_000_000;
const C: &str = "2222222222222222222222222222222222222222";
const TOKEN: &str = "0123456789abcdef01234567";
/// The base64 of a signature file as deploy_from_git.sh writes it (only its shape matters to the lobby).
const SIG: &str = "LS0tLS1CRUdJTiBTU0ggU0lHTkFUVVJFLS0tLS0K";

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc-deploy-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("requests")).unwrap();
    std::fs::create_dir_all(d.join("host")).unwrap();
    d
}

fn shared_with(dir: Option<&Path>) -> Shared {
    let mut st = state();
    st.opts.deploy_dir = dir.map(Path::to_path_buf);
    Arc::new(Mutex::new(st))
}

/// Root's state files as deploy_from_git.sh writes them.
fn root_files(d: &Path) {
    std::fs::write(
        d.join("available.json"),
        json!({"ok": true, "error": "", "checked": NOW - 60, "branch": "master", "master": C, "master_short": &C[..8],
               "running": "11111111", "running_found": true, "new_count": 1,
               "new": [{"sha": C, "short": &C[..8], "subject": "a fix", "date": NOW - 3600}],
               "older": [{"sha": "1".repeat(40), "short": "11111111", "subject": "before", "date": NOW - 7200}]})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        d.join("history.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"id": "1-00000001", "short": "11111111", "ok": true, "message": "deployed 11111111"}),
            json!({"id": "2-00000002", "short": "22222222", "ok": false, "message": "the build failed"})
        ),
    )
    .unwrap();
}

fn host_files(d: &Path, data: &[u8]) {
    let name = "decombat-servers-linux-x86_64-22222222.tar.gz";
    std::fs::write(d.join("host").join(name), data).unwrap();
    std::fs::write(
        d.join("host").join("desired.txt"),
        format!(
            "commit={C}\ntarball={name}\nsha256={}\nsize={}\nseq=7\nsig={SIG}\n",
            dc_rom::sha256_hex(data),
            data.len()
        ),
    )
    .unwrap();
}

fn get(sh: &Shared) -> Value {
    let req = parse_request("GET /ops/updates HTTP/1.1\r\nHost: 127.0.0.1:10090\r\n\r\n").unwrap();
    let (st, _, body) = serve_at(
        sh,
        &req,
        "GET /ops/updates HTTP/1.1\r\n\r\n",
        &[],
        None,
        NOW,
    );
    assert_eq!(st, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

/// A POST from the loopback dashboard: its origin and the per-process page token, unless the test changes them.
fn post(sh: &Shared, body: &Value, origin: &str, csrf: Option<&str>, ctype: &str) -> (u16, Value) {
    let tok = match csrf {
        Some(t) => t.to_string(),
        None => {
            let mut g = sh.lock().unwrap();
            let st = &mut *g;
            st.beta.csrf(&mut st.keys)
        }
    };
    let head = format!(
        "POST /ops/updates/deploy HTTP/1.1\r\nHost: 127.0.0.1:10090\r\nOrigin: {origin}\r\nContent-Type: {ctype}\r\nX-Admin-CSRF: {tok}\r\n\r\n"
    );
    let req = parse_request(&head).unwrap();
    let (st, _, out) = serve_at(sh, &req, &head, body.to_string().as_bytes(), None, NOW);
    (st, serde_json::from_str(&out).unwrap())
}

fn ok_post(sh: &Shared, body: Value) -> (u16, Value) {
    post(
        sh,
        &body,
        "http://127.0.0.1:10090",
        None,
        "application/json",
    )
}

fn requests(d: &Path) -> Vec<Value> {
    let mut v: Vec<_> = std::fs::read_dir(d.join("requests"))
        .unwrap()
        .flatten()
        .map(|e| {
            serde_json::from_str::<Value>(&std::fs::read_to_string(e.path()).unwrap()).unwrap()
        })
        .collect();
    v.sort_by_key(|x| x["at"].as_i64());
    v
}

fn deploy_events(sh: &Shared) -> Vec<(bool, String)> {
    let g = sh.lock().unwrap();
    let mut q =
        g.db.prepare("SELECT problem, detail FROM ops_events WHERE kind = 'deploy' ORDER BY id")
            .unwrap();
    q.query_map([], |r| {
        Ok((r.get::<_, i64>(0)? != 0, r.get::<_, String>(1)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn without_a_deploy_folder_the_tab_says_so_and_nothing_is_written() {
    let sh = shared_with(None);
    let v = get(&sh);
    assert_eq!(v["set_up"], false);
    assert!(
        v["message"].as_str().unwrap().contains("[deploy] dir"),
        "{v}"
    );
    let (st, out) = ok_post(&sh, json!({"sha": &C[..8]}));
    assert_eq!(st, 503, "{out}");
    // a folder without requests/ (deploy_setup.sh not run)
    let d = tmp("nosetup");
    std::fs::remove_dir_all(d.join("requests")).unwrap();
    let sh = shared_with(Some(&d));
    assert_eq!(get(&sh)["set_up"], false);
    assert_eq!(ok_post(&sh, json!({"sha": &C[..8]})).0, 503);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_tab_shows_the_builds_the_new_commits_the_status_and_the_history() {
    let d = tmp("get");
    root_files(&d);
    host_files(&d, b"tarball bytes");
    let sh = shared_with(Some(&d));
    let v = get(&sh);
    assert_eq!(v["set_up"], true);
    assert_eq!(v["lobby"]["build"], crate::opsmon::build_sha());
    assert_eq!(v["available"]["new"][0]["subject"], "a fix");
    assert_eq!(v["host_build"]["short"], &C[..8]);
    assert_eq!(v["matches"], 0);
    assert_eq!(
        (v["deploying"].clone(), v["pending"].clone()),
        (json!(false), json!(false))
    );
    // newest first
    assert_eq!(v["history"][0]["id"], "2-00000002");
    assert_eq!(v["history"][1]["id"], "1-00000001");
    // a malformed host pointer offers nothing
    std::fs::write(d.join("host").join("desired.txt"), "commit=x\n").unwrap();
    assert!(get(&sh)["host_build"].is_null());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_deploy_request_needs_the_write_guard_and_lands_as_a_file_for_root() {
    let d = tmp("post");
    root_files(&d);
    let sh = shared_with(Some(&d));
    let body = json!({"sha": &C[..8], "targets": ["hosts", "vps"], "end_matches": false});
    // LG-393: JSON, the page's own origin, the page token
    assert_eq!(
        post(&sh, &body, "http://127.0.0.1:10090", None, "text/plain").0,
        415
    );
    assert_eq!(
        post(&sh, &body, "http://evil.example", None, "application/json").0,
        403
    );
    assert_eq!(
        post(
            &sh,
            &body,
            "http://127.0.0.1:10090",
            Some("wrong"),
            "application/json"
        )
        .0,
        403
    );
    assert!(
        requests(&d).is_empty(),
        "nothing written for a refused guard"
    );
    // bad fields
    for bad in [
        json!({"sha": "zz"}),
        json!({"sha": "abc"}),
        json!({"sha": format!("{C}0")}),
        json!({"sha": &C[..8], "targets": []}),
        json!({"sha": &C[..8], "targets": ["vps", "everything"]}),
        json!({"sha": &C[..8], "targets": "vps"}),
        json!({"sha": &C[..8], "end_matches": "yes"}),
        json!(["x"]),
    ] {
        assert_eq!(ok_post(&sh, bad.clone()).0, 400, "{bad}");
    }
    assert!(requests(&d).is_empty());
    let (st, out) = ok_post(&sh, body.clone());
    assert_eq!(st, 200, "{out}");
    let r = requests(&d);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["sha"], &C[..8]);
    assert_eq!(r[0]["targets"], json!(["vps", "hosts"]), "in a fixed order");
    assert_eq!(r[0]["end_matches"], false);
    assert_eq!(r[0]["by"], "the admin port");
    let id = out["id"].as_str().unwrap().to_string();
    assert!(d.join("requests").join(format!("{id}.json")).is_file());
    assert!(
        id.starts_with(&format!("{NOW}-")) && id.len() == NOW.to_string().len() + 9,
        "{id}"
    );
    // one at a time: a waiting request, then a deploy in progress
    assert_eq!(ok_post(&sh, body.clone()).0, 409);
    std::fs::remove_file(d.join("requests").join(format!("{id}.json"))).unwrap();
    std::fs::write(
        d.join("status.json"),
        json!({"id": id, "phase": "building", "ok": null, "finished": null, "updated": NOW - 30})
            .to_string(),
    )
    .unwrap();
    assert_eq!(get(&sh)["deploying"], true);
    assert_eq!(ok_post(&sh, body.clone()).0, 409);
    // a status that has not moved for hours is a dead deploy, not a running one
    std::fs::write(
        d.join("status.json"),
        json!({"id": id, "phase": "building", "ok": null, "finished": null, "updated": NOW - 4 * 3600}).to_string(),
    )
    .unwrap();
    assert_eq!(get(&sh)["deploying"], false);
    // LG-394: every request and refusal is an event; the outcome once
    std::fs::write(
        d.join("status.json"),
        json!({"id": id, "short": &C[..8], "phase": "failed", "ok": false, "message": "the build failed",
               "finished": NOW, "updated": NOW})
        .to_string(),
    )
    .unwrap();
    get(&sh);
    get(&sh);
    let ev = deploy_events(&sh);
    assert!(
        ev.iter().any(|(p, t)| !p
            && t.contains(&format!(
                "deploy {id} requested: {} \"a fix\" to vps + hosts by the admin port",
                &C[..8]
            ))),
        "{ev:?}"
    );
    assert!(
        ev.iter()
            .any(|(p, t)| !p && t.contains("refused: another deploy runs or waits")),
        "{ev:?}"
    );
    let outcome: Vec<_> = ev
        .iter()
        .filter(|(_, t)| t.starts_with(&format!("deploy {id} finished")))
        .collect();
    assert_eq!(outcome.len(), 1, "{ev:?}");
    assert!(
        outcome[0].0 && outcome[0].1.contains("FAILED"),
        "a failed deploy is a problem: {ev:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

fn bare_game(id: u64) -> crate::game::Game {
    crate::game::Game {
        id,
        room: crate::room::RoomKey { channel: 1, no: 1 },
        port: 27015,
        map: 0,
        mode: 0,
        link: Default::default(),
        ready: true,
        over: false,
        players: Vec::new(),
        clients: Default::default(),
        left: Default::default(),
        result_done: false,
        entry_keys: Default::default(),
        seat_tickets: Default::default(),
        seat_told: Default::default(),
        reap_scheduled: false,
        tmp_workdir: None,
        hisec_keys: Default::default(),
        net: Default::default(),
    }
}

#[test]
fn running_matches_refuse_a_vps_deploy_unless_it_ends_them() {
    let d = tmp("matches");
    let sh = shared_with(Some(&d));
    sh.lock().unwrap().games.insert(9, bare_game(9));
    let (st, out) = ok_post(&sh, json!({"sha": &C[..8]}));
    assert_eq!(st, 409, "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap()
            .contains("end running matches"),
        "{out}"
    );
    assert!(requests(&d).is_empty());
    // the hosts alone restart nothing here (their updater waits for their own matches)
    assert_eq!(
        ok_post(&sh, json!({"sha": &C[..8], "targets": ["hosts"]})).0,
        200
    );
    for e in std::fs::read_dir(d.join("requests")).unwrap().flatten() {
        std::fs::remove_file(e.path()).unwrap();
    }
    let (st, _) = ok_post(&sh, json!({"sha": &C[..8], "end_matches": true}));
    assert_eq!(st, 200);
    assert_eq!(requests(&d)[0]["end_matches"], true);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn on_the_admin_web_the_session_origin_and_token_guard_the_write() {
    let d = tmp("web");
    let sh = shared_with(Some(&d));
    let w = crate::betaweb::WriteGuard {
        origin: "https://admin.example.com".into(),
        csrf: "session-token".into(),
        by: "the admin web".into(),
    };
    let send = |origin: &str, tok: &str| {
        let head = format!(
            "POST /ops/updates/deploy HTTP/1.1\r\nHost: admin.example.com\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nX-Admin-CSRF: {tok}\r\n\r\n"
        );
        let req = parse_request(&head).unwrap();
        serve_at(
            &sh,
            &req,
            &head,
            json!({"sha": &C[..8]}).to_string().as_bytes(),
            Some(&w),
            NOW,
        )
        .0
    };
    assert_eq!(send("http://admin.example.com", "session-token"), 403);
    assert_eq!(send("https://admin.example.com", "wrong"), 403);
    assert_eq!(send("https://admin.example.com", "session-token"), 200);
    assert_eq!(requests(&d)[0]["by"], "the admin web");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_dashboard_page_carries_its_write_token() {
    let sh = shared_with(None);
    let req = parse_request("GET /ops HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let (_, _, page) = crate::opsweb::serve(
        &sh,
        &req,
        "GET /ops HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        &[],
        None,
    );
    let tok = {
        let mut g = sh.lock().unwrap();
        let st = &mut *g;
        st.beta.csrf(&mut st.keys)
    };
    assert!(
        page.contains(&format!("<meta name=\"csrf\" content=\"{tok}\">")),
        "the loopback page's token"
    );
    assert!(!page.contains("__CSRF__"));
    let w = crate::betaweb::WriteGuard {
        origin: "https://admin.example.com".into(),
        csrf: "session-token".into(),
        by: "the admin web".into(),
    };
    let (_, _, page) = crate::opsweb::serve(&sh, &req, "GET /ops HTTP/1.1\r\n\r\n", &[], Some(&w));
    assert!(page.contains("<meta name=\"csrf\" content=\"session-token\">"));
}

#[test]
fn host_builds_are_checked_against_their_file() {
    let d = tmp("hostbuild");
    host_files(&d, b"0123456789");
    let b = host_build(&d).unwrap();
    assert_eq!((b.commit.as_str(), b.size), (C, 10));
    // HA-2: root's sequence number and signature are relayed as they are
    assert_eq!((b.seq, b.sig.as_str()), (7, SIG));
    let good = std::fs::read_to_string(d.join("host").join("desired.txt")).unwrap();
    for (what, text) in [
        ("no signature", good.replace(&format!("sig={SIG}\n"), "")),
        (
            "a signature of other characters",
            good.replace(&format!("sig={SIG}"), "sig=not base64!"),
        ),
        ("no sequence number", good.replace("seq=7\n", "")),
    ] {
        std::fs::write(d.join("host").join("desired.txt"), text).unwrap();
        assert!(host_build(&d).is_none(), "{what}");
    }
    std::fs::write(d.join("host").join("desired.txt"), good).unwrap();
    assert!(host_build(&d).is_some());
    // the file is not the size the pointer says
    std::fs::write(d.join("host").join(&b.tarball), b"short").unwrap();
    assert!(host_build(&d).is_none());
    // a name update.sh would refuse
    std::fs::write(
        d.join("host").join("desired.txt"),
        format!(
            "commit={C}\ntarball=../x.tar.gz\nsha256={}\nsize=5\n",
            "a".repeat(64)
        ),
    )
    .unwrap();
    assert!(host_build(&d).is_none());
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------------ HA-2 over a host connection

struct Agent {
    r: BufReader<TcpStream>,
    w: TcpStream,
}

impl Agent {
    fn dial(sh: &Shared) -> Agent {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (s, peer) = l.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let sh = Arc::clone(sh);
        std::thread::spawn(move || {
            let r = s.try_clone().unwrap();
            crate::hosts::serve(&sh, r, s, peer)
        });
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        Agent {
            r: BufReader::new(c.try_clone().unwrap()),
            w: c,
        }
    }
    fn send(&mut self, l: &str) {
        self.w.write_all(format!("{l}\n").as_bytes()).unwrap();
    }
    fn expect(&mut self) -> String {
        loop {
            let mut l = String::new();
            assert!(self.r.read_line(&mut l).unwrap() > 0, "the lobby closed");
            let l = l.trim_end().to_string();
            if !l.starts_with("ping ") {
                return l;
            }
        }
    }
}

fn host_lobby(d: &Path) -> Shared {
    let sh = shared_with(Some(d));
    {
        let mut g = sh.lock().unwrap();
        g.opts.host_agents = vec![dc_config::HostAgentCfg {
            name: "us-west".into(),
            token: TOKEN.into(),
            public_address: "203.0.113.9".into(),
        }];
        g.hosts.timing.refuse_delay = Duration::from_millis(10);
    }
    sh
}

#[test]
fn an_agent_that_takes_updates_is_offered_the_build_and_gets_it_whole() {
    let d = tmp("offer");
    let data: Vec<u8> = (0..30_000u32).map(|i| (i * 13 + 7) as u8).collect();
    let sh = host_lobby(&d);
    // published after this lobby started (a deploy to the hosts), so offered though it is not the lobby's build
    std::thread::sleep(Duration::from_millis(30));
    host_files(&d, &data);
    let mut a = Agent::dial(&sh);
    a.send(&format!("hello name=us-west token={TOKEN} version=11111111 ports=27888-27899 running=0 content=1 packs=1 update=1"));
    assert!(a.expect().starts_with("ok "));
    let sha = dc_rom::sha256_hex(&data);
    assert_eq!(
        a.expect(),
        format!(
            "update {C} {sha} {} decombat-servers-linux-x86_64-22222222.tar.gz 7 {SIG}",
            data.len()
        )
    );
    // another commit is refused; the offered one comes in pieces under the line limit
    a.send(&format!("getupdate {}", "3".repeat(40)));
    assert!(a
        .expect()
        .starts_with(&format!("updateno {} ", "3".repeat(40))));
    a.send(&format!("getupdate {C}"));
    let mut got = Vec::new();
    loop {
        let l = a.expect();
        assert!(l.len() < 16 * 1024, "a line over the limit");
        if let Some(b) = l.strip_prefix(&format!("updpart {C} ")) {
            got.extend(crate::identity::b64_decode(b).unwrap());
        } else {
            assert_eq!(l, format!("updend {C}"));
            break;
        }
    }
    assert_eq!(got, data);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn no_offer_for_an_agent_on_the_build_or_without_update_1_and_none_during_its_match() {
    let d = tmp("nooffer");
    host_files(&d, b"x");
    let sh = host_lobby(&d);
    // on the offered build already (a short id of it)
    let mut a = Agent::dial(&sh);
    a.send(&format!(
        "hello name=us-west token={TOKEN} version={} ports=1-2 running=0 update=1",
        &C[..8]
    ));
    assert!(a.expect().starts_with("ok "));
    a.send("ping 1");
    assert_eq!(a.expect(), "pong 1", "no update line before it");
    drop(a);
    let wait = std::time::Instant::now();
    while sh.lock().unwrap().hosts.online.contains_key("us-west")
        && wait.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    // an older agent (no update=1)
    let mut a = Agent::dial(&sh);
    a.send(&format!(
        "hello name=us-west token={TOKEN} version=11111111 ports=1-2 running=0 content=1 packs=1"
    ));
    assert!(a.expect().starts_with("ok "));
    a.send("ping 2");
    assert_eq!(a.expect(), "pong 2");
    // getupdate while a match of it runs
    {
        let mut g = sh.lock().unwrap();
        let mut game = bare_game(5);
        game.link = crate::game::Link::Remote(crate::game::RemoteLink {
            agent: "us-west".into(),
            address: "203.0.113.9".into(),
            port: Some(27888),
            pid: Some(1),
            tx: None,
            stdin_open: true,
            alive: true,
        });
        g.games.insert(5, game);
    }
    a.send(&format!("getupdate {C}"));
    assert_eq!(
        a.expect(),
        format!("updateno {C} a match runs on this host")
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_build_older_than_this_lobby_and_not_its_own_is_not_offered() {
    // deploy_all.sh installed this lobby by hand after the dashboard offered another build to the hosts
    let d = tmp("stale");
    host_files(&d, b"x");
    let day_ago = std::time::SystemTime::now() - Duration::from_secs(86_400);
    std::fs::File::options()
        .write(true)
        .open(d.join("host").join("desired.txt"))
        .unwrap()
        .set_modified(day_ago)
        .unwrap();
    let sh = host_lobby(&d);
    let mut a = Agent::dial(&sh);
    a.send(&format!(
        "hello name=us-west token={TOKEN} version=11111111 ports=1-2 running=0 update=1"
    ));
    assert!(a.expect().starts_with("ok "));
    a.send("ping 3");
    assert_eq!(a.expect(), "pong 3", "no update line");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_admin_port_deploys_only_with_its_token() {
    // LG-393: reading the tab needs no token there; a deploy does (the admin web deploys behind its key)
    let d = tmp("token");
    let sh = shared_with(Some(&d));
    let lo = Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let head = "POST /ops/updates/deploy HTTP/1.1\r\nHost: 127.0.0.1:10090\r\nOrigin: http://127.0.0.1:10090\r\nContent-Type: application/json\r\n\r\n";
    let body = json!({"sha": &C[..8]}).to_string();
    let (st, _, out) =
        crate::opsweb::handle_with_body(&sh, head, body.as_bytes(), lo, None, true).unwrap();
    assert_eq!(st, 403, "{out}");
    assert!(out.contains("--admin-token"), "{out}");
    let get = "GET /ops/updates HTTP/1.1\r\nHost: 127.0.0.1:10090\r\n\r\n";
    assert_eq!(
        crate::opsweb::handle_with_body(&sh, get, &[], lo, None, true)
            .unwrap()
            .0,
        200
    );
    // with a token the request goes on to the page guard (which wants the page token here)
    let (st, _, _) =
        crate::opsweb::handle_with_body(&sh, head, body.as_bytes(), lo, Some("t"), true).unwrap();
    assert_eq!(st, 403);
    assert!(requests(&d).is_empty());
    let tok = {
        let mut g = sh.lock().unwrap();
        let st = &mut *g;
        st.beta.csrf(&mut st.keys)
    };
    let head = format!(
        "{}\r\nX-Admin-CSRF: {tok}\r\n\r\n",
        head.trim_end_matches("\r\n")
    );
    let (st, _, out) =
        crate::opsweb::handle_with_body(&sh, &head, body.as_bytes(), lo, Some("t"), true).unwrap();
    assert_eq!(st, 200, "{out}");
    let _ = std::fs::remove_dir_all(&d);
}

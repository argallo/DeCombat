//! After the lobby stored a `@result` it sends `gain <uid> <exp> <gp>` to the game server, one line per `@player`
//! account, with what it added (DSERVER_PIPE.md §12.B). Real lobby binary, stand-in game server in `linger` mode
//! (it keeps reading stdin after `@end`), the lines read back from the stand-in's stdin record. Also: the switch
//! `dserver.send_gain_lines` from lobby.json and from the `DC_*` environment (server.toml layer), and the three
//! other keys the server.toml layer now carries (LDO-1, RM-1, AUTH-1).
// SPEC: DSERVER_PIPE.md §12.B (gain line), §4.5 (lobby effect of @result)
// DC-DEFAULT: DSV-35 (proposed) dserver.send_gain_lines; LDO-1, RM-1, AUTH-1 (proposed) via server.toml
// HARNESS: loadout_tokens.rs (same process-test style)

mod support;
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(20);

fn opts(json: &str, env: &[(&str, &str)]) -> LobbyOpts {
    LobbyOpts {
        dserver_exe: Some(fake_dserver_exe()),
        dserver_world: Some(r"Worlds\SnowValley".into()),
        lobby_json: Some(json.into()),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

/// One account's number and its (exp, gp).
type Totals = (u32, (i64, i64));

/// Plays one match with the stand-in; returns (alice, bob) account numbers and their (exp, gp) before the match.
fn run_match(l: &Lobby) -> (Totals, Totals, Client) {
    l.set_fake_mode("linger");
    let (mut a, na) = player(l, "alice", "Alice");
    let (mut b, nb) = player(l, "bob", "Bob");
    let before = |n: u32| {
        let c = l.character(n);
        (c.0, c.1)
    };
    let (ba, bb) = (before(na), before(nb));
    let no = open_room(&mut a, "Gain room", 0);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);
    let _ = game_address(&mut a, T);
    let _ = game_address(&mut b, T);
    // the progress push follows the stored result
    a.recv_until(0x3004, T);
    ((na, ba), (nb, bb), a)
}

fn gain_lines(l: &Lobby) -> Vec<String> {
    l.fake()
        .first()
        .map(|f| {
            f.stdin
                .iter()
                .filter(|s| s.starts_with("gain "))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_stored_result_sends_one_gain_line_per_player() {
    let l = Lobby::start("gain-on", opts("{}", &[]));
    let ((na, ba), (nb, bb), _a) = run_match(&l);
    assert!(
        wait_until(T, || gain_lines(&l).len() >= 2),
        "two gain lines expected, got {:?}\n{}",
        gain_lines(&l),
        l.log_text()
    );
    let lines = gain_lines(&l);
    assert_eq!(
        lines.len(),
        2,
        "exactly one per account (the second @result is ignored): {lines:?}"
    );
    // DC-DEFAULT: DSV-142 (proposed) `gain <uid> <exp> <gp> <exp_bonus> <gp_bonus>`; the bonus is the part of the
    // gain above the plain reward (no win bonus, map factor 1), never above the gain
    let mut bonus_seen = false;
    for (n, b) in [(na, ba), (nb, bb)] {
        let c = l.character(n);
        let want = format!("gain {n} {} {} ", c.0 - b.0, c.1 - b.1);
        let line = lines
            .iter()
            .find(|s| s.starts_with(&want))
            .unwrap_or_else(|| panic!("{want:?} not in {lines:?}"));
        let v: Vec<i64> = line
            .split_whitespace()
            .skip(2)
            .map(|x| x.parse().unwrap())
            .collect();
        assert_eq!(v.len(), 4, "{line:?}");
        assert!(
            v[2] <= v[0] && v[3] <= v[1],
            "bonus above the gain: {line:?}"
        );
        bonus_seen |= v[2] > 0 || v[3] > 0;
    }
    // without a ROM the map factor is 1, so the winner's win bonus is the whole bonus (with a ROM a factor below 1 can
    // eat it)
    if dc_rom::test_rom().is_none() {
        assert!(bonus_seen, "the winner's win bonus is a bonus: {lines:?}");
    }
    // the winner earned something, so the line is not a stand-in for zeros
    let nonzero = lines
        .iter()
        .any(|s| s.split_whitespace().skip(2).any(|v| v != "0"));
    assert!(nonzero, "{lines:?}");
}

#[test]
fn the_switch_in_lobby_json_turns_the_lines_off() {
    let l = Lobby::start(
        "gain-off-json",
        opts(r#"{"dserver.send_gain_lines": false}"#, &[]),
    );
    let (_, _, _a) = run_match(&l);
    assert!(
        wait_until(T, || l.match_rows() == 1),
        "the result should still be stored:\n{}",
        l.log_text()
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(gain_lines(&l).is_empty(), "{:?}", gain_lines(&l));
}

#[test]
fn server_toml_layer_wins_over_lobby_json_for_the_switch() {
    // lobby.json says on, the environment layer (same as server.toml) says off -> off
    let l = Lobby::start(
        "gain-off-env",
        opts(
            r#"{"dserver.send_gain_lines": true}"#,
            &[("DC_DSERVER__SEND_GAIN_LINES", "false")],
        ),
    );
    let (_, _, _a) = run_match(&l);
    assert!(wait_until(T, || l.match_rows() == 1), "{}", l.log_text());
    std::thread::sleep(Duration::from_millis(800));
    assert!(gain_lines(&l).is_empty(), "{:?}", gain_lines(&l));
}

#[test]
fn first_account_number_comes_from_the_server_toml_layer_when_set() {
    // lobby.json says 5000, the environment layer says 7000 -> 7000
    let l = Lobby::start(
        "auth1-env",
        opts(
            r#"{"auth.first_account_no": 5000, "bots.pool_size": 0}"#,
            &[("DC_AUTH__FIRST_ACCOUNT_NO", "7000")],
        ),
    );
    let (_a, na) = player(&l, "carol", "Carol");
    assert_eq!(na, 7000);
    // and lobby.json alone still works
    let l = Lobby::start(
        "auth1-json",
        opts(r#"{"auth.first_account_no": 5000, "bots.pool_size": 0}"#, &[]),
    );
    let (_a, na) = player(&l, "dave", "Dave");
    assert_eq!(na, 5000);
}

#[test]
fn loadout_tokens_switch_comes_from_the_server_toml_layer_when_set() {
    // lobby.json says on, the environment layer says off -> bare user lines
    let l = Lobby::start(
        "ldo-env",
        opts(
            r#"{"dserver.loadout_tokens": true}"#,
            &[("DC_DSERVER__LOADOUT_TOKENS", "false")],
        ),
    );
    l.set_fake_mode("record");
    let (mut a, _) = player(&l, "alice", "Alice");
    let (mut b, _) = player(&l, "bob", "Bob");
    let no = open_room(&mut a, "Env room", 0);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);
    let _ = game_address(&mut a, T);
    let _ = game_address(&mut b, T);
    assert!(wait_until(T, || l
        .fake()
        .first()
        .is_some_and(|f| f.users().len() == 2)));
    for u in l.fake()[0].users() {
        // no loadout tokens; the badge tokens (DSV-141, `rank=` and the clan) are not part of the switch
        assert!(
            u[7..].iter().all(|t| !["loadout=", "face=", "parts="]
                .iter()
                .any(|k| t.starts_with(k))),
            "no loadout tokens expected: {u:?}"
        );
    }
}

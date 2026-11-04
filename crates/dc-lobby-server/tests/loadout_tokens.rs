//! The `user` line the lobby writes to the game server carries the player's equipped weapon slots and face
//! (`loadout=<slot>:<item>,...`, `face=<n>`). Real lobby binary, stand-in game server, scripted accounts whose
//! equipment the test writes into the lobby database before the start.
// SPEC: DSERVER_PIPE.md §3.1 (user line, optional trailing k=v tokens)
// DC-DEFAULT: LDO-1 (proposed) loadout / face tokens; config dserver.loadout_tokens
// HARNESS: pipe_addendum.rs (same process-test style)

mod support;
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(20);

fn opts(json: &str) -> LobbyOpts {
    LobbyOpts {
        dserver_exe: Some(fake_dserver_exe()),
        dserver_world: Some(r"Worlds\SnowValley".into()),
        lobby_json: Some(json.into()),
        ..Default::default()
    }
}

/// Replaces the account's items with exactly `equipped` (slot, item number) plus one unequipped item.
fn set_equipment(l: &Lobby, account: u32, equipped: &[(i64, i64)], face: i64) {
    let db = l.db();
    db.execute("DELETE FROM items WHERE user_no=?1", [account as i64])
        .unwrap();
    for (slot, item) in equipped {
        db.execute(
            "INSERT INTO items (user_no, item_no, equip_slot) VALUES (?1, ?2, ?3)",
            [account as i64, *item, *slot],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO items (user_no, item_no, equip_slot) VALUES (?1, 9999, -1)",
        [account as i64],
    )
    .unwrap();
    db.execute(
        "UPDATE characters SET face=?2 WHERE user_no=?1",
        [account as i64, face],
    )
    .unwrap();
}

fn run_match(l: &Lobby, set: impl Fn(&Lobby, u32, u32)) -> Vec<Vec<String>> {
    l.set_fake_mode("record");
    let (mut a, na) = player(l, "alice", "Alice");
    let (mut b, nb) = player(l, "bob", "Bob");
    set(l, na, nb);
    let no = open_room(&mut a, "Loadout room", 0);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);
    let _ = game_address(&mut a, T);
    let _ = game_address(&mut b, T);
    assert!(
        wait_until(T, || l.fake().first().is_some_and(|f| f.users().len() == 2)),
        "two user lines expected:\n{}",
        l.log_text()
    );
    l.fake()[0].users()
}

fn tok<'a>(line: &'a [String], key: &str) -> Option<&'a str> {
    line.iter()
        .skip(7)
        .find_map(|t| t.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

#[test]
fn user_lines_carry_the_equipped_weapon_slots_and_the_face() {
    let l = Lobby::start("ldo-tokens", opts(r#"{"inventory.default_repair": "off"}"#));
    let users = run_match(&l, |l, na, nb| {
        // slots deliberately inserted out of order; gear (4), a backpack weapon (10) and slot 14 travel too, the
        // unequipped item and a slot outside the 0x323 view (15) stay out
        set_equipment(
            l,
            na,
            &[
                (2, 333),
                (10, 125),
                (0, 1234),
                (4, 55),
                (3, 31),
                (14, 66),
                (15, 5),
            ],
            3,
        );
        set_equipment(l, nb, &[(1, 4321)], 0);
    });
    let (na, nb) = (l.user_no("alice"), l.user_no("bob"));
    let line_of = |n: u32| {
        users
            .iter()
            .find(|u| u[1] == n.to_string())
            .unwrap_or_else(|| panic!("no user line for {n}: {users:?}"))
    };
    let a = line_of(na);
    assert_eq!(a[..2], ["user".to_string(), na.to_string()]);
    assert_eq!(a[6], "Alice", "the nick stays the sixth field: {a:?}");
    assert_eq!(
        tok(a, "loadout"),
        Some("0:1234,2:333,3:31,4:55,10:125,14:66"),
        "{a:?}"
    );
    assert_eq!(tok(a, "face"), Some("3"), "{a:?}");
    // no class / skin data exists in the character record: those tokens are not invented
    assert_eq!(tok(a, "char"), None);
    assert_eq!(tok(a, "skin"), None);
    assert_eq!(
        tok(a, "mod"),
        None,
        "an ordinary account is not flagged: {a:?}"
    );
    let b = line_of(nb);
    assert_eq!(tok(b, "loadout"), Some("1:4321"), "{b:?}");
    assert_eq!(tok(b, "face"), Some("0"), "{b:?}");
}

#[test]
fn a_moderator_keeps_the_mod_token_next_to_the_loadout() {
    // CD-64: mod=1 stays
    let l = Lobby::start("ldo-mod", opts(r#"{"inventory.default_repair": "off"}"#));
    let users = run_match(&l, |l, na, nb| {
        set_equipment(l, na, &[(0, 11)], 1);
        set_equipment(l, nb, &[(0, 12)], 1);
        l.db()
            .execute("UPDATE accounts SET gm=1 WHERE user_no=?1", [na as i64])
            .unwrap();
    });
    let na = l.user_no("alice");
    let a = users.iter().find(|u| u[1] == na.to_string()).unwrap();
    assert_eq!(tok(a, "mod"), Some("1"), "{a:?}");
    assert_eq!(tok(a, "loadout"), Some("0:11"), "{a:?}");
}

#[test]
fn the_tokens_can_be_switched_off() {
    let l = Lobby::start(
        "ldo-off",
        opts(r#"{"inventory.default_repair": "off", "dserver.loadout_tokens": false}"#),
    );
    let users = run_match(&l, |l, na, nb| {
        set_equipment(l, na, &[(0, 11)], 1);
        set_equipment(l, nb, &[(0, 12)], 1);
    });
    for u in &users {
        // no loadout tokens; the badge tokens (DSV-141, `rank=` and the clan) are not part of the switch
        assert!(
            u[7..].iter().all(|t| !["loadout=", "face=", "parts="]
                .iter()
                .any(|k| t.starts_with(k))),
            "no loadout tokens expected: {u:?}"
        );
    }
}

#[test]
fn accounts_start_at_the_configured_number() {
    // DC-DEFAULT: AUTH-1 (proposed) auth.first_account_no (default 1001) (existing-DB behaviour: db.rs unit test)
    let l = Lobby::start("auth1-default", opts(r#"{"bots.pool_size": 0}"#));
    let (_a, na) = player(&l, "alice", "Alice");
    let (_b, nb) = player(&l, "bob", "Bob");
    assert_eq!((na, nb), (1001, 1002));
    let l = Lobby::start("auth1-custom", opts(r#"{"auth.first_account_no": 5000, "bots.pool_size": 0}"#));
    let (_a, na) = player(&l, "carol", "Carol");
    assert_eq!(na, 5000);
}

#[test]
fn start_loadout_reaches_the_user_line_of_every_new_account() {
    // DC-DEFAULT: LG-4 (proposed, weapon-sweep) --start-loadout seeds new characters (capture sweep, one weapon per
    // session); the seeded slots travel as loadout= tokens (§11)
    let mut o = opts("{}");
    o.extra = vec!["--start-loadout".into(), "2:333, 0:1234".into()];
    let l = Lobby::start("ldo-seed", o);
    let users = run_match(&l, |_, _, _| {});
    assert_eq!(users.len(), 2, "{users:?}");
    for u in &users {
        let lo = tok(u, "loadout").unwrap_or_else(|| panic!("no loadout token: {u:?}"));
        let pairs: Vec<&str> = lo.split(',').collect();
        assert!(pairs.contains(&"0:1234"), "{u:?}");
        assert!(pairs.contains(&"2:333"), "{u:?}");
        assert!(
            !pairs.iter().any(|p| p.starts_with("0:") && *p != "0:1234"),
            "one item per slot: {u:?}"
        );
    }
    assert!(
        l.log_text().contains("start loadout equipped"),
        "{}",
        l.log_text()
    );
}

#[test]
fn a_bad_start_loadout_is_a_usage_error() {
    let exe = lobby_exe();
    let out = std::process::Command::new(exe)
        .args(["--start-loadout", "0:x", "--port", "1"])
        .output()
        .expect("run the lobby");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("--start-loadout"));
}

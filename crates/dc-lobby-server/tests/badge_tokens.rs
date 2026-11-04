//! The `user` line the lobby writes to the game server carries the player's rank (`rank=<n>`, the rank of the
//! account's experience by the 2009 client's own rule over the level table) and, for a clan member, the clan number,
//! name and mark (`clan=`, `clanlabel=`, `mark=<emblem>/<background>`). Real lobby binary, stand-in game server,
//! accounts whose experience and clan the test writes into the lobby database before the start.
// SPEC: DSERVER_PIPE.md §3.1 (user line, optional trailing k=v tokens); GAME_MESSAGES_2009_S2C.md §3.2.1 fields 15-17
// EVIDENCE: cshell+0x267f10 (rank of an experience value: first row whose bound is above it, else 0)
// DC-DEFAULT: DSV-141 (proposed) badge tokens; CD-05 the fallback rank table without a ROM
// HARNESS: loadout_tokens.rs (same process-test style)

mod support;
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(20);

fn opts() -> LobbyOpts {
    LobbyOpts {
        dserver_exe: Some(fake_dserver_exe()),
        dserver_world: Some(r"Worlds\SnowValley".into()),
        lobby_json: Some("{}".into()),
        ..Default::default()
    }
}

/// The experience bounds the lobby uses: the ROM level table (`[LevelExp]` `Experience<n>`, read at run time) when
/// CA_ROM is set, else the DeCombat fallback (CD-05: cumulative round(100 * 1.18^(n-1)), 55 rows).
fn bounds() -> Vec<i64> {
    if let Some(dir) = dc_rom::test_rom() {
        let vfs = dc_bute::RezVfs::new(&dir);
        if let Some(sec) =
            dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", "ATTRIBUTES\\LEVELS.TXT").and_then(
                |d| {
                    let s = d.get("LevelExp")?;
                    let v: Vec<i64> = (0..)
                        .map_while(|i| s.get(&format!("Experience{i}")))
                        .map(|v| v.trim().parse().unwrap_or(0))
                        .collect();
                    (!v.is_empty()).then_some(v)
                },
            )
        {
            return sec;
        }
    }
    let mut total = 0i64;
    (1..=55)
        .map(|n: i32| {
            total += (100.0 * 1.18f64.powi(n - 1)).round() as i64;
            total
        })
        .collect()
}

fn tok<'a>(line: &'a [String], key: &str) -> Option<&'a str> {
    line.iter()
        .skip(7)
        .find_map(|t| t.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

#[test]
fn user_lines_carry_the_rank_of_the_experience_and_the_clan() {
    let b = bounds();
    assert!(b.len() >= 5, "level table too short: {b:?}");
    // exactly on the bound of row 3: the first bound above it is row 4
    let exp_a = b[3];
    let l = Lobby::start("badge-tokens", opts());
    l.set_fake_mode("record");
    let (mut a, na) = player(&l, "alice", "Alice");
    let (mut bb, nb) = player(&l, "bob", "Bob");
    {
        let db = l.db();
        db.execute(
            "UPDATE characters SET exp=?2 WHERE user_no=?1",
            [na as i64, exp_a],
        )
        .unwrap();
        db.execute(
            "INSERT INTO clans (clan_uid, name, master, emblem, background) VALUES (77, 'Night Owls', ?1, 5, 9)",
            [na as i64],
        )
        .unwrap();
        db.execute(
            "UPDATE characters SET clan_uid=77 WHERE user_no=?1",
            [na as i64],
        )
        .unwrap();
        db.execute("UPDATE characters SET exp=0 WHERE user_no=?1", [nb as i64])
            .unwrap();
    }
    let no = open_room(&mut a, "Badge room", 0);
    join_room(&mut bb, no);
    ready(&mut bb, 1);
    ready(&mut a, 2);
    let _ = game_address(&mut a, T);
    let _ = game_address(&mut bb, T);
    assert!(
        wait_until(T, || l.fake().first().is_some_and(|f| f.users().len() == 2)),
        "two user lines expected:\n{}",
        l.log_text()
    );
    let users = l.fake()[0].users();
    let line = |n: u32| {
        users
            .iter()
            .find(|u| u.get(1).map(String::as_str) == Some(&n.to_string()))
            .cloned()
            .unwrap_or_else(|| panic!("no user line for {n}: {users:?}"))
    };
    let (ua, ub) = (line(na), line(nb));
    assert_eq!(ua[5], exp_a.to_string(), "{ua:?}");
    assert_eq!(tok(&ua, "rank"), Some("4"), "{ua:?}");
    assert_eq!(tok(&ua, "clan"), Some("77"), "{ua:?}");
    assert_eq!(tok(&ua, "clanlabel"), Some("Night%20Owls"), "{ua:?}");
    assert_eq!(tok(&ua, "mark"), Some("5/9"), "{ua:?}");
    // no experience: rank 0 when the first bound is above 0; no clan tokens
    let want_b = if b[0] > 0 { "0" } else { "1" };
    assert_eq!(tok(&ub, "rank"), Some(want_b), "{ub:?}");
    assert_eq!(tok(&ub, "clan"), None, "{ub:?}");
    assert_eq!(tok(&ub, "clanlabel"), None, "{ub:?}");
    assert_eq!(tok(&ub, "mark"), None, "{ub:?}");
}

//! Loopback proof of the rank / clan path, lobby line to client reader: the `user` line in the form the lobby writes
//! (dc-lobby-server `game::badge_tokens`, rank of a known experience by the 2009 rule over a synthetic level table)
//! goes through the control parser, the roster mark and the `GS_PLAYER_INFO` 0x6b writer, and the bot client's 0x6b
//! reader (an independent implementation of §3.2.1) reads back the rank; a spec reader written here reads the clan
//! block. Session-level wiring (`Game::info_with`) is the integrator hook of w1-dserverrank.
// SPEC: GAME_MESSAGES_2009_S2C.md §3.2.1 fields 15-17; DSERVER_PIPE.md §3.1
// EVIDENCE: cshell+0x267f10 (rank of an experience value)
// DC-DEFAULT: DSV-141 (proposed)

use dc_dserver::gs::{self, Look, PlayerInfo, PlayerMark};
use dc_dserver::pipe::{parse_control, Control};

/// The 2009 rank rule over synthetic bounds (the lobby's `client_rank`; the ROM table is read at run time there).
fn rank_of(bounds: &[u32], exp: u32) -> u32 {
    bounds
        .iter()
        .take(255)
        .position(|b| exp < *b)
        .map_or(0, |i| i as u32)
}

fn info(pid: u8, uid: u32) -> PlayerInfo {
    PlayerInfo {
        name: b"Player".to_vec(),
        pid,
        account_no: uid,
        character: Some(40 + pid as u16),
        look: Look::default(),
        kills: 3,
        deaths: 1,
        team: 1 + pid % 2,
        dead: false,
        kind: gs::INFO_UPDATE,
        mode_m1: 0,
        mode_m2: 0,
        mission: [0; 3],
        mark: PlayerMark::default(),
    }
}

/// Clan block of a 0x6b body (after the shell id), read per §3.2.1: `(has_group, name, group_no, g1, g2, g3, h1,
/// h2)`.
fn read_marks(m: &dc_wire::udp::BitWriter) -> (bool, String, u64, [u32; 3], u32, u32) {
    let mut r = m.reader();
    assert_eq!(r.read_u8(), 0x0d);
    assert_eq!(r.read_u8(), 0x6b);
    r.read_zstr8();
    r.read_u8();
    r.read_u32();
    r.read_bit();
    r.read_u16();
    for _ in 0..15 {
        r.read_s8();
    }
    r.read_zstr8();
    r.read_zstr8();
    r.read_s16();
    r.read_u8();
    r.skip_bits(24);
    r.read_u8();
    r.read_bit();
    r.read_u8();
    let has = r.read_bit();
    let (mut name, mut no, mut g) = (String::new(), 0, [0; 3]);
    if has {
        name = String::from_utf16_lossy(&r.read_zstr16());
        no = r.read_u64();
        g = [r.read_u32(), r.read_u32(), r.read_u32()];
    }
    let (h1, h2) = (r.read_u32(), r.read_u32());
    (has, name, no, g, h1, h2)
}

#[test]
fn lobby_line_to_roster_record_carries_rank_and_clan() {
    let bounds = [100u32, 218, 357, 521, 715, 943];
    for (exp, clan) in [(0u32, false), (521, true), (944, false), (400, true)] {
        let rank = rank_of(&bounds, exp);
        let mut line = format!("user 1001 5 99 0 {exp} Player face=0 rank={rank}");
        if clan {
            line.push_str(" clan=77 clanlabel=Night%20Owls mark=5/9");
        }
        let e = match parse_control(&line) {
            Control::User(u) => u,
            other => panic!("{other:?}"),
        };
        let mark = PlayerMark::of_roster(Some(&e));
        let m = gs::player_info_marked(&info(0, 1001), &mark);
        // the bot client's own reader
        let mut r = m.reader();
        r.read_u8();
        r.read_u8();
        let rec = dc_botclient::shell::read_player_info(&mut r);
        assert!(rec.complete, "{rec:?}");
        assert_eq!(rec.rank, rank, "exp {exp}");
        assert_eq!((rec.pid, rec.account_no, rec.kills), (0, 1001, 3));
        // the spec reader
        let (has, name, no, g, h1, h2) = read_marks(&m);
        assert_eq!(h2, rank);
        assert_eq!(h1, 0, "name colour: default");
        assert_eq!(has, clan);
        if clan {
            assert_eq!((name.as_str(), no, g), ("Night Owls", 77, [5, 9, 0]));
        }
    }
    // the known values of the table: exactly on a bound is the next row, past the last row is 0
    assert_eq!(rank_of(&bounds, 521), 4);
    assert_eq!(rank_of(&bounds, 944), 0);
}

//! LG-420 flows: clan applications (apply -> leader accept / decline) and invitations (invite -> yes / no), the mutual
//! case, the leader-only and member-limit refusals and the invitation's re-send at the next login. Fake lobby
//! sessions, the web routes of `clanweb.rs` and the relay commands of `messenger.rs`.

use super::*;
use crate::clanweb::{parse_request, route};
use crate::logs::Listener;
use crate::messenger::MESSENGER_PUSH;
use crate::msg::{CHAT_PUSH, CHAT_SEND};
use crate::net;
use crate::state::testing::*;
use crate::state::{Core, Outgoing};
use dc_wire::{PktReader, PktWriter};
use std::sync::mpsc::Receiver;

struct Client {
    sid: Sid,
    rx: Receiver<Outgoing>,
    got: Vec<(u16, Vec<u8>)>,
}

impl Client {
    fn pump(&mut self) {
        while let Ok(o) = self.rx.try_recv() {
            if let Outgoing::Frame(f) = o {
                self.got
                    .push((u16::from_le_bytes([f[0], f[1]]), f[8..].to_vec()));
            }
        }
    }

    fn clear(&mut self) {
        self.pump();
        self.got.clear();
    }

    /// The 0x3051 lines from the server since the last call.
    fn told(&mut self) -> Vec<String> {
        self.pump();
        let out = self
            .got
            .iter()
            .filter(|(id, _)| *id == CHAT_PUSH)
            .map(|(_, p)| {
                let n = u32_at(p, 0x34) as usize;
                PktReader::new(p).at(0x38).wstr(n)
            })
            .collect();
        self.got.retain(|(id, _)| *id != CHAT_PUSH);
        out
    }

    /// The 0x3f01 relay lines since the last call (fields).
    fn lines(&mut self) -> Vec<Vec<String>> {
        self.pump();
        let out = self
            .got
            .iter()
            .filter(|(id, _)| *id == MESSENGER_PUSH)
            .map(|(_, d)| {
                PktReader::new(d)
                    .at(6)
                    .wstr((d.len() - 6) / 2)
                    .split('\t')
                    .map(|s| s.to_string())
                    .collect()
            })
            .collect();
        self.got.retain(|(id, _)| *id != MESSENGER_PUSH);
        out
    }

    fn chat(&mut self, st: &mut Core, text: &str) {
        let mut w = PktWriter::empty();
        w.wstr(text, 121);
        let mut f = PktWriter::new(CHAT_SEND);
        f.bytes(w.raw_bytes());
        let p = f.into_payload();
        st.scope(|st| net::route(st, self.sid, CHAT_SEND as i32, &p))
            .unwrap();
        self.pump();
    }
}

/// An account with a character, logged into the lobby on `sid` (login = the name in lower case).
fn player(st: &mut Core, sid: Sid, nick: &str) -> (Client, u32) {
    let acc = crate::db::create_account(&st.db, &nick.to_lowercase(), 0).unwrap();
    st.scope(|st| crate::auth::create_character(st, acc, nick, 2))
        .unwrap();
    let rx = connect(st, sid, Listener::Lobby);
    {
        let s = st.sessions.get_mut(&sid).unwrap();
        s.account = Some(acc);
        s.in_lobby = true;
        s.channel = Some(1);
    }
    (
        Client {
            sid,
            rx,
            got: Vec::new(),
        },
        acc,
    )
}

fn get(st: &mut Core, target: &str) -> String {
    let req = parse_request(&format!("GET {} HTTP/1.1\r\nHost: x\r\n\r\n", target)).unwrap();
    let mut out = String::new();
    st.scope(|st| {
        out = route(st, &req)?.1;
        Ok(())
    })
    .unwrap();
    out
}

fn clan_no(st: &Core, a: u32) -> i64 {
    st.character(a).unwrap().clan_uid
}

fn open_rows(st: &Core) -> i64 {
    st.db
        .query_row("SELECT COUNT(*) FROM clan_requests", [], |r| r.get(0))
        .unwrap()
}

/// Carol leads Wolves; returns the clan number.
fn wolves(st: &mut Core, carol: &mut Client) -> i64 {
    get(st, "/clan/create?p=carol&name=Wolves&fmt=text");
    carol.clear();
    clan::clan_by_name(&st.db, "Wolves").unwrap().unwrap().uid
}

/// The owner report "join through clan search without them accepting you": the find page's join is an application;
/// the leader hears of it, sees the applicant on the manage page and accepts; the player hears "You joined".
#[test]
fn apply_then_the_leader_accepts() {
    let mut st = state();
    let (mut carol, _) = player(&mut st, 1, "Carol");
    let (mut dan, d) = player(&mut st, 2, "Dan");
    let k = wolves(&mut st, &mut carol);
    let body = get(&mut st, &format!("/clan/join?p=dan&c={k}&fmt=text"));
    assert!(
        body.starts_with("me\t0\t0\nmsg\t1\tApplication sent to Wolves.\n"),
        "{body}"
    );
    assert_eq!(clan_no(&st, d), 0, "not a member before the leader answers");
    assert_eq!(
        carol.told(),
        ["Dan applied to join Wolves. Open Clan Setting to accept or decline."]
    );
    // applying again keeps one row and tells the leader nothing new
    get(&mut st, &format!("/clan/join?p=dan&c={k}&fmt=text"));
    assert_eq!(open_rows(&st), 1);
    assert!(carol.told().is_empty());
    // the leader's page lists the applicant; a member's page would not
    let body = get(&mut st, "/clan?url=manager/guildmember&p=carol&fmt=text");
    assert!(body.contains(&format!("applicant\t{d}\tDan\t")), "{body}");
    let html = get(&mut st, "/clan?url=manager/guildmember&p=carol");
    assert!(
        html.contains(&format!("/clan/accept?p=carol&amp;a={d}"))
            && html.contains(&format!("/clan/decline?p=carol&amp;a={d}"))
            && html.contains("action=\"/clan/invite\""),
        "{html}"
    );
    let body = get(&mut st, &format!("/clan/accept?p=carol&a={d}&fmt=text"));
    assert!(
        body.contains("msg\t1\tDan joined the clan.\n")
            && body.contains(&format!("member\t{d}\t0\tDan\t"))
            && !body.contains("applicant\t"),
        "{body}"
    );
    assert_eq!(clan_no(&st, d), k);
    assert_eq!(open_rows(&st), 0);
    assert!(dan.told().contains(&"You joined Wolves.".to_string()));
    // a second accept finds no application
    let body = get(&mut st, &format!("/clan/accept?p=carol&a={d}&fmt=text"));
    assert!(body.contains("msg\t0\t"), "{body}");
}

#[test]
fn apply_then_the_leader_declines() {
    let mut st = state();
    let (mut carol, _) = player(&mut st, 1, "Carol");
    let (mut dan, d) = player(&mut st, 2, "Dan");
    let k = wolves(&mut st, &mut carol);
    // the chat command applies too
    dan.chat(&mut st, "!clan join wolves");
    assert_eq!(dan.told(), ["Application sent to Wolves."]);
    let body = get(&mut st, &format!("/clan/decline?p=carol&a={d}&fmt=text"));
    assert!(
        body.contains("msg\t1\tThe application of Dan was declined.\n")
            && !body.contains("applicant\t"),
        "{body}"
    );
    assert_eq!(clan_no(&st, d), 0);
    assert_eq!(open_rows(&st), 0);
    assert_eq!(dan.told(), ["Your application to Wolves was declined."]);
    // a new application; the leader's chat command accepts it
    dan.chat(&mut st, "!clan join Wolves");
    carol.clear();
    carol.chat(&mut st, "!clan accept dan");
    assert_eq!(carol.told(), ["Dan joined the clan."]);
    assert_eq!(clan_no(&st, d), k);
}

/// Only the leader answers or invites; a plain member is refused and nothing changes.
#[test]
fn a_member_cannot_accept_decline_or_invite() {
    let mut st = state();
    let (mut carol, _) = player(&mut st, 1, "Carol");
    let (_dan, d) = player(&mut st, 2, "Dan");
    let (_eve, e) = player(&mut st, 3, "Eve");
    let k = wolves(&mut st, &mut carol);
    get(&mut st, &format!("/clan/join?p=dan&c={k}&fmt=text"));
    get(&mut st, &format!("/clan/accept?p=carol&a={d}&fmt=text"));
    get(&mut st, &format!("/clan/join?p=eve&c={k}&fmt=text"));
    for t in [
        format!("/clan/accept?p=dan&a={e}&fmt=text"),
        format!("/clan/decline?p=dan&a={e}&fmt=text"),
        "/clan/invite?p=dan&n=Eve&fmt=text".to_string(),
    ] {
        let body = get(&mut st, &t);
        assert!(
            body.contains("msg\t0\t") && body.contains("only the clan leader may do that"),
            "{t}: {body}"
        );
    }
    let body = get(&mut st, "/clan?url=manager/guildmember&p=dan&fmt=text");
    assert!(
        !body.contains("applicant\t"),
        "a member sees no applications: {body}"
    );
    assert_eq!(clan_no(&st, e), 0);
    assert_eq!(open_rows(&st), 1, "Eve's application stays open");
    // LG-416 covers the new actions as well
    let head =
        "GET /clan/accept?p=carol&a=3 HTTP/1.1\r\nHost: x\r\nSec-Fetch-Site: cross-site\r\n\r\n";
    let req = parse_request(head).unwrap();
    let mut code = 0;
    st.scope(|st| {
        code = crate::clanweb::route_checked(st, &req, head, "127.0.0.1:1")?.0;
        Ok(())
    })
    .unwrap();
    assert_eq!(code, 403);
    assert_eq!(clan_no(&st, e), 0);
}

/// A full clan (CLN-6) refuses the application, and an accept that would pass the limit leaves the row open.
#[test]
fn a_full_clan_refuses_applications_and_accepts() {
    let mut st = state();
    let (mut carol, _) = player(&mut st, 1, "Carol");
    let (_dan, d) = player(&mut st, 2, "Dan");
    let (_eve, e) = player(&mut st, 3, "Eve");
    let k = wolves(&mut st, &mut carol);
    get(&mut st, &format!("/clan/join?p=dan&c={k}&fmt=text"));
    get(&mut st, &format!("/clan/join?p=eve&c={k}&fmt=text"));
    st.rules.clan.member_limits = vec![2];
    get(&mut st, &format!("/clan/accept?p=carol&a={d}&fmt=text"));
    assert_eq!(clan_no(&st, d), k);
    let body = get(&mut st, &format!("/clan/accept?p=carol&a={e}&fmt=text"));
    assert!(
        body.contains("msg\t0\tCannot accept: the clan is full."),
        "{body}"
    );
    assert_eq!(clan_no(&st, e), 0);
    assert_eq!(open_rows(&st), 1, "the application waits for a free place");
    let (_finn, _) = player(&mut st, 4, "Finn");
    let body = get(&mut st, &format!("/clan/join?p=finn&c={k}&fmt=text"));
    assert!(
        body.contains("msg\t0\tCannot join: the clan is full."),
        "{body}"
    );
    let body = get(&mut st, "/clan/invite?p=carol&n=Finn&fmt=text");
    assert!(
        body.contains("msg\t0\tCannot invite: the clan is full."),
        "{body}"
    );
    assert_eq!(open_rows(&st), 1);
}

/// The owner request "invite someone to clan": the leader invites by name, the online player gets the relay
/// question, Yes joins; refusals for an unknown name and a player in a clan.
#[test]
fn invite_then_yes_joins() {
    let mut st = state();
    let (mut carol, _) = player(&mut st, 1, "Carol");
    let (mut dan, d) = player(&mut st, 2, "Dan");
    let k = wolves(&mut st, &mut carol);
    let body = get(&mut st, "/clan/invite?p=carol&n=dan&fmt=text");
    assert!(
        body.contains("msg\t1\tInvitation sent to Dan.\n") && body.contains("member\t"),
        "{body}"
    );
    assert_eq!(
        dan.lines(),
        [vec![
            "claninvite".to_string(),
            "Wolves".into(),
            k.to_string()
        ]]
    );
    assert_eq!(clan_no(&st, d), 0);
    dan.chat(&mut st, &format!("!nm clanyes\t{k}"));
    assert_eq!(clan_no(&st, d), k);
    assert_eq!(dan.told(), ["You joined Wolves."]);
    assert_eq!(carol.told(), ["Dan joined Wolves."]);
    assert_eq!(open_rows(&st), 0);
    // a second yes has nothing to answer
    dan.chat(&mut st, &format!("!nm clanyes\t{k}"));
    assert_eq!(
        dan.told(),
        ["Cannot answer the invitation: no open invitation from that clan."]
    );
    let body = get(&mut st, "/clan/invite?p=carol&n=Nobody&fmt=text");
    assert!(body.contains("Cannot invite: no such player."), "{body}");
    let body = get(&mut st, "/clan/invite?p=carol&n=Dan&fmt=text");
    assert!(
        body.contains("Cannot invite: that player is already in a clan."),
        "{body}"
    );
    // the chat command invites too
    let (mut eve, _) = player(&mut st, 3, "Eve");
    carol.chat(&mut st, "!clan invite Eve");
    assert_eq!(carol.told(), ["Invitation sent to Eve."]);
    assert_eq!(eve.lines()[0][0], "claninvite");
}

#[test]
fn invite_then_no_removes_it() {
    let mut st = state();
    let (mut carol, _) = player(&mut st, 1, "Carol");
    let (mut dan, d) = player(&mut st, 2, "Dan");
    let k = wolves(&mut st, &mut carol);
    get(&mut st, "/clan/invite?p=carol&n=Dan&fmt=text");
    dan.clear();
    dan.chat(&mut st, &format!("!nm clanno\t{k}"));
    assert_eq!(clan_no(&st, d), 0);
    assert_eq!(open_rows(&st), 0);
    assert_eq!(dan.told(), ["You declined the invitation to Wolves."]);
    assert_eq!(carol.told(), ["Dan declined the invitation to Wolves."]);
    // nothing asks again at the next login
    st.scope(|st| crate::gm::on_lobby_login(st, 2, d)).unwrap();
    assert!(!dan.lines().iter().any(|l| l[0] == "claninvite"));
}

/// Both sides asked: an application after an invitation joins at once, an invitation after an application too.
#[test]
fn mutual_requests_join_at_once() {
    let mut st = state();
    let (mut carol, _) = player(&mut st, 1, "Carol");
    let (mut dan, d) = player(&mut st, 2, "Dan");
    let (mut eve, e) = player(&mut st, 3, "Eve");
    let k = wolves(&mut st, &mut carol);
    get(&mut st, "/clan/invite?p=carol&n=Dan&fmt=text");
    let body = get(&mut st, &format!("/clan/join?p=dan&c={k}&fmt=text"));
    assert!(body.contains("msg\t1\tYou joined Wolves.\n"), "{body}");
    assert_eq!(clan_no(&st, d), k);
    assert!(dan.told().contains(&"You joined Wolves.".to_string()));
    get(&mut st, &format!("/clan/join?p=eve&c={k}&fmt=text"));
    carol.clear();
    let body = get(&mut st, "/clan/invite?p=carol&n=Eve&fmt=text");
    assert!(body.contains("msg\t1\tEve joined Wolves.\n"), "{body}");
    assert_eq!(clan_no(&st, e), k);
    assert!(eve.told().contains(&"You joined Wolves.".to_string()));
    assert!(eve.lines().is_empty(), "no question once joined");
    assert_eq!(open_rows(&st), 0);
}

/// An invitation to an offline player waits: the next login asks again (as open friend requests do); the leader
/// hears of waiting applications at login.
#[test]
fn a_login_sends_the_open_invitation_again() {
    let mut st = state();
    let (mut carol, c) = player(&mut st, 1, "Carol");
    let (mut dan, d) = player(&mut st, 2, "Dan");
    let k = wolves(&mut st, &mut carol);
    // Dan offline when the invitation is made
    st.sessions.get_mut(&2).unwrap().in_lobby = false;
    get(&mut st, "/clan/invite?p=carol&n=Dan&fmt=text");
    st.sessions.get_mut(&2).unwrap().in_lobby = true;
    assert!(dan.lines().is_empty(), "nothing reached the offline player");
    st.scope(|st| crate::gm::on_lobby_login(st, 2, d)).unwrap();
    assert!(
        dan.lines().contains(&vec![
            "claninvite".to_string(),
            "Wolves".into(),
            k.to_string()
        ]),
        "the login asks again"
    );
    // another login before the answer asks once more; the answer ends it
    st.scope(|st| crate::gm::on_lobby_login(st, 2, d)).unwrap();
    assert!(dan.lines().iter().any(|l| l[0] == "claninvite"));
    dan.chat(&mut st, &format!("!nm clanyes\t{k}"));
    assert_eq!(clan_no(&st, d), k);
    // the leader's login counts the open applications
    let (_eve, _) = player(&mut st, 3, "Eve");
    get(&mut st, &format!("/clan/join?p=eve&c={k}&fmt=text"));
    carol.clear();
    st.scope(|st| crate::gm::on_lobby_login(st, 1, c)).unwrap();
    assert!(carol
        .told()
        .contains(&"1 open application to Wolves: open Clan Setting to answer.".to_string()));
}

/// `clan.join_approval` off keeps the join of before (at once).
#[test]
fn join_approval_off_joins_at_once() {
    let mut st = state();
    let (mut carol, _) = player(&mut st, 1, "Carol");
    let (_dan, d) = player(&mut st, 2, "Dan");
    let k = wolves(&mut st, &mut carol);
    assert!(st
        .rules
        .clan
        .apply("clan.join_approval", &dc_mod_lua::JsonValue::Bool(false)));
    let body = get(&mut st, &format!("/clan/join?p=dan&c={k}&fmt=text"));
    assert!(body.contains("msg\t1\tYou joined Wolves.\n"), "{body}");
    assert_eq!(clan_no(&st, d), k);
}

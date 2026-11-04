//! Modded servers in the post-login server list (LG-272, CLI-1622, CLI-1623; project decision: "mods come
//! after login and appear in the server list").
//!
//! The 2009 server-select screen lists the auth server's own entries (0x2000). Under them DeCombat lists the other
//! lobbies: the ones the login's lobby names in its own push (0x3f02, `[[lobby.servers]]` entries with
//! `lobby = "host:port"`) and the modded servers of the community document, merged by address (the lobby's entry
//! first, the document fills in its mods). Each row shows a MODDED badge, its name and region; the mods with their
//! Verified / Accepted / Unverified marks show in the panel on the right (the Event / Notice area), which also holds
//! the Mods button. No row and no panel line shows an address (CLI-1623): the address stays in the data.
//!
//! Picking a modded row leaves the lobby logged into, pins the row's certificate and set, and logs into that lobby
//! with the cached identity token (no password: the identity authority is still the first lobby's), so the mod
//! step (set read, consent prompt, download, load) runs as for any join; its server list then picks its first
//! entry by itself and the channel list opens. Leaving it (the server list again) unloads its mods and logs into the
//! first lobby again with the token. Without the identity (`--identity off`) the login screen asks for the password.
// DC-DEFAULT: LG-272 / CLI-1622 / CLI-1623 (proposed, project decision), see DC_DEFAULTS.md

use std::net::SocketAddr;

use super::catalog::Catalog;
use super::link::LinkCmd;
use super::modbrowser::{
    fit, server_marks, text, wrap, Button, Buttons, Mark, BUTTON, CMD_MODS, GOLD, T_MODDED,
    T_MODS, T_VANILLA, BLUE,
};
use super::popup::{BoxTag, Popup};
use super::usebox::{fill, measure, FONT_SIZE, MESSAGE_COLOUR, TEXT_COLOUR, TITLE_COLOUR};
use super::{Ctx, Go, MenuData};
use crate::community;
use crate::hud::items::DrawItem;
use crate::mods::ServerModSet;

/// The panel on the right (inside the 2009 Event / Notice frame of the server screen).
pub const PANEL: [i32; 4] = [592, 182, 386, 276];
const LINE: i32 = FONT_SIZE + 4;
/// Rows the panel shows (two lines each).
pub const PANEL_ROWS: usize = 5;

pub const T_PANEL_TITLE: &str = "Modded servers";
pub const T_PANEL_NOTE: &str = "A modded server's mods load when you join it and unload when you leave it.";
pub const T_PANEL_NONE: &str = "No modded server is listed.";
pub const T_NO_ADDRESS: &str = "This server cannot be reached: its address did not resolve.";

/// Where the player was before joining a lobby of the list (restored when leaving it).
#[derive(Debug, Clone, PartialEq)]
pub struct Home {
    pub auth: Option<SocketAddr>,
    pub pin: Option<String>,
    pub picked: Option<String>,
}

/// One other lobby of the server list.
#[derive(Debug, Clone, PartialEq)]
pub struct ModdedRow {
    pub name: String,
    /// Shown ("" = none).
    pub region: String,
    pub modded: bool,
    /// Data only: never drawn (CLI-1623).
    pub addr: Option<SocketAddr>,
    pub fingerprint: Option<String>,
    /// The community document's set of this server (`None`: read from the lobby at the join).
    pub expect: Option<ServerModSet>,
    /// (mod name, mark) from the community document; empty when the document does not list the server.
    pub mods: Vec<(String, Mark)>,
}

/// A community region code in words.
pub fn region_words(code: &str) -> String {
    match code {
        "na" => "North America",
        "sa" => "South America",
        "eu" => "Europe",
        "as" => "Asia",
        "oc" => "Oceania",
        "af" => "Africa",
        "lan" => "LAN",
        other => other,
    }
    .to_string()
}

fn non_empty(s: &str) -> Option<String> {
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The other lobbies of the list: the login lobby's own entries (0x3f02) first, then the community document's
/// modded servers whose address is not among them; a document entry with the same address fills in the mods, the
/// set and a missing pin. None while the player is in a lobby of the list (leaving it shows them again).
pub fn modded_rows(data: &MenuData) -> Vec<ModdedRow> {
    if data.modded_join.is_some() {
        return Vec::new();
    }
    let view = data.community.view();
    let doc = view.and_then(|v| v.doc.as_ref());
    let doc_entry = |addr: Option<SocketAddr>, address: &str| {
        let doc = doc?;
        let i = doc.servers.iter().enumerate().position(|(i, s)| {
            let a = view.and_then(|v| v.addrs.get(i).copied().flatten());
            (addr.is_some() && a == addr) || s.address.eq_ignore_ascii_case(address)
        })?;
        Some((doc, &doc.servers[i]))
    };
    let mut rows: Vec<ModdedRow> = data
        .other_lobbies
        .iter()
        .map(|o| {
            let l = &o.listed;
            let mut r = ModdedRow {
                name: l.name.clone(),
                region: l.region.clone(),
                modded: l.policy != "vanilla",
                addr: o.addr,
                fingerprint: non_empty(&l.fingerprint),
                expect: None,
                mods: Vec::new(),
            };
            if let Some((doc, s)) = doc_entry(o.addr, &l.lobby) {
                r.mods = server_marks(data, doc, s);
                r.expect = Some(community::expected_set(s));
                if r.fingerprint.is_none() {
                    r.fingerprint = s.fingerprint.clone();
                }
                if r.region.is_empty() {
                    r.region = region_words(&s.region);
                }
            }
            r
        })
        .collect();
    if let Some(doc) = doc {
        for (i, s) in doc.servers.iter().enumerate() {
            if s.policy != "modded" {
                continue;
            }
            let addr = view.and_then(|v| v.addrs.get(i).copied().flatten());
            let listed = data.other_lobbies.iter().any(|o| {
                (addr.is_some() && o.addr == addr) || o.listed.lobby.eq_ignore_ascii_case(&s.address)
            });
            if listed || addr.is_some_and(|a| data.lobby == Some(a)) {
                continue;
            }
            rows.push(ModdedRow {
                name: s.name.clone(),
                region: region_words(&s.region),
                modded: true,
                addr,
                fingerprint: s.fingerprint.clone(),
                expect: Some(community::expected_set(s)),
                mods: server_marks(data, doc, s),
            });
        }
    }
    // CLI-1632: what was read of the row itself when the document says nothing (never the expected set: the join
    // reads and pins the set itself)
    for r in rows.iter_mut().filter(|r| r.mods.is_empty()) {
        r.mods = known_marks(data, doc, r.addr);
    }
    rows
}

/// The mods and marks of the set known for `addr` (none: not known).
fn known_marks(
    data: &MenuData,
    doc: Option<&dc_config::directory::Community>,
    addr: Option<SocketAddr>,
) -> Vec<(String, Mark)> {
    let Some(k) = data.known_sets.iter().find(|k| Some(k.addr) == addr) else {
        return Vec::new();
    };
    k.set
        .mods
        .iter()
        .map(|m| {
            let key = m.key();
            let name = doc
                .and_then(|d| community::known(d, &key))
                .map(|x| x.name.clone())
                .or_else(|| m.name.clone())
                .unwrap_or_else(|| m.id.clone());
            let mark = if data.settings.mods_accepted.contains(&key) {
                Mark::Accepted
            } else {
                Mark::Unverified
            };
            (name, mark)
        })
        .collect()
}

/// The row's name as the list shows it: the name and the region.
pub fn row_label(r: &ModdedRow) -> String {
    if r.region.is_empty() {
        r.name.clone()
    } else {
        format!("{} ({})", r.name, r.region)
    }
}

/// Picks modded row `r` from the server list (CLI-1622): leave the lobby logged into, pin the row, and log into it
/// with the identity token (the login screen runs the join and its prompts); without the identity the login screen
/// asks for the password.
pub fn join(r: &ModdedRow, ctx: &mut Ctx) {
    let Some(addr) = r.addr else {
        ctx.popups.push(Popup::ok(T_NO_ADDRESS.into(), BoxTag::None));
        return;
    };
    let d = &mut *ctx.data;
    if d.home.is_none() {
        d.home = Some(Home {
            auth: d.auth,
            pin: dc_botclient::tls::current().pin,
            picked: d.picked_server.clone(),
        });
    }
    // the row's certificate pin, else none (trust on first use, CLI-1470): every instance has its own certificate,
    // so the run's own pin (the first lobby's) would refuse it
    match r.fingerprint.clone() {
        Some(p) => {
            if let Err(e) = dc_botclient::tls::set_pin(&p) {
                eprintln!("dc-client server list: {e}");
            }
        }
        None => dc_botclient::tls::clear_pin(),
    }
    d.auth = Some(addr);
    d.picked_server = Some(r.name.clone());
    d.modded_join = Some(r.name.clone());
    d.auto_pick = true;
    d.lobby_up = false;
    d.xray_offered = false;
    d.server = None;
    d.servers.clear();
    ctx.send(LinkCmd::Leave);
    ctx.send(LinkCmd::ExpectModSet(r.expect.clone()));
    ctx.data.auto_login = ctx.data.identity_web.is_some();
    *ctx.go = Some(Go::Login);
}

/// Restores the lobby the player came from (its address, pin and name); `None` when no join of the list is on.
pub fn restore_home(ctx: &mut Ctx) -> bool {
    let d = &mut *ctx.data;
    if d.modded_join.take().is_none() {
        return false;
    }
    d.auto_pick = false;
    if let Some(h) = d.home.take() {
        d.auth = h.auth;
        d.picked_server = h.picked;
        match h.pin {
            Some(p) => {
                let _ = dc_botclient::tls::set_pin(&p);
            }
            None => dc_botclient::tls::clear_pin(),
        }
    }
    ctx.send(LinkCmd::ExpectModSet(None));
    true
}

/// Leaves the lobby of the list for the one the player came from (CLI-1622): logout, mods unloaded, and the login
/// there again with the token (the password box without the identity).
pub fn go_home(ctx: &mut Ctx) {
    if !restore_home(ctx) {
        return;
    }
    let d = &mut *ctx.data;
    d.lobby_up = false;
    d.xray_offered = false;
    d.server = None;
    d.servers.clear();
    ctx.send(LinkCmd::Leave);
    ctx.data.auto_login = ctx.data.identity_web.is_some();
    *ctx.go = Some(Go::Login);
}

/// The Mods button of the server screen (bottom right of the panel).
pub fn mods_button() -> Buttons {
    let mut b = Buttons::default();
    b.list = vec![Button {
            cmd: CMD_MODS,
            at: [
                PANEL[0] + PANEL[2] - BUTTON[0] - 4,
                PANEL[1] + PANEL[3] - BUTTON[1] - 4,
            ],
            size: BUTTON,
            label: T_MODS.into(),
            on: true,
        kind: super::modbrowser::ButtonKind::Art,
        }];
    b
}

/// The panel: title, note, every row with its badge, name, region and mods; `selected` is the row picked in the
/// list (highlighted).
pub fn draw_panel(
    cat: &mut Catalog,
    rows: &[ModdedRow],
    selected: Option<usize>,
    buttons: &Buttons,
    out: &mut Vec<DrawItem>,
) {
    let [x, y, w, _] = PANEL;
    out.push(text(T_PANEL_TITLE, [x, y], TITLE_COLOUR));
    let mut ly = y + LINE + 2;
    for l in wrap(cat, T_PANEL_NOTE, w).iter().take(2) {
        out.push(text(l, [x, ly], MESSAGE_COLOUR));
        ly += LINE;
    }
    ly += 4;
    if rows.is_empty() {
        out.push(text(T_PANEL_NONE, [x, ly], TEXT_COLOUR));
    }
    // the rows from the selected one on when it would not show
    let first = selected
        .filter(|s| *s >= PANEL_ROWS)
        .map_or(0, |s| s + 1 - PANEL_ROWS);
    for (i, r) in rows.iter().enumerate().skip(first).take(PANEL_ROWS) {
        let h = 2 * LINE + 4;
        if selected == Some(i) {
            out.push(fill([x - 4, ly - 2], [w + 8, h], [70, 60, 30, 200]));
        }
        let (badge, bc) = if r.modded {
            (T_MODDED, GOLD)
        } else {
            (T_VANILLA, BLUE)
        };
        out.push(text(badge, [x, ly], bc));
        let nx = x + measure(cat, badge) + 10;
        let label = fit(cat, &row_label(r), x + w - nx);
        out.push(text(&label, [nx, ly], TEXT_COLOUR));
        let my = ly + LINE;
        if r.mods.is_empty() {
            let t = if r.modded {
                "Its mods show when you join it."
            } else {
                "Runs no mods."
            };
            out.push(text(t, [nx, my], MESSAGE_COLOUR));
        }
        let mut mx = nx;
        let end = x + w;
        for (j, (m, mark)) in r.mods.iter().enumerate() {
            let tag = format!(" [{}]", mark.text());
            let lw = measure(cat, &format!("{m}{tag}"));
            if mx + lw > end {
                out.push(text(&format!("+{} more", r.mods.len() - j), [mx, my], MESSAGE_COLOUR));
                break;
            }
            let mw = measure(cat, m);
            out.push(text(m, [mx, my], TEXT_COLOUR));
            out.push(text(&tag, [mx + mw, my], mark.colour()));
            mx += lw + 12;
        }
        ly += h + 2;
    }
    buttons.draw(cat, out);
}

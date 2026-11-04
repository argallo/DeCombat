//! client-scoreboard: the 2009 scoreboard rows and the match result with every player.
//!
//! - The in-game board (Tab, the death board, the result board) from the user's ROM: place numbers right-aligned,
//!   the level icon of each row, kills and deaths centred on their points, the own row under the translucent bar.
//! - A loopback match through dc-lobby-server with the scripted menu host and three dc-botclient players: the bots
//!   leave 3 s after the result screen 0x3f (their default), the client about 6 s after it, so the server's 0x6c
//!   departures reach the client before its result screen opens (the user's round had only the own row then).
//!   The result screen lists all four players with place numbers and level icons; MY SCORE shows the totals after
//!   the match with the earned amounts, and the totals equal the lobby's own progress push (before + earned).
//!
//! ROM-gated parts are skipped without `CA_ROM`; nothing of the ROM is asserted by value. PNGs (ROM-derived, never
//! committed) go to `reports/client-scoreboard/` when that folder's parent exists.
// SPEC: PROTOCOL_2009.md §3 (0x3004), §7; GAME_MESSAGES_2009_S2C.md §3.2.1 (0x6b), §3.4 (0x65)
// EVIDENCE: cshell+0xc0ac0 / cshell+0xc3180 (board rows), cshell+0x137720 / cshell+0x136410 (result rows, totals)

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dc_botclient::capdecode::{decode_file, unbundle};
use dc_botclient::lobby::LobbyProfile;
use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::{run_bot, BotConfig, RoomChoice};
use dc_botclient::shell::RosterRecord;
use dc_client::hud::state::HudEvent;
use dc_client::hud::{DrawItem, Hud, HudState};
use dc_client::matchexit::MatchResult;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::pilot::{
    headless_runner, run as run_pilot, GameBack, Pilot, PilotPlan, PilotRoom,
};
use dc_client::menu::result::{grouped, summary_texts};
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_client::overlay::text::RgbaImage;
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{Connect, HeadlessOptions, NetOptions, WorldFile};
use support::*;

fn out_dir() -> PathBuf {
    let o = dc_render::default_out_dir().join("client-scoreboard");
    let _ = std::fs::create_dir_all(&o);
    o
}

fn rom() -> Option<PathBuf> {
    let r = dc_rom::test_rom();
    if r.is_none() {
        println!("skipped: CA_ROM unset");
    }
    r
}

/// Saves an image over a dark grey backdrop (straight alpha).
fn save(img: &RgbaImage, name: &str) {
    let mut rgb = Vec::with_capacity((img.width * img.height * 3) as usize);
    for px in img.rgba.as_chunks::<4>().0 {
        let a = px[3] as u32;
        for c in &px[..3] {
            rgb.push(((*c as u32 * a + 48 * (255 - a)) / 255) as u8);
        }
    }
    if let Some(b) = image::RgbImage::from_raw(img.width, img.height, rgb) {
        let p = out_dir().join(format!("{name}.png"));
        let _ = b.save(&p);
        println!("png: {}", p.display());
    }
}

fn rec(pid: u8, name: &str, team: u8, kills: i16, deaths: u8) -> RosterRecord {
    RosterRecord {
        name: name.into(),
        pid,
        team,
        kills,
        deaths,
        complete: true,
        ..RosterRecord::default()
    }
}

/// Opaque pixels in the virtual rectangle `r` (1024 x 768 virtual screen).
fn opaque_in(img: &RgbaImage, r: [f32; 4]) -> usize {
    let (sx, sy) = (img.width as f32 / 1024.0, img.height as f32 / 768.0);
    let x0 = (r[0] * sx).floor().max(0.0) as u32;
    let y0 = (r[1] * sy).floor().max(0.0) as u32;
    let x1 = ((r[0] + r[2]) * sx).ceil().min(img.width as f32) as u32;
    let y1 = ((r[1] + r[3]) * sy).ceil().min(img.height as f32) as u32;
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| img.pixel(x, y)[3] > 100)
        .count()
}

#[test]
fn rom_scoreboard_rows_have_place_numbers_level_icons_and_centred_counts() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD");
    let cols = hud.layout.board_columns.clone();
    println!(
        "board columns: place {} level {} place label {} name label {} header font {}",
        cols.place.is_some(),
        cols.level.is_some(),
        cols.place_label.is_some(),
        cols.name_label.is_some(),
        cols.header_font.is_some()
    );
    assert!(cols.place.is_some(), "the place point resolves");
    assert!(
        cols.level.is_some(),
        "the level icon place and size resolve"
    );
    let small = hud.layout.rank_icons.as_ref().map_or(0, |r| r.small.len());
    assert!(small > 0, "the level table's icon list");
    let mut s = HudState::default();
    for e in [
        HudEvent::Own {
            pid: Some(3),
            mode_class: Some(1),
        },
        HudEvent::Roster(rec(0, "bot2", 1, 2, 3)),
        HudEvent::Roster(rec(1, "bot3", 2, 4, 1)),
        HudEvent::Roster(rec(2, "bot1", 2, 1, 2)),
        HudEvent::Roster(rec(3, "casey", 1, 3, 2)),
        HudEvent::Life(1),
        // CLI-1360: the ping column from the ping table
        HudEvent::Pings(vec![(0, 12), (1, 118), (2, 87), (3, 1)]),
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.show_board = true;
    s.tick(1000, 60_000);
    let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
    let items = dc_client::hud::build(&hud.layout, &s, &measure, &|_| None);
    let b = &hud.layout.board;
    let ping = dc_client::hud::board::ping_column(b.kills, b.deaths, cols.clan_at);
    println!(
        "board: rows {:?} team offset {} clan {:?} kills {:?} deaths {:?} mission {:?} window {:?} -> ping column {:?}",
        b.rows,
        b.team_offset,
        cols.clan_at,
        b.kills,
        b.deaths,
        cols.mission,
        b.window.as_ref().map(|w| (w.pos, w.size)),
        ping
    );
    let pings: Vec<[f32; 2]> = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, pos, .. } if text == "118" || text == "87" => Some(*pos),
            _ => None,
        })
        .collect();
    assert_eq!(pings.len(), 2, "two ping texts in the 0xa2 values");
    // the column sits between the clan name's room and the kills
    let half = dc_client::hud::board::PING_HALF_WIDTH;
    assert!(ping[0] + half <= b.kills[0] - half, "ping column {ping:?} left of the kills");
    if let Some(c) = cols.clan_at {
        assert!(ping[0] - half >= c[0] + 60, "ping column {ping:?} right of the clan name");
    }
    let mut places: Vec<&str> = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, right, .. } if *right => Some(text.as_str()),
            _ => None,
        })
        .collect();
    places.sort();
    assert_eq!(places, ["1", "1", "2", "2"], "place numbers per team list");
    let icons: Vec<[f32; 4]> = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { dst, tex, .. } if tex.to_ascii_lowercase().ends_with(".dtx") => {
                Some(*dst)
            }
            _ => None,
        })
        .collect();
    let (lv_at, lv_size) = cols.level.unwrap();
    let level_icons: Vec<[f32; 4]> = icons
        .into_iter()
        .filter(|d| d[2] == lv_size[0] as f32 && d[3] == lv_size[1] as f32)
        .collect();
    assert_eq!(
        level_icons.len(),
        4,
        "one level icon per row (offset {lv_at:?})"
    );
    let centred = items
        .iter()
        .filter(|i| matches!(i, DrawItem::Label { anchor, .. } if anchor[0] == 0.5))
        .count();
    assert!(
        centred >= 8,
        "kills and deaths of four rows are centred: {centred}"
    );
    assert!(
        items
            .iter()
            .any(|i| matches!(i, DrawItem::Fill { colour, .. } if colour[3] == 0x4c)),
        "the own row's bar"
    );
    let img = hud.render(&s);
    save(&img, "scoreboard_tab_rom");
    for d in &level_icons {
        let px = opaque_in(&img, *d);
        println!("level icon at {d:?}: {px} opaque pixels");
        assert!(px > 20, "the level icon is drawn at {d:?}");
    }
}

#[test]
fn rom_level_table_reads_bounds_and_icons() {
    let Some(rom) = rom() else {
        return;
    };
    let t = dc_client::hud::board::LevelTable::from_rom(&rom).expect("level table");
    println!(
        "level table: {} bounds, {} icons; level of 0 exp {}, of the new-account seed {}",
        t.bounds.len(),
        t.small.len(),
        t.level_of(0),
        t.level_of(225_000)
    );
    assert!(t.bounds.len() > 2 && !t.small.is_empty());
    assert!(
        t.bounds.windows(2).filter(|w| w[0] <= w[1]).count() + 1 >= t.bounds.len() / 2,
        "the bounds mostly rise"
    );
    assert!(t.icon_of_exp(0, "player").is_some());
}

/// w2-board: the ROM board draws the clan name, the mission value and the clan / mission header labels; the death
/// panel draws the clan name and the record's name colour. Nothing of the ROM is asserted by value: only that the
/// pinned keys resolve and the texts land at row + key point.
// EVIDENCE: cshell+0xc0ac0 (clan text at cshell+0x53640, mission at cshell+0x53870), cshell+0xbf540 (header labels),
// cshell+0xcbea0 / 0xca9a0 (death panel clan text and name colour)
#[test]
fn rom_board_and_death_panel_draw_the_clan() {
    let Some(rom) = rom() else {
        return;
    };
    let hud = Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD");
    let cols = hud.layout.board_columns.clone();
    println!(
        "w2-board columns: clan {:?} colour {:?} mission {:?} clan label {} mission label {}",
        cols.clan_at,
        cols.clan_colour,
        cols.mission,
        cols.clan_label.is_some(),
        cols.mission_label.is_some()
    );
    let clan_at = cols.clan_at.expect("the clan name point resolves");
    assert!(cols.clan_colour.is_some(), "the clan colour key resolves");
    assert!(cols.mission.is_some(), "the mission point resolves");
    let (clan_label, _) = cols.clan_label.clone().expect("the clan header label");
    let (mission_label, _) = cols
        .mission_label
        .clone()
        .expect("the mission header label");
    let death_clan = hud
        .layout
        .death
        .clan
        .expect("the death panel's clan point resolves");
    let clan = |name: &str, colour: u32| dc_botclient::shell::RosterClan {
        name: name.into(),
        group_no: 7,
        mark: [1, 1],
        colour,
    };
    let mut s = HudState::default();
    for e in [
        HudEvent::Own {
            pid: Some(3),
            mode_class: Some(1),
        },
        HudEvent::Roster(RosterRecord {
            clan: Some(clan("Owls", 0)),
            ..rec(0, "bot2", 1, 2, 3)
        }),
        HudEvent::Roster(RosterRecord {
            clan: Some(clan("Hawks", 0xffff0000)),
            name_colour: 0xff00ff00,
            ..rec(3, "casey", 1, 3, 2)
        }),
        HudEvent::Life(1),
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.show_board = true;
    s.tick(1000, 60_000);
    let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
    let items = dc_client::hud::build(&hud.layout, &s, &measure, &|_| None);
    let find = |items: &[DrawItem], want: &str| {
        items.iter().find_map(|i| match i {
            DrawItem::Text {
                text, pos, style, ..
            }
            | DrawItem::Label {
                text, pos, style, ..
            } if text == want => Some((*pos, style.colour)),
            _ => None,
        })
    };
    let (owls, owls_c) = find(&items, "Owls").expect("the clan text of a row");
    let (bot2, _) = find(&items, "bot2").expect("the name");
    let b = &hud.layout.board;
    assert_eq!(
        [owls[0] - bot2[0], owls[1] - bot2[1]],
        [
            (clan_at[0] - b.name[0]) as f32,
            (clan_at[1] - b.name[1]) as f32
        ],
        "clan text at row + the clan point"
    );
    assert_eq!(
        Some(owls_c),
        cols.clan_colour,
        "no colour word: the layout colour"
    );
    assert_eq!(find(&items, "Hawks").map(|h| h.1), Some([255, 0, 0, 255]));
    assert_eq!(find(&items, "casey").map(|h| h.1), Some([0, 255, 0, 255]));
    assert!(find(&items, &clan_label).is_some() && find(&items, &mission_label).is_some());
    // the death panel of the own record (no killer: the follow camera at once)
    s.show_board = false;
    s.life_state = 2;
    s.death_at = Some(1000);
    s.now_ms = 1500;
    let items = dc_client::hud::build(&hud.layout, &s, &measure, &|_| None);
    let (hawks, _) = find(&items, "Hawks").expect("the panel's clan text");
    assert_eq!(hawks, [death_clan.1[0] as f32, death_clan.1[1] as f32]);
    assert_eq!(find(&items, "casey").map(|h| h.1), Some([0, 255, 0, 255]));
}

// ---------------------------------------------------------------------------------------------------- loopback

/// dc-lobby-server (+ dc-dserver for the made-up world) on loopback ports.
struct Stack {
    lob: Proc,
    dir: PathBuf,
    log: PathBuf,
    auth: SocketAddr,
}

impl Stack {
    fn start(tag: &str, watchdog: &Watchdog) -> Stack {
        let lobby = lobby_exe();
        let dserver = dserver_exe();
        let dir = scratch(tag);
        let world = synth_world_file(&dir);
        let log = dir.join("lobby.log");
        let (port, lport, cport, web) = loop {
            let p = (
                free_tcp_port(),
                free_tcp_port(),
                free_tcp_port(),
                free_tcp_port(),
            );
            let mut v = vec![p.0, p.1, p.2, p.3, p.1.wrapping_add(1)];
            v.sort();
            v.dedup();
            if v.len() == 5 {
                break p;
            }
        };
        let gameport = free_udp_port();
        let dargs = format!(
            "-worldfile {} -announce Worlds\\Synth.dat -tick 20 -roundsecs 12 -holds 1500,800",
            world.display()
        );
        let mut cmd = Command::new(&lobby);
        cmd.args([
            "--port",
            &port.to_string(),
            "--lobbyport",
            &lport.to_string(),
        ])
        .args([
            "--clanport",
            &cport.to_string(),
            "--clanweb",
            &web.to_string(),
        ])
        .arg("--db")
        .arg(dir.join("lobby.db"))
        .args(["--countdown", "0.05", "--gameport", &gameport.to_string()])
        .arg("--log")
        .arg(&log)
        .arg("--dserver")
        .arg("--dserver-exe")
        .arg(&dserver)
        .arg("--dserver-logdir")
        .arg(&dir)
        .args(["--dserver-world", "Worlds\\Synth", "--dserver-args", &dargs])
        .current_dir(&dir);
        let lob = Proc::spawn(cmd);
        watchdog.watch_pid(lob.pid());
        watchdog.watch_log(&log);
        let up = (0..600).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            std::net::TcpStream::connect(("127.0.0.1", lport)).is_ok()
                && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
        });
        assert!(
            up,
            "lobby did not listen; log:\n{}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
        Stack {
            lob,
            auth: format!("127.0.0.1:{port}").parse().unwrap(),
            dir,
            log,
        }
    }

    /// Stops the lobby and its game servers by PID; returns the lobby log.
    fn stop(mut self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        self.lob.drain();
        if let Some(p) = self.lob.pid() {
            kill_pid(p);
        }
        let _ = self.lob.finish(Duration::from_secs(2));
        for p in logged_pids(&text) {
            kill_pid(p);
        }
        text
    }
}

/// Times (capture seconds) of the server's 0x3f and of every 0x6c in the client's own capture.
fn result_and_departures(capture: &std::path::Path) -> (Option<f64>, Vec<f64>) {
    let Ok(d) = decode_file(capture) else {
        return (None, Vec::new());
    };
    let mut screen = None;
    let mut left = Vec::new();
    for m in d.msgs.iter().filter(|m| !m.c2s) {
        for b in unbundle(&m.bits) {
            let by = b.as_bytes();
            if by.first() == Some(&13) {
                match by.get(1) {
                    Some(&0x3f) if screen.is_none() => screen = Some(m.t),
                    Some(&0x6c) => left.push(m.t),
                    _ => {}
                }
            }
        }
    }
    (screen, left)
}

/// The profile before a match and the match's result.
type Played = (LobbyProfile, Option<MatchResult>);

#[test]
fn loopback_result_screen_lists_every_player_with_ranks_and_totals() {
    // a hard limit: a run takes about 60 s alone; a lost peer must fail it, never hang it
    let watchdog = Watchdog::arm("scoreboard-4", Duration::from_secs(420));
    let stack = Stack::start("scoreboard-4", &watchdog);
    let auth = stack.auth;
    let dir = stack.dir.clone();
    let nick = "BoardHost";
    let mut base = NetOptions::new(
        Connect::Lobby {
            auth,
            room: RoomChoice::Any,
            password: String::new(),
            channel: None,
        },
        nick,
    );
    base.world_file = Some({
        let s = synth_source();
        WorldFile {
            bytes: s.bytes,
            stand_dims: SYNTH_DIMS,
            textures: s.textures,
        }
    });
    base.match_wait = Duration::from_secs(60);
    // the client leaves 6 s after 0x3f, the bots 3 s after it (their default): their 0x6c come first
    base.result_hold = Duration::from_secs(6);
    base.log = Some(dir.join("client_events.jsonl"));
    base.capture = Some(dir.join("client_capture.jsonl"));
    let mut link = LinkOptions::new(nick);
    link.log = Some(dir.join("menu_events.jsonl"));
    link.capture = Some(dir.join("menu_capture.jsonl"));
    let rom = std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir());
    let shots = std::env::var_os("DC_MENU_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if rom.is_some() {
                out_dir().join("loopback_menu")
            } else {
                dir.join("shots")
            }
        });
    let mut menu = Menu::new(MenuOptions {
        auth,
        user: nick.into(),
        link,
        rom: rom.clone(),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    println!("menu: {}", menu.source());
    let bots: Vec<_> = ["BoardBot1", "BoardBot2", "BoardBot3"]
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            std::thread::spawn(move || {
                // one after the other, so the three join the host's room in a known order
                std::thread::sleep(Duration::from_millis(400 * i as u64));
                let mut cfg = BotConfig::new(auth, name);
                cfg.room = RoomChoice::Any;
                cfg.motion = MotionKind::Circle;
                cfg.duration = Duration::from_secs(60);
                cfg.use_rom = false;
                cfg.load_ms = 300;
                cfg.match_wait = Duration::from_secs(90);
                run_bot(&cfg, &Recorder::null(name))
            })
        })
        .collect();
    // the host names TDM: since LG-70 (project decision, ae93e5a2: Elimination Pro playable) the creation box
    // opens on mode row 8 as the 2009 box does (`menu::create::START_MODE_ROW`), a 7-round match whose drawn 12 s
    // rounds outlast the client's 60 s run, so 0x3f never came; TDM ends on the 12 s test clock. Without the ROM the
    // box has no mode list and keeps its own mode.
    let tdm = rom
        .as_ref()
        .and_then(|rom| dc_bute::GameRoomData::from_rom(rom).ok())
        .and_then(|g| {
            dc_client::menu::pilot::RoomNames::from_rows(&g.maps(), &g.modes(false))
                .mode("TDM")
                .ok()
        });
    let mut pilot = Pilot::new(PilotPlan {
        password: "pw-board-1".into(),
        room: PilotRoom::Create {
            title: "board test".into(),
            mode: tdm,
            map: None,
            start_after: 4,
        },
        matches: 1,
        shots: Some(shots.clone()),
        timeout: Duration::from_secs(240),
    });
    let hopts = HeadlessOptions {
        frames: 60 * 60,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 12,
        font_bytes: Some(synth_font()),
        realtime: true,
        stop_after_over: Some(60),
        ..HeadlessOptions::default()
    };
    let mut lines = Vec::new();
    let seen: Arc<Mutex<Vec<Played>>> = Arc::default();
    let report = {
        let mut inner = headless_runner(&base, nick.into(), &hopts, &mut lines);
        let seen = seen.clone();
        let mut play = move |server: SocketAddr, lobby: dc_botclient::lobby::Lobby, mode: i32| {
            let before = lobby.profile.clone();
            let back: GameBack = inner(server, lobby, mode);
            seen.lock().unwrap().push((before, back.result.clone()));
            back
        };
        run_pilot(&mut menu, &mut pilot, &mut play)
    };
    let after = menu.data.profile.clone();
    let screen_after = menu.screen_name();
    println!("trail: {}", report.trail.join(" -> "));
    for l in &lines {
        println!("  {l}");
    }
    println!("  end: {} | boxes {:?}", report.end, report.boxes);
    for b in bots {
        match b.join().expect("bot thread") {
            Ok(v) => println!("  bot: stage {} end {}", v["stage"], v["end"]),
            Err(e) => println!("  bot error: {e}"),
        }
    }
    let text = stack.stop();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(report.matches, 1, "one match played; lobby log:\n{text}");
    assert_eq!(screen_after, "room", "back in the waiting room");
    let (before, result) = seen.first().cloned().expect("the match ran");
    let result = result.expect("a result screen came");

    // 1. the departures: the three bots' 0x6c reached the client after 0x3f, before its own leave
    let (t_screen, left) = result_and_departures(&dir.join("client_capture.jsonl"));
    let t_screen = t_screen.expect("0x3f in the client's capture");
    let late: Vec<f64> = left
        .iter()
        .map(|t| t - t_screen)
        .filter(|d| *d > 0.0)
        .collect();
    println!("0x6c after 0x3f (s): {late:?}");
    assert!(
        late.len() >= 3,
        "the bots left after the result screen message: {late:?}"
    );

    // 2. every player of the match on the result screen, two lists of two
    for r in &result.rows {
        println!(
            "  row pid {} {:<10} team {} kills {} deaths {} exp {}+{} (bonus {}) gp +{} (bonus {}) rank {}",
            r.pid,
            r.name,
            r.team,
            r.kills,
            r.deaths,
            r.exp_before,
            r.exp_gain,
            r.exp_bonus,
            r.gp_gain,
            r.gp_bonus,
            r.rank
        );
    }
    assert_eq!(result.rows.len(), 4, "four rows: {:?}", result.rows);
    let (l, r) = result.sides();
    assert_eq!(
        (result.rows_of(l).len(), result.rows_of(r).len()),
        (2, 2),
        "two per team"
    );
    for n in ["BoardHost", "BoardBot1", "BoardBot2", "BoardBot3"] {
        assert!(result.rows.iter().any(|r| r.name == n), "{n} listed");
    }

    // 3. MY SCORE: the stored totals + the earned amounts, equal to the lobby's own progress push
    let own = result.own_row().expect("own row").clone();
    let (exp_text, gp_text) = summary_texts(&result, before.exp, before.gp);
    println!(
        "  profile before exp {} gp {}; earned exp {} gp {}; after (lobby 0x3004) exp {} gp {}; MY SCORE \"{exp_text}\" \
         \"{gp_text}\"",
        before.exp, before.gp, own.exp_gain, own.gp_gain, after.exp, after.gp
    );
    assert!(
        own.exp_gain > 0 || own.gp_gain > 0,
        "the match earned something"
    );
    assert_eq!(
        (after.exp, after.gp),
        (before.exp + own.exp_gain, before.gp + own.gp_gain),
        "the waiting room's totals (the parked 0x3004) = before + earned"
    );
    assert!(
        exp_text.starts_with(&grouped(i64::from(after.exp))),
        "{exp_text}"
    );
    assert!(
        gp_text.starts_with(&grouped(i64::from(after.gp))),
        "{gp_text}"
    );
    assert!(exp_text.contains(&format!(
        "(+{}",
        grouped(i64::from(own.exp_gain - own.exp_bonus))
    )));

    // 4. the screen itself: place numbers and (with the ROM) level icons on all four rows
    menu.data.result = Some(result.clone());
    menu.data.profile = before.clone();
    menu.open(Go::Result);
    let items = menu.items();
    let texts: Vec<String> = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label { text, .. } | DrawItem::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    for n in ["BoardHost", "BoardBot1", "BoardBot2", "BoardBot3"] {
        assert!(texts.iter().any(|t| t.contains(n)), "{n} drawn: {texts:?}");
    }
    if rom.is_some() {
        let ones = texts.iter().filter(|t| *t == "1").count();
        let twos = texts.iter().filter(|t| *t == "2").count();
        assert!(
            ones >= 2 && twos >= 2,
            "place numbers 1 and 2 in both lists: {texts:?}"
        );
        let icons = items
            .iter()
            .filter(
                |i| matches!(i, DrawItem::Image { dst, .. } if dst[2] == 30.0 && dst[3] == 22.0),
            )
            .count();
        assert!(
            icons >= 4,
            "a level icon per row (and the MY SCORE icon): {icons}"
        );
        assert!(
            texts.contains(&exp_text) && texts.contains(&gp_text),
            "MY SCORE texts drawn"
        );
        let img = menu.image();
        let frame = dc_render::FrameImage {
            width: img.width,
            height: img.height,
            rgba: img.rgba,
        };
        let p = out_dir().join("result_screen_4_players.png");
        let _ = frame.save_png(&p);
        println!("png: {}", p.display());
    }
}

//! HUD round 2: the round clock, waiting banner, match intro, round result mark, first-kill notes, HiSec case HUD,
//! result screen and aim_cross, drawn from the user's ROM; and the layout of the rest state compared with the real
//! 2009 client's screenshots.
//!
//! ROM-gated (skipped without `CA_ROM`). The screenshot comparison also needs the capture folder of the hands-free
//! aim cross sessions (`DC_CAPTURES`; skipped when absent): it
//! measures each element of our composed HUD against the real client's frame at 1680 x 1050 by the pixel offset
//! that matches best, and requires every offset within 2 pixels. No ROM or screenshot value is in this file: the
//! element rectangles come from the loaded layout, the reference pixels from the capture at run time.
// SPEC: GAME_MESSAGES_2009_S2C.md §2.1, §2.1.1, §2.1.2, §3.3.1, §3.5; HISEC_2009.md §3; FMT_CRES_UI.md §5, §8
// OBSERVED: capture 20261025_123446_xhair-frozen/driver/rest_1.png (the 2009 client on the
// reference stack, SNOWVALLEY TDM, M4A1, health 100 / armour 100, clip 30 / 90, goal 140, no round clock)

use dc_botclient::shell::{KillRecord, RosterRecord, SummaryRow};
use dc_client::hud::msgs::{CaseEvent, IntroBlock, MatchBegin, HISEC_NOBODY};
use dc_client::hud::state::{AmmoView, CaseMark, HudEvent, RoundClock};
use dc_client::hud::{Hud, HudState};
use dc_client::overlay::text::RgbaImage;
use std::path::PathBuf;

fn rom() -> Option<PathBuf> {
    let r = dc_rom::test_rom();
    if r.is_none() {
        println!("skipped: CA_ROM unset");
    }
    r
}

fn save(img: &RgbaImage, name: &str) {
    let out = dc_render::default_out_dir().join("dc-client");
    let _ = std::fs::create_dir_all(&out);
    if let Some(b) = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()) {
        let _ = b.save(out.join(format!("hud2_{name}.png")));
    }
}

fn rec(pid: u8, name: &str, team: u8) -> RosterRecord {
    RosterRecord {
        name: name.into(),
        pid,
        team,
        complete: true,
        ..RosterRecord::default()
    }
}

/// The real client's rest state: TDM, own team 1, alive, 100 / 100, M4A1 30 / 90, 0 : 0, goal 140, clock hidden
/// (the reference stack sent `round_secs` 0 in 0x44 and 0x45).
fn rest_state(hud: &Hud, goal: u8) -> HudState {
    let mut s = HudState::default();
    s.texts.join = hud.layout.chat.join_form.clone();
    s.texts.leave = hud.layout.chat.leave_form.clone();
    for e in [
        HudEvent::Own {
            pid: Some(0),
            mode_class: Some(1),
        },
        HudEvent::Roster(rec(0, "alpha1", 1)),
        HudEvent::Roster(rec(1, "bravo1", 2)),
        HudEvent::Life(1),
        HudEvent::Status {
            kind: 2,
            value: 100.0,
        },
        HudEvent::Status {
            kind: 3,
            value: 100.0,
        },
        HudEvent::RoundBegin(0),
        HudEvent::TeamPair([0, 0]),
        HudEvent::MatchBegin(Box::default()),
        HudEvent::Goal(goal),
    ] {
        s.apply(&e, 1000, 0.0);
    }
    s.ammo = Some(AmmoView {
        weapon: 145,
        clip: 30,
        reserve: 90,
        clip_size: Some(30),
        single_shot: false,
        order_class: 1,
        infinite: false,
    });
    s.tick(1000, 1000);
    s
}

/// Opaque pixels of `img` inside the virtual rectangle `r` (x, y, w, h).
fn drawn_in(img: &RgbaImage, r: [f32; 4]) -> usize {
    let (sx, sy) = (img.width as f32 / 1024.0, img.height as f32 / 768.0);
    let (x0, y0) = ((r[0] * sx).floor() as u32, (r[1] * sy).floor() as u32);
    let (x1, y1) = (
        ((r[0] + r[2]) * sx).ceil().min(img.width as f32) as u32,
        ((r[1] + r[3]) * sy).ceil().min(img.height as f32) as u32,
    );
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| img.pixel(x, y)[3] > 100)
        .count()
}

fn rect(pos: [i32; 2], size: [i32; 2]) -> [f32; 4] {
    [pos[0] as f32, pos[1] as f32, size[0] as f32, size[1] as f32]
}

const W: u32 = 1024;
const H: u32 = 768;

#[test]
fn rom_round_clock_shows_digits_only_while_shown() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let c = hud.layout.clock.clone();
    let region = [
        c.minutes[0][0] as f32,
        c.minutes[0][1] as f32,
        (c.seconds[1][0] + c.size[0] - c.minutes[0][0]) as f32,
        c.size[1] as f32,
    ];
    let mut s = rest_state(&hud, 140);
    let hidden = hud.render(&s);
    assert_eq!(drawn_in(&hidden, region), 0, "0x45 with 0 hides the clock");
    s.apply(&HudEvent::RoundBegin(600), 2000, 0.0);
    s.tick(2000, 1000);
    let shown = hud.render(&s);
    save(&shown, "clock_600");
    let n = drawn_in(&shown, region);
    println!("clock region {region:?}: {n} pixels at 10:00");
    assert!(n > 100, "the digits are drawn: {n}");
    // a 0x48 hides it again; 0xd7 sub 4 shows it frozen
    s.apply(&HudEvent::ClockStop(3), 3000, 0.0);
    assert_eq!(drawn_in(&hud.render(&s), region), 0);
    s.apply(
        &HudEvent::CoopClock {
            running: false,
            secs: 75,
        },
        4000,
        0.0,
    );
    s.tick(9000, 1000);
    assert_eq!(s.clock.shows(9000), (1, 15), "frozen at 1:15");
    assert!(drawn_in(&hud.render(&s), region) > 100);
    assert!(hud.assets.missing.is_empty(), "{:?}", hud.assets.missing);
}

#[test]
fn rom_waiting_banner_and_intro_banner() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let banner = hud.layout.waiting.clone().expect("waiting banner");
    let mut s = rest_state(&hud, 140);
    let before = hud.render(&s);
    let r = rect(banner.pos, banner.size);
    assert_eq!(drawn_in(&before, r), 0);
    // the intro block's two images: the client's own operation and mission pictures of a ROM mode row are not
    // named here; the test uses the waiting banner's texture for both (any HUD texture shows the placement)
    let m = MatchBegin {
        show_intro: true,
        waiting_banner: true,
        intro: Some(IntroBlock {
            head: banner.tex.clone(),
            body: banner.tex.clone(),
            ..IntroBlock::default()
        }),
        ..MatchBegin::default()
    };
    s.apply(&HudEvent::MatchBegin(Box::new(m)), 10_000, 0.0);
    let l = hud.layout.intro.clone().expect("intro layout");
    // during the delay nothing of the intro is drawn; the waiting banner is up
    s.tick(10_000 + l.delay / 2, 1000);
    let during_delay = hud.render(&s);
    let n_banner = drawn_in(&during_delay, r);
    println!("waiting banner {r:?}: {n_banner} pixels");
    assert!(n_banner > 200, "the waiting banner is drawn: {n_banner}");
    let head = rect(l.head_pos, l.head_size);
    let base = drawn_in(&during_delay, head);
    // in the hold phase the title image is drawn at its place
    s.tick(10_000 + l.delay + (l.fade_in + l.hold) / 2, 1000);
    let hold = hud.render(&s);
    save(&hold, "intro_hold");
    let n_head = drawn_in(&hold, head);
    println!("intro title {head:?}: {base} pixels during the delay, {n_head} in the hold phase");
    assert!(n_head > base + 200, "the intro title is drawn: {n_head}");
    // after the duration it is gone; the waiting banner stays until the round result
    s.tick(10_000 + l.delay + l.duration + 10, 1000);
    let after = hud.render(&s);
    assert_eq!(drawn_in(&after, head), base);
    s.apply(
        &HudEvent::RoundResult {
            round_secs: 3,
            win_team: 1,
            wins: [0, 0],
        },
        20_000,
        0.0,
    );
    // the result board (window + win header) waits 3000 ms after 0x46 (cshell+0xc02e0)
    s.tick(20_000, 1000);
    let held = hud.render(&s);
    s.tick(23_001, 1000);
    let won = hud.render(&s);
    save(&won, "round_won");
    let rm = &hud.layout.result_mark;
    // the header is a child of the board window: screen place = window position + the mark key (cshell+0xc2490)
    let wpos = hud.layout.board.window.as_ref().expect("board window").pos;
    let mpos = [wpos[0] + rm.pos[0], wpos[1] + rm.pos[1]];
    let measure = |_f: i32, t: &str| t.len() as f32 * 8.0;
    let items = dc_client::hud::build(&hud.layout, &s, &measure, &|_| None);
    let at = [
        mpos[0] as f32,
        mpos[1] as f32,
        rm.size[0] as f32,
        rm.size[1] as f32,
    ];
    assert!(
        items.iter().any(|i| matches!(i,
            dc_client::hud::DrawItem::Image { dst, .. } if *dst == at)),
        "the result header is drawn at the window position plus the key: {at:?}"
    );
    let held_mark = drawn_in(&held, rect(mpos, rm.size));
    // the result mark's last rows may reach the banner's first ones: compare below the mark
    let below = (mpos[1] + rm.size[1]) as f32;
    let r_below = [r[0], r[1].max(below), r[2], r[1] + r[3] - r[1].max(below)];
    assert!(!s.waiting_banner);
    assert_eq!(
        drawn_in(&held, r_below),
        0,
        "0x46 takes the waiting banner down"
    );
    let mark = drawn_in(&won, rect(mpos, rm.size));
    println!("result mark {:?}: {mark} pixels", rect(mpos, rm.size));
    assert!(mark > 2000, "the win mark is drawn: {mark}");
    assert!(
        held_mark + 2000 < mark,
        "no win mark in the first 3000 ms: {held_mark} vs {mark}"
    );
    assert!(hud.assets.missing.is_empty(), "{:?}", hud.assets.missing);
}

#[test]
fn rom_first_kill_note_and_announcement() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let mut s = rest_state(&hud, 140);
    let mk = hud.layout.mk.clone().expect("multi-kill layout");
    let note = [
        mk.pos[0] as f32,
        (mk.pos[1] + mk.lift.min(0)) as f32,
        512.0,
        (mk.size[1] + mk.lift.abs()) as f32,
    ];
    let combo = hud.layout.opener_banner.clone().expect("announcement");
    let centre = [
        combo.centre[0] as f32 - combo.size[0] as f32 / 2.0,
        combo.centre[1] as f32 - combo.size[1] as f32 / 2.0,
        combo.size[0] as f32,
        combo.size[1] as f32,
    ];
    let before = hud.render(&s);
    let (n0, c0) = (drawn_in(&before, note), drawn_in(&before, centre));
    s.apply(
        &HudEvent::Kill(KillRecord {
            victim: 1,
            killer: 0,
            kill_kind: 1,
            weapon: 145,
            hit_zone: 3,
            streak_flag: 1,
            feed_value: 2,
            complete: true,
            ..KillRecord::default()
        }),
        5000,
        0.0,
    );
    s.tick(5000 + (combo.end * 500.0) as u32, 20_000);
    let after = hud.render(&s);
    save(&after, "kill_note");
    let (n1, c1) = (drawn_in(&after, note), drawn_in(&after, centre));
    println!("first-kill note {note:?}: {n0} -> {n1}; announcement {centre:?}: {c0} -> {c1}");
    assert!(n1 > n0 + 300, "the note is drawn");
    assert!(c1 > c0 + 2000, "the own announcement is drawn");
    // after the note time both are gone
    s.tick(5000 + mk.lifetime_ms + 10, 20_000);
    s.expire_notes(5000 + mk.lifetime_ms + 10, mk.lifetime_ms);
    let late = hud.render(&s);
    assert!(drawn_in(&late, note) <= n0 + 20);
    assert!(drawn_in(&late, centre) <= c0 + 20);
}

#[test]
fn rom_hisec_indicator_countdown_and_pickup_banner() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let (_, size) = hud.layout.hisec.indicator.clone().expect("indicator");
    let mut s = rest_state(&hud, 140);
    let at = [700.0f32, 300.0];
    let box_ = [
        at[0] - 40.0,
        at[1] - size[1] as f32 / 2.0 - 40.0,
        80.0,
        size[1] as f32 + 80.0,
    ];
    let base = drawn_in(&hud.render(&s), box_);
    s.case_marks = vec![CaseMark {
        key: 1,
        at: Some(at),
        remaining: 12,
    }];
    let free = hud.render(&s);
    save(&free, "hisec_free");
    let n_free = drawn_in(&free, box_);
    // reserved for the other player: nothing; reserved for me: drawn; at 3 s: the number turns red
    s.apply(
        &HudEvent::Hisec(CaseEvent::Owner { key: 1, owner: 1 }),
        2000,
        0.0,
    );
    let theirs = drawn_in(&hud.render(&s), box_);
    s.apply(
        &HudEvent::Hisec(CaseEvent::Owner { key: 1, owner: 0 }),
        2100,
        0.0,
    );
    s.case_marks[0].remaining = 3;
    let mine_img = hud.render(&s);
    save(&mine_img, "hisec_mine_3s");
    let red = mine_img
        .rgba
        .chunks(4)
        .filter(|p| p[3] > 150 && p[0] > 200 && p[1] < 60 && p[2] < 60)
        .count();
    println!("indicator box {box_:?}: base {base}, free {n_free}, other owner {theirs}; red number pixels {red}");
    assert!(n_free > base + 200, "indicator and number drawn: {n_free}");
    assert_eq!(theirs, base, "hidden for another owner");
    assert!(red > 10, "the countdown is red at <= 5: {red}");
    // behind the camera: nothing
    s.case_marks[0].at = None;
    assert_eq!(drawn_in(&hud.render(&s), box_), base);
    // the own pickup: the banner at its centre
    let p = hud.layout.hisec.pickup.clone().expect("pickup banner");
    let pr = [
        p.centre[0] as f32 - p.size[0] as f32 / 2.0,
        p.centre[1] as f32 - p.size[1] as f32 / 2.0,
        p.size[0] as f32,
        p.size[1] as f32,
    ];
    let b0 = drawn_in(&hud.render(&s), pr);
    s.apply(
        &HudEvent::Hisec(CaseEvent::Taken { key: 1, player: 0 }),
        3000,
        0.0,
    );
    s.tick(3000 + (p.end * 500.0) as u32, 20_000);
    let picked = hud.render(&s);
    save(&picked, "case_banner");
    let b1 = drawn_in(&picked, pr);
    println!("pickup banner {pr:?}: {b0} -> {b1}");
    assert!(b1 > b0 + 2000, "the pickup banner is drawn");
    assert!(s.case_owner.is_empty());
    let _ = HISEC_NOBODY;
    assert!(hud.assets.missing.is_empty(), "{:?}", hud.assets.missing);
}

#[test]
fn rom_result_screen_rows() {
    let Some(rom) = rom() else {
        return;
    };
    let mut hud = Hud::from_rom(&rom, W, H, None).expect("ROM HUD");
    let mut s = rest_state(&hud, 140);
    let w = hud.layout.board.window.clone().expect("board window");
    let r = rect(w.pos, w.size);
    let before = drawn_in(&hud.render(&s), r);
    for e in [
        HudEvent::Summary(SummaryRow {
            pid: 0,
            exp_gain: 43,
            gp_gain: 34,
            ..SummaryRow::default()
        }),
        HudEvent::ResultScreen {
            flag: false,
            value: 1,
        },
    ] {
        s.apply(&e, 9000, 0.0);
    }
    let img = hud.render(&s);
    save(&img, "result_screen");
    let n = drawn_in(&img, r);
    println!("result screen {r:?}: {before} -> {n}");
    assert!(n > before + 50_000, "the window and rows are drawn");
    assert_eq!(
        s.clock,
        RoundClock {
            shown: false,
            ..s.clock
        }
    );
}

/// One element's best pixel offset against the reference: `(dx, dy, mean error, pixels)`.
fn best_offset(
    ours: &RgbaImage,
    real: &image::RgbImage,
    r: [f32; 4],
) -> Option<(i32, i32, f32, usize)> {
    let (sx, sy) = (ours.width as f32 / 1024.0, ours.height as f32 / 768.0);
    let (x0, y0) = ((r[0] * sx).floor() as i32, (r[1] * sy).floor() as i32);
    let (x1, y1) = (
        ((r[0] + r[2]) * sx).ceil() as i32,
        ((r[1] + r[3]) * sy).ceil() as i32,
    );
    let mut pts = Vec::new();
    for y in y0.max(0)..y1.min(ours.height as i32) {
        for x in x0.max(0)..x1.min(ours.width as i32) {
            if ours.pixel(x as u32, y as u32)[3] > 200 {
                pts.push((x, y));
            }
        }
    }
    if pts.len() < 20 {
        return None;
    }
    let mut best: Option<(i32, i32, f32, usize)> = None;
    for dy in -4..=4 {
        for dx in -4..=4 {
            let mut e = 0u64;
            for &(x, y) in &pts {
                let (rx, ry) = (x + dx, y + dy);
                if rx < 0 || ry < 0 || rx >= real.width() as i32 || ry >= real.height() as i32 {
                    e += 765;
                    continue;
                }
                let a = ours.pixel(x as u32, y as u32);
                let b = real.get_pixel(rx as u32, ry as u32).0;
                e += (0..3)
                    .map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs() as u64)
                    .sum::<u64>();
            }
            let m = e as f32 / pts.len() as f32;
            if best.is_none_or(|b| m < b.2) {
                best = Some((dx, dy, m, pts.len()));
            }
        }
    }
    best
}

fn captures() -> Option<PathBuf> {
    let p = std::env::var_os("DC_CAPTURES")
        .map(PathBuf::from)
        .unwrap_or_default();
    let f = p.join("20261025_123446_xhair-frozen/driver/rest_1.png");
    if f.is_file() {
        Some(f)
    } else {
        println!("skipped: no capture screenshot at {}", f.display());
        None
    }
}

#[test]
fn rom_rest_layout_matches_the_2009_client_within_2_pixels() {
    let Some(rom) = rom() else {
        return;
    };
    let Some(shot) = captures() else {
        return;
    };
    let real = image::open(&shot).expect("screenshot").to_rgb8();
    let (w, h) = (real.width(), real.height());
    let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
    let s = rest_state(&hud, 140);
    let ours = hud.render(&s);
    save(&ours, "rest_1680x1050");
    let l = hud.layout.clone();
    let mut elements: Vec<(String, [f32; 4])> = Vec::new();
    let mut add = |name: &str, r: Option<[f32; 4]>| {
        if let Some(r) = r {
            elements.push((name.to_string(), r));
        }
    };
    let img = |i: &Option<dc_client::hud::layout::Img>| i.as_ref().map(|i| rect(i.pos, i.size));
    let digits3 = |d: &Option<dc_client::hud::layout::Digits>| {
        d.as_ref().map(|d| {
            let x0 = d.pos[0][0];
            let x1 = d.pos[2][0] + d.size[0];
            [
                x0 as f32,
                d.pos[0][1] as f32,
                (x1 - x0) as f32,
                d.size[1] as f32,
            ]
        })
    };
    add("stance", img(&l.health.stance_stand));
    add("health digits", digits3(&l.health.health_digits));
    add("armour digits", digits3(&l.health.armour_digits));
    add("stamina", img(&l.health.stamina_back));
    add("clip digits", digits3(&l.ammo.clip));
    add("reserve digits", digits3(&l.ammo.reserve));
    add("fire mode", img(&l.ammo.fire_auto));
    add("weapon name", l.ammo.name_image.map(|(p, z)| rect(p, z)));
    add("team 1 name", img(&l.score.team1_mark));
    add("team 2 name", img(&l.top.team2_base));
    add("team 1 digits", digits3(&l.score.team1));
    add("team 2 digits", digits3(&l.score.team2));
    add("mode title", img(&l.top.tdm_title));
    add("goal word", img(&l.top.goal_word));
    add(
        "own bar",
        l.top.own_bar.as_ref().map(|b| rect(b.team1, b.size)),
    );
    add("aim cross", Some([497.0, 369.0, 30.0, 30.0]));
    let mut worst = 0;
    let mut tested = 0;
    for (name, r) in &elements {
        match best_offset(&ours, &real, *r) {
            Some((dx, dy, err, n)) => {
                println!(
                    "{name:15} {r:?}: offset ({dx:+}, {dy:+}), error {err:.1} over {n} pixels"
                );
                assert!(err < 120.0, "{name}: no match ({err:.1})");
                worst = worst.max(dx.abs()).max(dy.abs());
                tested += 1;
            }
            None => println!("{name:15} {r:?}: nothing drawn"),
        }
    }
    println!("{tested} elements, worst offset {worst} px");
    assert!(tested >= 14, "elements tested: {tested}");
    assert!(
        worst <= 2,
        "every element within 2 pixels of the 2009 client: worst {worst}"
    );
}

#[test]
fn rom_crosshair_bars_match_the_2009_client() {
    let Some(rom) = rom() else {
        return;
    };
    let Some(shot) = captures() else {
        return;
    };
    let real = image::open(&shot).expect("screenshot").to_rgb8();
    let (w, h) = (real.width(), real.height());
    let mut hud = Hud::from_rom(&rom, w, h, None).expect("ROM HUD");
    let s = rest_state(&hud, 140);
    let img = hud.render(&s);
    let ours = |x: u32, y: u32| {
        let p = img.pixel(x, y);
        p[3] > 200 && p[1] > 200 && p[0] < 60 && p[2] < 60
    };
    let theirs = |x: u32, y: u32| {
        let p = real.get_pixel(x, y).0;
        p[1] > 150 && p[0] < 120 && p[2] < 120
    };
    let (cx, cy) = (w / 2 - 1, h / 2 - 1);
    let span = 60;
    let row = |f: &dyn Fn(u32, u32) -> bool| -> Vec<u32> {
        (cx - span..cx + span).filter(|&x| f(x, cy)).collect()
    };
    let col = |f: &dyn Fn(u32, u32) -> bool| -> Vec<u32> {
        (cy - span..cy + span).filter(|&y| f(cx, y)).collect()
    };
    let (ro, rt, co, ct) = (row(&ours), row(&theirs), col(&ours), col(&theirs));
    println!(
        "row {cy}: ours {ro:?}
        2009 {rt:?}
column {cx}: ours {co:?}
        2009 {ct:?}"
    );
    assert!(
        !rt.is_empty() && !ct.is_empty(),
        "the 2009 aim cross is found"
    );
    assert_eq!(ro, rt, "horizontal bars");
    assert_eq!(co, ct, "vertical bars");
}

/// A synthetic world whose frame state carries HUD inputs and one HiSec case object at `ahead` world units along
/// the spawn's view (negative: behind the player).
struct CaseSource {
    inner: dc_client::world::SynthSource,
    spawn: Option<dc_client::world::SpawnPoint>,
    frame: u32,
    ahead: f32,
}

impl dc_client::WorldSource for CaseSource {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn load(
        &mut self,
        gpu: Option<dc_client::world::GpuTarget<'_>>,
    ) -> Result<dc_client::world::LoadedWorld, dc_client::ClientError> {
        let w = self.inner.load(gpu)?;
        self.spawn = w.spawn(0).ok();
        Ok(w)
    }
    fn frame_state(&mut self) -> dc_client::world::FrameState {
        self.frame += 1;
        let t = self.frame * 50;
        let mut fs = dc_client::world::FrameState {
            life_state: Some(1),
            clock_ms: Some(t),
            ..dc_client::world::FrameState::default()
        };
        if self.frame == 1 {
            fs.hud_events = vec![
                (
                    t,
                    HudEvent::Own {
                        pid: Some(0),
                        mode_class: Some(1),
                    },
                ),
                (t, HudEvent::Roster(rec(0, "alpha1", 1))),
                (t, HudEvent::Life(1)),
                (
                    t,
                    HudEvent::Hisec(CaseEvent::Owner {
                        key: 4,
                        owner: HISEC_NOBODY,
                    }),
                ),
            ];
        }
        if let Some(sp) = self.spawn {
            let (s, c) = sp.yaw.sin_cos();
            fs.hud_cases = vec![dc_client::world::HudCase {
                key: 4,
                pos: [
                    sp.pos.x + s * self.ahead,
                    sp.pos.y,
                    sp.pos.z + c * self.ahead,
                ],
                remaining: 9,
            }];
        }
        fs
    }
}

/// The frame loop projects the case objects of its source onto the HUD (testkit headless run, ROM HUD): a case in
/// front of the player gets its indicator and countdown, one behind the player none.
#[test]
fn rom_headless_frame_loop_draws_the_case_indicator() {
    let Some(rom) = rom() else {
        return;
    };
    let run = |ahead: f32| {
        let opts = dc_client::HeadlessOptions {
            frames: 4,
            width: 1024,
            height: 768,
            out: None,
            font_bytes: Some(dc_client::testkit::synth_font()),
            hud: dc_client::hud::HudChoice::Rom(rom.clone()),
            debug_panel: false,
            ..dc_client::HeadlessOptions::default()
        };
        let mut src = CaseSource {
            inner: dc_client::testkit::synth_source(),
            spawn: None,
            frame: 0,
            ahead,
        };
        dc_client::run_headless(&mut src, None, &opts).expect("headless run")
    };
    let front = run(300.0);
    if front.adapter.is_none() {
        println!("skipped: no adapter ({:?})", front.render_skipped);
        return;
    }
    let behind = run(-300.0);
    let (a, b) = (front.last_hud.unwrap(), behind.last_hud.unwrap());
    save(&a, "frame_loop_case_ahead");
    // the indicator and its number are drawn around the screen's vertical centre line, above the aim point
    let band = [384.0, 200.0, 256.0, 184.0];
    let (na, nb) = (drawn_in(&a, band), drawn_in(&b, band));
    println!("case band {band:?}: ahead {na}, behind {nb}");
    assert!(
        na > nb + 150,
        "the indicator of the case ahead is drawn: {na} vs {nb}"
    );
}

/// The HUD's readers over every message of two captures: the reference stack's tdm6 (accepted by the real client) and the
/// user's rust6 round on the Rust stack (the real client showed the clock of its 0x45). Every 0x45 is read to its
/// last bit with its intro block; every 0x46 / 0x48 / 0x3f / 0xd6 and every case countdown note parses.
// OBSERVED: capture 20261025_065016_tdm6 (REFERENCE), 20261025_125432_rust6 (Rust stack, real
// client, user: the clock was shown)
#[test]
fn capture_messages_parse_with_the_hud_readers() {
    use dc_client::hud::msgs::{read_match_begin, read_shell, ObjectNote, ShellRead};
    let Some(shot) = captures() else {
        return;
    };
    let root = shot
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    for (name, want_cases) in [
        ("20261025_065016_tdm6", true),
        ("20261025_125432_rust6", false),
    ] {
        let path = root.join(name).join("capture.jsonl");
        let Ok(d) = dc_botclient::capdecode::decode_file(&path) else {
            println!("skipped {name}: no capture");
            continue;
        };
        let mut counts = std::collections::BTreeMap::<u8, usize>::new();
        let mut countdowns = 0;
        for m in d.msgs.iter().filter(|m| !m.c2s) {
            for inner in dc_botclient::capdecode::unbundle(&m.bits) {
                let b = inner.as_bytes();
                if b.first() != Some(&13) || inner.bit_len() < 16 {
                    continue;
                }
                let shell = b[1];
                let mut r = dc_wire::udp::BitReader::with_bit_len(b, inner.bit_len());
                r.skip_bits(16);
                let body = r.read_bit_string(r.remaining_bits());
                match shell {
                    0x45 => {
                        let mut br = body.reader();
                        let mb = read_match_begin(&mut br, Some(1));
                        assert_eq!(br.remaining_bits(), 0, "{name} 0x45 read to the end");
                        let intro = mb.intro.expect("intro block");
                        assert!(intro.head.ends_with(".dtx") && intro.body.ends_with(".dtx"));
                        assert!(intro.bases.is_some());
                        *counts.entry(shell).or_default() += 1;
                    }
                    0x46 | 0x48 | 0x3f | 0xd6 => {
                        let got = read_shell(shell, body.as_bytes(), body.bit_len(), Some(1));
                        assert!(got.is_some(), "{name} 0x{shell:02x} parses");
                        *counts.entry(shell).or_default() += 1;
                    }
                    0xa6 => {
                        if let Some(ShellRead::Note(ObjectNote::Countdown { .. })) =
                            read_shell(shell, body.as_bytes(), body.bit_len(), Some(1))
                        {
                            countdowns += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        println!("{name}: {counts:?}, case countdown notes {countdowns}");
        assert!(counts.get(&0x45).copied().unwrap_or(0) >= 1);
        if want_cases {
            assert!(counts.get(&0xd6).copied().unwrap_or(0) >= 6);
            assert!(countdowns > 10);
        } else {
            assert!(counts.get(&0x46).copied().unwrap_or(0) >= 1);
            assert!(counts.get(&0x3f).copied().unwrap_or(0) >= 1);
        }
    }
}

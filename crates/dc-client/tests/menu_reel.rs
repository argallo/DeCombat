//! CRATE-1 / CRATE-5: a Camo Crate in the inbox shows its own name and the pictures of the ROM case it looks like; with a
//! pack's reel on, opening it spins a strip of the loaded camos that stops on the award, then the award window opens with
//! the crate's name as its title. Its own test binary: the variants and the reel are process-wide settings.
//!
//! ROM-gated (skipped without `CA_ROM`). `DC_AWARD_SHOT=<dir>` saves the frames.

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{InboxRow, LinkEvent, LinkOptions};
use dc_client::menu::reel::{self, ReelSettings};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::steps::{resolve, Resolved};
use std::time::{Duration, Instant};

fn menu() -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    Some(Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    }))
}

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}");
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

fn texts(items: &[DrawItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn shot(m: &mut Menu, name: &str) {
    if let Some(dir) = std::env::var_os("DC_AWARD_SHOT") {
        let i = m.image();
        let f = dc_render::FrameImage {
            width: i.width,
            height: i.height,
            rgba: i.rgba,
        };
        let _ = f.save_png(&std::path::Path::new(&dir).join(format!("{name}.png")));
    }
}

#[test]
fn rom_camo_crate_spins_over_the_loaded_camos_and_opens_with_its_own_title() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.detach_link();
    if !m.cat.paths.award.complete() {
        eprintln!("skipped: the ROM's shell lacks the award window literals");
        return;
    }
    // two camos of the G36E and the L96A1, as a curated set would load them
    let content = dc_bute::modpatch::WeaponContent::from_json(concat!(
        r#"{"version":1,"weapons":["#,
        r#"{"weapon":502,"op":"register","fields":{"variant_of":"25","variant_name":"G36E Galaxy","variant_rarity":"legendary"}},"#,
        r#"{"weapon":503,"op":"register","fields":{"variant_of":"27","variant_name":"L96A1 Toxic","variant_rarity":"rare"}}"#,
        r#"],"ammo":[]}"#
    ))
    .unwrap();
    dc_client::camo::set_content(Some(&content));
    reel::set(Some(ReelSettings { ms: 6_000 }));
    let product = dc_bute::camo_crate::PRODUCT as u32;
    m.data.store.inbox = vec![InboxRow {
        entry: 61,
        product,
        received: 134_374_736_000_000_000,
        ..InboxRow::default()
    }];
    m.data.store.inbox_total = 1;
    m.data.store.inbox_fetched = true;
    m.open(Go::Lobby);
    click(&mut m, "lobby.inbox");
    let page = texts(&m.items());
    shot(&mut m, "crate_inbox");
    assert!(
        page.iter().any(|t| t.contains(dc_bute::camo_crate::NAME)),
        "the inbox row names the crate: {page:?}"
    );
    // CRATE-10: its detail panel lists the odds of each tier it holds (Regular 70, Rare 8, Legendary 1 of 79)
    for want in [
        "Regular gun x2",
        "  88.6%",
        "Rare camo",
        "  10.1%",
        "Legendary camo",
        "  1.3%",
    ] {
        assert!(page.iter().any(|t| t == want), "{want}: {page:?}");
    }
    m.input(MenuInput::Command(1027));
    assert!(m.popup().is_some(), "the open question");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("InboxOpen(61, {product})"));
    m.feed_event(LinkEvent::InboxOpened(dc_client::menu::link::OpenReply {
        ok: true,
        award_item: 502,
        days: 0,
        award_uid: 4243,
        ..Default::default()
    }));
    m.update();
    let a = m
        .popup()
        .and_then(|p| p.award.clone())
        .expect("award window");
    assert_eq!(a.own_title.as_deref(), Some(dc_bute::camo_crate::NAME));
    assert_eq!(a.name, "G36E Galaxy");
    let r = a.reel.clone().expect("the pack's reel");
    assert_eq!(r.cells[reel::WINNER].item, 502);
    // CRATE-10: the camos and their plain guns, mostly plain guns as the odds are; a plain gun has no tier
    assert!(r.cells.iter().all(|c| [25, 27, 502, 503].contains(&c.item)));
    assert!(r
        .cells
        .iter()
        .all(|c| c.rarity.is_some() == (c.item >= 500)));
    let plain = r.cells.iter().filter(|c| c.item < 500).count();
    assert!(plain > r.cells.len() / 2, "{plain} plain cells");
    assert_eq!(r.tier(), dc_bute::modpatch::Rarity::Legendary);
    assert_eq!(r.title, dc_bute::camo_crate::NAME);
    // the spin: the strip, its names and no award window yet; OK does nothing, it stops the strip
    let early = texts(&m.items());
    shot(&mut m, "crate_reel_start");
    assert!(
        early
            .iter()
            .any(|t| ["L96A1 Toxic", "G36E Galaxy", "L96A1", "G36E"].contains(&t.as_str())),
        "{early:?}"
    );
    assert!(!early.contains(&a.period), "no window during the spin");
    std::thread::sleep(Duration::from_millis(1_200));
    m.tick_popup_at(Instant::now());
    shot(&mut m, "crate_reel_mid");
    assert!(
        m.pending_sounds()
            .contains(&dc_client::menu::frontfx::SOUND_SELECT_CHANGE),
        "ticks while cells pass the mark"
    );
    let rom_cue = |cues: &[dc_client::sound::SoundCue], what: &str| {
        cues.iter().any(|c| {
            matches!(c, dc_client::sound::SoundCue::Interface { file: Some(_), what: w } if *w == what)
        })
    };
    // CRATE-7: the bomb run's countdown beeps from 10 % of the spin
    let mid = m.take_sound_cues();
    assert!(rom_cue(&mid, "start_count"), "{mid:?}");
    m.input(MenuInput::Key(MenuKey::Enter));
    let p = m.popup().expect("still open");
    assert!(
        p.award.as_ref().is_some_and(|a| a.spinning(Instant::now())),
        "skipped, holding"
    );
    let hold = p
        .award
        .as_ref()
        .and_then(|a| a.reel.as_ref())
        .map(|r| r.hold_after_stop())
        .unwrap();
    let stopped = Instant::now();
    // a Legendary: lights out, then the hit: the gun's own model leaps out and turns (CRATE-8), with the flash, brass and
    // searchlights (a frame takes a while to save, so each one waits for its moment and says when it was taken)
    let at = |m: &mut Menu, ms: u64, name: &str| {
        let due = stopped + Duration::from_millis(ms);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        m.tick_popup_at(Instant::now());
        eprintln!(
            "{name}: {} ms after the stop",
            stopped.elapsed().as_millis()
        );
        shot(m, name);
    };
    at(&mut m, 450, "crate_reel_hush");
    at(&mut m, reel::HUSH_MS + 190 + 150, "crate_reel_leap");
    at(&mut m, reel::HUSH_MS + 190 + 900, "crate_reel_show");
    // CRATE-7: the game's own sounds: a silent dark (the run ended with the stop), the gun fired and racked, the
    // match-win sound
    let cues = m.take_sound_cues();
    let gun = |k: u8| {
        cues.iter().any(|c| {
            matches!(c, dc_client::sound::SoundCue::OwnWeapon { k: kk, weapon: 502 } if *kk == k)
        })
    };
    assert!(
        gun(reel::WEAPON_FIRE) && gun(reel::WEAPON_SELECT),
        "{cues:?}"
    );
    assert!(
        !rom_cue(&cues, "start_count") && rom_cue(&cues, "match_won"),
        "{cues:?}"
    );
    // CRATE-8: the won gun's own model is drawn, from its own view
    let shown = m.items();
    assert!(
        shown.iter().any(
            |i| matches!(i, DrawItem::Image { tex, .. } if tex.starts_with("#case-showcase/"))
        ),
        "the showcase render is on the screen"
    );
    shot(&mut m, "crate_reel_show2");
    at(&mut m, reel::HUSH_MS + 190 + 2_400, "crate_reel_turned");
    // after the hold the window opens at once with its parts, titled with the crate's name
    m.tick_popup_at(stopped + hold + Duration::from_millis(50));
    let after = texts(&m.items());
    shot(&mut m, "crate_award");
    assert!(
        after.contains(&dc_bute::camo_crate::NAME.to_string()),
        "{after:?}"
    );
    assert!(after.contains(&a.period), "{after:?}");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.popup().is_none(), "OK closes the window");
    reel::set(None);
    dc_client::camo::set_content(None);
}

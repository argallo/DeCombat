//! The menu's side of a newer client (CLI-1863..CLI-1866; project decision, no 2009 counterpart): the box of an offer
//! ([`crate::selfupdate::Offer`]: a newer release, a refusal of a too old one, a friend pack's newer build) with
//! "Update now" / "Open page" / "Later", the progress box while an update downloads and unpacks, the client's quit
//! once the helper waits for it, and the failure box that falls back to the download page. A friend pack with an
//! update folder also gets a "Check update" button on the login screen and a look into the folder at the first
//! login screen of a run. The boxes are the 2009 message box with our own words and button captions; they open only
//! when no other box is up.
// DC-DEFAULT: CLI-1863 (proposed, USER REQUEST 2026-11-04) the offer box and its buttons; CLI-1866 the friend pack's
// check at the first login screen and by its button

use super::link::LinkCmd;
use super::popup::{BoxTag, Popup};
use super::{Menu, MenuOutcome};
use crate::selfupdate::{self, Action, Offer, Phase};
use std::sync::atomic::{AtomicBool, Ordering};

/// Which of our boxes is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A newer client: update / page / later.
    Offer,
    /// The update runs (a wait box with its progress).
    Working,
    /// The update failed: page / close.
    Failed,
}

pub const T_UPDATE: &str = "Update now";
pub const T_DOWNLOAD: &str = "Download";
pub const T_PAGE: &str = "Open page";
pub const T_LATER: &str = "Later";
pub const T_CLOSE: &str = "Close";
pub const T_CHECK: &str = "Check update";
/// The login screen's "Check update" button (a friend pack with an update folder only).
pub const CMD_CHECK: i32 = 110_301;

/// The menu's state of an update: the offer its boxes answer.
#[derive(Debug, Default)]
pub struct UpdateUi {
    offer: Option<Offer>,
}

/// The buttons of an offer's box (empty: an OK box, nothing to do but read it).
pub fn captions(o: &Offer) -> Vec<&'static str> {
    let last = if o.too_old { T_CLOSE } else { T_LATER };
    let page = !o.page.is_empty();
    match (selfupdate::action_for(o), page) {
        (Action::Swap, true) => vec![T_UPDATE, T_PAGE, last],
        (Action::Swap, false) => vec![T_UPDATE, last],
        (Action::Download, _) => vec![T_DOWNLOAD, T_PAGE, last],
        (Action::PageOnly, true) => vec![T_PAGE, last],
        (Action::PageOnly, false) => vec![],
    }
}

/// The box of an offer.
pub fn offer_box(o: &Offer) -> Popup {
    let c = captions(o);
    if c.is_empty() {
        return Popup::ok(o.text.clone(), BoxTag::None);
    }
    Popup::choice(o.text.clone(), BoxTag::Update(Step::Offer), &c)
}

/// The wait box's line: "Downloading the update: 3.1 MB of 41.0 MB".
pub fn progress_text(what: &str, done: u64, total: u64) -> String {
    use super::modcache::size_words;
    match (done, total) {
        (0, _) => format!("{what}..."),
        (d, 0) => format!("{what}: {}", size_words(d)),
        (d, t) => format!("{what}: {} of {}", size_words(d), size_words(t)),
    }
}

fn push(m: &mut Menu, p: Popup) {
    m.popups.push(p);
    m.dirty = true;
}

fn failed_box(m: &Menu, why: &str) -> Popup {
    let text = format!("The update did not work: {why}");
    match m.self_update.offer.as_ref().filter(|o| !o.page.is_empty()) {
        Some(_) => Popup::choice(
            format!("{text} The download page has the new version too."),
            BoxTag::Update(Step::Failed),
            &[T_PAGE, T_CLOSE],
        ),
        None => Popup::ok(text, BoxTag::None),
    }
}

/// Every menu frame: the progress of a running update, its end (quit, saved, failed), then a waiting offer or the
/// last update's failure when no box is up.
pub fn tick(m: &mut Menu) {
    if let Phase::Busy { what, done, total } = selfupdate::phase() {
        let text = progress_text(&what, done, total);
        match m
            .popups
            .iter_mut()
            .find(|p| p.tag == BoxTag::Update(Step::Working))
        {
            Some(p) => {
                if p.text != text {
                    p.text = text;
                    m.dirty = true;
                }
            }
            None => {
                let mut w = Popup::wait(text);
                w.tag = BoxTag::Update(Step::Working);
                push(m, w);
            }
        }
        return;
    }
    if let Some(end) = selfupdate::take_ended() {
        m.popups.retain(|p| p.tag != BoxTag::Update(Step::Working));
        m.dirty = true;
        match end {
            // the helper waits for this process: the client ends as by its quit question
            Phase::Staged => {
                m.trail.push("update: quit for the swap".into());
                m.link.send(LinkCmd::Logout);
                m.quit = true;
                m.outcome = Some(MenuOutcome::Quit);
            }
            Phase::Saved(path) => push(
                m,
                Popup::ok(
                    format!(
                        "The new version is in {}. Unzip it and put DeCombat in place of the old one.",
                        path.display()
                    ),
                    BoxTag::None,
                ),
            ),
            Phase::Failed(why) => {
                eprintln!("dc-client update: {why}");
                let p = failed_box(m, &why);
                push(m, p);
            }
            Phase::Note(text) => push(m, Popup::ok(text, BoxTag::None)),
            Phase::Idle | Phase::Busy { .. } => {}
        }
        return;
    }
    if !m.popups.is_empty() || m.fade.is_some() || m.outcome.is_some() {
        return;
    }
    if let Some(why) = selfupdate::take_last_failure() {
        push(
            m,
            Popup::ok(format!("The last update did not work: {why}"), BoxTag::None),
        );
        return;
    }
    if let Some(o) = selfupdate::take_offer() {
        let p = offer_box(&o);
        m.self_update.offer = Some(o);
        push(m, p);
    }
}

/// The answer of one of our boxes (`caption`: the button's, `None` for Escape).
pub fn answer(m: &mut Menu, step: Step, caption: Option<&str>) {
    let Some(o) = m.self_update.offer.clone() else {
        return;
    };
    match (step, caption) {
        (Step::Offer, Some(T_UPDATE | T_DOWNLOAD)) => {
            if !selfupdate::start(o) {
                push(
                    m,
                    Popup::ok("An update is running already.".into(), BoxTag::None),
                );
            }
        }
        (Step::Offer | Step::Failed, Some(T_PAGE)) => {
            if let Err(e) = selfupdate::open_url(&o.page) {
                push(
                    m,
                    Popup::ok(format!("{e}. The page is {}", o.page), BoxTag::None),
                );
            }
        }
        _ => {}
    }
}

/// The caption of the button that closed `p` (`None`: Escape).
pub fn chosen_caption(p: &Popup) -> Option<String> {
    p.chosen
        .and_then(|i| p.own_captions.as_ref().and_then(|c| c.get(i).cloned()))
}

/// The friend pack's look into its update folder at the first login screen of a run.
pub fn check_at_start() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if selfupdate::local_updates() && !DONE.swap(true, Ordering::Relaxed) {
        selfupdate::start_check(false);
    }
}

/// The login screen's "Check update" button (bottom right), for a friend pack with an update folder; none else.
pub fn check_button() -> super::modbrowser::Buttons {
    use super::modbrowser::{Button, ButtonKind, Buttons, BUTTON};
    let mut b = Buttons::default();
    if selfupdate::local_updates() {
        b.list = vec![Button {
            cmd: CMD_CHECK,
            at: [1024 - BUTTON[0] - 16, 768 - BUTTON[1] - 16],
            size: BUTTON,
            label: T_CHECK.into(),
            on: true,
            kind: ButtonKind::Art,
        }];
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selfupdate::{Pack, Source, OPENED, PACK_OVERRIDE};
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// The offer, the pack override and the phase are one per process: the tests take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn menu() -> Menu {
        let mut m = Menu::new(super::super::MenuOptions {
            auth: "127.0.0.1:1".parse().unwrap(),
            user: "tester".into(),
            link: super::super::link::LinkOptions::new("update-box"),
            rom: None,
            font: Some((crate::testkit::synth_font(), 0)),
            width: 1024,
            height: 768,
        });
        m.detach_link();
        m
    }

    fn release(too_old: bool) -> Offer {
        Offer {
            text: "DeCombat 0.4.0 is out. You have 0.3.0.".into(),
            too_old,
            page: "https://example.org/releases".into(),
            source: Source::Release {
                have: [0, 3, 0],
                zip: None,
            },
        }
    }

    fn click(m: &mut Menu, i: usize) {
        let r = m.popup().expect("a box").buttons()[i];
        let at = [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32];
        m.input(super::super::MenuInput::Move(at));
        m.input(super::super::MenuInput::Press(at));
        m.input(super::super::MenuInput::Release(at));
        m.update();
    }

    #[test]
    fn the_boxes_offer_what_this_computer_can_do() {
        let o = release(false);
        let want: Vec<&str> = match selfupdate::platform() {
            selfupdate::Platform::Windows => vec![T_UPDATE, T_PAGE, T_LATER],
            selfupdate::Platform::MacOs => vec![T_DOWNLOAD, T_PAGE, T_LATER],
            selfupdate::Platform::Other => vec![T_PAGE, T_LATER],
        };
        assert_eq!(captions(&o), want);
        assert_eq!(captions(&release(true)).last(), Some(&T_CLOSE));
        let local = Offer {
            text: "x".into(),
            too_old: false,
            page: String::new(),
            source: Source::Local {
                zip: PathBuf::from("x.zip"),
                commit: "abcd1234".into(),
            },
        };
        if cfg!(windows) {
            assert_eq!(captions(&local), vec![T_UPDATE, T_LATER]);
        } else {
            assert!(captions(&local).is_empty());
        }
        assert_eq!(
            progress_text("Downloading the update", 0, 0),
            "Downloading the update..."
        );
        assert_eq!(
            progress_text("Downloading the update", 2048, 4096),
            "Downloading the update: 2 KB of 4 KB"
        );
    }

    // CLI-1863: the offer waits for a free moment, shows its box, and "Open page" hands the page to the browser
    #[test]
    fn the_offer_box_opens_the_page_and_later_closes_it() {
        let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = menu();
        while m.fade.is_some() {
            m.update();
        }
        selfupdate::post_offer(release(true));
        m.update();
        let p = m.popup().expect("the offer box");
        assert_eq!(p.tag, BoxTag::Update(Step::Offer));
        assert!(p.text.contains("0.4.0"));
        let page_at = captions(&release(true))
            .iter()
            .position(|c| *c == T_PAGE)
            .unwrap();
        OPENED.lock().unwrap().clear();
        click(&mut m, page_at);
        assert!(m.popup().is_none());
        assert_eq!(
            *OPENED.lock().unwrap(),
            vec!["https://example.org/releases".to_string()]
        );
        // Close: nothing happens, no update starts
        selfupdate::post_offer(release(true));
        m.update();
        let close_at = captions(&release(true)).len() - 1;
        click(&mut m, close_at);
        assert!(m.popup().is_none());
        assert_eq!(selfupdate::phase(), Phase::Idle);
    }

    // CLI-1863: a failed update falls back to the page with its reason; a finished staging quits the client
    #[test]
    fn a_failure_offers_the_page_and_a_staging_quits() {
        let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = menu();
        while m.fade.is_some() {
            m.update();
        }
        m.self_update.offer = Some(release(false));
        *selfupdate::tests_phase() = Phase::Busy {
            what: "Downloading the update".into(),
            done: 1024,
            total: 4096,
        };
        m.update();
        let w = m.popup().expect("the progress box");
        assert_eq!(w.tag, BoxTag::Update(Step::Working));
        assert!(w.text.contains("1 KB of 4 KB"), "{}", w.text);
        *selfupdate::tests_phase() = Phase::Failed("the download answered 404".into());
        m.update();
        let f = m.popup().expect("the failure box");
        assert_eq!(f.tag, BoxTag::Update(Step::Failed));
        assert!(f.text.contains("404"), "{}", f.text);
        OPENED.lock().unwrap().clear();
        click(&mut m, 0);
        assert_eq!(
            *OPENED.lock().unwrap(),
            vec!["https://example.org/releases".to_string()]
        );
        *selfupdate::tests_phase() = Phase::Staged;
        m.update();
        assert!(
            m.trail.iter().any(|t| t.starts_with("update: quit")),
            "{:?}",
            m.trail
        );
        assert_eq!(selfupdate::phase(), Phase::Idle);
    }

    // CLI-1866: the "Check update" button exists only for a friend pack with an update folder
    #[test]
    fn the_check_button_is_a_friend_packs_only() {
        let _t = TURN.lock().unwrap_or_else(|e| e.into_inner());
        *PACK_OVERRIDE.lock().unwrap() = Some(None);
        assert!(check_button().list.is_empty());
        *PACK_OVERRIDE.lock().unwrap() = Some(Some(Pack {
            commit: "abcd1234".into(),
            update_dir: None,
            ..Default::default()
        }));
        assert!(check_button().list.is_empty());
        *PACK_OVERRIDE.lock().unwrap() = Some(Some(Pack {
            commit: "abcd1234".into(),
            update_dir: Some(PathBuf::from("C:/nowhere")),
            ..Default::default()
        }));
        assert_eq!(check_button().list.len(), 1);
        *PACK_OVERRIDE.lock().unwrap() = None;
    }
}

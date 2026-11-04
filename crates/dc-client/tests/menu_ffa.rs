//! FFA leftovers (CLI-781, CLI-782): the menu result page of a One Man Army match shows no team score pair, and
//! the waiting room of a class-6 room refuses a team change.
//!
//! ROM-gated (skipped without `CA_ROM`).

use dc_client::matchexit::MatchResult;
use dc_client::menu::catalog::Catalog;
use dc_client::menu::result::{ResultScreen, SCORE_LEFT, SCORE_RIGHT, WINDOW_TEAMS};
use dc_client::menu::MenuData;

fn screen(class: Option<u8>) -> Option<ResultScreen> {
    let rom = dc_rom::test_rom()?;
    let cat = Catalog::from_rom(&rom, 1024, 768).expect("catalog");
    let mut r = MatchResult::sample();
    r.mode_class = class;
    let data = MenuData {
        result: Some(r),
        ..MenuData::default()
    };
    Some(ResultScreen::new(&cat, &data))
}

/// CLI-781: cshell+0x132b80 hides window 0 controls 1..6 (the two teams' score digits) for classes 6 / 9 / 0xb.
#[test]
fn rom_ffa_result_page_hides_the_team_score_pair() {
    let Some(team) = screen(Some(1)) else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let ffa = screen(Some(6)).unwrap();
    for id in SCORE_LEFT.iter().chain(SCORE_RIGHT.iter()) {
        assert_eq!(
            team.image_shown(WINDOW_TEAMS, *id),
            Some(true),
            "team mode digit {id}"
        );
        assert_eq!(
            ffa.image_shown(WINDOW_TEAMS, *id),
            Some(false),
            "FFA digit {id} shown"
        );
    }
}

//! CLI-1940: the wait box's line while a join downloads a server's packs (a curated entry has dozens; the first join
//! took minutes and the plain "connecting" box looked like a hang).
//!
//! The menu shows the join's [`ModState`] in whichever wait box is open (login, server pick, entry switch); when the
//! join leaves the download the box gets its own text back.

use super::modcache::size_words;
use crate::mods::ModState;

// DC-DEFAULT: CLI-1940 (proposed, project decision) the texts below
const READING: &str = "Reading the server's pack list...";
const CHECKING: &str = "Checking the packs...";
const LOADING: &str = "Loading the packs...";

/// The wait box line for `state`; `None` = the box keeps (or gets back) its own text. `Failed` and `NeedsConsent`
/// belong to the screens (an error box, the consent prompt).
pub fn wait_text(state: &ModState) -> Option<String> {
    match state {
        ModState::FetchingSet => Some(READING.into()),
        ModState::Downloading {
            index,
            count,
            done,
            total,
        } => Some(download_text(*index, *count, *done, *total)),
        ModState::Verifying => Some(CHECKING.into()),
        ModState::Loading => Some(LOADING.into()),
        ModState::Idle | ModState::Ready | ModState::Failed(_) | ModState::NeedsConsent(_) => None,
    }
}

/// "Downloading server packs: 3 of 65 (12.4 MB of 90.1 MB)"; a total of 0 shows the bytes so far, no bytes yet
/// shows the count alone.
pub fn download_text(index: usize, count: usize, done: u64, total: u64) -> String {
    let head = format!("Downloading server packs: {index} of {count}");
    match (done, total) {
        (0, _) => head,
        (d, 0) => format!("{head} ({} so far)", size_words(d)),
        (d, t) => format!("{head} ({} of {})", size_words(d), size_words(t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_download_line_names_the_pack_and_the_bytes() {
        assert_eq!(
            download_text(1, 65, 0, 0),
            "Downloading server packs: 1 of 65"
        );
        assert_eq!(
            download_text(3, 65, 3 * 1024 * 1024, 0),
            "Downloading server packs: 3 of 65 (3.0 MB so far)"
        );
        assert_eq!(
            download_text(1, 1, 1024, 2048),
            "Downloading server packs: 1 of 1 (1 KB of 2 KB)"
        );
    }

    #[test]
    fn only_the_working_states_change_the_box() {
        assert!(wait_text(&ModState::FetchingSet).is_some());
        assert!(wait_text(&ModState::Verifying).is_some());
        assert!(wait_text(&ModState::Loading).is_some());
        assert!(wait_text(&ModState::Ready).is_none());
        assert!(wait_text(&ModState::Idle).is_none());
        assert!(wait_text(&ModState::Failed("x".into())).is_none());
    }
}

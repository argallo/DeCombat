//! The review list of curated packs (`community/vanilla_plus.json`, LG-315): what the reviewer accepted, one row per
//! pack, and the config lines of a featured entry made from it.
//!
//! ```json
//! {"version": 1, "packs": [{"id": "my_pack", "version": "1.0.0", "sha256": "<64 hex>",
//!   "source_repo": "https://github.com/someone/my_pack", "reviewer_note": "...",
//!   "release": "https://github.com/someone/my_pack/releases/tag/v1"}]}
//! ```
//!
//! `release` (optional, LG-315 amended) is the GitHub release page the zip was published on
//! (docs/PACKS_FROM_REPOSITORIES.md section 3); the asset there is `<id>-<version>.zip`.
// DC-DEFAULT: LG-315 (proposed, project decision curated packs reviewed by the user alone)

use crate::directory::{check_mod_ref, is_http_url};
use crate::schema::{is_release_page, ModRef};
use crate::Issue;
use serde::Deserialize;
use std::collections::BTreeSet;

/// Longest reviewer note, in characters.
pub const MAX_NOTE_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedPack {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub source_repo: String,
    pub reviewer_note: String,
    /// The release page the zip was published on (`https://github.com/<owner>/<repo>/releases/tag/<tag>`), when it
    /// came from a pack repository's release; a pack with no release (a map exported by hand) leaves it out.
    #[serde(default)]
    pub release: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewList {
    pub version: u32,
    pub packs: Vec<ReviewedPack>,
}

/// Reads and checks a list; `Err` holds every problem.
pub fn load(text: &str) -> Result<ReviewList, Vec<Issue>> {
    let list: ReviewList = serde_json::from_str(text).map_err(|e| {
        vec![Issue::error(
            "vanilla_plus",
            format!("not a review list: {e}"),
        )]
    })?;
    let issues = validate(&list);
    if issues.is_empty() {
        Ok(list)
    } else {
        Err(issues)
    }
}

/// Every problem of a list.
pub fn validate(list: &ReviewList) -> Vec<Issue> {
    let mut out = Vec::new();
    if list.version != 1 {
        out.push(Issue::error("version", "must be 1"));
    }
    let mut seen = BTreeSet::new();
    for (i, p) in list.packs.iter().enumerate() {
        let path = format!("packs[{i}]");
        let r = ModRef {
            id: p.id.clone(),
            version: p.version.clone(),
            sha256: p.sha256.clone(),
        };
        out.extend(check_mod_ref(&path, &r));
        if !seen.insert(p.id.as_str()) {
            out.push(Issue::error(
                format!("{path}.id"),
                format!("pack '{}' listed twice", p.id),
            ));
        }
        if !is_http_url(&p.source_repo) {
            out.push(Issue::error(
                format!("{path}.source_repo"),
                "must be an http(s) URL",
            ));
        }
        let note = p.reviewer_note.trim().chars().count();
        if note == 0 || note > MAX_NOTE_CHARS {
            out.push(Issue::error(
                format!("{path}.reviewer_note"),
                format!("must say what was reviewed, 1..{MAX_NOTE_CHARS} characters"),
            ));
        }
        if let Some(r) = &p.release {
            if !is_release_page(r) {
                out.push(Issue::error(
                    format!("{path}.release"),
                    "must be a GitHub release page, https://github.com/<owner>/<repo>/releases/tag/<tag> (a tag, never a branch)",
                ));
            }
        }
    }
    out
}

/// The release pages the list names, each once, in first-seen order (what an operator syncs from).
pub fn release_pages(list: &ReviewList) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in &list.packs {
        if let Some(r) = &p.release {
            if !out.contains(r) {
                out.push(r.clone());
            }
        }
    }
    out
}

/// The `mods = [...]` lines of a featured `[[lobby.servers]]` entry.
pub fn config_lines(list: &ReviewList) -> String {
    let mut s = String::from("mods = [\n");
    for p in &list.packs {
        s.push_str(&format!(
            "  {{ id = \"{}\", version = \"{}\", sha256 = \"{}\" }},\n",
            p.id, p.version, p.sha256
        ));
    }
    s.push_str("]\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(sha: &str, repo: &str, note: &str) -> String {
        format!(
            r#"{{"version":1,"packs":[{{"id":"my_pack","version":"1.0.0","sha256":"{sha}","source_repo":"{repo}","reviewer_note":"{note}"}}]}}"#
        )
    }

    #[test]
    fn a_good_list_loads_and_makes_config_lines_that_parse() {
        let sha = "ab".repeat(32);
        let list = load(&doc(&sha, "https://example.org/p", "reference gun: M4A1")).unwrap();
        let lines = config_lines(&list);
        #[derive(Deserialize)]
        struct E {
            mods: Vec<ModRef>,
        }
        let e: E = toml::from_str(&lines).unwrap();
        assert_eq!(e.mods.len(), 1);
        assert_eq!(e.mods[0].sha256, sha);
    }

    #[test]
    fn the_shipped_list_is_valid() {
        let text = include_str!("../../../community/vanilla_plus.json");
        let list = load(text).unwrap();
        // LG-315 amended: our packs name the mods repository's release; the two hand-exported maps name none
        let pages = release_pages(&list);
        assert_eq!(
            pages,
            ["https://github.com/argallo/DeCombat-mods/releases/tag/v1"]
        );
        let without: Vec<&str> = list
            .packs
            .iter()
            .filter(|p| p.release.is_none())
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(without, ["shipping_container", "freight_hall"]);
    }

    #[test]
    fn a_release_is_a_github_release_page_with_a_tag_never_a_branch() {
        let sha = "ab".repeat(32);
        let with = |r: &str| {
            format!(
                r#"{{"version":1,"packs":[{{"id":"my_pack","version":"1.0.0","sha256":"{sha}","source_repo":"https://example.org/p","reviewer_note":"n","release":"{r}"}}]}}"#
            )
        };
        let good = load(&with("https://github.com/someone/my_pack/releases/tag/v1")).unwrap();
        assert_eq!(
            good.packs[0].release.as_deref(),
            Some("https://github.com/someone/my_pack/releases/tag/v1")
        );
        assert_eq!(
            release_pages(&good),
            ["https://github.com/someone/my_pack/releases/tag/v1"]
        );
        for bad in [
            "https://github.com/someone/my_pack/releases/tag/main",
            "https://github.com/someone/my_pack/releases/tag/latest",
            "https://github.com/someone/my_pack/releases",
            "https://github.com/someone/my_pack/releases/tag/v1/extra",
            "https://github.com/someone/releases/tag/v1",
            "http://github.com/someone/my_pack/releases/tag/v1",
            "https://gitlab.com/someone/my_pack/releases/tag/v1",
            "",
        ] {
            let e = load(&with(bad)).unwrap_err();
            assert_eq!(e.len(), 1, "{bad}: {e:?}");
            assert_eq!(e[0].path, "packs[0].release", "{bad}");
        }
        // a row without the field is as before
        assert!(load(&doc(&sha, "https://example.org/p", "n"))
            .unwrap()
            .packs[0]
            .release
            .is_none());
    }

    #[test]
    fn bad_rows_are_refused() {
        let sha = "ab".repeat(32);
        let fields = |t: &str| {
            load(t)
                .unwrap_err()
                .iter()
                .map(|i| i.path.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            fields(&doc("nothex", "https://example.org/p", "n")),
            ["packs[0].sha256"]
        );
        assert_eq!(fields(&doc(&sha, "ftp://x", "n")), ["packs[0].source_repo"]);
        assert_eq!(
            fields(&doc(&sha, "https://example.org/p", " ")),
            ["packs[0].reviewer_note"]
        );
        assert!(load(r#"{"version":1,"packs":[{"id":"a"}]}"#).is_err());
        assert!(load(r#"{"version":1,"packs":[],"extra":1}"#).is_err());
        let two = format!(
            r#"{{"version":1,"packs":[{0},{0}]}}"#,
            format_args!(
                r#"{{"id":"a","version":"1","sha256":"{sha}","source_repo":"https://e.org","reviewer_note":"n"}}"#
            )
        );
        assert_eq!(fields(&two), ["packs[1].id"]);
    }
}

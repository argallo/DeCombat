//! The review list of curated packs (`community/vanilla_plus.json`, LG-315): what the reviewer accepted, one row per
//! pack, and the config lines of a featured entry made from it.
//!
//! ```json
//! {"version": 1, "packs": [{"id": "my_pack", "version": "1.0.0", "sha256": "<64 hex>",
//!   "source_repo": "https://github.com/someone/my_pack", "reviewer_note": "..."}]}
//! ```
// DC-DEFAULT: LG-315 (proposed, project decision curated packs reviewed by the user alone)

use crate::directory::{check_mod_ref, is_http_url};
use crate::schema::ModRef;
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
        assert!(load(text).is_ok());
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

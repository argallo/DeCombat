//! The committed community directory (`community/community.json`) must validate, and every mod a server
//! lists must be a verified known mod. SPEC: HOSTING_AND_MODS.md §8.2, §8.3.

use dc_config::directory::{badges, load_community, validate_community, Badge};

const FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../community/community.json"
);

#[test]
fn the_committed_directory_validates_and_its_mods_are_verified() {
    let text = std::fs::read_to_string(FILE).expect("community/community.json");
    let doc = load_community(&text).expect("parses");
    let issues = validate_community(&doc);
    assert!(issues.is_empty(), "{issues:?}");
    assert!(doc.servers.iter().any(|s| s.policy == "vanilla"));
    let mut modded = 0;
    for s in &doc.servers {
        for (m, b) in badges(s, &doc) {
            assert_eq!(b, Badge::Verified, "{} lists unverified {}", s.id, m.id);
            modded += 1;
        }
    }
    assert!(modded > 0);
}

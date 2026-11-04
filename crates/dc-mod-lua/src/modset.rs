//! The mod-set hash text, shared by the lobby (which advertises it) and the client (which sends it at login).
//!
//! SPEC: HOSTING_AND_MODS.md §4 item 6 (decided 2026-10-24): one line `id@version:sha256` followed by a line feed
//! per server-declared mod, in load order; client-visual mods are not part of it. The hash is the SHA-256 of that
//! text, so the empty set hashes to [`EMPTY_SET_HASH`], the only value a vanilla server accepts.

/// SHA-256 of the empty string: the mod-set hash of a vanilla server.
pub const EMPTY_SET_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// One entry of a mod set as the hash sees it: id, version, sha256 of the packed zip, kind (when known).
#[derive(Clone, Copy, Debug)]
pub struct HashEntry<'a> {
    pub id: &'a str,
    pub version: &'a str,
    pub sha256: &'a str,
    pub kind: Option<&'a str>,
}

/// The text that is hashed. An entry of unknown kind counts (the server declared it); `client_visual` does not.
pub fn mod_set_hash_text<'a>(entries: impl IntoIterator<Item = HashEntry<'a>>) -> String {
    let mut t = String::new();
    for e in entries {
        if e.kind == Some("client_visual") {
            continue;
        }
        t.push_str(e.id);
        t.push('@');
        t.push_str(e.version);
        t.push(':');
        t.push_str(e.sha256);
        t.push('\n');
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_text_lists_declared_mods_in_order_without_client_visual() {
        let e = |id, kind| HashEntry {
            id,
            version: "1.0.0",
            sha256: "ab",
            kind,
        };
        let t = mod_set_hash_text([
            e("a", Some("server_rules")),
            e("v", Some("client_visual")),
            e("b", None),
        ]);
        assert_eq!(t, "a@1.0.0:ab\nb@1.0.0:ab\n");
        assert_eq!(mod_set_hash_text([]), "");
    }
}

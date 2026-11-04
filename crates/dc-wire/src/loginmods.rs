//! The mod-set and content hashes of a login (LG-271, project decision). Our own design, no 2009 counterpart.
//!
//! **Carrier.** The 2009 login request 0x200 has no field for mods. Our clients append the two hashes to its
//! login-name field (+6, wstr[4096]; a name or `tok:<token>` uses at most about 2100 of its units), after the name
//! and after an optional `LOCAL:` / `tok:` form:
//!
//! ```text
//! <name | tok:<token>>;m=<mod-set hash, 64 lower-case hex>;c=<content hash, 64 lower-case hex, or 0>
//! ```
//!
//! - `m` is the mod-set hash of HOSTING_AND_MODS.md §4 item 6 (the empty-set hash [`EMPTY_SET_HASH`] on vanilla).
//! - `c` is the hash of the weapon content the client's own mods produced (`WeaponContent::hash`), or `0` when they
//!   produced none (vanilla, or a set without `mod.content.weapons`).
//! - The suffix starts at the LAST `;m=` of the field. A field without `;m=` is a client that knows nothing about
//!   mods (the original 2009 client, `dc-botclient` without `--mod-set-hash`): the empty set and no content. A `;m=`
//!   without `;c=` means content `0`.
//! - The lobby strips the suffix before any name handling (`LOCAL:`, `tok:`, account lookup), so names, tokens and
//!   logs never see it. The hashes are public (the server list publishes the set hash); they are not secrets.
//!
//! The typed-text field (+0x2006, wstr[17], the password box) is too short for a hash and stays the password.
// DC-DEFAULT: LG-271 (proposed, project decision) login carrier of the mod-set and content hashes

/// Starts the suffix: the mod-set hash follows.
pub const SET_KEY: &str = ";m=";
/// Starts the content hash.
pub const CONTENT_KEY: &str = ";c=";
/// The content hash of "no content at all".
pub const NO_CONTENT: &str = "0";
/// SHA-256 of the empty text: the hash of the empty mod set (vanilla).
pub const EMPTY_SET_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The hashes a login carries. Without a suffix: the empty set and no content ([`LoginMods::absent`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginMods {
    /// The mod-set hash as sent (lower-cased).
    pub set: String,
    /// The content hash as sent (lower-cased), [`NO_CONTENT`] when none.
    pub content: String,
    /// False when the field carried no suffix at all.
    pub sent: bool,
}

impl LoginMods {
    /// What a login without a suffix stands for.
    pub fn absent() -> LoginMods {
        LoginMods {
            set: EMPTY_SET_HASH.to_string(),
            content: NO_CONTENT.to_string(),
            sent: false,
        }
    }
}

/// The suffix a client appends to its login-name field.
pub fn suffix(set_hash: &str, content_hash: &str) -> String {
    format!("{SET_KEY}{set_hash}{CONTENT_KEY}{content_hash}")
}

/// LG-317: what a dc-botclient adds to its name, before any hash suffix. Honest clients only: it keeps a stock bot
/// out of the rewards under any name, and a changed binary can leave it out.
pub const BOT_KEY: &str = ";b=1";

/// LG-318: the prefix of the bot pool's characters (their logins are the same in lower case). A bot takes a member
/// without it for a person.
pub const POOL_NICK_PREFIX: &str = "PoolBot";
/// LG-318: login refusals of a bot seat. An empty name is refused with [`REASON_NO_POOL`] by a lobby without a pool and
/// by a lobby older than the pool alike.
pub const REASON_NO_POOL: u32 = 0x0003_0000;
pub const REASON_POOL_FULL: u32 = 0x0003_00f0;
pub const REASON_POOL_ADDRESS: u32 = 0x0003_00f1;

/// The name part of [`split`] without the bot mark, and whether the mark was there.
pub fn split_bot(name_part: &str) -> (&str, bool) {
    match name_part.strip_suffix(BOT_KEY) {
        Some(n) => (n, true),
        None => (name_part, false),
    }
}

/// Splits a login-name field into the name part and the hashes it carries.
pub fn split(field: &str) -> (&str, LoginMods) {
    let Some(at) = field.rfind(SET_KEY) else {
        return (field, LoginMods::absent());
    };
    let tail = &field[at + SET_KEY.len()..];
    let (set, content) = match tail.split_once(CONTENT_KEY) {
        Some((s, c)) => (s, c),
        None => (tail, NO_CONTENT),
    };
    (
        &field[..at],
        LoginMods {
            set: set.trim().to_ascii_lowercase(),
            content: content.trim().to_ascii_lowercase(),
            sent: true,
        },
    )
}

/// The first eight characters of a hash for a log line (`0` stays `0`).
pub fn short(hash: &str) -> &str {
    hash.get(..8).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn a_field_without_a_suffix_is_the_empty_set() {
        assert_eq!(split("LOCAL:robin"), ("LOCAL:robin", LoginMods::absent()));
        assert!(!split("robin").1.sent);
    }

    #[test]
    fn the_suffix_round_trips_for_every_login_form() {
        for name in [
            "robin",
            "LOCAL:robin",
            "tok:aGk.c2ln",
            "LOCAL:tok:aGk.c2ln",
            "a;b",
        ] {
            let field = format!("{name}{}", suffix(H, NO_CONTENT));
            let (n, m) = split(&field);
            assert_eq!(n, name);
            assert_eq!((m.set.as_str(), m.content.as_str(), m.sent), (H, "0", true));
        }
        let field = format!("x{}", suffix(EMPTY_SET_HASH, &H.to_ascii_uppercase()));
        assert_eq!(split(&field).1.content, H, "hashes are compared lower-case");
    }

    #[test]
    fn a_set_hash_without_content_means_no_content() {
        let field = format!("bob;m={H}");
        let (n, m) = split(&field);
        assert_eq!(
            (n, m.set.as_str(), m.content.as_str()),
            ("bob", H, NO_CONTENT)
        );
    }
}

//! The upper-body clip of a player character (tracker 1) by the held weapon's animation type, the stance, the
//! movement direction and the action, and the lower-body jump clip (tracker 2). The 2009 client picks no clip for
//! another character: it plays the trackers the server streams (`reports/client-anims.md` §1).
//!
//! 2009 evidence for the table's shape: the client's character animator (built at cshell+0x1980, looked up at
//! cshell+0x15220 / +0x15250, played at cshell+0x16040) indexes its upper-body entries by the weapon row's
//! `AniType` (the weapon row reader takes the key at cshell+0x17bd73), a direction class (still, forward, back,
//! left, right; the four diagonals fall onto forward / back) and an action (aim, fire, select, reload, two hit
//! reactions, the throw phases); each entry holds one clip per stance (run, walk, crouch, sprint) and a loop flag
//! (aim loops, every action plays once). Its lower-body entries are indexed by gait (walk, run, crouch, jump, ...)
//! and direction, the jump entries play once. The clip names follow one scheme for every weapon family: a family
//! prefix and a slot suffix.
//!
//! No clip name is in the tree. A clip of the user's ROM body model is classified at run time by splitting its
//! lower-case name into a prefix and a suffix and finding both by the SHA-256 key of their spelling (the form of
//! [`crate::chars::name_key`]); the keys below were made by a scratch run over the 2009 table and the ROM model,
//! never by name. The choices the reference stack streamed to the real 2009 client (captures tdm1, tdm6) follow the
//! same table (REFERENCE, `reports/dserver-anims.md` §2).
// DISASM: cshell+0x1980 (table builder: per entry the clip lookups by name through the model interface +0xa4,
// cshell+0x1600), cshell+0x15220 (upper entry index ((dir + anitype * 12) * 17 + action)), cshell+0x15250 (lower
// entry index (gait * 3 + 0x8c4) * 4 + dir), cshell+0x16040 (stance column: walk 1, crouch 2, sprint 3, else 0;
// the loop flag at entry word 5)
// DC-DEFAULT: DSV-125 (proposed) the upper-body table

use std::collections::BTreeMap;

use dc_net_engine::time::since;
use dc_net_engine::Millis;

use crate::chars::{name_key, NameKey, Rig};
use crate::game::{Dir, Gait, Motion, TRACKER_LOWER, TRACKER_UPPER};
use crate::modelobj::Tracker;

/// A clip of the composed model: combined index and length in ms.
pub type ClipRef = (u16, u32);

/// Per weapon `AniType` (the table's first index): the key of the lower-case clip-name prefix of its family, and
/// whether its fire entries follow the direction (the knife families fire one clip in every direction).
// DISASM: cshell+0x1980 (the families' entries), cshell+0x15220 (AniType = first index; rows 30, 31, 32 and 35 hold
// no player family)
// DC-DEFAULT: DSV-125 (proposed)
pub const FAMILIES: [(u8, NameKey, bool); 40] = [
    (
        0,
        ([0xf7, 0x30, 0x41, 0xbc, 0x98, 0x63, 0x1a, 0xb2], 3),
        true,
    ),
    (
        1,
        ([0xf7, 0x30, 0x41, 0xbc, 0x98, 0x63, 0x1a, 0xb2], 3),
        true,
    ),
    (
        2,
        ([0xe3, 0xdc, 0x58, 0xb2, 0xf0, 0x11, 0x78, 0x48], 3),
        true,
    ),
    (
        3,
        ([0xd0, 0x48, 0xec, 0x5f, 0x58, 0x95, 0x4a, 0x4a], 3),
        true,
    ),
    (
        4,
        ([0xef, 0xcb, 0xa5, 0x52, 0xbd, 0x72, 0xec, 0x1f], 3),
        true,
    ),
    (
        5,
        ([0x8f, 0x07, 0xec, 0x82, 0x48, 0x66, 0xb7, 0x68], 3),
        true,
    ),
    (
        6,
        ([0x5d, 0xb2, 0xb9, 0x3e, 0x58, 0x67, 0xbc, 0xdc], 3),
        true,
    ),
    (
        7,
        ([0xa0, 0x55, 0x72, 0x58, 0x62, 0x32, 0xa7, 0x20], 2),
        false,
    ),
    (
        8,
        ([0x28, 0x9c, 0x90, 0xde, 0xcf, 0x40, 0xc8, 0xec], 2),
        false,
    ),
    (
        9,
        ([0xe6, 0x18, 0x4c, 0xe1, 0x0e, 0x26, 0x61, 0x34], 2),
        false,
    ),
    (
        10,
        ([0x75, 0xa2, 0x88, 0xc0, 0xd6, 0x89, 0x8c, 0x5f], 2),
        true,
    ),
    (
        11,
        ([0x46, 0x9c, 0xc6, 0x43, 0xa1, 0x9f, 0xb7, 0x59], 2),
        true,
    ),
    (
        12,
        ([0x28, 0xe5, 0xeb, 0xab, 0xd9, 0xd8, 0xf6, 0xe2], 3),
        true,
    ),
    (
        13,
        ([0xc1, 0xbc, 0xce, 0x24, 0x04, 0x4d, 0x00, 0x41], 3),
        false,
    ),
    (
        14,
        ([0x83, 0x11, 0x6a, 0xcf, 0x18, 0xe4, 0xdc, 0x44], 2),
        false,
    ),
    (
        15,
        ([0xcd, 0xee, 0x64, 0xb7, 0xf3, 0xce, 0xe4, 0x07], 2),
        true,
    ),
    (
        16,
        ([0x56, 0x4a, 0xc5, 0x8d, 0x31, 0xe6, 0x10, 0xbc], 3),
        false,
    ),
    (
        17,
        ([0x94, 0xcc, 0xa6, 0xfd, 0xcf, 0xce, 0x3c, 0x99], 3),
        true,
    ),
    (
        18,
        ([0x23, 0x97, 0x78, 0x7f, 0x6e, 0x18, 0x6b, 0xf7], 3),
        true,
    ),
    (
        19,
        ([0x6d, 0x5d, 0xdd, 0xed, 0x7f, 0x5c, 0x23, 0x50], 3),
        true,
    ),
    (
        20,
        ([0x37, 0xbf, 0xc8, 0xba, 0x5e, 0xc5, 0x61, 0x36], 3),
        true,
    ),
    (
        21,
        ([0x15, 0x4f, 0xf2, 0x7b, 0x46, 0xf3, 0x55, 0x21], 3),
        true,
    ),
    (
        22,
        ([0xa4, 0x86, 0x7d, 0xd0, 0x66, 0x35, 0xe1, 0x50], 3),
        true,
    ),
    (
        23,
        ([0xe3, 0x65, 0x90, 0xfc, 0x5c, 0xb6, 0x50, 0x7a], 3),
        true,
    ),
    (
        24,
        ([0x75, 0xa2, 0x88, 0xc0, 0xd6, 0x89, 0x8c, 0x5f], 2),
        true,
    ),
    (
        25,
        ([0xd0, 0x48, 0xec, 0x5f, 0x58, 0x95, 0x4a, 0x4a], 3),
        true,
    ),
    (
        26,
        ([0xd0, 0x48, 0xec, 0x5f, 0x58, 0x95, 0x4a, 0x4a], 3),
        true,
    ),
    (
        27,
        ([0x5d, 0xb2, 0xb9, 0x3e, 0x58, 0x67, 0xbc, 0xdc], 3),
        true,
    ),
    (
        28,
        ([0xf0, 0x76, 0x35, 0x13, 0xa2, 0xa0, 0x84, 0x47], 3),
        true,
    ),
    (
        29,
        ([0x5d, 0xb2, 0xb9, 0x3e, 0x58, 0x67, 0xbc, 0xdc], 3),
        true,
    ),
    (
        33,
        ([0xd2, 0x9f, 0xc5, 0x6e, 0xb6, 0xff, 0x00, 0xba], 3),
        true,
    ),
    (
        34,
        ([0x46, 0x9c, 0xc6, 0x43, 0xa1, 0x9f, 0xb7, 0x59], 2),
        false,
    ),
    (
        36,
        ([0xe3, 0xdc, 0x58, 0xb2, 0xf0, 0x11, 0x78, 0x48], 3),
        true,
    ),
    (
        37,
        ([0x58, 0x2d, 0x98, 0xac, 0xad, 0xe9, 0x15, 0x96], 3),
        false,
    ),
    (
        38,
        ([0x56, 0x4a, 0xc5, 0x8d, 0x31, 0xe6, 0x10, 0xbc], 3),
        true,
    ),
    (
        39,
        ([0x82, 0x5c, 0xc0, 0x91, 0x48, 0x4a, 0x96, 0x59], 3),
        false,
    ),
    (
        40,
        ([0x5a, 0x7f, 0x20, 0x68, 0x9e, 0x34, 0x7e, 0x4a], 3),
        true,
    ),
    (
        41,
        ([0xde, 0xc4, 0x75, 0x96, 0xa4, 0x73, 0xc1, 0x69], 3),
        false,
    ),
    (
        42,
        ([0x75, 0xa2, 0x88, 0xc0, 0xd6, 0x89, 0x8c, 0x5f], 2),
        false,
    ),
    (
        43,
        ([0x15, 0x4f, 0xf2, 0x7b, 0x46, 0xf3, 0x55, 0x21], 3),
        false,
    ),
];
/// The family whose clips a weapon without a family of its own (or with no clips in the body model) uses: the
/// rifle family (`AniType` 0).
// DC-DEFAULT: DSV-125 (proposed)
pub const FALLBACK_ANI_TYPE: u8 = 0;
/// Longest family prefix (bytes).
pub const MAX_PREFIX: usize = 3;

/// Slot suffix keys, by stance (run, walk, crouch) and direction class (still, forward, back, left, right): aim
/// (the loop), fire, the throw's hold phase and its release phase (play once).
// DISASM: cshell+0x1980 (entries: action 0 aim, action 2 fire, actions 1 / 3 the hold and release phases of a
// throwable, three stance columns)
pub const SUFFIX_AIM: [[NameKey; 5]; 3] = [
    [
        ([0x75, 0xc1, 0x35, 0x4b, 0x34, 0x57, 0xf0, 0xe5], 4),
        ([0xfa, 0xd5, 0xaf, 0x9a, 0x9a, 0x87, 0x3c, 0xd5], 4),
        ([0x17, 0xf4, 0xa4, 0x58, 0x14, 0x51, 0x69, 0xe6], 4),
        ([0xd0, 0xd4, 0x7f, 0xfb, 0x98, 0xef, 0xe8, 0x2f], 4),
        ([0x41, 0x67, 0xa5, 0x7f, 0xdf, 0x7f, 0xe2, 0x7b], 4),
    ],
    [
        ([0xba, 0x34, 0x5a, 0x8c, 0xf2, 0xa2, 0xc3, 0x0f], 4),
        ([0xf8, 0x24, 0x28, 0xc7, 0x84, 0x7e, 0x27, 0x74], 4),
        ([0x0d, 0xbe, 0x48, 0x94, 0xc3, 0x73, 0xd8, 0x96], 4),
        ([0x10, 0x5c, 0x6f, 0x3f, 0x5e, 0xb2, 0x1f, 0xc0], 4),
        ([0xd6, 0x68, 0xb1, 0x68, 0x4a, 0xfb, 0xe3, 0xe2], 4),
    ],
    [
        ([0xc3, 0x6b, 0xfa, 0x1e, 0x44, 0x6c, 0xf2, 0xf6], 4),
        ([0xc8, 0x76, 0x4d, 0xd7, 0x6b, 0x6f, 0x77, 0x9e], 4),
        ([0x8b, 0xf5, 0x20, 0xdc, 0x2d, 0xe4, 0xe2, 0x1d], 4),
        ([0x0e, 0x99, 0x27, 0xdb, 0x97, 0x84, 0x15, 0x01], 4),
        ([0x45, 0x68, 0xe5, 0xaa, 0x02, 0x3b, 0x14, 0x4b], 4),
    ],
];
pub const SUFFIX_FIRE: [[NameKey; 5]; 3] = [
    [
        ([0xdd, 0x03, 0x7a, 0x38, 0xba, 0x5c, 0x8e, 0xce], 4),
        ([0xef, 0xa7, 0xc8, 0x09, 0x5d, 0x88, 0x54, 0xf8], 4),
        ([0x00, 0x8a, 0x06, 0xda, 0xbf, 0xd1, 0xcf, 0x5b], 4),
        ([0x10, 0x9b, 0x1e, 0x1c, 0xda, 0xd5, 0xa2, 0xc6], 4),
        ([0x56, 0x0b, 0x8a, 0x0f, 0xc7, 0x1b, 0xe7, 0x8e], 4),
    ],
    [
        ([0xe3, 0x50, 0x9c, 0xfa, 0xf2, 0x26, 0x3f, 0x63], 4),
        ([0xe6, 0x16, 0x1b, 0xf0, 0x6c, 0x9f, 0x1a, 0xe6], 4),
        ([0x09, 0x69, 0x22, 0x92, 0xe1, 0x57, 0xb7, 0xcd], 4),
        ([0x9a, 0xe0, 0xf9, 0x0b, 0x87, 0x72, 0xcb, 0x85], 4),
        ([0x76, 0x94, 0x31, 0x12, 0xca, 0xd8, 0x84, 0x67], 4),
    ],
    [
        ([0x0f, 0x10, 0x1a, 0xb3, 0xac, 0x14, 0xe2, 0x0a], 4),
        ([0x41, 0x18, 0x3e, 0xe6, 0x5b, 0x9a, 0x90, 0x2b], 4),
        ([0x31, 0x97, 0x7b, 0xf8, 0x90, 0xb3, 0x28, 0x71], 4),
        ([0x53, 0x1c, 0x1c, 0xdf, 0x6f, 0x0e, 0xb3, 0xe4], 4),
        ([0x4c, 0xad, 0x54, 0x52, 0x66, 0x01, 0x4d, 0x74], 4),
    ],
];
pub const SUFFIX_THROW_HOLD: [[NameKey; 5]; 3] = [
    [
        ([0xa2, 0x9f, 0x4f, 0x61, 0x84, 0x1b, 0x19, 0xd0], 6),
        ([0x4d, 0x09, 0x04, 0x38, 0xef, 0x54, 0x15, 0x4e], 6),
        ([0x9c, 0x71, 0x8c, 0x95, 0xcf, 0x77, 0x9f, 0xbe], 6),
        ([0x03, 0xad, 0x8c, 0x5d, 0x0e, 0x14, 0x21, 0xdb], 6),
        ([0x6c, 0x2f, 0x8c, 0xf9, 0x79, 0x58, 0x99, 0xf3], 6),
    ],
    [
        ([0xcd, 0x53, 0x73, 0x41, 0x49, 0x54, 0xe8, 0x01], 6),
        ([0xda, 0x92, 0x94, 0xe3, 0xc9, 0xc1, 0xd5, 0xc0], 6),
        ([0x85, 0x3f, 0xed, 0x6f, 0x6c, 0xd1, 0x0b, 0x6e], 6),
        ([0xda, 0x3c, 0x11, 0x9f, 0x92, 0xea, 0x09, 0x9f], 6),
        ([0xae, 0x23, 0x8c, 0x68, 0x6d, 0x7b, 0x04, 0x8e], 6),
    ],
    [
        ([0xc0, 0x1e, 0xb5, 0x7e, 0x4c, 0x18, 0x92, 0xa4], 6),
        ([0xb5, 0x11, 0x84, 0x2e, 0x10, 0xb0, 0xc1, 0x43], 6),
        ([0x19, 0x68, 0x61, 0x74, 0x20, 0xef, 0x4c, 0x62], 6),
        ([0xb7, 0x5a, 0x6c, 0x23, 0x01, 0x69, 0x64, 0x69], 6),
        ([0x49, 0xfc, 0xbf, 0x8d, 0xf1, 0x1d, 0x5a, 0xbb], 6),
    ],
];
pub const SUFFIX_THROW: [[NameKey; 5]; 3] = [
    [
        ([0xcb, 0x52, 0xbd, 0x96, 0x48, 0x37, 0xe2, 0x08], 6),
        ([0xff, 0xe2, 0x6c, 0x5a, 0x2e, 0x19, 0xe8, 0xbe], 6),
        ([0x44, 0x4d, 0x1e, 0x9c, 0x4c, 0xe2, 0x10, 0x60], 6),
        ([0xa0, 0xff, 0x39, 0xaf, 0x47, 0x2c, 0xda, 0x5a], 6),
        ([0x37, 0x5d, 0xc1, 0x0f, 0x74, 0x2d, 0xad, 0x26], 6),
    ],
    [
        ([0x58, 0x64, 0x43, 0x4a, 0x09, 0x14, 0x77, 0xa7], 6),
        ([0xaa, 0xb4, 0xf1, 0xdf, 0x0f, 0x86, 0x1f, 0x61], 6),
        ([0xff, 0x80, 0xff, 0x72, 0xf1, 0x89, 0x42, 0x1a], 6),
        ([0xf6, 0x2f, 0x4b, 0xa4, 0xbd, 0xfe, 0xa3, 0xef], 6),
        ([0xfb, 0xa5, 0x07, 0x5b, 0x2d, 0x50, 0x2b, 0xd5], 6),
    ],
    [
        ([0xec, 0x05, 0xca, 0x05, 0xeb, 0xde, 0x39, 0x27], 6),
        ([0x40, 0x51, 0x2a, 0xb0, 0xb0, 0x04, 0xe7, 0x6e], 6),
        ([0x65, 0x6b, 0x33, 0x31, 0xbc, 0x00, 0xc0, 0xce], 6),
        ([0x01, 0xc7, 0x8d, 0x6d, 0x5b, 0x21, 0x42, 0x0f], 6),
        ([0x0c, 0xd9, 0xad, 0xdc, 0x4b, 0x28, 0xb0, 0x1a], 6),
    ],
];
/// Select and reload suffix keys by variant: standing still, moving, crouched.
// DISASM: cshell+0x1980 (action 4 select, action 5 reload: the still direction class has its own clip, the moving
// classes share one, the crouch column its own)
pub const SUFFIX_SELECT: [NameKey; 3] = [
    ([0xac, 0xba, 0x22, 0xb0, 0x2c, 0x1f, 0x85, 0x23], 5),
    ([0x84, 0x04, 0x62, 0x25, 0xa4, 0x45, 0xba, 0x5e], 4),
    ([0x7f, 0xa9, 0xd2, 0xd7, 0x2e, 0xbb, 0xba, 0x24], 4),
];
pub const SUFFIX_RELOAD: [NameKey; 3] = [
    ([0xb9, 0x9c, 0x5c, 0xc8, 0xde, 0xd2, 0xcc, 0xb7], 5),
    ([0xed, 0x79, 0x2e, 0xe3, 0xd3, 0x99, 0xaf, 0xb0], 4),
    ([0xff, 0x7a, 0x32, 0x83, 0x87, 0xf8, 0x8f, 0xa9], 4),
];
/// The two hit reactions of the table (actions 6 and 7, every family, every stance column, play once; key of the
/// exact name). The ROM body's two short hit clips (467 / 400 ms with the default body).
// DISASM: cshell+0x1980 (actions 6 / 7 of every family: the same two clips, loop flag 0)
pub const CLIP_HIT: NameKey = ([0x6a, 0x0a, 0x2d, 0xbf, 0x1c, 0xa8, 0xee, 0xfa], 5);
pub const CLIP_HIT_BACK: NameKey = ([0xe9, 0x52, 0xd9, 0x51, 0x5b, 0xcf, 0x2a, 0x4e], 6);
/// The lower-body jump clips (the table's gait 3, play once; key of the exact name): standing still, then by
/// [`Dir`] in the order of [`Dir::ALL`]. DeCombat plays them from the client's jump event to the next ground
/// report; the reference stack streamed them rarely (2 of the real client's jumps in tdm1, 4 in tdm6, each while the
/// trailer reported no ground), so the trigger is ours.
// DISASM: cshell+0x1980 (lower entries of gait 3, loop flag 0), cshell+0x15250
// OBSERVED: capture 20261025_065016_tdm6, 20261025_000014_tdm1 (jump forward / jump right
// clips, `state1` 0, on tracker 2; REFERENCE)
// DC-DEFAULT: DSV-126 (proposed)
pub const CLIP_LOWER_JUMP_STAND: NameKey = ([0x0c, 0xc1, 0x84, 0x74, 0x47, 0xf1, 0xea, 0xcd], 3);
pub const CLIP_LOWER_JUMP: [NameKey; 8] = [
    ([0x06, 0x8a, 0xd1, 0x2b, 0x4c, 0xd3, 0x77, 0x6c], 3),
    ([0xb3, 0xd8, 0x6c, 0x44, 0x5d, 0xff, 0x80, 0x97], 3),
    ([0xf0, 0x8e, 0xa4, 0xdd, 0xd0, 0xf0, 0xea, 0x53], 3),
    ([0x0c, 0x8e, 0xa7, 0xce, 0x04, 0x4f, 0xd5, 0xee], 3),
    ([0x52, 0xfb, 0x4f, 0x4a, 0xb5, 0x1c, 0xb5, 0xb7], 4),
    ([0x87, 0x95, 0x92, 0xe5, 0xe3, 0x62, 0x78, 0xbf], 4),
    ([0x8e, 0x18, 0x70, 0x0e, 0x6e, 0xab, 0xb0, 0x24], 4),
    ([0x4a, 0xd1, 0xea, 0x2f, 0xe3, 0x2c, 0x5d, 0x61], 4),
];
/// A jump clip lasts at least this long before a ground report ends it.
// DC-DEFAULT: DSV-126 (proposed) 150 ms
pub const JUMP_MIN_MS: u32 = 150;
/// How long a throw hold lasts after the last `UP_THROW_HOLD` (the client repeats it while the button is held).
// SPEC: GAME_MESSAGES_2009_C2S.md §4.6 (repeated while held)
// DC-DEFAULT: DSV-126 (proposed) the throw-hold window of DSV-29
pub const THROW_HOLD_MS: u32 = crate::game::THROW_FIRE_WINDOW_MS;

/// Stance column of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stance {
    Run,
    Walk,
    Crouch,
}

/// Direction class of an entry (the diagonals fall onto forward and back).
// DISASM: cshell+0x1980 (direction rows 5 / 6 repeat the forward entry, rows 7 / 8 the back entry)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Heading {
    Still,
    Forward,
    Back,
    Left,
    Right,
}

/// Select / reload variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Variant {
    Still,
    Moving,
    Crouch,
}

/// A classified upper-body clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    Aim(Stance, Heading),
    Fire(Stance, Heading),
    ThrowHold(Stance, Heading),
    Throw(Stance, Heading),
    Select(Variant),
    Reload(Variant),
}

const STANCES: [Stance; 3] = [Stance::Run, Stance::Walk, Stance::Crouch];
const HEADINGS: [Heading; 5] = [
    Heading::Still,
    Heading::Forward,
    Heading::Back,
    Heading::Left,
    Heading::Right,
];
const VARIANTS: [Variant; 3] = [Variant::Still, Variant::Moving, Variant::Crouch];

/// The slot of a suffix key.
fn slot_of(k: NameKey) -> Option<Slot> {
    type Make = fn(Stance, Heading) -> Slot;
    let grids: [(&[[NameKey; 5]; 3], Make); 4] = [
        (&SUFFIX_AIM, Slot::Aim),
        (&SUFFIX_FIRE, Slot::Fire),
        (&SUFFIX_THROW_HOLD, Slot::ThrowHold),
        (&SUFFIX_THROW, Slot::Throw),
    ];
    for (grid, make) in grids {
        for (si, row) in grid.iter().enumerate() {
            if let Some(hi) = row.iter().position(|x| *x == k) {
                return Some(make(STANCES[si], HEADINGS[hi]));
            }
        }
    }
    if let Some(i) = SUFFIX_SELECT.iter().position(|x| *x == k) {
        return Some(Slot::Select(VARIANTS[i]));
    }
    SUFFIX_RELOAD
        .iter()
        .position(|x| *x == k)
        .map(|i| Slot::Reload(VARIANTS[i]))
}

/// Classifies a clip name of the ROM body model: the key of its family prefix and its slot, or `None` for a clip
/// outside the upper-body scheme. The name is compared in lower case (the 2009 table spells a few names with other
/// letter cases than the model).
pub fn classify(name: &[u8]) -> Option<(NameKey, Slot)> {
    let lower = name.to_ascii_lowercase();
    for k in 1..=MAX_PREFIX.min(lower.len().saturating_sub(1)) {
        let p = name_key(&lower[..k]);
        if !FAMILIES.iter().any(|f| f.1 == p) {
            continue;
        }
        if let Some(s) = slot_of(name_key(&lower[k..])) {
            return Some((p, s));
        }
    }
    None
}

/// What the server streams for a weapon class: the classified clips of one family.
#[derive(Debug, Clone, Copy)]
pub struct Family<'a> {
    clips: &'a BTreeMap<Slot, ClipRef>,
    fire_dirs: bool,
}

impl Family<'_> {
    pub fn get(&self, s: Slot) -> Option<ClipRef> {
        self.clips.get(&s).copied()
    }

    /// The first clip of `make(stance, heading)`, then the run column, then the still class of both.
    // DISASM: cshell+0x16040 (a stance column without a clip falls back to column 0)
    // DC-DEFAULT: DSV-125 (proposed) the still fallbacks (the reference stack streamed the rifle's still fire clip for every
    // direction: the rifle family has no moving fire clips)
    fn chain(&self, make: fn(Stance, Heading) -> Slot, st: Stance, h: Heading) -> Option<ClipRef> {
        [
            (st, h),
            (Stance::Run, h),
            (st, Heading::Still),
            (Stance::Run, Heading::Still),
        ]
        .iter()
        .find_map(|(s, h)| self.get(make(*s, *h)))
    }

    /// The select / reload clip: the crouch column first when crouched, then the still or moving variant of the
    /// direction class (the entry's column 0), then the other.
    // DISASM: cshell+0x16040 (a stance column without a clip falls back to column 0 of the same entry)
    fn variant(&self, make: fn(Variant) -> Slot, crouch: bool, h: Heading) -> Option<ClipRef> {
        let (first, other) = if h == Heading::Still {
            (Variant::Still, Variant::Moving)
        } else {
            (Variant::Moving, Variant::Still)
        };
        let order = [crouch.then_some(Variant::Crouch), Some(first), Some(other)];
        order.iter().flatten().find_map(|v| self.get(make(*v)))
    }
}

/// The classified upper-body clips of a rig, the hit reactions and the jump clips.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpperTable {
    families: BTreeMap<NameKey, BTreeMap<Slot, ClipRef>>,
    /// The hit reaction clips (front, back).
    pub hit: [Option<ClipRef>; 2],
    /// Jump clips: standing, then by [`Dir`] order.
    pub jump: [Option<ClipRef>; 9],
}

impl UpperTable {
    /// From a rig: the first clip of each family slot (the clips' order of the composed model).
    pub fn from_rig(rig: &Rig) -> UpperTable {
        let mut t = UpperTable::default();
        for (i, c) in rig.clips.iter().enumerate() {
            if let Some((p, s)) = c.upper {
                t.families
                    .entry(p)
                    .or_default()
                    .entry(s)
                    .or_insert((i as u16, c.len_ms));
            }
        }
        t.hit = [rig.clip(CLIP_HIT), rig.clip(CLIP_HIT_BACK)];
        t.jump[0] = rig.clip(CLIP_LOWER_JUMP_STAND);
        for (o, k) in t.jump[1..].iter_mut().zip(CLIP_LOWER_JUMP.iter()) {
            *o = rig.clip(*k);
        }
        t
    }

    /// No upper-body clip was classified (a synthetic rig or a body outside the scheme).
    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    /// Number of classified clips (counts for the ROM test).
    pub fn len(&self) -> usize {
        self.families.values().map(|f| f.len()).sum()
    }

    /// The family of an `AniType` with at least one aim clip in the rig, else the rifle family.
    pub fn family(&self, ani: Option<u8>) -> Option<Family<'_>> {
        let find = |a: u8| {
            let (_, p, fire_dirs) = FAMILIES.iter().find(|f| f.0 == a)?;
            let clips = self.families.get(p)?;
            clips
                .keys()
                .any(|s| matches!(s, Slot::Aim(..)))
                .then_some(Family {
                    clips,
                    fire_dirs: *fire_dirs,
                })
        };
        ani.and_then(find).or_else(|| find(FALLBACK_ANI_TYPE))
    }

    /// The looping aim clip of a motion.
    pub fn aim_clip(&self, ani: Option<u8>, motion: Motion) -> Option<ClipRef> {
        let f = self.family(ani)?;
        let (st, h) = stance_of(motion);
        f.chain(Slot::Aim, st, h)
    }

    /// The play-once clip of an action, chosen at its start.
    pub fn action_clip(&self, ani: Option<u8>, act: Act, motion: Motion) -> Option<ClipRef> {
        if let Act::Hit(back) = act {
            return self.hit[usize::from(back)].or(self.hit[0]);
        }
        let f = self.family(ani)?;
        let (st, h) = stance_of(motion);
        let crouch = st == Stance::Crouch;
        let fire_h = if f.fire_dirs { h } else { Heading::Still };
        match act {
            // a throwable family has no plain fire clip: its release phase
            Act::Fire => f
                .chain(Slot::Fire, st, fire_h)
                .or_else(|| f.chain(Slot::Throw, st, h)),
            Act::ThrowHold => f.chain(Slot::ThrowHold, st, h),
            Act::Throw => f
                .chain(Slot::Throw, st, h)
                .or_else(|| f.chain(Slot::Fire, st, fire_h)),
            Act::Select => f.variant(Slot::Select, crouch, h),
            Act::Reload => f.variant(Slot::Reload, crouch, h),
            Act::Hit(_) => None,
        }
    }

    /// The jump clip of a motion (standing still without a direction).
    pub fn jump_clip(&self, motion: Motion) -> Option<ClipRef> {
        let i = match motion {
            Motion::Step(_, d) => 1 + Dir::ALL.iter().position(|x| *x == d).unwrap_or(0),
            Motion::Walk => 1,
            Motion::Stand | Motion::Crouch => 0,
        };
        self.jump[i].or(self.jump[0])
    }
}

/// Stance column and direction class of a motion. Standing uses the run column (the reference stack streamed the run-stance
/// still aim clip to a standing player); the sprint gait of the 2009 table is not used (the reference stack streamed the run
/// clips while the client sprinted).
// DISASM: cshell+0x16040 (gait 0 = walk -> column 1, crouch -> column 2, sprint -> column 3, other gaits column 0)
// OBSERVED: capture 20261025_000014_tdm1, 20261025_065016_tdm6 (sprinting: the run-stance clips
// on both trackers; REFERENCE)
// DC-DEFAULT: DSV-125 (proposed)
pub fn stance_of(m: Motion) -> (Stance, Heading) {
    let heading = |d: Dir| match d {
        Dir::Forward | Dir::ForwardLeft | Dir::ForwardRight => Heading::Forward,
        Dir::Back | Dir::BackLeft | Dir::BackRight => Heading::Back,
        Dir::Left => Heading::Left,
        Dir::Right => Heading::Right,
    };
    match m {
        Motion::Stand => (Stance::Run, Heading::Still),
        Motion::Walk => (Stance::Walk, Heading::Forward),
        Motion::Crouch => (Stance::Crouch, Heading::Still),
        Motion::Step(g, d) => (
            match g {
                Gait::Run => Stance::Run,
                Gait::Walk => Stance::Walk,
                Gait::Crouch => Stance::Crouch,
            },
            heading(d),
        ),
    }
}

/// A play-once upper-body action. `Hit(back)`: the hit reaction (the second clip for a hit from behind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Fire,
    ThrowHold,
    Throw,
    Select,
    Reload,
    Hit(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Playing {
    act: Act,
    since: Millis,
    clip: ClipRef,
    /// A throw hold: time of the last `UP_THROW_HOLD`.
    held_at: Option<Millis>,
}

/// The animation state of one player character: the held weapon's `AniType`, the action in play and the time the
/// character left the ground.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pose {
    pub ani: Option<u8>,
    playing: Option<Playing>,
    airborne_since: Option<Millis>,
}

impl Pose {
    /// A fresh pose (spawn): no action, on the ground.
    pub fn reset(&mut self, ani: Option<u8>) {
        *self = Pose {
            ani,
            ..Pose::default()
        };
    }

    fn active(&self, now: Millis) -> Option<&Playing> {
        self.playing.as_ref().filter(|p| {
            since(now, p.since) < p.clip.1.max(1)
                || p.held_at.is_some_and(|h| since(now, h) < THROW_HOLD_MS)
        })
    }

    /// The action in play at `now`.
    pub fn action(&self, now: Millis) -> Option<Act> {
        self.active(now).map(|p| p.act)
    }

    /// Starts an action; the clip is chosen now (the reference stack kept a select clip while the direction changed). A new
    /// action replaces the one in play, except that a hit reaction only replaces a fire one-shot; a repeated throw
    /// hold keeps the hold. Returns false when the family has no clip for it.
    // OBSERVED: capture 20261025_000014_tdm1 (select / reload one-shots kept through direction
    // changes, back on the aim loop after the clip length: 733 ms / 1866 ms; rifle fire one-shots 100 ms; REFERENCE)
    // DC-DEFAULT: DSV-125 (proposed) action precedence; DSV-127 (proposed) the hit reaction's precedence
    pub fn start(&mut self, act: Act, table: &UpperTable, motion: Motion, now: Millis) -> bool {
        let current = self.active(now).copied();
        if matches!(act, Act::Hit(_)) && current.is_some_and(|p| p.act != Act::Fire) {
            return false;
        }
        if act == Act::ThrowHold {
            if let Some(p) = self.playing.as_mut() {
                if current.is_some_and(|c| c.act == Act::ThrowHold) {
                    p.held_at = Some(now);
                    return true;
                }
            }
        }
        let Some(clip) = table.action_clip(self.ani, act, motion) else {
            return false;
        };
        self.playing = Some(Playing {
            act,
            since: now,
            clip,
            held_at: (act == Act::ThrowHold).then_some(now),
        });
        true
    }

    /// Ends a throw hold (the release went out as a throw, or the weapon changed).
    pub fn stop(&mut self) {
        self.playing = None;
    }

    /// The client's jump event (`UP_MOVE_EVENT` category 1 event 1, sent at the take-off): the jump clip starts.
    pub fn jumped(&mut self, now: Millis) {
        self.airborne_since = Some(now);
    }

    /// The ground state of an accepted player update: ground ends a jump once [`JUMP_MIN_MS`] have passed (the
    /// take-off update may still report ground). Leaving the ground without a jump event (a fall) starts no clip.
    pub fn set_ground(&mut self, on_ground: bool, now: Millis) {
        if on_ground
            && self
                .airborne_since
                .is_some_and(|t| since(now, t) >= JUMP_MIN_MS)
        {
            self.airborne_since = None;
        }
    }

    pub fn airborne(&self) -> bool {
        self.airborne_since.is_some()
    }

    /// Tracker 1: the action's clip (play once, time since its start held at the clip's end), else the aim loop
    /// of the motion (`fallback` when the table has no clip for it).
    pub fn upper(
        &self,
        table: &UpperTable,
        motion: Motion,
        now: Millis,
        t_motion: u32,
        fallback: ClipRef,
    ) -> Tracker {
        if let Some(p) = self.active(now) {
            return Tracker {
                anim: p.clip.0,
                state0: true,
                state1: false,
                time_ms: since(now, p.since).min(p.clip.1),
                len_ms: p.clip.1,
                secondary: Some(TRACKER_UPPER),
            };
        }
        let (anim, len) = table.aim_clip(self.ani, motion).unwrap_or(fallback);
        Tracker {
            anim,
            state0: true,
            state1: true,
            time_ms: if len > 0 { t_motion % len } else { 0 },
            len_ms: len,
            secondary: Some(TRACKER_UPPER),
        }
    }

    /// Tracker 2 while airborne: the jump clip of the motion, play once from the take-off, held at its end.
    pub fn jump(&self, table: &UpperTable, motion: Motion, now: Millis) -> Option<Tracker> {
        let t0 = self.airborne_since?;
        let (anim, len) = table.jump_clip(motion)?;
        Some(Tracker {
            anim,
            state0: true,
            state1: false,
            time_ms: since(now, t0).min(len),
            len_ms: len,
            secondary: Some(TRACKER_LOWER),
        })
    }
}

impl crate::game::AnimSet {
    /// The three trackers of a living character with the upper-body table: [`AnimSet::trackers`] with tracker 1
    /// from the pose (weapon family, stance, direction, action) and tracker 2 on the jump clip while airborne.
    /// `table_on` false (`-directionalanims off`) or a rig without classified clips: the plain trackers.
    ///
    /// [`AnimSet::trackers`]: crate::game::AnimSet::trackers
    // DC-DEFAULT: DSV-125 (proposed); DSV-126 (proposed)
    pub fn trackers_posed(
        &self,
        motion: Motion,
        pose: &Pose,
        now: Millis,
        t_main: u32,
        t_motion: u32,
        table_on: bool,
    ) -> [Tracker; 3] {
        let mut t = self.trackers(motion, t_main, t_motion);
        if !table_on {
            return t;
        }
        t[1] = pose.upper(&self.table, motion, now, t_motion, (t[1].anim, t[1].len_ms));
        if let Some(j) = pose.jump(&self.table, motion, now) {
            t[2] = j;
        }
        // the rig's own weight sets (DSV-475)
        t[1].secondary = Some(self.secondary[0]);
        t[2].secondary = Some(self.secondary[1]);
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_keys_are_distinct_and_classify_back() {
        let mut all: Vec<NameKey> = Vec::new();
        for g in [&SUFFIX_AIM, &SUFFIX_FIRE, &SUFFIX_THROW_HOLD, &SUFFIX_THROW] {
            all.extend(g.iter().flatten().copied());
        }
        all.extend(SUFFIX_SELECT);
        all.extend(SUFFIX_RELOAD);
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n, "every suffix key once");
        for k in all {
            assert!(slot_of(k).is_some());
        }
        // a name that splits into no known prefix and suffix is not classified
        assert_eq!(classify(b"xyz"), None);
        assert_eq!(classify(b""), None);
    }

    #[test]
    fn motion_maps_to_the_table_columns() {
        assert_eq!(stance_of(Motion::Stand), (Stance::Run, Heading::Still));
        assert_eq!(
            stance_of(Motion::Step(Gait::Run, Dir::ForwardLeft)),
            (Stance::Run, Heading::Forward)
        );
        assert_eq!(
            stance_of(Motion::Step(Gait::Walk, Dir::BackRight)),
            (Stance::Walk, Heading::Back)
        );
        assert_eq!(
            stance_of(Motion::Step(Gait::Crouch, Dir::Left)),
            (Stance::Crouch, Heading::Left)
        );
    }

    fn table() -> UpperTable {
        let mut t = UpperTable::default();
        let rifle = FAMILIES[0].1;
        let mut m = BTreeMap::new();
        m.insert(Slot::Aim(Stance::Run, Heading::Still), (10, 4000));
        m.insert(Slot::Aim(Stance::Run, Heading::Forward), (11, 600));
        m.insert(Slot::Aim(Stance::Walk, Heading::Forward), (12, 1000));
        m.insert(Slot::Fire(Stance::Run, Heading::Still), (13, 100));
        m.insert(Slot::Select(Variant::Still), (14, 733));
        m.insert(Slot::Select(Variant::Moving), (15, 733));
        t.families.insert(rifle, m);
        t.hit[0] = Some((20, 467));
        t
    }

    #[test]
    fn fallbacks_and_play_once_timing() {
        let t = table();
        let run = Motion::Step(Gait::Run, Dir::Forward);
        // an unknown weapon type uses the rifle family; a missing column falls back to the run column
        assert_eq!(t.aim_clip(Some(99), run), Some((11, 600)));
        assert_eq!(
            t.aim_clip(None, Motion::Step(Gait::Crouch, Dir::Forward)),
            Some((11, 600))
        );
        assert_eq!(
            t.aim_clip(None, Motion::Step(Gait::Walk, Dir::Forward)),
            Some((12, 1000))
        );
        // moving fire without a moving fire clip -> the still fire clip
        assert_eq!(t.action_clip(None, Act::Fire, run), Some((13, 100)));
        let mut p = Pose::default();
        assert!(p.start(Act::Select, &t, Motion::Stand, 1000));
        let u = p.upper(&t, run, 1500, 0, (0, 0));
        assert_eq!((u.anim, u.state1, u.time_ms), (14, false, 500));
        // a hit does not replace a select; after the select the aim loop of the current motion
        assert!(!p.start(Act::Hit(false), &t, run, 1600));
        let u = p.upper(&t, run, 1800, 50, (0, 0));
        assert_eq!((u.anim, u.state1, u.time_ms), (11, true, 50));
        assert!(p.start(Act::Hit(false), &t, run, 1800));
        assert_eq!(p.action(1900), Some(Act::Hit(false)));
        // the jump clip needs the airborne state
        assert!(p.jump(&t, run, 2000).is_none());
    }
}

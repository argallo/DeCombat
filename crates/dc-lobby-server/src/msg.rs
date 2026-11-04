//! Message ids used by the core server. Numeric ids are 2009 wire facts; the constant names are this project's
//! names from `specs/PROTOCOL_2009.md` (Appendix A).
// SPEC: PROTOCOL_2009.md Appendix A

// auth server (§2)
pub const LOGIN_REQUEST: u16 = 0x200;
pub const LOGIN_REPLY: u16 = 0x201;
pub const NEW_CHAR: u16 = 0x202;
pub const NEW_CHAR_REPLY: u16 = 0x203;
pub const NAME_CHECK: u16 = 0x204;
pub const NAME_CHECK_REPLY: u16 = 0x205;
pub const FILE_CHECK: u16 = 0x206;
pub const FILE_CHECK_REPLY: u16 = 0x207;
pub const SERVER_PICK: u16 = 0x20a;
pub const SERVER_PICK_REPLY: u16 = 0x20b;
pub const REFERRER_CHECK: u16 = 0x20e;
pub const REFERRER_CHECK_REPLY: u16 = 0x20f;
pub const SERVER_LIST_PUSH: u16 = 0x2000;

// lobby login and character (§3.1, §3.3)
pub const LOBBY_LOGIN: u16 = 0x300;
pub const LOBBY_LOGIN_REPLY: u16 = 0x301;
pub const CHAR_SHEET_FETCH: u16 = 0x302;
pub const CHAR_SHEET_REPLY: u16 = 0x303;
pub const LOGOUT: u16 = 0x304;
pub const LOGOUT_REPLY: u16 = 0x305;
pub const LOCATION: u16 = 0x306;
pub const PRIVATE_STATE: u16 = 0x30a;
pub const PRIVATE_STATE_REPLY: u16 = 0x30b;
pub const PRIVATE_STATE_END: u16 = 0x30c;
pub const PRIVATE_STATE_END_REPLY: u16 = 0x30d;
pub const OPTION_SET: u16 = 0x308;
pub const OPTION_SET_REPLY: u16 = 0x309;
pub const ROOM_SETTINGS_FETCH: u16 = 0x310;
pub const ROOM_SETTINGS_REPLY: u16 = 0x311;
pub const INVENTORY: u16 = 0x320;
pub const INVENTORY_REPLY: u16 = 0x321;
pub const EQUIPMENT: u16 = 0x322;
pub const EQUIPMENT_REPLY: u16 = 0x323;
pub const ITEM_USE: u16 = 0x330;
pub const ITEM_USE_REPLY: u16 = 0x331;
pub const EXPIRY_CHECK: u16 = 0x33e;
pub const EXPIRY_PUSH: u16 = 0x33f;
pub const ITEM_SYNC: u16 = 0x342;
pub const ITEM_SYNC_REPLY: u16 = 0x343;
pub const BALANCE: u16 = 0x344;
pub const BALANCE_REPLY: u16 = 0x345;
pub const NOTICE_FETCH: u16 = 0x366;
pub const NOTICE_REPLY: u16 = 0x367;
pub const PROGRESS_PUSH: u16 = 0x3004;

// servers, channels, lobby lists (§3.4, §3.6)
pub const CHANNEL_USERS: u16 = 0x370;
pub const CHANNEL_USERS_REPLY: u16 = 0x371;
pub const CHANNEL_ENTER: u16 = 0x372;
pub const CHANNEL_ENTER_REPLY: u16 = 0x373;
pub const CHAN_EXIT: u16 = 0x374;
pub const CHAN_EXIT_REPLY: u16 = 0x375;
pub const CHANNEL_LIST: u16 = 0x376;
pub const CHANNEL_LIST_REPLY: u16 = 0x377;
pub const PROFILE_FETCH: u16 = 0x378;
pub const PROFILE_REPLY: u16 = 0x379;
pub const SERVER_LIST: u16 = 0x3b0;
pub const SERVER_LIST_REPLY: u16 = 0x3b1;
pub const SERVER_MOVE: u16 = 0x3b2;
pub const SERVER_MOVE_REPLY: u16 = 0x3b3;
pub const CHAT_SEND: u16 = 0x3a0;
pub const CHAT_PUSH: u16 = 0x3051;
pub const LOBBY_USER_ADD_PUSH: u16 = 0x3021;
pub const LOBBY_USER_REMOVE_PUSH: u16 = 0x3022;
pub const LOBBY_ROOM_ADD_PUSH: u16 = 0x3031;
pub const LOBBY_ROOM_REMOVE_PUSH: u16 = 0x3032;

// rooms (§3.7) and game start (§7)
pub const ROOM_LIST: u16 = 0x380;
pub const ROOM_LIST_REPLY: u16 = 0x381;
pub const ROOM_CREATE: u16 = 0x382;
pub const ROOM_CREATE_REPLY: u16 = 0x383;
pub const ROOM_ENTER: u16 = 0x384;
pub const ROOM_ENTER_REPLY: u16 = 0x385;
pub const ROOM_LEAVE: u16 = 0x386;
pub const ROOM_LEAVE_REPLY: u16 = 0x387;
pub const ROOM_INFO: u16 = 0x388;
pub const ROOM_INFO_REPLY: u16 = 0x389;
pub const ROOM_USERS: u16 = 0x38a;
pub const ROOM_USERS_REPLY: u16 = 0x38b;
pub const ROOM_SETTING_CHANGE: u16 = 0x38c;
pub const ROOM_SETTING_CHANGE_REPLY: u16 = 0x38d;
pub const ROOM_KICK: u16 = 0x38e;
pub const ROOM_KICK_REPLY: u16 = 0x38f;
pub const TEAM_CHANGE: u16 = 0x390;
pub const TEAM_CHANGE_REPLY: u16 = 0x391;
pub const ROOM_USER_FLAG: u16 = 0x392;
pub const ROOM_USER_FLAG_REPLY: u16 = 0x393;
pub const MATCH_EXIT: u16 = 0x394;
pub const ROOM_INVITE_ALL: u16 = 0x396;
pub const ROOM_INVITE_ALL_REPLY: u16 = 0x397;
pub const ROOM_INVITE_PUSH: u16 = 0x3023;
pub const ROOM_USER_ADD_PUSH: u16 = 0x3033;
pub const ROOM_USER_REMOVE_PUSH: u16 = 0x3034;
pub const ROOM_UPDATE_PUSH: u16 = 0x3035;
pub const ROOM_TEAM_PUSH: u16 = 0x3041;
pub const ROOM_USER_STATE_PUSH: u16 = 0x3042;
pub const MATCH_ADDRESS_PUSH: u16 = 0x3043;
pub const START_COUNTDOWN_PUSH: u16 = 0x3044;

/// Room update field types (0x3035 +0xa, 0x38c +0xa).
// SPEC: PROTOCOL_2009.md §3.7 (0x3035 types, EVIDENCE: cshell+0x168680 switch 1..0xf)
pub mod field {
    pub const LOCKED: u32 = 1;
    pub const HOST: u32 = 2;
    pub const MAP: u32 = 3;
    pub const MODE: u32 = 4;
    pub const PLAYERS: u32 = 5;
    pub const MAX_PLAYERS: u32 = 6;
    pub const STATE: u32 = 7;
    pub const GOAL: u32 = 8;
    pub const OPT_MID_JOIN: u32 = 9;
    pub const OPT_FRIENDLY_FIRE: u32 = 0xa;
    pub const PASSWORD: u32 = 0xc;
    pub const OPT_KILL_CAM: u32 = 0xd;
    pub const WEAPON_LIMIT: u32 = 0xe;
    pub const HOST_BADGE: u32 = 0xf;
}

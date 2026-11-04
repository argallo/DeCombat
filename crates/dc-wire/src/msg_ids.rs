//! Message ids of the 2009 lobby / auth TCP protocol, with the names `PROTOCOL_2009.md` gives them.
//!
//! SPEC: PROTOCOL_2009.md Appendix A (id table) and PROTOCOL_2009_coverage.tsv. Every constant carries the
//! section of its detail row. Ids that the spec lists only as unproven (Appendix B: 0x331, 0x33b, 0x33d,
//! 0x3001, 0x3002) and the ignored 0x313 have no layout or name there and are therefore not defined.
//! Request ids are even and replies are `request + 1` (SPEC: PROTOCOL_2009.md §1.2). The UDP game-connection
//! message ids belong to the session-layer spec (UDP_SESSION_2009.md, not yet written) and are not defined here.
//!
//! Generated from the spec text; regenerate rather than hand-edit when the spec changes.

/// `login_request` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x191e30)
pub const LOGIN_REQUEST: u16 = 0x200;

/// `login_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x42450)
pub const LOGIN_REPLY: u16 = 0x201;

/// `character_create` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x1922d0)
pub const CHARACTER_CREATE: u16 = 0x202;

/// `character_create_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x34e60)
pub const CHARACTER_CREATE_REPLY: u16 = 0x203;

/// `name_check` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x1921f0)
pub const NAME_CHECK: u16 = 0x204;

/// `name_check_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x34de0)
pub const NAME_CHECK_REPLY: u16 = 0x205;

/// `file_check` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x191fb0)
pub const FILE_CHECK: u16 = 0x206;

/// `file_check_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x38ce0)
pub const FILE_CHECK_REPLY: u16 = 0x207;

/// `server_pick` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x1924a0)
pub const SERVER_PICK: u16 = 0x20a;

/// `server_pick_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x34f60, cs+0x13dff0)
pub const SERVER_PICK_REPLY: u16 = 0x20b;

/// `referrer_check` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x1925b0)
pub const REFERRER_CHECK: u16 = 0x20e;

/// `referrer_check_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x35b80, cs+0x12cf70)
pub const REFERRER_CHECK_REPLY: u16 = 0x20f;

/// `server_list_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x38a20, cs+0x13eda0)
pub const SERVER_LIST_PUSH: u16 = 0x2000;

/// `auth_alive_push` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x42df0)
pub const AUTH_ALIVE_PUSH: u16 = 0x2001;

/// `bonus_event_push` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §2.2 (evidence cs+0x356b0)
pub const BONUS_EVENT_PUSH: u16 = 0x2002;

/// `lobby_login` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x18ab20)
pub const LOBBY_LOGIN: u16 = 0x300;

/// `lobby_login_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x420d0)
pub const LOBBY_LOGIN_REPLY: u16 = 0x301;

/// `character_info` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x18acf0)
pub const CHARACTER_INFO: u16 = 0x302;

/// `character_info_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x3f2c0)
pub const CHARACTER_INFO_REPLY: u16 = 0x303;

/// `logout` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18d210)
pub const LOGOUT: u16 = 0x304;

/// `logout_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x35160)
pub const LOGOUT_REPLY: u16 = 0x305;

/// `location` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18ac20)
pub const LOCATION: u16 = 0x306;

/// `option_set` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18da20)
pub const OPTION_SET: u16 = 0x308;

/// `option_set_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x35a20)
pub const OPTION_SET_REPLY: u16 = 0x309;

/// `private_state` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18b6b0)
pub const PRIVATE_STATE: u16 = 0x30a;

/// `private_state_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A (evidence cs+0x35c90)
pub const PRIVATE_STATE_REPLY: u16 = 0x30b;

/// `private_state_end` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18b780)
pub const PRIVATE_STATE_END: u16 = 0x30c;

/// `private_state_end_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A (evidence cs+0x35cc0)
pub const PRIVATE_STATE_END_REPLY: u16 = 0x30d;

/// `observer_mode` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18de40)
pub const OBSERVER_MODE: u16 = 0x30e;

/// `observer_mode_reply` (S->C, PROVEN (case)).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x42df0)
pub const OBSERVER_MODE_REPLY: u16 = 0x30f;

/// `room_settings_fetch` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x18ec30)
pub const ROOM_SETTINGS_FETCH: u16 = 0x310;

/// `room_settings_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x3dbf0, cs+0x3db40)
pub const ROOM_SETTINGS_REPLY: u16 = 0x311;

/// `player_report` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x273790, cs+0x18e9d0)
pub const PLAYER_REPORT: u16 = 0x312;

/// `inventory` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x18ae70)
pub const INVENTORY: u16 = 0x320;

/// `inventory_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x42340, cs+0x229130)
pub const INVENTORY_REPLY: u16 = 0x321;

/// `equipment` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x18adb0)
pub const EQUIPMENT: u16 = 0x322;

/// `equipment_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x229ca0)
pub const EQUIPMENT_REPLY: u16 = 0x323;

/// `equip` (C->S, PROVEN, LIVE). Spec note: also frame of 0x368 / 0x36a.
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18b0e0)
pub const EQUIP: u16 = 0x324;

/// `equip_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x34ab0)
pub const EQUIP_REPLY: u16 = 0x325;

/// `unequip` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18af30)
pub const UNEQUIP: u16 = 0x326;

/// `unequip_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x34a40)
pub const UNEQUIP_REPLY: u16 = 0x327;

/// `shop_buy` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18cf50)
pub const SHOP_BUY: u16 = 0x328;

/// `shop_buy_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x336a0)
pub const SHOP_BUY_REPLY: u16 = 0x329;

/// `gift_send` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x18e0c0)
pub const GIFT_SEND: u16 = 0x32a;

/// `gift_send_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x1d6cb0)
pub const GIFT_SEND_REPLY: u16 = 0x32b;

/// `inbox_page` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x18df10)
pub const INBOX_PAGE: u16 = 0x32c;

/// `inbox_page_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x33730)
pub const INBOX_PAGE_REPLY: u16 = 0x32d;

/// `inbox_open` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x18dfe0)
pub const INBOX_OPEN: u16 = 0x32e;

/// `inbox_open_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x33ca0)
pub const INBOX_OPEN_REPLY: u16 = 0x32f;

/// `item_use` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x18e1d0)
pub const ITEM_USE: u16 = 0x330;

/// `item_discard` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18a480)
pub const ITEM_DISCARD: u16 = 0x334;

/// `item_discard_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x32ee0)
pub const ITEM_DISCARD_REPLY: u16 = 0x335;

/// `attach` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18b1c0)
pub const ATTACH: u16 = 0x336;

/// `attach_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x34b10)
pub const ATTACH_REPLY: u16 = 0x337;

/// `detach` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18b2a0)
pub const DETACH: u16 = 0x338;

/// `detach_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x34b70)
pub const DETACH_REPLY: u16 = 0x339;

/// `tuning_sync` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18d2d0)
pub const TUNING_SYNC: u16 = 0x33a;

/// `period_finish` (C->S, PROVEN (dead)).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18d3c0)
pub const PERIOD_FINISH: u16 = 0x33c;

/// `expiry_check` (C->S, PROVEN (dead)).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18d480)
pub const EXPIRY_CHECK: u16 = 0x33e;

/// `expiry_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x22cde0)
pub const EXPIRY_PUSH: u16 = 0x33f;

/// `coupon_redeem` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.4 (evidence cs+0x18db00)
pub const COUPON_REDEEM: u16 = 0x340;

/// `coupon_redeem_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.4 (evidence cs+0x26a3e0)
pub const COUPON_REDEEM_REPLY: u16 = 0x341;

/// `item_sync` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x18dbf0)
pub const ITEM_SYNC: u16 = 0x342;

/// `item_sync_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x35c40)
pub const ITEM_SYNC_REPLY: u16 = 0x343;

/// `balance` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x18e310)
pub const BALANCE: u16 = 0x344;

/// `balance_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x337b0)
pub const BALANCE_REPLY: u16 = 0x345;

/// `recipient_check` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x18e410)
pub const RECIPIENT_CHECK: u16 = 0x346;

/// `recipient_check_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence dispatcher cs+0x42df0)
pub const RECIPIENT_CHECK_REPLY: u16 = 0x347;

/// `clan_leader_check` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x18e4f0)
pub const CLAN_LEADER_CHECK: u16 = 0x348;

/// `clan_leader_check_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence dispatcher cs+0x42df0)
pub const CLAN_LEADER_CHECK_REPLY: u16 = 0x349;

/// `clan_name_check` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x18e5d0)
pub const CLAN_NAME_CHECK: u16 = 0x34a;

/// `clan_name_check_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence dispatcher cs+0x42df0)
pub const CLAN_NAME_CHECK_REPLY: u16 = 0x34b;

/// `clan_mark_check` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x18e6b0)
pub const CLAN_MARK_CHECK: u16 = 0x34c;

/// `clan_mark_check_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x33940)
pub const CLAN_MARK_CHECK_REPLY: u16 = 0x34d;

/// `coupon_redeem_reply_b` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §4.4 (evidence cs+0x42df0 -> cs+0x26a3e0)
pub const COUPON_REDEEM_REPLY_B: u16 = 0x34f;

/// `shop_view_fetch` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x18ea90)
pub const SHOP_VIEW_FETCH: u16 = 0x350;

/// `shop_view_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x42ca0)
pub const SHOP_VIEW_REPLY: u16 = 0x351;

/// `list_fetch_0` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x18ed10)
pub const LIST_FETCH_0: u16 = 0x352;

/// `list_reply_0` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x2581c0)
pub const LIST_REPLY_0: u16 = 0x353;

/// `list_fetch_1` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x18ed10)
pub const LIST_FETCH_1: u16 = 0x354;

/// `list_reply_1` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x258240)
pub const LIST_REPLY_1: u16 = 0x355;

/// `list_fetch_2` (C->S, PROVEN, LIVE). Spec note: also frame of 0x35a / 0x35c.
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x18ed10)
pub const LIST_FETCH_2: u16 = 0x356;

/// `list_reply_2` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x2582c0)
pub const LIST_REPLY_2: u16 = 0x357;

/// `price_fetch` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x18eb60)
pub const PRICE_FETCH: u16 = 0x358;

/// `price_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x41350)
pub const PRICE_REPLY: u16 = 0x359;

/// `list_fetch_3` (C->S, PROVEN). Spec note: frame 0x356.
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x18ed10)
pub const LIST_FETCH_3: u16 = 0x35a;

/// `list_reply_3` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x42df0)
pub const LIST_REPLY_3: u16 = 0x35b;

/// `list_fetch_4` (C->S, PROVEN). Spec note: frame 0x356.
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x18ed10)
pub const LIST_FETCH_4: u16 = 0x35c;

/// `list_reply_4` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §4.1 (evidence cs+0x42df0)
pub const LIST_REPLY_4: u16 = 0x35d;

/// `function_item_a` (C->S, PROVEN (dormant)).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18ee60)
pub const FUNCTION_ITEM_A: u16 = 0x35e;

/// `function_item_b` (C->S, PROVEN (dormant)).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18ee60)
pub const FUNCTION_ITEM_B: u16 = 0x360;

/// `function_item_c` (C->S, PROVEN (dormant)).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18ee60)
pub const FUNCTION_ITEM_C: u16 = 0x362;

/// `function_item_d` (C->S, PROVEN (dormant)).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x18ee60)
pub const FUNCTION_ITEM_D: u16 = 0x364;

/// `function_item_a_reply` (S->C, PROVEN). The spec groups the four replies; the per-letter suffix is ours.
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x339d0)
pub const FUNCTION_ITEM_A_REPLY: u16 = 0x35f;

/// `function_item_b_reply` (S->C, PROVEN). The spec groups the four replies; the per-letter suffix is ours.
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x339d0)
pub const FUNCTION_ITEM_B_REPLY: u16 = 0x361;

/// `function_item_c_reply` (S->C, PROVEN). The spec groups the four replies; the per-letter suffix is ours.
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x339d0)
pub const FUNCTION_ITEM_C_REPLY: u16 = 0x363;

/// `function_item_d_reply` (S->C, PROVEN). The spec groups the four replies; the per-letter suffix is ours.
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x339d0)
pub const FUNCTION_ITEM_D_REPLY: u16 = 0x365;

/// `notice_fetch` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x18dcb0)
pub const NOTICE_FETCH: u16 = 0x366;

/// `notice_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence dispatcher cs+0x42df0)
pub const NOTICE_REPLY: u16 = 0x367;

/// `function_equip` (C->S, PROVEN). Spec note: frame 0x324.
// SPEC: PROTOCOL_2009.md Appendix A, §1.2 (evidence cs+0x18f7b0)
pub const FUNCTION_EQUIP: u16 = 0x368;

/// `function_equip_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A (evidence cs+0x36710)
pub const FUNCTION_EQUIP_REPLY: u16 = 0x369;

/// `function_unequip` (C->S, PROVEN). Spec note: frame 0x324.
// SPEC: PROTOCOL_2009.md Appendix A, §1.2 (evidence cs+0x18f890)
pub const FUNCTION_UNEQUIP: u16 = 0x36a;

/// `function_unequip_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A (evidence cs+0x36740)
pub const FUNCTION_UNEQUIP_REPLY: u16 = 0x36b;

/// `megaphone_send` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x18f470)
pub const MEGAPHONE_SEND: u16 = 0x36c;

/// `megaphone_send_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x36770)
pub const MEGAPHONE_SEND_REPLY: u16 = 0x36d;

/// `megaphone_queue` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x18f5d0)
pub const MEGAPHONE_QUEUE: u16 = 0x36e;

/// `megaphone_queue_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x367b0)
pub const MEGAPHONE_QUEUE_REPLY: u16 = 0x36f;

/// `channel_users` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x189f20)
pub const CHANNEL_USERS: u16 = 0x370;

/// `channel_users_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x1220e0)
pub const CHANNEL_USERS_REPLY: u16 = 0x371;

/// `channel_enter` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x189db0)
pub const CHANNEL_ENTER: u16 = 0x372;

/// `channel_enter_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x32e30, cs+0xfacd0)
pub const CHANNEL_ENTER_REPLY: u16 = 0x373;

/// `channel_leave` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18d050)
pub const CHANNEL_LEAVE: u16 = 0x374;

/// `channel_leave_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x35010)
pub const CHANNEL_LEAVE_REPLY: u16 = 0x375;

/// `channel_list` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18d110)
pub const CHANNEL_LIST: u16 = 0x376;

/// `channel_list_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x350f0, cs+0xfb880)
pub const CHANNEL_LIST_REPLY: u16 = 0x377;

/// `profile_fetch` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.1 (evidence cs+0x18dd70)
pub const PROFILE_FETCH: u16 = 0x378;

/// `profile_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.1 (evidence dispatcher cs+0x42df0)
pub const PROFILE_REPLY: u16 = 0x379;

/// `room_list` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x18ca00)
pub const ROOM_LIST: u16 = 0x380;

/// `room_list_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x332a0, cs+0x12a530)
pub const ROOM_LIST_REPLY: u16 = 0x381;

/// `room_create` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18a560)
pub const ROOM_CREATE: u16 = 0x382;

/// `room_create_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x33320, cs+0x138ad0)
pub const ROOM_CREATE_REPLY: u16 = 0x383;

/// `room_enter` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18a710)
pub const ROOM_ENTER: u16 = 0x384;

/// `room_enter_reply` (S->C, PROVEN (wrapper), LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x33370)
pub const ROOM_ENTER_REPLY: u16 = 0x385;

/// `room_leave` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18b380)
pub const ROOM_LEAVE: u16 = 0x386;

/// `room_leave_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x33400)
pub const ROOM_LEAVE_REPLY: u16 = 0x387;

/// `room_info` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18a820)
pub const ROOM_INFO: u16 = 0x388;

/// `room_info_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x1684c0)
pub const ROOM_INFO_REPLY: u16 = 0x389;

/// `room_users` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18b440)
pub const ROOM_USERS: u16 = 0x38a;

/// `room_users_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x166660)
pub const ROOM_USERS_REPLY: u16 = 0x38b;

/// `room_setting_change` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18a8e0, cs+0x18a9c0)
pub const ROOM_SETTING_CHANGE: u16 = 0x38c;

/// `room_setting_change_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x162370)
pub const ROOM_SETTING_CHANGE_REPLY: u16 = 0x38d;

/// `room_kick` (C->S, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18f280, cs+0x18f1a0)
pub const ROOM_KICK: u16 = 0x38e;

/// `room_kick_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x160340)
pub const ROOM_KICK_REPLY: u16 = 0x38f;

/// `team_change` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18b500)
pub const TEAM_CHANGE: u16 = 0x390;

/// `team_change_reply` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x15f070)
pub const TEAM_CHANGE_REPLY: u16 = 0x391;

/// `room_user_flag` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18b5d0)
pub const ROOM_USER_FLAG: u16 = 0x392;

/// `room_user_flag_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x15f0d0)
pub const ROOM_USER_FLAG_REPLY: u16 = 0x393;

/// `match_exit` (C->S, PROVEN, LIVE, PROVEN-CAPTURE): phase 0 = the member left the match (also sent on every
/// deliberate leave of a running match), then phase 1 after a failed connection. Formerly `match_join_failed`.
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 row 0x394, §7.1 / §7.2 (evidence cs+0x18b010)
pub const MATCH_EXIT: u16 = 0x394;

/// Deprecated alias of [`MATCH_EXIT`], kept for one release so the crates of other branches keep compiling.
#[deprecated(note = "0x394 is `match_exit` (PROTOCOL_2009.md §3.7); use MATCH_EXIT")]
pub const MATCH_JOIN_FAILED: u16 = MATCH_EXIT;

/// `room_invite_all` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x18d960)
pub const ROOM_INVITE_ALL: u16 = 0x396;

/// `room_invite_all_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x39060)
pub const ROOM_INVITE_ALL_REPLY: u16 = 0x397;

/// `chat_send` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x18a0d0)
pub const CHAT_SEND: u16 = 0x3a0;

/// `server_list` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18f0e0)
pub const SERVER_LIST: u16 = 0x3b0;

/// `server_list_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x13f0b0)
pub const SERVER_LIST_REPLY: u16 = 0x3b1;

/// `server_move` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x18f010)
pub const SERVER_MOVE: u16 = 0x3b2;

/// `server_move_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x13e200)
pub const SERVER_MOVE_REPLY: u16 = 0x3b3;

/// `clan_login` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x18d570)
pub const CLAN_LOGIN: u16 = 0x3c0;

/// `clan_login_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x356f0)
pub const CLAN_LOGIN_REPLY: u16 = 0x3c1;

/// `clan_info` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x18d630)
pub const CLAN_INFO: u16 = 0x3c2;

/// `clan_info_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x35750)
pub const CLAN_INFO_REPLY: u16 = 0x3c3;

/// `clan_members` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x18d6f0)
pub const CLAN_MEMBERS: u16 = 0x3c4;

/// `clan_members_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x35820)
pub const CLAN_MEMBERS_REPLY: u16 = 0x3c5;

/// `clan_notice` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x18d7b0)
pub const CLAN_NOTICE: u16 = 0x3c6;

/// `clan_notice_reply` (S->C, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x35880)
pub const CLAN_NOTICE_REPLY: u16 = 0x3c7;

/// `clan_notice_set` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x18d870)
pub const CLAN_NOTICE_SET: u16 = 0x3c8;

/// `clan_notice_set_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x358e0)
pub const CLAN_NOTICE_SET_REPLY: u16 = 0x3c9;

/// `op_restrict` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.4 (evidence cs+0x42920, cs+0x18e790)
pub const OP_RESTRICT: u16 = 0x3d0;

/// `op_restrict_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.4 (evidence cs+0x3e0f0)
pub const OP_RESTRICT_REPLY: u16 = 0x3d1;

/// `op_lookup` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.4 (evidence cs+0x40980, cs+0x18e850)
pub const OP_LOOKUP: u16 = 0x3d2;

/// `op_lookup_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.4 (evidence cs+0x40dd0)
pub const OP_LOOKUP_REPLY: u16 = 0x3d3;

/// `op_mute` (C->S, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.4 (evidence cs+0x40790, cs+0x18e910)
pub const OP_MUTE: u16 = 0x3d4;

/// `op_mute_reply` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.4 (evidence cs+0x3e300)
pub const OP_MUTE_REPLY: u16 = 0x3d5;

/// `grade_push` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x36690)
pub const GRADE_PUSH: u16 = 0x3003;

/// `progress_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.3 (evidence cs+0x40580)
pub const PROGRESS_PUSH: u16 = 0x3004;

/// `expiry_notice_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.2 (evidence cs+0x39280)
pub const EXPIRY_NOTICE_PUSH: u16 = 0x3011;

/// `clan_change_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x33ac0)
pub const CLAN_CHANGE_PUSH: u16 = 0x3012;

/// `case_announce_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x41470)
pub const CASE_ANNOUNCE_PUSH: u16 = 0x3013;

/// `inbox_stale_push` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §4.3 (evidence cs+0x33c20)
pub const INBOX_STALE_PUSH: u16 = 0x3014;

/// `lobby_user_add_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x122230)
pub const LOBBY_USER_ADD_PUSH: u16 = 0x3021;

/// `lobby_user_remove_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x1222b0)
pub const LOBBY_USER_REMOVE_PUSH: u16 = 0x3022;

/// `room_invite_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x39110)
pub const ROOM_INVITE_PUSH: u16 = 0x3023;

/// `lobby_room_add_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x12a8b0)
pub const LOBBY_ROOM_ADD_PUSH: u16 = 0x3031;

/// `lobby_room_remove_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x126130)
pub const LOBBY_ROOM_REMOVE_PUSH: u16 = 0x3032;

/// `room_user_add_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x166310)
pub const ROOM_USER_ADD_PUSH: u16 = 0x3033;

/// `room_user_remove_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x165400)
pub const ROOM_USER_REMOVE_PUSH: u16 = 0x3034;

/// `room_update_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x168680, cs+0x12ae60)
pub const ROOM_UPDATE_PUSH: u16 = 0x3035;

/// `room_info_stale_push` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x42df0)
pub const ROOM_INFO_STALE_PUSH: u16 = 0x3036;

/// `voice_room_push` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x348e0)
pub const VOICE_ROOM_PUSH: u16 = 0x3037;

/// `room_option_push` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.4 (evidence cs+0x38890)
pub const ROOM_OPTION_PUSH: u16 = 0x3038;

/// `room_team_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x166c90)
pub const ROOM_TEAM_PUSH: u16 = 0x3041;

/// `room_user_state_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.7 (evidence cs+0x1673a0)
pub const ROOM_USER_STATE_PUSH: u16 = 0x3042;

/// `match_address_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §7 (evidence cs+0x165650)
pub const MATCH_ADDRESS_PUSH: u16 = 0x3043;

/// `start_countdown_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §7 (evidence cs+0x15f2b0)
pub const START_COUNTDOWN_PUSH: u16 = 0x3044;

/// `chat_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x192f0)
pub const CHAT_PUSH: u16 = 0x3051;

/// `ticker_notice_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x34270)
pub const TICKER_NOTICE_PUSH: u16 = 0x3052;

/// `mute_until_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §6.4 (evidence cs+0x361c0, cs+0x1b1620)
pub const MUTE_UNTIL_PUSH: u16 = 0x3053;

/// `shop_view_version_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x36420)
pub const SHOP_VIEW_VERSION_PUSH: u16 = 0x3054;

/// `room_settings_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x3c890)
pub const ROOM_SETTINGS_PUSH: u16 = 0x3055;

/// `list_version_push_0` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x42df0)
pub const LIST_VERSION_PUSH_0: u16 = 0x3056;

/// `list_version_push_1` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x42df0)
pub const LIST_VERSION_PUSH_1: u16 = 0x3057;

/// `list_version_push_2` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x42df0)
pub const LIST_VERSION_PUSH_2: u16 = 0x3058;

/// `price_counter_push` (S->C, PROVEN).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x36450)
pub const PRICE_COUNTER_PUSH: u16 = 0x3059;

/// `list_version_push_3` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x42df0)
pub const LIST_VERSION_PUSH_3: u16 = 0x305a;

/// `list_version_push_4` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.5 (evidence cs+0x42df0)
pub const LIST_VERSION_PUSH_4: u16 = 0x305b;

/// `clan_info_stale_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x42df0)
pub const CLAN_INFO_STALE_PUSH: u16 = 0x3061;

/// `clan_members_stale_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x35980)
pub const CLAN_MEMBERS_STALE_PUSH: u16 = 0x3062;

/// `clan_notice_stale_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x35940)
pub const CLAN_NOTICE_STALE_PUSH: u16 = 0x3063;

/// `clan_removed_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §5.1 (evidence cs+0x42df0)
pub const CLAN_REMOVED_PUSH: u16 = 0x3064;

/// `broadcast_push` (S->C, PROVEN, LIVE).
// SPEC: PROTOCOL_2009.md Appendix A, §3.6 (evidence cs+0x38190)
pub const BROADCAST_PUSH: u16 = 0x3065;

/// `cak_messenger_push` (S->C, LIVE). Our own extension push; the 2009 client ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_MESSENGER_PUSH: u16 = 0x3f01;

/// `dc_modded_servers_push` (S->C). Our own extension push (LG-272, `moddedlist`); the 2009 client ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_MODDED_SERVERS_PUSH: u16 = 0x3f02;

/// `dc_room_hacks_push` (S->C, lobby connection). Our own extension push (LG-274, `roomhacks`); the 2009 client
/// ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ROOM_HACKS_PUSH: u16 = 0x3f03;

/// `dc_room_hacks_request` (C->S, lobby connection). Our own extension request (LG-274, `roomhacks`); the 2009
/// client never sends it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ROOM_HACKS_REQUEST: u16 = 0x3f04;

/// `dc_gun_game_offer` (S->C, lobby connection). Our own extension push (LG-305, `gungame`); the 2009 client ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_GUN_GAME_OFFER: u16 = 0x3f05;

/// `dc_gun_game_request` (C->S, lobby connection). Our own extension request (LG-305, `gungame`); the 2009 client
/// never sends it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_GUN_GAME_REQUEST: u16 = 0x3f06;

/// `dc_gun_game_room` (S->C, lobby connection). Our own extension push (LG-305, `gungame`); the 2009 client ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_GUN_GAME_ROOM: u16 = 0x3f07;

/// `dc_room_hardcore_push` (S->C, lobby connection). Our own extension push (LG-306, `roomhardcore`); the 2009 client
/// ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ROOM_HARDCORE_PUSH: u16 = 0x3f08;

/// `dc_room_hardcore_request` (C->S, lobby connection). Our own extension request (LG-306, `roomhardcore`); the 2009
/// client never sends it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ROOM_HARDCORE_REQUEST: u16 = 0x3f09;

/// `dc_room_possession_push` (S->C, lobby connection). Our own extension push (LG-307, `possession`); the 2009 client
/// ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ROOM_POSSESSION_PUSH: u16 = 0x3f0a;

/// `dc_room_possession_request` (C->S, lobby connection). Our own extension request (LG-307, `possession`); the 2009
/// client never sends it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ROOM_POSSESSION_REQUEST: u16 = 0x3f0b;
/// `dc_seat_ticket_push` (S->C, lobby connection, before 0x3043). Our own extension push (LG-327, `seatticket`); the 2009
/// client ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_SEAT_TICKET_PUSH: u16 = 0x3f0c;
/// `dc_entry_mods_push` (S->C, auth connection, before 0x2000). Our own extension push (LG-313, `entrymods`); the 2009
/// client ignores it. A lobby with no featured entry set never sends it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ENTRY_MODS_PUSH: u16 = 0x3f10;

/// `dc_entry_mods_claim` (C->S, auth or lobby connection, before 0x20a / 0x3b2). Our own extension request (LG-313,
/// `entrymods`); the 2009 client never sends it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ENTRY_MODS_CLAIM: u16 = 0x3f11;

/// `dc_entry_mods_refusal` (S->C, right before a refused 0x20b / 0x3b3). Our own extension push (LG-313, `entrymods`);
/// the 2009 client ignores it.
// SPEC: PROTOCOL_2009.md Appendix A (evidence our extension)
pub const DC_ENTRY_MODS_REFUSAL: u16 = 0x3f12;

/// Every id above with its spec name, sorted by id.
pub const TCP_MESSAGE_NAMES: &[(u16, &str)] = &[
    (LOGIN_REQUEST, "login_request"),
    (LOGIN_REPLY, "login_reply"),
    (CHARACTER_CREATE, "character_create"),
    (CHARACTER_CREATE_REPLY, "character_create_reply"),
    (NAME_CHECK, "name_check"),
    (NAME_CHECK_REPLY, "name_check_reply"),
    (FILE_CHECK, "file_check"),
    (FILE_CHECK_REPLY, "file_check_reply"),
    (SERVER_PICK, "server_pick"),
    (SERVER_PICK_REPLY, "server_pick_reply"),
    (REFERRER_CHECK, "referrer_check"),
    (REFERRER_CHECK_REPLY, "referrer_check_reply"),
    (LOBBY_LOGIN, "lobby_login"),
    (LOBBY_LOGIN_REPLY, "lobby_login_reply"),
    (CHARACTER_INFO, "character_info"),
    (CHARACTER_INFO_REPLY, "character_info_reply"),
    (LOGOUT, "logout"),
    (LOGOUT_REPLY, "logout_reply"),
    (LOCATION, "location"),
    (OPTION_SET, "option_set"),
    (OPTION_SET_REPLY, "option_set_reply"),
    (PRIVATE_STATE, "private_state"),
    (PRIVATE_STATE_REPLY, "private_state_reply"),
    (PRIVATE_STATE_END, "private_state_end"),
    (PRIVATE_STATE_END_REPLY, "private_state_end_reply"),
    (OBSERVER_MODE, "observer_mode"),
    (OBSERVER_MODE_REPLY, "observer_mode_reply"),
    (ROOM_SETTINGS_FETCH, "room_settings_fetch"),
    (ROOM_SETTINGS_REPLY, "room_settings_reply"),
    (PLAYER_REPORT, "player_report"),
    (INVENTORY, "inventory"),
    (INVENTORY_REPLY, "inventory_reply"),
    (EQUIPMENT, "equipment"),
    (EQUIPMENT_REPLY, "equipment_reply"),
    (EQUIP, "equip"),
    (EQUIP_REPLY, "equip_reply"),
    (UNEQUIP, "unequip"),
    (UNEQUIP_REPLY, "unequip_reply"),
    (SHOP_BUY, "shop_buy"),
    (SHOP_BUY_REPLY, "shop_buy_reply"),
    (GIFT_SEND, "gift_send"),
    (GIFT_SEND_REPLY, "gift_send_reply"),
    (INBOX_PAGE, "inbox_page"),
    (INBOX_PAGE_REPLY, "inbox_page_reply"),
    (INBOX_OPEN, "inbox_open"),
    (INBOX_OPEN_REPLY, "inbox_open_reply"),
    (ITEM_USE, "item_use"),
    (ITEM_DISCARD, "item_discard"),
    (ITEM_DISCARD_REPLY, "item_discard_reply"),
    (ATTACH, "attach"),
    (ATTACH_REPLY, "attach_reply"),
    (DETACH, "detach"),
    (DETACH_REPLY, "detach_reply"),
    (TUNING_SYNC, "tuning_sync"),
    (PERIOD_FINISH, "period_finish"),
    (EXPIRY_CHECK, "expiry_check"),
    (EXPIRY_PUSH, "expiry_push"),
    (COUPON_REDEEM, "coupon_redeem"),
    (COUPON_REDEEM_REPLY, "coupon_redeem_reply"),
    (ITEM_SYNC, "item_sync"),
    (ITEM_SYNC_REPLY, "item_sync_reply"),
    (BALANCE, "balance"),
    (BALANCE_REPLY, "balance_reply"),
    (RECIPIENT_CHECK, "recipient_check"),
    (RECIPIENT_CHECK_REPLY, "recipient_check_reply"),
    (CLAN_LEADER_CHECK, "clan_leader_check"),
    (CLAN_LEADER_CHECK_REPLY, "clan_leader_check_reply"),
    (CLAN_NAME_CHECK, "clan_name_check"),
    (CLAN_NAME_CHECK_REPLY, "clan_name_check_reply"),
    (CLAN_MARK_CHECK, "clan_mark_check"),
    (CLAN_MARK_CHECK_REPLY, "clan_mark_check_reply"),
    (COUPON_REDEEM_REPLY_B, "coupon_redeem_reply_b"),
    (SHOP_VIEW_FETCH, "shop_view_fetch"),
    (SHOP_VIEW_REPLY, "shop_view_reply"),
    (LIST_FETCH_0, "list_fetch_0"),
    (LIST_REPLY_0, "list_reply_0"),
    (LIST_FETCH_1, "list_fetch_1"),
    (LIST_REPLY_1, "list_reply_1"),
    (LIST_FETCH_2, "list_fetch_2"),
    (LIST_REPLY_2, "list_reply_2"),
    (PRICE_FETCH, "price_fetch"),
    (PRICE_REPLY, "price_reply"),
    (LIST_FETCH_3, "list_fetch_3"),
    (LIST_REPLY_3, "list_reply_3"),
    (LIST_FETCH_4, "list_fetch_4"),
    (LIST_REPLY_4, "list_reply_4"),
    (FUNCTION_ITEM_A, "function_item_a"),
    (FUNCTION_ITEM_A_REPLY, "function_item_a_reply"),
    (FUNCTION_ITEM_B, "function_item_b"),
    (FUNCTION_ITEM_B_REPLY, "function_item_b_reply"),
    (FUNCTION_ITEM_C, "function_item_c"),
    (FUNCTION_ITEM_C_REPLY, "function_item_c_reply"),
    (FUNCTION_ITEM_D, "function_item_d"),
    (FUNCTION_ITEM_D_REPLY, "function_item_d_reply"),
    (NOTICE_FETCH, "notice_fetch"),
    (NOTICE_REPLY, "notice_reply"),
    (FUNCTION_EQUIP, "function_equip"),
    (FUNCTION_EQUIP_REPLY, "function_equip_reply"),
    (FUNCTION_UNEQUIP, "function_unequip"),
    (FUNCTION_UNEQUIP_REPLY, "function_unequip_reply"),
    (MEGAPHONE_SEND, "megaphone_send"),
    (MEGAPHONE_SEND_REPLY, "megaphone_send_reply"),
    (MEGAPHONE_QUEUE, "megaphone_queue"),
    (MEGAPHONE_QUEUE_REPLY, "megaphone_queue_reply"),
    (CHANNEL_USERS, "channel_users"),
    (CHANNEL_USERS_REPLY, "channel_users_reply"),
    (CHANNEL_ENTER, "channel_enter"),
    (CHANNEL_ENTER_REPLY, "channel_enter_reply"),
    (CHANNEL_LEAVE, "channel_leave"),
    (CHANNEL_LEAVE_REPLY, "channel_leave_reply"),
    (CHANNEL_LIST, "channel_list"),
    (CHANNEL_LIST_REPLY, "channel_list_reply"),
    (PROFILE_FETCH, "profile_fetch"),
    (PROFILE_REPLY, "profile_reply"),
    (ROOM_LIST, "room_list"),
    (ROOM_LIST_REPLY, "room_list_reply"),
    (ROOM_CREATE, "room_create"),
    (ROOM_CREATE_REPLY, "room_create_reply"),
    (ROOM_ENTER, "room_enter"),
    (ROOM_ENTER_REPLY, "room_enter_reply"),
    (ROOM_LEAVE, "room_leave"),
    (ROOM_LEAVE_REPLY, "room_leave_reply"),
    (ROOM_INFO, "room_info"),
    (ROOM_INFO_REPLY, "room_info_reply"),
    (ROOM_USERS, "room_users"),
    (ROOM_USERS_REPLY, "room_users_reply"),
    (ROOM_SETTING_CHANGE, "room_setting_change"),
    (ROOM_SETTING_CHANGE_REPLY, "room_setting_change_reply"),
    (ROOM_KICK, "room_kick"),
    (ROOM_KICK_REPLY, "room_kick_reply"),
    (TEAM_CHANGE, "team_change"),
    (TEAM_CHANGE_REPLY, "team_change_reply"),
    (ROOM_USER_FLAG, "room_user_flag"),
    (ROOM_USER_FLAG_REPLY, "room_user_flag_reply"),
    (MATCH_EXIT, "match_exit"),
    (ROOM_INVITE_ALL, "room_invite_all"),
    (ROOM_INVITE_ALL_REPLY, "room_invite_all_reply"),
    (CHAT_SEND, "chat_send"),
    (SERVER_LIST, "server_list"),
    (SERVER_LIST_REPLY, "server_list_reply"),
    (SERVER_MOVE, "server_move"),
    (SERVER_MOVE_REPLY, "server_move_reply"),
    (CLAN_LOGIN, "clan_login"),
    (CLAN_LOGIN_REPLY, "clan_login_reply"),
    (CLAN_INFO, "clan_info"),
    (CLAN_INFO_REPLY, "clan_info_reply"),
    (CLAN_MEMBERS, "clan_members"),
    (CLAN_MEMBERS_REPLY, "clan_members_reply"),
    (CLAN_NOTICE, "clan_notice"),
    (CLAN_NOTICE_REPLY, "clan_notice_reply"),
    (CLAN_NOTICE_SET, "clan_notice_set"),
    (CLAN_NOTICE_SET_REPLY, "clan_notice_set_reply"),
    (OP_RESTRICT, "op_restrict"),
    (OP_RESTRICT_REPLY, "op_restrict_reply"),
    (OP_LOOKUP, "op_lookup"),
    (OP_LOOKUP_REPLY, "op_lookup_reply"),
    (OP_MUTE, "op_mute"),
    (OP_MUTE_REPLY, "op_mute_reply"),
    (SERVER_LIST_PUSH, "server_list_push"),
    (AUTH_ALIVE_PUSH, "auth_alive_push"),
    (BONUS_EVENT_PUSH, "bonus_event_push"),
    (GRADE_PUSH, "grade_push"),
    (PROGRESS_PUSH, "progress_push"),
    (EXPIRY_NOTICE_PUSH, "expiry_notice_push"),
    (CLAN_CHANGE_PUSH, "clan_change_push"),
    (CASE_ANNOUNCE_PUSH, "case_announce_push"),
    (INBOX_STALE_PUSH, "inbox_stale_push"),
    (LOBBY_USER_ADD_PUSH, "lobby_user_add_push"),
    (LOBBY_USER_REMOVE_PUSH, "lobby_user_remove_push"),
    (ROOM_INVITE_PUSH, "room_invite_push"),
    (LOBBY_ROOM_ADD_PUSH, "lobby_room_add_push"),
    (LOBBY_ROOM_REMOVE_PUSH, "lobby_room_remove_push"),
    (ROOM_USER_ADD_PUSH, "room_user_add_push"),
    (ROOM_USER_REMOVE_PUSH, "room_user_remove_push"),
    (ROOM_UPDATE_PUSH, "room_update_push"),
    (ROOM_INFO_STALE_PUSH, "room_info_stale_push"),
    (VOICE_ROOM_PUSH, "voice_room_push"),
    (ROOM_OPTION_PUSH, "room_option_push"),
    (ROOM_TEAM_PUSH, "room_team_push"),
    (ROOM_USER_STATE_PUSH, "room_user_state_push"),
    (MATCH_ADDRESS_PUSH, "match_address_push"),
    (START_COUNTDOWN_PUSH, "start_countdown_push"),
    (CHAT_PUSH, "chat_push"),
    (TICKER_NOTICE_PUSH, "ticker_notice_push"),
    (MUTE_UNTIL_PUSH, "mute_until_push"),
    (SHOP_VIEW_VERSION_PUSH, "shop_view_version_push"),
    (ROOM_SETTINGS_PUSH, "room_settings_push"),
    (LIST_VERSION_PUSH_0, "list_version_push_0"),
    (LIST_VERSION_PUSH_1, "list_version_push_1"),
    (LIST_VERSION_PUSH_2, "list_version_push_2"),
    (PRICE_COUNTER_PUSH, "price_counter_push"),
    (LIST_VERSION_PUSH_3, "list_version_push_3"),
    (LIST_VERSION_PUSH_4, "list_version_push_4"),
    (CLAN_INFO_STALE_PUSH, "clan_info_stale_push"),
    (CLAN_MEMBERS_STALE_PUSH, "clan_members_stale_push"),
    (CLAN_NOTICE_STALE_PUSH, "clan_notice_stale_push"),
    (CLAN_REMOVED_PUSH, "clan_removed_push"),
    (BROADCAST_PUSH, "broadcast_push"),
    (DC_MESSENGER_PUSH, "cak_messenger_push"),
    (DC_MODDED_SERVERS_PUSH, "dc_modded_servers_push"),
    (DC_ROOM_HACKS_PUSH, "dc_room_hacks_push"),
    (DC_ROOM_HACKS_REQUEST, "dc_room_hacks_request"),
    (DC_GUN_GAME_OFFER, "dc_gun_game_offer"),
    (DC_GUN_GAME_REQUEST, "dc_gun_game_request"),
    (DC_GUN_GAME_ROOM, "dc_gun_game_room"),
    (DC_ROOM_HARDCORE_PUSH, "dc_room_hardcore_push"),
    (DC_ROOM_HARDCORE_REQUEST, "dc_room_hardcore_request"),
    (DC_ROOM_POSSESSION_PUSH, "dc_room_possession_push"),
    (DC_ROOM_POSSESSION_REQUEST, "dc_room_possession_request"),
    (DC_SEAT_TICKET_PUSH, "dc_seat_ticket_push"),
    (DC_ENTRY_MODS_PUSH, "dc_entry_mods_push"),
    (DC_ENTRY_MODS_CLAIM, "dc_entry_mods_claim"),
    (DC_ENTRY_MODS_REFUSAL, "dc_entry_mods_refusal"),
];

/// Spec name of a TCP message id, if the spec names it.
pub fn tcp_message_name(id: u16) -> Option<&'static str> {
    TCP_MESSAGE_NAMES
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(_, n)| *n)
}

//! Message ids of the shop, inventory, inbox, coupon, megaphone and item-window paths. The numbers are 2009 wire
//! facts; the constant names are this project's names from `specs/PROTOCOL_2009.md` §4 and Appendix A.
// SPEC: PROTOCOL_2009.md §4 (4.1 catalogue lists, 4.2 buy / equip / discard / attach, 4.3 inbox / gifts / item use,
// 4.4 coupons), §3.5 (version pushes), §6.2 (0x3065), Appendix A

// equip, unequip, buy, gift, inbox (§4.2, §4.3)
pub const EQUIP: u16 = 0x324;
pub const EQUIP_REPLY: u16 = 0x325;
pub const UNEQUIP: u16 = 0x326;
pub const UNEQUIP_REPLY: u16 = 0x327;
pub const SHOP_BUY: u16 = 0x328;
pub const SHOP_BUY_REPLY: u16 = 0x329;
pub const GIFT_SEND: u16 = 0x32a;
pub const GIFT_SEND_REPLY: u16 = 0x32b;
pub const INBOX_PAGE: u16 = 0x32c;
pub const INBOX_PAGE_REPLY: u16 = 0x32d;
pub const INBOX_OPEN: u16 = 0x32e;
pub const INBOX_OPEN_REPLY: u16 = 0x32f;
pub const ITEM_DISCARD: u16 = 0x334;
pub const ITEM_DISCARD_REPLY: u16 = 0x335;
pub const ATTACH: u16 = 0x336;
pub const ATTACH_REPLY: u16 = 0x337;
pub const DETACH: u16 = 0x338;
pub const DETACH_REPLY: u16 = 0x339;
pub const TUNING_SYNC: u16 = 0x33a;

// coupons, recipient check (§4.3, §4.4)
pub const COUPON_REDEEM: u16 = 0x340;
pub const COUPON_REDEEM_REPLY: u16 = 0x341;
pub const RECIPIENT_CHECK: u16 = 0x346;
pub const RECIPIENT_CHECK_REPLY: u16 = 0x347;

// catalogue lists (§4.1); 0x35a / 0x35c arrive framed as 0x356 (inner-id dispatch, §1.2)
pub const SHOP_VIEW_FETCH: u16 = 0x350;
pub const SHOP_VIEW_REPLY: u16 = 0x351;
pub const LIST_FETCH_0: u16 = 0x352;
pub const LIST_FETCH_1: u16 = 0x354;
pub const LIST_FETCH_2: u16 = 0x356;
pub const PRICE_FETCH: u16 = 0x358;
pub const PRICE_REPLY: u16 = 0x359;
pub const LIST_FETCH_3: u16 = 0x35a;
pub const LIST_REPLY_3: u16 = 0x35b;
pub const LIST_FETCH_4: u16 = 0x35c;
pub const LIST_REPLY_4: u16 = 0x35d;

// function items and the megaphone (§4.2, §4.3); 0x368 / 0x36a arrive framed as 0x324
pub const FUNCTION_EQUIP: u16 = 0x368;
pub const FUNCTION_EQUIP_REPLY: u16 = 0x369;
pub const FUNCTION_UNEQUIP: u16 = 0x36a;
pub const FUNCTION_UNEQUIP_REPLY: u16 = 0x36b;
pub const MEGAPHONE_SEND: u16 = 0x36c;
pub const MEGAPHONE_SEND_REPLY: u16 = 0x36d;
pub const MEGAPHONE_QUEUE: u16 = 0x36e;
pub const MEGAPHONE_QUEUE_REPLY: u16 = 0x36f;

// pushes (§3.5, §4.3, §5.1, §6.2)
pub const INBOX_STALE_PUSH: u16 = 0x3014;
pub const BROADCAST_PUSH: u16 = 0x3065;
pub const CLAN_INFO_STALE_PUSH: u16 = 0x3061;

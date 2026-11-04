//! w2-lobbychat: the chat page's run-time parts the lobby and the waiting room share: the chat edit box the 2009
//! shell builds in code (it is in no layout page), the chat window's per-screen panel texture, the send rules (empty
//! field, mute end, repeated line), the chat lines of 0x3051 and the lobby's notice bar (0x3052 / 0x3065).
//!
//! The 2009 chat page builds ONE edit control for the whole session (control id and command 0x75, at most 0x77
//! characters) and places it at (0xb9, 0x2b1) whenever a lobby screen shows the page: the room list clips it at x
//! 0x1b8 and swaps the chat window's panel for its narrow one, the waiting room clips it at x 0x26a with the wide one.
//! Enter in the field or the page's Enter button (command 0x75) sends the text with 0x3a0; the client never prints its
//! own line: the server's echo 0x3051 does.
// SPEC: PROTOCOL_2009.md §3.6 (0x3a0 wstr[121] text PROVEN cs+0x18a0d0; 0x3051 echo; 0x3052 ticker), §6.4 (0x3053)
// EVIDENCE: cshell+0x1e880 (chat page build: the edit control created once with id 0x75, the page's font and size,
// at most 0x77 characters, place (0xb9, 0x2b1), the page's text colour for all its colours, empty text, clip
// rectangle place + (0xff, 0xf)), cshell+0x1f850 (the page's font: index 4, size 0xd), cshell+0x1ba40 (page
// colours: text 0xffffffff, GM lines 0xffffff00), cshell+0x1bb70 (page shown on screen 0xf: the room list's panel
// literal, clip (0xb9, 0x2b1)-(0x1b8, 0x2c0); on screen 9: the waiting room's panel literal, clip to (0x26a,
// 0x2c0)), cshell+0x205e0 (key 0xd -> command 0x75), cshell+0x1bf20 (command 0x75 -> the send, vtable +0x14c),
// cshell+0x20ed0 (the send: nothing for an empty field; repeat rule; cshell+0x449b0; then the field is cleared)

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::catalog::Catalog;
use super::link::LinkCmd;
use super::lobby::CHAT_PANEL;
use super::roomtext::printf_int;
use super::sheet::{place_text, Look, Part, Sheet};
use super::{pages, Ctx, MenuData};
use crate::hud::items::DrawItem;
use dc_ui_data::pages::Align;
use dc_ui_data::WidgetKind;

/// The chat edit control's id and the command its Enter (and the page's Enter button) sends.
pub const CHAT_BOX_ID: i32 = 0x75;
pub const CMD_CHAT_SEND: i32 = 0x75;
/// Place, character limit and clip rectangle of the chat edit control.
pub const CHAT_EDIT_AT: [i32; 2] = [0xb9, 0x2b1];
pub const CHAT_MAX_CHARS: usize = 0x77;
pub const CHAT_CLIP_BOTTOM: i32 = 0x2c0;
pub const LOBBY_CHAT_CLIP_RIGHT: i32 = 0x1b8;
pub const ROOM_CHAT_CLIP_RIGHT: i32 = 0x26a;
/// The chat page's font and size (edit control and lines).
pub const CHAT_PAGE_FONT: i32 = 4;
pub const CHAT_PAGE_SIZE: i32 = 13;
/// The chat page's text colour and the colour of GM lines and of the client's own warning lines.
pub const CHAT_COLOUR: u32 = 0xffff_ffff;
pub const GM_COLOUR: u32 = 0xffff_ff00;
// EVIDENCE: cshell+0x40ca0 (the mute line through cshell+0x40bd0 with 0xffffff00)
pub const WARN_COLOUR: u32 = 0xffff_ff00;
/// The edit control's caret colour (the chat build keeps the control's default).
// EVIDENCE: cshell+0x2f74c0 (edit control constructor: +0x1a8 = 0x89bc6ae0)
// OBSERVED: lobbyfx4/real/R10_lobby.png (the 2009 caret (105, 61, 123) over (8, 7, 5))
pub const CHAT_CARET_COLOUR: u32 = 0x89bc_6ae0;
/// The caret's blink half-period of the chat control.
// EVIDENCE: cshell+0x1e880 (cshell+0x2f79c0 with the float 0x3eaa7efa)
pub const CHAT_CARET_PERIOD: f32 = 0.333;
/// The chat keeps this many lines.
// EVIDENCE: cshell+0x1c8d0 (more than 100 lines -> the first goes)
pub const CHAT_LINES: usize = 100;
/// Strings of the send rules: muted for hours (`%d`), muted for less than an hour, the same line too often.
// EVIDENCE: cshell+0x40ca0 (0x185d with remaining minutes / 60, else 0x185e), cshell+0x20ed0 (0x1028)
pub const TEXT_MUTED_HOURS: i32 = 0x185d;
pub const TEXT_MUTED: i32 = 0x185e;
pub const TEXT_REPEATED: i32 = 0x1028;
/// The repeat rule: after the same line three times in a row, more lines wait this long after the third.
// EVIDENCE: cshell+0x20ed0 (count > 2 and less than 60000 ms since -> refused; else count and time reset)
pub const REPEAT_LIMIT: u32 = 2;
pub const REPEAT_WAIT: Duration = Duration::from_secs(60);
/// FILETIME ticks per minute.
const TICKS_PER_MINUTE: u64 = 60 * dc_botclient::clock::TICKS_PER_SEC;

/// Which lobby screen shows the chat page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatScreen {
    /// The room list (screen 0xf): narrow panel, field clipped at x 0x1b8.
    Lobby,
    /// The waiting room (screen 9): wide panel, field clipped at x 0x26a.
    Room,
}

impl ChatScreen {
    pub fn clip_right(self) -> i32 {
        match self {
            ChatScreen::Lobby => LOBBY_CHAT_CLIP_RIGHT,
            ChatScreen::Room => ROOM_CHAT_CLIP_RIGHT,
        }
    }
}

/// The chat window of the sheet (window [`CHAT_PANEL`] of the chat page).
fn chat_frame(sheet: &Sheet) -> Option<usize> {
    sheet.parts.iter().rposition(|p| {
        p.kind == WidgetKind::Window && p.layer == CHAT_PANEL && p.page == pages::CHAT
    })
}

/// The chat page's button 2 of the chat window: a button without a command right of the field.
pub const CHAT_SPARE_BUTTON: i32 = 2;

/// Gives the chat window the panel the 2009 chat page shows on `screen` (when the user's binary names it) and hides
/// the window's button without a command.
// OBSERVED: lobbyfx4/real/R20_room.png (2009 waiting room: nothing drawn at the button's record place
// 598..635 x 678..711, the field runs on under it), R10_lobby.png (the character info covers the place)
// DC-DEFAULT: CLI-484 (proposed) the chat window's command-less button 2 is hidden (the 2009 code that hides it is
// not identified)
pub fn chat_panel(sheet: &mut Sheet, cat: &Catalog, screen: ChatScreen) {
    let Some(wi) = chat_frame(sheet) else {
        return;
    };
    if let Some(b) = sheet.part_mut(CHAT_PANEL, CHAT_SPARE_BUTTON, WidgetKind::Button) {
        if b.command == 0 && b.page == pages::CHAT {
            b.shown = false;
        }
    }
    let panel = match screen {
        ChatScreen::Lobby => cat.paths.chat_panels[0].clone(),
        ChatScreen::Room => cat.paths.chat_panels[1].clone(),
    };
    if let (Some(t), Look::Panel { tex, .. }) = (panel, &mut sheet.parts[wi].look) {
        *tex = t;
    }
}

/// Builds the chat edit box into `sheet` the way the 2009 chat page places it for `screen` and gives it the
/// keyboard.
// DC-DEFAULT: CLI-483 (proposed) the chat box has the keyboard when the room list or the waiting room opens and
// after each send (typing goes to the chat); a click on it gives it the keyboard back; on the room list Enter with an
// empty field still joins the selected room (dc-client's keyboard join)
pub fn add_chat_edit(sheet: &mut Sheet, screen: ChatScreen) -> Option<usize> {
    let wi = chat_frame(sheet)?;
    let at = CHAT_EDIT_AT;
    let size = [screen.clip_right() - at[0], CHAT_CLIP_BOTTOM - at[1]];
    let mut part = Part::runtime(
        pages::CHAT,
        WidgetKind::EditBox,
        CHAT_PANEL,
        CHAT_BOX_ID,
        CMD_CHAT_SEND,
        at,
        size,
        Look::Edit {
            font: CHAT_PAGE_FONT,
            size: CHAT_PAGE_SIZE,
            max_len: CHAT_MAX_CHARS,
            caret_colour: CHAT_CARET_COLOUR,
            caret_period: CHAT_CARET_PERIOD,
            clip: size[0],
            text_colour: CHAT_COLOUR,
        },
    );
    part.parent = Some(wi);
    part.hit = Some([at[0], at[1], size[0], size[1]]);
    sheet.parts.push(part);
    let i = sheet.parts.len() - 1;
    sheet.focus(Some(i));
    Some(i)
}

/// The chat edit box of `sheet`.
pub fn chat_box(sheet: &Sheet) -> Option<usize> {
    sheet.parts.iter().position(|p| {
        p.kind == WidgetKind::EditBox && p.id == CHAT_BOX_ID && p.rec == super::sheet::RUNTIME_REC
    })
}

/// The typed chat text.
pub fn typed_chat(sheet: &Sheet) -> &str {
    chat_box(sheet).map_or("", |i| sheet.parts[i].text.as_str())
}

/// What a send attempt came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatOutcome {
    /// Goes out with 0x3a0.
    Send,
    /// Muted: `Some(hours)` with an hour or more left (string 0x185d), `None` with less (0x185e).
    Muted(Option<u64>),
    /// The same line a fourth time within a minute of the third: refused (the field is cleared all the same).
    Repeated,
}

/// The repeat rule's state (one per session, as the 2009 globals).
#[derive(Debug, Clone, Default)]
pub struct ChatGuard {
    last: String,
    count: u32,
    since: Option<Instant>,
}

impl ChatGuard {
    /// Decides a send of `text` at `now` with `muted_minutes` of mute left, and counts it.
    // EVIDENCE: cshell+0x20ed0 (before: count > 2 -> refused while less than 60 s since the count passed 2, else
    // count and time reset; after the send path: the same text as the last -> count + 1, past 2 the time is taken;
    // another text -> it becomes the last, count 1), cshell+0x449b0 (slash command, mute, flood checks, then 0x3a0),
    // cshell+0x40ca0 (muted while at least a minute is left)
    // DC-DEFAULT: CLI-485 (proposed) the 2009 flood block (a timer ring of unknown size, cshell+0x3c5d0) is not
    // checked (amended by gm-client: the operator's slash commands are [`submit_chat_at`]'s, CLI-1301; the
    // operator's lines do not count as repeats, [`ChatGuard::submit_as`], CLI-1304)
    pub fn submit(&mut self, text: &str, muted_minutes: u64, now: Instant) -> ChatOutcome {
        self.submit_as(text, muted_minutes, now, false)
    }

    /// [`ChatGuard::submit`] for an `operator` or not: an operator's same line starts the count again.
    // EVIDENCE: cshell+0x20ed0 (the same text as the last AND the operator flag clear -> count + 1; else the text
    // becomes the last with count 1)
    pub fn submit_as(
        &mut self,
        text: &str,
        muted_minutes: u64,
        now: Instant,
        operator: bool,
    ) -> ChatOutcome {
        if self.count > REPEAT_LIMIT {
            if self
                .since
                .is_some_and(|t| now.saturating_duration_since(t) < REPEAT_WAIT)
            {
                return ChatOutcome::Repeated;
            }
            self.count = 0;
            self.since = None;
        }
        let out = if muted_minutes > 0 {
            let hours = muted_minutes / 60;
            ChatOutcome::Muted((hours >= 1).then_some(hours))
        } else {
            ChatOutcome::Send
        };
        if text == self.last && !operator {
            self.count += 1;
            if self.count > REPEAT_LIMIT {
                self.since = Some(now);
            }
        } else {
            self.last = text.to_string();
            self.count = 1;
        }
        out
    }
}

/// Whole minutes of mute left at `now` (local FILETIME ticks), 0 when the end has passed.
// EVIDENCE: cshell+0x1b1640 (seconds from now to the stored end; below 1 -> 0, else seconds / 60)
pub fn mute_minutes_left(until: u64, now: u64) -> u64 {
    until.saturating_sub(now) / TICKS_PER_MINUTE
}

/// The send of the chat page (Enter in the field or its Enter button): an empty field does nothing; else the rules
/// decide, a warning line explains a refusal, and the field is cleared (a repeat refusal clears it too:
/// cshell+0x20ed0 calls the edit control's clear, cshell+0x2f4930, before leaving the 0x1028 branch).
/// Returns whether the field had text.
pub fn submit_chat(sheet: &mut Sheet, ctx: &mut Ctx) -> bool {
    submit_chat_at(
        sheet,
        ctx,
        dc_botclient::clock::local_ticks(),
        Instant::now(),
    )
}

/// [`submit_chat`] at a given local clock (FILETIME ticks) and monotonic time.
pub fn submit_chat_at(sheet: &mut Sheet, ctx: &mut Ctx, local_now: u64, now: Instant) -> bool {
    let Some(i) = chat_box(sheet) else {
        return false;
    };
    let text = sheet.parts[i].text.clone();
    if text.is_empty() {
        return false;
    }
    // CLI-1841 / CLI-1843: `/report`, `/diag`, `/nettest` are the client's own (never sent, before the repeat guard)
    if super::diagbox::lobby_line(&text, ctx) {
        sheet.parts[i].text.clear();
        sheet.focus(Some(i));
        return true;
    }
    let left = mute_minutes_left(ctx.data.muted_until, local_now);
    let operator = ctx.data.gm.operator;
    let outcome = ctx.data.chat_guard.submit_as(&text, left, now, operator);
    // CLI-1301: an operator's slash line is a command (before the mute check), never chat
    // EVIDENCE: cshell+0x20ed0 (repeat refusal first), cshell+0x449b0 (cshell+0x42920 before the mute check and the
    // send)
    if outcome != ChatOutcome::Repeated && operator {
        let words = ctx.cat.paths.gm.words.clone();
        if let Some(cmd) = super::gm::parse_command(&text, words.as_ref()) {
            run_op_command(cmd, ctx);
            sheet.parts[i].text.clear();
            sheet.focus(Some(i));
            return true;
        }
    }
    match outcome {
        // the repeat warning's colour is the mute line's (CLI-487)
        ChatOutcome::Repeated => {
            let line = ctx.cat.text(TEXT_REPEATED);
            ctx.data.say_in(line, WARN_COLOUR);
        }
        ChatOutcome::Muted(hours) => {
            let line = match hours {
                Some(h) => printf_int(
                    &ctx.cat.text(TEXT_MUTED_HOURS),
                    h.min(i32::MAX as u64) as i32,
                ),
                None => ctx.cat.text(TEXT_MUTED),
            };
            ctx.data.say_in(line, WARN_COLOUR);
        }
        ChatOutcome::Send if ctx.data.chat_target == super::chatdrop::TARGET_CLAN => {
            // the chat target "Clan chat": the line goes to the clan's members over the relay
            // DC-DEFAULT: CLI-563 (the clan chat's relay command)
            ctx.send(LinkCmd::Messenger(super::clan::clan_chat_command(&text)))
        }
        ChatOutcome::Send => ctx.send(LinkCmd::Chat(text)),
    }
    sheet.parts[i].text.clear();
    sheet.focus(Some(i));
    true
}

/// CLI-1301: sends an operator command and keeps it for its reply's wording.
fn run_op_command(cmd: super::gm::OpCommand, ctx: &mut Ctx) {
    use super::gm::OpCommand;
    match cmd {
        OpCommand::Restrict(r) => {
            ctx.send(LinkCmd::OpRestrict {
                name: r.name.clone(),
                action: r.action,
                period: r.period,
                reason: r.reason.clone(),
            });
            ctx.data.gm.restricts.push_back(r);
        }
        OpCommand::Lookup(q) => ctx.send(LinkCmd::OpLookup(q)),
        OpCommand::Mute(m) => {
            ctx.send(LinkCmd::OpMute {
                name: m.name.clone(),
                on: m.on,
                minutes: m.minutes,
            });
            ctx.data.gm.mutes.push_back(m);
        }
        OpCommand::Nothing => {}
    }
}

/// A 0x3051 line as the chat shows it: the name, the separator and the text; GM senders (name starting with "GM")
/// in the GM colour.
// EVIDENCE: cshell+0x1c8d0 (name, then the three-character separator literal at VA 0x374cd9c4 and the text; a name
// beginning with the two-letter GM literal takes the GM font and colour), cshell+0x1ba40 (+0xa28 0xffffff00)
// DC-DEFAULT: CLI-487 (proposed) a name colour id above 0 (the colour table of the attributes archive) is drawn in
// the chat colour; GM lines keep the chat font
pub fn echo_line(name: &str, text: &str) -> (String, u32) {
    let colour = if name.starts_with("GM") {
        GM_COLOUR
    } else {
        CHAT_COLOUR
    };
    (format!("{name} : {text}"), colour)
}

/// The notice bar of the room list: x span, top and font.
// EVIDENCE: cshell+0x1211b0 (the bar object built with font 0xe size 0xe, left 0x97, top 0x5c, right 0x2e6, bottom
// 0x73, the text starting at the right edge), cshell+0x6b1f0 (one pixel per show time / (span + text width) ms:
// a pass lasts the show time), cshell+0x6cb60 (moved by elapsed time; hidden once its right end is left of the
// span), cshell+0x34270 (0x3052 colour 0xffeb191e)
pub const TICKER_LEFT: f32 = 0x97 as f32;
pub const TICKER_RIGHT: f32 = 0x2e6 as f32;
pub const TICKER_TOP: f32 = 0x5c as f32;
pub const TICKER_FONT: i32 = 0xe;
pub const TICKER_SIZE: i32 = 0xe;
pub const TICKER_COLOUR: u32 = 0xffeb_191e;
/// At most this many texts wait for the bar.
// EVIDENCE: cshell+0x6ca20 (with a text on the bar the new one joins the waiting list; more than 2 waiting -> the
// oldest goes)
pub const TICKER_QUEUE: usize = 2;
/// A pass's show time when the push gives none.
// DC-DEFAULT: CLI-488 (proposed) 0x3052 period = one pass's show time, repeated `count` times; a broadcast one pass
// of its seconds on the air; a zero time takes 10 s; every text in the 0x3052 colour; the bar shows on the room list
// only and keeps its queue across screens; texts are cut at whole characters at the span's ends
pub const TICKER_DEFAULT_MS: u32 = 10_000;

#[derive(Debug, Clone, PartialEq)]
struct TickerItem {
    text: String,
    pass_ms: u32,
    passes: u32,
    start: Instant,
}

impl TickerItem {
    fn end(&self) -> Instant {
        self.start + Duration::from_millis(u64::from(self.pass_ms) * u64::from(self.passes))
    }
}

/// The notice bar's queue: each text scrolls through the span once per pass, one text after the other.
#[derive(Debug, Clone, Default)]
pub struct Ticker {
    items: VecDeque<TickerItem>,
}

impl Ticker {
    /// Queues `text` at `now`.
    pub fn push(&mut self, text: String, show_ms: u32, repeat: u32, now: Instant) {
        self.items.retain(|t| t.end() > now);
        let pass_ms = if show_ms == 0 {
            TICKER_DEFAULT_MS
        } else {
            show_ms
        };
        let start = self.items.back().map_or(now, |t| t.end().max(now));
        self.items.push_back(TickerItem {
            text,
            pass_ms,
            passes: repeat.max(1),
            start,
        });
        // waiting texts beyond the limit: the oldest waiting one goes and the rest move up
        let waiting = self.items.iter().filter(|t| t.start > now).count();
        if waiting > TICKER_QUEUE {
            if let Some(k) = self.items.iter().position(|t| t.start > now) {
                self.items.remove(k);
                let mut at = self
                    .items
                    .iter()
                    .take(k)
                    .map(TickerItem::end)
                    .max()
                    .unwrap_or(now)
                    .max(now);
                for t in self.items.iter_mut().skip(k) {
                    t.start = at;
                    at = t.end();
                }
            }
        }
    }

    /// A text is on the bar at `now`.
    pub fn active(&self, now: Instant) -> bool {
        self.items.iter().any(|t| t.start <= now && t.end() > now)
    }

    /// The text on the bar at `now` and how far its pass has run (0..1).
    pub fn current(&self, now: Instant) -> Option<(&str, f32)> {
        let t = self
            .items
            .iter()
            .find(|t| t.start <= now && t.end() > now)?;
        let ms = now.saturating_duration_since(t.start).as_millis() as u64;
        let pass = u64::from(t.pass_ms.max(1));
        Some((&t.text, (ms % pass) as f32 / pass as f32))
    }
}

/// The bar's text at `now`: the characters inside the span at the pass's place (the text enters at the right end
/// and has left the span when the pass ends).
pub fn draw_ticker(cat: &mut Catalog, ticker: &Ticker, now: Instant, out: &mut Vec<DrawItem>) {
    let Some((text, f)) = ticker.current(now) else {
        return;
    };
    let mut m = |t: &str| {
        cat.measure_text(TICKER_FONT, TICKER_SIZE, t)
            .map_or(0.0, |m| m.0)
    };
    let width = m(text);
    let x0 = TICKER_RIGHT - f * (TICKER_RIGHT - TICKER_LEFT + width);
    // characters whose box lies inside the span
    let (mut first, mut last) = (None, None);
    let mut x = x0;
    for (k, ch) in text.char_indices() {
        let w = m(&text[k..k + ch.len_utf8()]);
        if x >= TICKER_LEFT && x + w <= TICKER_RIGHT {
            first.get_or_insert((k, x));
            last = Some(k + ch.len_utf8());
        }
        x += w;
    }
    if let (Some((a, xa)), Some(b)) = (first, last) {
        out.push(place_text(
            &text[a..b],
            [xa.round() as i32, TICKER_TOP as i32],
            [0, 0],
            Align::default(),
            TICKER_FONT,
            TICKER_SIZE,
            TICKER_COLOUR,
        ));
    }
}

/// Keeps a 0x3051 line in the chat.
pub fn say_chat(data: &mut MenuData, name: &str, text: &str) {
    let (line, colour) = echo_line(name, text);
    data.say_in(line, colour);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repeat_rule_refuses_a_fourth_same_line_within_a_minute_of_the_third() {
        let t0 = Instant::now();
        let mut g = ChatGuard::default();
        for k in 0..3 {
            assert_eq!(
                g.submit("hi", 0, t0 + Duration::from_secs(k)),
                ChatOutcome::Send
            );
        }
        let third = t0 + Duration::from_secs(2);
        assert_eq!(
            g.submit("hi", 0, third + Duration::from_secs(10)),
            ChatOutcome::Repeated
        );
        // another text is refused as well while the wait runs (the 2009 check comes before the text compare)
        assert_eq!(
            g.submit("other", 0, third + Duration::from_secs(30)),
            ChatOutcome::Repeated
        );
        // a minute after the third: allowed again
        assert_eq!(
            g.submit("other", 0, third + Duration::from_secs(61)),
            ChatOutcome::Send
        );
        assert_eq!(
            g.submit("hi", 0, third + Duration::from_secs(62)),
            ChatOutcome::Send
        );
    }

    #[test]
    fn the_mute_needs_a_whole_minute_and_names_hours() {
        let m = TICKS_PER_MINUTE;
        assert_eq!(mute_minutes_left(0, 1_000 * m), 0);
        assert_eq!(
            mute_minutes_left(10 * m + m / 2, 10 * m),
            0,
            "30 s left: not muted"
        );
        assert_eq!(mute_minutes_left(12 * m, 10 * m), 2);
        let mut g = ChatGuard::default();
        let now = Instant::now();
        assert_eq!(g.submit("a", 2, now), ChatOutcome::Muted(None));
        assert_eq!(g.submit("b", 125, now), ChatOutcome::Muted(Some(2)));
        assert_eq!(g.submit("c", 0, now), ChatOutcome::Send);
    }

    #[test]
    fn gm_lines_take_the_gm_colour() {
        assert_eq!(
            echo_line("Bob", "hi"),
            ("Bob : hi".to_string(), CHAT_COLOUR)
        );
        assert_eq!(echo_line("GM_Ann", "x").1, GM_COLOUR);
    }

    #[test]
    fn the_ticker_runs_its_passes_by_the_clock_and_queues_at_most_two() {
        let t0 = Instant::now();
        let mut k = Ticker::default();
        k.push("a".into(), 4_000, 2, t0);
        k.push("b".into(), 1_000, 1, t0);
        assert_eq!(
            k.current(t0 + Duration::from_millis(1_000)),
            Some(("a", 0.25))
        );
        assert_eq!(
            k.current(t0 + Duration::from_millis(5_000)),
            Some(("a", 0.25)),
            "second pass"
        );
        assert_eq!(
            k.current(t0 + Duration::from_millis(8_500)),
            Some(("b", 0.5))
        );
        assert!(!k.active(t0 + Duration::from_millis(9_000)));
        let mut q = Ticker::default();
        for (i, s) in ["1", "2", "3", "4", "5"].iter().enumerate() {
            q.push(
                s.to_string(),
                1_000,
                1,
                t0 + Duration::from_millis(i as u64),
            );
        }
        let now = t0 + Duration::from_millis(10);
        let texts: Vec<&str> = q.items.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["1", "4", "5"], "one on the bar, two waiting");
        assert_eq!(q.current(now).map(|c| c.0), Some("1"));
    }

    #[test]
    fn a_long_line_shows_its_tail() {
        let w = |t: &str| t.chars().count() as f32 * 10.0;
        assert_eq!(fit("abcdef", 35.0, w), "def");
        assert_eq!(fit("ab", 35.0, w), "ab");
    }

    fn fit(t: &str, width: f32, m: impl FnMut(&str) -> f32) -> String {
        super::super::sheet::fitting_tail(t, width, m).to_string()
    }
}

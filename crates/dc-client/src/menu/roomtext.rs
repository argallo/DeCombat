//! Room setting values as the room creation box and the waiting room show them: the room number, the player count,
//! the goal, the weapon limit, and the greedy word wrap of the map description. Every text comes from the ROM's
//! string table at run time; only the numeric forms are written here.
// EVIDENCE: cshell+0x167e90 (waiting room settings: (5, 9) number, (6, 11) description, (6, 19) players, (6, 17)
// goal, (6, 14) mode full name, (6, 21) weapon limit), cshell+0x1391e0 / cshell+0x138e90 (room creation box:
// players (1, 16) and mode full name (1, 17) through the same helpers)

use super::catalog::Catalog;

/// Mode rows whose player count reads as two equal teams.
// EVIDENCE: cshell+0x262510 (rows 0, 1, 4, 6, 8: `%d vs %d` with half the maximum each)
pub const TEAM_ROWS: [i32; 5] = [0, 1, 4, 6, 8];
/// Mode rows whose player count is the players string (a printf form with the maximum).
// EVIDENCE: cshell+0x262510 (rows 2, 3, 5, 7: string 0x18f5 with the maximum)
pub const SOLO_ROWS: [i32; 4] = [2, 3, 5, 7];
pub const PLAYERS_FORM: i32 = 0x18f5;

/// Goal strings (FormatMessage forms with the goal as insert 1) per mode row.
// EVIDENCE: cshell+0x2623f0 (rows 0, 3, 6: 0x152c; rows 1, 7, 8: 0x152d; row 5: 0x1830; row 2: 0x56f / 0x570 /
// 0x571 for goal 0 / 1 / 2; row 4: the integer followed by a points word), cshell+0x1a0280 (LoadString +
// FormatMessage with the goal)
pub const GOAL_KILLS: i32 = 0x152c;
pub const GOAL_ROUNDS: i32 = 0x152d;
pub const GOAL_MINUTES: i32 = 0x1830;
pub const GOAL_LEVELS: [i32; 3] = [0x56f, 0x570, 0x571];

/// Weapon limit strings by the room's weapon limit (0x389 +0x84).
// EVIDENCE: cshell+0x265780 (the list built at load in this order), cshell+0x263a10 (index -> string id, -1 past
// the list), cshell+0x167e90 ((6, 21) gets the string of the room's weapon limit)
pub const ARMS_TEXTS: [i32; 9] = [
    0x1772, 0x1773, 0x1774, 0x1775, 0x1776, 0x177f, 0x1794, 0x18ad, 0x18b2,
];

/// Wrap width of the waiting room's map description (virtual pixels).
// DISASM: cshell+0x168108 (after its text, control (6, 11) gets vtable +0x140 with 0xeb, 0)
// OBSERVED: lobbyfx4/real/R20_room.png (2009: six lines, the longest x 775..1003)
pub const DESCRIPTION_WRAP: i32 = 0xeb;
/// The same width in DeCombat's own text measure (unhinted advances, CLI-724), and the lines' pitch.
// DC-DEFAULT: CLI-751 (proposed) the description breaks at 231 of DeCombat's measure (OBSERVED
// screens-sweep/cpp/create/shop_bb01_waiting.png: the reference stack's Junk Flea breaks bracket the
// limit to 230.2 .. 232.3; the 2009 Warhead breaks of R20_room.png fit it), 16 px apart (OBSERVED
// lobbyfx4/real/R20_room.png: 2009 lines at y 429, 445, .. 509)
pub const DESCRIPTION_WRAP_MEASURE: i32 = 231;
pub const DESCRIPTION_PITCH: i32 = 16;

/// The room number as the waiting room shows it: three digits and a dot, one more than the wire number.
// EVIDENCE: cshell+0x167e90 (`%03d.` of the room's +0x552c + 1), cshell+0x1684c0 (+0x552c = 0x389 +0xe)
// SPEC: PROTOCOL_2009.md §3.7 table 3.7a (+0xe room number, shown + 1)
pub fn room_number_label(number: u32) -> String {
    format!("{:03}.", u64::from(number) + 1)
}

/// The player count of mode row `mode` with `max` players.
// EVIDENCE: cshell+0x262510 (other rows: nothing)
pub fn player_count_label(cat: &Catalog, mode: i32, max: i32) -> String {
    if TEAM_ROWS.contains(&mode) {
        let half = max / 2;
        format!("{half} vs {half}")
    } else if SOLO_ROWS.contains(&mode) {
        printf_int(&cat.text(PLAYERS_FORM), max)
    } else {
        String::new()
    }
}

/// The goal of mode row `mode`.
pub fn goal_label(cat: &Catalog, mode: i32, goal: i32) -> String {
    match mode {
        0 | 3 | 6 => format_message(&cat.text(GOAL_KILLS), goal),
        1 | 7 | 8 => format_message(&cat.text(GOAL_ROUNDS), goal),
        5 => format_message(&cat.text(GOAL_MINUTES), goal),
        2 => usize::try_from(goal)
            .ok()
            .and_then(|g| GOAL_LEVELS.get(g))
            .map(|id| cat.text(*id))
            .unwrap_or_default(),
        // EVIDENCE: cshell+0x2623f0 (row 4: the wide printf form at cshell+0x3f4334, the integer and a points word,
        // read from the user's binary: `catalog::pins::GOAL_POINTS_FORM`)
        // DC-DEFAULT: CLI-542 (proposed) without that literal the goal shows the number alone
        4 => match cat.paths.goal_points.as_deref() {
            Some(form) => printf_int(form, goal),
            None => goal.to_string(),
        },
        _ => String::new(),
    }
}

/// The weapon limit text (empty past the list).
pub fn weapon_rule_label(cat: &Catalog, weapons: i32) -> String {
    usize::try_from(weapons)
        .ok()
        .and_then(|i| ARMS_TEXTS.get(i))
        .map(|id| cat.text(*id))
        .unwrap_or_default()
}

/// A printf form with one integer conversion.
pub fn printf_int(form: &str, n: i32) -> String {
    form.replacen("%d", &n.to_string(), 1)
}

/// A FormatMessage form with the integer `n` as insert 1 (`%1` with or without a `!d!` type).
pub fn format_message(form: &str, n: i32) -> String {
    let n = n.to_string();
    form.replace("%1!d!", &n).replace("%1", &n)
}

/// Greedy word wrap: words separated by single spaces fill a line while its width stays within `width`; a word wider
/// than a line gets a line of its own; a newline in the text always breaks.
// DC-DEFAULT: CLI-477 (proposed) the description wraps at spaces, greedily, keeping the text's own spaces
pub fn wrap_lines(text: &str, width: f32, mut measure: impl FnMut(&str) -> f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            if line.is_empty() {
                line.push_str(word);
                continue;
            }
            let wider = format!("{line} {word}");
            if measure(&wider) <= width {
                line = wider;
            } else {
                out.push(std::mem::take(&mut line));
                line.push_str(word);
            }
        }
        out.push(line);
    }
    out
}

/// `lines` cut to at most `max`: the last kept line loses words until it fits `width` with "..." after it.
// DC-DEFAULT: CLI-1626 (proposed) the map description under a Hacks row keeps four lines
pub fn cut_lines(
    mut lines: Vec<String>,
    max: usize,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    if max == 0 || lines.len() <= max {
        return lines;
    }
    lines.truncate(max);
    if let Some(last) = lines.last_mut() {
        let mut words: Vec<&str> = last.split(' ').collect();
        let mut out = format!("{}...", words.join(" "));
        while measure(&out) > width && words.len() > 1 {
            words.pop();
            out = format!("{}...", words.join(" "));
        }
        *last = out;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Catalog {
        Catalog::fallback(None, 1024, 768)
    }

    #[test]
    fn room_number_has_three_digits_a_dot_and_counts_from_one() {
        assert_eq!(room_number_label(0), "001.");
        assert_eq!(room_number_label(41), "042.");
        assert_eq!(room_number_label(999), "1000.");
    }

    #[test]
    fn team_rows_split_the_maximum() {
        let c = cat();
        assert_eq!(player_count_label(&c, 8, 16), "8 vs 8");
        assert_eq!(player_count_label(&c, 0, 12), "6 vs 6");
        // ROM-less catalogs show the string id
        assert_eq!(player_count_label(&c, 3, 12), format!("#{PLAYERS_FORM}"));
        assert_eq!(player_count_label(&c, 9, 12), "");
    }

    #[test]
    fn goal_follows_the_mode_row() {
        let c = cat();
        // row 4: the code's own form (none without a ROM: the number alone)
        assert_eq!(goal_label(&c, 4, 7), "7");
        let mut f = cat();
        f.paths.goal_points = Some("%d pts".into());
        assert_eq!(goal_label(&f, 4, 7), "7 pts");
        assert_eq!(goal_label(&c, 2, 1), format!("#{}", GOAL_LEVELS[1]));
        assert_eq!(goal_label(&c, 2, 3), "");
        assert_eq!(goal_label(&c, 1, 7), format!("#{GOAL_ROUNDS}"));
        assert_eq!(format_message("%1!d! R", 7), "7 R");
        assert_eq!(format_message("x %1", 3), "x 3");
    }

    #[test]
    fn arms_past_the_list_are_empty() {
        let c = cat();
        assert_eq!(weapon_rule_label(&c, 0), format!("#{}", ARMS_TEXTS[0]));
        assert_eq!(weapon_rule_label(&c, 9), "");
        assert_eq!(weapon_rule_label(&c, -1), "");
    }

    #[test]
    fn wrap_is_greedy_and_keeps_long_words() {
        let m = |s: &str| s.chars().count() as f32;
        assert_eq!(wrap_lines("aa bb cc", 5.0, m), vec!["aa bb", "cc"]);
        assert_eq!(wrap_lines("aaaaaaa b", 5.0, m), vec!["aaaaaaa", "b"]);
        assert_eq!(wrap_lines("a\nb", 50.0, m), vec!["a", "b"]);
        assert_eq!(wrap_lines("", 5.0, m), vec![""]);
    }

    /// CLI-1626: a description cut to two lines ends its second line with "..." inside the width.
    #[test]
    fn cut_lines_keeps_the_first_lines_and_marks_the_cut() {
        let w = |t: &str| t.chars().count() as f32;
        let lines = wrap_lines("aa bb cc dd ee ff gg", 5.0, w);
        assert_eq!(lines, ["aa bb", "cc dd", "ee ff", "gg"]);
        assert_eq!(cut_lines(lines.clone(), 2, 5.0, w), ["aa bb", "cc..."]);
        assert_eq!(cut_lines(lines.clone(), 4, 5.0, w), lines);
        assert_eq!(cut_lines(lines.clone(), 0, 5.0, w), lines);
    }
}

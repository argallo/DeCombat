//! The world's command language at run time, the full form (DSV-570): the statements of the trigger network a
//! mission world drives its stages with, parsed into a small tree and run by the doors' statement runner
//! ([`crate::doors::run_world`], one interpreter for every command text of the world).
//!
//! The grammar is read only from the command texts the user's ROM worlds carry (USER DIRECTION 2026-11-05, Q-1 of
//! `docs/FIRETEAM_AI_DESIGN.md` decided for option B): statements are separated by `;` or line breaks; a statement
//! is a word list with parenthesised groups, and an unclosed group ends with its statement (the ROM texts leave
//! some open). The statement forms and what each does here are DeCombat rules (DSV-570..DSV-574); a form this file
//! does not know is skipped and counted.
//!
//! The objects the statements address beside the doors, triggers and models: event counters (a value and the
//! commands run when it is counted up to given values), timers (a time, the command run when they are stopped in
//! time and the one run when the time runs out) and activation boxes (a box a player stays in for a
//! time). Their world objects are read by [`crate::world`] (classes and properties by hash).
// OBSERVED: ROM SANDHOG_CP1 and the other co-op world's command texts (statement words, argument forms and
// object kinds only; hashes in the tree, no ROM text)
// DC-DEFAULT: DSV-570..DSV-574 (proposed)

use std::collections::BTreeMap;

use crate::world::PropHash;
use dc_net_engine::time::later;
use dc_net_engine::Millis;

fn word_key(w: &[u8]) -> PropHash {
    let l = w.to_ascii_lowercase();
    (dc_world::hash16(&l), l.len())
}

/// The statement words (hash of the lower-case word and its length).
// OBSERVED: the ROM world files' command texts (hashes only)
pub(crate) const WORD_MSG: PropHash = ([0xe4, 0x6b, 0x32, 0x01, 0x65, 0xee, 0xc9, 0x1e], 3);
const WORD_DELAY: PropHash = ([0x6e, 0x16, 0xdb, 0x10, 0x22, 0x67, 0xe6, 0xe7], 5);
const WORD_DELAYID: PropHash = ([0x9b, 0xef, 0x46, 0xd3, 0xbc, 0x6c, 0x13, 0x20], 7);
const WORD_DELAYVARIABLE: PropHash = ([0x58, 0x0c, 0x6b, 0xad, 0x60, 0x80, 0xb6, 0x9d], 13);
const WORD_ABORT: PropHash = ([0x3a, 0x53, 0xdb, 0x8a, 0x2c, 0x8a, 0x17, 0xee], 5);
const WORD_INT: PropHash = ([0x6d, 0xa8, 0x8c, 0x34, 0xba, 0x12, 0x4c, 0x41], 3);
const WORD_SET: PropHash = ([0x6e, 0xe0, 0xeb, 0x49, 0x0f, 0xf8, 0x32, 0x10], 3);
pub(crate) const WORD_ADD: PropHash = ([0x7e, 0x9e, 0x5a, 0xc3, 0x0f, 0x22, 0x16, 0xfd], 3);
const WORD_SUB: PropHash = ([0xdd, 0xc6, 0xe2, 0xb2, 0x24, 0xd0, 0xfd, 0x82], 3);
const WORD_IF: PropHash = ([0x93, 0x5f, 0x68, 0x31, 0x9d, 0x4f, 0x22, 0x7e], 2);
const WORD_THEN: PropHash = ([0x21, 0xaf, 0x6f, 0x12, 0x60, 0xf9, 0x27, 0xf5], 4);
const WORD_AND: PropHash = ([0x62, 0x01, 0x11, 0x1b, 0x83, 0xa0, 0xcb, 0x5b], 3);
const WORD_OR: PropHash = ([0x71, 0x75, 0x51, 0x7a, 0x37, 0x0b, 0x5c, 0xd2], 2);
const WORD_RAND2: PropHash = ([0x08, 0x73, 0x1e, 0xdf, 0xd3, 0x9c, 0x7d, 0x89], 5);
const WORD_RAND3: PropHash = ([0x72, 0xfb, 0xd7, 0xd8, 0x9c, 0x5a, 0x13, 0x26], 5);
/// Message words beside the doors' (`on`, `off`, `lock`, `unlock`, `trigger`, `visible`, `solid`).
pub(crate) const WORD_SPAWN: PropHash = ([0xa0, 0x4a, 0xfb, 0x36, 0x8a, 0x49, 0x46, 0xb0], 5);
pub(crate) const WORD_INC: PropHash = ([0xfa, 0x93, 0x83, 0xa4, 0xbc, 0x91, 0x06, 0xe8], 3);
pub(crate) const WORD_DEC: PropHash = ([0xe7, 0x50, 0x2c, 0x79, 0x9b, 0x8f, 0x76, 0xfb], 3);
pub(crate) const WORD_TIMEAMOUNT: PropHash = ([0xf3, 0x2f, 0xb4, 0x7b, 0x83, 0xc8, 0xa2, 0x18], 10);
pub(crate) const WORD_START: PropHash = ([0xcc, 0xed, 0x28, 0xc6, 0xdc, 0x3f, 0x99, 0xc2], 5);
pub(crate) const WORD_STOP: PropHash = ([0x6c, 0x45, 0xcb, 0x72, 0xa3, 0x6e, 0x63, 0xd5], 4);
pub(crate) const WORD_CHECK: PropHash = ([0x20, 0xf6, 0x5c, 0x28, 0x67, 0x1b, 0x40, 0x93], 5);
pub(crate) const WORD_FAIL: PropHash = ([0x51, 0x28, 0x0d, 0xab, 0xfb, 0xc8, 0x80, 0xcd], 4);
pub(crate) const WORD_INCLIFECOUNT: PropHash =
    ([0x15, 0xb0, 0x96, 0xd6, 0x28, 0x6d, 0x1f, 0xa9], 12);
pub(crate) const WORD_MISSIONCOMPLETE: PropHash =
    ([0x73, 0xf9, 0xc3, 0xd6, 0x3f, 0x1d, 0x45, 0xd1], 15);
pub(crate) const WORD_AIFREEZE: PropHash = ([0x6e, 0x9c, 0x05, 0x1a, 0x11, 0x23, 0x60, 0x90], 8);
pub(crate) const WORD_COOPTIMER: PropHash = ([0xcd, 0xac, 0x2b, 0xb1, 0x0b, 0x33, 0x4a, 0xb5], 9);
pub(crate) const WORD_REMOVE: PropHash = ([0x7e, 0x56, 0x08, 0xab, 0x61, 0x00, 0x17, 0xaf], 6);
pub(crate) const WORD_HIDDEN: PropHash = ([0xe5, 0x64, 0xb4, 0x08, 0x1d, 0x7a, 0x9e, 0xa4], 6);
/// The round world's words (DSV-578): `(healinpart)` to the game control, `pause` to an effect object.
// OBSERVED: ROM CabinFever command texts (the kill counter's round-end events, the ending triggers)
pub(crate) const WORD_HEALINPART: PropHash = ([0x0d, 0x19, 0x84, 0x71, 0x43, 0xce, 0x66, 0x44], 10);
pub(crate) const WORD_PAUSE: PropHash = ([0x62, 0x10, 0xc0, 0xbf, 0x05, 0x39, 0x67, 0x16], 5);
/// The presentation words of the co-op worlds (DSV-579..DSV-582): `(play <key>)` to a caption or message trigger,
/// `(moveto <point>)` / `(fov x y)` to a camera, `(hidecharacter 0|1)` to the game control, `enable` / `disable` to
/// an AI volume.
// OBSERVED: ROM SANDHOG_CP1 and CabinFever command texts (word forms and argument kinds only; hashes in the tree)
pub(crate) const WORD_PLAY: PropHash = ([0xab, 0xa4, 0xcc, 0x9b, 0x8a, 0xf8, 0x2d, 0xf5], 4);
pub(crate) const WORD_MOVETO: PropHash = ([0x8a, 0xa1, 0x9c, 0xf4, 0x44, 0xbb, 0x8f, 0x1d], 6);
pub(crate) const WORD_FOV: PropHash = ([0x86, 0x4f, 0xa3, 0x6e, 0xe7, 0xf5, 0x48, 0xaa], 3);
pub(crate) const WORD_HIDECHARACTER: PropHash =
    ([0xa9, 0xec, 0xae, 0xe9, 0x91, 0x57, 0xfe, 0xcb], 13);
pub(crate) const WORD_ENABLE: PropHash = ([0xe9, 0x71, 0x66, 0xb5, 0x45, 0x00, 0xda, 0x3a], 6);
pub(crate) const WORD_DISABLE: PropHash = ([0xe9, 0xd8, 0x99, 0x2f, 0x34, 0x81, 0x62, 0xfd], 7);

/// Nesting of groups a statement may have (deeper groups are read as words of their parent).
// DC-DEFAULT: DSV-570 (proposed) 32 (the ROM texts nest at most 5 deep)
pub const SCRIPT_DEPTH: usize = 32;

/// A token of one statement: a word or a parenthesised group.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Word(Vec<u8>),
    Group(Vec<Node>),
}

/// A number or a variable (names compare without case; an unset variable is 0).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f32),
    Var(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    Cmp(Value, Cmp, Value),
    All(Vec<Cond>),
    Any(Vec<Cond>),
    /// A condition this file cannot read: false.
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarOp {
    /// `int` (declare) and `set`.
    Set,
    Add,
    Sub,
}

/// One statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `msg <target> <words...>`: the words after the target, groups flattened (`(spawn _A)` -> `spawn _A`).
    Msg {
        target: Vec<u8>,
        words: Vec<Vec<u8>>,
    },
    /// `delay <secs> (...)`, `delayid <id> <secs> (...)`, `delayvariable <var> (...)`: the body after the wait.
    Delay {
        wait: Value,
        id: Option<Vec<u8>>,
        body: Vec<Stmt>,
    },
    /// `abort <id>`: drops the waiting `delayid` bodies of that id.
    Abort(Vec<u8>),
    /// `int` / `set` / `add` / `sub` `<var> <value>`.
    Var {
        op: VarOp,
        name: Vec<u8>,
        value: Value,
    },
    /// `if (<cond>) then (<statements>)`; conditions compare with `==`, `!=`, `<`, `<=`, `>`, `>=` and combine with
    /// `and` / `or`.
    If { cond: Cond, then: Vec<Stmt> },
    /// `rand2` / `rand3` `(...)(...)[(...)]`: one of the groups.
    Rand(Vec<Vec<Stmt>>),
    /// A statement this file does not run.
    Other,
}

fn tokens(piece: &[u8]) -> Vec<Node> {
    let mut stack: Vec<Vec<Node>> = vec![Vec::new()];
    let mut word: Vec<u8> = Vec::new();
    let flush = |word: &mut Vec<u8>, stack: &mut Vec<Vec<Node>>| {
        if !word.is_empty() {
            if let Some(top) = stack.last_mut() {
                top.push(Node::Word(std::mem::take(word)));
            }
        }
    };
    let op = |b: u8| matches!(b, b'=' | b'<' | b'>' | b'!');
    for &b in piece {
        match b {
            b'(' => {
                flush(&mut word, &mut stack);
                if stack.len() <= SCRIPT_DEPTH {
                    stack.push(Vec::new());
                }
            }
            b')' => {
                flush(&mut word, &mut stack);
                if stack.len() > 1 {
                    let g = stack.pop().unwrap_or_default();
                    if let Some(top) = stack.last_mut() {
                        top.push(Node::Group(g));
                    }
                }
            }
            _ if b.is_ascii_whitespace() => flush(&mut word, &mut stack),
            _ => {
                // a comparison operator is a word of its own (`a==1`)
                if !word.is_empty() && op(b) != op(word[word.len() - 1]) {
                    flush(&mut word, &mut stack);
                }
                word.push(b);
            }
        }
    }
    flush(&mut word, &mut stack);
    while stack.len() > 1 {
        let g = stack.pop().unwrap_or_default();
        if let Some(top) = stack.last_mut() {
            top.push(Node::Group(g));
        }
    }
    stack.pop().unwrap_or_default()
}

fn flat(nodes: &[Node], out: &mut Vec<Vec<u8>>) {
    for n in nodes {
        match n {
            Node::Word(w) => out.push(w.clone()),
            Node::Group(g) => flat(g, out),
        }
    }
}

fn word_at(nodes: &[Node], i: usize) -> Option<&[u8]> {
    match nodes.get(i)? {
        Node::Word(w) => Some(w),
        Node::Group(g) => match g.as_slice() {
            [Node::Word(w)] => Some(w),
            _ => None,
        },
    }
}

fn number(w: &[u8]) -> Option<f32> {
    let t = std::str::from_utf8(w).ok()?.trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn value_of(w: Option<&[u8]>) -> Value {
    match w {
        Some(w) => match number(w) {
            Some(n) => Value::Num(n),
            None => Value::Var(w.to_ascii_lowercase()),
        },
        None => Value::Num(0.0),
    }
}

fn cmp_of(w: &[u8]) -> Option<Cmp> {
    Some(match w {
        b"==" | b"=" => Cmp::Eq,
        b"!=" | b"<>" => Cmp::Ne,
        b"<" => Cmp::Lt,
        b"<=" | b"=<" => Cmp::Le,
        b">" => Cmp::Gt,
        b">=" | b"=>" => Cmp::Ge,
        _ => return None,
    })
}

fn cond_of(nodes: &[Node]) -> Cond {
    let is = |n: &Node, k: PropHash| matches!(n, Node::Word(w) if word_key(w) == k);
    let split = |k: PropHash| -> Vec<&[Node]> { nodes.split(|n| is(n, k)).collect() };
    if nodes.iter().any(|n| is(n, WORD_OR)) {
        return Cond::Any(split(WORD_OR).into_iter().map(cond_of).collect());
    }
    if nodes.iter().any(|n| is(n, WORD_AND)) {
        return Cond::All(split(WORD_AND).into_iter().map(cond_of).collect());
    }
    match nodes {
        [Node::Group(g)] => cond_of(g),
        [_, Node::Word(op), _] => match (cmp_of(op), word_at(nodes, 0), word_at(nodes, 2)) {
            (Some(c), Some(a), Some(b)) => Cond::Cmp(value_of(Some(a)), c, value_of(Some(b))),
            _ => Cond::Never,
        },
        _ => Cond::Never,
    }
}

/// The statements of a token list: a leading word starts one statement (the rest are its arguments); groups
/// before it are statements of their own (`then ((msg a on)(msg b on))`).
fn stmts_of(nodes: &[Node]) -> Vec<Stmt> {
    let mut out = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        match n {
            Node::Group(g) => out.extend(stmts_of(g)),
            Node::Word(_) => {
                out.push(stmt_of(&nodes[i..]));
                break;
            }
        }
    }
    out
}

fn stmt_of(nodes: &[Node]) -> Stmt {
    let Some(head) = word_at(nodes, 0) else {
        return Stmt::Other;
    };
    let k = word_key(head);
    let secs = |i: usize| value_of(word_at(nodes, i));
    if k == WORD_MSG {
        let Some(target) = word_at(nodes, 1) else {
            return Stmt::Other;
        };
        let mut words = Vec::new();
        flat(nodes.get(2..).unwrap_or(&[]), &mut words);
        return Stmt::Msg {
            target: target.to_vec(),
            words,
        };
    }
    if k == WORD_DELAY {
        return Stmt::Delay {
            wait: secs(1),
            id: None,
            body: stmts_of(nodes.get(2..).unwrap_or(&[])),
        };
    }
    if k == WORD_DELAYID {
        return Stmt::Delay {
            wait: secs(2),
            id: word_at(nodes, 1).map(<[u8]>::to_ascii_lowercase),
            body: stmts_of(nodes.get(3..).unwrap_or(&[])),
        };
    }
    if k == WORD_DELAYVARIABLE {
        return match word_at(nodes, 1) {
            Some(v) => Stmt::Delay {
                wait: Value::Var(v.to_ascii_lowercase()),
                id: None,
                body: stmts_of(nodes.get(2..).unwrap_or(&[])),
            },
            None => Stmt::Other,
        };
    }
    if k == WORD_ABORT {
        return word_at(nodes, 1).map_or(Stmt::Other, |id| Stmt::Abort(id.to_ascii_lowercase()));
    }
    let op = if k == WORD_INT || k == WORD_SET {
        Some(VarOp::Set)
    } else if k == WORD_ADD {
        Some(VarOp::Add)
    } else if k == WORD_SUB {
        Some(VarOp::Sub)
    } else {
        None
    };
    if let Some(op) = op {
        return match word_at(nodes, 1) {
            Some(name) => Stmt::Var {
                op,
                name: name.to_ascii_lowercase(),
                value: value_of(word_at(nodes, 2)),
            },
            None => Stmt::Other,
        };
    }
    if k == WORD_IF {
        let cond = match nodes.get(1) {
            Some(Node::Group(g)) => cond_of(g),
            _ => Cond::Never,
        };
        let then_at = nodes
            .iter()
            .position(|n| matches!(n, Node::Word(w) if word_key(w) == WORD_THEN));
        return match then_at {
            Some(t) => Stmt::If {
                cond,
                then: stmts_of(&nodes[t + 1..]),
            },
            None => Stmt::Other,
        };
    }
    if k == WORD_RAND2 || k == WORD_RAND3 {
        let options: Vec<Vec<Stmt>> = nodes[1..]
            .iter()
            .filter_map(|n| match n {
                Node::Group(g) => Some(stmts_of(g)),
                Node::Word(_) => None,
            })
            .collect();
        return if options.is_empty() {
            Stmt::Other
        } else {
            Stmt::Rand(options)
        };
    }
    Stmt::Other
}

/// The statements of a command text, in order.
// DC-DEFAULT: DSV-570 (proposed)
pub fn parse_script(text: &[u8]) -> Vec<Stmt> {
    text.split(|&b| b == b';' || b == b'\n' || b == b'\r')
        .flat_map(|piece| stmts_of(&tokens(piece)))
        .collect()
}

/// Whether a command text spawns AI characters from the world's spawners (`msg <spawner> (spawn <name>)`, also
/// inside `if`, `delay` and `rand` bodies): the mark of a mission world.
pub fn spawns_ai(text: &[u8]) -> bool {
    fn any(s: &[Stmt]) -> bool {
        s.iter().any(|s| match s {
            Stmt::Msg { words, .. } => words.first().is_some_and(|w| word_key(w) == WORD_SPAWN),
            Stmt::Delay { body, .. } => any(body),
            Stmt::If { then, .. } => any(then),
            Stmt::Rand(o) => o.iter().any(|b| any(b)),
            _ => false,
        })
    }
    any(&parse_script(text))
}

/// Every `msg` statement of a statement list, also inside `if`, `delay` and `rand` bodies.
fn msgs<'s>(list: &'s [Stmt], out: &mut Vec<(&'s [u8], &'s [Vec<u8>])>) {
    for s in list {
        match s {
            Stmt::Msg { target, words } => out.push((target, words)),
            Stmt::Delay { body, .. } => msgs(body, out),
            Stmt::If { then, .. } => msgs(then, out),
            Stmt::Rand(o) => o.iter().for_each(|b| msgs(b, out)),
            _ => {}
        }
    }
}

/// The round loop of a round world (DSV-576): the world counts its rounds on one event counter and ends each round
/// with the events of another counter whose values the file leaves unset.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoundNet {
    /// The triggers the server fires when the match starts: they count the round counter up and no command text of
    /// the world fires them.
    pub start_boxes: Vec<Vec<u8>>,
    /// The counter whose value is the round number.
    pub round_counter: Vec<u8>,
    /// The counter the AI templates' death commands count up (the kill counter).
    pub kill_counter: Vec<u8>,
    /// Per round number: the command the server runs when that round is cleared (the kill counter's unset events).
    pub clear_events: Vec<(u32, Vec<u8>)>,
}

/// Reads the round loop of a world from its triggers, its event counters and its AI templates' death commands
/// (`None`: not a round world). A start box is a trigger that counts up a counter with events and that no text of
/// the world fires; the kill counter is a counter the death commands count up whose events are all unset.
// DC-DEFAULT: DSV-576 (proposed)
// OBSERVED: ROM CabinFever (two logic boxes count the round counter up: one fired by nothing, one by the kill
// counter's events; the kill counter's twenty events have commands and the value -1; its AI templates' death
// commands count it up)
pub fn round_network(
    triggers: &[crate::doors::ScriptTrigger],
    objects: &MissionObjects,
    death_cmds: &[&[u8]],
) -> Option<RoundNet> {
    let parsed = |t: &[u8]| parse_script(t);
    let mut texts: Vec<Vec<Stmt>> = Vec::new();
    texts.extend(
        triggers
            .iter()
            .flat_map(|t| t.cmds.iter().map(|c| parsed(c))),
    );
    for c in &objects.counters {
        texts.extend(c.events.iter().map(|e| parsed(&e.1)));
        texts.extend(c.unset.iter().map(|e| parsed(&e.1)));
    }
    for t in &objects.timers {
        texts.push(parsed(&t.in_time));
        texts.push(parsed(&t.over_time));
    }
    for a in &objects.activates {
        for c in [&a.work, &a.release, &a.activate] {
            texts.push(parsed(c));
        }
    }
    let deaths: Vec<Vec<Stmt>> = death_cmds.iter().map(|d| parsed(d)).collect();
    texts.extend(deaths.iter().cloned());
    let mut fired: Vec<Vec<u8>> = Vec::new();
    for t in &texts {
        let mut m = Vec::new();
        msgs(t, &mut m);
        for (target, words) in m {
            if words
                .first()
                .is_some_and(|w| word_key(w) == crate::doors::WORD_TRIGGER)
            {
                fired.push(target.to_ascii_lowercase());
            }
        }
    }
    let incs = |list: &[Stmt]| -> Vec<Vec<u8>> {
        let mut m = Vec::new();
        msgs(list, &mut m);
        m.into_iter()
            .filter(|(_, w)| w.first().is_some_and(|w| word_key(w) == WORD_INC))
            .map(|(t, _)| t.to_ascii_lowercase())
            .collect()
    };
    let counter = |n: &[u8]| {
        objects
            .counters
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(n))
    };
    let kill = deaths
        .iter()
        .flat_map(|d| incs(d))
        .filter_map(|n| counter(&n))
        .find(|c| c.events.is_empty() && !c.unset.is_empty())?;
    let mut start_boxes = Vec::new();
    let mut round_counter: Option<Vec<u8>> = None;
    for t in triggers {
        if fired.contains(&t.name.to_ascii_lowercase()) {
            continue;
        }
        let list: Vec<Stmt> = t.cmds.iter().flat_map(|c| parsed(c)).collect();
        let Some(c) = incs(&list)
            .into_iter()
            .filter_map(|n| counter(&n))
            .find(|c| !c.events.is_empty() && !c.name.eq_ignore_ascii_case(&kill.name))
        else {
            continue;
        };
        if round_counter
            .as_deref()
            .is_some_and(|r| !r.eq_ignore_ascii_case(&c.name))
        {
            continue;
        }
        round_counter = Some(c.name.clone());
        start_boxes.push(t.name.clone());
    }
    Some(RoundNet {
        start_boxes,
        round_counter: round_counter?,
        kill_counter: kill.name.clone(),
        clear_events: kill.unset.clone(),
    })
}

// ------------------------------------------------------------------------------------------- world objects

/// An event counter of the world file: its start value and, per event, the value it is counted up to and the
/// command run when it gets there.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CounterInfo {
    pub name: Vec<u8>,
    pub start: i32,
    pub events: Vec<(i32, Vec<u8>)>,
    /// The events whose value the file leaves unset (below 0) but whose command it gives: the event number (1-based)
    /// and the command. Counting never reaches them; a round world's server runs them (DSV-576).
    pub unset: Vec<(u32, Vec<u8>)>,
}

/// A timer of the world file: its time and the commands run when it is stopped in time / when the time runs out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TimerInfo {
    pub name: Vec<u8>,
    pub secs: f32,
    pub in_time: Vec<u8>,
    pub over_time: Vec<u8>,
}

/// An activation box of the world file: a box a player stays in for `secs` (DSV-573).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ActivateInfo {
    pub name: Vec<u8>,
    pub pos: [f32; 3],
    /// Half extents.
    pub dims: [f32; 3],
    pub locked: bool,
    /// Activations in all (0: no limit).
    pub max: u32,
    pub secs: f32,
    /// Run when a player starts, gives up, completes the activation.
    pub work: Vec<u8>,
    pub release: Vec<u8>,
    pub activate: Vec<u8>,
}

/// The mission objects of a world beside its triggers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MissionObjects {
    pub counters: Vec<CounterInfo>,
    pub timers: Vec<TimerInfo>,
    pub activates: Vec<ActivateInfo>,
}

/// What a statement asks of the game (the co-op rules run them, DSV-571 / DSV-572).
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `msg <spawner> (spawn <name>)` (or `spawn` alone): the spawner creates its target AI under `name`.
    Spawn {
        spawner: Vec<u8>,
        name: Option<Vec<u8>>,
    },
    /// `(inclifecount n)` to the game control: every human gets `n` lives more.
    Lives(i32),
    /// `(missioncomplete 1)`: the mission is complete.
    Complete,
    /// `(aifreeze 0|1)`: the AI characters stand still and hold fire, or go on.
    Freeze(bool),
    /// `(cooptimer on|off)`.
    MissionTimer(bool),
    /// A mission goal: `(add n)` shows goal `n`, `(check n)` completes it, `(fail n)` fails it.
    Goal(GoalOp, u32),
    /// The round counter of a round world was counted up to `n`: round `n` starts (DSV-576).
    Round(u32),
    /// `(healinpart)` to the game control: the living humans recover part of their health (DSV-578).
    HealInPart,
}

/// What a statement asks the clients to show (DSV-579..DSV-582): the session and the co-op rules resolve the names
/// against the world's cameras, music triggers and text triggers.
#[derive(Debug, Clone, PartialEq)]
pub enum Cue {
    /// `msg <trigger> (play <key>)`: a caption or message trigger shows the dialogue text `key` of the GMS.
    Play { trigger: Vec<u8>, key: Vec<u8> },
    /// `msg <target> on|off` (a camera, a music trigger; other names are not presentation objects).
    Switch { target: Vec<u8>, on: bool },
    /// `msg <camera> (moveto <point>)`.
    MoveTo { camera: Vec<u8>, point: Vec<u8> },
    /// `msg <camera> (fov x y)` in degrees.
    Fov { camera: Vec<u8>, fov: [f32; 2] },
    /// `(hidecharacter 0|1)` to the game control (the cut scenes).
    HideCharacters(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalOp {
    Add,
    Check,
    Fail,
}

/// Most effects one room keeps between two game ticks (the rest are dropped and counted).
// DC-DEFAULT: DSV-570 (proposed) 1024, as the statement budget of one run
pub const EFFECTS_MAX: usize = 1024;

/// How far around its box a player's position still counts as inside an activation box.
// DC-DEFAULT: DSV-573 (proposed) 16 units, as the touched triggers (DSV-563)
pub const ACTIVATE_MARGIN: f32 = 16.0;

/// The run-time state of the mission objects and the variables of one room.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Network {
    pub objects: MissionObjects,
    pub vars: BTreeMap<Vec<u8>, f32>,
    pub counters: Vec<i32>,
    /// The time each timer runs out (`None`: not running), and its current time in seconds.
    pub timers: Vec<(Option<Millis>, f32)>,
    pub activate_locked: Vec<bool>,
    pub activate_fires: Vec<u32>,
    /// The player who works a box and since when.
    pub working: Vec<Option<(u8, Millis)>>,
    pub effects: Vec<Effect>,
    /// Command texts to run with the next statements (counter events, timers, activation boxes).
    pub queued: Vec<Vec<u8>>,
    /// The `on` / `off` messages of the run, by target name (the session switches the kill volumes of those names,
    /// DSV-571).
    pub switched: Vec<(Vec<u8>, bool)>,
    /// Statements of a form this file does not run, and messages nobody here takes.
    pub skipped: u32,
    /// The round counter of a round world (DSV-576): counting it up to `n` starts round `n` ([`Effect::Round`]).
    pub round_counter: Option<Vec<u8>>,
    /// Statements the server takes but has nothing to send for (`pause` to an effect object, DSV-578).
    pub client_only: u32,
    /// What the run asks the clients to show, in statement order (DSV-579..DSV-582).
    pub cues: Vec<Cue>,
    /// The AI volumes the statements disabled (lower-case names; `enable` takes one back, DSV-582): server state for
    /// the AI navigation.
    pub ai_volumes_off: std::collections::BTreeSet<Vec<u8>>,
    rng: u64,
}

impl Network {
    pub fn new(objects: MissionObjects, seed: u64) -> Network {
        let mut n = Network {
            objects,
            rng: seed | 1,
            ..Network::default()
        };
        n.reset();
        n
    }

    /// The state of a new round: the file's values and locks, no variable set, nothing running.
    pub fn reset(&mut self) {
        let o = &self.objects;
        self.vars.clear();
        self.counters = o.counters.iter().map(|c| c.start).collect();
        self.timers = o.timers.iter().map(|t| (None, t.secs)).collect();
        self.activate_locked = o.activates.iter().map(|a| a.locked).collect();
        self.activate_fires = vec![0; o.activates.len()];
        self.working = vec![None; o.activates.len()];
        self.effects.clear();
        self.queued.clear();
        self.switched.clear();
        self.cues.clear();
        self.ai_volumes_off.clear();
    }

    /// A presentation cue of the run (DSV-579..DSV-582); past [`EFFECTS_MAX`] waiting cues the rest are counted.
    pub fn push_cue(&mut self, c: Cue) {
        if self.cues.len() < EFFECTS_MAX {
            self.cues.push(c);
        } else {
            self.skipped = self.skipped.saturating_add(1);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.objects == MissionObjects::default()
    }

    /// A deterministic draw in `0..n` (xorshift64*).
    // DC-DEFAULT: FT-17 (proposed) seeded per room
    pub fn draw(&mut self, n: usize) -> usize {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % n.max(1)
    }

    pub fn var(&self, name: &[u8]) -> f32 {
        self.vars
            .get(&name.to_ascii_lowercase())
            .copied()
            .unwrap_or(0.0)
    }

    pub fn value(&self, v: &Value) -> f32 {
        match v {
            Value::Num(n) => *n,
            Value::Var(name) => self.var(name),
        }
    }

    pub fn test(&self, c: &Cond) -> bool {
        match c {
            Cond::Cmp(a, op, b) => {
                let (a, b) = (self.value(a), self.value(b));
                match op {
                    Cmp::Eq => a == b,
                    Cmp::Ne => a != b,
                    Cmp::Lt => a < b,
                    Cmp::Le => a <= b,
                    Cmp::Gt => a > b,
                    Cmp::Ge => a >= b,
                }
            }
            Cond::All(v) => !v.is_empty() && v.iter().all(|c| self.test(c)),
            Cond::Any(v) => v.iter().any(|c| self.test(c)),
            Cond::Never => false,
        }
    }

    pub fn set_var(&mut self, op: VarOp, name: &[u8], v: f32) {
        let e = self.vars.entry(name.to_ascii_lowercase()).or_insert(0.0);
        match op {
            VarOp::Set => *e = v,
            VarOp::Add => *e += v,
            VarOp::Sub => *e -= v,
        }
    }

    pub fn push_effect(&mut self, e: Effect) {
        if self.effects.len() < EFFECTS_MAX {
            self.effects.push(e);
        } else {
            self.skipped = self.skipped.saturating_add(1);
        }
    }

    fn queue(&mut self, text: &[u8]) {
        if !text.is_empty() && self.queued.len() < EFFECTS_MAX {
            self.queued.push(text.to_vec());
        }
    }

    /// `msg <counter> (inc n)` / `(dec n)`: the counter's value changes; an event whose value it was counted up to
    /// runs its command. False when no counter has that name.
    // DC-DEFAULT: DSV-572 (proposed) an event runs when an `inc` brings the value to the event's value
    pub fn count(&mut self, name: &[u8], by: i32) -> bool {
        let mut hit = false;
        for i in 0..self.objects.counters.len() {
            if !self.objects.counters[i].name.eq_ignore_ascii_case(name) {
                continue;
            }
            hit = true;
            let before = self.counters[i];
            let after = before.saturating_add(by);
            self.counters[i] = after;
            // DC-DEFAULT: DSV-576 (proposed) the round counter counted up starts the round of its new value
            let is_round = self
                .round_counter
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(name));
            if by > 0 && is_round && after > 0 {
                self.push_effect(Effect::Round(after as u32));
            }
            if by > 0 {
                let due: Vec<Vec<u8>> = self.objects.counters[i]
                    .events
                    .iter()
                    .filter(|(v, _)| before < *v && after >= *v)
                    .map(|(_, c)| c.clone())
                    .collect();
                for c in due {
                    self.queue(&c);
                }
            }
        }
        hit
    }

    /// `msg <timer> start` / `stop` / `(timeamount secs)`. False when no timer has that name.
    // DC-DEFAULT: DSV-572 (proposed) stopped while running: its in-time command; run out: its over-time command
    pub fn timer(&mut self, name: &[u8], word: PropHash, arg: Option<f32>, now: Millis) -> bool {
        let mut hit = false;
        for i in 0..self.objects.timers.len() {
            if !self.objects.timers[i].name.eq_ignore_ascii_case(name) {
                continue;
            }
            hit = true;
            if word == WORD_START {
                let ms = (self.timers[i].1.max(0.0) * 1000.0).min(86_400_000.0) as u32;
                self.timers[i].0 = Some(now.wrapping_add(ms));
            } else if word == WORD_STOP {
                if self.timers[i].0.take().is_some() {
                    let c = self.objects.timers[i].in_time.clone();
                    self.queue(&c);
                }
            } else if word == WORD_TIMEAMOUNT {
                if let Some(s) = arg.filter(|s| *s >= 0.0) {
                    self.timers[i].1 = s;
                }
            }
        }
        hit
    }

    /// The timers whose time ran out at `now`: their over-time commands are queued.
    pub fn timers_due(&mut self, now: Millis) {
        for i in 0..self.timers.len() {
            if self.timers[i].0.is_some_and(|t| !later(t, now)) {
                self.timers[i].0 = None;
                let c = self.objects.timers[i].over_time.clone();
                self.queue(&c);
            }
        }
    }

    /// `lock` / `unlock` of the activation boxes of that name.
    pub fn set_activate_lock(&mut self, name: &[u8], locked: bool) {
        for (i, a) in self.objects.activates.iter().enumerate() {
            if a.name.eq_ignore_ascii_case(name) {
                self.activate_locked[i] = locked;
            }
        }
    }

    fn inside(a: &ActivateInfo, p: [f32; 3]) -> bool {
        (0..3).all(|k| (p[k] - a.pos[k]).abs() <= a.dims[k] + ACTIVATE_MARGIN)
    }

    /// One step of the activation boxes with the living players `(pid, position)`: an unlocked box with activations
    /// left that a player stands in starts its work (the work command runs); the worker who left the box, died or
    /// was locked out gives up (the release command runs); one who stayed for the box's time completes it (the
    /// activate command runs, one activation spent).
    // DC-DEFAULT: DSV-573 (proposed) staying in the box is the activation, as the Spy Hunt uplink (DSV-233 / DSV-323)
    pub fn activate_tick(&mut self, players: &[(u8, [f32; 3])], now: Millis) {
        for i in 0..self.working.len() {
            let a = &self.objects.activates[i];
            let Some((pid, since_at)) = self.working[i] else {
                let spent = a.max > 0 && self.activate_fires[i] >= a.max;
                if self.activate_locked[i] || spent {
                    continue;
                }
                if let Some((p, _)) = players.iter().find(|(_, at)| Self::inside(a, *at)) {
                    self.working[i] = Some((*p, now));
                    let c = a.work.clone();
                    self.queue(&c);
                }
                continue;
            };
            let here = players
                .iter()
                .any(|(p, at)| *p == pid && Self::inside(a, *at));
            if !here || self.activate_locked[i] {
                self.working[i] = None;
                let c = a.release.clone();
                self.queue(&c);
                continue;
            }
            let ms = (a.secs.max(0.0) * 1000.0) as u32;
            if dc_net_engine::time::since(now, since_at) >= ms {
                self.working[i] = None;
                self.activate_fires[i] = self.activate_fires[i].saturating_add(1);
                let c = a.activate.clone();
                self.queue(&c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(t: &str, w: &[&str]) -> Stmt {
        Stmt::Msg {
            target: t.as_bytes().to_vec(),
            words: w.iter().map(|w| w.as_bytes().to_vec()).collect(),
        }
    }

    /// The forms of DSV-570 as the ROM texts write them, including groups left open at the statement's end.
    #[test]
    fn statement_forms() {
        let s = parse_script(b"int A 10;\r\nSub A 1;\r\nset B 2.5\r\nmsg S_X (spawn _X);");
        assert_eq!(
            s,
            [
                Stmt::Var {
                    op: VarOp::Set,
                    name: b"a".to_vec(),
                    value: Value::Num(10.0)
                },
                Stmt::Var {
                    op: VarOp::Sub,
                    name: b"a".to_vec(),
                    value: Value::Num(1.0)
                },
                Stmt::Var {
                    op: VarOp::Set,
                    name: b"b".to_vec(),
                    value: Value::Num(2.5)
                },
                msg("S_X", &["spawn", "_X"]),
            ]
        );
        let s = parse_script(b"if (A <= 3) then (msg T trigger);");
        assert_eq!(
            s,
            [Stmt::If {
                cond: Cond::Cmp(Value::Var(b"a".to_vec()), Cmp::Le, Value::Num(3.0)),
                then: vec![msg("T", &["trigger"])]
            }]
        );
        // an unclosed group ends with its statement; several groups after `then` are several statements
        let s = parse_script(b"delay 3 (msg C (play X);\nif ((A == 1) AND (B==2)) then ((msg D on)(delay 1 (msg D off)));");
        assert_eq!(s.len(), 2);
        assert_eq!(
            s[0],
            Stmt::Delay {
                wait: Value::Num(3.0),
                id: None,
                body: vec![msg("C", &["play", "X"])]
            }
        );
        let Stmt::If { cond, then } = &s[1] else {
            panic!("{:?}", s[1]);
        };
        assert!(matches!(cond, Cond::All(v) if v.len() == 2));
        assert_eq!(then.len(), 2);
        let s = parse_script(b"Rand3 (set S 3)(set S 2)(set S 1)");
        assert!(matches!(&s[0], Stmt::Rand(o) if o.len() == 3));
        let s = parse_script(
            b"delayid K 9.9 (msg M remove);\r\nabort K;\r\ndelayvariable W (msg S (spawn _S));",
        );
        assert!(matches!(&s[0], Stmt::Delay { id: Some(k), .. } if k == b"k"));
        assert_eq!(s[1], Stmt::Abort(b"k".to_vec()));
        assert!(matches!(&s[2], Stmt::Delay { wait: Value::Var(v), .. } if v == b"w"));
        assert_eq!(parse_script(b"frobnicate 1 2"), [Stmt::Other]);
        assert!(spawns_ai(b"if (A > 0) then (msg S (spawn _S));"));
        assert!(!spawns_ai(b"msg S on;"));
    }

    #[test]
    fn conditions_and_variables() {
        let mut n = Network::new(MissionObjects::default(), 7);
        n.set_var(VarOp::Set, b"A", 2.0);
        n.set_var(VarOp::Sub, b"a", 1.0);
        assert_eq!(n.var(b"A"), 1.0);
        let c = |t: &str| match &parse_script(t.as_bytes())[0] {
            Stmt::If { cond, .. } => n.test(cond),
            s => panic!("{s:?}"),
        };
        assert!(c("if (A == 1) then (x)"));
        assert!(!c("if (A > 1) then (x)"));
        assert!(c("if ((A == 2) OR (Unset == 0)) then (x)"));
        assert!(!c("if ((A == 1) AND (Unset == 1)) then (x)"));
        assert!(!c("if (A) then (x)"), "an unreadable condition is false");
        let draws: Vec<usize> = (0..30).map(|_| n.draw(3)).collect();
        assert!(draws.iter().all(|d| *d < 3) && draws.iter().any(|d| *d != draws[0]));
    }

    #[test]
    fn counters_timers_and_activation_boxes() {
        let mut n = Network::new(
            MissionObjects {
                counters: vec![CounterInfo {
                    name: b"Lock".to_vec(),
                    start: 0,
                    events: vec![(2, b"msg Done unlock".to_vec())],
                    unset: Vec::new(),
                }],
                timers: vec![TimerInfo {
                    name: b"T".to_vec(),
                    secs: 10.0,
                    in_time: b"set X 1".to_vec(),
                    over_time: b"set X 0".to_vec(),
                }],
                activates: vec![ActivateInfo {
                    name: b"Box".to_vec(),
                    pos: [0.0; 3],
                    dims: [10.0; 3],
                    locked: true,
                    max: 1,
                    secs: 2.0,
                    work: b"w".to_vec(),
                    release: b"r".to_vec(),
                    activate: b"a".to_vec(),
                }],
            },
            1,
        );
        assert!(n.count(b"lock", 1) && n.queued.is_empty());
        n.count(b"lock", 1);
        assert_eq!(n.queued, [b"msg Done unlock".to_vec()], "counted up to 2");
        n.count(b"lock", 1);
        assert_eq!(n.queued.len(), 1, "only on the way up to the value");
        n.queued.clear();
        n.timer(b"t", WORD_TIMEAMOUNT, Some(3.0), 0);
        n.timer(b"t", WORD_START, None, 0);
        n.timers_due(2999);
        assert!(n.queued.is_empty());
        n.timers_due(3000);
        assert_eq!(n.queued, [b"set X 0".to_vec()], "ran out");
        n.queued.clear();
        n.timer(b"t", WORD_START, None, 5000);
        n.timer(b"t", WORD_STOP, None, 6000);
        assert_eq!(n.queued, [b"set X 1".to_vec()], "stopped in time");
        n.queued.clear();
        // the box: locked, then worked for its time by the player standing in it
        let at = [5.0, 0.0, 0.0];
        n.activate_tick(&[(1, at)], 0);
        assert!(n.queued.is_empty(), "locked");
        n.set_activate_lock(b"box", false);
        n.activate_tick(&[(1, [100.0, 0.0, 0.0])], 0);
        assert!(n.queued.is_empty(), "outside");
        n.activate_tick(&[(1, at)], 0);
        n.activate_tick(&[(1, at), (2, at)], 1000);
        n.activate_tick(&[(2, at)], 1500);
        assert_eq!(
            n.queued,
            [b"w".to_vec(), b"r".to_vec()],
            "the worker left: released"
        );
        n.queued.clear();
        n.activate_tick(&[(2, at)], 2000);
        n.activate_tick(&[(2, at)], 4000);
        assert_eq!(
            n.queued,
            [b"w".to_vec(), b"a".to_vec()],
            "held 2 s: activated"
        );
        n.queued.clear();
        n.activate_tick(&[(2, at)], 5000);
        assert!(n.queued.is_empty(), "one activation");
    }
}

//! The world's start scripts (DSV-37): the start-up command objects, the trigger objects' numbered commands and the
//! mode controllers' object-start commands, and their evaluation at a room's start. The game server takes from it
//! which objects it creates and which hazard volumes act (DSV-607); the bot reads the same world and evaluates the
//! same rule to know which hazard volumes hurt (BOT-177), so both run this one code.

use std::collections::BTreeMap;

use crate::objects::{FieldValue, Object};
use crate::{hash16, names, ObjectRole, World};

/// A command word as hash and length of its lower-case text.
pub type WordKey = ([u8; 8], usize);

/// The scripts of a world: what its start-up command objects, trigger objects and mode controllers say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartScripts {
    /// The start-up command objects' commands, in file order.
    pub startup: Vec<Vec<u8>>,
    /// Trigger objects: name -> their numbered commands in order.
    pub triggers: Vec<(Vec<u8>, Vec<Vec<u8>>)>,
    /// Mode controllers: name -> object-start command.
    pub controls: Vec<(Vec<u8>, Vec<u8>)>,
}

/// The same scripts borrowed (the game server keeps them in its own world record).
#[derive(Debug, Clone, Copy)]
pub struct ScriptsRef<'s> {
    pub startup: &'s [Vec<u8>],
    pub triggers: &'s [(Vec<u8>, Vec<Vec<u8>>)],
    pub controls: &'s [(Vec<u8>, Vec<u8>)],
}

impl StartScripts {
    pub fn as_ref(&self) -> ScriptsRef<'_> {
        ScriptsRef {
            startup: &self.startup,
            triggers: &self.triggers,
            controls: &self.controls,
        }
    }
}

fn text_named<'a>(o: &Object<'a>, k: &names::Key) -> Option<&'a [u8]> {
    match o.prop_named(k).map(|p| p.value) {
        Some(FieldValue::Text(t)) => Some(t),
        _ => None,
    }
}

/// Whether an object of this role is one of the script objects [`start_scripts`] reads.
pub fn is_script_role(role: Option<ObjectRole>) -> bool {
    matches!(
        role,
        Some(ObjectRole::StartupCommand)
            | Some(ObjectRole::TriggerVolume)
            | Some(ObjectRole::ModeController)
    )
}

/// The world's start scripts, in file order.
// SPEC: FMT_DAT85.md §6.2 (object roles); OBSERVED: command texts of the ROM world files (hashes only)
// DC-DEFAULT: DSV-37 (extended, staged)
pub fn start_scripts(w: &World<'_>) -> StartScripts {
    let mut s = StartScripts::default();
    for o in &w.objects {
        let name = || o.name().map(<[u8]>::to_vec).unwrap_or_default();
        match o.role {
            Some(ObjectRole::StartupCommand) => {
                if let Some(t) = text_named(o, &names::PROP_COMMAND) {
                    s.startup.push(t.to_vec());
                }
            }
            Some(ObjectRole::TriggerVolume) => {
                let cmds: Vec<Vec<u8>> = names::PROP_TRIGGER_COMMANDS
                    .iter()
                    .filter_map(|k| text_named(o, k))
                    .filter(|t| !t.is_empty())
                    .map(<[u8]>::to_vec)
                    .collect();
                s.triggers.push((name(), cmds));
            }
            Some(ObjectRole::ModeController) => {
                if let Some(t) = text_named(o, &names::PROP_OBJECT_START_COMMAND) {
                    s.controls.push((name(), t.to_vec()));
                }
            }
            _ => {}
        }
    }
    s
}

/// Command words of the world's command texts (hash and length of the lower-case word).
// OBSERVED: command texts of the ROM world files (hashes only): the message statement, its trigger / on / off /
// visible / solid arguments and the delayed statement
pub const WORD_MSG: WordKey = ([0xe4, 0x6b, 0x32, 0x01, 0x65, 0xee, 0xc9, 0x1e], 3);
pub const WORD_TRIGGER: WordKey = ([0x68, 0x32, 0x59, 0xfe, 0xab, 0xbf, 0x5e, 0xb3], 7);
pub const WORD_ON: WordKey = ([0xb8, 0xd3, 0x1e, 0x85, 0x27, 0x25, 0xaf, 0xb1], 2);
pub const WORD_OFF: WordKey = ([0xb4, 0xdc, 0x66, 0xdd, 0xe8, 0x06, 0x26, 0x1b], 3);
pub const WORD_VISIBLE: WordKey = ([0xd4, 0x2e, 0xf1, 0x49, 0x79, 0x00, 0xbc, 0x6e], 7);
pub const WORD_SOLID: WordKey = ([0x4b, 0x3d, 0x4a, 0x0d, 0xd7, 0x32, 0xf7, 0x91], 5);

/// The key of a command word.
pub fn word_key(w: &[u8]) -> WordKey {
    let l = w.to_ascii_lowercase();
    (hash16(&l), l.len())
}

/// The 0 / 1 value of a `visible` / `solid` statement: a number counts by its value (`0`, `0.0` are false), the words
/// `true` / `false` by their meaning (any case), anything else is true when it has a byte other than `0`.
// DC-DEFAULT: DSV-562 (proposed) the ROM worlds write only `0` and `1`; the start scripts read the value the same way
// (DSV-37)
pub fn script_flag(v: &[u8]) -> bool {
    if let Some(n) = std::str::from_utf8(v)
        .ok()
        .and_then(|t| t.trim().parse::<f32>().ok())
    {
        return n != 0.0;
    }
    if v.eq_ignore_ascii_case(b"true") {
        return true;
    }
    if v.eq_ignore_ascii_case(b"false") {
        return false;
    }
    v.iter().any(|&b| b != b'0')
}

/// What the start scripts did to one named object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptOverride {
    pub visible: Option<bool>,
    pub solid: Option<bool>,
    pub on: Option<bool>,
}

/// Limits of the script evaluation (nesting of trigger messages, statements in all).
const SCRIPT_DEPTH: u32 = 8;
const SCRIPT_STATEMENTS: u32 = 4096;

struct ScriptRun<'a> {
    scripts: ScriptsRef<'a>,
    out: BTreeMap<Vec<u8>, ScriptOverride>,
    statements: u32,
    /// The triggers fired, one entry per fire (DSV-562 seeds the run-time counts with them).
    fired: Vec<Vec<u8>>,
}

impl ScriptRun<'_> {
    fn fire(&mut self, name: &[u8], depth: u32) {
        if depth > SCRIPT_DEPTH {
            return;
        }
        let Some(cmds) = self
            .scripts
            .triggers
            .iter()
            .find(|t| t.0.eq_ignore_ascii_case(name))
            .map(|t| t.1.clone())
        else {
            return;
        };
        self.fired.push(name.to_vec());
        for c in &cmds {
            self.run(c, depth + 1);
        }
    }

    fn run(&mut self, text: &[u8], depth: u32) {
        for st in text.split(|&b| b == b';' || b == b'\n' || b == b'\r') {
            self.statements += 1;
            if self.statements > SCRIPT_STATEMENTS {
                return;
            }
            let words: Vec<&[u8]> = st
                .split(|b| b.is_ascii_whitespace() || *b == b'(' || *b == b')')
                .filter(|w| !w.is_empty())
                .collect();
            // only immediate message statements; delayed ones (and every other statement) do not run at the start
            if words.len() < 3 || word_key(words[0]) != WORD_MSG {
                continue;
            }
            let target = words[1].to_ascii_lowercase();
            let arg = word_key(words[2]);
            let value = words.get(3).map(|v| script_flag(v));
            if arg == WORD_TRIGGER {
                self.fire(&target, depth);
                continue;
            }
            let o = self.out.entry(target).or_default();
            if arg == WORD_ON {
                o.on = Some(true);
            } else if arg == WORD_OFF {
                o.on = Some(false);
            } else if arg == WORD_VISIBLE {
                if let Some(v) = value {
                    o.visible = Some(v);
                }
            } else if arg == WORD_SOLID {
                if let Some(v) = value {
                    o.solid = Some(v);
                }
            }
        }
    }
}

/// Evaluates the world's start scripts: every start-up command object, then the room's start triggers (the first two
/// fields of the room's `trig` setting: the mission's and the map's start trigger) and the object-start command of
/// the mode controller it names (the fourth field). Only immediate `msg <object> <argument>` statements count:
/// `trigger` fires a trigger object's numbered commands, `on` / `off`, `visible 0|1` and `solid 0|1` change the
/// object; delayed statements and the end trigger (third field) do not run. Names compare without case. Returns the
/// overrides by lower-case object name and the triggers fired (one entry per fire, in order).
// OBSERVED: capture 20261025_065016_tdm6 (the reference stack's objects of that world at load are those
// this evaluation leaves: the two objects the start-up command made non-solid or switched off and the model the start
// trigger made invisible and non-solid were removed before the real client joined, the effect switched off by it was
// never created, the breakables made invisible by the mission start trigger and visible again by the map's start
// trigger were created visible; REFERENCE); DSERVER_PIPE.md §3.2 (`trig` field order of the room line, our lobby's
// writer in dc-bute)
// DC-DEFAULT: DSV-37 (extended, staged) the start-script evaluation
pub fn start_scripts_run(
    scripts: ScriptsRef<'_>,
    trig: Option<&str>,
) -> (BTreeMap<Vec<u8>, ScriptOverride>, Vec<Vec<u8>>) {
    let mut r = ScriptRun {
        scripts,
        out: BTreeMap::new(),
        statements: 0,
        fired: Vec::new(),
    };
    for c in scripts.startup {
        r.run(c, 1);
    }
    let fields: Vec<&str> = trig.map(|t| t.split('|').collect()).unwrap_or_default();
    for f in fields.iter().take(2) {
        if !f.trim().is_empty() {
            r.fire(f.trim().as_bytes(), 1);
        }
    }
    if let Some(ctrl) = fields.get(3).map(|f| f.trim()).filter(|f| !f.is_empty()) {
        if let Some(cmd) = scripts
            .controls
            .iter()
            .find(|c| c.0.eq_ignore_ascii_case(ctrl.as_bytes()))
            .map(|c| c.1.clone())
        {
            r.run(&cmd, 1);
        }
    }
    (r.out, r.fired)
}

/// Whether the start scripts leave the object `name` acting: every object acts unless they sent it `off` (DSV-607,
/// the hazard volumes).
// DC-DEFAULT: DSV-607 (proposed)
pub fn left_on(ov: &BTreeMap<Vec<u8>, ScriptOverride>, name: &[u8]) -> bool {
    ov.get(&name.to_ascii_lowercase()).and_then(|o| o.on) != Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts() -> StartScripts {
        StartScripts {
            startup: vec![b"msg Door1 (solid 0)".to_vec()],
            triggers: vec![
                (
                    b"MapStart".to_vec(),
                    vec![b"msg Hazard off; msg Sub trigger".to_vec()],
                ),
                (b"Sub".to_vec(), vec![b"msg Water OFF".to_vec()]),
                (b"OtherStart".to_vec(), vec![b"msg Fx on".to_vec()]),
            ],
            controls: vec![(b"Ctrl".to_vec(), b"msg Lamp visible 0".to_vec())],
        }
    }

    #[test]
    fn the_start_triggers_switch_objects_off_and_others_stay_on() {
        let s = scripts();
        let (ov, fired) = start_scripts_run(s.as_ref(), Some("|MapStart|End|Ctrl"));
        assert!(!left_on(&ov, b"HAZARD"));
        assert!(!left_on(&ov, b"water"));
        assert!(left_on(&ov, b"fx"));
        assert!(left_on(&ov, b"unnamed"));
        assert_eq!(ov[&b"door1".to_vec()].solid, Some(false));
        assert_eq!(ov[&b"lamp".to_vec()].visible, Some(false));
        assert_eq!(fired, vec![b"MapStart".to_vec(), b"sub".to_vec()]);
        // another room's start trigger leaves the hazard on; the end trigger never runs
        let (ov, _) = start_scripts_run(s.as_ref(), Some("OtherStart||MapStart|"));
        assert!(left_on(&ov, b"hazard"));
        let (ov, _) = start_scripts_run(s.as_ref(), None);
        assert!(left_on(&ov, b"hazard"));
    }

    #[test]
    fn script_flags_read_numbers_and_words() {
        for (v, want) in [
            (&b"0"[..], false),
            (b"1", true),
            (b"0.0", false),
            (b"FALSE", false),
            (b"x", true),
        ] {
            assert_eq!(script_flag(v), want);
        }
    }
}

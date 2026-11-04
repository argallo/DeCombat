//! The door "use" prompt: the 2009 client's target code finds the object under the cross, and when that object has an
//! activate entry (a door's type 0x39 client object) within the activation distance, the text id of the entry's
//! activate-type row and state becomes the target text. The HUD element draws one layout image while the target text
//! is the door's "open" or "close" text.
//!
//! - The entry: the door's create blob `[0x39][row:u8][disabled:bool1][state:u8]`; object notes (shell 0xa6 type
//!   0x39) change the disabled flag (sub 1) and the state (sub 2).
//! - The target: the 2009 client takes the first thing the aim ray from the eye meets (a door within 110 units of the
//!   eye). Ours takes the door the use key turns: our server's use key ignores the view and turns the nearest
//!   usable door within its reach (DSV-270), so the prompt shows exactly while the key would turn one (CLI-1795;
//!   Rattlesnake's garage levers are thin handles half inside the wall that the aim ray almost never met).
//! - The text: row field [state] of the activate-type table ([`dc_ui_data::activate`]), none for -1.
//! - The image: texture, place and size from the HUD layout set, drawn while the text id is 0x1fd or 0x208.
// DISASM: cshell+0x2e2230 (the blob read into the entry and the entry's registration), cshell+0x2e1a70 (the object
// notes: sub 1 disabled, sub 2 state), cshell+0x2b2da0 (the target update: ray of 10000 units from the camera,
// cshell+0x2b27c0; distance against the client value read at cshell+0x2b1fd0 with the default 110.0; the entry whose
// object is the target, cshell+0x2b30c2..0x2b3193; the text id = row field [state], cshell+0x2b1d00), cshell+0x82860
// (the image while the target text id is 0x1fd or 0x208, cshell+0x82b34..0x82bd0; layout getters cshell+0x57910 /
// 0x579d0 / 0x57a40), cshell+0x85440..0x85513 (the element runs from the HUD draw)

use std::collections::BTreeMap;

use dc_botclient::objects::ObjectTable;
use dc_ui_data::activate::ActivateTypes;
use dc_ui_data::{HudKey as K, HudView};
use dc_wire::udp::BitReader;

use super::items::DrawItem;
use super::layout::Img;
use super::state::HudState;

/// Shell object type of a door and the sub-ids of its object notes.
// DISASM: cshell+0x222480 (factory case 0x39), cshell+0x2e1a70 (sub 1, sub 2)
pub const OBJ_DOOR: u8 = 0x39;
pub const NOTE_DISABLED: u8 = 1;
pub const NOTE_STATE: u8 = 2;
/// The target ray's length and the activation distance (the client value's default).
// DISASM: cshell+0x2b2e2d (10000.0 passed to the ray), cshell+0x2b1fe0 (the value's default 110.0)
pub const TARGET_RAY: f32 = 10_000.0;
pub const ACTIVATION_DISTANCE: f32 = 110.0;
/// The two text ids the element draws its image for (the door's open / close texts).
// DISASM: cshell+0x82b34..0x82b52 (`cmp ax, 0x1fd`, `cmp ax, 0x208`)
pub const DOOR_TEXTS: [u32; 2] = [0x1fd, 0x208];
/// The target text of an item (a client object of type 0x17 under the cross: pickups, dropped weapons): the element
/// draws the item image for it.
// DISASM: cshell+0x2b31a0..0x2b31cb (an activatable target within the activation distance that is no door: type 0x17
// -> 0x1c6, else 0x1c5), cshell+0x82a5e (`cmp ax, 0x1c6` -> the item image, layout getters cshell+0x56d40 / 0x56e00 /
// 0x56e70)
pub const ITEM_TEXT: u32 = 0x1c6;

/// A door's activate entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UseEntry {
    pub row: u8,
    pub disabled: bool,
    pub state: u8,
}

/// The entry of a door create blob (type byte first).
pub fn read_door_blob(blob: &dc_wire::udp::BitWriter) -> Option<UseEntry> {
    let mut r = blob.reader();
    if r.remaining_bits() < 25 || r.read_u8() != OBJ_DOOR {
        return None;
    }
    Some(UseEntry {
        row: r.read_u8(),
        disabled: r.read_bit(),
        state: r.read_u8(),
    })
}

/// A door's object note `[0x39][obj:u16][sub:u8]` + `[disabled:bool1]` (sub 1) or `[state:u8]` (sub 2): `(obj, sub,
/// value)`.
pub fn read_door_note(body: &[u8], bits: usize) -> Option<(u16, u8, u8)> {
    let mut r = BitReader::with_bit_len(body, bits);
    if r.remaining_bits() < 33 || r.read_u8() != OBJ_DOOR {
        return None;
    }
    let obj = r.read_u16();
    match r.read_u8() {
        NOTE_DISABLED => Some((obj, NOTE_DISABLED, u8::from(r.read_bit()))),
        NOTE_STATE if r.remaining_bits() >= 8 => Some((obj, NOTE_STATE, r.read_u8())),
        _ => None,
    }
}

/// The doors' entries of the session: from each door's create blob, changed by its notes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UseEntries {
    pub doors: BTreeMap<u16, UseEntry>,
}

impl UseEntries {
    /// Takes the doors of the object table: new ones from their blob, gone ones dropped.
    pub fn sync(&mut self, t: &ObjectTable) {
        self.doors.retain(|obj, _| {
            t.objects
                .get(obj)
                .is_some_and(|o| o.blob_type == Some(OBJ_DOOR))
        });
        for (&obj, o) in &t.objects {
            if o.blob_type != Some(OBJ_DOOR) || self.doors.contains_key(&obj) {
                continue;
            }
            if let Some(e) = o.blob.as_ref().and_then(read_door_blob) {
                self.doors.insert(obj, e);
            }
        }
    }

    /// An object note to a door.
    pub fn on_note(&mut self, obj: u16, sub: u8, value: u8) {
        if let Some(e) = self.doors.get_mut(&obj) {
            match sub {
                NOTE_DISABLED => e.disabled = value != 0,
                NOTE_STATE => e.state = value,
                _ => {}
            }
        }
    }

    /// The entry of the door the use key turns now: the nearest enabled entry whose world model's file box (by the
    /// door's brush name, `file_box`) lies within [`USE_REACH`] of the own body position `at`; `None` when the key
    /// would turn nothing.
    // DC-DEFAULT: CLI-1795 (proposed) the prompt follows our server's use key (DSV-270: the nearest unlocked use door
    // within 128 units of its box, whatever the view), not the 2009 aim ray
    pub fn reach_target(
        &self,
        t: &ObjectTable,
        at: [f32; 3],
        file_box: impl Fn(&[u8]) -> Option<([f32; 3], [f32; 3])>,
    ) -> Option<UseEntry> {
        self.doors
            .iter()
            .filter(|(_, e)| !e.disabled)
            .filter_map(|(obj, e)| {
                let (lo, hi) = file_box(t.objects.get(obj)?.name.as_ref()?)?;
                let d = box_distance(lo, hi, at);
                (d <= USE_REACH).then_some((d, *e))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, e)| e)
    }
}

/// How near (units from the own body position to a door model's file box) the use key reaches a door: our server's
/// reach.
// DC-DEFAULT: CLI-1795 (proposed) = DSV-270's 128 units (dc_dserver::doors::DOOR_USE_REACH)
pub const USE_REACH: f32 = 128.0;

/// Distance from `p` to the box `lo..hi` (0 inside).
fn box_distance(lo: [f32; 3], hi: [f32; 3], p: [f32; 3]) -> f32 {
    (0..3)
        .map(|k| (lo[k] - p[k]).max(p[k] - hi[k]).max(0.0).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// The prompt's layout image and the activate-type table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PromptLayout {
    pub image: Option<Img>,
    /// The item image (the target text [`ITEM_TEXT`], CLI-1352).
    pub item: Option<Img>,
    pub types: ActivateTypes,
}

impl PromptLayout {
    pub fn load(v: &HudView<'_>) -> Self {
        PromptLayout {
            image: (|| {
                Some(Img {
                    tex: v.text(K::DoorPromptTex)?.to_string(),
                    pos: v.pair(K::DoorPromptPos)?,
                    size: v.pair(K::DoorPromptSize)?,
                })
            })(),
            item: (|| {
                Some(Img {
                    tex: v.text(K::GetItemPromptTex)?.to_string(),
                    pos: v.pair(K::GetItemPromptPos)?,
                    size: v.pair(K::GetItemPromptSize)?,
                })
            })(),
            types: ActivateTypes::default(),
        }
    }
}

/// The target text id of the frame's use target.
pub fn text_id(l: &PromptLayout, s: &HudState) -> Option<u32> {
    let e = s.use_target?;
    l.types.text_id(e.row, e.state)
}

/// Draws the image while the own player is alive and the target text is a door text: the whole texture shrinks or
/// grows into the layout size (`tex_size` gives the texture's own size; the layout size serves when it is unknown).
// DISASM: cshell+0xbda50 (the element's image is an image control of the layout size, cshell+0x3436c0 / 0x343860)
// DC-DEFAULT: CLI-1230 (proposed) drawn for an enabled entry only (the 2009 code loads the text of a disabled entry
// too; our server disables the doors players may not use), while the own player is alive and the target lasts (the
// 2009 control's own timing is not traced)
pub fn draw(
    out: &mut Vec<DrawItem>,
    l: &PromptLayout,
    s: &HudState,
    tex_size: super::items::TexSize<'_>,
) {
    if s.life_state != 1 {
        return;
    }
    // CLI-1352: a dropped weapon the use key would take is the item target: the item image (it wins over a door, as
    // the use key does)
    // DC-DEFAULT: CLI-1352 (proposed) the item target is the drop the use key takes (CLI-675: reach across the floor,
    // not the 2009 aim ray, since our pickup is by reach); the element's own exceptions (player state field 10, mode
    // classes 9 / 11 under conditions not read, cshell+0x82a64..0x82af3) are not applied
    if s.item_target {
        if let Some(i) = &l.item {
            out.push(whole_image(i, tex_size));
        }
        return;
    }
    if s.use_target.is_none_or(|e| e.disabled) {
        return;
    }
    let (Some(i), Some(id)) = (&l.image, text_id(l, s)) else {
        return;
    };
    if !DOOR_TEXTS.contains(&id) {
        return;
    }
    out.push(whole_image(i, tex_size));
}

/// The whole texture shrunk or grown into the layout size at the layout place.
fn whole_image(i: &Img, tex_size: super::items::TexSize<'_>) -> DrawItem {
    DrawItem::Image {
        tex: i.tex.clone(),
        src: tex_size(&i.tex).unwrap_or([i.size[0] as f32, i.size[1] as f32]),
        dst: [
            i.pos[0] as f32,
            i.pos[1] as f32,
            i.size[0] as f32,
            i.size[1] as f32,
        ],
        tint: [255; 4],
        turn: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> PromptLayout {
        PromptLayout {
            image: Some(Img {
                tex: "p.dtx".into(),
                pos: [400, 500],
                size: [200, 40],
            }),
            item: Some(Img {
                tex: "i.dtx".into(),
                pos: [256, 446],
                size: [512, 64],
            }),
            types: ActivateTypes {
                rows: [(0u8, [0x208, 0x1fd, 9000, 9001]), (1, [7, -1, -1, -1])]
                    .into_iter()
                    .collect(),
            },
        }
    }

    fn blob(row: u8, disabled: bool, state: u8) -> dc_wire::udp::BitWriter {
        let mut w = dc_wire::udp::BitWriter::new();
        w.write_u8(OBJ_DOOR)
            .write_u8(row)
            .write_bit(disabled)
            .write_u8(state);
        w
    }

    #[test]
    fn the_blob_and_the_notes_make_the_entry() {
        assert_eq!(
            read_door_blob(&blob(0, false, 1)),
            Some(UseEntry {
                row: 0,
                disabled: false,
                state: 1
            })
        );
        let mut t = ObjectTable::default();
        let mut o = dc_botclient::objects::ClientObject::default();
        o.blob_type = Some(OBJ_DOOR);
        o.blob = Some(blob(0, false, 1));
        o.name = Some(b"SlidingDoor2".to_vec());
        t.objects.insert(7, o);
        let mut e = UseEntries::default();
        e.sync(&t);
        let mut n = dc_wire::udp::BitWriter::new();
        n.write_u8(OBJ_DOOR)
            .write_u16(7)
            .write_u8(NOTE_STATE)
            .write_u8(0);
        let (obj, sub, v) = read_door_note(n.as_bytes(), n.bit_len()).unwrap();
        e.on_note(obj, sub, v);
        assert_eq!(e.doors[&7].state, 0);
        let mut n = dc_wire::udp::BitWriter::new();
        n.write_u8(OBJ_DOOR)
            .write_u16(7)
            .write_u8(NOTE_DISABLED)
            .write_bit(true);
        let (obj, sub, v) = read_door_note(n.as_bytes(), n.bit_len()).unwrap();
        e.on_note(obj, sub, v);
        assert!(e.doors[&7].disabled);
        // the target (CLI-1795): an enabled door whose model box lies within the use key's reach of the body
        let boxes = |n: &[u8]| {
            n.eq_ignore_ascii_case(b"slidingdoor2")
                .then_some(([0.0, 0.0, 0.0], [10.0, 100.0, 60.0]))
        };
        let reach = |e: &UseEntries, at| e.reach_target(&t, at, boxes);
        assert_eq!(reach(&e, [5.0, 50.0, 30.0]), None, "disabled");
        e.on_note(7, NOTE_DISABLED, 0);
        assert_eq!(
            reach(&e, [5.0, 50.0, 30.0]).map(|x| x.state),
            Some(0),
            "inside"
        );
        assert_eq!(
            reach(&e, [138.0, 50.0, 30.0]).map(|x| x.state),
            Some(0),
            "128 away"
        );
        assert_eq!(reach(&e, [139.0, 50.0, 30.0]), None, "too far");
        assert_eq!(
            e.reach_target(&t, [5.0, 50.0, 30.0], |_| None),
            None,
            "no model"
        );
        // the nearer of two enabled doors
        let mut o = t.objects[&7].clone();
        o.name = Some(b"Lever".to_vec());
        o.blob = Some(blob(0, false, 1));
        t.objects.insert(9, o);
        e.sync(&t);
        let two = |n: &[u8]| {
            boxes(n).or_else(|| {
                n.eq_ignore_ascii_case(b"lever")
                    .then_some(([100.0, 40.0, 0.0], [104.0, 44.0, 4.0]))
            })
        };
        assert_eq!(
            e.reach_target(&t, [90.0, 40.0, 0.0], two).map(|x| x.state),
            Some(1),
            "the lever"
        );
        assert_eq!(
            e.reach_target(&t, [20.0, 40.0, 0.0], two).map(|x| x.state),
            Some(0),
            "the door"
        );
        t.objects.remove(&9);
        t.objects.remove(&7);
        e.sync(&t);
        assert!(e.doors.is_empty());
    }

    #[test]
    fn the_image_shows_for_the_door_texts_of_an_enabled_entry() {
        let l = layout();
        let mut s = HudState {
            life_state: 1,
            ..Default::default()
        };
        let drawn = |s: &HudState| {
            let mut out = Vec::new();
            draw(&mut out, &l, s, &|_| Some([256.0, 64.0]));
            if let Some(DrawItem::Image { src, .. }) = out.first() {
                assert_eq!(*src, [256.0, 64.0], "the whole texture");
            }
            out.len()
        };
        assert_eq!(drawn(&s), 0, "no target");
        for state in [0, 1] {
            s.use_target = Some(UseEntry {
                row: 0,
                disabled: false,
                state,
            });
            assert_eq!(drawn(&s), 1, "state {state}");
        }
        s.use_target = Some(UseEntry {
            row: 0,
            disabled: false,
            state: 2,
        });
        assert_eq!(drawn(&s), 0, "a text the element has no image for");
        s.use_target = Some(UseEntry {
            row: 1,
            disabled: false,
            state: 0,
        });
        assert_eq!(drawn(&s), 0);
        s.use_target = Some(UseEntry {
            row: 0,
            disabled: true,
            state: 1,
        });
        assert_eq!(drawn(&s), 0, "disabled");
        s.use_target = Some(UseEntry::default());
        s.life_state = 2;
        assert_eq!(drawn(&s), 0, "dead");
    }

    #[test]
    fn a_drop_in_reach_draws_the_item_image_over_a_door() {
        // CLI-1352
        let l = layout();
        let mut s = HudState {
            life_state: 1,
            item_target: true,
            ..Default::default()
        };
        let texs = |s: &HudState| {
            let mut out = Vec::new();
            draw(&mut out, &l, s, &|_| None);
            out.iter()
                .filter_map(|i| match i {
                    DrawItem::Image { tex, dst, .. } => Some((tex.clone(), *dst)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(texs(&s), vec![("i.dtx".to_string(), [256.0, 446.0, 512.0, 64.0])]);
        s.use_target = Some(UseEntry {
            row: 0,
            disabled: false,
            state: 1,
        });
        assert_eq!(texs(&s).len(), 1);
        assert_eq!(texs(&s)[0].0, "i.dtx");
        s.item_target = false;
        assert_eq!(texs(&s)[0].0, "p.dtx");
        s.item_target = true;
        s.life_state = 2;
        assert!(texs(&s).is_empty(), "dead");
    }
}

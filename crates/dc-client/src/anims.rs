//! How remote characters animate: the client plays the animation trackers the server sends for each model object
//! and draws every character-like model object the server creates, including the corpse a death leaves behind.
//!
//! The 2009 split (UDP_SESSION_2009_B.md §4.3, §4.4, §3.5.5): the server chooses the clips. Every stream entry of a
//! character carries three trackers (main; tracker 1 with weight set 0, the upper body; tracker 2 with weight set
//! 1, the lower body); the clip of the lower body follows the movement (stand, walk, run, crouch in eight
//! directions), the clip of the upper body the held weapon, firing and the hit reaction; a dead character is hidden
//! (flags 0x0802) and a separate corpse object (shell blob type 0x26) is created at its position with one tracker
//! on a death clip. The client's part, implemented here:
//!
//! - **Tracker time.** A tracker's time runs on locally from the time its block carried. A looping tracker
//!   (state bit 1 set) wraps at the clip length; a tracker that plays once (state bit 1 clear: the death clip, the
//!   hit reaction and the other one-shot upper-body clips) stops on its last frame and holds it.
//! - **Corpses.** A kind-1 object created with a type-0x26 blob is drawn with its own model block and tracker,
//!   placed at its position and rotation with the box of its record, like a character, until its removal record.
//!
//! What the 2009 client itself selects per movement state, weapon class and hit is nothing: those choices arrive in
//! the trackers (see the report `reports/client-anims.md` for the evidence and for the server
//! side of the rule).
// SPEC: UDP_SESSION_2009_B.md §3.5.5 (death: character hidden, corpse create `cf` 0x25bb, blob type 0x26, one
// tracker on a death clip with state1 0, removed about 18.8 s later; REFERENCE, accepted by the real client), §4.3
// (three trackers per stream entry; "a dead character in the stream": the death clip held at its end), §4.4
// (animation block; state bits 0 / 1 copied into the tracker flag byte by engine+0xa2300)
// EVIDENCE: engine+0xa2530 (animation block reader), engine+0xa2300 (tracker apply)

use std::collections::BTreeMap;

use dc_botclient::objects::ClientObject;
use dc_botclient::session::BotSession;

use crate::world::{ModelRef, RemoteBody, RemoteLook, RemoteTracker};

/// Shell blob type of the corpse client object (the type byte of its create blob).
// SPEC: UDP_SESSION_2009_B.md §3.5.5 (corpse create, blob type 0x26); EVIDENCE: cshell+0x2b6470 (the type-0x26
// record reader: seven u8, three bits, one u8, two vectors)
pub const BLOB_CORPSE: u8 = 0x26;

/// Object kind of a model object (characters, corpses, weapons).
// SPEC: UDP_SESSION_2009_B.md §3.5 (kind 1)
pub const KIND_MODEL: u8 = 1;

/// Most time a looping tracker runs on past its last block (the stream refreshes it 20-30 times a second).
// DC-DEFAULT: CLI-37 (proposed, unchanged) 1000 ms
pub const LOOP_EXTRAPOLATE_MS: f32 = crate::net::TRACKER_EXTRAPOLATE_MS as f32;

/// The time (ms) a tracker shows `age_ms` after a block set it to `entry_ms`, for a clip of `len_ms`.
///
/// Looping: the time runs on for at most [`LOOP_EXTRAPOLATE_MS`] (the pose wraps it at the clip length). Playing
/// once: the time runs on to the clip's end and stays there (no cap: a corpse's tracker is set once, at its
/// create, and plays its whole clip locally).
// SPEC: UDP_SESSION_2009_B.md §4.3 (the reference stack's corpse clip held at 1336 = the clip end from seq 3363, not wrapped),
// §4.4 (state1); DC-DEFAULT: CLI-180 (proposed) state bit 1 = loops; clear = play once and hold the last frame
pub fn tracker_time(entry_ms: f32, age_ms: f32, len_ms: Option<u32>, looping: bool) -> f32 {
    let age = age_ms.max(0.0);
    if looping {
        return entry_ms + age.min(LOOP_EXTRAPOLATE_MS);
    }
    let t = entry_ms + age;
    match len_ms {
        Some(len) => t.min(len as f32),
        None => t,
    }
}

/// How far before its clip's end a held tracker is posed (ms): the pose wraps a time at the clip length to the
/// clip's start, so the held time stays just inside the last key segment (the shown pose is the last key's).
// DC-DEFAULT: CLI-180 (proposed) part of the hold rule; 0.5 ms is below the 1 ms key time resolution (FMT_LTB.md §10.2)
pub const HOLD_BEFORE_END_MS: f32 = 0.5;

/// The trackers of a pose with the play-once rule applied: each tracker whose `once` flag is set gets its time
/// clamped to its clip's last frame (`len` gives the length of a combined clip index).
// DC-DEFAULT: CLI-180 (proposed)
pub fn hold_once(
    trackers: &mut [RemoteTracker],
    once: &[bool],
    len: impl Fn(usize) -> Option<u32>,
) {
    for (t, &o) in trackers.iter_mut().zip(once) {
        if o {
            if let Some(l) = len(t.clip) {
                let end = (l as f32 - HOLD_BEFORE_END_MS).max(0.0);
                t.time_ms = t.time_ms.clamp(0.0, end);
            }
        }
    }
}

/// True for the corpse a death leaves: a model object created with a type-0x26 blob.
// SPEC: UDP_SESSION_2009_B.md §3.5.5
pub fn is_corpse(o: &ClientObject) -> bool {
    o.kind == KIND_MODEL && o.blob_type == Some(BLOB_CORPSE)
}

/// When the animation block now held by an object arrived (ms of the client's clock), for objects the roster
/// tracks do not cover (corpses).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimClocks {
    /// Object -> (animation blocks counted when last looked at, the frame time that count changed).
    seen: BTreeMap<u16, (u32, u32)>,
}

impl AnimClocks {
    /// Notes object `obj` with `updates` animation blocks so far at `now`; returns the ms since its latest block.
    // DC-DEFAULT: CLI-181 (proposed) a corpse's block counts as arrived at the first frame that sees it (the frame
    // loop looks at the session once per frame; the error is one frame)
    pub fn age(&mut self, obj: u16, updates: u32, now: u32) -> u32 {
        let e = self.seen.entry(obj).or_insert((updates, now));
        if e.0 != updates {
            *e = (updates, now);
        }
        now.saturating_sub(e.1)
    }

    /// Forgets every object not in `live`.
    pub fn retain(&mut self, live: &[u16]) {
        self.seen.retain(|k, _| live.contains(k));
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// What the frame loop needs besides the roster characters: the corpses to draw and, per drawn object, which of
/// its trackers play once (in tracker order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimFrame {
    /// Corpse objects drawn like characters (`pid` 0xFF, no name, no weapon).
    pub corpses: Vec<RemoteBody>,
    /// Object -> per tracker: plays once (state bit 1 clear).
    pub once: BTreeMap<u16, Vec<bool>>,
}

impl AnimFrame {
    /// The play-once flags of an object's trackers (empty: all loop).
    pub fn once_of(&self, obj: u16) -> &[bool] {
        self.once.get(&obj).map_or(&[], |v| v.as_slice())
    }
}

/// Model refs of a model block with names from the session file table (entries whose model file is unknown are
/// left out, as for a character).
// SPEC: UDP_SESSION_2009_B.md §3.4 (model block), part A §4.4 (file table names)
fn model_refs(
    models: &[dc_botclient::objects::ModelFileEntry],
    name: &impl Fn(u16) -> Option<String>,
) -> Vec<ModelRef> {
    models
        .iter()
        .filter_map(|e| {
            Some(ModelRef {
                model: name(e.model)?,
                textures: e
                    .textures
                    .iter()
                    .map(|t| name(*t).unwrap_or_default())
                    .collect(),
            })
        })
        .collect()
}

/// The drawn form of a corpse object: its position (the victim's box centre at the death), the yaw of its
/// rotation, the box of its record (`half_default` when it has none), its model block and its trackers advanced by
/// `age_ms` with the play-once rule (the clip length is applied when the model is posed).
// SPEC: UDP_SESSION_2009_B.md §3.5.5 (corpse create: model block = the victim's body, child model, five textures,
// three styles; one tracker; position = the victim's last position; flags 0x1803 = shown)
pub fn corpse_body(
    obj: u16,
    o: &ClientObject,
    name: impl Fn(u16) -> Option<String>,
    age_ms: u32,
    half_default: [f32; 3],
) -> Option<RemoteBody> {
    let pos = o.pos?;
    if !pos.iter().all(|c| c.is_finite()) {
        return None;
    }
    let trackers = o
        .anim
        .iter()
        .map(|a| RemoteTracker {
            clip: a.anim as usize,
            time_ms: tracker_time(a.time_ms as f32, age_ms as f32, None, a.state1),
            blend_set: a.blend_set.map(usize::from),
        })
        .collect();
    // the hidden form (bit 0x0002 set, 0x0001 clear) is not drawn, as for a character (CLI-42)
    let visible = !(o.flags & 0x0002 != 0 && o.flags & 0x0001 == 0);
    Some(RemoteBody {
        obj,
        pid: 0xFF,
        name: String::new(),
        team: 0,
        pos,
        half: o.dims.unwrap_or(half_default),
        yaw: Some(o.rot.map_or(0.0, dc_wire::udp::quat_yaw)),
        look: Some(RemoteLook {
            styles: Vec::new(),
            models: model_refs(&o.models, &name),
            weapon: None,
            trackers,
            visible,
            aim: None,
        }),
    })
}

/// The frame's corpses and play-once flags from the session: every corpse object in the table, and the flags of
/// the roster characters `characters` and of the corpses.
// DC-DEFAULT: CLI-183 (proposed) besides the roster characters only the corpse objects are drawn with models here
// (pickups belong to the drops path); EVIDENCE: cshell+0x222480 (shell factory, case 0x26), cshell+0x2da880 (the
// corpse client object: no tracker set, no clip lookup; it answers the death clip's keys)
pub fn frame(
    s: &BotSession,
    clocks: &mut AnimClocks,
    characters: &[u16],
    now: u32,
    half_default: [f32; 3],
) -> AnimFrame {
    let name = |id: u16| -> Option<String> {
        (id != 0xFFFF)
            .then(|| s.obs.files.get(&id).map(|f| f.1.clone()))
            .flatten()
    };
    let mut out = AnimFrame::default();
    let mut live = Vec::new();
    for (&obj, o) in &s.table.objects {
        if is_corpse(o) {
            live.push(obj);
            let age = clocks.age(obj, o.anim_updates, now);
            if let Some(b) = corpse_body(obj, o, name, age, half_default) {
                out.corpses.push(b);
                out.once
                    .insert(obj, o.anim.iter().map(|a| !a.state1).collect());
            }
        } else if characters.contains(&obj) {
            let once: Vec<bool> = o.anim.iter().map(|a| !a.state1).collect();
            if once.iter().any(|&x| x) {
                out.once.insert(obj, once);
            }
        }
    }
    clocks.retain(&live);
    out
}

/// One tracker as it is drawn this frame: the tracker with its shown time, and while the engine blends into a clip
/// that just replaced another one on this tracker, the previous clip frozen at the switch and the blend fraction
/// (0 = all previous, 1 = all current).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    pub tracker: RemoteTracker,
    pub from: Option<RemoteTracker>,
    pub fraction: f32,
}

/// The switch state of one tracker of one object.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    clip: usize,
    /// The time shown last frame (the previous clip's frozen time at a switch).
    shown: f32,
    blend_set: Option<usize>,
    /// The previous clip while a blend runs.
    from: Option<RemoteTracker>,
    /// The blend length of the current clip if it was blended into (the clip then lags the block's time by it).
    lag: f32,
}

/// The engine's clip switch per tracker: when a block changes the clip of a tracker that showed a valid pose, the
/// pose blends from the previous clip (frozen where it was) into the new clip over the new clip's blend time
/// (FMT_LTB.md clip `blend_ms`); during the blend the new clip's time does not run, afterwards it runs on from its
/// start, so it stays the blend time behind the time the block carried. A clip with blend time 0 and a tracker seen
/// for the first time switch at once. The same clip in a later block changes nothing.
// EVIDENCE: engine+0xa2530 (the stream reader passes blend = 1), engine+0xa2300 (a tracker is reset only when its
// clip changed or it is not playing; the rate argument -1 keeps the rate: no rate on the wire), engine+0x2ca0 (set
// clip: keeps the previous clip, key and time; blend counter 0 when the previous pose is valid, else 0xffff = no
// blend), engine+0x2e10 (time advance: the counter runs first, fraction = counter / the clip's blend time, the
// clip time stands still until the counter ends, the remainder then advances the clip), engine+0x3040 (set time
// goes through the same advance)
// DC-DEFAULT: CLI-182 (proposed) the blend runs on the block's tracker time (this client re-reads the time of every
// block, the engine plays a kept clip on locally); the fraction mixes whole poses (previous-clip pose and current
// pose per node: linear positions, spherical rotations)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipBlends {
    slots: BTreeMap<u16, Vec<Slot>>,
}

impl ClipBlends {
    /// The layers of object `obj` for this frame's trackers; `blend_ms` and `len` give a combined clip's blend time
    /// and length (`None`: unknown).
    pub fn step(
        &mut self,
        obj: u16,
        trackers: &[RemoteTracker],
        blend_ms: impl Fn(usize) -> Option<u32>,
        len: impl Fn(usize) -> Option<u32>,
    ) -> Vec<Layer> {
        let slots = self.slots.entry(obj).or_default();
        if slots.len() != trackers.len() {
            // a different tracker set (a new object, a respawn): no previous pose to blend from
            slots.clear();
        }
        let mut out = Vec::with_capacity(trackers.len());
        for (i, t) in trackers.iter().enumerate() {
            if slots.len() <= i {
                slots.push(Slot {
                    clip: t.clip,
                    shown: t.time_ms,
                    blend_set: t.blend_set,
                    from: None,
                    lag: 0.0,
                });
            }
            let s = &mut slots[i];
            if s.clip != t.clip || s.blend_set != t.blend_set {
                let b = blend_ms(t.clip).unwrap_or(0) as f32;
                s.from = (b > 0.0).then_some(RemoteTracker {
                    clip: s.clip,
                    time_ms: s.shown,
                    blend_set: s.blend_set,
                });
                s.lag = b.max(0.0);
                s.clip = t.clip;
                s.blend_set = t.blend_set;
            }
            if let Some(from) = s.from {
                if t.time_ms < s.lag {
                    s.shown = 0.0;
                    out.push(Layer {
                        tracker: RemoteTracker { time_ms: 0.0, ..*t },
                        from: Some(from),
                        fraction: (t.time_ms / s.lag).clamp(0.0, 1.0),
                    });
                    continue;
                }
                s.from = None;
            }
            let mut shown = t.time_ms - s.lag;
            if shown < 0.0 {
                // a looping clip that wrapped since: the lag goes round the clip
                shown = (shown + len(t.clip).map_or(0.0, |l| l as f32)).max(0.0);
            }
            s.shown = shown;
            out.push(Layer {
                tracker: RemoteTracker {
                    time_ms: shown,
                    ..*t
                },
                from: None,
                fraction: 1.0,
            });
        }
        out
    }

    /// Forgets every object not in `live`.
    pub fn retain(&mut self, live: &[u16]) {
        self.slots.retain(|k, _| live.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_botclient::objects::{AnimTracker, ModelFileEntry};

    fn tracker(anim: u16, time_ms: u32, state1: bool) -> AnimTracker {
        AnimTracker {
            anim,
            state0: true,
            state1,
            time_ms,
            tracker: None,
            blend_set: None,
        }
    }

    #[test]
    fn looping_trackers_run_on_for_a_second_at_most() {
        assert_eq!(tracker_time(100.0, 40.0, Some(600), true), 140.0);
        // the wrap happens in the pose; the extrapolation stops at the cap
        assert_eq!(tracker_time(100.0, 5000.0, Some(600), true), 1100.0);
        assert_eq!(tracker_time(100.0, -5.0, Some(600), true), 100.0);
    }

    #[test]
    fn play_once_trackers_hold_their_last_frame() {
        // a death clip of 1333 ms created at 0: runs the whole clip locally, then holds
        assert_eq!(tracker_time(0.0, 700.0, Some(1333), false), 700.0);
        assert_eq!(tracker_time(0.0, 1500.0, Some(1333), false), 1333.0);
        assert_eq!(tracker_time(0.0, 60_000.0, Some(1333), false), 1333.0);
        // the server holding the time at the end: the client adds nothing past it
        assert_eq!(tracker_time(1336.0, 30.0, Some(1336), false), 1336.0);
        // no length known yet: runs on (the pose clamps once the model is loaded)
        assert_eq!(tracker_time(0.0, 2000.0, None, false), 2000.0);
    }

    #[test]
    fn hold_once_clamps_only_the_flagged_trackers() {
        let mut t = vec![
            RemoteTracker {
                clip: 88,
                time_ms: 4000.0,
                blend_set: None,
            },
            RemoteTracker {
                clip: 37,
                time_ms: 4000.0,
                blend_set: Some(1),
            },
        ];
        hold_once(&mut t, &[true, false], |c| {
            Some(if c == 88 { 1333 } else { 600 })
        });
        assert_eq!(t[0].time_ms, 1333.0 - HOLD_BEFORE_END_MS);
        assert_eq!(
            t[1].time_ms, 4000.0,
            "a looping tracker wraps in the pose instead"
        );
        // fewer flags than trackers: the rest loop
        let mut u = t.clone();
        hold_once(&mut u, &[], |_| Some(1));
        assert_eq!(u, t);
    }

    fn corpse_object() -> ClientObject {
        let mut o = ClientObject::default();
        o.kind = KIND_MODEL;
        o.blob_type = Some(BLOB_CORPSE);
        o.pos = Some([10.0, 50.0, -20.0]);
        o.rot = Some([0.0, (0.5f32).sin(), 0.0, (0.5f32).cos()]);
        o.flags = 0x1803;
        o.dims = Some([15.0, 50.0, 15.0]);
        o.models = vec![ModelFileEntry {
            model: 3,
            subs: vec![],
            textures: vec![4, 0xFFFF],
            styles: vec![],
        }];
        o.anim = vec![tracker(88, 0, false)];
        o.anim_updates = 1;
        o
    }

    /// world-fixes (CLI-585, the user's "when an enemy dies its head disappears"): a corpse whose model block carries
    /// the victim's face entry (the head) after the body is drawn with both, the face second (the scene's face slot,
    /// which follows the body's death pose).
    #[test]
    fn a_corpse_keeps_the_face_entry_of_its_model_block() {
        let mut o = corpse_object();
        o.models.push(ModelFileEntry {
            model: 5,
            subs: vec![],
            textures: vec![6],
            styles: vec![],
        });
        let name = |id: u16| match id {
            3 => Some("M/BODY.LTB".to_string()),
            4 => Some("T/SKIN.DTX".to_string()),
            5 => Some("M/HEAD.LTB".to_string()),
            6 => Some("T/FACE.DTX".to_string()),
            _ => None,
        };
        let look = corpse_body(9, &o, name, 0, [1.0; 3]).unwrap().look.unwrap();
        let models: Vec<&str> = look.models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(models, ["M/BODY.LTB", "M/HEAD.LTB"]);
    }

    #[test]
    fn a_corpse_is_a_model_object_with_the_type_0x26_blob() {
        let o = corpse_object();
        assert!(is_corpse(&o));
        let mut c = o.clone();
        c.blob_type = Some(0x18);
        assert!(!is_corpse(&c), "a character record");
        let mut k = o.clone();
        k.kind = 0;
        assert!(!is_corpse(&k));
    }

    #[test]
    fn a_corpse_is_drawn_like_a_character_with_its_own_clip() {
        let o = corpse_object();
        let name = |id: u16| match id {
            3 => Some("M\\BODY.LTB".to_string()),
            4 => Some("T\\SKIN.DTX".to_string()),
            _ => None,
        };
        let b = corpse_body(9, &o, name, 500, [1.0; 3]).expect("drawn");
        assert_eq!(b.obj, 9);
        assert_eq!(b.pos, [10.0, 50.0, -20.0]);
        assert_eq!(b.half, [15.0, 50.0, 15.0]);
        assert!((b.yaw.unwrap() - 1.0).abs() < 1e-5, "yaw of the rotation");
        let look = b.look.unwrap();
        assert!(look.visible);
        assert_eq!(look.weapon, None);
        assert_eq!(look.models.len(), 1);
        assert_eq!(look.models[0].model, "M\\BODY.LTB");
        assert_eq!(
            look.models[0].textures,
            vec!["T\\SKIN.DTX".to_string(), String::new()]
        );
        assert_eq!(look.trackers.len(), 1);
        assert_eq!(look.trackers[0].clip, 88);
        assert_eq!(
            look.trackers[0].time_ms, 500.0,
            "played locally from the create"
        );
        assert_eq!(look.trackers[0].blend_set, None);
        // no box in the record: the default; no position: not drawn
        let mut nd = o.clone();
        nd.dims = None;
        assert_eq!(
            corpse_body(9, &nd, name, 0, [1.0; 3]).unwrap().half,
            [1.0; 3]
        );
        let mut np = o;
        np.pos = None;
        assert!(corpse_body(9, &np, name, 0, [1.0; 3]).is_none());
    }

    #[test]
    fn the_clock_counts_from_the_frame_that_saw_the_block() {
        let mut c = AnimClocks::default();
        assert_eq!(c.age(5, 1, 1000), 0);
        assert_eq!(c.age(5, 1, 1400), 400);
        // a new block restarts the age
        assert_eq!(c.age(5, 2, 1500), 0);
        assert_eq!(c.age(5, 2, 1533), 33);
        c.retain(&[]);
        assert!(c.is_empty());
    }

    fn rt(clip: usize, time_ms: f32, blend_set: Option<usize>) -> RemoteTracker {
        RemoteTracker {
            clip,
            time_ms,
            blend_set,
        }
    }

    #[test]
    fn a_clip_change_blends_over_the_new_clips_blend_time() {
        let mut b = ClipBlends::default();
        let blend = |c: usize| Some(if c == 98 { 100 } else { 0 });
        let len = |_c: usize| Some(1000);
        // first sight: no blend
        let l = b.step(7, &[rt(0, 500.0, None), rt(99, 300.0, Some(0))], blend, len);
        assert_eq!(l[1].from, None);
        assert_eq!(l[1].tracker.time_ms, 300.0);
        // the same clip later: plays on, no blend
        let l = b.step(7, &[rt(0, 533.0, None), rt(99, 333.0, Some(0))], blend, len);
        assert_eq!(
            (l[1].from, l[1].fraction, l[1].tracker.time_ms),
            (None, 1.0, 333.0)
        );
        // the upper body switches to a clip with a 100 ms blend: from the frozen previous clip, the new one at 0
        let l = b.step(7, &[rt(0, 566.0, None), rt(98, 40.0, Some(0))], blend, len);
        assert_eq!(l[1].from, Some(rt(99, 333.0, Some(0))));
        assert!((l[1].fraction - 0.4).abs() < 1e-6);
        assert_eq!(
            l[1].tracker.time_ms, 0.0,
            "the clip time stands during the blend"
        );
        assert_eq!(l[0].from, None, "the main tracker is untouched");
        // after the blend the clip runs, the blend time behind the block's time
        let l = b.step(7, &[rt(0, 600.0, None), rt(98, 130.0, Some(0))], blend, len);
        assert_eq!((l[1].from, l[1].fraction), (None, 1.0));
        assert_eq!(l[1].tracker.time_ms, 30.0);
        // the looping clip wrapped on the server: the lag goes round
        let l = b.step(7, &[rt(0, 633.0, None), rt(98, 20.0, Some(0))], blend, len);
        assert_eq!(l[1].tracker.time_ms, 920.0);
        // a clip without blend time switches at once
        let l = b.step(7, &[rt(0, 666.0, None), rt(110, 10.0, Some(0))], blend, len);
        assert_eq!((l[1].from, l[1].tracker.time_ms), (None, 10.0));
        // a different tracker set: a fresh start
        let l = b.step(7, &[rt(88, 0.0, None)], blend, len);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].from, None);
        b.retain(&[]);
        assert_eq!(b, ClipBlends::default());
    }
}

//! Client-side readers of the engine object messages: `S2C_OBJ_GROUP` (id 8, reliable records) and
//! `S2C_OBJ_STREAM` (id 10, unreliable compact entries + the time entry), applied to a client object table.
//!
//! A 2009 client ends the session with error 0x2c on a malformed record (UDP_SESSION_2009_B.md §2.3, addendum
//! B4 / B5). The bot reports such a record as a protocol error, seeks to the record's announced end and goes
//! on, so one bad record does not hide the rest of a capture.
// SPEC: UDP_SESSION_2009_B.md §3, §4, §5, §9.1; UDP_SESSION_2009_B_addendum_disasm.md B3-B5

use std::collections::BTreeMap;

use dc_wire::udp::{read_quat128, BitReader, BitWriter, Pos48, Vec72};

/// Change-mask bits of an object record.
// SPEC: UDP_SESSION_2009_B.md §4.1
pub mod cf {
    pub const NEW: u16 = 0x0001;
    pub const POS: u16 = 0x0202;
    pub const ROT: u16 = 0x0404;
    pub const FLAGS: u16 = 0x0008;
    pub const SCALE: u16 = 0x0010;
    pub const ANIM: u16 = 0x2020;
    pub const COLOR: u16 = 0x0040;
    pub const ATTACH: u16 = 0x0100;
    pub const POS_SNAP: u16 = 0x0200;
    pub const FILES: u16 = 0x0800;
    pub const DIMS: u16 = 0x8000;
}

/// Object kinds with kind-specific fields.
// SPEC: UDP_SESSION_2009_B.md §3.3 (obj_kind table)
pub mod kind {
    pub const PLAIN: u8 = 0;
    pub const MODEL: u8 = 1;
    pub const BRUSH: u8 = 2;
    pub const BILLBOARD: u8 = 3;
    pub const LAMP: u8 = 4;
    pub const ZONE: u8 = 9;
}

/// Object flag bit 0x0100: positions of change records travel as raw floats.
// SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (0x0100 = full precision)
pub const FLAG_FULL_PRECISION: u16 = 0x0100;
/// Session error code of a malformed object message.
// SPEC: UDP_SESSION_2009_A.md §6.1 (0x2C), UDP_SESSION_2009_B.md §2.3
pub const ERR_BAD_RECORD: u32 = 0x2c;
/// Session error code of a model block with no model.
// SPEC: UDP_SESSION_2009_B.md §3.4 (model_count 0 -> code 1)
pub const ERR_BAD_MODEL_BLOCK: u32 = 1;

/// The world box `pos48` positions are fixed point over (the world file's extents).
// SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T14 (box = world extents, precise = 1, e = 0)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtentBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl ExtentBox {
    /// Decodes a `pos48`: `min + (max - min) * (8 * p + 0.5) * 2^-19` (y: `4 * p`, `2^-18`).
    // SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T14 (decode engine+0x21910 with precise = 1)
    pub fn decode(&self, p: Pos48) -> [f32; 3] {
        p.decode(self.min, self.max, true, 0)
    }
}

/// `vec72` decode: `o = q * major * 2^-18`, negated by the sign bits; axis 0 / 3 -> (major, o1, o2).
// SPEC: UDP_TRANSPORT_2009_addendum_disasm.md T15 (decoder engine+0x216a0)
pub fn decode_vec72(v: Vec72) -> [f32; 3] {
    let s = 1.0f32 / 262_144.0;
    let mut o1 = v.q1() as f32 * v.major * s;
    let mut o2 = v.q2() as f32 * v.major * s;
    if v.vflags & 0x20 != 0 {
        o1 = -o1;
    }
    if v.vflags & 0x04 != 0 {
        o2 = -o2;
    }
    match v.major_axis() {
        1 => [o1, v.major, o2],
        2 => [o1, o2, v.major],
        _ => [v.major, o1, o2],
    }
}

/// What the client knows about one object.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClientObject {
    pub kind: u8,
    pub create_pos: [f32; 3],
    /// Latest position (world units), when it could be decoded.
    pub pos: Option<[f32; 3]>,
    /// Latest compact position as received (kept when no world box is known).
    pub pos48: Option<Pos48>,
    pub vel: Option<[f32; 3]>,
    pub rot: Option<[f32; 4]>,
    pub yaw8: Option<i8>,
    pub flags: u16,
    pub dims: Option<[f32; 3]>,
    pub records: u32,
    pub stream_entries: u32,
    /// The server time (seconds) of the stream message that carried the newest stream entry of this object (the
    /// time entry comes last in the message, B §9.1); `None` until a message with a time entry carried one.
    pub stream_time: Option<f32>,
    /// Records (create or change) that placed the object with `CF_POS_SNAP` (a decoded position, no smoothing:
    /// spawn, respawn, the visibility park / show).
    // SPEC: UDP_SESSION_2009_B.md §3.3 (the reader adds CF_POS_SNAP on create), §4.1 row 0x0200 (PLAUSIBLY "place
    // without smoothing")
    pub pos_snaps: u32,
    /// Bits of the create blob handed to the shell (0 = none).
    pub blob_bits: u32,
    /// Distance covered between consecutive stream positions (world units; record positions and the jump from
    /// the create position are not counted).
    pub travel: f32,
    last_stream_pos: Option<[f32; 3]>,
    /// Bounding box of every decoded position.
    pub pos_min: Option<[f32; 3]>,
    pub pos_max: Option<[f32; 3]>,
    /// The first decoded positions, with the stream / record they came from (for reports).
    pub samples: Vec<([f32; 3], bool)>,
    /// Model file block of a kind-1 object (create or the latest `CF_FILES`): one entry per model, file ids of the
    /// session file table.
    pub models: Vec<ModelFileEntry>,
    /// The latest animation block (create, `CF_ANIM` or a stream entry), main tracker first.
    pub anim: Vec<AnimTracker>,
    /// Animation blocks received so far (records and stream entries).
    pub anim_updates: u32,
    /// The latest attachment list (`CF_ATTACH`).
    pub attachments: Vec<Attachment>,
    /// First byte of the create blob (the shell object type, for example 0x18 for a character record).
    pub blob_type: Option<u8>,
    /// The whole create blob (type byte first) when it is at most [`BLOB_KEEP_BITS`] long (the client HUD reads the
    /// HiSec case key from a type 0x17 blob).
    // SPEC: GAME_MESSAGES_2009_S2C.md §8.2 (type 0x17 kind 5: `[case_key:u32]`)
    pub blob: Option<BitWriter>,
    /// The name of a named brush or volume (kinds 2 and 9) as its create carried it.
    // SPEC: UDP_SESSION_2009_B.md §3.3 (kind 2 `[name:zstr8]`, kind 9 `[name:zstr8][code:u16]`)
    pub name: Option<Vec<u8>>,
    /// The table's create count when this object was created (a generation: an id removed and created again in
    /// one batch of records gets a new value, so a reader that compares tables frame by frame sees the new object).
    pub created: u64,
    /// The container code of a named volume (kind 9): 7 kill volume, 11 ladder in the reference stack's records.
    // SPEC: UDP_SESSION_2009_B.md §3.3 (kind 9 `[name:zstr8][code:u16]`)
    pub code: Option<u16>,
    /// The four flag words of the latest flag block (`flags`, `flags2`, `uflags`, `flags3`).
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 3
    pub flag_words: Option<(u16, u16, u32, u32)>,
    /// The user flag word of the latest flag block (`CF_FLAGS`, third word; 0 before any flag block). The 2009 client
    /// reads it per object, for example the world camera's "live" bit 0x20.
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 3 (`[flags:u16][flags2:u16][uflags:u32][flags3:u32]`)
    // EVIDENCE: engine+0xa32f0 (the third word stored at the object's +0x8c); DISASM: cshell+0x1f8439..0x1f8451
    // (the camera test reads flag word 2 of the camera's object and tests 0x20)
    pub uflags: u32,
    /// The file id of a billboard (kind 3) from its create or the latest `CF_FILES` (session file table).
    // SPEC: UDP_SESSION_2009_B.md §3.3 (kind 3 `[file:u16]`), §4.2 item 1 (`CF_FILES` resends it for kind 3)
    pub sprite_file: Option<u16>,
    /// The latest scale block (`CF_SCALE`); a billboard's carries two components and the third reads 1.0.
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 5 (`[sx][sy]` then `[sz]` except for kind 3, where the reader uses 1.0)
    pub scale: Option<[f32; 3]>,
    /// The latest colour block (`CF_COLOR`): red, green, blue, alpha, extra.
    // SPEC: UDP_SESSION_2009_B.md §4.2 item 4 (`[r:u8][g:u8][b:u8][a:u8][extra:u8]`)
    pub color: Option<[u8; 5]>,
}

/// Create blobs up to this many bits are kept in [`ClientObject::blob`].
pub const BLOB_KEEP_BITS: u32 = 1024;

impl ClientObject {
    fn moved_to(&mut self, p: [f32; 3], stream: bool) {
        if self.samples.len() < 64 {
            self.samples.push((p, stream));
        }
        let lo = self.pos_min.get_or_insert(p);
        let hi_init = p;
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
        }
        let hi = self.pos_max.get_or_insert(hi_init);
        for i in 0..3 {
            hi[i] = hi[i].max(p[i]);
        }
        if stream {
            if let Some(q) = self.last_stream_pos {
                let d = (0..3)
                    .map(|i| (p[i] - q[i]) * (p[i] - q[i]))
                    .sum::<f32>()
                    .sqrt();
                if d.is_finite() {
                    self.travel += d;
                }
            }
            self.last_stream_pos = Some(p);
        }
        self.pos = Some(p);
    }
}

/// A gap between consecutive time entries (server clock) that counts as a server stall.
// DC-DEFAULT: test tooling (not a 2009 rule): 2.5 ticks of the default 10 Hz server
pub const SERVER_STALL_S: f32 = 0.25;
/// One sound start of an `S2C_OBJ_GROUP` sound record: a one-shot (kind 0) or a tracked sound (kind 1, `track` set).
/// Fields that the record's `dmask` did not carry come from the per-file cache.
// SPEC: UDP_SESSION_2009_B.md §8.1 (per-file cache, `dmask`), §8.2 (one-shot), §8.3 (tracked sound)
#[derive(Debug, Clone, PartialEq)]
pub struct SoundStart {
    /// Tracked-sound id (object id space); `None` for a one-shot.
    pub track: Option<u16>,
    /// File id of the session file table.
    pub file: u16,
    /// Request flags after the cache update (`AUDIO_2009.md` §3.2 bits).
    pub flags: u16,
    pub prio: u8,
    pub outer: u16,
    /// `inner = inner_ratio * outer / 255`.
    pub inner_ratio: u8,
    /// 0..=100 (100 when the record carries none).
    pub volume: u8,
    pub pitch: f32,
    /// Start offset (ms) of a tracked sound with flag 0x0400.
    pub start_ms: u32,
    pub tag: u32,
    /// Position (decoded with the world box); `None` for a sound "at the listener" or when no box is known.
    pub pos: Option<[f32; 3]>,
}

impl SoundStart {
    /// Inner radius: `inner_ratio * outer / 255`.
    // SPEC: UDP_SESSION_2009_B.md §8.2 (`inner = inner_ratio * outer_radius / 255`, reader constant 0.003921569)
    pub fn inner(&self) -> f32 {
        self.inner_ratio as f32 * self.outer as f32 * 0.003_921_569
    }
}

/// What the sound records of an `S2C_OBJ_GROUP` asked for, in arrival order (the consumer drains them).
// SPEC: UDP_SESSION_2009_B.md §8.3 (`tmask` bit 0 start, bit 1 position, bit 5 PLAUSIBLY stop; removal through the
// removed-object record of §5.1, because the id table is shared)
#[derive(Debug, Clone, PartialEq)]
pub enum SoundNote {
    Start(SoundStart),
    /// `tmask` bit 1 without a start: the playing tracked sound moves.
    Move {
        track: u16,
        pos: [f32; 3],
    },
    /// `tmask` bit 5 (PLAUSIBLE stop) or the tracked id in a removal record.
    Stop {
        track: u16,
    },
}

/// The per-file sound description cache: flags, priority, outer radius, inner ratio.
// SPEC: UDP_SESSION_2009_B.md §8.1 (with no entry a zeroed temporary is used)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SoundCacheEntry {
    pub flags: u16,
    pub prio: u8,
    pub outer: u16,
    pub inner_ratio: u8,
}

/// Most sound events the table keeps for a consumer that does not drain them (a bot never does).
pub const SOUND_EVENTS_KEEP: usize = 512;

/// The client object table and the counters of everything the readers saw.
#[derive(Debug, Clone, Default)]
pub struct ObjectTable {
    pub objects: BTreeMap<u16, ClientObject>,
    /// The last state of every object a removal record took away (latest removal per id).
    pub removed: BTreeMap<u16, ClientObject>,
    pub extents: Option<ExtentBox>,
    /// Latest accepted server time (seconds) from the stream's time entries.
    pub server_time: Option<f32>,
    pub time_entries: u64,
    pub stale_time_entries: u64,
    /// Accepted time entries that came more than [`SERVER_STALL_S`] of server time after the previous one: ticks the
    /// server itself did not run (a loaded machine), measured on the server's own clock.
    pub server_stalls: u64,
    pub creates: u64,
    pub changes: u64,
    pub removals: u64,
    pub sound_records: u64,
    /// Sound starts, moves and stops read from the sound records (drained by the consumer; at most
    /// [`SOUND_EVENTS_KEEP`] are kept).
    pub sound_events: Vec<SoundNote>,
    /// The per-file description cache of the sound records.
    pub sound_cache: BTreeMap<u16, SoundCacheEntry>,
    /// Tracked-sound ids started and not yet removed.
    pub tracked_sounds: std::collections::BTreeSet<u16>,
    pub stream_entries: u64,
    pub unknown_stream_entries: u64,
    /// Would-be-fatal errors for a 2009 client (code, text).
    pub errors: Vec<(u32, String)>,
}

/// Reads an animation block (main tracker first, `more` bit before every entry).
// SPEC: UDP_SESSION_2009_B.md §4.4 (reader engine+0xa2530)
pub fn skip_anim_block(r: &mut BitReader<'_>) {
    let mut main = true;
    while r.read_bit() {
        if r.read_bit() {
            r.read_bits(14);
            r.read_bit();
        } else {
            r.read_bits(8);
        }
        r.read_bit();
        let tsel = r.read_bits(2) as usize;
        r.read_bits([1, 6, 10, 32][tsel]);
        if !main {
            r.read_u8();
            if r.read_bit() {
                r.read_u8();
            } else {
                r.read_bits(3);
            }
        }
        main = false;
        if r.at_end() {
            break;
        }
    }
}

/// Reads a model file block; `Err` for a block with no model (the client fails the record with code 1).
// SPEC: UDP_SESSION_2009_B.md §3.4 (reader engine+0xa2870 / engine+0xa2700)
pub fn read_model_block(r: &mut BitReader<'_>) -> Result<Vec<u16>, String> {
    let n = r.read_bits(6);
    if n == 0 {
        return Err("model block with model_count 0".into());
    }
    let mut files = Vec::new();
    for m in 0..n {
        files.push(r.read_u16());
        if m == 0 {
            let subs = r.read_bits(6);
            for _ in 0..subs.saturating_sub(1) {
                r.read_u16();
            }
        }
        let tex = r.read_bits(6);
        for _ in 0..tex {
            r.read_u16();
        }
        let styles = r.read_bits(4);
        for _ in 0..styles {
            r.read_u16();
        }
    }
    Ok(files)
}

/// One entry of a model file block: the model file, its sub files (entry 0 only), textures and render styles, all
/// as file ids of the session file table (0xFFFF = empty slot).
// SPEC: UDP_SESSION_2009_B.md §3.4 (order and widths PROVEN; meaning of the three lists PLAUSIBLE)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelFileEntry {
    pub model: u16,
    /// Further files of entry 0 (`sub_count - 1` of them; the count includes the entry itself).
    pub subs: Vec<u16>,
    pub textures: Vec<u16>,
    pub styles: Vec<u16>,
}

/// One tracker of an animation block.
// SPEC: UDP_SESSION_2009_B.md §4.4 (reader engine+0xa2530): main tracker first (no tracker id, no weight set),
// `time_ms = time8 << 3`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnimTracker {
    /// Animation index of the composed model (body clips first, then the child model's).
    pub anim: u16,
    pub state0: bool,
    pub state1: bool,
    pub time_ms: u32,
    /// `None` for the main tracker.
    pub tracker: Option<u8>,
    /// Weight set of a secondary tracker.
    pub blend_set: Option<u8>,
}

/// One entry of an attachment list (`CF_ATTACH`).
// SPEC: UDP_SESSION_2009_B.md §4.2 item 8
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Attachment {
    pub child: u16,
    pub socket: u32,
    pub offset: [f32; 3],
    pub rot: [f32; 4],
}

/// Reads a model file block with every file id; `Err` for a block with no model (the client fails the record with
/// code 1). Consumes the same bits as [`read_model_block`].
// SPEC: UDP_SESSION_2009_B.md §3.4 (reader engine+0xa2870 / engine+0xa2700)
pub fn read_model_entries(r: &mut BitReader<'_>) -> Result<Vec<ModelFileEntry>, String> {
    let n = r.read_bits(6);
    if n == 0 {
        return Err("model block with model_count 0".into());
    }
    let mut out = Vec::with_capacity(n as usize);
    for m in 0..n {
        let mut e = ModelFileEntry {
            model: r.read_u16(),
            ..ModelFileEntry::default()
        };
        if m == 0 {
            let subs = r.read_bits(6);
            for _ in 0..subs.saturating_sub(1) {
                e.subs.push(r.read_u16());
            }
        }
        let tex = r.read_bits(6);
        for _ in 0..tex {
            e.textures.push(r.read_u16());
        }
        let styles = r.read_bits(4);
        for _ in 0..styles {
            e.styles.push(r.read_u16());
        }
        out.push(e);
    }
    Ok(out)
}

/// Reads an animation block with every tracker. Consumes the same bits as [`skip_anim_block`].
// SPEC: UDP_SESSION_2009_B.md §4.4 (reader engine+0xa2530)
pub fn read_anim_block(r: &mut BitReader<'_>) -> Vec<AnimTracker> {
    let mut out = Vec::new();
    let mut main = true;
    while r.read_bit() {
        let mut t = AnimTracker::default();
        if r.read_bit() {
            t.anim = r.read_bits(14) as u16;
            t.state0 = r.read_bit();
        } else {
            t.anim = r.read_bits(8) as u16;
            t.state0 = true;
        }
        t.state1 = r.read_bit();
        let tsel = r.read_bits(2) as usize;
        t.time_ms = r.read_bits([1, 6, 10, 32][tsel]) << 3;
        if !main {
            t.tracker = Some(r.read_u8());
            t.blend_set = Some(if r.read_bit() {
                r.read_u8()
            } else {
                r.read_bits(3) as u8
            });
        }
        main = false;
        out.push(t);
        if r.at_end() {
            break;
        }
    }
    out
}

fn vec96(r: &mut BitReader<'_>) -> [f32; 3] {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

impl ObjectTable {
    /// Applies one `S2C_OBJ_GROUP` message (the bits after the id byte).
    // SPEC: UDP_SESSION_2009_B_addendum_disasm.md B4 (the client's id 8 loop), B5 (special kinds)
    /// An object by id, also when it was removed meanwhile (its last state).
    pub fn seen(&self, id: u16) -> Option<&ClientObject> {
        self.objects.get(&id).or_else(|| self.removed.get(&id))
    }

    fn push_sound(&mut self, e: SoundNote) {
        if self.sound_events.len() >= SOUND_EVENTS_KEEP {
            self.sound_events.remove(0);
        }
        self.sound_events.push(e);
    }

    fn sound_pos(&self, p: Pos48) -> Option<[f32; 3]> {
        self.extents.map(|b| b.decode(p))
    }

    /// One sound record after `[0][kind]` (kind 0 one-shot, kind 1 tracked); `end` is the record's end bit.
    // SPEC: UDP_SESSION_2009_B.md §8.2 (one-shot, reader engine+0xa3a50), §8.3 (tracked, reader engine+0xa3070 /
    // engine+0xa2e90), §8.1 (the cache supplies what `dmask` does not carry; `sflags` tested after the update)
    fn read_sound(&mut self, r: &mut BitReader<'_>, kind: u8, end: usize) {
        let (tmask, track) = if kind == 1 {
            (r.read_u8(), Some(r.read_u16()))
        } else {
            (1, None)
        };
        let mut start = None;
        if tmask & 1 != 0 {
            let file = r.read_u16();
            let dmask = r.read_u8();
            let mut c = self.sound_cache.get(&file).copied().unwrap_or_default();
            if dmask & 1 != 0 {
                c.flags = r.read_u16();
            }
            if dmask & 2 != 0 {
                c.prio = r.read_u8();
            }
            if c.flags & 3 != 0 && dmask & 4 != 0 {
                c.outer = r.read_u16();
                c.inner_ratio = r.read_u8();
            }
            let volume = if c.flags & 0x40 != 0 {
                r.read_u8()
            } else {
                100
            };
            let pitch = if c.flags & 0x80 != 0 {
                r.read_f32()
            } else {
                1.0
            };
            let mut start_ms = 0;
            let mut pos = None;
            if kind == 1 {
                if c.flags & 0x400 != 0 {
                    let b = r.read_u8();
                    start_ms = if b == 0xFF { r.read_u32() } else { b as u32 };
                }
            } else if c.flags & 3 != 0 && c.flags & 0x800 == 0 {
                pos = self.sound_pos(Pos48::read(r));
            }
            let tag = r.read_u32();
            if r.bit_pos() > end {
                return;
            }
            self.sound_cache.insert(file, c);
            start = Some(SoundStart {
                track,
                file,
                flags: c.flags,
                prio: c.prio,
                outer: c.outer,
                inner_ratio: c.inner_ratio,
                volume,
                pitch,
                start_ms,
                tag,
                pos,
            });
        }
        let moved = if tmask & 2 != 0 {
            let p = Pos48::read(r);
            if r.bit_pos() > end {
                return;
            }
            self.sound_pos(p)
        } else {
            None
        };
        if let Some(mut s) = start {
            if s.pos.is_none() && s.flags & 0x800 == 0 {
                s.pos = moved;
            }
            if let Some(t) = s.track {
                self.tracked_sounds.insert(t);
            }
            self.push_sound(SoundNote::Start(s));
        } else if let (Some(track), Some(pos)) = (track, moved) {
            self.push_sound(SoundNote::Move { track, pos });
        }
        if let (Some(track), true) = (track, tmask & 0x20 != 0) {
            self.tracked_sounds.remove(&track);
            self.push_sound(SoundNote::Stop { track });
        }
    }

    pub fn apply_group(&mut self, body: &BitWriter) {
        let mut r = body.reader();
        while r.remaining_bits() >= 32 {
            let rec_bits = r.read_u32() as usize;
            let start = r.bit_pos();
            if rec_bits > r.remaining_bits() {
                self.errors.push((
                    ERR_BAD_RECORD,
                    format!("record of {rec_bits} bits with {} left", r.remaining_bits()),
                ));
                return;
            }
            let end = start + rec_bits;
            let lo = r.read_u8();
            let mut to_end_of_message = false;
            let res = if lo != 0 {
                let cfm = if lo & 0x80 != 0 {
                    lo as u16 | (r.read_u8() as u16) << 8
                } else {
                    lo as u16
                };
                let obj = r.read_u16();
                self.apply_record(&mut r, obj, cfm)
            } else {
                match r.read_u8() {
                    k @ (0 | 1) => {
                        // sound records: read into sound events; the record length bounds them (a record that
                        // does not fit its length gives no event)
                        self.sound_records += 1;
                        self.read_sound(&mut r, k, end);
                        r.seek_bits(end);
                        Ok(())
                    }
                    3 => {
                        // SPEC: UDP_SESSION_2009_B.md §5.1 (ids until the end of the message)
                        while r.remaining_bits() >= 16 {
                            let id = r.read_u16();
                            if let Some(o) = self.objects.remove(&id) {
                                self.removals += 1;
                                self.removed.insert(id, o);
                            }
                            if self.tracked_sounds.remove(&id) {
                                self.push_sound(SoundNote::Stop { track: id });
                            }
                        }
                        to_end_of_message = true;
                        Ok(())
                    }
                    k => Err(format!("special record kind {k}")),
                }
            };
            if let Err(e) = res {
                self.errors.push((ERR_BAD_RECORD, e));
                r.seek_bits(end);
                continue;
            }
            if r.bit_pos() != end && !(to_end_of_message && r.at_end()) {
                self.errors.push((
                    ERR_BAD_RECORD,
                    format!(
                        "record length {rec_bits} but {} bits read (cf {lo:#04x})",
                        r.bit_pos() - start
                    ),
                ));
                r.seek_bits(end);
            }
        }
    }

    /// One object record (create or change).
    // SPEC: UDP_SESSION_2009_B.md §3.3 (create), §4.2 (change fields in order)
    fn apply_record(
        &mut self,
        r: &mut BitReader<'_>,
        obj: u16,
        mut cfm: u16,
    ) -> Result<(), String> {
        let mut o = if cfm & cf::NEW != 0 {
            let kind_byte = r.read_u8();
            let k = kind_byte & 0x3f;
            let create_pos = vec96(r);
            let mut blob_bits = 0u32;
            let mut blob_type = None;
            let mut blob = None;
            if kind_byte & 0x40 != 0 {
                blob_bits = if kind_byte & 0x80 != 0 {
                    r.read_u16() as u32
                } else {
                    r.read_u8() as u32
                };
                if blob_bits <= BLOB_KEEP_BITS && r.remaining_bits() >= blob_bits as usize {
                    let at = r.bit_pos();
                    blob = Some(r.read_bit_string(blob_bits as usize));
                    r.seek_bits(at);
                }
                if blob_bits >= 8 && r.remaining_bits() >= 8 {
                    blob_type = Some(r.read_u8());
                    r.skip_bits(blob_bits as usize - 8);
                } else {
                    r.skip_bits(blob_bits as usize);
                }
            }
            let mut models = Vec::new();
            let mut name = None;
            let mut code = None;
            let mut sprite_file = None;
            match k {
                kind::MODEL => {
                    models = read_model_entries(r)?;
                }
                kind::BRUSH => {
                    name = Some(r.read_zstr8());
                }
                kind::ZONE => {
                    name = Some(r.read_zstr8());
                    code = Some(r.read_u16());
                }
                kind::BILLBOARD => {
                    sprite_file = Some(r.read_u16());
                }
                _ => {}
            }
            // SPEC: UDP_SESSION_2009_B.md §3.3 (the reader adds CF_POS_SNAP itself on create)
            cfm |= cf::POS_SNAP;
            self.creates += 1;
            ClientObject {
                kind: k,
                create_pos,
                pos: Some(create_pos),
                blob_bits,
                blob_type,
                blob,
                models,
                name,
                code,
                sprite_file,
                created: self.creates,
                ..ClientObject::default()
            }
        } else {
            self.changes += 1;
            self.objects
                .get(&obj)
                .cloned()
                .ok_or_else(|| format!("change record for unknown object {obj}"))?
        };
        o.records += 1;
        let k = o.kind;
        if cfm & cf::FILES != 0 {
            match k {
                kind::MODEL => {
                    o.models = read_model_entries(r)?;
                }
                kind::BILLBOARD => {
                    o.sprite_file = Some(r.read_u16());
                }
                _ => {}
            }
        }
        if cfm & cf::ANIM != 0 {
            match k {
                kind::MODEL => {
                    o.anim = read_anim_block(r);
                    o.anim_updates += 1;
                }
                kind::BILLBOARD => {
                    r.read_u64();
                }
                _ => {}
            }
        }
        if cfm & cf::FLAGS != 0 {
            o.flags = r.read_u16();
            let f2 = r.read_u16();
            let uf = r.read_u32();
            let f3 = r.read_u32();
            o.flag_words = Some((o.flags, f2, uf, f3));
            o.uflags = uf;
        }
        if cfm & cf::COLOR != 0 {
            let mut c = [0u8; 5];
            for b in &mut c {
                *b = r.read_u8();
            }
            o.color = Some(c);
            if k == kind::LAMP {
                r.read_u16();
            }
        }
        if cfm & cf::SCALE != 0 {
            let sx = r.read_f32();
            let sy = r.read_f32();
            let sz = if k != kind::BILLBOARD {
                r.read_f32()
            } else {
                1.0
            };
            o.scale = Some([sx, sy, sz]);
        }
        if cfm & cf::POS != 0 {
            let snap = cfm & cf::POS_SNAP != 0;
            if o.flags & FLAG_FULL_PRECISION != 0 {
                let p = vec96(r);
                o.moved_to(p, false);
                o.vel = Some(vec96(r));
                o.pos48 = None;
                o.pos_snaps += u32::from(snap);
            } else {
                let p = Pos48::read(r);
                let v = Vec72::read(r);
                o.pos48 = Some(p);
                // without a world box the create position stays the best known position
                if let Some(d) = self.extents.map(|b| b.decode(p)) {
                    o.moved_to(d, false);
                    o.pos_snaps += u32::from(snap);
                }
                o.vel = Some(decode_vec72(v));
            }
        }
        if cfm & cf::ROT != 0 {
            o.rot = Some(read_quat128(r));
        }
        if cfm & cf::ATTACH != 0 {
            // SPEC: UDP_SESSION_2009_B.md §4.2 item 8 (the reader first clears the object's attachments)
            o.attachments.clear();
            loop {
                if r.remaining_bits() < 16 {
                    return Err("attachment list without its end marker".into());
                }
                let child = r.read_u16();
                if child == 0xFFFF {
                    break;
                }
                let socket = r.read_u32();
                let offset = vec96(r);
                let rot = read_quat128(r);
                o.attachments.push(Attachment {
                    child,
                    socket,
                    offset,
                    rot,
                });
            }
            if k == kind::MODEL {
                r.skip_bits(64 * 32);
            }
        }
        if cfm & cf::DIMS != 0 {
            o.dims = Some(vec96(r));
        }
        self.objects.insert(obj, o);
        Ok(())
    }

    /// Applies one `S2C_OBJ_STREAM` message (the bits after the id byte).
    // SPEC: UDP_SESSION_2009_B.md §4.3 (reader engine+0xa4160), §9.1 (time entry, only later times accepted)
    pub fn apply_stream(&mut self, body: &BitWriter) {
        let mut r = body.reader();
        // the objects whose entries came since the last time entry: that entry's time is theirs
        let mut pending: Vec<u16> = Vec::new();
        while r.remaining_bits() >= 20 {
            let obj = r.read_u16();
            let uf = r.read_bits(4);
            if obj == 0xFFFF && uf == 0 {
                let t = r.read_f32();
                for id in pending.drain(..) {
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.stream_time = Some(t);
                    }
                }
                self.time_entries += 1;
                if self.server_time.is_none_or(|s| t > s) {
                    if self.server_time.is_some_and(|s| t - s > SERVER_STALL_S) {
                        self.server_stalls += 1;
                    }
                    self.server_time = Some(t);
                } else {
                    self.stale_time_entries += 1;
                }
                continue;
            }
            self.stream_entries += 1;
            let known_kind = self.objects.get(&obj).map(|o| o.kind);
            if known_kind.is_none() {
                self.unknown_stream_entries += 1;
            }
            let mut pos = None;
            let mut pos48 = None;
            let mut vel = None;
            if uf & 0x4 != 0 {
                let p = Pos48::read(&mut r);
                pos48 = Some(p);
                pos = self.extents.map(|b| b.decode(p));
                vel = Some(if r.read_bit() {
                    decode_vec72(Vec72::read(&mut r))
                } else {
                    [0.0; 3]
                });
            }
            let mut yaw = None;
            let mut rot = None;
            let mut anim = None;
            if uf & 0x8 != 0 {
                yaw = Some(r.read_s8());
            } else if uf & 0x2 != 0 {
                rot = Some(read_quat128(&mut r));
            }
            if uf & 0x1 != 0 {
                if known_kind.is_some_and(|k| k != kind::MODEL) {
                    // SPEC: UDP_SESSION_2009_B.md §4.3 (UF_ANIM on a non-model object: the reader returns)
                    return;
                }
                anim = Some(read_anim_block(&mut r));
            }
            if let Some(o) = self.objects.get_mut(&obj) {
                o.stream_entries += 1;
                pending.push(obj);
                if let Some(a) = anim {
                    o.anim = a;
                    o.anim_updates += 1;
                }
                if pos48.is_some() {
                    o.pos48 = pos48;
                    if let Some(p) = pos {
                        o.moved_to(p, true);
                    }
                    o.vel = vel;
                }
                if yaw.is_some() {
                    o.yaw8 = yaw;
                }
                if rot.is_some() {
                    o.rot = rot;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(w: &mut BitWriter, rec: &BitWriter) {
        w.write_u32(rec.bit_len() as u32).append(rec);
    }

    fn create_plain(obj: u16, pos: [f32; 3], full: bool) -> BitWriter {
        // cf = NEW | POS | FLAGS (+ the high byte marker when needed)
        let mut rec = BitWriter::new();
        let cfm: u16 = cf::NEW | 0x0002 | cf::FLAGS;
        rec.write_u8(cfm as u8).write_u16(obj);
        rec.write_u8(kind::PLAIN);
        for c in pos {
            rec.write_f32(c);
        }
        rec.write_u16(if full { FLAG_FULL_PRECISION } else { 0 })
            .write_u16(0)
            .write_u32(0)
            .write_u32(0);
        if full {
            for c in pos {
                rec.write_f32(c);
            }
            for _ in 0..3 {
                rec.write_f32(0.0);
            }
        } else {
            Pos48::default().write(&mut rec);
            Vec72::default().write(&mut rec);
        }
        rec
    }

    /// w2-sprites: a billboard's create (the reference stack's sprite form `cf` 0x04db: file, flag words, colour, two-component
    /// scale, compact position, rotation) keeps its file id, scale (third component 1.0) and colour; a later
    /// `CF_FILES` record names a new file.
    #[test]
    fn billboard_create_keeps_file_scale_and_colour() {
        let mut t = ObjectTable::default();
        let mut w = BitWriter::new();
        let mut rec = BitWriter::new();
        let cfm: u16 = cf::NEW | 0x0002 | cf::FLAGS | cf::COLOR | cf::SCALE | 0x0400;
        rec.write_u8((cfm as u8) | 0x80)
            .write_u8((cfm >> 8) as u8)
            .write_u16(40);
        rec.write_u8(kind::BILLBOARD);
        for c in [0.0f32; 3] {
            rec.write_f32(c);
        }
        rec.write_u16(0x0123);
        rec.write_u16(0x0081)
            .write_u16(0x0002)
            .write_u32(1)
            .write_u32(0);
        for b in [200u8, 100, 50, 255, 0] {
            rec.write_u8(b);
        }
        rec.write_f32(0.5).write_f32(0.25);
        Pos48::default().write(&mut rec);
        Vec72::default().write(&mut rec);
        dc_wire::udp::write_quat128(&mut rec, [0.0, 0.0, 0.0, 1.0]);
        record(&mut w, &rec);
        t.apply_group(&w);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        let o = &t.objects[&40];
        assert_eq!(o.kind, kind::BILLBOARD);
        assert_eq!(o.sprite_file, Some(0x0123));
        assert_eq!(o.scale, Some([0.5, 0.25, 1.0]));
        assert_eq!(o.color, Some([200, 100, 50, 255, 0]));
        assert_eq!(o.flag_words, Some((0x0081, 0x0002, 1, 0)));
        assert_eq!(o.rot, Some([0.0, 0.0, 0.0, 1.0]));
        let mut w = BitWriter::new();
        let mut rec = BitWriter::new();
        rec.write_u8(0x80)
            .write_u8(0x08)
            .write_u16(40)
            .write_u16(0x0124);
        record(&mut w, &rec);
        t.apply_group(&w);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert_eq!(t.objects[&40].sprite_file, Some(0x0124));
    }

    #[test]
    fn group_create_change_remove() {
        let mut t = ObjectTable::default();
        let mut w = BitWriter::new();
        record(&mut w, &create_plain(5, [1.0, 2.0, 3.0], true));
        record(&mut w, &create_plain(6, [4.0, 5.0, 6.0], false));
        // sound record kind 0, bounded by its length
        let mut snd = BitWriter::new();
        snd.write_u8(0)
            .write_u8(0)
            .write_u16(0x1234)
            .write_bits(5, 3);
        record(&mut w, &snd);
        t.apply_group(&w);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert_eq!(t.creates, 2);
        assert_eq!(t.sound_records, 1);
        assert_eq!(t.objects[&5].pos, Some([1.0, 2.0, 3.0]));
        // object 6 is compact: no box known -> the create position stays the position
        assert_eq!(t.objects[&6].pos, Some([4.0, 5.0, 6.0]));
        assert!(t.objects[&6].pos48.is_some());
        // removal record: ids to the end of the message
        let mut w = BitWriter::new();
        let mut rm = BitWriter::new();
        rm.write_u8(0).write_u8(3).write_u16(6).write_u16(77);
        record(&mut w, &rm);
        t.apply_group(&w);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert_eq!(t.removals, 1);
        assert!(!t.objects.contains_key(&6));
        // the id created again: a new generation
        let first = t.objects[&5].created;
        let mut w = BitWriter::new();
        let mut rm = BitWriter::new();
        rm.write_u8(0).write_u8(3).write_u16(5);
        record(&mut w, &rm);
        t.apply_group(&w);
        let mut w = BitWriter::new();
        record(&mut w, &create_plain(5, [7.0, 8.0, 9.0], true));
        t.apply_group(&w);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert!(t.objects[&5].created > first);
    }

    #[test]
    fn change_for_unknown_object_and_bad_length_are_reported() {
        let mut t = ObjectTable::default();
        let mut w = BitWriter::new();
        let mut rec = BitWriter::new();
        rec.write_u8(cf::FLAGS as u8)
            .write_u16(9)
            .write_u16(0)
            .write_u16(0);
        rec.write_u32(0).write_u32(0);
        record(&mut w, &rec);
        // a create with a wrong length: 8 bits more announced than written
        let good = create_plain(3, [0.0; 3], true);
        w.write_u32(good.bit_len() as u32 + 8)
            .append(&good)
            .write_u8(0);
        t.apply_group(&w);
        assert_eq!(t.errors.len(), 2, "{:?}", t.errors);
        assert!(t.errors.iter().all(|e| e.0 == ERR_BAD_RECORD));
        assert!(t.objects.contains_key(&3));
    }

    #[test]
    fn stream_time_and_position_entries() {
        let mut t = ObjectTable {
            extents: Some(ExtentBox {
                min: [-512.0, -64.0, -512.0],
                max: [512.0, 256.0, 512.0],
            }),
            ..ObjectTable::default()
        };
        let mut g = BitWriter::new();
        record(&mut g, &create_plain(2, [0.0; 3], false));
        t.apply_group(&g);
        let (p, _) = Pos48::encode_presumed(
            [100.0, 0.0, -50.0],
            t.extents.unwrap().min,
            t.extents.unwrap().max,
        );
        let mut s = BitWriter::new();
        s.write_u16(2).write_bits(0x4, 4);
        p.write(&mut s);
        s.write_bit(true);
        Vec72::encode([10.0, 0.0, 0.0]).write(&mut s);
        s.write_u16(0xFFFF).write_bits(0, 4).write_f32(1.5);
        s.write_u16(0xFFFF).write_bits(0, 4).write_f32(1.0);
        t.apply_stream(&s);
        let o = &t.objects[&2];
        // the time entry that follows the object's entry is its time, not the later (stale) one
        assert_eq!(o.stream_time, Some(1.5));
        let pos = o.pos.unwrap();
        assert!(
            (pos[0] - 100.0).abs() < 0.2 && (pos[2] + 50.0).abs() < 0.2,
            "{pos:?}"
        );
        assert!((o.vel.unwrap()[0] - 10.0).abs() < 1e-3);
        assert_eq!(t.server_time, Some(1.5));
        assert_eq!(t.stale_time_entries, 1);
    }

    #[test]
    fn vec72_decoder_inverts_the_encoder() {
        for v in [[3.0f32, -1.0, 0.5], [-0.25, 8.0, 2.0], [0.0, 0.0, -9.0]] {
            let d = decode_vec72(Vec72::encode(v));
            for i in 0..3 {
                assert!(
                    (d[i] - v[i]).abs()
                        <= v.iter().fold(0.0f32, |m, x| m.max(x.abs())) / 100_000.0 + 1e-6
                );
            }
        }
    }

    #[test]
    fn anim_block_with_secondary_tracker() {
        let mut w = BitWriter::new();
        // main: narrow anim 5, state1, tsel 1 (6 bits)
        w.write_bit(true)
            .write_bit(false)
            .write_bits(5, 8)
            .write_bit(true)
            .write_bits(1, 2)
            .write_bits(9, 6);
        // secondary: wide anim 300 + state0, state1, tsel 3 (32 bits), tracker 2, narrow weight set 3
        w.write_bit(true)
            .write_bit(true)
            .write_bits(300, 14)
            .write_bit(true)
            .write_bit(false);
        w.write_bits(3, 2)
            .write_u32(77)
            .write_u8(2)
            .write_bit(false)
            .write_bits(3, 3);
        w.write_bit(false).write_u8(0xAB);
        let mut r = w.reader();
        skip_anim_block(&mut r);
        assert_eq!(r.read_u8(), 0xAB);
        // the full reader consumes the same bits and keeps every field
        let mut r = w.reader();
        let a = read_anim_block(&mut r);
        assert_eq!(r.read_u8(), 0xAB);
        assert_eq!(
            a,
            vec![
                AnimTracker {
                    anim: 5,
                    state0: true,
                    state1: true,
                    time_ms: 9 << 3,
                    tracker: None,
                    blend_set: None,
                },
                AnimTracker {
                    anim: 300,
                    state0: true,
                    state1: false,
                    time_ms: 77 << 3,
                    tracker: Some(2),
                    blend_set: Some(3),
                },
            ]
        );
    }

    /// A kind-1 create with a blob, a two-entry model block, an attachment and a later stream entry with an
    /// animation block: the table keeps the file ids, the attachment and the trackers (synthetic values).
    #[test]
    fn model_object_keeps_files_attachments_and_trackers() {
        let mut rec = BitWriter::new();
        let cfm: u16 = cf::NEW | 0x0002 | cf::FLAGS | 0x0080 | cf::ATTACH;
        rec.write_u8(cfm as u8)
            .write_u8((cfm >> 8) as u8)
            .write_u16(40);
        rec.write_u8(kind::MODEL | 0x40);
        for _ in 0..3 {
            rec.write_f32(0.0);
        }
        // blob: 16 bits, type byte 0x18 then one byte
        rec.write_u8(16).write_u8(0x18).write_u8(7);
        // model block: 2 entries; entry 0 with one sub file, 2 textures, 1 style; entry 1 with 1 texture
        rec.write_bits(2, 6)
            .write_u16(10)
            .write_bits(2, 6)
            .write_u16(11);
        rec.write_bits(2, 6)
            .write_u16(12)
            .write_u16(13)
            .write_bits(1, 4)
            .write_u16(14);
        rec.write_u16(20)
            .write_bits(1, 6)
            .write_u16(21)
            .write_bits(0, 4);
        rec.write_u16(0x0800).write_u16(0).write_u32(0).write_u32(0);
        Pos48::default().write(&mut rec);
        Vec72::default().write(&mut rec);
        rec.write_u16(41).write_u32(2);
        for c in [0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] {
            rec.write_f32(c);
        }
        rec.write_u16(0xFFFF);
        for _ in 0..64 {
            rec.write_u32(0);
        }
        let mut g = BitWriter::new();
        record(&mut g, &rec);
        let mut t = ObjectTable::default();
        t.apply_group(&g);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        let o = &t.objects[&40];
        assert_eq!(o.blob_type, Some(0x18));
        assert_eq!(o.models.len(), 2);
        assert_eq!(
            (
                o.models[0].model,
                &o.models[0].subs,
                &o.models[0].textures,
                &o.models[0].styles
            ),
            (10, &vec![11], &vec![12, 13], &vec![14])
        );
        assert_eq!((o.models[1].model, &o.models[1].textures), (20, &vec![21]));
        assert_eq!(o.attachments.len(), 1);
        assert_eq!((o.attachments[0].child, o.attachments[0].socket), (41, 2));
        assert_eq!(o.attachments[0].rot, [0.0, 0.0, 0.0, 1.0]);
        // a stream entry with yaw8 and a three-tracker block
        let mut s = BitWriter::new();
        s.write_u16(40).write_bits(0x8 | 0x1, 4).write_s8(10);
        for (k, (anim, time8)) in [(0u16, 4u32), (96, 5), (6, 6)].iter().enumerate() {
            s.write_bit(true)
                .write_bit(false)
                .write_bits(*anim as u32, 8)
                .write_bit(true);
            s.write_bits(2, 2).write_bits(*time8, 10);
            if k > 0 {
                s.write_u8(k as u8)
                    .write_bit(false)
                    .write_bits(k as u32 - 1, 3);
            }
        }
        s.write_bit(false);
        t.apply_stream(&s);
        let o = &t.objects[&40];
        assert_eq!(o.yaw8, Some(10));
        assert_eq!(o.anim_updates, 1);
        let got: Vec<(u16, u32, Option<u8>, Option<u8>)> = o
            .anim
            .iter()
            .map(|a| (a.anim, a.time_ms, a.tracker, a.blend_set))
            .collect();
        assert_eq!(
            got,
            vec![
                (0, 32, None, None),
                (96, 40, Some(1), Some(0)),
                (6, 48, Some(2), Some(1))
            ]
        );
    }

    fn bytes(s: &str) -> BitWriter {
        let mut w = BitWriter::new();
        for i in (0..s.len()).step_by(2) {
            w.write_u8(u8::from_str_radix(&s[i..i + 2], 16).unwrap());
        }
        w
    }

    #[test]
    fn the_reference_world_sound_record_becomes_a_tracked_start_and_the_cache_fills_later_records() {
        let mut t = ObjectTable {
            extents: Some(ExtentBox {
                min: [-1000.0; 3],
                max: [1000.0; 3],
            }),
            ..ObjectTable::default()
        };
        // OBSERVED: UDP_SESSION_2009_B.md §8.3 (tdm4 c6 seq 807, the looping wind: the 18 bytes before its pos48)
        let mut rec = bytes("000103d6000800073501001027ff00000000");
        Pos48 {
            px: 0x8000,
            py: 0x8000,
            pz: 0x8000,
        }
        .write(&mut rec);
        // a second start of the same file with dmask 0: flags, priority and radii from the cache
        let mut again = BitWriter::new();
        again
            .write_u8(0)
            .write_u8(1)
            .write_u8(3)
            .write_u16(0x00d8)
            .write_u16(8)
            .write_u8(0)
            .write_u32(0);
        Pos48::default().write(&mut again);
        let mut w = BitWriter::new();
        record(&mut w, &rec);
        record(&mut w, &again);
        t.apply_group(&w);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert_eq!(t.sound_records, 2);
        let SoundNote::Start(s) = &t.sound_events[0] else {
            panic!("{:?}", t.sound_events)
        };
        assert_eq!(
            (s.track, s.file, s.flags, s.prio, s.outer, s.inner_ratio),
            (Some(0xd6), 8, 0x0135, 0, 10000, 255)
        );
        assert_eq!((s.volume, s.tag), (100, 0));
        assert!((s.inner() - 10000.0).abs() < 0.5);
        let p = s.pos.expect("decoded position");
        assert!(p.iter().all(|v| v.abs() < 1.0), "{p:?}");
        let SoundNote::Start(s2) = &t.sound_events[1] else {
            panic!()
        };
        assert_eq!(
            (s2.track, s2.flags, s2.outer, s2.inner_ratio),
            (Some(0xd8), 0x0135, 10000, 255)
        );
        // a position update, then the removal record ends both
        let mut w = BitWriter::new();
        let mut mv = BitWriter::new();
        mv.write_u8(0).write_u8(1).write_u8(2).write_u16(0xd6);
        Pos48::default().write(&mut mv);
        record(&mut w, &mv);
        let mut rm = BitWriter::new();
        rm.write_u8(0).write_u8(3).write_u16(0xd6).write_u16(0xd8);
        record(&mut w, &rm);
        t.apply_group(&w);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert!(matches!(
            t.sound_events[2],
            SoundNote::Move { track: 0xd6, .. }
        ));
        assert_eq!(t.sound_events[3], SoundNote::Stop { track: 0xd6 });
        assert_eq!(t.sound_events[4], SoundNote::Stop { track: 0xd8 });
        assert!(t.tracked_sounds.is_empty());
    }
}

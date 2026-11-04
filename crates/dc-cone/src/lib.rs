//! The 2009 shot cone, one implementation for the client (`dc-client`), the server's shot check (`dc-dserver`) and
//! the bots (`dc-botclient`).
//!
//! The 2009 client tilts every pellet away from the forward axis of its aim rotation by two draws of a fixed
//! 10000-entry table (the ROM attribute entry [`TABLE_ENTRY`]), read at the player's shot counter, which the server
//! sets (0x7d `own_value`) and advances (0xda kind 1) and which the `UP_FIRE` 0x82 carries as `stat` after the
//! pellet's two draws. The offsets `(2r - 1) h / 1000` along the rotation's right and up axes form a square cone whose
//! half-width `h` is one integer from the weapon row's perturb bounds, the shot scale and the spread fraction. Every
//! step is binary32, like the 2009 client's x87 at 24-bit precision.
//!
//! Parts: the table ([`ShotTable`], [`generated_table`], [`rom_table`]), the reads ([`ShotCounter`], [`draw_pair`],
//! [`draws_of_stat`]), the row keys ([`ConeRow`], [`ModRow`]) and the fire routine's numbers ([`shot_scale`],
//! [`pellets`], [`range`]), the half-width ([`half_width_of`], [`half_width`]), the direction ([`axes`], [`offset`],
//! [`tilt`], [`pellet_dir`], [`pellet`]) and the inverse ([`implied_h`], [`replay`]).
//!
//! The table is never part of the tree: it is read from the user's ROM at run time, or rebuilt by its generator (the
//! C runtime's classic `rand()` from state 1) where no ROM entry is loaded.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1 (table), AC-2 (cone), AC-3 (fire routine), AC-5 (replay), AC-6, AC-7
// EVIDENCE: cshell+0x176cb0 (cone), cshell+0x2973a0 (table read), cshell+0x17f7b0 (row binary defaults)
// DISASM: cshell+0x2974d0 (table load), cshell+0x23bca0 (fire routine: scale, pellets, range, zoom byte)

use std::sync::OnceLock;

use dc_bute::ButeDoc;
use dc_rez::RezVfs;

// --------------------------------------------------------------------------------------------- the table

/// Archive, entry, tag and key prefix of the random table (`Random0`, `Random1`, ...).
// DISASM: cshell+0x2974d0 (entry name `Attributes\randomtable.txt` handed by cshell+0x29e740; tag and key strings
// VA 0x374f7760 / 0x374f7768, key format `%s%d`)
// OBSERVED: ROM table key RANDOMTABLE.TXT (entry of ATTRIBUTES.rez), tag RandomTable, keys Random<i>
pub const TABLE_REZ: &str = "ATTRIBUTES.rez";
pub const TABLE_ENTRY: &str = "ATTRIBUTES\\RANDOMTABLE.TXT";
pub const TABLE_TAG: &str = "RandomTable";
pub const TABLE_KEY: &str = "Random";
/// Entries the load reserves and the generator produces.
// DISASM: cshell+0x297538 (immediate 0x2710)
pub const TABLE_LEN: usize = 10_000;
/// SHA-256 of the table as 10000 little-endian binary32 values: the identity of the 2009 table, checked against both
/// the generator and the ROM entry without writing a value down.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.2
pub const TABLE_SHA256: &str = "c25eed27d064c4331cb91d8edae4c28276ca3103218c42c280c5c52b160ba386";

/// The table rebuilt by its generator: `rand()` (`state := state x 214013 + 2531011`, output bits 16..30) from
/// state 1, each output times `f32(1 / 32766)` in binary32, written with six significant digits and read back (the
/// ROM holds the values as text).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.3 (the generator equals the ROM entry, 10000 of 10000)
pub fn generated_table() -> Vec<f32> {
    let mut state: u32 = 1;
    let step = 1.0f32 / 32766.0f32;
    (0..TABLE_LEN)
        .map(|_| {
            state = state.wrapping_mul(214_013).wrapping_add(2_531_011);
            let r = ((state >> 16) & 0x7fff) as f32;
            let text = format!("{:.5e}", f64::from(r * step));
            text.parse::<f64>().unwrap_or(0.0) as f32
        })
        .collect()
}

/// [`generated_table`], built once per process.
pub fn shared_generated() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(generated_table)
}

/// The table of a parsed attribute document: the contiguous run `Random0`, `Random1`, ... of tag `RandomTable`, each
/// read as a float; `None` when the run is empty or a value does not parse.
// DISASM: cshell+0x2974d0 (count := 0; while the tag has key `Random<count>`: read it as a float, count += 1)
pub fn table_from_doc(doc: &ButeDoc) -> Option<Vec<f32>> {
    let mut v = Vec::with_capacity(TABLE_LEN);
    while let Some(s) = doc.get_str(TABLE_TAG, &format!("{TABLE_KEY}{}", v.len())) {
        let t = s.trim();
        let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
        v.push(t.parse::<f64>().ok()? as f32);
    }
    (!v.is_empty()).then_some(v)
}

/// The table as the 2009 client loads it from the user's ROM (`None`: no entry, or [`table_from_doc`] fails).
pub fn rom_table(vfs: &RezVfs) -> Option<Vec<f32>> {
    let doc = ButeDoc::from_vfs(vfs, TABLE_REZ, TABLE_ENTRY)?;
    table_from_doc(&doc)
}

/// SHA-256 (hex) of a table as little-endian binary32 values (compare with [`TABLE_SHA256`]).
pub fn table_sha256(t: &[f32]) -> String {
    let bytes: Vec<u8> = t.iter().flat_map(|v| v.to_le_bytes()).collect();
    dc_rez::dc_rom::sha256_hex(&bytes)
}

/// The cone's random table (values in [0, 1)) with where it came from. Dereferences to the values.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotTable {
    values: Vec<f32>,
    /// "rom" or "generator".
    pub source: &'static str,
}

impl ShotTable {
    /// The ROM entry's table ([`rom_table`]).
    pub fn from_rom(vfs: &RezVfs) -> Option<ShotTable> {
        rom_table(vfs).map(|values| ShotTable {
            values,
            source: "rom",
        })
    }

    /// The table of a parsed attribute document ([`table_from_doc`]).
    pub fn from_doc(doc: &ButeDoc) -> Option<ShotTable> {
        table_from_doc(doc).map(|values| ShotTable {
            values,
            source: "rom",
        })
    }

    /// The generator's table ([`generated_table`]).
    pub fn generated() -> ShotTable {
        ShotTable {
            values: shared_generated().to_vec(),
            source: "generator",
        }
    }

    /// The ROM's table, else the generator's.
    pub fn open(rom: Option<&std::path::Path>) -> ShotTable {
        rom.and_then(|p| Self::from_rom(&RezVfs::new(p)))
            .unwrap_or_else(Self::generated)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<f32> {
        self.values.get(i).copied()
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }
}

impl std::ops::Deref for ShotTable {
    type Target = [f32];

    fn deref(&self) -> &[f32] {
        &self.values
    }
}

// --------------------------------------------------------------------------------------------- the reads

/// One read of the table: a counter at or beyond the table size is first reduced modulo the size and the reduced
/// value is kept; the caller adds 1 after the read. An empty table reads the middle (no offset).
// EVIDENCE: cshell+0x2973a0 (seed >= count: seed := seed mod count, stored back); DISASM: cshell+0x2973a0
// (fld dword [data + 4 x seed]); SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.4
pub fn read(t: &[f32], seed: &mut u32) -> f32 {
    let n = t.len() as u32;
    if n == 0 {
        return 0.5;
    }
    if *seed >= n {
        *seed %= n;
    }
    t[*seed as usize]
}

/// The two draws of one pellet from the counter `c` (right axis, then up axis) and the counter after them (the
/// `stat` the message carries).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.4, AC-2.4
pub fn draw_pair(t: &[f32], c: u32) -> (f32, f32, u32) {
    let mut s = c;
    let r1 = read(t, &mut s);
    s = s.wrapping_add(1);
    let r2 = read(t, &mut s);
    s = s.wrapping_add(1);
    (r1, r2, s)
}

/// The two draws a message's `stat` implies: entries `(stat - 2) mod N` and `(stat - 1) mod N`.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.4 (wrap rule), AC-5 (replay method)
pub fn draws_of_stat(t: &[f32], stat: u32) -> (f32, f32) {
    let n = t.len() as u64;
    if n == 0 {
        return (0.5, 0.5);
    }
    let s = u64::from(stat);
    (
        t[((s + 2 * n - 2) % n) as usize],
        t[((s + 2 * n - 1) % n) as usize],
    )
}

/// The shot counter (`local_player`+0x1fc of the 2009 client).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShotCounter {
    pub value: u32,
}

impl ShotCounter {
    /// `GS_MATCH_SETTINGS` 0x7d: the counter := `own_value`.
    // SPEC: GAME_MESSAGES_2009_S2C.md 0x7d row (`local_player`+0x1fc := own_value); addendum AC-1.5
    pub fn set(&mut self, v: u32) {
        self.value = v;
    }

    /// `GS_PLAYER_POINTS` 0xda kind 1: the counter += `delta`.
    // SPEC: GAME_MESSAGES_2009_S2C.md 0xda row (`local_player`+0x1fc += delta, cshell+0x61521..0x61539)
    pub fn add(&mut self, delta: u32) {
        self.value = self.value.wrapping_add(delta);
    }

    /// One draw ([`read`]), then the counter is incremented.
    // EVIDENCE: cshell+0x2973a0; the caller adds 1 (addendum AC-1.4)
    pub fn draw(&mut self, t: &[f32]) -> f32 {
        let r = read(t, &mut self.value);
        self.value = self.value.wrapping_add(1);
        r
    }
}

// --------------------------------------------------------------------------------------------- the row's cone keys

/// Row keys of the cone and the fire routine.
// OBSERVED: ROM table keys WEAPONS.TXT [Weapon<n>] MinPerturb, MaxPerturb, ZoomedMinPerturb, ZoomedMaxPerturb,
// TripleShotMinPerturb, TripleShotMaxPerturb, DynamicPerturbFactor, VectorsPerRound, Range; [Mod<n>]
// DynamicPerturbFactor; key strings of CShell.dll DynnamicPerturbFactorJump / ...Walk / ...Run (the binary's
// spelling), RangePer, VectorsPerRoundPer (no ROM row sets these five)
pub mod key {
    pub const MIN: &str = "MinPerturb";
    pub const MAX: &str = "MaxPerturb";
    pub const ZOOMED_MIN: &str = "ZoomedMinPerturb";
    pub const ZOOMED_MAX: &str = "ZoomedMaxPerturb";
    pub const TRIPLE_MIN: &str = "TripleShotMinPerturb";
    pub const TRIPLE_MAX: &str = "TripleShotMaxPerturb";
    pub const SCALE: &str = "DynamicPerturbFactor";
    pub const SCALE_JUMP: &str = "DynnamicPerturbFactorJump";
    pub const SCALE_WALK: &str = "DynnamicPerturbFactorWalk";
    pub const SCALE_RUN: &str = "DynnamicPerturbFactorRun";
    pub const RANGE_PER: &str = "RangePer";
    pub const PELLETS_PER: &str = "VectorsPerRoundPer";
}

/// Binary defaults of the row fields the cone reads when the row has no such key.
// EVIDENCE: cshell+0x17f7b0 (row constructor stores: DynamicPerturbFactor 1.0, the three movement factors 1.0, the
// zoomed pair 1 and 500, VectorsPerRound 1, Range 3000); WEAPON_FIRE_2009.md §2.3, §2.5, §5.2, §5.4
pub mod row_default {
    pub const SCALE: f32 = 1.0;
    pub const MOVE_SCALE: f32 = 1.0;
    pub const ZOOMED_MIN: i32 = 1;
    pub const ZOOMED_MAX: i32 = 500;
    pub const PELLETS: u32 = 1;
    pub const RANGE: f32 = 3000.0;
}

/// The cone numbers of one weapon row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeRow {
    pub min: i32,
    pub max: i32,
    /// `ZoomedMinPerturb`, `ZoomedMaxPerturb` (0 = use the plain bound).
    pub zoomed: (i32, i32),
    /// `TripleShotMinPerturb`, `TripleShotMaxPerturb` (0 = use the plain bound).
    pub triple: (i32, i32),
    /// `DynamicPerturbFactor`.
    pub scale: f32,
    /// Jump, walk, run factors.
    pub scale_jump: f32,
    pub scale_walk: f32,
    pub scale_run: f32,
}

impl Default for ConeRow {
    /// A row with none of the keys: no plain cone (the bounds 0), the binary defaults otherwise.
    // DC-DEFAULT: CLI-120 a missing MinPerturb / MaxPerturb is 0 (no cone); a weapon without a row has none
    fn default() -> Self {
        ConeRow {
            min: 0,
            max: 0,
            zoomed: (row_default::ZOOMED_MIN, row_default::ZOOMED_MAX),
            triple: (0, 0),
            scale: row_default::SCALE,
            scale_jump: row_default::MOVE_SCALE,
            scale_walk: row_default::MOVE_SCALE,
            scale_run: row_default::MOVE_SCALE,
        }
    }
}

/// A finite number of a section key (case-insensitive name; a trailing `f` is allowed).
pub fn num(sec: &dc_bute::SectionMap, k: &str) -> Option<f32> {
    let t = sec
        .iter()
        .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
        .map(|(_, v)| v)?
        .trim();
    let t = t.strip_suffix(['f', 'F']).unwrap_or(t);
    t.parse::<f32>().ok().filter(|x| x.is_finite())
}

impl ConeRow {
    /// The cone keys of a `[Weapon<n>]` section; integer keys are truncated like the client's integer reader.
    // EVIDENCE: cshell+0x17bd20 (MinPerturb +0x288, MaxPerturb +0x28c read as integers); addendum AC-2.1 (zoomed
    // +0x298 / +0x29c, triple +0x330 / +0x334), AC-3.2 (+0x274, +0x278 run, +0x27c walk, +0x280 jump)
    pub fn of_section(sec: &dc_bute::SectionMap) -> ConeRow {
        let d = ConeRow::default();
        let int = |k: &str, dflt: i32| num(sec, k).map_or(dflt, |v| v.trunc() as i32);
        let f = |k: &str, dflt: f32| num(sec, k).unwrap_or(dflt);
        ConeRow {
            min: int(key::MIN, d.min),
            max: int(key::MAX, d.max),
            zoomed: (
                int(key::ZOOMED_MIN, d.zoomed.0),
                int(key::ZOOMED_MAX, d.zoomed.1),
            ),
            triple: (int(key::TRIPLE_MIN, 0), int(key::TRIPLE_MAX, 0)),
            scale: f(key::SCALE, d.scale),
            scale_jump: f(key::SCALE_JUMP, d.scale_jump),
            scale_walk: f(key::SCALE_WALK, d.scale_walk),
            scale_run: f(key::SCALE_RUN, d.scale_run),
        }
    }
}

/// The fire-routine numbers of one `[Mod<n>]` row.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModRow {
    /// `DynamicPerturbFactor` (added to the row's, AC-3.2).
    pub scale: f32,
    /// `RangePer` (slot 0 only, AC-3.4).
    pub range_per: f32,
    /// `VectorsPerRoundPer` (slot 2 only, AC-3.4).
    pub pellets_per: f32,
}

impl ModRow {
    // DC-DEFAULT: CLI-122 a missing mod key reads 0 (the mod constructor's defaults, cshell+0x175730, are not read;
    // no ROM mod row sets RangePer or VectorsPerRoundPer)
    pub fn of_section(sec: &dc_bute::SectionMap) -> ModRow {
        ModRow {
            scale: num(sec, key::SCALE).unwrap_or(0.0),
            range_per: num(sec, key::RANGE_PER).unwrap_or(0.0),
            pellets_per: num(sec, key::PELLETS_PER).unwrap_or(0.0),
        }
    }
}

/// The weapon's three mod slots.
pub type ModSlots = [Option<ModRow>; 3];
/// No mod attached.
pub const NO_MODS: ModSlots = [None; 3];

// --------------------------------------------------------------------------------------------- the fire routine

/// The movement state the fire routine picks its factor from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveState {
    /// Airborne after a jump (mover +0x80).
    pub airborne: bool,
    /// Control bit ANY_MOVE 0x800.
    pub any_move: bool,
    /// Control bit RUN 0x200.
    pub run: bool,
}

impl MoveState {
    /// The state of a control word (mover+0x24) and the airborne flag.
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-3.2 (mover+0x80; control word bits 0x800, 0x200); MOVEMENT_2009.md
    pub fn of_controls(controls: u32, airborne: bool) -> MoveState {
        use dc_sim::player::bits;
        MoveState {
            airborne,
            any_move: controls & bits::ANY_MOVE != 0,
            run: controls & bits::RUN != 0,
        }
    }
}

/// The shot scale (0x82 `shot_scale`): the row's DynamicPerturbFactor plus the mods', raised to the movement factor
/// (jump when airborne, else walk with ANY_MOVE, else run with RUN) when that factor is non-zero and larger.
// DISASM: cshell+0x23bca0 (sum over the mod slots, store weapon+0x20c; mover+0x80, control 0x800, 0x200; compare with
// 0.0 and raise); SPEC: addendum AC-3.2
pub fn shot_scale(row: &ConeRow, mods: &ModSlots, m: MoveState) -> f32 {
    let mut scale = row.scale;
    let mut add = 0.0f32;
    for md in mods.iter().flatten() {
        add += md.scale;
    }
    scale += add;
    let factor = if m.airborne {
        Some(row.scale_jump)
    } else if m.any_move {
        Some(row.scale_walk)
    } else if m.run {
        Some(row.scale_run)
    } else {
        None
    };
    if let Some(f) = factor {
        if f != 0.0 && scale < f {
            scale = f;
        }
    }
    scale
}

/// Pellets of one shot: `VectorsPerRound + trunc(VectorsPerRound x VectorsPerRoundPer)` of mod slot 2.
// DISASM: cshell+0x23bca0 (n := row +0x2b0; slot 2: n += trunc(n x mod +0x244)); SPEC: addendum AC-3.4
pub fn pellets(row_pellets: Option<u32>, mods: &ModSlots) -> u32 {
    let n = row_pellets.unwrap_or(row_default::PELLETS);
    let extra = mods[2].map_or(0, |m| (n as f32 * m.pellets_per).trunc() as i64);
    (i64::from(n) + extra).clamp(0, i64::from(u32::MAX)) as u32
}

/// Trace length of a hitscan pellet: `trunc(Range x (1 + RangePer))` of mod slot 0; 3000 without a row range.
// DISASM: cshell+0x23aa06..0x23aa32 (range x (1 + mod slot 0 +0x234)); EVIDENCE: cshell+0x17f7b0 (Range default 3000)
pub fn range(row_range: Option<f32>, mods: &ModSlots) -> f32 {
    let r = row_range.unwrap_or(row_default::RANGE);
    match mods[0] {
        Some(m) => (r * (1.0 + m.range_per)).trunc(),
        None => r,
    }
}

// --------------------------------------------------------------------------------------------- the half-width

/// What the cone of one pellet does for a given row, state, scale and spread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HalfWidth {
    /// The bounds are not known (a key the state needs is absent, or the arithmetic left the finite range).
    Unknown,
    /// The gate failed (`lo > hi` or `hi <= 0`): nothing drawn, the counter does not move, the pellet keeps the
    /// forward axis.
    NoDraw,
    /// The half-width in thousandths.
    H(i64),
}

/// One bound by state: the zoomed value while zoomed when it is non-zero, else the triple-shot value in triple-shot
/// mode when it is non-zero, else the plain one.
// DISASM: cshell+0x176ce1..0x176d6a (each bound on its own); SPEC: addendum AC-2.1
pub fn pick(plain: i32, zoomed: i32, triple: i32, zoom: bool, tri: bool) -> i32 {
    if zoom && zoomed != 0 {
        zoomed
    } else if tri && triple != 0 {
        triple
    } else {
        plain
    }
}

/// The scaled bounds `(trunc(lo x scale), trunc(hi x scale))`, binary32.
// DISASM: cshell+0x176ce1..0x176d6a; SPEC: addendum AC-2.1, AC-2.6
pub fn scaled_bounds(lo: i32, hi: i32, scale: f32) -> (f32, f32) {
    ((lo as f32 * scale).trunc(), (hi as f32 * scale).trunc())
}

/// The half-width for the picked bounds `lo`, `hi` (unscaled), the scale and the spread fraction `f`: the gate
/// `lo <= hi && hi > 0` on the scaled bounds, then `trunc(trunc((hi - lo) f) scale + lo)`, every product rounded to
/// binary32.
// EVIDENCE: cshell+0x176cb0 (bounds, gate, half-width); DISASM: cshell+0x176d6a..0x176dc6 (x87 operands); SPEC:
// addendum AC-2.2, AC-2.3, AC-2.6
pub fn half_width_of(lo: i32, hi: i32, scale: f32, f: f32) -> HalfWidth {
    let (lo, hi) = scaled_bounds(lo, hi, scale);
    if !(lo <= hi && hi > 0.0) {
        return HalfWidth::NoDraw;
    }
    let h = (((hi - lo) * f).trunc() * scale + lo).trunc();
    if h.is_finite() {
        HalfWidth::H(h as i64)
    } else {
        HalfWidth::Unknown
    }
}

/// The integer half-width of a row for spread `f` (`None`: the gate refuses, nothing is drawn).
// SPEC: addendum AC-2.1 .. AC-2.3
pub fn half_width(row: &ConeRow, zoom: bool, triple: bool, scale: f32, f: f32) -> Option<i64> {
    let lo = pick(row.min, row.zoomed.0, row.triple.0, zoom, triple);
    let hi = pick(row.max, row.zoomed.1, row.triple.1, zoom, triple);
    match half_width_of(lo, hi, scale, f) {
        HalfWidth::H(h) => Some(h),
        HalfWidth::NoDraw | HalfWidth::Unknown => None,
    }
}

// --------------------------------------------------------------------------------------------- the direction

/// Right, up and forward axes of a rotation `(x, y, z, w)` (rotated +X, +Y, +Z), binary32.
// EVIDENCE: cshell+0x1b800 (right), cshell+0x1b850 (up), cshell+0x1b8a0 (forward)
pub fn axes(q: [f32; 4]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let [x, y, z, w] = q;
    (
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ],
        [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ],
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    )
}

/// One axis offset for the draw `r` and the half-width `h`: `(r x 2h - h) x 0.001`, binary32.
// DISASM: cshell+0x176de1..0x176dfb (r x 2h - h, times the binary32 0.001 at cshell+0x3ce5cc)
pub fn offset(r: f32, h: i64) -> f32 {
    let h = h as f32;
    (r * (2.0 * h) - h) * 0.001f32
}

/// `normalize(fwd + o_r x right + o_u x up)`: square root of the sum of squares, reciprocal, three multiplies.
// EVIDENCE: cshell+0xb5ab0 (normalisation helper); SPEC: addendum AC-2.4
pub fn tilt(q: [f32; 4], o_r: f32, o_u: f32) -> [f32; 3] {
    let (right, up, fwd) = axes(q);
    let d = [0, 1, 2].map(|i| fwd[i] + o_r * right[i] + o_u * up[i]);
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !(l.is_finite() && l > 1e-12) {
        return fwd;
    }
    let inv = 1.0 / l;
    [d[0] * inv, d[1] * inv, d[2] * inv]
}

/// The pellet direction for the draws `(r1, r2)` and the half-width `h` (every pellet starts from the unperturbed
/// forward axis of `aim`).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-2.4, AC-3.4 item 1
pub fn pellet_dir(aim: [f32; 4], r1: f32, r2: f32, h: i64) -> [f32; 3] {
    tilt(aim, offset(r1, h), offset(r2, h))
}

/// What one shot's cone needs besides the counter and the table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeInputs {
    pub aim: [f32; 4],
    /// The weapon's current spread (both axes).
    pub spread: f32,
    pub scale: f32,
    pub zoom: bool,
    pub triple: bool,
}

/// One pellet: from the forward axis, two draws (right, then up) when the gate passes; the counter moves by two
/// then, by none otherwise.
// EVIDENCE: cshell+0x176cb0; DISASM: cshell+0x23bca0 (every pellet starts from the forward axis); SPEC: addendum AC-2,
// AC-3.4
pub fn pellet(row: &ConeRow, c: &ConeInputs, counter: &mut ShotCounter, t: &[f32]) -> [f32; 3] {
    let Some(h) = half_width(row, c.zoom, c.triple, c.scale, c.spread) else {
        return axes(c.aim).2;
    };
    let r1 = counter.draw(t);
    let o_r = offset(r1, h);
    let r2 = counter.draw(t);
    let o_u = offset(r2, h);
    tilt(c.aim, o_r, o_u)
}

// --------------------------------------------------------------------------------------------- the inverse

type V3 = [f64; 3];

/// Dot product in f64.
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Angle between two directions (radians, f64); pi for a zero or non-finite vector.
pub fn angle(a: [f32; 3], b: [f32; 3]) -> f64 {
    let (a, b) = (a.map(f64::from), b.map(f64::from));
    let la = dot(a, a).sqrt();
    let lb = dot(b, b).sqrt();
    if !(la > 0.0 && lb > 0.0 && la.is_finite() && lb.is_finite()) {
        return std::f64::consts::PI;
    }
    (dot(a, b) / (la * lb)).clamp(-1.0, 1.0).acos()
}

/// The half-width a claimed direction implies for the draws `(r1, r2)` (from the axis whose draw is further from the
/// middle, the better conditioned one), with the binary32 axes of `aim`; `None` for a direction not in front of the
/// aim.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-5 (method)
pub fn implied_h(aim: [f32; 4], dir: [f32; 3], r1: f32, r2: f32) -> Option<f64> {
    let (right, up, fwd) = axes(aim);
    let d = dir.map(f64::from);
    let df = dot(d, fwd.map(f64::from));
    if df.is_nan() || df <= 0.0 {
        return None;
    }
    let (a, b) = (
        dot(d, right.map(f64::from)) / df,
        dot(d, up.map(f64::from)) / df,
    );
    let (e1, e2) = (2.0 * f64::from(r1) - 1.0, 2.0 * f64::from(r2) - 1.0);
    let h = if e1.abs() >= e2.abs() {
        a / (0.001 * e1)
    } else {
        b / (0.001 * e2)
    };
    h.is_finite().then_some(h)
}

/// Right, up and forward axes of `q` computed in f64 (the replay's frame).
pub fn axes64(q: [f32; 4]) -> (V3, V3, V3) {
    let [x, y, z, w] = q.map(f64::from);
    (
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ],
        [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ],
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    )
}

/// The replay of one message (the method that reproduced 3807 of 3807 real-client pellets): `Some(h)` when the sent
/// `dir` is the forward axis of `aim` tilted by the two table draws of `stat` with one integer half-width `h`
/// (rounded from the implied value, in f64), within single-precision noise (2e-5 rad).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-5
pub fn replay(t: &[f32], aim: [f32; 4], dir: [f32; 3], stat: u32) -> Option<i64> {
    if t.is_empty() {
        return None;
    }
    let (right, up, fwd) = axes64(aim);
    let d = dir.map(f64::from);
    let dn = dot(d, d).sqrt();
    let df = dot(d, fwd);
    if df.is_nan() || df <= 0.0 {
        return None;
    }
    let (a, b) = (dot(d, right) / df, dot(d, up) / df);
    let (r1, r2) = draws_of_stat(t, stat);
    let (e1, e2) = (2.0 * f64::from(r1) - 1.0, 2.0 * f64::from(r2) - 1.0);
    let h = if e1.abs() >= e2.abs() {
        a / (0.001 * e1)
    } else {
        b / (0.001 * e2)
    };
    if !h.is_finite() || h < -0.5 {
        return None;
    }
    let h = h.round() as i64;
    let off = |r: f32| (f64::from(r) * 2.0 * h as f64 - h as f64) * 0.001;
    let (o1, o2) = (off(r1), off(r2));
    let p = [0, 1, 2].map(|i| fwd[i] + o1 * right[i] + o2 * up[i]);
    let cos = dot(p, d) / (dot(p, p).sqrt() * dn);
    (cos.clamp(-1.0, 1.0).acos() < 2e-5).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> ConeRow {
        ConeRow {
            min: 20,
            max: 90,
            zoomed: (2, 10),
            triple: (0, 40),
            ..ConeRow::default()
        }
    }

    /// Yaw about +Y, then pitch about +X (the bots' and dc-client's aim rotation).
    fn quat_yaw_pitch(yaw: f32, pitch: f32) -> [f32; 4] {
        let q = dc_sim::Quat::from_yaw_pitch(yaw, pitch);
        [q.x, q.y, q.z, q.w]
    }

    #[test]
    fn generator_has_the_2009_table_identity() {
        let t = generated_table();
        assert_eq!(t.len(), TABLE_LEN);
        assert!(t.iter().all(|v| (0.0..1.0).contains(v)));
        assert_eq!(table_sha256(&t), TABLE_SHA256);
        assert_eq!(shared_generated(), &t[..]);
        let s = ShotTable::generated();
        assert_eq!((s.values(), s.source), (&t[..], "generator"));
    }

    /// The ROM entry and the generator give the same table (compared by hash; no value is written down).
    #[test]
    fn rom_entry_equals_the_generator() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let t = ShotTable::from_rom(&RezVfs::new(&rom)).expect("ROM random table entry");
        assert_eq!((t.len(), t.source), (TABLE_LEN, "rom"));
        assert_eq!(table_sha256(&t), TABLE_SHA256);
        assert_eq!(&t[..], shared_generated());
        assert_eq!(ShotTable::open(Some(&rom)).source, "rom");
    }

    #[test]
    fn table_from_a_document_stops_at_the_first_gap() {
        let doc = ButeDoc::parse("[RandomTable]\nRandom0 = 0.25\nRandom1 = 0.5f\nRandom3 = 0.75\n");
        assert_eq!(table_from_doc(&doc), Some(vec![0.25, 0.5]));
        assert_eq!(
            table_from_doc(&ButeDoc::parse("[Other]\nRandom0 = 1\n")),
            None
        );
        assert_eq!(ShotTable::open(None).source, "generator");
    }

    #[test]
    fn reads_wrap_with_store_back_and_the_counter_agrees_with_draw_pair() {
        let t: Vec<f32> = (0..10).map(|i| i as f32 / 10.0).collect();
        assert_eq!(draw_pair(&t, 0), (0.0, 0.1, 2));
        // N - 1 reads N - 1, then N reduced to 0: the message carries 1
        assert_eq!(draw_pair(&t, 9), (0.9, 0.0, 1));
        // N reads 0 and 1 and carries 2
        assert_eq!(draw_pair(&t, 10), (0.0, 0.1, 2));
        assert_eq!(draw_pair(&t, 25).2, 7);
        for c in [0u32, 3, 9, 10, 25] {
            let (a, b, s) = draw_pair(&t, c);
            assert_eq!(draws_of_stat(&t, s), (a, b), "counter {c}");
            let mut k = ShotCounter { value: c };
            assert_eq!((k.draw(&t), k.draw(&t), k.value), (a, b, s));
        }
        let mut k = ShotCounter::default();
        k.set(5);
        k.add(3);
        assert_eq!(k.value, 8);
        assert_eq!(read(&[], &mut k.value), 0.5);
    }

    #[test]
    fn half_width_bounds_zoom_triple_and_gate() {
        let r = row();
        assert_eq!(half_width(&r, false, false, 1.0, 0.5), Some(55));
        assert_eq!(half_width(&r, true, false, 1.0, 0.5), Some(6));
        // triple: only the non-zero max replaces
        assert_eq!(half_width(&r, false, true, 1.0, 0.5), Some(30));
        assert_eq!(half_width(&r, false, false, 1.5, 0.5), Some(108));
        assert_eq!(
            half_width(&ConeRow::default(), false, false, 1.0, 1.0),
            None
        );
        assert_eq!(half_width_of(20, 90, 0.0, 1.0), HalfWidth::NoDraw);
        assert_eq!(
            half_width_of(20, 90, f32::INFINITY, 1.0),
            HalfWidth::Unknown
        );
        // AC-2.6: equal bounds, scale = the binary32 nearest 1.3: the product rounds up to an integer first
        assert_eq!(half_width_of(10, 10, 1.3, 0.0), HalfWidth::H(13));
        assert_eq!(pick(5, 7, 9, true, true), 7);
        assert_eq!(pick(5, 0, 9, true, true), 9);
        assert_eq!(pick(5, 0, 0, true, true), 5);
    }

    #[test]
    fn scale_pellets_and_range() {
        let mut r = row();
        r.scale = 0.0;
        let walk = MoveState::of_controls(dc_sim::player::bits::ANY_MOVE, false);
        assert_eq!(shot_scale(&r, &NO_MODS, MoveState::default()), 0.0);
        assert_eq!(shot_scale(&r, &NO_MODS, walk), 1.0);
        r.scale = 1.2;
        assert_eq!(shot_scale(&r, &NO_MODS, walk), 1.2);
        assert!(MoveState::of_controls(dc_sim::player::bits::RUN, true).airborne);
        assert_eq!(pellets(None, &NO_MODS), 1);
        assert_eq!(range(None, &NO_MODS), 3000.0);
    }

    #[test]
    fn pellets_replay_through_the_inverse() {
        let t = ShotTable::generated();
        let r = row();
        let q = quat_yaw_pitch(0.7, -0.2);
        let (right, up, fwd) = axes(q);
        let c = ConeInputs {
            aim: q,
            spread: 1.0,
            scale: 1.0,
            zoom: false,
            triple: false,
        };
        let mut k = ShotCounter { value: 9990 };
        for n in 0..40u32 {
            let before = k.value;
            let d = pellet(&r, &c, &mut k, &t);
            let (r1, r2, s) = draw_pair(&t, before);
            assert_eq!(k.value, s);
            assert_eq!(d, pellet_dir(q, r1, r2, 90));
            assert_eq!(replay(&t, q, d, k.value), Some(90), "{n}");
            let ih = implied_h(q, d, r1, r2).unwrap();
            assert!((ih - 90.0).abs() < 1e-2, "{ih}");
            let df = d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2];
            let a = (d[0] * right[0] + d[1] * right[1] + d[2] * right[2]) / df;
            let b = (d[0] * up[0] + d[1] * up[1] + d[2] * up[2]) / df;
            assert!(a.abs() <= 0.090 && b.abs() <= 0.090);
        }
        assert!(k.value < 9990, "the counter wrapped at the table size");
        // the forward axis replays as h 0; a gate refusal draws nothing
        assert_eq!(replay(&t, q, fwd, 40), Some(0));
        let before = k;
        assert_eq!(pellet(&ConeRow::default(), &c, &mut k, &t), fwd);
        assert_eq!(k, before);
        assert!(angle(fwd, fwd) < 1e-6);
    }
}

//! The 2009 shot cone and the server's shot-direction check (DSV-102).
//!
//! The 2009 client tilts every pellet away from the forward axis of its shot rotation by two draws of a fixed
//! 10000-entry table (the ROM attribute entry [`CONE_TABLE_ENTRY`]), indexed by a per-player counter that the
//! `UP_FIRE` 0x82 reports as `stat` after the pellet's two draws. The offsets form a square cone whose half-width is
//! one integer (thousandths of a tangent unit) computed from the weapon row's perturb bounds, the shot scale and the
//! spread fraction (`specs/WEAPON_FIRE_2009_addendum_cone.md` AC-1 .. AC-3). So the server can rebuild every pellet
//! direction from the message's own `aim`, `stat`, `frac`, `shot_scale` and zoom bit and compare it with the claimed
//! `dir` ([`check`]). The client stays authoritative on the counter (the server follows `stat`); the server checks
//! the direction only.
//!
//! The table is never part of the tree: [`rom_table`] reads it from the user's ROM at run time and
//! [`generated_table`] rebuilds it from its generator (AC-1.3) where no ROM entry is loaded. The table, the reads and
//! the cone arithmetic are `dc-cone`'s (shared with dc-client and the bots, re-exported here); this module holds the
//! server's side: the row bounds with absent keys, the verdicts, the per-link state and the counters.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1 (table), AC-2 (cone), AC-3 (fire loop), AC-5 (replay), AC-6 (server)
// DISASM: cshell+0x176cb0 (cone), cshell+0x2973a0 (table read), cshell+0x2974d0 (table load), cshell+0x23bca0 (fire loop)
// DC-DEFAULT: DSV-102 (proposed) the check, its verdicts and its modes

use crate::up::UpFire;

pub use dc_cone::{
    angle, axes, draw_pair, draws_of_stat, generated_table, implied_h, offset, pellet_dir,
    rom_table, shared_generated, table_from_doc, table_sha256, HalfWidth,
    TABLE_ENTRY as CONE_TABLE_ENTRY, TABLE_KEY as CONE_TABLE_KEY, TABLE_LEN, TABLE_SHA256,
    TABLE_TAG as CONE_TABLE_TAG,
};

/// Default largest angle between the claimed and the rebuilt direction of a `cone_ok` pellet.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-5 (worst replayed pellet 1.7e-7 rad; the replay accepts below 2e-5)
// DC-DEFAULT: DSV-102 (proposed) 2e-5 rad
pub const DEFAULT_TOLERANCE_RAD: f64 = 2e-5;
/// A pellet proves that its link draws from the table when it matched a rebuilt direction tilted at least this many
/// tolerances away from the forward axis (a client without the table matches such a point only by chance).
// DC-DEFAULT: DSV-102 (proposed) 10 x the tolerance
pub const CONFORM_TILT_FACTOR: f64 = 10.0;

/// The six perturb bounds of a weapon row (integers, thousandths). `None` = the key is absent from the row (its
/// binary default is not modelled: a shot that needs it is checked without bounds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConeBounds {
    pub min: i32,
    pub max: i32,
    pub zoomed_min: Option<i32>,
    pub zoomed_max: Option<i32>,
    pub triple_min: Option<i32>,
    pub triple_max: Option<i32>,
}

impl ConeBounds {
    /// The picked bounds `(lo, hi)` for the zoom and triple-shot states ([`dc_cone::pick`], each bound on its own),
    /// `None` when a key the state needs is absent.
    // DISASM: cshell+0x176ce1..0x176d6a (bounds)
    fn picked(&self, zoom: bool, triple: bool) -> Option<(i32, i32)> {
        let one = |plain: i32, zoomed: Option<i32>, tripled: Option<i32>| -> Option<i32> {
            let z = if zoom { zoomed? } else { 0 };
            let t = if triple && z == 0 { tripled? } else { 0 };
            Some(dc_cone::pick(plain, z, t, zoom, triple))
        };
        Some((
            one(self.min, self.zoomed_min, self.triple_min)?,
            one(self.max, self.zoomed_max, self.triple_max)?,
        ))
    }

    /// The scaled bounds `(lo, hi)` for the zoom and triple-shot states (a non-zero zoomed / triple-shot value
    /// replaces the plain one, each bound on its own), `None` when a needed key is absent.
    // DISASM: cshell+0x176ce1..0x176d6a (bounds), every product in binary32 (AC-2.6)
    pub fn scaled(&self, zoom: bool, triple: bool, scale: f32) -> Option<(f32, f32)> {
        let (lo, hi) = self.picked(zoom, triple)?;
        Some(dc_cone::scaled_bounds(lo, hi, scale))
    }

    /// The half-width for the spread fraction `f`: `trunc(trunc((hi - lo) f) scale + lo)`, in binary32.
    // DISASM: cshell+0x176d6a..0x176dc6 (gate lo <= hi and hi > 0; range product, scale, lo added scaled)
    pub fn half_width(&self, zoom: bool, triple: bool, scale: f32, f: f32) -> HalfWidth {
        match self.picked(zoom, triple) {
            Some((lo, hi)) => dc_cone::half_width_of(lo, hi, scale, f),
            None => HalfWidth::Unknown,
        }
    }

    /// The half-widths a message's spread byte allows (the spread lies in `[frac / 255, (frac + 1) / 255)`):
    /// `Some(Some((lo, hi)))`, `Some(None)` when the gate fails, `None` when a needed key is absent.
    pub fn h_range(
        &self,
        zoom: bool,
        triple: bool,
        scale: f32,
        frac: u8,
    ) -> Option<Option<(i64, i64)>> {
        let a = self.half_width(zoom, triple, scale, f32::from(frac) / 255.0);
        let b = self.half_width(zoom, triple, scale, (f32::from(frac) + 0.999) / 255.0);
        match (a, b) {
            (HalfWidth::H(x), HalfWidth::H(y)) => Some(Some((x.min(y), x.max(y)))),
            (HalfWidth::NoDraw, HalfWidth::NoDraw) => Some(None),
            _ => None,
        }
    }
}

/// The verdict of one pellet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConeVerdict {
    /// The claimed direction is the rebuilt one.
    Ok,
    /// The claimed direction is not the rebuilt one, on a link that has drawn from the table before.
    Off,
    /// The claimed direction is not the rebuilt one, on a link that never matched a tilted pellet (a client that does
    /// not draw from the table: DeCombat's bots and `dc-client` today).
    Na,
    /// Not checked (short message).
    Unchecked,
}

impl ConeVerdict {
    pub fn name(self) -> &'static str {
        match self {
            ConeVerdict::Ok => "cone_ok",
            ConeVerdict::Off => "cone_off",
            ConeVerdict::Na => "cone_na",
            ConeVerdict::Unchecked => "cone_unchecked",
        }
    }
}

/// The check's mode (`-combat cone_check=log|refuse|off`).
// DC-DEFAULT: DSV-102 (proposed, project decision pending on `refuse`) log
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConeMode {
    /// Every pellet is checked and counted, offending pellets are logged; nothing is refused.
    #[default]
    Log,
    /// As `Log`, and a `cone_off` pellet does no damage and releases no throw (its hit claim gets the verdict
    /// `cone_off`, its shot effect the miss form).
    Refuse,
    /// No check.
    Off,
}

impl ConeMode {
    pub fn parse(v: &str) -> Option<ConeMode> {
        match v.to_ascii_lowercase().as_str() {
            "log" => Some(ConeMode::Log),
            "refuse" => Some(ConeMode::Refuse),
            "off" => Some(ConeMode::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ConeMode::Log => "log",
            ConeMode::Refuse => "refuse",
            ConeMode::Off => "off",
        }
    }
}

/// The check's state of one player link: the shot counter as the server expects it and whether the link has shown
/// that it draws from the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinkCone {
    /// The counter before the next pellet (the client's `local_player + 0x1fc`).
    pub counter: u32,
    /// A pellet of this link matched a tilted rebuilt direction.
    pub conforming: bool,
    /// `cone_off` lines logged for this link.
    pub logged: u32,
    /// The `cone_na` note was logged for this link.
    pub na_noted: bool,
}

impl LinkCone {
    /// The counter the server just set with `GS_MATCH_SETTINGS` 0x7d `own_value` (the client stores it, `:=`).
    // SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.5 (0x7d own_value := ; 0xda kind 1 +=)
    pub fn set_seed(&mut self, seed: u32) {
        self.counter = seed;
    }
}

/// The result of [`check`] for one pellet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeCheck {
    pub verdict: ConeVerdict,
    /// Angle between the claimed and the best rebuilt direction (radians).
    pub error: f64,
    /// The half-width of the best rebuilt direction (`None`: the pellet keeps the forward axis by the row).
    pub h: Option<i64>,
    /// The row interval the half-width was taken from (`None`: no bounds were applied).
    pub bounds: Option<(i64, i64)>,
    /// The `stat` the server expected from its own counter.
    pub expected_stat: u32,
}

/// Checks one pellet against the cone: the draws of its `stat`, the half-width of the weapon row's bounds (`bounds`;
/// `None` without a ROM row: any non-negative integer half-width) for its zoom bit, `wflag`, `shot_scale` and spread
/// byte, then the angle between the rebuilt and the claimed `dir`. The link's counter follows the message's `stat`
/// (the client is authoritative on the count).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-6 items 2-4
// DC-DEFAULT: DSV-102 (proposed)
pub fn check(
    table: &[f32],
    bounds: Option<&ConeBounds>,
    link: &mut LinkCone,
    f: &UpFire,
    tolerance: f64,
) -> ConeCheck {
    let unchecked = ConeCheck {
        verdict: ConeVerdict::Unchecked,
        error: 0.0,
        h: None,
        bounds: None,
        expected_stat: link.counter,
    };
    if !f.complete || table.is_empty() {
        return unchecked;
    }
    let scale = f32::from_bits(f.shot_no);
    let (zoom, triple) = (f.moving, f.wflag);
    // the row's interval for this message: Some(Some(lo, hi)), Some(None) = no draw, None = no bounds
    let interval = bounds.and_then(|b| b.h_range(zoom, triple, scale, f.frac));
    let draws = !matches!(interval, Some(None));
    let expected_stat = if draws {
        draw_pair(table, link.counter).2
    } else {
        link.counter
    };
    link.counter = f.stat;
    let (r1, r2) = draws_of_stat(table, f.stat);
    let claimed = f.dir;
    let (_, _, fwd) = axes(f.aim);
    let finite = f.aim.iter().all(|c| c.is_finite()) && f.dir.iter().all(|c| c.is_finite());
    // candidate half-widths: the implied one, clamped into the row interval
    let implied = if finite {
        implied_h(f.aim, f.dir, r1, r2)
    } else {
        None
    };
    let mut cands: Vec<Option<i64>> = Vec::with_capacity(3);
    match interval {
        Some(None) => cands.push(None),
        Some(Some((lo, hi))) => {
            let x = implied.unwrap_or(lo as f64).clamp(lo as f64, hi as f64);
            cands.push(Some(x.floor() as i64));
            cands.push(Some(x.ceil() as i64));
        }
        None => {
            let x = implied.unwrap_or(0.0).clamp(0.0, 1e9);
            cands.push(Some(x.floor() as i64));
            cands.push(Some(x.ceil() as i64));
        }
    }
    let mut best = (f64::INFINITY, None, fwd);
    for h in cands {
        let p = match h {
            Some(h) => pellet_dir(f.aim, r1, r2, h),
            None => fwd,
        };
        let e = if finite {
            angle(p, claimed)
        } else {
            std::f64::consts::PI
        };
        if e < best.0 {
            best = (e, h, p);
        }
    }
    let (error, h, p) = best;
    let verdict = if error <= tolerance {
        if angle(p, fwd) > CONFORM_TILT_FACTOR * tolerance {
            link.conforming = true;
        }
        ConeVerdict::Ok
    } else if link.conforming {
        ConeVerdict::Off
    } else {
        ConeVerdict::Na
    };
    ConeCheck {
        verdict,
        error,
        h,
        bounds: interval.flatten(),
        expected_stat,
    }
}

/// The counters behind the `cone:` log line (DSV-102).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConeCounters {
    pub ok: u64,
    pub off: u64,
    pub na: u64,
    pub unchecked: u64,
    /// Pellets whose `stat` was not the one the server's counter expected (the counter then follows `stat`).
    pub stat_skew: u64,
    /// Pellets checked without row bounds.
    pub unbounded: u64,
    /// `cone_off` pellets refused (`refuse` mode).
    pub refused: u64,
    /// Largest error of a `cone_off` pellet.
    pub worst_off: f64,
}

impl ConeCounters {
    pub fn add(&mut self, c: &ConeCheck, f: &UpFire, refused: bool) {
        match c.verdict {
            ConeVerdict::Ok => self.ok += 1,
            ConeVerdict::Off => {
                self.off += 1;
                self.worst_off = self.worst_off.max(c.error);
            }
            ConeVerdict::Na => self.na += 1,
            ConeVerdict::Unchecked => {
                self.unchecked += 1;
                return;
            }
        }
        if c.expected_stat != f.stat {
            self.stat_skew += 1;
        }
        if c.bounds.is_none() && c.h.is_some() {
            self.unbounded += 1;
        }
        if refused {
            self.refused += 1;
        }
    }

    pub fn pellets(&self) -> u64 {
        self.ok + self.off + self.na + self.unchecked
    }

    /// `cone: 40 pellets, 38 ok, 1 off (worst 3.1e-2 rad, 0 refused), 1 na, 0 unchecked; 0 without row bounds;
    /// stat skew 0; mode log`.
    pub fn summary(&self, mode: ConeMode) -> String {
        if mode == ConeMode::Off {
            return "cone: check off".to_string();
        }
        format!(
            "cone: {} pellets, {} ok, {} off (worst {:.1e} rad, {} refused), {} na, {} unchecked; {} without row \
             bounds; stat skew {}; mode {}",
            self.pellets(),
            self.ok,
            self.off,
            self.worst_off,
            self.refused,
            self.na,
            self.unchecked,
            self.unbounded,
            self.stat_skew,
            mode.name()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_bute::ButeDoc;
    use dc_rez::RezVfs;

    fn quat_yaw_pitch(yaw: f32, pitch: f32) -> [f32; 4] {
        // yaw about +Y, then pitch about +X
        let (sy, cy) = (yaw * 0.5).sin_cos();
        let (sp, cp) = (pitch * 0.5).sin_cos();
        [cy * sp, sy * cp, -sy * sp, cy * cp]
    }

    fn fire(aim: [f32; 4], dir: [f32; 3], stat: u32, frac: u8, scale: f32) -> UpFire {
        UpFire {
            aim,
            dir,
            stat,
            frac,
            shot_no: scale.to_bits(),
            complete: true,
            ..UpFire::default()
        }
    }

    const B: ConeBounds = ConeBounds {
        min: 20,
        max: 120,
        zoomed_min: Some(2),
        zoomed_max: Some(10),
        triple_min: Some(0),
        triple_max: Some(0),
    };

    #[test]
    fn generator_has_the_2009_table_identity() {
        let t = generated_table();
        assert_eq!(t.len(), TABLE_LEN);
        assert!(t.iter().all(|v| (0.0..1.0).contains(v)));
        assert_eq!(table_sha256(&t), TABLE_SHA256);
        assert_eq!(shared_generated(), &t[..]);
    }

    /// The ROM entry and the generator give the same table (compared by hash; no value is written down).
    #[test]
    fn rom_entry_equals_the_generator() {
        let rom = dc_rez::dc_rom::rom_or_skip!();
        let t = rom_table(&RezVfs::new(&rom)).expect("ROM random table entry");
        assert_eq!(t.len(), TABLE_LEN);
        assert_eq!(table_sha256(&t), TABLE_SHA256);
        assert_eq!(table_sha256(&t), table_sha256(shared_generated()));
    }

    #[test]
    fn table_from_a_document_stops_at_the_first_gap() {
        let doc = ButeDoc::parse("[RandomTable]\nRandom0 = 0.25\nRandom1 = 0.5f\nRandom3 = 0.75\n");
        assert_eq!(table_from_doc(&doc), Some(vec![0.25, 0.5]));
        assert_eq!(
            table_from_doc(&ButeDoc::parse("[Other]\nRandom0 = 1\n")),
            None
        );
    }

    #[test]
    fn draws_wrap_at_the_table_size() {
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
        }
    }

    #[test]
    fn bounds_by_state_and_scale() {
        assert_eq!(B.half_width(false, false, 1.0, 0.0), HalfWidth::H(20));
        assert_eq!(B.half_width(false, false, 1.0, 1.0), HalfWidth::H(120));
        // lo 40, hi 240: trunc(200 x 0.5) x 2 + 40
        assert_eq!(B.half_width(false, false, 2.0, 0.5), HalfWidth::H(240));
        assert_eq!(B.half_width(true, false, 1.0, 0.0), HalfWidth::H(2));
        // triple-shot pair 0: the plain bounds
        assert_eq!(B.half_width(false, true, 1.0, 0.0), HalfWidth::H(20));
        // scale 0: the gate fails
        assert_eq!(B.half_width(false, false, 0.0, 0.5), HalfWidth::NoDraw);
        let no_zoom = ConeBounds {
            zoomed_min: None,
            ..B
        };
        assert_eq!(
            no_zoom.half_width(true, false, 1.0, 0.0),
            HalfWidth::Unknown
        );
        assert_eq!(B.h_range(false, false, 1.0, 255), Some(Some((120, 120))));
        assert_eq!(B.h_range(false, false, 1.0, 128), Some(Some((70, 70))));
        assert_eq!(B.h_range(false, false, 0.0, 128), Some(None));
    }

    #[test]
    fn a_rebuilt_pellet_passes_and_a_forged_one_is_off_after_the_link_conformed() {
        let t = shared_generated();
        let aim = quat_yaw_pitch(0.7, -0.2);
        let mut link = LinkCone::default();
        link.set_seed(40);
        // pellet 1: the client's own cone
        let (r1, r2, stat) = draw_pair(t, link.counter);
        let h = match B.half_width(false, false, 1.0, 64.0 / 255.0) {
            HalfWidth::H(h) => h,
            x => panic!("{x:?}"),
        };
        let dir = pellet_dir(aim, r1, r2, h);
        let c = check(
            t,
            Some(&B),
            &mut link,
            &fire(aim, dir, stat, 64, 1.0),
            DEFAULT_TOLERANCE_RAD,
        );
        assert_eq!(c.verdict, ConeVerdict::Ok, "{c:?}");
        assert_eq!((c.expected_stat, c.h), (42, Some(h)));
        assert!(link.conforming);
        // pellet 2: no spread (the forward axis)
        let (_, _, fwd) = axes(aim);
        let c = check(
            t,
            Some(&B),
            &mut link,
            &fire(aim, fwd, 44, 64, 1.0),
            DEFAULT_TOLERANCE_RAD,
        );
        assert_eq!(c.verdict, ConeVerdict::Off);
        assert!(c.error > 1e-3, "{c:?}");
        assert_eq!(c.expected_stat, 44);
        // pellet 3: the right cone but a half-width beyond the row's interval for the spread byte
        let (r1, r2, stat) = draw_pair(t, link.counter);
        let wide = pellet_dir(aim, r1, r2, 120);
        let c = check(
            t,
            Some(&B),
            &mut link,
            &fire(aim, wide, stat, 0, 1.0),
            DEFAULT_TOLERANCE_RAD,
        );
        assert_eq!(c.verdict, ConeVerdict::Off);
        assert_eq!(c.bounds, Some((20, 20)));
        // without row bounds the same pellet only has to follow the cone's shape
        let c = check(
            t,
            None,
            &mut link,
            &fire(aim, wide, stat + 2, 0, 1.0),
            DEFAULT_TOLERANCE_RAD,
        );
        assert_eq!(c.verdict, ConeVerdict::Off, "other draws");
        let (r1, r2, stat) = draw_pair(t, link.counter);
        let wide = pellet_dir(aim, r1, r2, 120);
        let c = check(
            t,
            None,
            &mut link,
            &fire(aim, wide, stat, 0, 1.0),
            DEFAULT_TOLERANCE_RAD,
        );
        assert_eq!(
            (c.verdict, c.h, c.bounds),
            (ConeVerdict::Ok, Some(120), None)
        );
    }

    #[test]
    fn a_link_that_never_drew_is_na_and_the_counter_follows_stat() {
        let t = shared_generated();
        let aim = quat_yaw_pitch(-1.0, 0.1);
        let (_, _, fwd) = axes(aim);
        let mut link = LinkCone::default();
        let c = check(
            t,
            Some(&B),
            &mut link,
            &fire(aim, fwd, 7, 0, 1.0),
            DEFAULT_TOLERANCE_RAD,
        );
        assert_eq!((c.verdict, c.expected_stat), (ConeVerdict::Na, 2));
        assert_eq!(link.counter, 7);
        assert!(!link.conforming);
        // the gate fails with scale 0: the forward axis is the cone's own direction and nothing is drawn
        let c = check(
            t,
            Some(&B),
            &mut link,
            &fire(aim, fwd, 7, 0, 0.0),
            DEFAULT_TOLERANCE_RAD,
        );
        assert_eq!(
            (c.verdict, c.expected_stat, c.h),
            (ConeVerdict::Ok, 7, None)
        );
        assert!(!link.conforming);
        let mut short = fire(aim, fwd, 9, 0, 1.0);
        short.complete = false;
        let c = check(t, Some(&B), &mut link, &short, DEFAULT_TOLERANCE_RAD);
        assert_eq!(c.verdict, ConeVerdict::Unchecked);
        assert_eq!(link.counter, 7);
    }

    #[test]
    fn summary_line() {
        let mut c = ConeCounters::default();
        let f = UpFire {
            stat: 2,
            ..UpFire::default()
        };
        let ok = ConeCheck {
            verdict: ConeVerdict::Ok,
            error: 0.0,
            h: Some(3),
            bounds: Some((3, 3)),
            expected_stat: 2,
        };
        c.add(&ok, &f, false);
        c.add(
            &ConeCheck {
                verdict: ConeVerdict::Off,
                error: 0.03,
                expected_stat: 4,
                ..ok
            },
            &f,
            true,
        );
        assert_eq!(
            c.summary(ConeMode::Refuse),
            "cone: 2 pellets, 1 ok, 1 off (worst 3.0e-2 rad, 1 refused), 0 na, 0 unchecked; 0 without row bounds; \
             stat skew 1; mode refuse"
        );
        assert_eq!(c.summary(ConeMode::Off), "cone: check off");
    }
}

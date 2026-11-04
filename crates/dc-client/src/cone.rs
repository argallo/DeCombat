//! The 2009 shot cone: the fixed random table, the shot counter, the square cone of one pellet, the shot scale, the
//! pellet count and the trace range of the fire routine.
//!
//! Every pellet starts from the aim rotation's forward axis and is tilted along the rotation's right and up axes by
//! two draws of the table, `(2r - 1) h / 1000` each, then normalised; `h` is one integer half-width (thousandths of a
//! tangent unit) from the weapon row's bounds, the shot scale and the current spread. The draws read the table at
//! the shot counter, which the server sets (0x7d `own_value`) and advances (0xda kind 1), and which goes out as the
//! 0x82 `stat` after the pellet's two draws. All arithmetic is binary32 like the 2009 client's x87 at 24-bit
//! precision.
//!
//! The table is read from the user's ROM at run time (`Attributes\randomtable.txt`, tag `RandomTable`, keys
//! `Random0` ... while present). Without a ROM the same values are rebuilt by their generator (the C runtime's classic
//! `rand()` from state 1, scaled by 1/32766, printed to six significant digits and parsed back); no table value is
//! stored in the tree.
//!
//! The implementation is the shared `dc-cone` crate (the same code the server's shot check and the bots use); this
//! module re-exports it under the client's names and adds the client's movement state.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1 (table), AC-2 (cone), AC-3 (fire routine), AC-7 (dc-client)
// EVIDENCE: cshell+0x176cb0 (cone), cshell+0x2973a0 (table read), cshell+0x17f7b0 (row binary defaults)
// DISASM: cshell+0x2974d0 (table load), cshell+0x23bca0 (fire routine: scale, pellets, range, zoom byte)

pub use dc_cone::{
    axes, half_width, key, offset, pellet, pellets, range, row_default, shot_scale, tilt,
    ConeInputs, ConeRow, ModRow, ModSlots, MoveState, ShotCounter, ShotTable, TABLE_ENTRY,
    TABLE_KEY, TABLE_LEN, TABLE_REZ, TABLE_TAG,
};

/// One own pellet: [`pellet`], or with the room's no-recoil (`steady`, LG-274) the cone's centre, the forward axis
/// of `c.aim`; the table's draws run either way, so the counter (the 0x82 `stat`) moves as without it.
// DC-DEFAULT: CLI-1627 (proposed, project decision) no spread in a no-recoil room
pub fn own_pellet(
    row: &ConeRow,
    c: &ConeInputs,
    counter: &mut ShotCounter,
    t: &[f32],
    steady: bool,
) -> [f32; 3] {
    let dir = pellet(row, c, counter, t);
    if steady {
        axes(c.aim).2
    } else {
        dir
    }
}

/// The movement state the fire routine picks its factor from, for the client's motion (control word, airborne
/// after a jump).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-3.2 (mover+0x80; control bits 0x800, 0x200)
pub fn move_state(m: &crate::spread::Motion) -> MoveState {
    MoveState::of_controls(m.controls, m.airborne)
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

    #[test]
    fn counter_draws_wrap_with_store_back() {
        let t = ShotTable::generated();
        assert_eq!(t.len(), TABLE_LEN);
        assert!(t.values().iter().all(|v| (0.0..1.0).contains(v)));
        let mut c = ShotCounter::default();
        c.set(TABLE_LEN as u32 - 1);
        let a = c.draw(&t);
        let b = c.draw(&t);
        assert_eq!(a.to_bits(), t[TABLE_LEN - 1].to_bits());
        assert_eq!(b.to_bits(), t[0].to_bits());
        assert_eq!(
            c.value, 1,
            "the reduced value is stored back, then incremented"
        );
        c.set(TABLE_LEN as u32);
        c.draw(&t);
        c.draw(&t);
        assert_eq!(c.value, 2);
        c.add(5);
        assert_eq!(c.value, 7);
    }

    #[test]
    fn half_width_bounds_zoom_and_triple() {
        let r = row();
        // plain: trunc(trunc(70 x 0.5) x 1 + 20) = 55
        assert_eq!(half_width(&r, false, false, 1.0, 0.5), Some(55));
        // zoomed pair replaces both bounds: trunc(trunc(8 x 0.5) + 2) = 6
        assert_eq!(half_width(&r, true, false, 1.0, 0.5), Some(6));
        // triple: only the non-zero max replaces: trunc(trunc(20 x 0.5) + 20) = 30
        assert_eq!(half_width(&r, false, true, 1.0, 0.5), Some(30));
        // the scale multiplies both bounds and the truncated range product
        // lo = trunc(20 x 1.5) = 30, hi = trunc(90 x 1.5) = 135, trunc(trunc(105 x 0.5) x 1.5 + 30) = trunc(52 x 1.5 + 30)
        assert_eq!(half_width(&r, false, false, 1.5, 0.5), Some(108));
        // the gate: no positive max, nothing drawn
        assert_eq!(
            half_width(&ConeRow::default(), false, false, 1.0, 1.0),
            None
        );
        assert_eq!(half_width(&r, false, false, 0.0, 1.0), None);
    }

    #[test]
    fn binary32_rounding_before_the_truncation() {
        // the AC-2.6 case: equal bounds, scale = the binary32 nearest 1.3; the product rounds up to an integer first
        let s = 1.3f32;
        let r = ConeRow {
            min: 10,
            max: 10,
            ..ConeRow::default()
        };
        assert_eq!(half_width(&r, false, false, s, 0.0), Some(13));
        assert_eq!((10.0f64 * f64::from(s)).trunc(), 12.0);
    }

    #[test]
    fn scale_is_raised_to_the_movement_factor() {
        let mut r = row();
        r.scale = 0.0;
        let none: ModSlots = [None; 3];
        assert_eq!(shot_scale(&r, &none, MoveState::default()), 0.0);
        let walk = MoveState {
            any_move: true,
            ..MoveState::default()
        };
        assert_eq!(shot_scale(&r, &none, walk), 1.0);
        r.scale = 1.2;
        assert_eq!(
            shot_scale(&r, &none, walk),
            1.2,
            "only raised, never lowered"
        );
        r.scale_jump = 2.0;
        let jump = MoveState {
            airborne: true,
            any_move: true,
            run: true,
        };
        assert_eq!(shot_scale(&r, &none, jump), 2.0);
        r.scale_run = 0.0;
        let run = MoveState {
            run: true,
            ..MoveState::default()
        };
        assert_eq!(
            shot_scale(&r, &none, run),
            1.2,
            "a zero factor does not apply"
        );
        let m = Some(ModRow {
            scale: 0.25,
            ..ModRow::default()
        });
        assert_eq!(shot_scale(&r, &[m, None, m], MoveState::default()), 1.7);
    }

    #[test]
    fn pellets_and_range_with_mods() {
        let none: ModSlots = [None; 3];
        assert_eq!(pellets(None, &none), 1);
        assert_eq!(pellets(Some(8), &none), 8);
        let m = ModRow {
            pellets_per: 0.3,
            range_per: 0.5,
            ..ModRow::default()
        };
        assert_eq!(pellets(Some(8), &[None, None, Some(m)]), 10);
        assert_eq!(pellets(Some(8), &[Some(m), None, None]), 8, "slot 2 only");
        assert_eq!(range(None, &none), 3000.0);
        assert_eq!(range(Some(5000.0), &[Some(m), None, None]), 7500.0);
        assert_eq!(
            range(Some(5000.0), &[None, Some(m), None]),
            5000.0,
            "slot 0 only"
        );
    }

    #[test]
    fn pellet_is_square_around_the_forward_axis() {
        let t = ShotTable::generated();
        let r = row();
        let q = dc_botclient::shooter::aim_quat(0.7, -0.2);
        let (right, up, fwd) = axes(q);
        let f = dc_botclient::shooter::forward(0.7, -0.2);
        for i in 0..3 {
            assert!((fwd[i] - f[i]).abs() < 1e-5, "forward axis");
        }
        let c = ConeInputs {
            aim: q,
            spread: 1.0,
            scale: 1.0,
            zoom: false,
            triple: false,
        };
        let mut k = ShotCounter::default();
        for n in 0..200u32 {
            let before = k.value;
            let d = pellet(&r, &c, &mut k, &t);
            assert_eq!(k.value, before + 2);
            let df = d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2];
            let a = (d[0] * right[0] + d[1] * right[1] + d[2] * right[2]) / df;
            let b = (d[0] * up[0] + d[1] * up[1] + d[2] * up[2]) / df;
            let want_a = offset(t[(2 * n) as usize], 90);
            let want_b = offset(t[(2 * n + 1) as usize], 90);
            assert!(
                (a - want_a).abs() < 1e-5 && (b - want_b).abs() < 1e-5,
                "{n}"
            );
            assert!(a.abs() <= 0.090 && b.abs() <= 0.090);
        }
        // a refused gate: the forward axis, no draw
        let before = k;
        let d = pellet(&ConeRow::default(), &c, &mut k, &t);
        assert_eq!(k, before);
        assert_eq!(d, fwd);
    }

    /// CLI-1627: a no-recoil room's own pellet is the cone's centre, and the table's draws still move the counter.
    #[test]
    fn a_steady_own_pellet_is_the_forward_axis_and_still_counts() {
        let t = ShotTable::generated();
        let q = dc_botclient::shooter::aim_quat(0.3, 0.1);
        let fwd = axes(q).2;
        let c = ConeInputs {
            aim: q,
            spread: 1.0,
            scale: 1.0,
            zoom: false,
            triple: false,
        };
        let mut k = ShotCounter::default();
        let mut j = ShotCounter::default();
        for _ in 0..20 {
            assert_eq!(own_pellet(&row(), &c, &mut k, &t, true), fwd);
            assert_eq!(own_pellet(&row(), &c, &mut j, &t, false), pellet(&row(), &c, &mut ShotCounter { value: j.value - 2 }, &t));
        }
        assert_eq!((k.value, j.value), (40, 40));
        assert_ne!(own_pellet(&row(), &c, &mut j, &t, false), fwd, "the cone spreads without it");
    }
}

//! The engine integrator: gravity, ground support and exponential friction over one physics step.
// SPEC: MOVEMENT_2009.md §8.3 (engine+0x6fe50)

use crate::fmath;
use crate::vec3::V3;

/// Inputs of one integration step that are not the body's velocity and acceleration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntegrateEnv {
    /// World force `F` (the gravity record's force during the player's step: `(0, g', 0)`).
    // SPEC: MOVEMENT_2009.md §8.1 step 3, §8.2
    pub force: V3,
    /// The body's gravity flag (engine object flag 0x20000).
    pub gravity: bool,
    /// Plane normal of the standing contact, when the body stands on something.
    pub standing: Option<V3>,
    /// The gravity record's standable value (negative: -0.7071 by default).
    // SPEC: MOVEMENT_2009.md §8.2 (standable = -0.7071)
    pub standable: f32,
    /// Friction coefficient `k` (object +0x130).
    pub friction: f32,
    /// Dead zone on `|v|^2` and `|a|^2`.
    pub dead_zone_sq: f32,
}

/// Which branch the step took (for tests and diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Branch {
    Rest,
    Free,
    Falling,
    Steep,
    Leaving,
    Supported,
    SupportedFriction,
}

/// Result of [`integrate`]: the displacement for this step and the branch taken. `v` and `a` are updated in
/// place (the engine writes the acceleration back, so after a falling or steep step it includes `F`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Integrated {
    pub out: V3,
    pub branch: Branch,
}

/// `PLAIN(a)`: `out = v dt + a (dt^2 / 4)`, `v += a dt`. The quarter is the 2009 behaviour (the code halves
/// `0.5 dt^2` again before use).
// SPEC: MOVEMENT_2009.md §8.3 (PLAIN, dt^2/4, constant at VA 0x672918), §15 item 6
fn plain(v: &mut V3, a: V3, dt: f32) -> V3 {
    let half_dt2 = 0.5 * dt * dt;
    let quarter = half_dt2 * 0.5;
    let out = *v * dt + a * quarter;
    *v += a * dt;
    out
}

/// `FRICTION(a, k)`: the exact solution of `dv/dt = a - k v` over `dt`.
// SPEC: MOVEMENT_2009.md §8.3 (FRICTION: E = exp(-k dt); out = (a/k) dt + (1 - E)(1/k)^2 (k v - a);
// v = a/k + (v - a/k) E)
fn friction(v: &mut V3, a: V3, k: f32, dt: f32) -> V3 {
    let e = fmath::exp(-k * dt);
    let inv_k = 1.0 / k;
    let a_k = a * inv_k;
    let out = a_k * dt + (*v * k - a) * ((1.0 - e) * (inv_k * inv_k));
    *v = a_k + (*v - a_k) * e;
    out
}

/// One integration step. Mirrors the engine's branch structure exactly; see the spec's pseudo-code.
// SPEC: MOVEMENT_2009.md §8.3
pub fn integrate(v: &mut V3, a: &mut V3, env: &IntegrateEnv, dt: f32) -> Integrated {
    // dead zones, both tested on the input values
    if v.len_sq() < env.dead_zone_sq {
        *v = V3::ZERO;
    }
    if a.len_sq() < env.dead_zone_sq {
        *a = V3::ZERO;
    }
    let rest = |v: &mut V3, a: &mut V3| {
        *v = V3::ZERO;
        *a = V3::ZERO;
        Integrated {
            out: V3::ZERO,
            branch: Branch::Rest,
        }
    };
    if !env.gravity {
        if v.is_zero() && a.is_zero() {
            return rest(v, a);
        }
        let out = plain(v, *a, dt);
        return Integrated {
            out,
            branch: Branch::Free,
        };
    }
    let f = env.force;
    let Some(standing) = env.standing else {
        *a += f;
        let out = plain(v, *a, dt);
        return Integrated {
            out,
            branch: Branch::Falling,
        };
    };
    if v.is_zero() && a.is_zero() {
        return rest(v, a);
    }
    // standing polygon normal (a contact without a polygon would use -dir(F); every contact here has one)
    let n = standing;
    let big_a = *a + f;
    let fd = f.with_len(1.0);
    if fd.dot_xzy(n) > env.standable {
        // steep: normal more than 45 degrees away from "up"
        let m = big_a.len();
        let mut na = big_a - n * big_a.dot_xzy(n);
        let e = na.dot_xzy(fd);
        if e < 0.0 {
            na -= fd * e;
        }
        na = na.with_len(m);
        *a = na;
        let out = plain(v, na, dt);
        return Integrated {
            out,
            branch: Branch::Steep,
        };
    }
    if (*v + f * dt).dot(n) > 0.01 {
        // leaving the surface
        *a = big_a;
        let out = plain(v, big_a, dt);
        return Integrated {
            out,
            branch: Branch::Leaving,
        };
    }
    // supported by the surface, gravity cancelled
    let an = a.dot(n);
    if an > 0.01 {
        let m = a.len();
        *a = (*a - n * (an + 1.0)).with_len(m);
    }
    if env.friction > 0.0 {
        let out = friction(v, *a, env.friction, dt);
        Integrated {
            out,
            branch: Branch::SupportedFriction,
        }
    } else {
        let out = plain(v, *a, dt);
        Integrated {
            out,
            branch: Branch::Supported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(standing: Option<V3>, k: f32) -> IntegrateEnv {
        IntegrateEnv {
            force: V3::new(0.0, -1100.0, 0.0),
            gravity: true,
            standing,
            standable: -crate::params::MovementParams::DEFAULT.standable_cos,
            friction: k,
            dead_zone_sq: 0.1,
        }
    }

    #[test]
    fn free_fall_uses_quarter_dt_squared() {
        let mut v = V3::new(0.0, 330.0, 0.0);
        let mut a = V3::ZERO;
        let dt = 0.1;
        let r = integrate(&mut v, &mut a, &env(None, 5.0), dt);
        assert_eq!(r.branch, Branch::Falling);
        // 330 * 0.1 - 1100 * 0.01 / 4 = 33 - 2.75
        assert!((r.out.y - 30.25).abs() < 1e-4, "{:?}", r.out);
        assert!((v.y - 220.0).abs() < 1e-3);
        assert_eq!(
            a,
            V3::new(0.0, -1100.0, 0.0),
            "acceleration written back incl. F"
        );
    }

    #[test]
    fn dead_zones_and_rest() {
        let mut v = V3::new(0.2, 0.0, 0.2); // |v|^2 = 0.08 < 0.1
        let mut a = V3::new(0.3, 0.0, 0.0); // 0.09 < 0.1
        let r = integrate(&mut v, &mut a, &env(Some(V3::UP), 5.0), 1.0 / 60.0);
        assert_eq!(r.branch, Branch::Rest);
        assert_eq!((v, a, r.out), (V3::ZERO, V3::ZERO, V3::ZERO));
    }

    #[test]
    fn friction_terminal_speed_is_a_over_k() {
        // supported on flat ground, constant 3000 forward: terminal |a| / k = 600
        let mut v = V3::ZERO;
        for _ in 0..2000 {
            let mut a = V3::new(0.0, 0.0, 3000.0);
            integrate(&mut v, &mut a, &env(Some(V3::UP), 5.0), 0.01);
        }
        assert!((v.z - 600.0).abs() < 0.01, "{v:?}");
        assert_eq!(v.y, 0.0, "gravity cancelled while supported");
    }

    #[test]
    fn friction_decays_as_exp_minus_5t() {
        // no input: v(t) = v0 exp(-5 t)
        let mut v = V3::new(285.0, 0.0, 0.0);
        let mut total = 0.0f32;
        for _ in 0..20 {
            let mut a = V3::ZERO;
            let r = integrate(&mut v, &mut a, &env(Some(V3::UP), 5.0), 0.01);
            assert_eq!(r.branch, Branch::SupportedFriction);
            total += r.out.x;
        }
        let want_v = 285.0 * (-1.0f64).exp(); // t = 0.2 s, k t = 1
        assert!(((v.x as f64) - want_v).abs() < 0.01, "{v:?} vs {want_v}");
        let want_x = 285.0 / 5.0 * (1.0 - (-1.0f64).exp());
        assert!(
            ((total as f64) - want_x).abs() < 0.01,
            "{total} vs {want_x}"
        );
    }

    #[test]
    fn steep_contact_slides_down_without_climbing() {
        // 60 degree slope: n.y = 0.5 < 0.7071 -> steep branch
        let n = V3::new(0.0, 0.5, 0.866_025_4);
        // at rest (v = a = 0) the standing case rests even on a steep contact; a landing body is moving
        let mut v0 = V3::ZERO;
        let mut a0 = V3::ZERO;
        assert_eq!(
            integrate(&mut v0, &mut a0, &env(Some(n), 5.0), 0.1).branch,
            Branch::Rest
        );
        let mut v = V3::new(0.0, -10.0, 0.0);
        let mut a = V3::ZERO;
        let r = integrate(&mut v, &mut a, &env(Some(n), 5.0), 0.1);
        assert_eq!(r.branch, Branch::Steep);
        // acceleration lies in the slope plane, points downhill, magnitude |A| = 1100
        assert!(a.dot(n).abs() < 1e-2, "{a:?}");
        assert!(a.y < 0.0 && a.z > 0.0);
        assert!((a.len() - 1100.0).abs() < 0.1);
    }

    #[test]
    fn leaving_the_surface_takes_full_gravity() {
        let mut v = V3::new(0.0, 330.0, 0.0);
        let mut a = V3::ZERO;
        let r = integrate(&mut v, &mut a, &env(Some(V3::UP), 5.0), 1.0 / 60.0);
        assert_eq!(r.branch, Branch::Leaving);
        assert!((v.y - (330.0 - 1100.0 / 60.0)).abs() < 1e-3);
    }

    #[test]
    fn into_surface_part_removed_plus_one() {
        let n = V3::UP;
        let mut v = V3::new(10.0, 0.0, 0.0);
        let mut a = V3::new(0.0, 100.0, 0.0); // pushes away from the surface: dot(a, n) > 0.01
        let r = integrate(&mut v, &mut a, &env(Some(n), 0.0), 0.01);
        assert_eq!(r.branch, Branch::Supported);
        // a - n (100 + 1) = (0, -1, 0), rescaled to |a| = 100
        assert!((a.y + 100.0).abs() < 1e-3, "{a:?}");
    }
}

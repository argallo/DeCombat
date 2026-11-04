//! ROM-gated (skipped without CA_ROM), CLI-1136: every launcher round (ammunition `Type` 1 with a projectile row)
//! names an effect group in its projectile row that the effect file holds; on a 10 s flight at 1300 units/s (the
//! server's DSV-181 flight) the group rides on the object (a looping one to the end of the flight, another one for
//! its own length) and the object's removal ends it.
//! Only counts are asserted; no ROM name or value is printed.
// EVIDENCE: cshell+0x2cab60, cshell+0x292f90

use dc_client::fx::{EffectLibrary, FxEvent, FxRunner, FxTables, TextureStore};

#[test]
fn rom_launcher_rounds_carry_their_effect_for_the_whole_flight() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("skipped: CA_ROM not set");
        return;
    };
    if !rom.is_dir() {
        return;
    }
    let tables = FxTables::open(&rom);
    let library = EffectLibrary::open(&rom);
    let launchers: Vec<u16> = tables
        .ammo
        .iter()
        .filter(|(_, a)| a.ammo_type == 1)
        .map(|(&id, _)| id)
        .filter(|&id| tables.projectile(id).is_some_and(|p| p.fx.is_some()))
        .collect();
    assert!(
        !launchers.is_empty(),
        "no launcher round with a projectile effect"
    );
    for &ammo in &launchers {
        let row = tables.projectile(ammo).unwrap().clone();
        let group = library
            .get(row.fx.as_deref().unwrap())
            .unwrap_or_else(|| panic!("ammo {ammo}: group missing"));
        // a looping group lasts the whole flight, another one its own length
        let lasts_ms = if row.fx_loop {
            10_000
        } else {
            (group.length_s * 1000.0) as u32
        };
        let mut fx = FxRunner::new(tables.clone(), library.clone(), TextureStore::default());
        // a 10 s flight at 1300 units/s along +x, one frame each 50 ms
        let mut t = 0u32;
        while t <= 10_000 {
            let pos = [1.3 * t as f32, 100.0, 0.0];
            fx.feed(
                t,
                &[FxEvent::Thrown {
                    obj: 77,
                    ammo,
                    pos,
                    rot: [0.0, 0.0, 0.0, 1.0],
                }],
                None,
            );
            if t > 0 && t % 1000 == 0 && t < lasts_ms {
                assert!(
                    fx.live_counts().0 > 0,
                    "ammo {ammo}: the effect ended at {t} ms (loop {})",
                    row.fx_loop
                );
            }
            t += 50;
        }
        fx.feed(t, &[FxEvent::WorldGone(77)], None);
        if !row.fx_smooth {
            assert_eq!(fx.live_counts().0, 0, "ammo {ammo}: the removal cuts it");
        }
    }
}

/// CLI-1136 (amended): a launched round without a model (the flamethrower's flame: its projectile row names an
/// effect group and no model) starts its group in the object's rotation, so the flame's jet keys fire the way the
/// shot went; a turned object gives a turned group. Counts and rotations only.
// EVIDENCE: cshell+0x2cab60 (the group is created on the object)
#[test]
fn rom_a_flame_round_starts_its_group_in_the_objects_rotation() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("skipped: CA_ROM not set");
        return;
    };
    if !rom.is_dir() {
        return;
    }
    let tables = FxTables::open(&rom);
    let library = EffectLibrary::open(&rom);
    let flames: Vec<u16> = tables
        .ammo
        .iter()
        .filter(|(_, a)| a.ammo_type == 1)
        .map(|(&id, _)| id)
        .filter(|&id| {
            tables
                .projectile(id)
                .is_some_and(|p| p.fx.is_some() && p.model.is_none())
        })
        .collect();
    assert!(!flames.is_empty(), "no launched round without a model");
    // a shot turned 90 degrees about +Y (yaw) and pitched down a little
    let (sy, cy) = (std::f32::consts::FRAC_PI_4).sin_cos();
    let (sp, cp) = (0.1f32).sin_cos();
    let rot = [cy * sp, sy * cp, -sy * sp, cy * cp];
    for &ammo in &flames {
        let mut fx = FxRunner::new(tables.clone(), library.clone(), TextureStore::default());
        fx.feed(
            1000,
            &[FxEvent::Thrown {
                obj: 9,
                ammo,
                pos: [0.0, 50.0, 0.0],
                rot,
            }],
            None,
        );
        let frames = fx.live_frames();
        assert!(!frames.is_empty(), "ammo {ammo}: the group runs");
        for (_, r) in frames {
            let dot: f32 = (0..4).map(|i| r[i] * rot[i]).sum();
            assert!(dot.abs() > 0.9999, "ammo {ammo}: group rotation {r:?}, object {rot:?}");
        }
    }
}

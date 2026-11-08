//! ROM-gated (skipped without CA_ROM), CLI-1925: the impact group of a lingering-cloud round (an ammunition row whose
//! impact effect holds particles living five seconds or more: the smoke grenade's) keeps its cloud
//! where it burst for the particles' whole life. Before the fix the key's friction was read as the velocity fraction
//! kept, so the burst flew thousands of units out within its first seconds ("the smoke immediately dissipates").
//! CLI-1930: the thrown rounds with a lingering hazard (progressive damage, no blast, a lifetime of five seconds or more:
//! the incendiary and gas grenades, DSV-635) are among them: the fire or gas the server's blast record starts stands
//! where it went off.
//! Only counts and distances are asserted; no ROM name or value is printed.
// EVIDENCE: clientfx+0x1f3a0 (per-frame particle step, velocity x (1 - friction)^dt), clientfx+0x1d530
// OBSERVED: ROM table keys WEAPONS.TXT [Ammo<n>] ProgDamage, ProgDamageLifetime, ImpactFXName (read at run time)

use dc_client::fx::{EffectLibrary, FxRunner, FxTables, KeyKind, TextureStore};
use glam::{Quat, Vec3};
use std::collections::BTreeMap;

fn rom() -> Option<std::path::PathBuf> {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("skipped: CA_ROM not set");
        return None;
    };
    rom.is_dir().then_some(rom)
}

/// The impact groups holding particles living at least 5 s, by ammunition row: (group, shortest life, particles of
/// one emission).
fn clouds(tables: &FxTables, library: &EffectLibrary) -> BTreeMap<u16, (String, f32, u32)> {
    let mut out = BTreeMap::new();
    for &ammo in tables.ammo.keys() {
        let Some(name) = tables.impact(ammo).and_then(|r| r.fx.clone()) else {
            continue;
        };
        let Some(g) = library.get(&name) else {
            continue;
        };
        for k in &g.keys {
            if let KeyKind::Particles(p) = &k.kind {
                if p.life.0 >= 5.0 && p.per_emission > 0 {
                    assert!(p.keep < 1.0, "a lingering cloud is dragged");
                    out.entry(ammo)
                        .or_insert((name.clone(), p.life.0, p.per_emission));
                }
            }
        }
    }
    out
}

/// Most of the way through the shortest life every particle of the burst is still drawn, near the burst.
fn assert_lingers(tables: &FxTables, library: &EffectLibrary, name: &str, life: f32, count: u32) {
    let cam = dc_render::Camera::looking(Vec3::new(0.0, 0.0, -3000.0), 0.0, 0.0, 1.5, 1.2);
    let mut r = FxRunner::new(tables.clone(), library.clone(), TextureStore::default());
    assert!(r.start_group(name, Vec3::ZERO, Quat::IDENTITY, false, 0.0));
    let ms = (life * 900.0) as u32;
    for t in (0..=ms).step_by(100) {
        r.feed(t, &[], None);
    }
    let d = r.draw(ms, &cam, None);
    let far = d
        .world
        .iter()
        .map(|q| q.centre.length())
        .fold(0.0f32, f32::max);
    assert!(
        d.world.len() >= count as usize,
        "{} quads of a {count}-particle burst at {ms} ms",
        d.world.len()
    );
    assert!(
        far < 1000.0,
        "a particle {far:.0} units from the burst at {ms} ms"
    );
}

#[test]
fn rom_a_smoke_cloud_lingers_where_it_burst() {
    let Some(rom) = rom() else {
        return;
    };
    let tables = FxTables::open(&rom);
    let library = EffectLibrary::open(&rom);
    let found = clouds(&tables, &library);
    assert!(!found.is_empty(), "no lingering-cloud impact group");
    for (name, life, count) in found.values() {
        assert_lingers(&tables, &library, name, *life, *count);
    }
}

#[test]
fn rom_the_fire_and_gas_grenades_leave_a_lingering_cloud() {
    let Some(rom) = rom() else {
        return;
    };
    let tables = FxTables::open(&rom);
    let library = EffectLibrary::open(&rom);
    let found = clouds(&tables, &library);
    let vfs = dc_rez::RezVfs::new(&rom);
    let doc = dc_bute::ButeDoc::from_vfs(
        &vfs,
        dc_client::arms::ATTRIBUTES_REZ,
        dc_client::arms::WEAPONS_ENTRY,
    )
    .expect("the weapon table");
    let num = |sec: &dc_bute::SectionMap, k: &str| {
        sec.get(k)
            .and_then(|v| v.trim().trim_matches('"').parse::<f32>().ok())
            .unwrap_or(0.0)
    };
    let mut hazards = 0;
    for (tag, sec) in doc.iter() {
        let Some(ammo) = tag
            .strip_prefix("Ammo")
            .and_then(|n| n.parse::<u16>().ok())
        else {
            continue;
        };
        // the server's hazard rows: progressive damage and no blast (DSV-455 / DSV-635)
        let blast = num(sec, "AreaDamage") > 0.0 && num(sec, "AreaDamageRadius") > 0.0;
        if blast || !(num(sec, "ProgDamage") > 0.0 && num(sec, "ProgDamageLifetime") >= 5.0) {
            continue;
        }
        // (a hazard row whose impact effect is a short burst, such as a shell's, has no cloud to test)
        let Some((name, life, count)) = found.get(&ammo) else {
            continue;
        };
        hazards += 1;
        assert_lingers(&tables, &library, name, *life, *count);
    }
    assert!(hazards >= 2, "the fire and gas grenade rows: {hazards}");
}

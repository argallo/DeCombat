//! ROM-gated measurement (fix-first-match-drop): what the sound director costs the frame thread when remote players
//! appear for the first time in a process, in the INLINE mode the menu's carried director used before this branch:
//! the footstep rig of each distinct body model (body model read plus its animation set) and the first decode of each
//! weapon sound. Prints the times (the window now reads the rigs on the director's rig thread, CLI-1700).
//! Skipped without `CA_ROM`. Nothing of the ROM is asserted by value.
// ORIGINAL: our own measurement harness; DC-DEFAULT: CLI-1700 (proposed, project decision), see DC_DEFAULTS.md

use std::sync::Arc;
use std::time::Instant;

use dc_client::sndcache::PcmCache;
use dc_client::sound::SoundDirector;
use dc_render::{CharacterTable, ModelFiles};
use dc_rez::RezVfs;

#[test]
fn rom_first_sight_costs_on_the_frame_thread() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let t = Instant::now();
    let director = SoundDirector::open(Some(&rom));
    println!("sound director open: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);

    // footstep rigs: every distinct body model of the character table, fresh archives (process-cold)
    let vfs = RezVfs::new(&rom);
    let table = CharacterTable::from_vfs(&vfs).expect("character table");
    let mut bodies: Vec<String> = table.bodies.values().map(|b| b.model.clone()).collect();
    bodies.sort();
    bodies.dedup();
    let mut rig_ms = Vec::new();
    for m in bodies.iter().take(7) {
        let t = Instant::now();
        let f = ModelFiles::load(&vfs, m);
        let _ = f.map(|f| f.anim_set());
        rig_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let t = Instant::now();
    for m in bodies.iter().take(7) {
        let _ = ModelFiles::load(&vfs, m).map(|f| f.anim_set());
    }
    let rig_again = t.elapsed().as_secs_f64() * 1e3;
    println!(
        "footstep rigs (body model + animation set), {} distinct bodies: first {:?} ms, sum {:.0} ms; again {:.0} ms",
        rig_ms.len(),
        rig_ms.iter().map(|v| v.round() as u32).collect::<Vec<_>>(),
        rig_ms.iter().sum::<f64>(),
        rig_again
    );

    // weapon sounds: every sound of 7 weapons, first decode (inline, as the frame thread does it)
    let vfs = Arc::new(RezVfs::new(&rom));
    let mut cache = PcmCache::new(256 << 20);
    let mut files = 0;
    let t = Instant::now();
    let mut worst = 0.0f64;
    for w in director.weapons.values().filter(|w| !w.files.is_empty()).take(7) {
        for f in w.files.iter().filter(|f| !f.is_empty()) {
            let t1 = Instant::now();
            let _ = cache.get(Some(&vfs), f);
            worst = worst.max(t1.elapsed().as_secs_f64() * 1e3);
            files += 1;
        }
    }
    println!(
        "weapon sounds of 7 weapons: {files} files, first decode {:.0} ms total, worst {worst:.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
}

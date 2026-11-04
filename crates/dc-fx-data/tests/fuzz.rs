//! Mutation fuzz: 10,000 mutated buffers per parser. A parser may return Ok or Err but must never panic,
//! hang or allocate without bound. Deterministic seeds.

mod common;
use common::*;
use dc_fx_data::fcf::FcfFile;
use dc_fx_data::fxf::{Encoding, FxfFile};
use dc_fx_data::tfg::TextureGroup;
use dc_fx_data::tfs::{self, TextureScript};
use dc_fx_data::*;

const N: usize = 10_000;

fn fuzz(seeds: &[Vec<u8>], salt: u64, mut f: impl FnMut(&[u8]) -> bool) -> (usize, usize) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ salt);
    let (mut ok, mut err) = (0, 0);
    for i in 0..N {
        let seed = &seeds[i % seeds.len()];
        let buf = mutate(&mut rng, seed);
        if f(&buf) {
            ok += 1;
        } else {
            err += 1;
        }
    }
    (ok, err)
}

#[test]
fn fuzz_spr() {
    let seeds = vec![
        spr(&["a\\b.dtx", "a\\c.dtx"], 10, 0, 0, 0),
        spr(&[], 1, 0, 0, 0),
        spr(&["x"; 40], 30, 1, 1, 7),
    ];
    let (ok, err) = fuzz(&seeds, 1, |b| match Sprite::parse(b) {
        Ok(s) => {
            let mut c = SpriteCursor::new(&s);
            for d in [0u32, 1, 17, u32::MAX, 5000] {
                c.advance(&s, d);
                if let Some(i) = c.frame {
                    assert!(i < s.frames.len());
                }
            }
            true
        }
        Err(_) => false,
    });
    println!("spr ok={ok} err={err}");
    assert!(ok > 0 && err > 0);
}

#[test]
fn fuzz_tfs() {
    let seeds = vec![
        tfs(
            &["a", "b"],
            4,
            0,
            3,
            &[1.0, 2.0],
            &[ins(2, 0x26, 0x28, 0), ins(5, 1, 0, 1), ins(0, 0, 1, 0x1c)],
        ),
        tfs(&[], 6, 0, 0, &[], &[]),
        tfs(
            &["p"],
            5,
            0,
            3,
            &[0.5],
            &[
                ins(4, 0x26, 0x28, 0),
                ins(6, 0, 0, 0x10),
                ins(8, 0, 1, 2),
                ins(9, 0, 1, 3),
            ],
        ),
    ];
    let (ok, err) = fuzz(&seeds, 2, |b| match TextureScript::parse(b) {
        Ok(t) => {
            let mut st = t.new_state();
            for k in 0..3 {
                let m = t.run(
                    &mut st,
                    &tfs::ScriptInputs {
                        time: k as f32,
                        delta: 0.1,
                        ..Default::default()
                    },
                );
                let _ = m;
            }
            true
        }
        Err(_) => false,
    });
    println!("tfs ok={ok} err={err}");
    assert!(ok > 0 && err > 0);
}

#[test]
fn fuzz_tfg() {
    let seeds = vec![tfg_two_stage(), {
        let mut v = Vec::new();
        for w in [1u32, 2, 0, 0] {
            le32(&mut v, w);
        }
        v
    }];
    let (ok, err) = fuzz(&seeds, 3, |b| TextureGroup::parse(b).is_ok());
    println!("tfg ok={ok} err={err}");
    assert!(ok > 0 && err > 0);
}

#[test]
fn fuzz_fxf_text() {
    let seeds = vec![fxf_text(["alpha", "beta", "gamma"]).into_bytes()];
    let (ok, err) = fuzz(&seeds, 4, |b| {
        // exercise both detection and the forced text path
        let _ = FxfFile::parse(b);
        match FxfFile::parse_as(b, Encoding::Text) {
            Ok(f) => {
                let _ = f.counts();
                true
            }
            Err(_) => false,
        }
    });
    println!("fxf text ok={ok} err={err}");
    assert!(ok > 0 && err > 0);
}

#[test]
fn fuzz_fxf_binary() {
    let seeds = vec![fxf_binary("zeta")];
    let (ok, err) = fuzz(&seeds, 5, |b| {
        match FxfFile::parse_as(b, Encoding::Binary) {
            Ok(f) => {
                let _ = f.counts();
                true
            }
            Err(_) => false,
        }
    });
    println!("fxf bin ok={ok} err={err}");
    assert!(ok > 0 && err > 0);
}

#[test]
fn fuzz_fcf() {
    let seeds = vec![fcf_text().into_bytes()];
    let (ok, err) = fuzz(&seeds, 6, |b| FcfFile::parse(b).is_ok());
    println!("fcf ok={ok} err={err}");
    assert!(ok > 0 && err > 0);
}

#[test]
fn fuzz_dropped_never_loads() {
    let seeds = vec![vec![0u8; 16]];
    let (ok, _) = fuzz(&seeds, 7, |b| reject_dropped("a.ltc", b).is_ok());
    assert_eq!(ok, 0);
}

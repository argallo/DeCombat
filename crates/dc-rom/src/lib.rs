//! 2009 ROM access: SHA-256, hostile-input-safe PE parser, build identification, pristine guard,
//! archive-key extraction and the runtime table extractors.
//!
//! No Nexon data lives in this crate: tables are read from the user's client folder (`CA_ROM`) at run time and
//! verified against pinned SHA-256 digests. Production code takes the ROM path from [`resolve_rom`] (the one
//! place `CA_ROM` is read) or as an explicit argument.

mod error;
pub mod journal;
mod pe;
mod rom;
pub mod server_rom;
mod sha256;
mod tables;
pub mod trace;

pub use error::RomError;
pub use pe::PeFile;
#[allow(deprecated)]
pub use rom::{
    crypt_key, crypt_key_from_pe, guard_pristine, guard_pristine_files, identify_among,
    identify_rom, repo_root, resolve_rom, rom_dir, test_rom, KnownBuild, RomId, KNOWN_BUILDS,
    SENTINEL_FILES,
};
pub use sha256::{sha256, sha256_file, sha256_hex, Sha256};
pub use tables::{
    chatfilter, chatfilter_decode, cmdtab, cmdtab_decode, defbinds, defbinds_decode, defcfg,
    defcfg_decode, enginevars, enginevars_decode, modelnodes, modelnodes_decode, rendervars,
    rendervars_decode, EngineVar, MnOp, RenderVar, MN_ADD, MN_CTOR_END, MN_FILENAME, MN_MEMSET,
    MN_ST_ADDR, MN_ST_ARG, MN_ST_CONST, MN_ST_KIND, MN_ST_TEMP,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::synth_pe;
    use std::path::Path;

    // ---- canonical serialisation helpers: ROM-derived values are compared by hash + count only ----------

    fn lp(buf: &mut Vec<u8>, bytes: &[u8]) {
        buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(bytes);
    }
    fn strs_canon<S: AsRef<str>>(items: &[S]) -> String {
        let mut b = Vec::new();
        for s in items {
            let bytes: Vec<u8> = s.as_ref().chars().map(|c| c as u32 as u8).collect();
            lp(&mut b, &bytes);
        }
        sha256_hex(&b)
    }

    fn engine(rom: &Path) -> PeFile {
        PeFile::from_file(&rom.join("Engine.exe")).expect("load Engine.exe")
    }
    fn cshell(rom: &Path) -> PeFile {
        PeFile::from_file(&rom.join("Game").join("CShell.dll")).expect("load CShell.dll")
    }

    #[test]
    fn rom_identify_and_key() {
        let rom = rom_or_skip!();
        assert_eq!(identify_rom(&rom).unwrap(), RomId::Eu20091221);
        let key = crypt_key(&rom).expect("crypt_key");
        assert_eq!(sha256_hex(&key).len(), 64);
        assert!((4..=56).contains(&key.len()));
    }

    #[test]
    fn rom_table_extractors_match_pinned_digests() {
        let rom = rom_or_skip!();
        let cs = cshell(&rom);
        let en = engine(&rom);

        let (wf_h, wf) = chatfilter(&cs).expect("chatfilter");
        assert_eq!(
            wf_h,
            "edf39ef4f36d40b3633a0c90b9e81051c6b6516a939bc35e758f6dea8c11c72e"
        );
        assert_eq!(wf.len(), 3636);
        assert_eq!(
            strs_canon(&wf),
            "0034015f8646b1f0e0c9a42688a824ac50547ca6f485ffbc87da57aeb982e32f"
        );

        let (cfg_h, cfg) = defcfg(&en).expect("defcfg");
        assert_eq!(
            cfg_h,
            "4f3556c9f66ab89f16eceeb8b728580ccf25dafa0a3dca04d7905927b03e65e7"
        );
        assert_eq!(cfg.len(), 183);
        assert_eq!(
            strs_canon(&cfg),
            "e60696dd7e40d9c6e6a2a4b454183dcf25c5df0d350074c5fdf079846ae0fca0"
        );

        let (cmd_h, cmds) = cmdtab(&cs).expect("cmdtab");
        assert_eq!(
            cmd_h,
            "5a8848bd8e493e2147f8fd89dc9de1bc6aed0a9025b361a9cfd33936164fcb4b"
        );
        assert_eq!(cmds.len(), 36);
        let mut b = Vec::new();
        for r in &cmds {
            for v in r {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        assert_eq!(
            sha256_hex(&b),
            "5a8848bd8e493e2147f8fd89dc9de1bc6aed0a9025b361a9cfd33936164fcb4b"
        );

        let (h1, h2, ops, names) = modelnodes(&cs).expect("modelnodes");
        assert_eq!(
            h1,
            "a90f0eafa4440dacd4131671b4028cb0d5aaf67d3d122572cb7fe402b56c7849"
        );
        assert_eq!(
            h2,
            "e9ce57b916da1c07bd57136a7654767d6a8e490dc7c782fa00c68f924be96a08"
        );
        assert_eq!(ops.len(), 8618);
        assert_eq!(names.len(), 1747);
        let mut ob = Vec::new();
        for &(op, off, a, b2) in &ops {
            ob.extend_from_slice(&op.to_le_bytes());
            ob.extend_from_slice(&off.to_le_bytes());
            ob.extend_from_slice(&a.to_le_bytes());
            ob.extend_from_slice(&b2.to_le_bytes());
        }
        assert_eq!(
            sha256_hex(&ob),
            "be3951f0077e01566f58a1e728d8f4f4599630583b73975fad433835e893681d"
        );
        assert_eq!(
            strs_canon(&names),
            "ee5005a4f1a8c0a50bb051010a49974492a8fecf05a9908f4fdcf0d471aa6e5a"
        );

        let (rv_hs, rvs) = rendervars(&en).expect("rendervars");
        assert_eq!(
            rv_hs,
            vec![
                "e1c6666cce6da9e04d8b0846e3aa48046c2188b1e7976767df8aded7c0d73303",
                "637b6381a852b1e4d0eac915a32632192fdf53a22de9ba40b71272421c8498bb",
            ]
        );
        assert_eq!(rvs.len(), 185);
        let mut rb = Vec::new();
        for (n, t, f, i) in &rvs {
            lp(
                &mut rb,
                &n.chars().map(|c| c as u32 as u8).collect::<Vec<_>>(),
            );
            lp(&mut rb, t.as_bytes());
            rb.extend_from_slice(&f.to_bits().to_le_bytes());
            rb.extend_from_slice(&i.to_le_bytes());
        }
        assert_eq!(
            sha256_hex(&rb),
            "cebb0c64670ed8f7c4c0202e25794eaa3c1910c8a4f38a247d84f6869c438a68"
        );

        let (ev1, ev2, evs) = enginevars(&en).expect("enginevars");
        assert_eq!(
            ev1,
            "8c9501b8d2cbb41c52a3a0cf541a936674c3c36ec55a9515d2ed22929f1e870d"
        );
        assert_eq!(
            ev2,
            "5703fe2bb1cda6e542c221c58fb6a4aa378a79341254e21a6ac00e21ce0a913a"
        );
        assert_eq!(evs.len(), 102);
        let mut eb = Vec::new();
        for (n, t, v) in &evs {
            lp(
                &mut eb,
                &n.chars().map(|c| c as u32 as u8).collect::<Vec<_>>(),
            );
            lp(&mut eb, t.as_bytes());
            match v {
                Some(x) => {
                    eb.push(1);
                    eb.extend_from_slice(&x.to_bits().to_le_bytes());
                }
                None => eb.push(0),
            }
        }
        assert_eq!(
            sha256_hex(&eb),
            "2004c6d8c33527bb14c171a3c4a1af2d5081616b34660a0c3041f7f19d28fd77"
        );

        let (d1, d2, dbs) = defbinds(&cs).expect("defbinds");
        assert_eq!(
            d1,
            "9c9c124ab8e16232d84b95bb16db70d469a90ee0af202e88683f7ea139e209d0"
        );
        assert_eq!(
            d2,
            "b3ed123c135be33a88240c7f3304df7df860345633d717bedaa28959fbabfa06"
        );
        assert_eq!(dbs.len(), 7);
        let mut db = Vec::new();
        for (id, s) in &dbs {
            db.extend_from_slice(&id.to_le_bytes());
            lp(
                &mut db,
                &s.chars().map(|c| c as u32 as u8).collect::<Vec<_>>(),
            );
        }
        assert_eq!(
            sha256_hex(&db),
            "b287aa86ee43f0b183d5afa35a944a1da7e4eb2fcf60af78e9ca0407984d540b"
        );
    }

    #[test]
    fn extractors_refuse_unpinned_code() {
        // A syntactically valid PE that is not the supported build: every extractor names its table.
        let pe = PeFile::from_bytes(synth_pe(0x40_0000, 0x1000, &[0u8; 64])).unwrap();
        // Ranges are outside this image: typed range error, not a panic.
        assert!(matches!(
            chatfilter(&pe),
            Err(RomError::RvaOutOfRange { .. })
        ));
        // Large synthetic section containing the table rvas but wrong bytes: hash mismatch with table name.
        let mut payload = vec![0u8; 0x5000];
        payload[0] = 0x90;
        let pe = PeFile::from_bytes(synth_pe(0x40_0000, 0x455d40, &payload)).unwrap();
        match cmdtab(&pe) {
            Err(RomError::TableHashMismatch { table, .. }) => assert_eq!(table, "cmdtab"),
            other => panic!("unexpected {other:?}"),
        }
    }

    // ---- seeded fuzzing (no panics on hostile input) ----------------------------------------------------

    struct XorShift(u64);
    impl XorShift {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % (n as u64).max(1)) as usize
        }
    }

    const OPS: &[u8] = &[
        0x50, 0x55, 0x58, 0x5d, 0x68, 0x6a, 0x8b, 0x89, 0x8d, 0x33, 0xc7, 0xc6, 0x83, 0x81, 0xe8,
        0xff, 0xc3, 0xc2, 0xb8, 0xa1, 0xf3, 0xab, 0x64, 0x75, 0xeb, 0xe9, 0x2b, 0x8a, 0x84, 0x4c,
        0x24, 0x15, 0x92, 0xfa,
    ];

    fn gen_code(r: &mut XorShift, len: usize, base: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity(len + 4);
        while v.len() < len {
            match r.below(8) {
                0 => v.extend_from_slice(
                    &(base.wrapping_add(0x1000 + r.below(len + 64) as u32)).to_le_bytes(),
                ),
                1 => v.extend_from_slice(&(r.next() as u32).to_le_bytes()),
                2..=5 => v.push(OPS[r.below(OPS.len())]),
                _ => v.push(r.next() as u8),
            }
        }
        v.truncate(len);
        v
    }

    /// Syntactically valid instruction streams so the fuzzer reaches the success paths of the decoders.
    fn gen_wf(r: &mut XorShift, base: u32, n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..n {
            match r.below(9) {
                0 => v.push(0x64),
                1 => v.extend_from_slice(&[0x6a, r.next() as u8]),
                2 => {
                    v.push(0x68);
                    v.extend_from_slice(
                        &base.wrapping_add(0x1000 + r.below(64) as u32).to_le_bytes(),
                    );
                }
                3 => v.extend_from_slice(&[
                    0x8d,
                    0x4c,
                    0x24,
                    if r.below(2) == 0 {
                        0x2c
                    } else {
                        r.next() as u8
                    },
                ]),
                4 => {
                    v.extend_from_slice(&[0xff, 0x15]);
                    v.extend_from_slice(&base.wrapping_add(0x3ca5a0).to_le_bytes());
                }
                5 => v.extend_from_slice(&[0x8b, 0xc1]),
                6 => v.push(0x50 + r.below(16) as u8),
                7 => v.extend_from_slice(&[0xc7, 0x44, 0x24, r.next() as u8, 1, 2, 3, 4]),
                _ => v.push(0xc3),
            }
        }
        v
    }

    fn gen_db(r: &mut XorShift, base: u32, strs_rva: u32, nstr: usize, n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..n {
            match r.below(5) {
                0 => v.extend_from_slice(&[0x83, 0xfa, r.next() as u8]),
                1 => {
                    v.extend_from_slice(&[0x81, 0xfa]);
                    v.extend_from_slice(&(r.next() as u32).to_le_bytes());
                }
                2 | 3 => {
                    v.push(0x68);
                    v.extend_from_slice(
                        &base
                            .wrapping_add(strs_rva + r.below(nstr) as u32)
                            .to_le_bytes(),
                    );
                }
                _ => v.extend_from_slice(&[0x40 + r.below(16) as u8, 0x75, r.next() as u8]),
            }
        }
        v
    }

    #[test]
    fn fuzz_structured_streams_reach_success_paths() {
        let mut r = XorShift(0xdead_beef_cafe_f00d);
        let base = 0x40_0000u32;
        let (mut wf_ok, mut wf_keys, mut db_ok, mut db_items, mut rv_ok) = (0, 0, 0, 0, 0);
        for _ in 0..10_000 {
            // word strings live in the section at 0x1000
            let mut payload = Vec::new();
            for k in 0..64u16 {
                payload.extend_from_slice(&(0x61 + k % 26).to_le_bytes());
                payload.extend_from_slice(&0u16.to_le_bytes());
            }
            let n_wf = 1 + r.below(40);
            let wf = gen_wf(&mut r, base, n_wf);
            let pe = PeFile::from_bytes(synth_pe(base, 0x1000, &payload)).unwrap();
            if let Ok(keys) = chatfilter_decode(&pe, &wf) {
                wf_ok += 1;
                wf_keys += keys.len();
            }
            let pool: Vec<u8> = (0..64)
                .map(|i| if i % 7 == 6 { 0 } else { b'a' + (i % 26) as u8 })
                .collect();
            let n_db = 1 + r.below(20);
            let db = gen_db(&mut r, base, 0x1000, 64, n_db);
            if let Ok(items) = defbinds_decode(&pe, &db, &pool, 0x1000) {
                db_ok += 1;
                db_items += items.len();
            }
            // render var records pointing at a name inside the section
            let mut rec = Vec::new();
            for _ in 0..1 + r.below(4) {
                rec.extend_from_slice(
                    &[0x6860c8u32, 0x6860c0, 0x6860d0, 0x1][r.below(4)].to_le_bytes(),
                );
                rec.extend_from_slice(&(r.next() as u32).to_le_bytes());
                rec.extend_from_slice(
                    &base
                        .wrapping_add(0x1000 + 8 * r.below(8) as u32)
                        .to_le_bytes(),
                );
                rec.extend_from_slice(&[0u8; 12]);
            }
            let mut sec = vec![b'n'; 80];
            sec[79] = 0;
            let n_rec = rec.len() / 24;
            sec.extend_from_slice(&rec);
            let pe2 = PeFile::from_bytes(synth_pe(base, 0x1000, &sec)).unwrap();
            if rendervars_decode(&pe2, &[(0x1000 + 80, n_rec)]).is_ok() {
                rv_ok += 1;
            }
        }
        println!("structured fuzz: chatfilter ok={wf_ok} keys={wf_keys} defbinds ok={db_ok} items={db_items} rendervars ok={rv_ok}");
        assert!(wf_ok > 1000 && wf_keys > 0 && db_ok > 1000 && db_items > 0 && rv_ok > 0);
    }

    #[test]
    fn fuzz_pe_from_bytes() {
        let mut r = XorShift(0x9e37_79b9_7f4a_7c15);
        let seed = synth_pe(0x40_0000, 0x1000, b"seed payload\0");
        for i in 0..10_000 {
            let buf: Vec<u8> = if i % 2 == 0 {
                let n = r.below(700);
                (0..n).map(|_| r.next() as u8).collect()
            } else {
                let mut b = seed.clone();
                for _ in 0..1 + r.below(8) {
                    let p = r.below(b.len().min(0x200));
                    b[p] = r.next() as u8;
                }
                if r.below(4) == 0 {
                    let t = r.below(b.len());
                    b.truncate(t);
                }
                b
            };
            if let Ok(pe) = PeFile::from_bytes(buf) {
                let _ = pe.rva(r.next() as u32, r.below(100_000));
                let _ = pe.rva_or_zero(r.next() as u32, r.below(64));
                let _ = pe.cstr(r.next() as u32);
                let _ = pe.wstr(0x1000 + r.below(32) as u32);
                let _ = pe.cstr(0x40_1000);
                let _ = chatfilter(&pe);
                let _ = defcfg(&pe);
                let _ = cmdtab(&pe);
                let _ = modelnodes(&pe);
                let _ = rendervars(&pe);
                let _ = enginevars(&pe);
                let _ = defbinds(&pe);
                let _ = crypt_key_from_pe(&pe, r.next() as u32, "00");
            }
        }
    }

    #[test]
    fn fuzz_each_extractor_decoder() {
        let mut r = XorShift(0x1234_5678_9abc_def1);
        let base = 0x40_0000u32;
        let mut ok = [0u32; 6];
        for _ in 0..10_000 {
            let len = 16 + r.below(1500);
            let payload = gen_code(&mut r, len, base);
            let pe = PeFile::from_bytes(synth_pe(base, 0x1000, &payload)).unwrap();
            let code = &payload[..];
            ok[0] += chatfilter_decode(&pe, code).is_ok() as u32;
            let _ = defcfg_decode(code);
            ok[1] += cmdtab_decode(code).is_ok() as u32;
            ok[2] += modelnodes_decode(
                &pe,
                (0x1000, len / 2),
                (0x1000 + (len / 2) as u32, len - len / 2),
            )
            .is_ok() as u32;
            let _ = modelnodes_decode(&pe, (0x1000, len), (0x1000, len));
            ok[3] += rendervars_decode(
                &pe,
                &[(0x1000, len / 24), (0x1000 + r.below(len) as u32, 3)],
            )
            .is_ok() as u32;
            ok[4] += enginevars_decode(&pe, code).is_ok() as u32;
            let strs_rva = 0x1000 + r.below(len) as u32;
            ok[5] += defbinds_decode(
                &pe,
                code,
                &payload[(strs_rva - 0x1000) as usize..],
                strs_rva,
            )
            .is_ok() as u32;
        }
        println!("fuzz decoder Ok counts (chatfilter, cmdtab, modelnodes, rendervars, enginevars, defbinds): {ok:?}");
        assert!(ok[1] > 0, "fuzz must reach successful decodes");
    }

    #[test]
    fn decoders_handle_empty_and_degenerate_input() {
        let pe = PeFile::from_bytes(synth_pe(0x40_0000, 0x1000, &[0u8; 8])).unwrap();
        assert!(chatfilter_decode(&pe, &[]).unwrap().is_empty());
        assert!(defcfg_decode(&[]).is_empty());
        assert!(cmdtab_decode(&[1, 2, 3]).unwrap().is_empty());
        assert!(enginevars_decode(&pe, &[]).unwrap().1.is_empty());
        assert!(defbinds_decode(&pe, &[0x68], &[], 0).is_err());
        assert!(modelnodes_decode(&pe, (0x1000, 8), (0x1000, 8)).is_err());
    }
}

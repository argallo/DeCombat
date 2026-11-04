//! Synthetic-fixture tests. SPEC: FMT_SPR_LTC_FX.md §2 to §7, §8/§9.

mod common;
use common::*;
use dc_fx_data::fcf::FcfFile;
use dc_fx_data::fxf::{Encoding, FieldValue, FxfFile, PropType};
use dc_fx_data::tfg::{Stage, TextureGroup};
use dc_fx_data::tfs::{self, TextureScript};
use dc_fx_data::*;

// ---- .spr ----------------------------------------------------------------------------------------------

#[test]
fn spr_parses_and_keeps_header_words() {
    let b = spr(
        &["a\\x.dtx", "a\\y.dtx", "a\\z.dtx"],
        10,
        0x1_0001,
        2,
        0xCAFE,
    );
    let s = Sprite::parse(&b).unwrap();
    assert_eq!(s.frames.len(), 3);
    assert_eq!(s.frames_per_second, 10);
    assert_eq!((s.flag_a, s.flag_b, s.key_word), (1, 2, 0xCAFE)); // only the low byte of flags is kept
    assert_eq!(s.trailing, 0);
    assert_eq!(s.frame_period_ms(), 100);
    assert_eq!(s.length_ms(), 300);
}

#[test]
fn spr_zero_frames_is_valid_header_only() {
    let s = Sprite::parse(&spr(&[], 8, 0, 0, 0)).unwrap();
    assert!(s.frames.is_empty());
    assert_eq!(s.length_ms(), 0);
    let mut c = SpriteCursor::new(&s);
    assert_eq!(c.frame, None);
    c.advance(&s, 50);
    assert_eq!(c.frame, None);
}

#[test]
fn spr_name_rule_1000_bytes() {
    let ok = "p".repeat(1000);
    let bad = "p".repeat(1001);
    assert!(Sprite::parse(&spr(&[&ok], 1, 0, 0, 0)).is_ok());
    assert!(matches!(
        Sprite::parse(&spr(&[&bad], 1, 0, 0, 0)),
        Err(FxError::NameTooLong { len: 1001, .. })
    ));
}

#[test]
fn spr_rejects_empty_truncated_and_space_filled() {
    assert_eq!(Sprite::parse(&[]), Err(FxError::Empty));
    let b = spr(&["abc"], 5, 0, 0, 0);
    assert!(matches!(
        Sprite::parse(&b[..b.len() - 1]),
        Err(FxError::Truncated { .. })
    ));
    // a header filled with 0x20 bytes: frames_declared 0x20202020 cannot be satisfied
    assert!(Sprite::parse(&[0x20u8; 40]).is_err());
    // frame count larger than the remaining bytes can hold
    let mut b = spr(&["abc"], 5, 0, 0, 0);
    b[0..4].copy_from_slice(&1_000_000u32.to_le_bytes());
    assert!(matches!(Sprite::parse(&b), Err(FxError::Truncated { .. })));
}

#[test]
fn spr_name_is_cut_at_nul() {
    let b = spr(&["ab\0cd"], 1, 0, 0, 0);
    assert_eq!(Sprite::parse(&b).unwrap().frames[0], "ab");
}

#[test]
fn spr_playback_math() {
    // 4 frames at 10 fps: 100 ms per frame, length 400 ms
    let s = Sprite::parse(&spr(&["a", "b", "c", "d"], 10, 0, 0, 0)).unwrap();
    let mut c = SpriteCursor::new(&s);
    assert_eq!(c.frame, Some(0));
    c.advance(&s, 0);
    assert_eq!(c.frame, Some(0));
    c.advance(&s, 100);
    assert_eq!(c.frame, Some(1));
    c.advance(&s, 250);
    assert_eq!((c.time_ms, c.frame), (350, Some(3)));
    c.advance(&s, 100); // 450 wraps to 50
    assert_eq!((c.time_ms, c.frame), (50, Some(0)));
    // non-looping: hold the last frame
    let mut h = SpriteCursor::new(&s);
    h.looping = false;
    h.advance(&s, 5000);
    assert_eq!((h.time_ms, h.frame), (399, Some(3)));
    // paused: time does not move
    let mut p = SpriteCursor::new(&s);
    p.playing = false;
    p.advance(&s, 500);
    assert_eq!((p.time_ms, p.frame), (0, Some(0)));
}

#[test]
fn spr_fps_zero_and_above_1000_have_no_length() {
    for fps in [0u32, 1001, 5000] {
        let s = Sprite::parse(&spr(&["a", "b"], fps, 0, 0, 0)).unwrap();
        assert_eq!(s.length_ms(), 0, "fps {fps}");
        let mut c = SpriteCursor::new(&s);
        c.advance(&s, 100);
        assert_eq!(c.frame, None);
    }
    // fps 1000: 1 ms per frame
    let s = Sprite::parse(&spr(&["a", "b"], 1000, 0, 0, 0)).unwrap();
    assert_eq!(s.length_ms(), 2);
}

// ---- .tfs ----------------------------------------------------------------------------------------------

#[test]
fn tfs_parses_header_and_names() {
    let code = [ins(7, 0x28, 0, 0x1c)];
    let b = tfs(&["u", "v"], 4, 0, 3, &[2.0], &code);
    let t = TextureScript::parse(&b).unwrap();
    assert_eq!(t.param_names, vec!["u", "v"]);
    assert_eq!((t.coord_mode, t.out_mode, t.update_flags), (4, 0, 3));
    assert_eq!(t.constants, vec![2.0]);
    assert_eq!(t.code.len(), 1);
}

#[test]
fn tfs_all_ten_opcodes() {
    // r0 = 7, r1 = 2 via constants at 0x28, 0x29
    let consts = [7.0f32, 2.0];
    let (c0, c1) = (0x28u8, 0x29u8);
    let run = |code: &[[u8; 4]]| {
        let t = TextureScript::parse(&tfs(&[], 0, 0, 0, &consts, code)).unwrap();
        let mut st = t.new_state();
        t.run(&mut st, &tfs::ScriptInputs::default());
        st.regs[0]
    };
    assert_eq!(run(&[ins(0, c0, c1, 0)]), 9.0);
    assert_eq!(run(&[ins(1, c0, c1, 0)]), 5.0);
    assert_eq!(run(&[ins(2, c0, c1, 0)]), 14.0);
    assert_eq!(run(&[ins(3, c0, c1, 0)]), 3.5);
    assert_eq!(run(&[ins(4, c0, c1, 0)]), 1.0); // fmod(7, 2)
    assert_eq!(run(&[ins(6, c0, 0, 0)]), -7.0);
    assert_eq!(run(&[ins(7, c0, 0, 0)]), 7.0);
    assert_eq!(run(&[ins(8, c0, c1, 0)]), 2.0);
    assert_eq!(run(&[ins(9, c0, c1, 0)]), 7.0);
    // op 5 with each of the 6 functions, argument 7.0 -> r0
    let x = 7.0f32;
    assert_eq!(run(&[ins(5, 0, c0, 0)]), x.cos());
    assert_eq!(run(&[ins(5, 1, c0, 0)]), x.sin());
    assert_eq!(run(&[ins(5, 2, c0, 0)]), x.tan());
    assert_eq!(run(&[ins(5, 3, c0, 0)]), 1.0); // sin(7) = 0.657 >= 0
    assert_eq!(run(&[ins(5, 4, c0, 0)]), 1.0); // cos(7) = 0.754 >= 0
    assert_eq!(run(&[ins(5, 5, c0, 0)]), x * 0.017453292);
    // negative side of the square waves: sin(5) < 0, cos(3) < 0
    let t = TextureScript::parse(&tfs(
        &[],
        0,
        0,
        0,
        &[5.0, 3.0],
        &[ins(5, 3, 0x28, 0), ins(5, 4, 0x29, 1)],
    ))
    .unwrap();
    let mut st = t.new_state();
    t.run(&mut st, &Default::default());
    assert_eq!((st.regs[0], st.regs[1]), (-1.0, -1.0));
}

#[test]
fn tfs_matrix_is_identity_when_script_is_empty_and_scroll_works() {
    let t = TextureScript::parse(&tfs(&[], 4, 0, 3, &[], &[])).unwrap();
    let m = tfs::run_at(&t, [0.0; 6], 1.0, 0.0);
    let mut id = [0.0f32; 16];
    for i in 0..4 {
        id[i * 5] = 1.0;
    }
    assert_eq!(m, id);
    // scroll: m[0x1c - 0x10 = 12] (row 3 col 0) = time * param0
    let code = [ins(2, 0x26, 0x20, 0x1c)];
    let t = TextureScript::parse(&tfs(&[], 6, 0, 3, &[], &code)).unwrap();
    let m = tfs::run_at(&t, [0.5, 0.0, 0.0, 0.0, 0.0, 0.0], 4.0, 0.1);
    assert_eq!(m[12], 2.0);
    assert_eq!(m[0], 1.0);
}

#[test]
fn tfs_state_persists_in_parameter_registers() {
    // p0 = p0 + delta, then the caller reads p0 back
    let code = [ins(0, 0x20, 0x27, 0x20)];
    let t = TextureScript::parse(&tfs(&[], 0, 0, 0, &[], &code)).unwrap();
    let mut st = t.new_state();
    let mut inp = tfs::ScriptInputs {
        delta: 0.25,
        ..Default::default()
    };
    t.run(&mut st, &inp);
    inp.params = st.params();
    t.run(&mut st, &inp);
    assert_eq!(st.params()[0], 0.5);
}

#[test]
fn tfs_camera_registers_are_readable() {
    let code = [ins(7, 0x4a, 0, 0)];
    let t = TextureScript::parse(&tfs(&[], 0, 0, 0, &[], &code)).unwrap();
    let mut st = t.new_state();
    t.run(
        &mut st,
        &tfs::ScriptInputs {
            camera: [1.0, 2.0, 3.0],
            ..Default::default()
        },
    );
    assert_eq!(st.regs[0], 3.0);
}

#[test]
fn tfs_rejections() {
    let good = tfs(&[], 0, 0, 0, &[], &[ins(0, 0, 0, 0)]);
    assert!(TextureScript::parse(&good).is_ok());
    assert_eq!(TextureScript::parse(&[]), Err(FxError::Empty));
    // bad version
    let mut b = good.clone();
    b[0] = 2;
    assert!(matches!(
        TextureScript::parse(&b),
        Err(FxError::BadVersion { .. })
    ));
    // opcode 10
    assert!(matches!(
        TextureScript::parse(&tfs(&[], 0, 0, 0, &[], &[ins(10, 0, 0, 0)])),
        Err(FxError::BadField {
            what: "script opcode",
            ..
        })
    ));
    // function 6
    assert!(TextureScript::parse(&tfs(&[], 0, 0, 0, &[], &[ins(5, 6, 0, 0)])).is_err());
    // register 0x4b is out, 0x4a is in
    assert!(TextureScript::parse(&tfs(&[], 0, 0, 0, &[], &[ins(7, 0x4b, 0, 0)])).is_err());
    assert!(TextureScript::parse(&tfs(&[], 0, 0, 0, &[], &[ins(7, 0, 0, 0x4b)])).is_err());
    assert!(TextureScript::parse(&tfs(&[], 0, 0, 0, &[], &[ins(0, 0x4a, 0x4a, 0x4a)])).is_ok());
    // unused operand b of a move is not checked
    assert!(TextureScript::parse(&tfs(&[], 0, 0, 0, &[], &[ins(7, 0, 0xff, 0)])).is_ok());
    // too many constants (0x21)
    let c = vec![0.0f32; 0x21];
    assert!(matches!(
        TextureScript::parse(&tfs(&[], 0, 0, 0, &c, &[])),
        Err(FxError::TooMany { .. })
    ));
    assert!(TextureScript::parse(&tfs(&[], 0, 0, 0, &[0.0; 0x20], &[])).is_ok());
    // coord_mode 7
    assert!(TextureScript::parse(&tfs(&[], 7, 0, 0, &[], &[])).is_err());
    // code_len not a multiple of 4
    let mut b = tfs(&[], 0, 0, 0, &[], &[ins(0, 0, 0, 0)]);
    let n = b.len();
    b[n - 8..n - 4].copy_from_slice(&3u32.to_le_bytes());
    assert!(TextureScript::parse(&b).is_err());
    // truncated code
    let b = tfs(&[], 0, 0, 0, &[], &[ins(0, 0, 0, 0), ins(0, 0, 0, 0)]);
    assert!(matches!(
        TextureScript::parse(&b[..b.len() - 1]),
        Err(FxError::Truncated { .. })
    ));
}

#[test]
fn tfs_malformed_name_block_yields_no_names_but_still_parses() {
    let mut b = tfs(&["abc"], 0, 0, 0, &[], &[]);
    // names_len says 5 ([1,'a','b','c',0]); break the NUL
    b[6 + 4] = b'x';
    let t = TextureScript::parse(&b).unwrap();
    assert!(t.param_names.is_empty());
}

// ---- .tfg ----------------------------------------------------------------------------------------------

#[test]
fn tfg_two_stage_fixture() {
    let g = TextureGroup::parse(&tfg_two_stage()).unwrap();
    assert_eq!(g.trailing, 0);
    assert_eq!(g.stages.len(), 2);
    match &g.stages[0] {
        Stage::Script {
            channel,
            script,
            param_count,
            params,
        } => {
            assert_eq!(
                (*channel, script.as_str(), *param_count),
                (1, "dir\\scroll.tfs", 8)
            );
            assert_eq!(params, &[0.5, 1.5, 2.5, 3.5, 4.5, 5.5]);
        }
        o => panic!("{o:?}"),
    }
    assert_eq!(
        g.stages[1],
        Stage::Share {
            channel: 2,
            source_stage: 0
        }
    );
}

#[test]
fn tfg_rules() {
    // stages_declared clamps to 2
    let mut b = Vec::new();
    for w in [1u32, 9, 0, 0] {
        le32(&mut b, w);
    }
    let g = TextureGroup::parse(&b).unwrap();
    assert_eq!(g.stages, vec![Stage::Unused, Stage::Unused]);
    assert_eq!(g.trailing, 0);
    // wrong version
    let mut v2 = b.clone();
    v2[0] = 2;
    assert!(matches!(
        TextureGroup::parse(&v2),
        Err(FxError::BadVersion { .. })
    ));
    // share source >= 2
    let mut s = Vec::new();
    for w in [1u32, 1, 1, 1, 2] {
        le32(&mut s, w);
    }
    assert!(TextureGroup::parse(&s).is_err());
    // script path above 0x104
    let mut s = Vec::new();
    for w in [1u32, 1, 2, 1] {
        le32(&mut s, w);
    }
    s.extend_from_slice(&0x105u16.to_le_bytes());
    s.extend(std::iter::repeat_n(b'a', 0x105));
    le32(&mut s, 0);
    assert!(matches!(
        TextureGroup::parse(&s),
        Err(FxError::NameTooLong { .. })
    ));
    // param count beyond the data
    let mut s = Vec::new();
    for w in [1u32, 1, 2, 1] {
        le32(&mut s, w);
    }
    s.extend_from_slice(&1u16.to_le_bytes());
    s.push(b'a');
    le32(&mut s, 1000);
    assert!(TextureGroup::parse(&s).is_err());
    // unknown kind reads nothing more
    let mut s = Vec::new();
    for w in [1u32, 1, 77] {
        le32(&mut s, w);
    }
    assert_eq!(
        TextureGroup::parse(&s).unwrap().stages,
        vec![Stage::Other { kind: 77 }]
    );
}

// ---- .fxf ----------------------------------------------------------------------------------------------

#[test]
fn fxf_text_groups_keys_properties() {
    let t = fxf_text(["alpha", "beta", "gamma"]);
    let f = FxfFile::parse_as(t.as_bytes(), Encoding::Text).unwrap();
    assert_eq!(f.trailing, 0);
    assert_eq!(f.groups.len(), 1);
    let g = &f.groups[0];
    // the word value is cut at the first space (SPEC §5.3)
    assert_eq!(g.name, "grp");
    assert_eq!(g.length_ms, 2000);
    assert_eq!(g.length_s, 2.0);
    assert_eq!(g.keys.len(), 3);
    // unresolved names stay None but keep a sha-id
    assert!(g
        .keys
        .iter()
        .all(|k| k.effect_type.is_none() && k.type_id.len() == 16));
    // key 0: clamped times, 8 properties, one of each type
    let k = g.keys.iter().find(|k| k.id == 10).unwrap();
    assert_eq!((k.start, k.end), (0.0, 2.0)); // -1.5 -> 0, 9.0 -> 2.0
    assert!(k.whole_group);
    assert_eq!(k.attach_node, "node");
    assert_eq!(k.properties.len(), 8);
    let tys: Vec<_> = k.properties.iter().map(|p| p.prop_type()).collect();
    assert_eq!(
        tys,
        [
            PropType::String,
            PropType::Int,
            PropType::Float,
            PropType::Choice,
            PropType::Vec3,
            PropType::Vec4,
            PropType::ColourKey,
            PropType::Path
        ]
    );
    assert_eq!(k.properties[1].value, FieldValue::Int(-7));
    assert_eq!(k.properties[2].value, FieldValue::Float(1.5));
    assert_eq!(
        k.properties[3].value,
        FieldValue::Choice {
            index: 4,
            raw: "4,a,b".into()
        }
    );
    // `1 2 3` is cut by the single-token rule only for word shapes; vec3 reads 3 floats
    assert_eq!(k.properties[4].value, FieldValue::Vec3([1.0, 2.0, 3.0]));
    assert_eq!(
        k.properties[5].value,
        FieldValue::Vec4([1.0, 2.0, 3.0, 4.0])
    );
    // t = 0.5; c1 = 1.0 -> 255, c2 = 0.5 -> 127, c3 = 0 -> 0, c4 = 1.0 -> 255
    assert_eq!(
        k.properties[6].value,
        FieldValue::ColourKey {
            t: 0.5,
            packed: 0xFF00_7FFF
        }
    );
    assert_eq!(
        k.properties[7].value.path_parts(),
        Some(("spr", Some("a.spr")))
    );
    let c = f.counts();
    assert_eq!((c.groups, c.keys, c.properties), (1, 3, 8));
}

#[test]
fn fxf_link_rules_and_ordering() {
    let t = fxf_text(["a", "b", "c"]);
    let f = FxfFile::parse_as(t.as_bytes(), Encoding::Text).unwrap();
    let ids: Vec<u32> = f.groups[0].keys.iter().map(|k| k.id).collect();
    // key 11 links to 12 which came later: swapped so the target is first
    assert_eq!(ids, vec![10, 12, 11]);
    let k12 = &f.groups[0].keys[1];
    assert!(!k12.linked, "a key linked to its own id is unlinked");
    let k11 = &f.groups[0].keys[2];
    assert!(k11.linked && k11.link_id == 12);
}

#[test]
fn fxf_text_tolerates_crlf_and_blank_values() {
    let t = fxf_text(["a", "b", "c"]).replace('\n', "\r\n");
    assert!(FxfFile::parse_as(t.as_bytes(), Encoding::Text).is_ok());
    assert_eq!(FxfFile::parse(b""), Err(FxError::Empty));
}

#[test]
fn fxf_text_truncation_is_an_error_not_a_panic() {
    let t = fxf_text(["a", "b", "c"]);
    for cut in [0usize, 5, 40, t.len() / 2, t.len() - 20] {
        let r = FxfFile::parse_as(&t.as_bytes()[..cut], Encoding::Text);
        assert!(r.is_err(), "cut {cut}");
    }
}

#[test]
fn fxf_binary_round() {
    let b = fxf_binary("zeta");
    assert_eq!(fxf::detect_encoding(&b), Encoding::Binary);
    let f = FxfFile::parse(&b).unwrap();
    assert_eq!(f.encoding, Encoding::Binary);
    assert_eq!(f.trailing, 0);
    let g = &f.groups[0];
    assert_eq!((g.name.as_str(), g.length_ms), ("bin grp", 1000)); // binary names keep spaces
    let k = &g.keys[0];
    assert_eq!((k.id, k.link_id, k.linked), (5, 6, true));
    assert_eq!(k.attach_node, "attach");
    assert_eq!((k.start, k.end), (0.1, 0.9));
    assert_eq!(k.properties.len(), 8);
    assert_eq!(k.properties[0].value, FieldValue::String("str".into()));
    assert_eq!(k.properties[1].value, FieldValue::Int(-3));
    assert_eq!(
        k.properties[3].value,
        FieldValue::Choice {
            index: 2,
            raw: "2,x,y".into()
        }
    );
    assert_eq!(
        k.properties[6].value,
        FieldValue::ColourKey {
            t: 0.5,
            packed: 0xAABBCCDD
        }
    );
    assert_eq!(
        k.properties[7].value.path_parts(),
        Some(("dtx", Some("x.dtx")))
    );
    for cut in [0usize, 3, 100, 200, b.len() - 1] {
        assert!(
            FxfFile::parse_as(&b[..cut], Encoding::Binary).is_err(),
            "cut {cut}"
        );
    }
}

#[test]
fn fxf_path_placeholder_means_none() {
    let v = FieldValue::Path("dtx|...".into());
    assert_eq!(v.path_parts(), Some(("dtx", None)));
    assert_eq!(FieldValue::Int(1).path_parts(), None);
}

#[test]
fn fxf_property_count_bound() {
    let mut t = fxf_text(["a", "b", "c"]);
    t = t.replacen("pc 8", "pc 513", 1);
    assert!(matches!(
        FxfFile::parse_as(t.as_bytes(), Encoding::Text),
        Err(FxError::TooMany {
            what: "properties in key",
            count: 513
        })
    ));
}

#[test]
fn registry_shape() {
    assert_eq!(FxType::all().len(), 19);
    for (i, t) in FxType::all().iter().enumerate() {
        assert_eq!(t.index(), i);
        assert_eq!(FxType::from_index(i), Some(*t));
        assert_eq!(t.id().len(), 16);
    }
    assert_eq!(FxType::from_index(19), None);
    assert_eq!(FxType::resolve("not-a-type"), None);
    assert_eq!(FxType::resolve(""), None);
    assert_eq!(FxType::SpawnGroup.type_flags(), 0);
    assert_eq!(FxType::FallingParticles.type_flags(), 0x37);
}

// ---- .fcf ----------------------------------------------------------------------------------------------

#[test]
fn fcf_tree_and_windows() {
    let f = FcfFile::parse(fcf_text().as_bytes()).unwrap();
    assert_eq!(f.roots.len(), 2);
    assert_eq!(f.nodes, 3);
    assert_eq!(f.blocks, 2); // root block + the folder's block
    assert_eq!(f.roots[0].children.len(), 1);
    assert_eq!(f.roots[0].children[0].name, "leaf");
    assert_eq!(f.windows.len(), 1);
    assert_eq!((f.windows[0].left, f.windows[0].bottom), (10, 400));
    assert!(!f.trailing_nonblank);
}

#[test]
fn fcf_depth_bound_and_truncation() {
    // 100 nested single-child folders
    let mut s = String::new();
    for _ in 0..100 {
        s.push_str("c 1\nn x\ni 0\nsi 0\nh 1\nst 0\n");
    }
    assert_eq!(FcfFile::parse(s.as_bytes()), Err(FxError::TooDeep));
    let t = fcf_text();
    assert!(FcfFile::parse(&t.as_bytes()[..t.len() / 2]).is_err());
    assert_eq!(FcfFile::parse(b""), Err(FxError::Empty));
}

// ---- unsupported -----------------------------------------------------------------------------------------

#[test]
fn dropped_families_return_typed_error() {
    for (n, fam) in [
        ("x\\a.LTC", DroppedFamily::Ltc),
        ("a.lta", DroppedFamily::Lta),
        ("b.Wst", DroppedFamily::Wst),
        ("c.pk", DroppedFamily::Pk),
    ] {
        assert_eq!(DroppedFamily::from_name(n), Some(fam));
        assert!(matches!(
            reject_dropped(n, b"x"),
            Err(FxError::Unsupported { .. })
        ));
    }
    assert_eq!(reject_dropped("a.spr", b""), Ok(()));
    assert_eq!(reject_dropped("noext", b""), Ok(()));
    assert!(FxError::Unsupported { family: "x" }
        .to_string()
        .contains("DROP"));
}

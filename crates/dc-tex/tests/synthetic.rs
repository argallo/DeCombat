//! Synthetic fixtures built by hand from the public block definitions and FMT_DTX.md.

use dc_tex::*;

fn file(h: &DtxHeader, body: &[u8]) -> Vec<u8> {
    let mut v = h.to_bytes().to_vec();
    v.extend_from_slice(body);
    v
}

fn hdr(w: u16, h: u16, levels: u16, fmt: u8) -> DtxHeader {
    DtxHeader {
        width: w,
        height: h,
        level_count: levels,
        format: fmt,
        ..DtxHeader::default()
    }
}

#[test]
fn header_round_trip_and_offsets() {
    let mut h = hdr(256, 128, 3, 6);
    h.section_count = 0;
    h.flags = 0x88;
    h.user_word = 0xDEADBEEF;
    h.lod_group = 2;
    h.upload_levels = 5;
    h.upload_skip = 1;
    h.unnamed = 9;
    h.device_priority = 7;
    h.scale_minus_one = -1.0;
    h.angle_deg = -90;
    h.set_command("AlphaRef 128");
    let b = h.to_bytes();
    assert_eq!(b.len(), 0xA4);
    // SPEC: FMT_DTX.md §2 offsets
    assert_eq!(u16::from_le_bytes([b[4], b[5]]), 256);
    assert_eq!(u16::from_le_bytes([b[6], b[7]]), 128);
    assert_eq!(&b[8..12], &(-5i32).to_le_bytes());
    assert_eq!(b[0x1A], 6);
    assert_eq!(&b[0x24..0x24 + 12], b"AlphaRef 128");
    assert_eq!(DtxHeader::parse(&b).unwrap(), h);
    assert_eq!(h.command_str(), "AlphaRef 128");
    assert_eq!(h.effective_upload_levels(), 5);
}

#[test]
fn header_validation() {
    let mut h = hdr(4, 4, 1, 4);
    h.kind = 1;
    assert_eq!(DtxHeader::parse(&h.to_bytes()), Err(DtxError::BadKind(1)));
    let mut h = hdr(4, 4, 1, 4);
    h.version = -4;
    assert_eq!(
        DtxHeader::parse(&h.to_bytes()),
        Err(DtxError::BadVersion(-4))
    );
    for n in [0u16, 9] {
        let h = hdr(4, 4, n, 4);
        assert_eq!(
            DtxHeader::parse(&h.to_bytes()),
            Err(DtxError::BadLevelCount(n))
        );
    }
    assert!(matches!(
        DtxHeader::parse(&[0; 10]),
        Err(DtxError::ShortHeader { .. })
    ));
    let h = hdr(4, 4, 1, 42);
    assert_eq!(
        DtxFile::parse(&file(&h, &[0; 64])),
        Err(DtxError::BadFormat(42))
    );
}

#[test]
fn level_size_rule() {
    assert_eq!(PixelFormat::Bgra8.level_bytes(8, 4), Some(128));
    assert_eq!(PixelFormat::Dxt1.level_bytes(8, 8), Some(32));
    assert_eq!(PixelFormat::Dxt3.level_bytes(8, 8), Some(64));
    assert_eq!(PixelFormat::Dxt5.level_bytes(4, 4), Some(16));
    // no rounding up to whole blocks (SPEC §3)
    assert_eq!(PixelFormat::Dxt1.level_bytes(2, 2), Some(2));
    assert_eq!(PixelFormat::Rgb24.level_bytes(2, 2), Some(12));
    assert_eq!(PixelFormat::Texel16.level_bytes(2, 2), Some(8));
    assert_eq!(PixelFormat::from_code(0).unwrap(), PixelFormat::Bgra8);
}

#[test]
fn mip_slicing_dxt1() {
    // 8x8 DXT1, 3 levels: 32 + 8 + (2x2 -> 2 bytes) = 42
    let h = hdr(8, 8, 3, 4);
    let body: Vec<u8> = (0..42u8).collect();
    let f = DtxFile::parse(&file(&h, &body)).unwrap();
    assert_eq!(f.level_count(), 3);
    assert_eq!(f.level(0), &body[0..32]);
    assert_eq!(f.level(1), &body[32..40]);
    assert_eq!(f.level(2), &body[40..42]);
    assert_eq!((f.level_width(2), f.level_height(2)), (Some(2), Some(2)));
    assert!(f.level(3).is_empty());
    assert_eq!(f.trailing(), 0);
    // sub-block level: lenient decode gives w*h*4 zeroed bytes, strict reports the shortfall
    assert_eq!(f.decode_rgba8(2), vec![0u8; 16]);
    assert!(matches!(
        f.try_decode_rgba8(2, 0),
        Err(DtxError::ShortLevel { .. })
    ));
    let mut longer = file(&h, &body);
    longer.push(0);
    assert_eq!(DtxFile::parse(&longer).unwrap().trailing(), 1);
}

#[test]
fn truncated_is_error() {
    let h = hdr(8, 8, 3, 4);
    assert!(matches!(
        DtxFile::parse(&file(&h, &[0; 41])),
        Err(DtxError::Truncated { need: 42, have: 41 })
    ));
}

#[test]
fn dxt1_four_colour_block() {
    // c0 = pure red 565 (0xF800), c1 = pure blue (0x001F); c0 > c1 so four-colour mode.
    // Index rows (2 bits per texel, LSB first): row0 = 0,1,2,3; row1 = 3,2,1,0; row2 all 0; row3 all 1.
    let row = |a: u8, b: u8, c: u8, d: u8| a | (b << 2) | (c << 4) | (d << 6);
    let blk = [
        0x00,
        0xF8,
        0x1F,
        0x00,
        row(0, 1, 2, 3),
        row(3, 2, 1, 0),
        0x00,
        0x55,
    ];
    let f = DtxFile::parse(&file(&hdr(4, 4, 1, 4), &blk)).unwrap();
    let px = f.decode_rgba8(0);
    assert_eq!(px.len(), 64);
    let at = |x: usize, y: usize| {
        [
            px[(y * 4 + x) * 4],
            px[(y * 4 + x) * 4 + 1],
            px[(y * 4 + x) * 4 + 2],
            px[(y * 4 + x) * 4 + 3],
        ]
    };
    assert_eq!(at(0, 0), [255, 0, 0, 255]); // c0
    assert_eq!(at(1, 0), [0, 0, 255, 255]); // c1
    assert_eq!(at(2, 0), [170, 0, 85, 255]); // (2*c0 + c1) / 3
    assert_eq!(at(3, 0), [85, 0, 170, 255]); // (c0 + 2*c1) / 3
    assert_eq!(at(0, 1), [85, 0, 170, 255]);
    assert_eq!(at(3, 1), [255, 0, 0, 255]);
    assert_eq!(at(2, 2), [255, 0, 0, 255]);
    assert_eq!(at(3, 3), [0, 0, 255, 255]);
}

#[test]
fn dxt1_three_colour_block_with_transparent_entry() {
    // c0 = 0x0000 (black) <= c1 = 0xFFFF (white): three-colour mode, index 3 = transparent black.
    // Indices: texel0 = 0, texel1 = 1, texel2 = 2 (midpoint), texel3 = 3 (transparent).
    let blk = [0x00, 0x00, 0xFF, 0xFF, 0b11_10_01_00, 0, 0, 0];
    let f = DtxFile::parse(&file(&hdr(4, 4, 1, 4), &blk)).unwrap();
    let px = f.decode_rgba8(0);
    assert_eq!(&px[0..4], &[0, 0, 0, 255]);
    assert_eq!(&px[4..8], &[255, 255, 255, 255]);
    assert_eq!(&px[8..12], &[127, 127, 127, 255]);
    assert_eq!(&px[12..16], &[0, 0, 0, 0]);
}

#[test]
fn dxt3_explicit_alpha() {
    // alpha nibbles: texel0 = 0x0, texel1 = 0xF, texel2 = 0x8, texel3 = 0x1; colour all index 0 (green).
    let mut blk = vec![0u8; 16];
    blk[0] = 0xF0; // texel0 low nibble 0, texel1 high nibble F
    blk[1] = 0x18; // texel2 = 8, texel3 = 1
    blk[8..12].copy_from_slice(&[0xE0, 0x07, 0x00, 0x00]); // c0 = 0x07E0 green, c1 = 0
    let f = DtxFile::parse(&file(&hdr(4, 4, 1, 5), &blk)).unwrap();
    let px = f.decode_rgba8(0);
    let a = |i: usize| px[i * 4 + 3];
    assert_eq!((a(0), a(1), a(2), a(3)), (0, 255, 136, 17));
    assert_eq!(&px[0..3], &[0, 255, 0]);
}

#[test]
fn dxt5_alpha_modes() {
    // Eight-value mode (a0 > a1): a0 = 255, a1 = 0; palette index i -> (7-i)/7 steps.
    // texel indices 0..=7 in the first eight texels, packed 3 bits each LSB first.
    let mut bits = 0u64;
    for i in 0..8u64 {
        bits |= i << (3 * i);
    }
    let mut blk = vec![0u8; 16];
    blk[0] = 255;
    blk[1] = 0;
    blk[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    blk[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes()); // white
    let f = DtxFile::parse(&file(&hdr(4, 4, 1, 6), &blk)).unwrap();
    let px = f.decode_rgba8(0);
    let alphas: Vec<u8> = (0..8).map(|i| px[i * 4 + 3]).collect();
    assert_eq!(alphas, vec![255, 0, 218, 182, 145, 109, 72, 36]);

    // Six-value mode (a0 <= a1): indices 6 and 7 are 0 and 255.
    let mut bits = 0u64;
    for i in 0..8u64 {
        bits |= i << (3 * i);
    }
    let mut blk = vec![0u8; 16];
    blk[0] = 0;
    blk[1] = 100;
    blk[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    let px = DtxFile::parse(&file(&hdr(4, 4, 1, 6), &blk))
        .unwrap()
        .decode_rgba8(0);
    let alphas: Vec<u8> = (0..8).map(|i| px[i * 4 + 3]).collect();
    assert_eq!(alphas, vec![0, 100, 20, 40, 60, 80, 0, 255]);
}

#[test]
fn bgra_swizzle() {
    let body = [10u8, 20, 30, 40, 1, 2, 3, 4]; // two texels, B,G,R,A
    let f = DtxFile::parse(&file(&hdr(2, 1, 1, 3), &body)).unwrap();
    assert_eq!(f.decode_rgba8(0), vec![30, 20, 10, 40, 3, 2, 1, 4]);
    // format 0 is read as 3
    let f0 = DtxFile::parse(&file(&hdr(2, 1, 1, 0), &body)).unwrap();
    assert_eq!(f0.decode_rgba8(0), f.decode_rgba8(0));
}

#[test]
fn partial_block_is_cropped() {
    // 2x2 DXT1 level that carries a full 8-byte block still decodes to 2x2 output.
    let blk = [0x00, 0xF8, 0x1F, 0x00, 0, 0, 0, 0];
    let mut h = hdr(2, 2, 1, 4);
    h.flags = 0;
    let mut bytes = file(&h, &blk[..2]);
    bytes.extend_from_slice(&blk[2..]); // trailing bytes ignored by the size rule
    let f = DtxFile::parse(&bytes).unwrap();
    assert_eq!(f.level(0).len(), 2);
    assert_eq!(f.trailing(), 6);
}

#[test]
fn lod_group_skip() {
    // lod_group 2 adds bias 1 (SPEC §4.2): level 0 is seeked over.
    let mut h = hdr(8, 8, 3, 3);
    h.lod_group = 2;
    let body = vec![7u8; 8 * 8 * 4 + 4 * 4 * 4 + 2 * 2 * 4];
    let f = DtxFile::parse(&file(&h, &body)).unwrap();
    assert_eq!(f.skipped(), 1);
    assert_eq!(f.level_count(), 2);
    assert_eq!(f.level_width(0), Some(4));
    // cap at level_count - 1
    assert_eq!(load_skip(0, 3, 99), 2);
    assert_eq!(load_skip(0, 3, -4), 0);
    assert_eq!(load_skip(9, 3, 1), 1);
    let f = DtxFile::parse_with_bias(&file(&hdr(8, 8, 3, 3), &body), 1).unwrap();
    assert_eq!(f.skipped(), 1);
}

#[test]
fn six_face_layout() {
    // 2x2 BGRA, 1 level, six faces: face-major, 0x20-byte gap after face 0 only. SPEC §4.1.
    let mut h = hdr(2, 2, 1, 3);
    h.flags = FLAG_SIX_FACE;
    let mut body = Vec::new();
    body.extend_from_slice(&[1u8; 16]);
    body.extend_from_slice(&[0xEE; 0x20]);
    for f in 2..=6u8 {
        body.extend_from_slice(&[f; 16]);
    }
    let f = DtxFile::parse(&file(&h, &body)).unwrap();
    assert_eq!(f.face_count(), 6);
    for k in 0..6 {
        assert_eq!(f.face(0, k), &[k as u8 + 1; 16][..]);
    }
    assert!(f.face(0, 6).is_empty());
    assert!(DtxFile::parse(&file(&h, &body[..body.len() - 1])).is_err());
}

#[test]
fn command_keywords() {
    let c = parse_command("AlphaRef 128").unwrap();
    assert_eq!(c.alpha_test_value, Some(128));
    assert!(c.material.is_none());
    let c = parse_command("alpharef -3; MipMapBias 1.5").unwrap();
    assert_eq!((c.alpha_test_value, c.mip_bias), (Some(-3), Some(1.5)));
    let c = parse_command("EnvMapAlpha some_tex").unwrap();
    let m = c.material.unwrap();
    assert_eq!((m.kind, m.kind.mode()), (MaterialKind::EnvMapAlpha, 3));
    assert_eq!(m.textures, vec!["some_tex"]);
    let c = parse_command("DetailTex a\nAlphaRef 5").unwrap();
    assert_eq!(c.material.unwrap().kind, MaterialKind::DetailTex);
    assert_eq!(c.alpha_test_value, Some(5));
    // engine test order: DetailTex before EnvMap regardless of statement order
    let c = parse_command("EnvMap e; DetailTex d").unwrap();
    assert_eq!(c.material.unwrap().textures, vec!["d"]);
    // too few arguments
    assert_eq!(
        parse_command("EnvBumpMap one"),
        Err(DtxError::CommandArgs("EnvBumpMap"))
    );
    assert_eq!(
        parse_command("DetailTex"),
        Err(DtxError::CommandArgs("DetailTex"))
    );
    assert_eq!(parse_command("AlphaRef").unwrap(), CommandSet::default());
    // unknown residue is ignored
    assert_eq!(parse_command("zzz 1 2 3").unwrap(), CommandSet::default());
    let c = parse_command("Effect 4 a b c d e")
        .unwrap()
        .material
        .unwrap();
    assert_eq!((c.effect_id, c.textures.len()), (Some(4), 4));
    // through the file API
    let mut h = hdr(4, 4, 1, 3);
    h.set_command("AlphaRef 77");
    let f = DtxFile::parse(&file(&h, &[0; 64])).unwrap();
    assert_eq!(f.commands().unwrap().alpha_test_value, Some(77));
}

/// Tiny xorshift so the fuzz is seeded and reproducible without extra crates.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn seeded_fuzz_never_panics() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for round in 0..10_000u32 {
        let len = (rng.next() % 600) as usize;
        let mut buf: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        // Make a large share of the buffers structurally plausible so deep paths run.
        if round % 2 == 0 && buf.len() >= HEADER_LEN {
            buf[0..4].fill(0);
            buf[8..12].copy_from_slice(&(-5i32).to_le_bytes());
            buf[0x0C] = 1 + (rng.next() % 8) as u8;
            buf[0x0D] = 0;
            buf[0x1A] = (rng.next() % 10) as u8;
            if round % 4 == 0 {
                buf[4..8].copy_from_slice(&[
                    (rng.next() % 40) as u8,
                    0,
                    (rng.next() % 40) as u8,
                    0,
                ]);
            }
            if round % 8 == 0 {
                buf[0x10..0x14].copy_from_slice(&FLAG_SIX_FACE.to_le_bytes());
            }
        }
        if let Ok(f) = DtxFile::parse_with_bias(&buf, (rng.next() % 12) as i32 - 3) {
            for l in 0..10 {
                let _ = f.decode_rgba8(l);
                let _ = f.try_decode_rgba8(l, (rng.next() % 8) as usize);
            }
            let _ = f.commands();
        }
        let _ = parse_command(&String::from_utf8_lossy(&buf));
    }
    // hostile dimensions with no data must fail cleanly, not allocate
    let mut h = hdr(65535, 65535, 8, 3);
    h.flags = FLAG_SIX_FACE;
    assert!(matches!(
        DtxFile::parse(&file(&h, &[])),
        Err(DtxError::Truncated { .. })
    ));
}

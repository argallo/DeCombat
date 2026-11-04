//! Render styles of the model passes: the engine's enum values of a render-style file (`dc_model::StyleFile`)
//! translated to Direct3D 9 fixed-function semantics, the renderer's built-in default style, and the per-model
//! style list (one style per render-style slot of the object).
//!
//! Evidence (our own disassembly / the clean decompiler output of the 2009 `Engine.exe`):
//! - translation tables filled once by engine+0x1b50d0 into the .bss block at VA 0x7e8318 (colour ops at
//!   +0x00, alpha ops +0x44, colour / alpha arguments +0x70 / +0x80, fill +0x90, cull +0x98, source / dest blend
//!   +0xac / +0xd8, compare +0x104, texture-coordinate index +0x120, its stage flag bytes +0x148, address
//!   +0x15c, min / mag / mip filters +0x16c / +0x180 / +0x194, depth enable / write bytes +0xa7 / +0xa4);
//! - their use in the pass setup engine+0x1a88f0 and the device capability check engine+0x161d80 (whose
//!   per-op capability bits confirm the colour-op table: value 4 tests the MODULATE2X bit);
//! - the default style built by engine+0x162890 (slot 14 of the style vtable, called by engine+0x1a81b0 at
//!   model-renderer start-up) and used for a piece whose style slot is empty (engine+0x1b9390).
//!
//! The Direct3D enum values and the meaning of the stage operations are PUBLIC (Direct3D 9 documentation).
//! Names are ours.

use dc_model::{StyleFile, StyleMaterial, StylePass, StyleStage};
use dc_rez::RezVfs;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Texture-stage operation (Direct3D 9 `D3DTEXTUREOP` meaning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StageOp {
    Disable,
    SelectArg1,
    SelectArg2,
    Modulate,
    Modulate2x,
    Add,
    AddSigned,
    AddSigned2x,
    Subtract,
    BlendTextureAlpha,
    BlendFactorAlpha,
    BlendCurrentAlpha,
    ModulateAlphaAddColour,
    ModulateInvAlphaAddColour,
    BumpEnvMap,
    BumpEnvMapLuminance,
    DotProduct3,
}

impl StageOp {
    /// Our shader code of the op (`model.wgsl` uses the same numbers: the public `D3DTEXTUREOP` values).
    pub fn code(self) -> u32 {
        match self {
            StageOp::Disable => 1,
            StageOp::SelectArg1 => 2,
            StageOp::SelectArg2 => 3,
            StageOp::Modulate => 4,
            StageOp::Modulate2x => 5,
            StageOp::Add => 7,
            StageOp::AddSigned => 8,
            StageOp::AddSigned2x => 9,
            StageOp::Subtract => 10,
            StageOp::BlendTextureAlpha => 13,
            StageOp::BlendFactorAlpha => 14,
            StageOp::BlendCurrentAlpha => 16,
            StageOp::ModulateAlphaAddColour => 18,
            StageOp::ModulateInvAlphaAddColour => 20,
            StageOp::BumpEnvMap => 22,
            StageOp::BumpEnvMapLuminance => 23,
            StageOp::DotProduct3 => 24,
        }
    }
}

/// Engine colour-op values 0..16.
// EVIDENCE: engine+0x1b50d0 (dwords +0x00..+0x40 of VA 0x7e8318: 1, 2, 3, 4, 5, 0x10, 0xe, 7, 0x18, 0x16, 0x17,
// 8, 9, 0xa, 0x12, 0x14, 0xd); DISASM: engine+0x161e10 (capability bit per value: 1 SELECTARG1, 2 SELECTARG2,
// 3 MODULATE, 4 MODULATE2X, 5 BLENDCURRENTALPHA, 6 BLENDFACTORALPHA, 7 ADD, 8 DOTPRODUCT3, 9 BUMPENVMAP,
// 10 BUMPENVMAPLUMINANCE, 16 BLENDTEXTUREALPHA)
const COLOUR_OPS: [StageOp; 17] = [
    StageOp::Disable,
    StageOp::SelectArg1,
    StageOp::SelectArg2,
    StageOp::Modulate,
    StageOp::Modulate2x,
    StageOp::BlendCurrentAlpha,
    StageOp::BlendFactorAlpha,
    StageOp::Add,
    StageOp::DotProduct3,
    StageOp::BumpEnvMap,
    StageOp::BumpEnvMapLuminance,
    StageOp::AddSigned,
    StageOp::AddSigned2x,
    StageOp::Subtract,
    StageOp::ModulateAlphaAddColour,
    StageOp::ModulateInvAlphaAddColour,
    StageOp::BlendTextureAlpha,
];

/// Engine alpha-op values 0..10.
// EVIDENCE: engine+0x1b50d0 (dwords +0x44..+0x6c: 1, 2, 3, 4, 0x10, 0xe, 7, 8, 9, 0xa, 0xd); DISASM:
// engine+0x161e63 (capability bits of values 1..6 and 10)
const ALPHA_OPS: [StageOp; 11] = [
    StageOp::Disable,
    StageOp::SelectArg1,
    StageOp::SelectArg2,
    StageOp::Modulate,
    StageOp::BlendCurrentAlpha,
    StageOp::BlendFactorAlpha,
    StageOp::Add,
    StageOp::AddSigned,
    StageOp::AddSigned2x,
    StageOp::Subtract,
    StageOp::BlendTextureAlpha,
];

/// Colour op of an engine value. The engine indexes its table without a bound check, so values 17..27 read the
/// alpha-op table that follows it in memory (OBSERVED: one ROM style uses colour op 19 on stage 1, which reads
/// SELECTARG2); anything further reads unrelated words and is treated as DISABLE here.
// EVIDENCE: engine+0x1a8d92 (`mov edx, [edx*4 + 0x7e8318]`, no bound check)
// DC-DEFAULT: REN-8 (proposed), see DC_DEFAULTS.md (values past both tables read DISABLE)
pub fn colour_op(v: u32) -> StageOp {
    let v = v as usize;
    if v < COLOUR_OPS.len() {
        COLOUR_OPS[v]
    } else if v - COLOUR_OPS.len() < ALPHA_OPS.len() {
        ALPHA_OPS[v - COLOUR_OPS.len()]
    } else {
        StageOp::Disable
    }
}

/// Alpha op of an engine value (DeCombat: values past the table are DISABLE).
// EVIDENCE: engine+0x1a8dbd (`mov edx, [edx*4 + 0x7e835c]`)
pub fn alpha_op(v: u32) -> StageOp {
    ALPHA_OPS
        .get(v as usize)
        .copied()
        .unwrap_or(StageOp::Disable)
}

/// Stage argument (`D3DTA_*` meaning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageArg {
    Current,
    Diffuse,
    Texture,
    Factor,
}

impl StageArg {
    /// Our shader code (the public `D3DTA_*` values).
    pub fn code(self) -> u32 {
        match self {
            StageArg::Diffuse => 0,
            StageArg::Current => 1,
            StageArg::Texture => 2,
            StageArg::Factor => 3,
        }
    }
}

/// Engine argument values 0..3 (colour and alpha tables are equal).
// EVIDENCE: engine+0x1b50d0 (dwords +0x70..+0x7c and +0x80..+0x8c: 1, 0, 2, 3 = CURRENT, DIFFUSE, TEXTURE,
// TFACTOR); DeCombat: other values read as CURRENT
pub fn stage_arg(v: u32) -> StageArg {
    match v {
        1 => StageArg::Diffuse,
        2 => StageArg::Texture,
        3 => StageArg::Factor,
        _ => StageArg::Current,
    }
}

/// Blend factor (`D3DBLEND_*` meaning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColour,
    InvSrcColour,
    SrcAlpha,
    InvSrcAlpha,
    DestColour,
    InvDestColour,
}

/// Source / destination factors of engine blend values 0..10; value 0 means blending off.
// EVIDENCE: engine+0x1b50d0 (source dwords +0xac..+0xd4: 2, 2, 0xa, 5, 3, 9, 3, 3, 5, 5, 9; dest dwords
// +0xd8..+0x100: 1, 2, 2, 6, 4, 0xa, 9, 2, 1, 2, 1); engine+0x1a8c91 (value 0: ALPHABLENDENABLE off)
pub fn blend_factors(v: u32) -> Option<(BlendFactor, BlendFactor)> {
    use BlendFactor::*;
    Some(match v {
        1 => (One, One),
        2 => (InvDestColour, One),
        3 => (SrcAlpha, InvSrcAlpha),
        4 => (SrcColour, InvSrcColour),
        5 => (DestColour, InvDestColour),
        6 => (SrcColour, DestColour),
        7 => (SrcColour, One),
        8 => (SrcAlpha, Zero),
        9 => (SrcAlpha, One),
        10 => (DestColour, Zero),
        _ => return None,
    })
}

/// Comparison function (`D3DCMP_*` meaning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Compare {
    Always,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}

impl Compare {
    /// Our shader code for the alpha test (0 = off).
    pub fn code(self) -> u32 {
        match self {
            Compare::Less => 2,
            Compare::Equal => 3,
            Compare::LessEqual => 4,
            Compare::Greater => 5,
            Compare::NotEqual => 6,
            Compare::GreaterEqual => 7,
            Compare::Always => 8,
        }
    }
}

/// Engine compare values 0..6.
// EVIDENCE: engine+0x1b50d0 (dwords +0x104..+0x11c: 8, 2, 4, 5, 7, 3, 6 = ALWAYS, LESS, LESSEQUAL, GREATER,
// GREATEREQUAL, EQUAL, NOTEQUAL); DeCombat: other values read as ALWAYS
pub fn compare(v: u32) -> Compare {
    match v {
        1 => Compare::Less,
        2 => Compare::LessEqual,
        3 => Compare::Greater,
        4 => Compare::GreaterEqual,
        5 => Compare::Equal,
        6 => Compare::NotEqual,
        _ => Compare::Always,
    }
}

/// Cull mode (`D3DCULL_*` meaning; D3D's front faces are clockwise on screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cull {
    None,
    /// Counter-clockwise faces are culled (back faces).
    Ccw,
    /// Clockwise faces are culled.
    Cw,
}

/// Engine cull values 0..2.
// EVIDENCE: engine+0x1b50d0 (dwords +0x98..+0xa0: 1, 3, 2 = NONE, CCW, CW); DeCombat: others NONE
pub fn cull(v: u32) -> Cull {
    match v {
        1 => Cull::Ccw,
        2 => Cull::Cw,
        _ => Cull::None,
    }
}

/// Depth enable / write of engine depth-mode values 0..2: (test + write), (test only), (off).
// EVIDENCE: engine+0x1b50d0 (bytes +0xa7..+0xa9: 1, 1, 0 ZENABLE; +0xa4..+0xa6: 1, 0, 0 ZWRITEENABLE),
// engine+0x1a8cfa / 0x1a8d1c (both indexed by pass +0x1e4)
pub fn depth_mode(v: u32) -> (bool, bool) {
    match v {
        0 => (true, true),
        1 => (true, false),
        _ => (false, false),
    }
}

/// Texture addressing (`D3DTADDRESS_*` meaning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Address {
    Wrap,
    Clamp,
    Mirror,
    MirrorOnce,
}

/// Engine address values 0..3.
// EVIDENCE: engine+0x1b50d0 (dwords +0x15c..+0x168: 1, 3, 2, 5 = WRAP, CLAMP, MIRROR, MIRRORONCE)
pub fn address(v: u32) -> Address {
    match v {
        1 => Address::Clamp,
        2 => Address::Mirror,
        3 => Address::MirrorOnce,
        _ => Address::Wrap,
    }
}

/// Texture filtering of an engine filter value: (linear min / mag, mip filter) where the mip filter is
/// `None` (no mips), `Some(false)` point, `Some(true)` linear; value 3 is anisotropic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Filter {
    pub linear: bool,
    pub mip: Option<bool>,
    pub anisotropic: bool,
}

/// Engine filter values 0..4.
// EVIDENCE: engine+0x1b50d0 (min / mag dwords +0x16c / +0x180: 1, 2, 2, 3, 1; mip +0x194: 0, 1, 2, 2, 1 =
// D3DTEXF NONE / POINT / LINEAR / ANISOTROPIC), engine+0x1a8eea..0x1a8f4a (MINFILTER, MAGFILTER, MIPFILTER from
// stage +0x28)
pub fn filter(v: u32) -> Filter {
    match v {
        0 => Filter {
            linear: false,
            mip: None,
            anisotropic: false,
        },
        1 => Filter {
            linear: true,
            mip: Some(false),
            anisotropic: false,
        },
        3 => Filter {
            linear: true,
            mip: Some(true),
            anisotropic: true,
        },
        4 => Filter {
            linear: false,
            mip: Some(false),
            anisotropic: false,
        },
        _ => Filter {
            linear: true,
            mip: Some(true),
            anisotropic: false,
        },
    }
}

/// Texture-coordinate source of a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UvSource {
    /// Vertex texture-coordinate set 0..3.
    Set(u32),
    /// Generated: camera-space normal, position, reflection vector; `world` = the engine multiplies the stage
    /// matrix with its camera matrix (values 7..9).
    CameraNormal {
        world: bool,
    },
    CameraPosition {
        world: bool,
    },
    CameraReflection {
        world: bool,
    },
}

impl UvSource {
    /// Our shader code: 0..3 set, 4 normal, 5 position, 6 reflection, + 8 for the world variants.
    pub fn code(self) -> u32 {
        match self {
            UvSource::Set(k) => k.min(3),
            UvSource::CameraNormal { world } => 4 + 8 * world as u32,
            UvSource::CameraPosition { world } => 5 + 8 * world as u32,
            UvSource::CameraReflection { world } => 6 + 8 * world as u32,
        }
    }
}

/// Engine texture-coordinate values 0..9.
// EVIDENCE: engine+0x1b50d0 (dwords +0x120..+0x144: 0, 1, 2, 3, 0x10000, 0x20000, 0x30000, 0x10000, 0x20000,
// 0x30000 = TEXCOORDINDEX set 0..3, D3DTSS_TCI_CAMERASPACENORMAL / POSITION / REFLECTIONVECTOR; bytes +0x148..: 0
// for 0..3 and 1 for 4..9 = OR the stage index in; bytes +0x152..: 1 for 7..9 = multiply with the camera matrix
// at VA 0x7e5a44), engine+0x1a8e7f..0x1a8ea5, engine+0x1a8f8b; DeCombat: other values read as set 0
pub fn uv_source(v: u32) -> UvSource {
    match v {
        0..=3 => UvSource::Set(v),
        4 => UvSource::CameraNormal { world: false },
        5 => UvSource::CameraPosition { world: false },
        6 => UvSource::CameraReflection { world: false },
        7 => UvSource::CameraNormal { world: true },
        8 => UvSource::CameraPosition { world: true },
        9 => UvSource::CameraReflection { world: true },
        _ => UvSource::Set(0),
    }
}

/// Which texture a stage binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageTexture {
    /// Nothing is bound.
    None,
    /// The piece's texture `k` (0..3): the object's texture at the piece's material slot `k`.
    Piece(u32),
    /// The stage keeps whatever was bound before (engine values above 4).
    Keep,
}

/// Engine texture-source values: 0 none, 1..4 the piece's textures 0..3, others keep the binding.
// EVIDENCE: engine+0x1a8240 (switch on stage +0x00: 0 -> null, 1..4 -> piece entry +0x00..+0x0c; default: no
// call); DISASM: engine+0x1a827a (`cmp eax, 4; ja`)
pub fn stage_texture(v: u32) -> StageTexture {
    match v {
        0 => StageTexture::None,
        1..=4 => StageTexture::Piece(v - 1),
        _ => StageTexture::Keep,
    }
}

/// One stage after translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stage {
    pub texture: StageTexture,
    pub colour_op: StageOp,
    pub colour_args: [StageArg; 2],
    pub alpha_op: StageOp,
    pub alpha_args: [StageArg; 2],
    pub uv: UvSource,
    pub address: [Address; 2],
    pub filter: Filter,
    /// Texture transform when on: the stored row-major matrix, the coordinate count, the projected switch.
    pub transform: Option<([f32; 16], u32, bool)>,
}

impl Stage {
    pub fn from_record(s: &StyleStage) -> Stage {
        Stage {
            texture: stage_texture(s.texture),
            colour_op: colour_op(s.colour_op),
            colour_args: [stage_arg(s.colour_arg1), stage_arg(s.colour_arg2)],
            alpha_op: alpha_op(s.alpha_op),
            alpha_args: [stage_arg(s.alpha_arg1), stage_arg(s.alpha_arg2)],
            uv: uv_source(s.uv_source),
            address: [address(s.address_u), address(s.address_v)],
            filter: filter(s.filter),
            // EVIDENCE: engine+0x1a8f50 (with the switch: TEXTURETRANSFORMFLAGS = count | 0x100 when projected,
            // SetTransform(TEXTURE0 + stage, matrix); without: TEXTURETRANSFORMFLAGS 0)
            transform: s
                .transform()
                .then_some((s.matrix, s.transform_count, s.projected())),
        }
    }
}

/// One pass after translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pass {
    pub stages: [Stage; 4],
    /// `None`: blending off.
    pub blend: Option<(BlendFactor, BlendFactor)>,
    pub depth_test: bool,
    pub depth_write: bool,
    pub depth_compare: Compare,
    pub cull: Cull,
    /// TEXTUREFACTOR as stored (0xAARRGGBB).
    pub texture_factor: u32,
    pub alpha_reference: u32,
    /// `None`: alpha test off.
    pub alpha_test: Option<Compare>,
    pub lighting: bool,
    /// Engine fill value 0 (wireframe); drawn solid and counted.
    // DC-DEFAULT: REN-5 (proposed), see DC_DEFAULTS.md
    pub wireframe: bool,
    /// The bump block is on (the bump ops pass the current colour through).
    // DC-DEFAULT: REN-3 (proposed), see DC_DEFAULTS.md
    pub bump: bool,
    /// DeCombat only ([`RenderStyle::plain`]): alpha test GREATEREQUAL at the texture's own reference when the
    /// texture carries one (the state before render styles were decoded).
    pub alpha_from_texture: bool,
}

impl Pass {
    // EVIDENCE: engine+0x1a88f0 (pass +0x1e0 blend, +0x1e4 depth mode, +0x1e8 cull, +0x1ec TEXTUREFACTOR, +0x1f4
    // lighting, +0x1f8 ZFUNC, +0x1fc alpha test (0 -> ALPHATESTENABLE off, else ALPHAFUNC), +0x200 FILLMODE,
    // +0x204 bump); engine+0x1a8240 (ALPHAREF = pass +0x1f0 after the textures are bound); fill table
    // engine+0x1b50d0 (dwords +0x90 / +0x94: 2, 3 = WIREFRAME, SOLID)
    pub fn from_record(p: &StylePass) -> Pass {
        let (depth_test, depth_write) = depth_mode(p.depth_mode);
        Pass {
            stages: p.stages.map(|s| Stage::from_record(&s)),
            blend: blend_factors(p.blend),
            depth_test,
            depth_write,
            depth_compare: compare(p.depth_compare),
            cull: cull(p.cull),
            texture_factor: p.texture_factor,
            alpha_reference: p.alpha_reference,
            alpha_test: (p.alpha_test != 0).then(|| compare(p.alpha_test)),
            lighting: p.lighting(),
            wireframe: p.fill == 0,
            bump: p.bump(),
            alpha_from_texture: false,
        }
    }

    /// The stages the device runs: up to the first stage whose colour op is DISABLE.
    // SPEC: public D3D9 rule (a disabled colour op ends the cascade); EVIDENCE: engine+0x1a8de5 (both ops
    // disabled: the remaining stages are not set)
    pub fn active_stages(&self) -> usize {
        self.stages
            .iter()
            .position(|s| s.colour_op == StageOp::Disable)
            .unwrap_or(4)
    }
}

/// A render style ready for the model pass.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderStyle {
    pub material: StyleMaterial,
    pub passes: Vec<Pass>,
    /// The options block selects the effect path (byte 0 non-zero). No ROM style does (OBSERVED); DeCombat draws
    /// the passes anyway and counts it.
    // EVIDENCE: engine+0x1b94e0 (options byte 0 tested before the passes); DISASM: engine+0x1624c0
    pub effect: bool,
    /// Where it came from (a ROM path, or a built-in name).
    pub source: String,
}

impl RenderStyle {
    /// The first variant of a parsed file (the loader keeps the first variant its device accepts; every ROM
    /// file has exactly one, OBSERVED). `None` for a file without passes.
    pub fn from_file(f: &StyleFile, source: &str) -> Option<RenderStyle> {
        let v = f.first()?;
        if v.passes.is_empty() {
            return None;
        }
        Some(RenderStyle {
            material: v.material,
            passes: v.passes.iter().map(Pass::from_record).collect(),
            effect: v.options.is_some_and(|o| o[0] != 0),
            source: source.to_string(),
        })
    }

    /// The renderer's own default style, used for a piece whose style slot is empty, out of range, or names a
    /// file that did not load: material ambient (1, 1, 1, 1), diffuse (0.8, 0.8, 0.8, 1), emissive and specular
    /// black, power 20; one pass: stage 0 = the piece's first texture, MODULATE(TEXTURE, DIFFUSE) for colour and
    /// alpha, uv set 0, wrap, trilinear; stages 1..3 off; no blending, depth test + write with LESSEQUAL, cull
    /// CCW, TEXTUREFACTOR 0x80808080, alpha reference 128 with the alpha test off, lighting on, solid fill.
    // DISASM: engine+0x162890 (material at style +0x0c..+0x4c, constants VA 0x6898c4 = 0.8 and VA 0x6898c0 = 20.0;
    // the pass record built on the stack and added through engine+0x162160 / 0x1621a0), engine+0x1a81b0 (built
    // once and kept at VA 0x7e84e0); EVIDENCE: engine+0x1b9390 (a missing style -> the value at VA 0x7e84e0)
    pub fn engine_default() -> RenderStyle {
        let mut p = StylePass::default();
        for s in p.stages.iter_mut() {
            s.colour_arg1 = 2;
            s.colour_arg2 = 1;
            s.alpha_arg1 = 2;
            s.alpha_arg2 = 1;
            s.filter = 2;
            s.transform_count = 2;
        }
        p.stages[0].texture = 1;
        p.stages[0].colour_op = 3;
        p.stages[0].alpha_op = 3;
        p.blend = 0;
        p.depth_mode = 0;
        p.cull = 1;
        p.texture_factor = 0x8080_8080;
        p.alpha_reference = 0x80;
        p.lighting_word = 1;
        p.depth_compare = 2;
        p.alpha_test = 0;
        p.fill = 1;
        p.bump_stage = 1;
        p.bump_scale = 1.0;
        RenderStyle {
            material: StyleMaterial {
                ambient: [1.0; 4],
                diffuse: [0.8, 0.8, 0.8, 1.0],
                emissive: [0.0, 0.0, 0.0, 1.0],
                specular: [0.0, 0.0, 0.0, 1.0],
                power: 20.0,
            },
            passes: vec![Pass::from_record(&p)],
            effect: false,
            source: "engine default".to_string(),
        }
    }

    /// The state the model pass used before render styles were decoded (render-models, RENDER_2009 addendum
    /// M10): lighting on with a white material, MODULATE(TEXTURE, DIFFUSE), no culling, depth test + write.
    /// DeCombat: kept for the "without styles" comparison and for callers that pass no style list.
    pub fn plain() -> RenderStyle {
        let mut s = RenderStyle::engine_default();
        s.material.diffuse = [1.0; 4];
        s.material.power = 0.0;
        s.passes[0].cull = Cull::None;
        s.passes[0].alpha_from_texture = true;
        s.source = "plain".to_string();
        s
    }

    /// Whether any pass blends.
    pub fn blends(&self) -> bool {
        self.passes.iter().any(|p| p.blend.is_some())
    }
}

/// Styles read from the user's ROM, by path (upper case, `\` separators).
#[derive(Debug, Default)]
pub struct StyleLibrary {
    styles: BTreeMap<String, Option<Arc<RenderStyle>>>,
}

/// Normalised ROM path of a style name (as the tables write it).
pub fn style_key(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .replace('/', "\\")
        .to_ascii_uppercase()
}

impl StyleLibrary {
    pub fn new() -> StyleLibrary {
        StyleLibrary::default()
    }

    /// The style a ROM path names, read once; `None` when the entry is missing or does not load (the caller
    /// then uses [`RenderStyle::engine_default`], as the engine does).
    pub fn get(&mut self, vfs: &RezVfs, path: &str) -> Option<Arc<RenderStyle>> {
        let key = style_key(path);
        if key.is_empty() {
            return None;
        }
        if let Some(s) = self.styles.get(&key) {
            return s.clone();
        }
        let s = crate::model::read_rom_entry(vfs, &key)
            .ok()
            .and_then(|b| StyleFile::parse(&b).ok())
            .and_then(|f| RenderStyle::from_file(&f, &key))
            .map(Arc::new);
        self.styles.insert(key, s.clone());
        s
    }

    /// The style list of a model entry from its table names (at most 8, as the object's model block carries).
    // SPEC: UDP_SESSION_2009_B.md §3.4 (style_count <= 8); EVIDENCE: engine+0x6bd20 (style slot < 8, model < 32)
    pub fn list(&mut self, vfs: &RezVfs, paths: &[String]) -> Vec<Option<Arc<RenderStyle>>> {
        paths
            .iter()
            .take(MAX_STYLE_SLOTS)
            .map(|p| self.get(vfs, p))
            .collect()
    }
}

/// Render-style slots per model entry of an object.
// EVIDENCE: engine+0x6bd20 (`if style < 8 && model < 0x20`, table at object +0x1344 with 8 entries per model)
pub const MAX_STYLE_SLOTS: usize = 8;

/// The style a piece is drawn with: `styles[slot]` when present, else the engine default.
// EVIDENCE: engine+0x1b9390 (lookup by the piece's style index; not found or null -> the default at VA 0x7e84e0)
pub fn style_for_slot(styles: &[Option<Arc<RenderStyle>>], slot: u32) -> Arc<RenderStyle> {
    styles
        .get(slot as usize)
        .and_then(|s| s.clone())
        .unwrap_or_else(|| Arc::new(RenderStyle::engine_default()))
}

/// Census of a style list for reports: per slot the source, the stage-0 colour op and the pass count.
pub fn describe(styles: &[Option<Arc<RenderStyle>>]) -> Vec<String> {
    styles
        .iter()
        .map(|s| match s {
            Some(s) => format!(
                "{:?} x{} lit {}",
                s.passes[0].stages[0].colour_op,
                s.passes.len(),
                s.passes[0].lighting
            ),
            None => "missing (engine default)".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        assert_eq!(colour_op(3), StageOp::Modulate);
        assert_eq!(colour_op(4), StageOp::Modulate2x);
        assert_eq!(colour_op(16), StageOp::BlendTextureAlpha);
        // past the colour table: the alpha table follows in memory
        assert_eq!(colour_op(17), StageOp::Disable);
        assert_eq!(colour_op(19), StageOp::SelectArg2);
        assert_eq!(colour_op(1000), StageOp::Disable);
        assert_eq!(alpha_op(4), StageOp::BlendCurrentAlpha);
        assert_eq!(alpha_op(11), StageOp::Disable);
        assert_eq!(stage_arg(0), StageArg::Current);
        assert_eq!(stage_arg(1), StageArg::Diffuse);
        assert_eq!(blend_factors(0), None);
        assert_eq!(
            blend_factors(3),
            Some((BlendFactor::SrcAlpha, BlendFactor::InvSrcAlpha))
        );
        assert_eq!(compare(2), Compare::LessEqual);
        assert_eq!(compare(0), Compare::Always);
        assert_eq!(depth_mode(1), (true, false));
        assert_eq!(depth_mode(2), (false, false));
        assert_eq!(cull(1), Cull::Ccw);
        assert_eq!(address(1), Address::Clamp);
        assert_eq!(uv_source(6), UvSource::CameraReflection { world: false });
        assert_eq!(UvSource::CameraReflection { world: true }.code(), 14);
        assert_eq!(stage_texture(1), StageTexture::Piece(0));
        assert_eq!(stage_texture(5), StageTexture::Keep);
        assert_eq!(StageOp::Modulate2x.code(), 5);
    }

    #[test]
    fn default_style_matches_the_engine_record() {
        let d = RenderStyle::engine_default();
        assert_eq!(d.material.diffuse, [0.8, 0.8, 0.8, 1.0]);
        assert_eq!(d.passes.len(), 1);
        let p = &d.passes[0];
        assert_eq!(p.active_stages(), 1);
        assert_eq!(p.stages[0].colour_op, StageOp::Modulate);
        assert_eq!(
            p.stages[0].colour_args,
            [StageArg::Texture, StageArg::Diffuse]
        );
        assert_eq!(p.stages[0].texture, StageTexture::Piece(0));
        assert_eq!(p.cull, Cull::Ccw);
        assert_eq!(p.depth_compare, Compare::LessEqual);
        assert!(p.lighting && p.depth_test && p.depth_write);
        assert_eq!(p.alpha_test, None);
        assert_eq!(p.blend, None);
        assert!(!p.wireframe);
        let plain = RenderStyle::plain();
        assert_eq!(plain.passes[0].cull, Cull::None);
        assert_eq!(plain.material.diffuse, [1.0; 4]);
    }

    #[test]
    fn slot_lookup_falls_back_to_the_default() {
        let s = Arc::new(RenderStyle::plain());
        let list = vec![Some(s.clone()), None];
        assert_eq!(style_for_slot(&list, 0).source, "plain");
        assert_eq!(style_for_slot(&list, 1).source, "engine default");
        assert_eq!(style_for_slot(&list, 7).source, "engine default");
        assert_eq!(style_key(" \"rs/a.ltb\" "), "RS\\A.LTB");
    }
}

//! `.tfs` texture script: a 4-byte-instruction float register VM producing a 4x4 texture matrix.
//! SPEC: FMT_SPR_LTC_FX.md §4.

use crate::error::{latin1_cstr, Bin, FxError};

/// Registers addressable by a script: 0x00..=0x4A (scratch, matrix, parameters, time, constants, camera vec3).
/// // EVIDENCE: engine+0x1b4500 (object is 0x144 bytes; SPEC §4.3 says every index must be bounded)
pub const REG_COUNT: usize = 0x4B;
pub const REG_MATRIX: usize = 0x10;
pub const REG_PARAMS: usize = 0x20;
pub const REG_TIME: usize = 0x26;
pub const REG_DELTA: usize = 0x27;
pub const REG_CONST: usize = 0x28;
pub const REG_CAMERA: usize = 0x48;
/// Constants fit below the camera registers. // EVIDENCE: SPEC §4.3
pub const MAX_CONSTS: usize = REG_CAMERA - REG_CONST;
pub const OP_COUNT: u8 = 10;
pub const FUNC_COUNT: u8 = 6;

/// The 10 opcodes. // EVIDENCE: engine+0x1b4090..engine+0x1b4220 (table at VA 0x6d66b0)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    /// C `fmod`. // PUBLIC: C99 7.12.10.1
    Rem,
    /// `func[a](r[b])`
    Call,
    Neg,
    Mov,
    Min,
    Max,
}

impl Op {
    pub fn from_code(c: u8) -> Option<Op> {
        Some(match c {
            0 => Op::Add,
            1 => Op::Sub,
            2 => Op::Mul,
            3 => Op::Div,
            4 => Op::Rem,
            5 => Op::Call,
            6 => Op::Neg,
            7 => Op::Mov,
            8 => Op::Min,
            9 => Op::Max,
            _ => return None,
        })
    }
}

/// The 6 unary functions. // EVIDENCE: engine+0x1b3ff0..engine+0x1b4080 (table at VA 0x6d6698)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Cos,
    Sin,
    Tan,
    /// `+1` if `sin(x) >= 0` else `-1`
    SquareSin,
    /// `+1` if `cos(x) >= 0` else `-1`
    SquareCos,
    /// `x * 0.017453292`
    DegToRad,
}

impl Func {
    pub fn from_code(c: u8) -> Option<Func> {
        Some(match c {
            0 => Func::Cos,
            1 => Func::Sin,
            2 => Func::Tan,
            3 => Func::SquareSin,
            4 => Func::SquareCos,
            5 => Func::DegToRad,
            _ => return None,
        })
    }
    pub fn eval(self, x: f32) -> f32 {
        match self {
            Func::Cos => x.cos(),
            Func::Sin => x.sin(),
            Func::Tan => x.tan(),
            Func::SquareSin => {
                if x.sin() >= 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
            Func::SquareCos => {
                if x.cos() >= 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
            Func::DegToRad => x * 0.017453292,
        }
    }
}

/// One decoded instruction (`op, a, b, dst`). For [`Op::Call`], `a` is the function index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instr {
    pub op: Op,
    pub a: u8,
    pub b: u8,
    pub dst: u8,
}

/// A parsed, validated texture script.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureScript {
    pub version: u16,
    /// Authoring names of the user parameters (not read by the engine); empty if the block is malformed.
    pub param_names: Vec<String>,
    pub coord_mode: u32,
    pub out_mode: u32,
    pub update_flags: u32,
    pub constants: Vec<f32>,
    pub code: Vec<Instr>,
}

fn check_reg(r: u8) -> Result<(), FxError> {
    if (r as usize) < REG_COUNT {
        Ok(())
    } else {
        Err(FxError::BadField {
            what: "script register",
            value: r as u32,
        })
    }
}

fn parse_names(block: &[u8]) -> Vec<String> {
    let Some((&count, mut rest)) = block.split_first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for _ in 0..count {
        let Some(end) = rest.iter().position(|&c| c == 0) else {
            return Vec::new();
        };
        out.push(latin1_cstr(&rest[..end], end));
        rest = &rest[end + 1..];
    }
    if rest.is_empty() {
        out
    } else {
        Vec::new()
    }
}

impl TextureScript {
    /// Parse a whole `.tfs` entry. SPEC: FMT_SPR_LTC_FX.md §4.2. Unlike the 2009 loader this bounds
    /// `const_count`, checks opcodes, function indices and registers up front, and requires `code_len % 4 == 0`.
    pub fn parse(data: &[u8]) -> Result<TextureScript, FxError> {
        if data.is_empty() {
            return Err(FxError::Empty);
        }
        let mut r = Bin::new(data);
        let version = r.u16("script version")?;
        if version != 1 {
            return Err(FxError::BadVersion {
                what: "texture script",
                found: version as u32,
            });
        }
        let names_len = r.u32("script names_len")? as usize;
        let param_names = parse_names(r.bytes(names_len, "script param names")?);
        let coord_mode = r.u32("script coord_mode")?;
        if coord_mode > 6 {
            return Err(FxError::BadField {
                what: "script coord_mode",
                value: coord_mode,
            });
        }
        let out_mode = r.u32("script out_mode")?;
        let update_flags = r.u32("script update_flags")?;
        let const_count = r.u32("script const_count")?;
        if const_count as usize > MAX_CONSTS {
            return Err(FxError::TooMany {
                what: "script constants",
                count: const_count,
            });
        }
        let mut constants = Vec::with_capacity(const_count as usize);
        for _ in 0..const_count {
            constants.push(r.f32("script constant")?);
        }
        let code_len = r.u32("script code_len")? as usize;
        if !code_len.is_multiple_of(4) {
            return Err(FxError::BadField {
                what: "script code_len",
                value: code_len as u32,
            });
        }
        let raw = r.bytes(code_len, "script code")?;
        let mut code = Vec::with_capacity(code_len / 4);
        for w in raw.as_chunks::<4>().0 {
            let (opc, a, b, dst) = (w[0], w[1], w[2], w[3]);
            let op = Op::from_code(opc).ok_or(FxError::BadField {
                what: "script opcode",
                value: opc as u32,
            })?;
            check_reg(dst)?;
            match op {
                Op::Call => {
                    if Func::from_code(a).is_none() {
                        return Err(FxError::BadField {
                            what: "script function",
                            value: a as u32,
                        });
                    }
                    check_reg(b)?;
                }
                Op::Neg | Op::Mov => check_reg(a)?,
                _ => {
                    check_reg(a)?;
                    check_reg(b)?;
                }
            }
            code.push(Instr { op, a, b, dst });
        }
        Ok(TextureScript {
            version,
            param_names,
            coord_mode,
            out_mode,
            update_flags,
            constants,
            code,
        })
    }

    /// Fresh register file: zeros with the constants loaded at 0x28.. (scratch persists between runs like
    /// the engine's script object).
    pub fn new_state(&self) -> ProgramState {
        let mut regs = [0.0f32; REG_COUNT];
        for (i, c) in self.constants.iter().enumerate() {
            regs[REG_CONST + i] = *c;
        }
        ProgramState { regs }
    }

    /// Run the script once. SPEC: FMT_SPR_LTC_FX.md §4.3/§4.4. The matrix is reset to identity, the six
    /// parameters, the two time values and the camera vector are written in, the program runs, and the
    /// parameters are left in `state` for the caller to read back (`state.params()`).
    pub fn run(&self, state: &mut ProgramState, inputs: &ScriptInputs) -> [f32; 16] {
        let regs = &mut state.regs;
        for i in 0..16 {
            regs[REG_MATRIX + i] = if i % 5 == 0 { 1.0 } else { 0.0 };
        }
        regs[REG_PARAMS..REG_PARAMS + 6].copy_from_slice(&inputs.params);
        regs[REG_TIME] = inputs.time;
        regs[REG_DELTA] = inputs.delta;
        regs[REG_CAMERA..REG_CAMERA + 3].copy_from_slice(&inputs.camera);
        for ins in &self.code {
            let (a, b) = (
                regs[ins.a as usize % REG_COUNT],
                regs[ins.b as usize % REG_COUNT],
            );
            let v = match ins.op {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                Op::Div => a / b,
                Op::Rem => a % b,
                Op::Call => Func::from_code(ins.a).map_or(0.0, |f| f.eval(b)),
                Op::Neg => -a,
                Op::Mov => a,
                Op::Min => {
                    if a < b {
                        a
                    } else {
                        b
                    }
                }
                Op::Max => {
                    if a > b {
                        a
                    } else {
                        b
                    }
                }
            };
            regs[ins.dst as usize % REG_COUNT] = v;
        }
        let mut m = [0.0f32; 16];
        m.copy_from_slice(&regs[REG_MATRIX..REG_MATRIX + 16]);
        m
    }
}

/// Values the caller supplies for one run.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScriptInputs {
    pub params: [f32; 6],
    /// Register 0x26: running time of the group (units unproven, SPEC §11 item 3).
    pub time: f32,
    /// Register 0x27: time since the previous update.
    pub delta: f32,
    /// Registers 0x48..0x4A.
    pub camera: [f32; 3],
}

/// Register file of one script instance.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramState {
    pub regs: [f32; REG_COUNT],
}

impl ProgramState {
    /// The six parameter registers as left by the last run (scripts may keep state in them).
    pub fn params(&self) -> [f32; 6] {
        let mut p = [0.0; 6];
        p.copy_from_slice(&self.regs[REG_PARAMS..REG_PARAMS + 6]);
        p
    }
}

/// Run `script` once from a fresh state at `time` with `delta` since the last update.
pub fn run_at(script: &TextureScript, params: [f32; 6], time: f32, delta: f32) -> [f32; 16] {
    let mut st = script.new_state();
    script.run(
        &mut st,
        &ScriptInputs {
            params,
            time,
            delta,
            camera: [0.0; 3],
        },
    )
}

//! `.tfg` texture effect group: binds up to two texture-coordinate stages to scripts.
//! SPEC: FMT_SPR_LTC_FX.md §3.

use crate::error::{latin1_cstr, Bin, FxError};

/// Longest script path. // EVIDENCE: engine+0x183800 (0x104)
pub const MAX_SCRIPT_PATH: usize = 0x104;
pub const MAX_STAGES: u32 = 2;
pub const KEPT_PARAMS: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// kind 0: nothing more is read.
    Unused,
    /// kind 1: reuse the matrix of `source_stage` (< 2). Channel 0 leaves the stage inactive.
    Share { channel: u32, source_stage: u32 },
    /// kind 2: own script. `params` are the first six values (zero-padded), `param_count` the stored count.
    Script {
        channel: u32,
        script: String,
        param_count: u32,
        params: [f32; KEPT_PARAMS],
    },
    /// Any other kind: the loader reads nothing more for it.
    Other { kind: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextureGroup {
    pub stages: Vec<Stage>,
    pub trailing: usize,
}

impl TextureGroup {
    /// Parse a whole `.tfg` entry. SPEC: FMT_SPR_LTC_FX.md §3.2.
    pub fn parse(data: &[u8]) -> Result<TextureGroup, FxError> {
        if data.is_empty() {
            return Err(FxError::Empty);
        }
        let mut r = Bin::new(data);
        let version = r.u32("group version")?;
        if version != 1 {
            return Err(FxError::BadVersion {
                what: "texture group",
                found: version,
            });
        }
        let stages_declared = r.u32("group stages_declared")?.min(MAX_STAGES);
        let mut stages = Vec::new();
        for _ in 0..stages_declared {
            let kind = r.u32("stage kind")?;
            stages.push(match kind {
                0 => Stage::Unused,
                1 => {
                    let channel = r.u32("stage channel")?;
                    let source_stage = r.u32("stage source_stage")?;
                    if source_stage >= MAX_STAGES {
                        return Err(FxError::BadField {
                            what: "stage source_stage",
                            value: source_stage,
                        });
                    }
                    Stage::Share {
                        channel,
                        source_stage,
                    }
                }
                2 => {
                    let channel = r.u32("stage channel")?;
                    let len = r.u16("stage script length")? as usize;
                    if len > MAX_SCRIPT_PATH {
                        return Err(FxError::NameTooLong {
                            what: "stage script path",
                            len,
                        });
                    }
                    let script = latin1_cstr(r.bytes(len, "stage script path")?, len);
                    let param_count = r.u32("stage param_count")?;
                    if (param_count as usize) > r.remaining() / 4 {
                        return Err(FxError::Truncated {
                            what: "stage params",
                            offset: r.pos,
                        });
                    }
                    let mut params = [0.0f32; KEPT_PARAMS];
                    for i in 0..param_count as usize {
                        let v = r.f32("stage param")?;
                        if i < KEPT_PARAMS {
                            params[i] = v;
                        }
                    }
                    Stage::Script {
                        channel,
                        script,
                        param_count,
                        params,
                    }
                }
                k => Stage::Other { kind: k },
            });
        }
        Ok(TextureGroup {
            stages,
            trailing: r.remaining(),
        })
    }
}

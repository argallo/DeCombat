//! Runtime table extractors for the user's 2009 `Engine.exe` / `CShell.dll`.
//!
//! Every extractor first compares the SHA-256 of the exact code/table bytes it is about to decode with a
//! pinned constant (hashes only, never the data) and returns [`RomError::TableHashMismatch`] naming the table
//! when the binary is not the supported build. The `*_decode` functions skip that check so they can be fuzzed
//! with synthetic input; they never panic on arbitrary bytes.
//!
//! EVIDENCE: all rvas below are decompiler-derived facts about the 2009 binaries (engine+0x255db8 etc.).

use crate::error::RomError;
use crate::pe::PeFile;
use crate::sha256::sha256_hex;
use std::collections::HashMap;

type S<T> = Result<T, String>;

// --- String-error shims over the PE reader (internal decoders report plain messages; the public wrappers
// attach the table name). ---------------------------------------------------------------------------------

fn rv(pe: &PeFile, rva: u32, len: usize) -> S<&[u8]> {
    pe.rva(rva, len).map_err(|e| e.to_string())
}
fn rvz(pe: &PeFile, va: u32, len: usize) -> S<Vec<u8>> {
    pe.rva_or_zero(va, len).map_err(|e| e.to_string())
}
fn cs(pe: &PeFile, va: u32) -> S<String> {
    pe.cstr(va).map_err(|e| e.to_string())
}
fn ws(pe: &PeFile, va: u32) -> S<String> {
    pe.wstr(va).map_err(|e| e.to_string())
}
fn read_u32_le(d: &[u8], off: usize) -> S<u32> {
    crate::pe::read_u32_le(d, off).map_err(|e| e.to_string())
}
fn read_i32_le(d: &[u8], off: usize) -> S<i32> {
    crate::pe::read_i32_le(d, off).map_err(|e| e.to_string())
}
fn read_i8(d: &[u8], off: usize) -> S<i8> {
    crate::pe::read_i8(d, off).map_err(|e| e.to_string())
}

fn named<T>(table: &'static str, r: S<T>) -> Result<T, RomError> {
    r.map_err(|m| RomError::decode(table, m))
}

fn check_hash(
    table: &'static str,
    data: &[u8],
    expected: &'static str,
) -> Result<String, RomError> {
    let actual = sha256_hex(data);
    if actual == expected {
        Ok(actual)
    } else {
        Err(RomError::TableHashMismatch {
            table,
            expected,
            actual,
        })
    }
}

// Pinned SHA-256 digests of the supported (2009-12-21 EU test) table/code bytes. Hashes only.
const H_WORDFILTER_CODE: &str = "edf39ef4f36d40b3633a0c90b9e81051c6b6516a939bc35e758f6dea8c11c72e";
const H_DEFCFG_RAW: &str = "4f3556c9f66ab89f16eceeb8b728580ccf25dafa0a3dca04d7905927b03e65e7";
const H_CMDTAB_RAW: &str = "5a8848bd8e493e2147f8fd89dc9de1bc6aed0a9025b361a9cfd33936164fcb4b";
const H_MN_CTOR: &str = "a90f0eafa4440dacd4131671b4028cb0d5aaf67d3d122572cb7fe402b56c7849";
const H_MN_INIT: &str = "e9ce57b916da1c07bd57136a7654767d6a8e490dc7c782fa00c68f924be96a08";
const H_RV_RANGES: [&str; 2] = [
    "e1c6666cce6da9e04d8b0846e3aa48046c2188b1e7976767df8aded7c0d73303",
    "637b6381a852b1e4d0eac915a32632192fdf53a22de9ba40b71272421c8498bb",
];
const H_EV_TABLE: &str = "8c9501b8d2cbb41c52a3a0cf541a936674c3c36ec55a9515d2ed22929f1e870d";
const H_EV_VALUES: &str = "5703fe2bb1cda6e542c221c58fb6a4aa378a79341254e21a6ac00e21ce0a913a";
const H_DB_CODE: &str = "9c9c124ab8e16232d84b95bb16db70d469a90ee0af202e88683f7ea139e209d0";
const H_DB_STR: &str = "b3ed123c135be33a88240c7f3304df7df860345633d717bedaa28959fbabfa06";

fn modrm_len(code: &[u8], i: usize) -> S<usize> {
    let m = *code
        .get(i)
        .ok_or_else(|| format!("modrm_len out of bounds at +{:#x}", i))?;
    let mod_bits = m >> 6;
    let rm = m & 7;
    let mut n = 1usize;
    if mod_bits == 3 {
        return Ok(n);
    }
    if rm == 4 {
        let sib = *code
            .get(i + 1)
            .ok_or_else(|| format!("sib out of bounds at +{:#x}", i + 1))?;
        n += 1;
        if mod_bits == 0 && (sib & 7) == 5 {
            n += 4;
        }
    } else if mod_bits == 0 && rm == 5 {
        n += 4;
    }
    if mod_bits == 1 {
        n += 1;
    } else if mod_bits == 2 {
        n += 4;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------------------------------------
// chatfilter
// ---------------------------------------------------------------------------------------------------------

/// Extracts the CShell word-filter table (`rva = 0x2fea70`, `size = 0x3d546`).
/// Returns `(code_sha256_hex, keys)`.
pub fn chatfilter(pe: &PeFile) -> Result<(String, Vec<String>), RomError> {
    const RVA: u32 = 0x2fea70;
    const SIZE: usize = 0x3d546;
    let code = pe.rva(RVA, SIZE)?;
    let h = check_hash("chatfilter", code, H_WORDFILTER_CODE)?;
    let keys = chatfilter_decode(pe, code)?;
    Ok((h, keys))
}

/// Decoder body of [`chatfilter`] without the pinned-hash check.
pub fn chatfilter_decode(pe: &PeFile, code: &[u8]) -> Result<Vec<String>, RomError> {
    named("chatfilter", chatfilter_inner(pe, code))
}

fn chatfilter_inner(pe: &PeFile, code: &[u8]) -> S<Vec<String>> {
    const ASSIGN: u32 = 0x3ca5a0;
    const DISP: u32 = 0x2c;

    let mut keys = Vec::new();
    let mut i = 0usize;
    let mut last_push: Option<u32> = None;
    let mut last_lea: Option<u32> = None;
    let byte = |k: usize| -> S<u8> {
        code.get(k)
            .copied()
            .ok_or_else(|| format!("chatfilter: truncated instruction at +{:x}", k))
    };

    while i < code.len() {
        let op = code[i];
        if op == 0x64 {
            // fs: prefix
            i += 1;
            continue;
        }
        if op == 0x6a {
            i += 2;
        } else if op == 0x68 {
            last_push = Some(read_u32_le(code, i + 1)?);
            i += 5;
        } else if op == 0xa1 || op == 0xe8 || (0xb8..=0xbf).contains(&op) {
            i += 5;
        } else if (0x50..=0x5f).contains(&op) || op == 0xc3 {
            i += 1;
        } else if op == 0x8b || op == 0x89 || op == 0x33 {
            i += 1 + modrm_len(code, i + 1)?;
        } else if op == 0x8d {
            let m = byte(i + 1)?;
            if (m == 0x4c || m == 0x8c) && byte(i + 2)? == 0x24 {
                last_lea = Some(if m == 0x4c {
                    byte(i + 3)? as u32
                } else {
                    read_u32_le(code, i + 3)?
                });
            }
            i += 1 + modrm_len(code, i + 1)?;
        } else if op == 0xc7 {
            i += 1 + modrm_len(code, i + 1)? + 4;
        } else if op == 0xc6 || op == 0x83 {
            i += 1 + modrm_len(code, i + 1)? + 1;
        } else if op == 0xff {
            if byte(i + 1)? == 0x15 {
                let tgt = read_u32_le(code, i + 2)?.wrapping_sub(pe.base);
                if tgt == ASSIGN && last_lea == Some(DISP) {
                    let push_va =
                        last_push.ok_or_else(|| "chatfilter: missing last_push".to_string())?;
                    let rva = push_va.checked_sub(pe.base).ok_or_else(|| {
                        format!("chatfilter: string va {:#x} below image base", push_va)
                    })?;
                    keys.push(ws(pe, rva)?);
                }
            }
            i += 1 + modrm_len(code, i + 1)?;
        } else {
            return Err(format!("opcode {:02x} at +{:x}", op, i));
        }
    }
    Ok(keys)
}

// ---------------------------------------------------------------------------------------------------------
// defcfg
// ---------------------------------------------------------------------------------------------------------

/// Extracts the Engine embedded default configuration (`rva = 0x255db8`, 183 x 256-byte entries).
/// Returns `(raw_sha256_hex, lines)`.
pub fn defcfg(pe: &PeFile) -> Result<(String, Vec<String>), RomError> {
    const RVA: u32 = 0x255db8;
    const N: usize = 183;
    let raw = pe.rva(RVA, N * 256)?;
    let h = check_hash("defcfg", raw, H_DEFCFG_RAW)?;
    Ok((h, defcfg_decode(raw)))
}

/// Decoder body of [`defcfg`]: splits `raw` into 256-byte NUL-terminated Latin-1 lines.
pub fn defcfg_decode(raw: &[u8]) -> Vec<String> {
    raw.as_chunks::<256>()
        .0
        .iter()
        .map(|chunk| {
            let end = chunk.iter().position(|&b| b == 0).unwrap_or(chunk.len());
            chunk[..end].iter().map(|&b| b as char).collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------------------
// cmdtab
// ---------------------------------------------------------------------------------------------------------

/// Extracts the CShell command table (`rva = 0x455d40`, 36 x 16-byte entries).
/// Returns `(raw_sha256_hex, records)`.
pub fn cmdtab(pe: &PeFile) -> Result<(String, Vec<[i32; 4]>), RomError> {
    const RVA: u32 = 0x455d40;
    const N: usize = 36;
    let raw = pe.rva(RVA, N * 16)?;
    let h = check_hash("cmdtab", raw, H_CMDTAB_RAW)?;
    Ok((h, cmdtab_decode(raw)?))
}

/// Decoder body of [`cmdtab`]: 16-byte records of four little-endian `i32`.
pub fn cmdtab_decode(raw: &[u8]) -> Result<Vec<[i32; 4]>, RomError> {
    named("cmdtab", cmdtab_inner(raw))
}

fn cmdtab_inner(raw: &[u8]) -> S<Vec<[i32; 4]>> {
    let mut recs = Vec::with_capacity(raw.len() / 16);
    for k in 0..raw.len() / 16 {
        let b = 16 * k;
        recs.push([
            read_i32_le(raw, b)?,
            read_i32_le(raw, b + 4)?,
            read_i32_le(raw, b + 8)?,
            read_i32_le(raw, b + 12)?,
        ]);
    }
    Ok(recs)
}

// Model-node program op codes (our own numbering for the decoded op stream).
pub const MN_ADD: u32 = 1;
pub const MN_ST_CONST: u32 = 2;
pub const MN_ST_TEMP: u32 = 3;
pub const MN_ST_ADDR: u32 = 4;
pub const MN_ST_ARG: u32 = 5;
pub const MN_ST_KIND: u32 = 6;
pub const MN_MEMSET: u32 = 7;
pub const MN_FILENAME: u32 = 8;
pub const MN_CTOR_END: u32 = 9;

const MN_CTOR: (u32, usize) = (0x10e0, 0x315);
const MN_INIT: (u32, usize) = (0x1980, 0x12ec1);
const MN_CALL_ADD: u32 = 0x1600;
const MN_CALL_RECT: u32 = 0x10a0;
const MN_CALL_CHKSTK: u32 = 0x3a5e50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SymVal {
    U,
    Stk(i32),
    This(i32),
    Arg,
    Const(i32),
    Temp(u32),
}

fn mn_run(
    pe: &PeFile,
    rva: u32,
    size: usize,
    ops: &mut Vec<(u32, i32, u32, u32)>,
    names: &mut Vec<String>,
) -> Result<usize, String> {
    let code = rv(pe, rva, size)?;
    let base = pe.base;
    let mut reg = [SymVal::U; 8];
    reg[4] = SymVal::Stk(0);
    reg[1] = SymVal::This(0);
    let mut stk: HashMap<i32, SymVal> = HashMap::new();
    stk.insert(4, SymVal::Arg);

    let modrm = |i: usize| -> Result<(u8, usize, Option<usize>, i32, usize), String> {
        let m = *code
            .get(i)
            .ok_or_else(|| format!("mn modrm out of bounds at {:x}", (rva as u64 + i as u64)))?;
        let mod_bits = m >> 6;
        let r = ((m >> 3) & 7) as usize;
        let rm = (m & 7) as usize;
        let mut n = 1usize;
        let mut b = Some(rm);
        let mut disp = 0i32;
        if mod_bits == 3 {
            return Ok((mod_bits, r, Some(rm), 0, 1));
        }
        if rm == 4 {
            let sib = *code
                .get(i + 1)
                .ok_or_else(|| format!("mn sib out of bounds at {:x}", (rva as u64 + i as u64)))?;
            n += 1;
            if ((sib >> 3) & 7) != 4 {
                return Err(format!("index reg at {:x}", (rva as u64 + i as u64)));
            }
            let base_reg = (sib & 7) as usize;
            if mod_bits == 0 && base_reg == 5 {
                return Err(format!("sib disp32 at {:x}", (rva as u64 + i as u64)));
            }
            b = Some(base_reg);
        } else if mod_bits == 0 && rm == 5 {
            let d = read_i32_le(code, i + 1)?;
            return Ok((mod_bits, r, None, d, 5));
        }
        if mod_bits == 1 {
            disp = read_i8(code, i + n)? as i32;
            n += 1;
        } else if mod_bits == 2 {
            disp = read_i32_le(code, i + n)?;
            n += 4;
        }
        Ok((mod_bits, r, b, disp, n))
    };

    let addr = |reg: &[SymVal; 8], b: Option<usize>, disp: i32| -> Result<SymVal, String> {
        let idx = b.ok_or_else(|| "address from None".to_string())?;
        match reg[idx] {
            SymVal::This(off) => Ok(SymVal::This(off.wrapping_add(disp))),
            SymVal::Stk(off) => Ok(SymVal::Stk(off.wrapping_add(disp))),
            other => Err(format!("address from {:?}", other)),
        }
    };

    let write = |stk: &mut HashMap<i32, SymVal>,
                 ops: &mut Vec<(u32, i32, u32, u32)>,
                 a: SymVal,
                 v: SymVal|
     -> Result<(), String> {
        match a {
            SymVal::Stk(off) => {
                stk.insert(off, v);
                Ok(())
            }
            SymVal::This(off) => match v {
                SymVal::Const(c) => {
                    ops.push((MN_ST_CONST, off, c as u32, 0));
                    Ok(())
                }
                SymVal::Temp(t) => {
                    ops.push((MN_ST_TEMP, off, t, 0));
                    Ok(())
                }
                SymVal::This(target_off) => {
                    ops.push((MN_ST_ADDR, off, target_off as u32, 0));
                    Ok(())
                }
                SymVal::Arg => {
                    ops.push((MN_ST_ARG, off, 0, 0));
                    Ok(())
                }
                other => Err(format!("store of {:?}", other)),
            },
            other => Err(format!("invalid write destination {:?}", other)),
        }
    };

    let push =
        |reg: &mut [SymVal; 8], stk: &mut HashMap<i32, SymVal>, v: SymVal| -> Result<(), String> {
            let sp = match reg[4] {
                SymVal::Stk(off) => off.wrapping_sub(4),
                other => return Err(format!("push with invalid esp {:?}", other)),
            };
            reg[4] = SymVal::Stk(sp);
            stk.insert(sp, v);
            Ok(())
        };

    let arg = |reg: &[SymVal; 8], stk: &HashMap<i32, SymVal>, k: i32| -> Result<SymVal, String> {
        let sp = match reg[4] {
            SymVal::Stk(off) => off,
            other => return Err(format!("arg with invalid esp {:?}", other)),
        };
        Ok(*stk
            .get(&sp.wrapping_add(k.wrapping_mul(4)))
            .unwrap_or(&SymVal::U))
    };

    let mut i = 0usize;
    while i < size {
        let op = code[i];
        if (0x50..=0x57).contains(&op) {
            let v = reg[(op - 0x50) as usize];
            push(&mut reg, &mut stk, v)?;
            i += 1;
        } else if (0x58..=0x5f).contains(&op) {
            let top = arg(&reg, &stk, 0)?;
            reg[(op - 0x58) as usize] = top;
            if let SymVal::Stk(sp) = reg[4] {
                reg[4] = SymVal::Stk(sp.wrapping_add(4));
            }
            i += 1;
        } else if op == 0x68 {
            let imm = read_i32_le(code, i + 1)?;
            push(&mut reg, &mut stk, SymVal::Const(imm))?;
            i += 5;
        } else if op == 0x6a {
            let imm = read_i8(code, i + 1)? as i32;
            push(&mut reg, &mut stk, SymVal::Const(imm))?;
            i += 2;
        } else if (0xb8..=0xbf).contains(&op) {
            let imm = read_i32_le(code, i + 1)?;
            reg[(op - 0xb8) as usize] = SymVal::Const(imm);
            i += 5;
        } else if op == 0x8b || op == 0x89 || op == 0x8d || op == 0x33 || op == 0xc7 {
            let (mod_bits, r, b, disp, mut n) = modrm(i + 1)?;
            if op == 0x33 {
                if mod_bits != 3 || b != Some(r) {
                    return Err(format!("xor at {:x}", (rva as u64 + i as u64)));
                }
                reg[r] = SymVal::Const(0);
            } else if op == 0x8d {
                reg[r] = addr(&reg, b, disp)?;
            } else if op == 0xc7 {
                let target = addr(&reg, b, disp)?;
                let imm = read_i32_le(code, i + 1 + n)?;
                write(&mut stk, ops, target, SymVal::Const(imm))?;
                n += 4;
            } else if mod_bits == 3 {
                let b_idx = b.unwrap();
                if op == 0x8b {
                    reg[r] = reg[b_idx];
                } else {
                    reg[b_idx] = reg[r];
                }
            } else if op == 0x8b {
                if b.is_none() || reg[b.unwrap()] == SymVal::U {
                    reg[r] = SymVal::U;
                } else {
                    let a = addr(&reg, b, disp)?;
                    match a {
                        SymVal::Stk(off) => {
                            reg[r] = *stk.get(&off).unwrap_or(&SymVal::U);
                        }
                        other => {
                            return Err(format!(
                                "read of {:?} at {:x}",
                                other,
                                (rva as u64 + i as u64)
                            ));
                        }
                    }
                }
            } else {
                if b.is_none() {
                    return Err(format!("absolute store at {:x}", (rva as u64 + i as u64)));
                }
                let target = addr(&reg, b, disp)?;
                write(&mut stk, ops, target, reg[r])?;
            }
            i += 1 + n;
        } else if op == 0x81 || op == 0x83 {
            let (mod_bits, r, b, _disp, mut n) = modrm(i + 1)?;
            if mod_bits != 3 {
                return Err(format!("alu mem at {:x}", (rva as u64 + i as u64)));
            }
            let imm = if op == 0x83 {
                let v = read_i8(code, i + 1 + n)? as i32;
                n += 1;
                v
            } else {
                let v = read_i32_le(code, i + 1 + n)?;
                n += 4;
                v
            };
            let b_idx = b.unwrap();
            let v = reg[b_idx];
            if r == 1 && imm == -1 {
                reg[b_idx] = SymVal::Const(-1);
            } else if r == 0 || r == 5 {
                let delta = if r == 0 { imm } else { imm.wrapping_neg() };
                match v {
                    SymVal::Const(c) => reg[b_idx] = SymVal::Const(c.wrapping_add(delta)),
                    SymVal::Stk(s) => reg[b_idx] = SymVal::Stk(s.wrapping_add(delta)),
                    _ => return Err(format!("alu at {:x}", (rva as u64 + i as u64))),
                }
            } else {
                return Err(format!("alu at {:x}", (rva as u64 + i as u64)));
            }
            i += 1 + n;
        } else if op == 0xe8 {
            let rel = read_i32_le(code, i + 1)?;
            let t = ((rva as i64) + (i as i64) + 5 + (rel as i64)) as u32;
            if t == MN_CALL_ADD {
                let s = arg(&reg, &stk, 0)?;
                let c = match s {
                    SymVal::Const(c) => c,
                    _ => return Err(format!("Add arg at {:x}", (rva as u64 + i as u64))),
                };
                let p = (c as u32).wrapping_sub(base);
                let mut e = p;
                while rv(pe, e, 1)?[0] != 0 {
                    e += 1;
                }
                let slice = rv(pe, p, (e - p) as usize)?;
                let name: String = slice.iter().map(|&b| b as char).collect();
                names.push(name);
                let idx = (names.len() - 1) as u32;
                ops.push((MN_ADD, 0, idx, 0));
                if let SymVal::Stk(sp) = reg[4] {
                    reg[4] = SymVal::Stk(sp.wrapping_add(4));
                }
                reg[0] = SymVal::Temp(idx);
            } else if t == MN_CALL_RECT {
                let obj = reg[1];
                let a = [
                    arg(&reg, &stk, 0)?,
                    arg(&reg, &stk, 1)?,
                    arg(&reg, &stk, 2)?,
                    arg(&reg, &stk, 3)?,
                    arg(&reg, &stk, 4)?,
                    arg(&reg, &stk, 5)?,
                ];
                if let SymVal::Stk(sp) = reg[4] {
                    reg[4] = SymVal::Stk(sp.wrapping_add(24));
                }
                let offsets = [0x14, 0, 4, 8, 0xc, 0x10];
                for (&off, &v) in offsets.iter().zip(a.iter()) {
                    let dst = match obj {
                        SymVal::This(base_off) => SymVal::This(base_off.wrapping_add(off)),
                        SymVal::Stk(base_off) => SymVal::Stk(base_off.wrapping_add(off)),
                        other => return Err(format!("rect obj {:?}", other)),
                    };
                    write(&mut stk, ops, dst, v)?;
                }
                reg[0] = obj;
            } else if t == MN_CALL_CHKSTK {
                let sz = match reg[0] {
                    SymVal::Const(c) => c,
                    _ => return Err("chkstk size".to_string()),
                };
                if let SymVal::Stk(sp) = reg[4] {
                    reg[4] = SymVal::Stk(sp.wrapping_sub(sz));
                }
                reg[0] = SymVal::U;
            } else {
                return Err(format!("call {:x} at {:x}", t, (rva as u64 + i as u64)));
            }
            if t != MN_CALL_CHKSTK {
                reg[1] = SymVal::U;
                reg[2] = SymVal::U;
            }
            i += 5;
        } else if op == 0xff && code.get(i + 1) == Some(&0x92) {
            ops.push((MN_FILENAME, 0, 0, 0));
            if let SymVal::Stk(sp) = reg[4] {
                reg[4] = SymVal::Stk(sp.wrapping_add(12));
            }
            reg[0] = SymVal::U;
            reg[1] = SymVal::U;
            reg[2] = SymVal::U;
            i += 6;
        } else if op == 0xf3 && code.get(i + 1) == Some(&0xab) {
            let (d_off, count, val) = match (reg[7], reg[1], reg[0]) {
                (SymVal::This(d_off), SymVal::Const(count), SymVal::Const(val)) => {
                    (d_off, count, val)
                }
                _ => return Err("rep stosd".to_string()),
            };
            ops.push((MN_MEMSET, d_off, count as u32, val as u32));
            reg[7] = SymVal::This(d_off.wrapping_add(count.wrapping_mul(4)));
            reg[1] = SymVal::Const(0);
            i += 2;
        } else if op == 0xc2 || op == 0xc3 {
            return Ok(i + if op == 0xc2 { 3 } else { 1 });
        } else {
            return Err(format!(
                "opcode {:02x} at {:x}",
                op,
                (rva as u64 + i as u64)
            ));
        }
    }
    Err("no ret".to_string())
}

fn mn_ret01(pe: &PeFile, va: u32) -> Result<u32, String> {
    let b = rv(
        pe,
        va.checked_sub(pe.base)
            .ok_or_else(|| format!("vtable fn {:x} below image base", va))?,
        6,
    )?;
    if &b[..3] == b"\x33\xc0\xc3" {
        return Ok(0);
    }
    if b == b"\xb8\x01\x00\x00\x00\xc3" {
        return Ok(1);
    }
    Err(format!("vtable fn {:x}", va))
}

/// One decoded model-node program step `(op, offset, a, b)`.
pub type MnOp = (u32, i32, u32, u32);

/// Extracts `CModelNodeTable` ctor (`0x10e0`) and `Init` (`0x1980`) op program and animation names.
/// Returns `(ctor_sha256, init_sha256, ops, names)`.
pub fn modelnodes(pe: &PeFile) -> Result<(String, String, Vec<MnOp>, Vec<String>), RomError> {
    let ctor_sha = check_hash("modelnodes.ctor", pe.rva(MN_CTOR.0, MN_CTOR.1)?, H_MN_CTOR)?;
    let init_sha = check_hash("modelnodes.init", pe.rva(MN_INIT.0, MN_INIT.1)?, H_MN_INIT)?;
    let (ops, names) = modelnodes_decode(pe, MN_CTOR, MN_INIT)?;
    Ok((ctor_sha, init_sha, ops, names))
}

/// Decoder body of [`modelnodes`] for explicit `(rva, size)` code ranges, without the pinned-hash check.
pub fn modelnodes_decode(
    pe: &PeFile,
    ctor: (u32, usize),
    init: (u32, usize),
) -> Result<(Vec<MnOp>, Vec<String>), RomError> {
    named("modelnodes", modelnodes_inner(pe, ctor, init))
}

fn modelnodes_inner(
    pe: &PeFile,
    ctor: (u32, usize),
    init: (u32, usize),
) -> S<(Vec<MnOp>, Vec<String>)> {
    let mut ops = Vec::new();
    let mut names = Vec::new();
    mn_run(pe, ctor.0, ctor.1, &mut ops, &mut names)?;

    let mut out = Vec::with_capacity(ops.len() + 8600);
    let vt_hi = (pe.base as u64) + 0x0100_0000;
    for o in ops {
        if o.0 == MN_ST_CONST && o.2 >= pe.base && (o.2 as u64) < vt_hi {
            let vt_bytes = rv(pe, o.2 - pe.base, 8)?;
            let vt0 = read_u32_le(vt_bytes, 0)?;
            let vt1 = read_u32_le(vt_bytes, 4)?;
            let kind = mn_ret01(pe, vt0)? | (mn_ret01(pe, vt1)? << 1);
            out.push((MN_ST_KIND, o.1, kind, 0));
        } else {
            out.push(o);
        }
    }
    out.push((MN_CTOR_END, 0, 0, 0));
    mn_run(pe, init.0, init.1, &mut out, &mut names)?;
    Ok((out, names))
}

// ---------------------------------------------------------------------------------------------------------
// rendervars
// ---------------------------------------------------------------------------------------------------------

/// Renderer ConVar record: `(name, type_str, default_float, int_val)`.
pub type RenderVar = (String, String, f32, i32);

/// Extracts the Engine renderer ConVar table (`0x2739a8` x 184 + `0x275448` x 1).
/// Returns `(range_sha256s, vars)`.
pub fn rendervars(pe: &PeFile) -> Result<(Vec<String>, Vec<RenderVar>), RomError> {
    const RV_RANGES: [(u32, usize); 2] = [(0x2739a8, 184), (0x275448, 1)];
    let mut hs = Vec::with_capacity(RV_RANGES.len());
    for (k, &(rva, n)) in RV_RANGES.iter().enumerate() {
        let b = pe.rva(rva, n * 24)?;
        hs.push(check_hash("rendervars", b, H_RV_RANGES[k])?);
    }
    let out = rendervars_decode(pe, &RV_RANGES)?;
    Ok((hs, out))
}

/// Decoder body of [`rendervars`] for explicit `(rva, count)` ranges of 24-byte records.
pub fn rendervars_decode(pe: &PeFile, ranges: &[(u32, usize)]) -> Result<Vec<RenderVar>, RomError> {
    named("rendervars", rendervars_inner(pe, ranges))
}

fn rendervars_inner(pe: &PeFile, ranges: &[(u32, usize)]) -> S<Vec<RenderVar>> {
    let mut out = Vec::new();
    for &(rva, n) in ranges {
        let len = n
            .checked_mul(24)
            .ok_or_else(|| "range too large".to_string())?;
        let b = rv(pe, rva, len)?;
        for i in 0..n {
            let base = 24 * i;
            let vt = read_u32_le(b, base)?;
            let df = f32::from_bits(read_u32_le(b, base + 4)?);
            let nm = read_u32_le(b, base + 8)?;
            let iv = read_i32_le(b, base + 20)?;
            let vt_name = match vt {
                0x6860c8 => "int",
                0x6860c0 => "float",
                0x6860d0 => "other",
                other => return Err(format!("unknown rendervar vtable {:#x}", other)),
            };
            out.push((cs(pe, nm)?, vt_name.to_string(), df, iv));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------
// enginevars
// ---------------------------------------------------------------------------------------------------------

/// Engine variable record: `(name, type_str, optional_value)`.
pub type EngineVar = (String, String, Option<f64>);

/// Extracts the Engine variable table (`0x26b2a8` x 102).
/// Returns `(table_sha256, values_sha256, vars)`.
pub fn enginevars(pe: &PeFile) -> Result<(String, String, Vec<EngineVar>), RomError> {
    const RVA: u32 = 0x26b2a8;
    const N: usize = 102;
    let b = pe.rva(RVA, N * 20)?;
    let table_sha = check_hash("enginevars.table", b, H_EV_TABLE)?;
    let (vals, out) = enginevars_decode(pe, b)?;
    let values_sha = check_hash("enginevars.values", &vals, H_EV_VALUES)?;
    Ok((table_sha, values_sha, out))
}

/// Decoder body of [`enginevars`] for a raw table of 20-byte records. Returns `(value_bytes, vars)`.
pub fn enginevars_decode(pe: &PeFile, table: &[u8]) -> Result<(Vec<u8>, Vec<EngineVar>), RomError> {
    named("enginevars", enginevars_inner(pe, table))
}

fn enginevars_inner(pe: &PeFile, b: &[u8]) -> S<(Vec<u8>, Vec<EngineVar>)> {
    let n = b.len() / 20;
    let mut vals = Vec::new();
    let mut out = Vec::with_capacity(n);

    for i in 0..n {
        let base = 20 * i;
        let nm = read_u32_le(b, base)?;
        let pf = read_u32_le(b, base + 4)?;
        let pl = read_u32_le(b, base + 8)?;
        let pc = read_u32_le(b, base + 12)?;
        let _ps = read_u32_le(b, base + 16)?;

        if pf != 0 || pl != 0 {
            let target_va = if pf != 0 { pf } else { pl };
            let v4 = rvz(pe, target_va, 4)?;
            vals.extend_from_slice(&v4);
            let arr: [u8; 4] = [v4[0], v4[1], v4[2], v4[3]];
            if pf != 0 {
                let val = f32::from_le_bytes(arr) as f64;
                out.push((cs(pe, nm)?, "float".to_string(), Some(val)));
            } else {
                let val = i32::from_le_bytes(arr) as f64;
                out.push((cs(pe, nm)?, "int".to_string(), Some(val)));
            }
        } else {
            let kind = if pc != 0 { "cvar" } else { "string" };
            out.push((cs(pe, nm)?, kind.to_string(), None));
        }
    }
    Ok((vals, out))
}

// ---------------------------------------------------------------------------------------------------------
// defbinds
// ---------------------------------------------------------------------------------------------------------

const DB_CODE: (u32, usize) = (0x1eca0d, 0x137);
const DB_STR: (u32, usize) = (0x3eeb0c, 0x4e);

/// `(code_sha256, strings_sha256, [(command_id, default_binding_str)])`.
pub type DefBinds = (String, String, Vec<(i32, String)>);

/// Extracts the CShell `CUserProfile::LoadControls` default bindings (`0x1eca0d`, `0x3eeb0c`).
/// Returns a [`DefBinds`].
pub fn defbinds(pe: &PeFile) -> Result<DefBinds, RomError> {
    let code = pe.rva(DB_CODE.0, DB_CODE.1)?;
    let strs = pe.rva(DB_STR.0, DB_STR.1)?;
    let h1 = check_hash("defbinds.code", code, H_DB_CODE)?;
    let h2 = check_hash("defbinds.strings", strs, H_DB_STR)?;
    let out = defbinds_decode(pe, code, strs, DB_STR.0)?;
    Ok((h1, h2, out))
}

/// Decoder body of [`defbinds`]: `strs` is the string pool that starts at RVA `strs_rva`.
pub fn defbinds_decode(
    pe: &PeFile,
    code: &[u8],
    strs: &[u8],
    strs_rva: u32,
) -> Result<Vec<(i32, String)>, RomError> {
    named("defbinds", defbinds_inner(pe, code, strs, strs_rva))
}

fn defbinds_inner(pe: &PeFile, code: &[u8], strs: &[u8], strs_rva: u32) -> S<Vec<(i32, String)>> {
    let mut out = Vec::new();
    let mut cmp: Option<i32> = None;
    let mut i = 0usize;

    while i < code.len() {
        let op = code[i];
        let next = code.get(i + 1).copied();
        if op == 0x81 && next == Some(0xfa) {
            cmp = Some(read_i32_le(code, i + 2)?);
            i += 6;
        } else if op == 0x83 && next == Some(0xfa) {
            cmp = Some(read_i8(code, i + 2)? as i32);
            i += 3;
        } else if op == 0x68 {
            let va = read_u32_le(code, i + 1)?;
            let r = va.wrapping_sub(pe.base);
            let off = r.checked_sub(strs_rva).map(|o| o as usize);
            let (cmp_val, tail) = match (cmp, off.and_then(|o| strs.get(o..))) {
                (Some(v), Some(t)) => (v, t),
                _ => return Err(format!("defbinds: push at +{:x}", i)),
            };
            let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
            let s: String = tail[..end].iter().map(|&b| b as char).collect();
            out.push((cmp_val, s));
            cmp = None;
            i += 5;
        } else if matches!(op, 0x2b | 0x8a | 0x84 | 0x89 | 0x8d | 0xff) {
            i += 1 + modrm_len(code, i + 1)?;
        } else if op == 0x81 {
            i += 1 + modrm_len(code, i + 1)? + 4;
        } else if op == 0x83 {
            i += 1 + modrm_len(code, i + 1)? + 1;
        } else if (0x40..=0x57).contains(&op) {
            i += 1;
        } else if op == 0x75 || op == 0xeb {
            i += 2;
        } else if op == 0xe9 {
            i += 5;
        } else {
            return Err(format!("defbinds: opcode {:02x} at +{:x}", op, i));
        }
    }
    Ok(out)
}

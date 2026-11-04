//! Rust side of the Tier-1 differential harness.
//!
//! Produces the same files as the original Python reference dump (`entries.jsonl`, `decrypt.jsonl`,
//! `bute.jsonl`, `pe.json`) but through our crates (`dc-rez`, `dc-bute`, `dc-rom`), so a diff tests the crates.
//! Output is ROM-derived (paths, offsets, sizes, hashes): never commit it.
//!
//! Canonical JSON: sorted object keys, compact separators, UTF-8, LF line ends; hashes are SHA-256 over such
//! bytes. Floats in hashed table output are replaced by the string `f32:<8 hex of the f32 bits>`.
//! HARNESS: rust_refdump.py (record fields, ordering and canonical form).

use dc_bute::{ButeDoc, ParseOptions};
use dc_rez::{is_plain, walk_rez, Blowfish};
use dc_rom::{sha256_hex, PeFile, Sha256};
use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Chunk size for streaming; a multiple of 8 so cipher blocks never straddle chunks.
const CHUNK: usize = 8 << 20;

/// Canonical JSON bytes of a value. `serde_json::Value` objects are `BTreeMap`s, so keys come out sorted.
pub fn canon(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("json serialisation")
}

fn sha(b: &[u8]) -> String {
    sha256_hex(b)
}

fn hex(d: [u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 over data with trailing NUL bytes removed, computed incrementally.
struct StripHash {
    h: Sha256,
    zeros: usize,
}

impl StripHash {
    fn new() -> Self {
        Self {
            h: Sha256::new(),
            zeros: 0,
        }
    }
    fn feed(&mut self, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        let keep = b.iter().rposition(|&c| c != 0).map_or(0, |p| p + 1);
        if keep > 0 {
            const Z: [u8; 4096] = [0; 4096];
            let mut n = self.zeros;
            while n > 0 {
                let k = n.min(Z.len());
                self.h.update(&Z[..k]);
                n -= k;
            }
            self.h.update(&b[..keep]);
            self.zeros = b.len() - keep;
        } else {
            self.zeros += b.len();
        }
    }
    fn hex(self) -> String {
        hex(self.h.finalize())
    }
}

/// Every `.rez` under the ROM, sorted by lower-cased relative path with `/` separators.
fn find_rez(rom: &Path) -> Res<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![rom.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let p = e.path();
            if e.file_type()?.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rez")) {
                let rel = p.strip_prefix(rom)?.to_string_lossy().replace('\\', "/");
                out.push((rel, p));
            }
        }
    }
    out.sort_by_key(|(r, _)| r.to_lowercase());
    Ok(out)
}

struct Streamed {
    plain: bool,
    sha_raw: String,
    sha_dec: String,
    data: Option<Vec<u8>>,
}

/// Reads one entry in chunks: raw hash, decrypted hash (trailing NULs stripped) and optionally the plaintext.
fn process_entry(
    f: &mut File,
    off: u64,
    size: u32,
    cipher: &Blowfish,
    want: bool,
) -> Res<Streamed> {
    f.seek(SeekFrom::Start(off))?;
    let mut head = vec![0u8; (size as usize).min(64)];
    f.read_exact(&mut head)?;
    let plain = is_plain(&head);
    f.seek(SeekFrom::Start(off))?;
    let mut raw_h = Sha256::new();
    let mut dec_h = StripHash::new();
    let mut keep = if want { Some(Vec::new()) } else { None };
    let mut left = size as usize;
    let mut buf = vec![0u8; CHUNK.min(left.max(1))];
    while left > 0 {
        let n = left.min(CHUNK);
        f.read_exact(&mut buf[..n])?;
        left -= n;
        raw_h.update(&buf[..n]);
        // every chunk but the last is a multiple of 8; the last one carries the plain tail, which
        // decrypt_bytes leaves untouched.
        let d: Vec<u8>;
        let slice: &[u8] = if plain {
            &buf[..n]
        } else {
            d = cipher.decrypt_bytes(&buf[..n]);
            &d
        };
        dec_h.feed(slice);
        if let Some(k) = keep.as_mut() {
            k.extend_from_slice(slice);
        }
    }
    Ok(Streamed {
        plain,
        sha_raw: hex(raw_h.finalize()),
        sha_dec: dec_h.hex(),
        data: keep,
    })
}

fn bute_sections(doc: &ButeDoc) -> Value {
    let secs: Vec<Value> = doc
        .order
        .iter()
        .map(|name| {
            let kv: Vec<Value> = doc.sections[name]
                .iter()
                .map(|(k, v)| json!([k, v]))
                .collect();
            json!([name, kv])
        })
        .collect();
    Value::Array(secs)
}

/// Counts reported by [`run`].
#[derive(Default, Debug, Clone)]
pub struct Counts {
    pub entries: u64,
    pub decrypt: u64,
    pub bute: u64,
    pub plain_entries: u64,
    pub pe: u64,
}

impl Counts {
    pub fn to_json(&self) -> String {
        let v = json!({"entries": self.entries, "decrypt": self.decrypt, "bute": self.bute,
                       "plain_entries": self.plain_entries, "pe": self.pe});
        String::from_utf8(canon(&v)).unwrap_or_default()
    }
}

fn line_writer(out: &Path, name: &str, on: bool) -> Res<Option<BufWriter<File>>> {
    Ok(if on {
        Some(BufWriter::new(File::create(out.join(name))?))
    } else {
        None
    })
}

fn put(w: &mut Option<BufWriter<File>>, v: &Value) -> Res<()> {
    if let Some(w) = w {
        w.write_all(&canon(v))?;
        w.write_all(b"\n")?;
    }
    Ok(())
}

/// Dumps the selected parts (`entries`, `decrypt`, `bute`, `pe`, comma separated) of the ROM into `out`.
pub fn run(rom: &Path, out: &Path, only: &str) -> Res<Counts> {
    let rom = std::fs::canonicalize(rom)?;
    std::fs::create_dir_all(out)?;
    let out = std::fs::canonicalize(out)?;
    if out.starts_with(&rom) {
        return Err("refusing to write inside the ROM".into());
    }
    let sel: Vec<&str> = only.split(',').collect();
    let has = |n: &str| sel.contains(&n);
    let mut c = Counts::default();

    if has("entries") || has("decrypt") || has("bute") {
        let cipher = Blowfish::new(&dc_rom::crypt_key(&rom)?)?;
        let mut ent_f = line_writer(&out, "entries.jsonl", has("entries"))?;
        let mut dec_f = line_writer(&out, "decrypt.jsonl", has("decrypt"))?;
        let mut but_f = line_writer(&out, "bute.jsonl", has("bute"))?;
        for (rez, rp) in find_rez(&rom)? {
            let ents = walk_rez(&rp)?;
            let mut f = File::open(&rp)?;
            for (path, off, size) in ents {
                c.entries += 1;
                put(
                    &mut ent_f,
                    &json!({"rez": rez, "path": path, "off": off, "size": size}),
                )?;
                if dec_f.is_none() && but_f.is_none() {
                    continue;
                }
                let ext = path.to_lowercase();
                let ext = ext.rsplit('.').next().unwrap_or("").to_string();
                let want = but_f.is_some() && (ext == "txt" || ext == "gms");
                let s = process_entry(&mut f, off, size, &cipher, want)?;
                c.plain_entries += s.plain as u64;
                if dec_f.is_some() {
                    put(
                        &mut dec_f,
                        &json!({"rez": rez, "path": path, "plain": s.plain,
                                            "sha_raw": s.sha_raw, "sha_dec": s.sha_dec}),
                    )?;
                    c.decrypt += 1;
                }
                if let Some(data) = s.data {
                    let stripped = doc_parse(&data, true);
                    let unstripped = doc_parse(&data, false);
                    let keys: usize = unstripped.sections.values().map(|m| m.len()).sum();
                    put(
                        &mut but_f,
                        &json!({"path": path, "rez": rez, "sections": unstripped.order.len(),
                                            "keys": keys,
                                            "sha_stripped": sha(&canon(&bute_sections(&stripped))),
                                            "sha_unstripped": sha(&canon(&bute_sections(&unstripped)))}),
                    )?;
                    c.bute += 1;
                }
            }
        }
        for w in [ent_f, dec_f, but_f].into_iter().flatten() {
            w.into_inner().map_err(|e| e.to_string())?.sync_all()?;
        }
    }
    if has("pe") {
        let rows = pe_tables(&rom)?;
        c.pe = rows.len() as u64;
        let mut b = canon(&Value::Array(rows));
        b.push(b'\n');
        std::fs::write(out.join("pe.json"), b)?;
    }
    Ok(c)
}

fn doc_parse(data: &[u8], strip: bool) -> ButeDoc {
    ButeDoc::parse_bytes_latin1_with(
        data,
        ParseOptions {
            strip_section_names: strip,
        },
    )
}

/// Floats become `f32:<bits>` strings so canonical JSON never holds NaN or infinities.
fn f32s(x: f64) -> Value {
    Value::String(format!("f32:{:08x}", (x as f32).to_bits()))
}

fn strs(v: &[String]) -> Value {
    Value::Array(v.iter().map(|s| Value::String(s.clone())).collect())
}

fn row(name: &str, code: String, count: usize, out: Value) -> Value {
    json!({"name": name, "code_sha": code, "count": count, "sha_output": sha(&canon(&out))})
}

fn pe_tables(rom: &Path) -> Res<Vec<Value>> {
    let en = PeFile::from_file(&rom.join("Engine.exe"))?;
    let cs = PeFile::from_file(&rom.join("Game").join("CShell.dll"))?;
    let mut rows = Vec::new();

    let (h, keys) = dc_rom::chatfilter(&cs)?; // output row keeps the harness name "wordfilter"
    rows.push(row("wordfilter", h, keys.len(), strs(&keys)));

    let (h, lines) = dc_rom::defcfg(&en)?;
    rows.push(row("defcfg", h, lines.len(), strs(&lines)));

    let (h, recs) = dc_rom::cmdtab(&cs)?;
    rows.push(row("cmdtab", h, recs.len(), json!(recs)));

    let (c1, c2, ops, names) = dc_rom::modelnodes(&cs)?;
    let opsv: Vec<Value> = ops
        .iter()
        .map(|&(a, b, c, d)| json!([a, b, c, d]))
        .collect();
    rows.push(row(
        "modelnodes",
        format!("{c1}+{c2}"),
        ops.len(),
        json!([opsv, strs(&names)]),
    ));

    let (hs, rv) = dc_rom::rendervars(&en)?;
    let rvv: Vec<Value> = rv
        .iter()
        .map(|(n, t, f, i)| json!([n, t, f32s(*f as f64), i]))
        .collect();
    rows.push(row("rendervars", hs.join("+"), rv.len(), Value::Array(rvv)));

    let (h1, h2, ev) = dc_rom::enginevars(&en)?;
    let evv: Vec<Value> = ev
        .iter()
        .map(|(n, t, v)| {
            json!([
                n,
                t,
                v.map_or(Value::Null, |x| if t == "int" {
                    json!(x as i64)
                } else {
                    f32s(x)
                })
            ])
        })
        .collect();
    rows.push(row(
        "enginevars",
        format!("{h1}+{h2}"),
        ev.len(),
        Value::Array(evv),
    ));

    let (h1, h2, db) = dc_rom::defbinds(&cs)?;
    let dbv: Vec<Value> = db.iter().map(|(i, s)| json!([i, s])).collect();
    rows.push(row(
        "defbinds",
        format!("{h1}+{h2}"),
        db.len(),
        Value::Array(dbv),
    ));
    Ok(rows)
}

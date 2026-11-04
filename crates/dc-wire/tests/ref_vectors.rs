//! Replays the Python reference TCP frames (HARNESS: rust_refdump.py, `wire_vectors.jsonl`: 1000 frames from
//! xorshift64* seed 20261024; ops u8/u16/u32/i32/i64/f32 bits/wstr/at) through `PktWriter` and asserts byte
//! equality. Gated: the file lives in `DC_REF_DIR` (default `ref` of the project) and the
//! test skips with a message when it is absent. Deliberate differences are listed in
//! `tests/ref_vectors_allow.txt` as `seed_index<TAB>reason`.

use dc_wire::PktWriter;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

const DEFAULT_REF_DIR: &str = "ref";

fn ref_file() -> Option<PathBuf> {
    let dir = std::env::var_os("DC_REF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REF_DIR));
    let f = dir.join("wire_vectors.jsonl");
    f.is_file().then_some(f)
}

fn allowlist() -> BTreeMap<u64, String> {
    let text = include_str!("ref_vectors_allow.txt");
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (idx, reason) = line
            .split_once('\t')
            .expect("allowlist line: seed_index<TAB>reason");
        assert!(
            !reason.trim().is_empty(),
            "allowlist entry {idx} has no reason"
        );
        m.insert(idx.trim().parse().expect("seed_index"), reason.to_string());
    }
    m
}

/// Builds the frame from the ops. `f32_via_float` selects `PktWriter::f32(f32::from_bits(bits))` instead of
/// the bit-exact `PktWriter::f32_bits(bits)`.
fn build(pid: u16, ops: &[Value], f32_via_float: bool) -> Vec<u8> {
    let mut w = PktWriter::new(pid);
    for op in ops {
        let kind = op["op"].as_str().expect("op");
        match kind {
            "u8" => {
                w.u8(op["v"].as_u64().unwrap() as u8);
            }
            "u16" => {
                w.u16(op["v"].as_u64().unwrap() as u16);
            }
            "u32" => {
                w.u32(op["v"].as_u64().unwrap() as u32);
            }
            "i32" => {
                w.i32(op["v"].as_i64().unwrap() as i32);
            }
            "i64" => {
                w.i64(op["v"].as_i64().unwrap());
            }
            "f32" => {
                let bits = op["bits"].as_u64().unwrap() as u32;
                if f32_via_float {
                    w.f32(f32::from_bits(bits));
                } else {
                    w.f32_bits(bits);
                }
            }
            "wstr" => {
                w.wstr(
                    op["s"].as_str().unwrap(),
                    op["n"].as_u64().unwrap() as usize,
                );
            }
            "at" => {
                w.at(op["off"].as_u64().unwrap() as usize);
            }
            other => panic!("unknown op {other}"),
        }
    }
    w.to_frame()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn python_reference_frames_match() {
    let Some(path) = ref_file() else {
        eprintln!(
            "SKIP python_reference_frames_match: wire_vectors.jsonl not found (set DC_REF_DIR)"
        );
        return;
    };
    let allow = allowlist();
    let text = std::fs::read_to_string(&path).expect("read wire_vectors.jsonl");
    let mut n = 0usize;
    let mut mismatched = Vec::new();
    let mut float_path_diffs = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).expect("json line");
        let idx = v["seed_index"].as_u64().expect("seed_index");
        let pid = v["pid"].as_u64().expect("pid") as u16;
        let ops = v["ops"].as_array().expect("ops");
        let want = v["frame_hex"].as_str().expect("frame_hex");
        n += 1;
        let got = hex(&build(pid, ops, false));
        if got != want && !allow.contains_key(&idx) {
            mismatched.push(idx);
        }
        // the float path is informational: it differs only where an f32 round trip changes NaN bits
        if hex(&build(pid, ops, true)) != want {
            float_path_diffs.push(idx);
        }
    }
    eprintln!(
        "replayed {n} reference frames; {} mismatches; {} allowlisted; f32-through-float path differs on {} frames",
        mismatched.len(),
        allow.len(),
        float_path_diffs.len()
    );
    assert_eq!(n, 1000, "expected 1000 reference frames");
    assert!(
        mismatched.is_empty(),
        "frames differ from the Python reference: {mismatched:?}"
    );
}

//! Tier-1 differential test: dump the ROM in-process through the Rust crates and compare record by record
//! with the Python reference dump. ROM-gated (skips when `CA_ROM` is unset) and reference-gated (skips when the
//! reference directory is missing). Failure messages name files, keys and field names, never values.
//! HARNESS: rust_refdump.py, rust_refdiff.py

use dc_rom::rom_or_skip;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DEFAULT_REF: &str = "ref";

fn ref_dir() -> PathBuf {
    std::env::var_os("DC_REF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_REF))
}

fn load_jsonl(p: &Path, key: &[&str]) -> BTreeMap<String, Value> {
    let text = std::fs::read_to_string(p).expect("read jsonl");
    let mut m = BTreeMap::new();
    for ln in text.split('\n').filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(ln).expect("json line");
        let k = key
            .iter()
            .map(|f| v[*f].as_str().unwrap_or("").to_string())
            .collect::<Vec<_>>()
            .join("/");
        m.insert(k, v);
    }
    m
}

fn diff_maps(
    name: &str,
    a: &BTreeMap<String, Value>,
    b: &BTreeMap<String, Value>,
    bad: &mut Vec<String>,
) {
    for (k, va) in a {
        match b.get(k) {
            None => bad.push(format!("{name}: {k} missing in rust")),
            Some(vb) => {
                let (oa, ob) = (va.as_object().unwrap(), vb.as_object().unwrap());
                let mut fields: Vec<&String> = oa
                    .keys()
                    .chain(ob.keys())
                    .filter(|f| oa.get(*f) != ob.get(*f))
                    .collect();
                fields.sort();
                fields.dedup();
                if !fields.is_empty() {
                    let f: Vec<&str> = fields.iter().map(|s| s.as_str()).collect();
                    bad.push(format!("{name}: {k} mismatch: {}", f.join(",")));
                }
            }
        }
    }
    for k in b.keys() {
        if !a.contains_key(k) {
            bad.push(format!("{name}: {k} extra in rust"));
        }
    }
}

#[test]
fn rust_dump_matches_python_reference() {
    let rom = rom_or_skip!();
    let refd = ref_dir();
    if !refd.join("entries.jsonl").is_file() {
        println!("skipped: no reference dump at the DC_REF_DIR location");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("dc-xtask-refdump-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let counts = dc_xtask::run(&rom, &tmp, "entries,decrypt,bute,pe").expect("dump");
    let mut bad = Vec::new();
    let mut compared = 0usize;
    for (file, key) in [
        ("entries.jsonl", ["rez", "path"]),
        ("decrypt.jsonl", ["rez", "path"]),
        ("bute.jsonl", ["rez", "path"]),
    ] {
        let a = load_jsonl(&refd.join(file), &key);
        let b = load_jsonl(&tmp.join(file), &key);
        compared += a.len();
        diff_maps(file, &a, &b, &mut bad);
    }
    let pe = |d: &Path| -> BTreeMap<String, Value> {
        let v: Value =
            serde_json::from_slice(&std::fs::read(d.join("pe.json")).expect("pe.json")).unwrap();
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| (r["name"].as_str().unwrap().to_string(), r.clone()))
            .collect()
    };
    let (pa, pb) = (pe(&refd), pe(&tmp));
    compared += pa.len();
    diff_maps("pe.json", &pa, &pb, &mut bad);
    let _ = std::fs::remove_dir_all(&tmp);
    println!(
        "compared {compared} reference records ({counts:?}), {} problems",
        bad.len()
    );
    assert!(bad.is_empty(), "differences:\n{}", bad.join("\n"));
}

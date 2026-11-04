# DIFF_ALLOWLIST.md: deliberate differences between the Rust dump and the Python reference

Status 2026-10-24: zero field-level differences in entries.jsonl (15,107), decrypt.jsonl (15,107), bute.jsonl (199)
and pe.json (7 tables). Only file-level entries are listed here.

| item | difference | reason |
|---|---|---|
| wire_vectors.jsonl | present in the reference dir, not produced by `refdump` | The wire layer (dc-wire udp/msg_ids) is being rewritten by another worker; the wire vectors get their own Rust generator then. `rust_refdiff.py` reports "only in ref" for this file until that exists. |
| pe.json enginevars, integer-kind rows | emitted as JSON integers (kind "int"), float-kind rows as `f32:<bits>` strings | Mirrors how the reference serialises the two kinds; not a crate difference. |

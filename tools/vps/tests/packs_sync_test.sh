#!/usr/bin/env bash
# packs_sync_test.sh: tests of tools/vps/packs_sync.sh against a fake release folder (zips, packs.json, SHA256SUMS made
# here), on a temporary mods folder. No network, no root.
#   bash tools/vps/tests/packs_sync_test.sh
# Runs under Git Bash on Windows and on Linux. Needs python. Exit 0 = all passed.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VPS=$(cd "$HERE/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/dc-packs-sync-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
export DC_TEST=1 DC_ROOT="$T/opt"
PY=python3
python3 -c 'import zipfile' >/dev/null 2>&1 || PY=python
export DC_PYTHON=$PY

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$*"; }
check() { local d=$1; shift; if "$@"; then ok; else bad "$d"; fi; }
has() { if printf '%s\n' "$2" | grep -qF -- "$3"; then ok; else bad "$1: no '$3' in: $2"; fi; }
hasnt() { if printf '%s\n' "$2" | grep -qF -- "$3"; then bad "$1: unwanted '$3' in: $2"; else ok; fi; }
count_zips() { find "$1" -maxdepth 1 -name '*.zip' 2>/dev/null | wc -l | tr -d ' '; }

tr -d '\r' < "$VPS/packs_sync.sh" > "$T/packs_sync.sh"
tr -d '\r' < "$VPS/common.sh" > "$T/common.sh"
RUN="bash $T/packs_sync.sh"

# ---- the fake release: two packs, their zips, packs.json and SHA256SUMS
REL="$T/release"
mkdir -p "$REL"
"$PY" - "$REL" <<'PY'
import hashlib, json, os, sys, zipfile
rel = sys.argv[1]
rows = []
for mid, ver, body in (("camo_a", "1.0.0", b"a" * 300), ("reel_b", "0.1.1", b"b" * 500)):
    name = f"{mid}-{ver}.zip"
    with zipfile.ZipFile(os.path.join(rel, name), "w") as z:
        z.writestr("manifest.json", json.dumps({"id": mid, "version": ver}))
        z.writestr("main.lua", body.decode())
    sha = hashlib.sha256(open(os.path.join(rel, name), "rb").read()).hexdigest()
    rows.append({"id": mid, "version": ver, "sha256": sha, "zip": name, "kind": "shared"})
json.dump({"version": 1, "packs": rows}, open(os.path.join(rel, "packs.json"), "w"), indent=1)
open(os.path.join(rel, "SHA256SUMS"), "w").write("".join(f"{r['sha256']}  {r['zip']}\n" for r in rows))
open(os.path.join(rel, "rows.txt"), "w").write("\n".join(f"{r['id']} {r['sha256']}" for r in rows))
PY
SHA_A=$(grep '^camo_a ' "$REL/rows.txt" | cut -d' ' -f2)
SHA_B=$(grep '^reel_b ' "$REL/rows.txt" | cut -d' ' -f2)

# ---- refusals before any fetch: a branch name, an alias, a bad repo, a bad tag
for t in main master latest HEAD; do
    out=$($RUN example/packs "$t" --base-url "$REL" --mods-dir "$T/m0" 2>&1); rc=$?
    check "tag '$t' refused with exit 2 (rc $rc)" test "$rc" -eq 2
    has "tag '$t' says why" "$out" "is a branch or an alias"
done
check "a refused tag fetched and installed nothing" test ! -d "$T/m0"
out=$($RUN "not a repo" v1 --base-url "$REL" --mods-dir "$T/m0" 2>&1); rc=$?
check "bad repo: exit 2" test "$rc" -eq 2
out=$($RUN example/packs "feature/x" --base-url "$REL" --mods-dir "$T/m0" 2>&1); rc=$?
check "a tag that is not v<n> or a date: exit 2" test "$rc" -eq 2

# ---- the happy path installs both under their hash and prints the rows
out=$($RUN example/packs v1 --base-url "$REL" --mods-dir "$T/m1" 2>&1); rc=$?
check "install: exit 0 ($out)" test "$rc" -eq 0
check "camo_a installed as <sha>.zip" test -f "$T/m1/$SHA_A.zip"
check "reel_b installed as <sha>.zip" test -f "$T/m1/$SHA_B.zip"
check "installed bytes are the release's" cmp -s "$T/m1/$SHA_A.zip" "$REL/camo_a-1.0.0.zip"
has "entry rows printed" "$out" "{ id = \"camo_a\", version = \"1.0.0\", sha256 = \"$SHA_A\" },"
has "entry rows printed (b)" "$out" "{ id = \"reel_b\", version = \"0.1.1\", sha256 = \"$SHA_B\" },"
check "exactly two zips" test "$(count_zips "$T/m1")" = 2

# ---- a re-run is idempotent: everything present, nothing rewritten
touch -d '2001-01-01 00:00:00' "$T/m1/$SHA_A.zip" 2>/dev/null || touch -t 200101010000 "$T/m1/$SHA_A.zip"
before=$(ls -l --time-style=+%s "$T/m1/$SHA_A.zip" 2>/dev/null | awk '{print $6}')
out=$($RUN example/packs v1 --base-url "$REL" --mods-dir "$T/m1" 2>&1); rc=$?
check "re-run: exit 0" test "$rc" -eq 0
has "re-run says present" "$out" "present  camo_a 1.0.0"
hasnt "re-run installs nothing" "$out" "installed camo_a"
after=$(ls -l --time-style=+%s "$T/m1/$SHA_A.zip" 2>/dev/null | awk '{print $6}')
check "re-run leaves the file untouched" test "$before" = "$after"
check "still two zips" test "$(count_zips "$T/m1")" = 2

# ---- --format set and --only
out=$($RUN example/packs v1 --base-url "$REL" --mods-dir "$T/m1" --format set --only reel_b 2>&1); rc=$?
check "--only --format set: exit 0" test "$rc" -eq 0
has "set block" "$out" "[[mods.set]]"
has "set block id" "$out" "id = \"reel_b\""
hasnt "--only leaves camo_a out" "$out" "camo_a"
out=$($RUN example/packs v1 --base-url "$REL" --mods-dir "$T/m1" --only nothing_here 2>&1); rc=$?
check "--only with an unknown id: exit 1" test "$rc" -eq 1
has "unknown id named" "$out" "nothing_here - - not in packs.json"

# ---- --dry-run installs nothing
out=$($RUN example/packs v1 --base-url "$REL" --mods-dir "$T/m2" --dry-run 2>&1); rc=$?
check "dry run: exit 0" test "$rc" -eq 0
has "dry run says would get" "$out" "would get camo_a 1.0.0"
check "dry run installed nothing" test ! -d "$T/m2"

# ---- a SHA256SUMS mismatch (the sums file lists another hash for reel_b) installs nothing at all
cp -r "$REL" "$T/rel_sums"
sed -i "s/$SHA_B/$(printf 'f%.0s' $(seq 1 64))/" "$T/rel_sums/SHA256SUMS"
out=$($RUN example/packs v1 --base-url "$T/rel_sums" --mods-dir "$T/m3" 2>&1); rc=$?
check "SHA256SUMS mismatch: exit 1" test "$rc" -eq 1
has "SHA256SUMS mismatch named" "$out" "SHA256SUMS says ffffffffffff, packs.json says"
has "SHA256SUMS mismatch: nothing installed" "$out" "nothing installed"
check "SHA256SUMS mismatch: no zip at all (camo_a was fine)" test "$(count_zips "$T/m3")" = 0

# ---- a packs.json row whose declared hash differs from the bytes (sums and bytes agree with each other) installs nothing
cp -r "$REL" "$T/rel_decl"
"$PY" - "$T/rel_decl" <<'PY'
import json, sys, os
p = os.path.join(sys.argv[1], "packs.json")
d = json.load(open(p))
d["packs"][0]["sha256"] = "e" * 64
json.dump(d, open(p, "w"))
PY
out=$($RUN example/packs v1 --base-url "$T/rel_decl" --mods-dir "$T/m4" 2>&1); rc=$?
check "declared-hash mismatch: exit 1" test "$rc" -eq 1
has "declared-hash mismatch named" "$out" "packs.json says eeeeeeeeeeee"
check "declared-hash mismatch: nothing installed" test "$(count_zips "$T/m4")" = 0

# ---- a zip whose bytes differ from what both files say (a swapped asset) installs nothing, not even the good pack
cp -r "$REL" "$T/rel_bytes"
printf 'tampered' >> "$T/rel_bytes/reel_b-0.1.1.zip"
out=$($RUN example/packs v1 --base-url "$T/rel_bytes" --mods-dir "$T/m5" 2>&1); rc=$?
check "tampered zip: exit 1" test "$rc" -eq 1
has "tampered zip named" "$out" "reel_b 0.1.1: reel_b-0.1.1.zip hashes"
has "tampered zip: camo_a was checked" "$out" "checked  camo_a 1.0.0"
check "tampered zip: nothing installed, camo_a included" test "$(count_zips "$T/m5")" = 0

# ---- a missing asset
cp -r "$REL" "$T/rel_missing"
rm "$T/rel_missing/camo_a-1.0.0.zip"
out=$($RUN example/packs v1 --base-url "$T/rel_missing" --mods-dir "$T/m6" 2>&1); rc=$?
check "missing zip: exit 1" test "$rc" -eq 1
has "missing zip named" "$out" "camo_a-1.0.0.zip is not in the release"
check "missing zip: nothing installed" test "$(count_zips "$T/m6")" = 0

# ---- no release of that tag
out=$($RUN example/packs v9 --base-url "$T/nowhere" --mods-dir "$T/m7" 2>&1); rc=$?
check "no release: exit 1" test "$rc" -eq 1
has "no release named" "$out" "SHA256SUMS of release v9 is not there"

printf 'packs_sync_test: %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]

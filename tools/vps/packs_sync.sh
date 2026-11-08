#!/usr/bin/env bash
# packs_sync.sh: install the packs of a pack repository's TAGGED release into a lobby's mods folder, each zip checked
# against the release's SHA256SUMS and against the hash its packs.json row declares, and installed as <sha256>.zip
# (docs/PACKS_FROM_REPOSITORIES.md section 2). Two phases: every selected zip is downloaded and checked first; one
# failure installs nothing. A zip already there under its hash is left alone, so a re-run changes nothing. The
# script never edits server.toml: it prints the rows to paste.
#
#   sudo bash /opt/decombat/tools/packs_sync.sh <owner/repo> <tag> [--only id,id,...] [--mods-dir DIR] [--format entry|set]
#                                               [--dry-run] [--base-url URL-or-folder]
#
# <tag> is a release tag (v1, v1.2.0, 2026-11-09): a branch name (main, master, latest, HEAD, ...) is refused, because a
# branch moves and a pack is its hash. --base-url replaces https://github.com/<owner/repo>/releases/download/<tag>
# (a folder works too: the tests use one). Exit 0 all installed or present, 1 a check failed (nothing installed), 2 usage.
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=common.sh
. "$HERE/common.sh"

usage() {
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

REPO=${1:-}
TAG=${2:-}
[ -n "$REPO" ] && [ -n "$TAG" ] || usage
shift 2
ONLY=""
MODS_DIR=""
FORMAT=entry
DRY=0
BASE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --only) ONLY=${2:-}; shift 2 ;;
        --mods-dir) MODS_DIR=${2:-}; shift 2 ;;
        --format) FORMAT=${2:-}; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        --base-url) BASE=${2:-}; shift 2 ;;
        -h|--help) usage ;;
        *) die "unknown argument $1 (see --help)" ;;
    esac
done

# ---- what may be fetched: owner/repo and a tag that is not a branch
printf '%s' "$REPO" | grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' || { echo "error: '$REPO' is not owner/repo" >&2; exit 2; }
case "$TAG" in
    main|master|latest|HEAD|head|develop|dev|trunk|release|stable|nightly)
        echo "error: '$TAG' is a branch or an alias, not a release tag: a branch moves, a pack is its hash (use the tag, e.g. v1)" >&2
        exit 2 ;;
esac
printf '%s' "$TAG" | grep -Eq '^v?[0-9][A-Za-z0-9._-]*$' || { echo "error: '$TAG' is not a release tag (v1, v1.2.0, 2026-11-09)" >&2; exit 2; }
case "$FORMAT" in entry|set) ;; *) die "--format entry|set" ;; esac
[ -n "$BASE" ] || BASE="https://github.com/$REPO/releases/download/$TAG"
[ -n "$MODS_DIR" ] || MODS_DIR="$DC_ROOT/data/mods"
[ "$DC_TEST" = 1 ] || [ "$DRY" = 1 ] || need_root

WORK=$(mktemp -d "${TMPDIR:-/tmp}/dc-packs-sync.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# fetch NAME: the release asset NAME into $WORK/NAME (a folder base is copied, anything else goes through curl)
fetch() {
    local name=$1
    if [ -d "$BASE" ]; then
        [ -f "$BASE/$name" ] && cp "$BASE/$name" "$WORK/$name"
    else
        curl -fsSL --max-time 600 -o "$WORK/$name" "$BASE/$name"
    fi
}
sha_of() { sha256sum "$1" | cut -d' ' -f1; }

say "release $REPO $TAG"
fetch SHA256SUMS || die "SHA256SUMS of release $TAG is not there: no release of that tag, or it has no SHA256SUMS"
fetch packs.json || die "packs.json of release $TAG is not there (the release was not made by the packs workflow)"

# ---- the rows: id, version, sha256, zip name; packs.json's hash must equal the SHA256SUMS line of the same zip
ROWS=$("$DC_PYTHON" - "$WORK/packs.json" "$WORK/SHA256SUMS" "$ONLY" <<'PY'
import json, re, sys
sys.stdout.reconfigure(newline="\n")  # LF only, also from a Windows python
packs = json.load(open(sys.argv[1], encoding="utf-8-sig"))
packs = packs["packs"] if isinstance(packs, dict) else packs
sums = {}
for line in open(sys.argv[2], encoding="utf-8"):
    parts = line.split()
    if len(parts) >= 2:
        sums[parts[1].lstrip("*")] = parts[0].lower()
only = [o for o in sys.argv[3].split(",") if o]
bad = 0
PLAIN = re.compile(r"^[A-Za-z0-9._-]{1,100}$")
ZIP = re.compile(r"^[A-Za-z0-9._-]{1,200}\.zip$")
for p in packs:
    # a remote row is a stranger's text: its id, version and zip name must be plain words before any of them is used
    # as a path or passed through the shell (no slash, no space, no newline, no leading dash)
    pid, ver = p.get("id"), p.get("version")
    if not (isinstance(pid, str) and PLAIN.match(pid) and not pid.startswith("-")
            and isinstance(ver, str) and PLAIN.match(ver) and not ver.startswith("-")):
        print(f"BAD ? ? ? a row has an id or version that is not a plain word: nothing fetched"); bad += 1; continue
    if only and pid not in only:
        continue
    zip_name = p.get("zip") or f'{pid}-{ver}.zip'
    if not (isinstance(zip_name, str) and ZIP.match(zip_name) and not zip_name.startswith("-") and ".." not in zip_name.split(".")):
        print(f"BAD {pid} {ver} ? the zip name is not a plain file name: nothing fetched"); bad += 1; continue
    declared = str(p.get("sha256", "")).lower()
    listed = sums.get(zip_name)
    if len(declared) != 64:
        print(f'BAD {p["id"]} {p["version"]} {zip_name} packs.json has no sha256'); bad += 1; continue
    if listed is None:
        print(f'BAD {p["id"]} {p["version"]} {zip_name} not in SHA256SUMS'); bad += 1; continue
    if listed != declared:
        print(f'BAD {p["id"]} {p["version"]} {zip_name} SHA256SUMS says {listed[:12]}, packs.json says {declared[:12]}'); bad += 1; continue
    print(f'ROW {p["id"]} {p["version"]} {zip_name} {declared}')
missing = [o for o in only if not any(p["id"] == o for p in packs)]
for m in missing:
    print(f"BAD {m} - - not in packs.json"); bad += 1
sys.exit(1 if bad else 0)
PY
) || {
    printf '%s\n' "$ROWS" | grep '^BAD' | sed 's/^BAD /   refused: /'
    die "the release's packs.json and SHA256SUMS disagree, or a pack is missing: nothing installed"
}
[ -n "$ROWS" ] || die "no pack selected"

# ---- phase 1: every selected zip present or downloaded into $WORK and checked
fail=0
PLAN=""
while read -r _ id version zip sha; do
    dest="$MODS_DIR/$sha.zip"
    if [ -f "$dest" ] && [ "$(sha_of "$dest")" = "$sha" ]; then
        say "  present  $id $version ($sha)"
        PLAN="$PLAN$id $version $sha present"$'\n'
        continue
    fi
    if [ "$DRY" = 1 ]; then
        say "  would get $id $version -> $dest"
        PLAN="$PLAN$id $version $sha dry"$'\n'
        continue
    fi
    if ! fetch "$zip"; then
        say "  FAILED   $id $version: $zip is not in the release"
        fail=1
        continue
    fi
    got=$(sha_of "$WORK/$zip")
    if [ "$got" != "$sha" ]; then
        say "  FAILED   $id $version: $zip hashes $got, the release lists $sha"
        rm -f "$WORK/$zip"
        fail=1
        continue
    fi
    say "  checked  $id $version ($sha, $(wc -c < "$WORK/$zip" | tr -d ' ') bytes)"
    PLAN="$PLAN$id $version $sha $zip"$'\n'
done <<< "$ROWS"
[ "$fail" -eq 0 ] || die "a pack failed its check: nothing installed"

# no_symlink_path PATH: true when no component of PATH is a symbolic link (checked right before root writes there:
# a folder swapped for a link between the request and the install must not redirect the write)
no_symlink_path() {
    local p=$1 acc="" c
    case "$p" in [A-Za-z]:*) acc=${p:0:2}; p=${p:2} ;; esac
    local IFS=/
    for c in $p; do
        [ -n "$c" ] || continue
        acc="$acc/$c"
        [ -L "$acc" ] && return 1
    done
    return 0
}

# ---- phase 2: install what was checked
if [ "$DRY" = 0 ]; then
    no_symlink_path "$MODS_DIR" || die "$MODS_DIR has a symbolic link in its path: nothing installed"
    mkdir -p "$MODS_DIR"
    no_symlink_path "$MODS_DIR" || die "$MODS_DIR has a symbolic link in its path: nothing installed"
    while read -r id version sha what; do
        [ -n "$id" ] || continue
        case "$what" in present|dry) continue ;; esac
        if [ "$DC_TEST" = 1 ] || [ "$(id -u)" -ne 0 ]; then
            install -m 0644 "$WORK/$what" "$MODS_DIR/$sha.zip"
        else
            install -m 0644 -o "$DC_USER" -g "$DC_USER" "$WORK/$what" "$MODS_DIR/$sha.zip"
        fi
        say "  installed $id $version -> $MODS_DIR/$sha.zip"
    done <<< "$PLAN"
fi

# ---- the rows to paste (server.toml is never edited here)
say ""
if [ "$FORMAT" = set ]; then
    say "# [[mods.set]] rows (a modded lobby) for release $REPO $TAG:"
    while read -r id version sha _; do
        [ -n "$id" ] || continue
        printf '[[mods.set]]\nid = "%s"\nversion = "%s"\nsha256 = "%s"\n\n' "$id" "$version" "$sha"
    done <<< "$PLAN"
else
    say "# the entry's mods = [...] rows (a curated [[lobby.servers]] entry) for release $REPO $TAG:"
    while read -r id version sha _; do
        [ -n "$id" ] || continue
        printf '  { id = "%s", version = "%s", sha256 = "%s" },\n' "$id" "$version" "$sha"
    done <<< "$PLAN"
fi
[ "$DRY" = 0 ] || say "# dry run: nothing was installed"

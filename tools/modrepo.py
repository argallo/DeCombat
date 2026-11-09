#!/usr/bin/env python3
"""modrepo.py: a mod repository of its own for DeCombat packs (Python 3, standard library only).

A mod lives in its own git repository, not in the game's. This tool makes such a repository, keeps the packs in it
buildable with `dc-modkit`, and prints the rows the directory files and a server's config want:

    python3 tools/modrepo.py init <dir> --github <owner/repo> --decombat-rev <DeCombat tag> [--name <title>]
    python3 tools/modrepo.py add <repo> <mod folder>... [--generator <dir>]
    python3 tools/modrepo.py build <repo> [--modkit <dc-modkit exe>] [--rom <client folder>] [--only <id>...]
    python3 tools/modrepo.py rows <repo> --format vanilla-plus|known-mods|server-toml|catalog-toml [--release-tag <tag>]
    python3 tools/modrepo.py verify <repo> --against <json file>...
    python3 tools/modrepo.py lock <repo>
    python3 tools/modrepo.py selftest

Layout of a repository made by `init`:

    modrepo.json                the repository's name, its GitHub `owner/repo`, the DeCombat tag its CI builds dc-modkit from
    mods/<id>/                  one pack per folder: manifest.json, main.lua, assets/, screenshots/ (what `dc-modkit scaffold` writes)
    generators/<id>/            optional: the scripts that drew a pack's own art (so the art is reproducible)
    dist/                       `build` output, ignored by git: <id>-<version>.zip, packs.json, SHA256SUMS
    packs.lock.json             `lock` output, committed: the hashes a trusted local build gave; CI fails when its build differs
    tools/modrepo.py            this file, so the repository builds without the game's checkout
    .github/workflows/packs.yml lint + validate + pack on every push; a `v*` tag publishes a release with the zips

`build` packs every `mods/<id>` with `dc-modkit pack` from a clean copy (`git archive`, so Windows line endings never
change a hash), after `dc-modkit validate` and `dc-modkit lint`, and writes `dist/packs.json`: one row per pack with
id, version, sha256, size, kind, tags, source_repo. `rows` turns that file into the review-list row (with
`--release-tag <tag>` each row also names its release page, `https://github.com/<owner>/<repo>/releases/tag/<tag>`;
the workflow passes the tag it is publishing)
(`community/vanilla_plus.json`), the known-mods row (`community/community.json`), or `[[mods.set]]` / `[[mods.catalog]]`
blocks for a server's `server.toml`. `verify` compares the hashes in `dist/packs.json` with the rows of one or more
directory files and exits 1 on a difference: the servers and the review list are keyed by hash, so a pack that does
not reproduce its listed hash is a pack that was changed after review. `lock` copies `dist/packs.json` to
`packs.lock.json`; the generated workflow verifies its own build against that file, so a dc-modkit at another revision
that packs different bytes fails the run instead of publishing hashes nobody reviewed.

`dc-modkit` is found through `--modkit`, the `DC_MODKIT` environment variable, `dc-modkit` on PATH, or (inside a
DeCombat checkout) `cargo run -q -p dc-modkit --`. The generated workflow downloads the ModKit asset of the DeCombat
release `decombat_rev` names (`DeCombat-ModKit-linux-x86_64-<tag>.zip`, checked against that release's SHA256SUMS) and
builds it from source with `cargo install --git` only when the release has none, with a warning in the run.
Exit codes: 0 ok, 1 a check failed, 2 usage.
"""
import argparse
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

DECOMBAT_GIT = "https://github.com/argallo/DeCombat"
BRANCH_NAMES = {"main", "master", "latest", "HEAD", "develop", "dev", "trunk"}
MOD_ID = re.compile(r"^[a-z][a-z0-9_]{2,63}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")

GITATTRIBUTES = """# mod packages hash their bytes (dc-modkit pack is deterministic): one line ending on every checkout
mods/** text eol=lf
mods/**/*.png binary
mods/**/*.jpg binary
mods/**/*.jpeg binary
mods/**/*.dat binary
mods/**/*.dtx binary
mods/**/*.r16 binary
mods/**/*.wav binary
mods/**/*.ogg binary
generators/** text eol=lf
generators/**/*.png binary
tools/*.py text eol=lf
"""

GITIGNORE = """dist/
__pycache__/
*.pyc
# generated art lives in the packs as DTX; the generators' PNG output is not committed
generators/**/*.png
"""

WORKFLOW = """name: packs
# Every push and pull request: validate, lint and pack every mod under mods/ with dc-modkit (built from the DeCombat
# revision in modrepo.json). A tag v* publishes a GitHub Release with the zips, packs.json and SHA256SUMS: the
# sha256 in packs.json is what goes into the directory row and into a server's config.
on:
  push:
    branches: [main, master]
    tags: ["v*"]
  pull_request:
  workflow_dispatch:
permissions:
  contents: write
jobs:
  packs:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      - name: Resolve the DeCombat revision (a tag or a commit of __DECOMBAT_GIT__, never a branch)
        id: rev
        run: |
          rev=$(python3 -c 'import json;print(json.load(open("modrepo.json"))["decombat_rev"])')
          echo "modrepo.json decombat_rev = $rev"
          case "$rev" in
            main|master|latest|HEAD|develop|dev|trunk)
              echo "::error::decombat_rev '$rev' is a branch name: cargo install --rev needs a commit and a branch moves; pin a tag (git ls-remote --tags __DECOMBAT_GIT__) or a 40-hex commit"
              exit 1 ;;
          esac
          if printf '%s' "$rev" | grep -Eq '^[0-9a-f]{40}$'; then
            echo "kind=rev" >> "$GITHUB_OUTPUT"
          elif git ls-remote --exit-code --tags __DECOMBAT_GIT__ "refs/tags/$rev" >/dev/null 2>&1; then
            echo "kind=tag" >> "$GITHUB_OUTPUT"
          else
            echo "::error::decombat_rev '$rev' is neither a tag of __DECOMBAT_GIT__ nor a 40-hex commit. The tags there:"
            git ls-remote --tags __DECOMBAT_GIT__ | sed 's#.*refs/tags/##' | grep -v '{}$' | tail -12
            exit 1
          fi
          echo "rev=$rev" >> "$GITHUB_OUTPUT"
      - name: dc-modkit, the ModKit asset of the DeCombat release decombat_rev names (checked against its SHA256SUMS)
        id: modkit
        run: |
          set -euo pipefail
          rev="${{ steps.rev.outputs.rev }}"
          zip="DeCombat-ModKit-linux-x86_64-$rev.zip"
          base="__DECOMBAT_GIT__/releases/download/$rev"
          mkdir -p "$HOME/modkit"
          cd "$HOME/modkit"
          if [ "${{ steps.rev.outputs.kind }}" = tag ] && curl -fsSL -o SHA256SUMS "$base/SHA256SUMS" && curl -fsSL -o "$zip" "$base/$zip"; then
            # a zip that does not match the release's own checksum fails the run; it never falls back
            awk -v z="$zip" '$2 == z' SHA256SUMS > want.sha
            [ -s want.sha ] || { echo "::error::$zip is not in the SHA256SUMS of release $rev"; exit 1; }
            sha256sum -c want.sha
            unzip -q -o "$zip" -d x
            exe=$(find "$PWD/x" -type f -name dc-modkit | head -n 1)
            chmod +x "$exe"
            echo "dc-modkit from the ModKit asset of $rev: $exe"
            echo "exe=$exe" >> "$GITHUB_OUTPUT"
          else
            echo "::warning::no ModKit asset ($zip) on release $rev of __DECOMBAT_GIT__ (a release before ModKit, or a commit pin): dc-modkit is BUILT FROM SOURCE below, which takes several minutes"
            echo "exe=" >> "$GITHUB_OUTPUT"
          fi
      - name: Install Rust (only without the ModKit asset)
        if: steps.modkit.outputs.exe == ''
        run: rustup toolchain install stable --profile minimal && rustup default stable
      - name: Cache dc-modkit (only without the ModKit asset)
        if: steps.modkit.outputs.exe == ''
        id: cache
        uses: actions/cache@v4
        with:
          path: ~/.cargo/bin/dc-modkit
          key: dc-modkit-${{ runner.os }}-${{ steps.rev.outputs.rev }}
      - name: Build dc-modkit from source (only without the ModKit asset)
        if: steps.modkit.outputs.exe == '' && steps.cache.outputs.cache-hit != 'true'
        run: cargo install --git __DECOMBAT_GIT__ --${{ steps.rev.outputs.kind }} "${{ steps.rev.outputs.rev }}" dc-modkit --locked
      - name: Build the packs
        run: |
          exe="${{ steps.modkit.outputs.exe }}"
          [ -n "$exe" ] || exe="$HOME/.cargo/bin/dc-modkit"
          python3 tools/modrepo.py build . --modkit "$exe"
      - name: The hashes are the locked ones (packs.lock.json, written by `modrepo.py lock` after a trusted local build)
        run: |
          if [ -f packs.lock.json ]; then
            python3 tools/modrepo.py verify . --against packs.lock.json --all
          else
            echo "::warning::no packs.lock.json: the hashes of this build were not checked against a local build (run modrepo.py lock and commit it)"
          fi
      - uses: actions/upload-artifact@v4
        with:
          name: packs
          path: dist/
      - name: Publish the release
        if: startsWith(github.ref, 'refs/tags/v')
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          python3 tools/modrepo.py rows . --format known-mods --release-tag "${GITHUB_REF_NAME}" > dist/known_mods.json
          python3 tools/modrepo.py rows . --format vanilla-plus --release-tag "${GITHUB_REF_NAME}" > dist/vanilla_plus_rows.json
          python3 tools/modrepo.py rows . --format server-toml > dist/server_toml_lines.txt
          gh release create "${GITHUB_REF_NAME}" dist/*.zip dist/packs.json dist/SHA256SUMS dist/known_mods.json dist/vanilla_plus_rows.json dist/server_toml_lines.txt --title "${GITHUB_REF_NAME}" --notes-file dist/NOTES.md
"""

README = """# {name}

DeCombat mod packs, one folder each under `mods/`. Every pack is Lua text, data and its own original assets: no
file from the game client, no asset taken from anywhere without a licence that allows it. The game's rules for
packs are in the DeCombat repository (`docs/MODDING_GUIDE.md`); this repository only has to build.

## Build

    python3 tools/modrepo.py build .            # validates, lints and packs every mod into dist/

`dc-modkit` comes from the DeCombat release: the ModKit zip for your platform (`DeCombat-ModKit-win|mac|linux-x86_64-<tag>.zip`
on {decombat_git}/releases), or build it with `cargo install --git {decombat_git} dc-modkit --locked`; pass the binary
with `--modkit`. Add `--rom <your client folder>` to check weapon rows against the real tables. The workflow uses the
ModKit of the tag in modrepo.json.

## Release

Tag `v<date or number>` and push the tag: the workflow builds the packs and publishes a release with every zip,
`packs.json` (id, version, sha256 per pack), `SHA256SUMS` and the rows ready to paste:

- `vanilla_plus_rows.json`: the review-list rows for a pull request against `community/vanilla_plus.json` (fill in
  `reviewer_note`);
- `known_mods.json`: the verified-badge rows for `community/community.json`;
- `server_toml_lines.txt`: `[[mods.set]]` / `[[mods.catalog]]` blocks for a server operator.

Operators install `<sha256>.zip` from the release into their lobby's mods folder; players download it from the
server. The sha256 of the published zip is the one in the row: a different byte is a different pack.

## Change a pack

Bump `version` in its `manifest.json` with every change that reaches players; a new version is a new row. Keep the
first screenshot a picture of your own making. `generators/<id>/` holds the scripts that drew a pack's art, when it
was drawn by code.
"""

NOTES = """Packs of this release (id, version, sha256). Install a zip as `<sha256>.zip` in a lobby's mods folder; the sha256
is the one a directory row carries.

| pack | version | sha256 |
|---|---|---|
{rows}
"""


def die(msg, code=1):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(code)


def read_json(path):
    with open(path, "r", encoding="utf-8-sig") as f:
        return json.load(f)


def write_text(path, text):
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def git(args, cwd, check=True):
    r = subprocess.run(["git"] + args, cwd=cwd, capture_output=True)
    if check and r.returncode != 0:
        raise RuntimeError(r.stderr.decode(errors="replace").strip())
    return r


def git_root(path):
    """The git work tree holding `path`, or None."""
    try:
        r = git(["rev-parse", "--show-toplevel"], cwd=path if os.path.isdir(path) else os.path.dirname(path), check=False)
    except FileNotFoundError:
        return None
    if r.returncode != 0:
        return None
    return r.stdout.decode().strip()


def clean_copy(src_dir, dst_parent):
    """`src_dir` as git holds it at HEAD (LF text, no untracked files) under `dst_parent`; falls back to a plain copy
    when the folder is not tracked. Returns the copied folder and the way it was copied."""
    root = git_root(src_dir)
    rel = None
    if root:
        rel = os.path.relpath(os.path.abspath(src_dir), root).replace(os.sep, "/")
        tracked = git(["ls-files", "--error-unmatch", rel], cwd=root, check=False).returncode == 0
        if tracked:
            dirty = git(["status", "--porcelain", "--", rel], cwd=root).stdout.decode().strip()
            if dirty:
                print(f"warning: {rel} has uncommitted changes; the clean copy is HEAD, not the working tree", file=sys.stderr)
            data = git(["archive", "HEAD", rel], cwd=root).stdout
            with tarfile.open(fileobj=io.BytesIO(data)) as t:
                if hasattr(tarfile, "data_filter"):
                    t.extractall(dst_parent, filter="data")
                else:
                    t.extractall(dst_parent)
            return os.path.join(dst_parent, *rel.split("/")), "git archive"
    dst = os.path.join(dst_parent, os.path.basename(os.path.normpath(src_dir)))
    shutil.copytree(src_dir, dst)
    return dst, "copy"


def repo_meta(repo):
    path = os.path.join(repo, "modrepo.json")
    if not os.path.isfile(path):
        die(f"{path} is missing: not a mod repository made by `modrepo.py init`")
    return read_json(path)


def github_url(spec):
    if not spec:
        return None
    spec = spec.strip()
    if spec.startswith("https://"):
        return spec.rstrip("/")
    if re.match(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", spec):
        return f"https://github.com/{spec}"
    die(f"--github wants owner/repo or an https URL, not {spec!r}", 2)


# ---------------------------------------------------------------- init

def cmd_init(a):
    repo = a.dir
    if os.path.isdir(repo) and os.listdir(repo) and not a.force:
        die(f"{repo} exists and is not empty (use --force to add the files anyway)")
    url = github_url(a.github)
    rev = a.decombat_rev.strip()
    if rev in BRANCH_NAMES or not re.match(r"^(v?[0-9][A-Za-z0-9._-]*|[0-9a-f]{40})$", rev):
        die(f"--decombat-rev {rev!r}: pin a tag of the DeCombat repository (`git ls-remote --tags {DECOMBAT_GIT}`, e.g. "
            "v0.5.0-beta6) or a 40-hex commit; a branch moves and cargo install --rev does not take one", 2)
    name = a.name or os.path.basename(os.path.normpath(repo))
    os.makedirs(os.path.join(repo, "mods"), exist_ok=True)
    os.makedirs(os.path.join(repo, "generators"), exist_ok=True)
    write_text(os.path.join(repo, "modrepo.json"), json.dumps({
        "name": name,
        "github": url,
        "decombat_rev": rev,
    }, indent=2) + "\n")
    write_text(os.path.join(repo, ".gitattributes"), GITATTRIBUTES)
    write_text(os.path.join(repo, ".gitignore"), GITIGNORE)
    write_text(os.path.join(repo, "README.md"), README.format(name=name, decombat_git=DECOMBAT_GIT))
    write_text(os.path.join(repo, ".github", "workflows", "packs.yml"), WORKFLOW.replace("__DECOMBAT_GIT__", DECOMBAT_GIT))
    write_text(os.path.join(repo, "mods", ".gitkeep"), "")
    write_text(os.path.join(repo, "generators", "README.md"),
               "One folder per pack whose art was drawn by code: the script and the arguments that reproduce the pack's pattern\n"
               "pixel for pixel. The PNG it writes is not committed; the pack holds the DTX made from it.\n")
    with open(os.path.abspath(__file__), "r", encoding="utf-8") as f:
        write_text(os.path.join(repo, "tools", "modrepo.py"), f.read())
    print(f"made {repo}: mods/, generators/, tools/modrepo.py, .github/workflows/packs.yml, README.md")
    print("next: `modrepo.py add <repo> <mod folder>`, then `git init`, commit, push, and tag v1 for the first release")


# ---------------------------------------------------------------- add

def cmd_add(a):
    repo = a.repo
    repo_meta(repo)
    added = 0
    for src in a.mods:
        if not os.path.isfile(os.path.join(src, "manifest.json")):
            die(f"{src} has no manifest.json")
        man = read_json(os.path.join(src, "manifest.json"))
        mid = man.get("id", "")
        if not MOD_ID.match(mid):
            die(f"{src}: manifest id {mid!r} is not a mod id")
        dst = os.path.join(repo, "mods", mid)
        if os.path.exists(dst):
            if not a.force:
                die(f"{dst} exists (use --force to replace it)")
            shutil.rmtree(dst)
        with tempfile.TemporaryDirectory(prefix="modrepo_add_") as tmp:
            copied, how = clean_copy(src, tmp)
            shutil.copytree(copied, dst)
        print(f"added mods/{mid} ({man.get('version', '?')}, {how})")
        added += 1
    if a.generator:
        if len(a.mods) != 1:
            die("--generator goes with exactly one mod folder", 2)
        man = read_json(os.path.join(a.mods[0], "manifest.json"))
        gdst = os.path.join(repo, "generators", man["id"])
        if os.path.exists(gdst):
            shutil.rmtree(gdst)
        if os.path.isdir(a.generator):
            shutil.copytree(a.generator, gdst, ignore=shutil.ignore_patterns("*.png", "__pycache__"))
        else:
            os.makedirs(gdst)
            shutil.copy2(a.generator, gdst)
        print(f"added generators/{man['id']}")
    print(f"{added} pack(s) added")


# ---------------------------------------------------------------- build

def find_modkit(explicit, repo):
    if explicit:
        # one executable, or a command line such as "cargo run -q -p dc-modkit --"
        return [explicit] if os.path.isfile(explicit) or " " not in explicit else explicit.split()
    env = os.environ.get("DC_MODKIT")
    if env:
        return [env]
    on_path = shutil.which("dc-modkit")
    if on_path:
        return [on_path]
    # inside a DeCombat checkout: the crate itself
    here = os.path.dirname(os.path.abspath(__file__))
    for base in (os.path.dirname(here), git_root(repo) or ""):
        if base and os.path.isdir(os.path.join(base, "crates", "dc-modkit")):
            return ["cargo", "run", "-q", "--manifest-path", os.path.join(base, "Cargo.toml"), "-p", "dc-modkit", "--"]
    die("dc-modkit not found: pass --modkit <exe>, set DC_MODKIT, or download the ModKit zip of a release "
        f"({DECOMBAT_GIT}/releases) or `cargo install --git {DECOMBAT_GIT} dc-modkit --locked`")


def run_modkit(modkit, args):
    r = subprocess.run(modkit + args, capture_output=True, text=True, encoding="utf-8", errors="replace")
    try:
        j = json.loads(r.stdout) if r.stdout.strip() else {}
    except json.JSONDecodeError:
        j = {}
    return r.returncode, j, (r.stdout + r.stderr).strip()


def list_mods(repo, only=None):
    mods_dir = os.path.join(repo, "mods")
    if not os.path.isdir(mods_dir):
        die(f"{mods_dir} is missing")
    ids = sorted(d for d in os.listdir(mods_dir)
                 if os.path.isfile(os.path.join(mods_dir, d, "manifest.json")))
    if only:
        missing = [o for o in only if o not in ids]
        if missing:
            die(f"no such pack(s) under mods/: {', '.join(missing)}")
        ids = [i for i in ids if i in only]
    return ids


def cmd_build(a):
    repo = a.repo
    meta = repo_meta(repo)
    modkit = find_modkit(a.modkit, repo)
    ids = list_mods(repo, a.only)
    if not ids:
        die("no packs under mods/")
    dist = os.path.join(repo, "dist")
    os.makedirs(dist, exist_ok=True)
    rows = []
    failed = 0
    with tempfile.TemporaryDirectory(prefix="modrepo_build_") as tmp:
        for mid in ids:
            src = os.path.join(repo, "mods", mid)
            copied, how = clean_copy(src, os.path.join(tmp, mid))
            man = read_json(os.path.join(copied, "manifest.json"))
            if man.get("id") != mid:
                print(f"FAIL {mid}: the folder is named {mid} but the manifest id is {man.get('id')!r}")
                failed += 1
                continue
            version = str(man.get("version", ""))
            vargs = ["validate", copied, "--json"] + (["--rom", a.rom] if a.rom else [])
            code, j, text = run_modkit(modkit, vargs)
            if code != 0 or not j.get("ok", False):
                print(f"FAIL {mid} {version}: validate\n  " + "\n  ".join(j.get("errors") or [text[-800:]]))
                failed += 1
                continue
            code, j, text = run_modkit(modkit, ["lint", copied, "--json"])
            errors = [d for d in j.get("diagnostics", []) if str(d.get("severity", "error")).lower() == "error"]
            if code != 0 or not j.get("ok", False) or errors:
                print(f"FAIL {mid} {version}: lint\n  " + "\n  ".join(json.dumps(d) for d in errors) or text[-800:])
                failed += 1
                continue
            zip_path = os.path.join(dist, f"{mid}-{version}.zip")
            code, j, text = run_modkit(modkit, ["pack", copied, "-o", zip_path, "--json"])
            if code != 0 or not j.get("ok", False) or not os.path.isfile(zip_path):
                print(f"FAIL {mid} {version}: pack\n  {text[-800:]}")
                failed += 1
                continue
            sha = j.get("sha256") or sha256_file(zip_path)
            if sha != sha256_file(zip_path):
                print(f"FAIL {mid} {version}: dc-modkit reports {sha[:12]} but the zip hashes differently")
                failed += 1
                continue
            entry = j.get("modset_entry", {})
            source = github_url(man.get("github")) if man.get("github") else meta.get("github")
            row = {
                "id": mid,
                "name": man.get("name", mid),
                "version": version,
                "sha256": sha,
                "size": os.path.getsize(zip_path),
                "kind": entry.get("kind") or man.get("kind"),
                "requires_client": bool(entry.get("requires_client", man.get("requires_client", False))),
                "capabilities": entry.get("capabilities") or man.get("capabilities", []),
                "tags": entry.get("tags") or man.get("tags", []),
                "author": man.get("author", ""),
                "description": man.get("description", ""),
                "source_repo": source,
                "zip": os.path.basename(zip_path),
                "copied": how,
            }
            rows.append(row)
            print(f"ok   {mid} {version} {sha[:12]} {row['size']} B ({how})")
    write_text(os.path.join(dist, "packs.json"), json.dumps({"version": 1, "packs": rows}, indent=2) + "\n")
    write_text(os.path.join(dist, "SHA256SUMS"), "".join(f"{r['sha256']}  {r['zip']}\n" for r in rows))
    write_text(os.path.join(dist, "NOTES.md"), NOTES.format(
        rows="\n".join(f"| {r['id']} | {r['version']} | `{r['sha256']}` |" for r in rows)))
    print(f"\n{len(rows)} pack(s) built, {failed} failed -> {dist}")
    if failed:
        sys.exit(1)


# ---------------------------------------------------------------- rows

def load_packs(repo):
    path = os.path.join(repo, "dist", "packs.json")
    if not os.path.isfile(path):
        die(f"{path} is missing: run `modrepo.py build` first")
    return read_json(path)["packs"]


def toml_str(s):
    return json.dumps(s)


def release_page(repo, tag):
    """The GitHub release page of `tag` in this repository (modrepo.json's github URL), or None without a usable tag."""
    if not tag:
        return None
    tag = tag.strip()
    if tag in BRANCH_NAMES or not re.match(r"^v?[0-9][A-Za-z0-9._-]{0,63}$", tag):
        die(f"--release-tag {tag!r}: a release tag (v1, v0.5.0-beta6), never a branch", 2)
    meta = read_json(os.path.join(repo, "modrepo.json")) if os.path.isfile(os.path.join(repo, "modrepo.json")) else {}
    url = (meta.get("github") or "").rstrip("/")
    if not url.startswith("https://github.com/"):
        die("--release-tag needs modrepo.json with the repository's GitHub URL (modrepo.py init --github owner/repo)", 2)
    return f"{url}/releases/tag/{tag}"


def cmd_rows(a):
    packs = load_packs(a.repo)
    page = release_page(a.repo, a.release_tag)
    if a.format == "vanilla-plus":
        out = []
        for p in packs:
            row = {
                "id": p["id"], "version": p["version"], "sha256": p["sha256"],
                "source_repo": p["source_repo"] or "https://github.com/<owner>/<repo>",
            }
            if page:
                row["release"] = page
            row["reviewer_note"] = "<what the pack is; for a gun the reference gun and each difference; who approved and when>"
            out.append(row)
        print(json.dumps(out, indent=2))
    elif a.format == "known-mods":
        out = []
        for p in packs:
            row = {
                "id": p["id"], "name": p["name"], "version": p["version"], "sha256": p["sha256"],
                "source_repo": p["source_repo"] or "https://github.com/<owner>/<repo>",
                "author": p["author"], "description": p["description"], "capabilities": p["capabilities"],
            }
            if page:
                row["release"] = page
            out.append(row)
        print(json.dumps(out, indent=2))
    elif a.format in ("server-toml", "catalog-toml"):
        table = "mods.set" if a.format == "server-toml" else "mods.catalog"
        chosen = packs if a.format == "server-toml" else [p for p in packs if p.get("kind") == "client_visual"]
        for p in chosen:
            print(f"[[{table}]]\nid = {toml_str(p['id'])}\nversion = {toml_str(p['version'])}\nsha256 = {toml_str(p['sha256'])}\n")
        if a.format == "server-toml":
            inline = ", ".join(
                "{ id = %s, version = %s, sha256 = %s }" % (toml_str(p["id"]), toml_str(p["version"]), toml_str(p["sha256"]))
                for p in packs)
            print(f"# or, on a [[lobby.servers]] entry:\n# mods = [{inline}]")
    else:
        die(f"unknown format {a.format}", 2)


# ---------------------------------------------------------------- verify

def directory_rows(path):
    """(id, version, sha256) rows of a review list, a community document, a server_list.json, a packs.json or a lock."""
    doc = read_json(path)
    rows = []
    for key in ("packs", "known_mods"):
        for r in doc.get(key, []) if isinstance(doc, dict) else []:
            rows.append((r.get("id"), str(r.get("version")), r.get("sha256"), key))
    if isinstance(doc, dict) and "mod_set" in doc:
        for r in doc["mod_set"].get("mods", []):
            rows.append((r.get("id"), str(r.get("version")), r.get("sha256"), "mod_set"))
    if isinstance(doc, list):
        for r in doc:
            rows.append((r.get("id"), str(r.get("version")), r.get("sha256"), "list"))
    return rows


def cmd_verify(a):
    packs = {p["id"]: p for p in load_packs(a.repo)}
    ok = diff = absent = 0
    for path in a.against:
        rows = directory_rows(path)
        for mid, version, sha, where in rows:
            p = packs.get(mid)
            if not p:
                if a.all:
                    print(f"ABSENT {mid} {version}: not in this repository ({os.path.basename(path)} {where})")
                    absent += 1
                continue
            if p["sha256"] == sha and p["version"] == version:
                ok += 1
                print(f"OK     {mid} {version}")
            else:
                diff += 1
                why = "version" if p["version"] != version else "bytes"
                print(f"DIFF   {mid}: row {version} {sha[:12]} vs built {p['version']} {p['sha256'][:12]} ({why}; {os.path.basename(path)} {where})")
    print(f"\n{ok} same, {diff} different" + (f", {absent} absent" if a.all else ""))
    if diff:
        sys.exit(1)


# ---------------------------------------------------------------- lock

def cmd_lock(a):
    packs = load_packs(a.repo)
    out = os.path.join(a.repo, "packs.lock.json")
    write_text(out, json.dumps({
        "version": 1,
        "note": "written by modrepo.py lock from a trusted local build; the workflow fails when its build hashes differently",
        "packs": [{k: p[k] for k in ("id", "version", "sha256", "size")} for p in packs],
    }, indent=2) + "\n")
    print(f"{len(packs)} pack(s) locked in {out}")


# ---------------------------------------------------------------- selftest

STUB_MODKIT = r'''import sys, json, os, zipfile, hashlib
# a stand-in for dc-modkit in the self-test: validate/lint say ok, pack zips the folder deterministically
args = sys.argv[1:]
verb = args[0]
if verb in ("validate", "lint"):
    print(json.dumps({"ok": True, "errors": [], "diagnostics": [], "warnings": []})); sys.exit(0)
if verb == "pack":
    src = args[1]; out = args[args.index("-o") + 1]
    man = json.load(open(os.path.join(src, "manifest.json"), encoding="utf-8"))
    names = []
    for root, dirs, files in os.walk(src):
        dirs.sort()
        for f in sorted(files):
            names.append(os.path.join(root, f))
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for n in names:
            info = zipfile.ZipInfo(os.path.relpath(n, src).replace(os.sep, "/"), date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, open(n, "rb").read())
    sha = hashlib.sha256(open(out, "rb").read()).hexdigest()
    print(json.dumps({"ok": True, "sha256": sha, "size": os.path.getsize(out),
                      "modset_entry": {"id": man["id"], "version": man["version"], "sha256": sha, "kind": man.get("kind"),
                                       "requires_client": man.get("requires_client", False),
                                       "capabilities": man.get("capabilities", []), "tags": man.get("tags", [])}}))
    sys.exit(0)
sys.exit(2)
'''


def cmd_selftest(_a):
    import contextlib

    def run(argv):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
            code = 0
            try:
                main(argv)
            except SystemExit as e:
                code = int(e.code or 0)
        return code, buf.getvalue()

    with tempfile.TemporaryDirectory(prefix="modrepo_selftest_") as tmp:
        stub = os.path.join(tmp, "stub_modkit.py")
        write_text(stub, STUB_MODKIT)
        modkit = f"{sys.executable} -I {stub}"
        repo = os.path.join(tmp, "repo")
        code, out = run(["init", repo, "--github", "example/packs", "--name", "Example packs", "--decombat-rev", "main"])
        assert code == 2 and "a branch moves" in out, "a branch name as the pin is refused: " + out
        code, out = run(["init", repo, "--github", "example/packs", "--name", "Example packs", "--decombat-rev", "v0.5.0-beta6", "--force"])
        assert code == 0, out
        assert read_json(os.path.join(repo, "modrepo.json"))["decombat_rev"] == "v0.5.0-beta6"
        with open(os.path.join(repo, ".github", "workflows", "packs.yml"), encoding="utf-8") as f:
            wf = f.read()
        assert "is a branch name" in wf and "--${{ steps.rev.outputs.kind }}" in wf
        # MOD-30: the ModKit asset first, checked against the release's SHA256SUMS; source only without it, loudly
        assert "DeCombat-ModKit-linux-x86_64-$rev.zip" in wf and "sha256sum -c want.sha" in wf, "the ModKit download"
        assert "BUILT FROM SOURCE" in wf and "if: steps.modkit.outputs.exe == ''" in wf, "the loud fallback"
        assert wf.index("ModKit asset of the DeCombat release") < wf.index("cargo install --git"), "download before build"
        assert "verify . --against packs.lock.json --all" in wf, "the lock check stays"
        for f in ("modrepo.json", ".gitattributes", ".gitignore", "README.md", "tools/modrepo.py",
                  ".github/workflows/packs.yml", "generators/README.md"):
            assert os.path.isfile(os.path.join(repo, f)), f
        assert read_json(os.path.join(repo, "modrepo.json"))["github"] == "https://github.com/example/packs"
        # a pack with CRLF text: the clean copy keeps the bytes (no git here), so add is a plain copy
        src = os.path.join(tmp, "src", "camo_x")
        os.makedirs(os.path.join(src, "assets"))
        with open(os.path.join(src, "manifest.json"), "wb") as f:
            f.write(json.dumps({"id": "camo_x", "name": "Camo X", "version": "1.2.0", "kind": "shared",
                                "requires_client": True, "capabilities": ["content.weapons", "assets"],
                                "tags": ["variant", "camo"], "author": "Someone",
                                "description": "A test camo."}).encode())
        with open(os.path.join(src, "main.lua"), "wb") as f:
            f.write(b"return function(mod)\r\nend\r\n")
        gen = os.path.join(tmp, "gen")
        os.makedirs(gen)
        write_text(os.path.join(gen, "gen.py"), "print('art')\n")
        write_text(os.path.join(gen, "paint.png"), "not committed")
        code, out = run(["add", repo, src, "--generator", gen])
        assert code == 0, out
        assert os.path.isfile(os.path.join(repo, "mods", "camo_x", "main.lua"))
        assert os.path.isfile(os.path.join(repo, "generators", "camo_x", "gen.py"))
        assert not os.path.exists(os.path.join(repo, "generators", "camo_x", "paint.png")), "PNG output is not copied"
        code, out = run(["add", repo, src])
        assert code == 1 and "exists" in out, "a second add without --force refuses: " + out
        # a second pack, client_visual, for the catalogue form
        src2 = os.path.join(tmp, "src", "hud_y")
        os.makedirs(src2)
        write_text(os.path.join(src2, "manifest.json"), json.dumps({
            "id": "hud_y", "name": "HUD Y", "version": "0.1.0", "kind": "client_visual",
            "capabilities": ["content.hud"], "github": "someone/hud-y"}))
        write_text(os.path.join(src2, "main.lua"), "return function(mod) end\n")
        code, out = run(["add", repo, src2])
        assert code == 0, out
        code, out = run(["build", repo, "--modkit", modkit])
        assert code == 0, out
        packs = read_json(os.path.join(repo, "dist", "packs.json"))["packs"]
        assert [p["id"] for p in packs] == ["camo_x", "hud_y"], packs
        assert packs[0]["tags"] == ["variant", "camo"] and packs[0]["source_repo"] == "https://github.com/example/packs"
        assert packs[1]["source_repo"] == "https://github.com/someone/hud-y", "the manifest's github wins"
        assert SHA256.match(packs[0]["sha256"])
        assert sha256_file(os.path.join(repo, "dist", "camo_x-1.2.0.zip")) == packs[0]["sha256"]
        assert os.path.isfile(os.path.join(repo, "dist", "SHA256SUMS")) and os.path.isfile(os.path.join(repo, "dist", "NOTES.md"))
        # the same bytes build the same hash
        first = packs[0]["sha256"]
        code, out = run(["build", repo, "--modkit", modkit, "--only", "camo_x"])
        assert code == 0 and read_json(os.path.join(repo, "dist", "packs.json"))["packs"][0]["sha256"] == first
        code, out = run(["build", repo, "--modkit", modkit])
        assert code == 0
        # rows
        code, out = run(["rows", repo, "--format", "vanilla-plus"])
        rows = json.loads(out)
        assert rows[0]["id"] == "camo_x" and rows[0]["sha256"] == first and "reviewer_note" in rows[0]
        assert "release" not in rows[0], "no tag, no release page"
        # with the tag being published, every row names its release page; a branch name is refused
        code, out = run(["rows", repo, "--format", "vanilla-plus", "--release-tag", "v3"])
        rows = json.loads(out)
        assert code == 0 and rows[0]["release"] == "https://github.com/example/packs/releases/tag/v3", out
        assert list(rows[0]) == ["id", "version", "sha256", "source_repo", "release", "reviewer_note"], list(rows[0])
        code, out = run(["rows", repo, "--format", "known-mods", "--release-tag", "v3"])
        assert code == 0 and json.loads(out)[1]["release"].endswith("/releases/tag/v3"), out
        code, out = run(["rows", repo, "--format", "vanilla-plus", "--release-tag", "main"])
        assert code == 2 and "never a branch" in out, out
        code, out = run(["rows", repo, "--format", "known-mods"])
        assert json.loads(out)[0]["capabilities"] == ["content.weapons", "assets"]
        code, out = run(["rows", repo, "--format", "server-toml"])
        assert out.count("[[mods.set]]") == 2 and "# mods = [{ id = \"camo_x\"" in out, out
        code, out = run(["rows", repo, "--format", "catalog-toml"])
        assert out.count("[[mods.catalog]]") == 1 and "hud_y" in out, out
        # lock: the lock file verifies clean, and a changed lock is a difference
        code, out = run(["lock", repo])
        assert code == 0 and os.path.isfile(os.path.join(repo, "packs.lock.json")), out
        code, out = run(["verify", repo, "--against", os.path.join(repo, "packs.lock.json"), "--all"])
        assert code == 0 and "2 same, 0 different, 0 absent" in out, out
        lock = read_json(os.path.join(repo, "packs.lock.json"))
        lock["packs"][0]["sha256"] = "a" * 64
        write_text(os.path.join(repo, "packs.lock.json"), json.dumps(lock))
        code, out = run(["verify", repo, "--against", os.path.join(repo, "packs.lock.json")])
        assert code == 1 and "DIFF   camo_x" in out, out
        # verify: same, different, absent
        review = os.path.join(tmp, "vanilla_plus.json")
        write_text(review, json.dumps({"version": 1, "packs": [
            {"id": "camo_x", "version": "1.2.0", "sha256": first, "source_repo": "x"},
            {"id": "other", "version": "1.0.0", "sha256": "0" * 64, "source_repo": "x"}]}))
        code, out = run(["verify", repo, "--against", review])
        assert code == 0 and "OK     camo_x 1.2.0" in out and "ABSENT" not in out, out
        code, out = run(["verify", repo, "--against", review, "--all"])
        assert code == 0 and "ABSENT other" in out, out
        write_text(review, json.dumps({"version": 1, "packs": [{"id": "camo_x", "version": "1.1.0", "sha256": first}]}))
        code, out = run(["verify", repo, "--against", review])
        assert code == 1 and "DIFF   camo_x" in out and "(version" in out, out
        write_text(review, json.dumps({"known_mods": [{"id": "camo_x", "version": "1.2.0", "sha256": "f" * 64}]}))
        code, out = run(["verify", repo, "--against", review])
        assert code == 1 and "(bytes" in out, out
    print("modrepo.py selftest: ok")


# ---------------------------------------------------------------- main

def main(argv=None):
    ap = argparse.ArgumentParser(prog="modrepo.py", description=__doc__.split("\n\n")[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="verb", required=True)
    p = sub.add_parser("init", help="make a mod repository")
    p.add_argument("dir")
    p.add_argument("--github", help="owner/repo of the repository on GitHub (the packs' source_repo)")
    p.add_argument("--name", help="the repository's title (default: the folder name)")
    p.add_argument("--decombat-rev", required=True,
                   help="the DeCombat tag (or 40-hex commit) the workflow builds dc-modkit from, e.g. v0.5.0-beta6; never a branch")
    p.add_argument("--force", action="store_true")
    p.set_defaults(fn=cmd_init)
    p = sub.add_parser("add", help="copy pack folders into mods/<id> (a clean git copy when tracked)")
    p.add_argument("repo")
    p.add_argument("mods", nargs="+")
    p.add_argument("--generator", help="the script or folder that drew the one pack's art -> generators/<id>")
    p.add_argument("--force", action="store_true")
    p.set_defaults(fn=cmd_add)
    p = sub.add_parser("build", help="validate, lint and pack every mods/<id> into dist/")
    p.add_argument("repo")
    p.add_argument("--modkit", help="dc-modkit executable (or a command line)")
    p.add_argument("--rom", help="the client folder for validate --rom")
    p.add_argument("--only", nargs="+", help="these pack ids only")
    p.set_defaults(fn=cmd_build)
    p = sub.add_parser("rows", help="print directory rows or server.toml blocks from dist/packs.json")
    p.add_argument("repo")
    p.add_argument("--format", required=True, choices=["vanilla-plus", "known-mods", "server-toml", "catalog-toml"])
    p.add_argument("--release-tag", help="the release tag being published: review-list and known-mods rows then name its page")
    p.set_defaults(fn=cmd_rows)
    p = sub.add_parser("verify", help="compare dist/packs.json with directory rows; exit 1 on a difference")
    p.add_argument("repo")
    p.add_argument("--against", nargs="+", required=True, help="vanilla_plus.json, community.json, server_list.json or packs.json files")
    p.add_argument("--all", action="store_true", help="also list rows this repository has no pack for")
    p.set_defaults(fn=cmd_verify)
    p = sub.add_parser("lock", help="write packs.lock.json from dist/packs.json (commit it; CI verifies against it)")
    p.add_argument("repo")
    p.set_defaults(fn=cmd_lock)
    p = sub.add_parser("selftest", help="run the built-in test (no dc-modkit needed)")
    p.set_defaults(fn=cmd_selftest)
    a = ap.parse_args(argv)
    a.fn(a)


if __name__ == "__main__":
    main()

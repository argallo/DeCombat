#!/usr/bin/env python3
"""gunkit_examples.py: fill the `examples/` folder the GunKit package ships (docs/PACKS_FROM_REPOSITORIES.md section 4).

    python3 tools/gunkit_examples.py <out dir>                                   # the zips of the list's own release
    python3 tools/gunkit_examples.py <out dir> --release <owner/repo> <tag>      # the zips of that tagged release
    python3 tools/gunkit_examples.py <out dir> --base-url <URL or folder>         # the same assets from elsewhere (tests)
    python3 tools/gunkit_examples.py <out dir> --from <packs folder>              # copy the folders of a checkout
    python3 tools/gunkit_examples.py selftest

The list is tools/gunkit_examples.txt: a `release <owner/repo> <tag>` line (the tagged release the zips are assets
of; release.yml fetches from it) and one line per pack (id, version, sha256 of `<id>-<version>.zip`). Each zip is
checked against its line before it is unpacked into `<out dir>/<id>/`; a mismatch unpacks nothing. `--from` copies
`<packs folder>/<id>` (a checkout's examples/mods) without a hash check, for a local package. Python 3, standard
library only. Exit 0 ok, 1 a check failed, 2 usage.
"""
import argparse
import hashlib
import io
import os
import re
import shutil
import sys
import tempfile
import urllib.request
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
LIST = os.path.join(HERE, "gunkit_examples.txt")


def read_list(path=LIST):
    """The pack rows (id, version, sha256) of the list; the list's `release` line is read by list_release."""
    return read_list_full(path)[1]


def list_release(path=LIST):
    """(owner/repo, tag) of the list's `release` line, or None without one."""
    return read_list_full(path)[0]


def read_list_full(path=LIST):
    release, rows = None, []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split()
            if parts[0] == "release":
                if len(parts) != 3:
                    raise ValueError("the release line is `release <owner/repo> <tag>`")
                release = (parts[1], parts[2])
                continue
            mid, version, sha = parts
            rows.append((mid, version, sha.lower()))
    return release, rows


def fetch(base, name):
    if os.path.isdir(base):
        with open(os.path.join(base, name), "rb") as f:
            return f.read()
    with urllib.request.urlopen(f"{base.rstrip('/')}/{name}", timeout=120) as r:
        return r.read()


def unpack(data, dest):
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        for info in z.infolist():
            parts = info.filename.split("/")
            if any(p in ("", "..") for p in parts) or info.filename.startswith("/"):
                raise ValueError(f"unsafe path in zip: {info.filename}")
        z.extractall(dest)


def fill(out, base=None, from_dir=None, rows=None):
    rows = rows or read_list()
    staged = []
    for mid, version, sha in rows:
        if from_dir:
            src = os.path.join(from_dir, mid)
            if not os.path.isfile(os.path.join(src, "manifest.json")):
                print(f"FAIL {mid}: {src} has no manifest.json")
                return 1
            staged.append((mid, ("dir", src)))
            print(f"ok   {mid} {version} (folder {src})")
            continue
        name = f"{mid}-{version}.zip"
        try:
            data = fetch(base, name)
        except Exception as e:  # noqa: BLE001
            print(f"FAIL {mid} {version}: {name} not fetched: {e}")
            return 1
        got = hashlib.sha256(data).hexdigest()
        if got != sha:
            print(f"FAIL {mid} {version}: {name} hashes {got[:12]}, the list says {sha[:12]}: nothing unpacked")
            return 1
        staged.append((mid, ("zip", data)))
        print(f"ok   {mid} {version} {sha[:12]} ({len(data)} bytes)")
    # everything checked: now write
    os.makedirs(out, exist_ok=True)
    for mid, (kind, payload) in staged:
        dest = os.path.join(out, mid)
        if os.path.isdir(dest):
            shutil.rmtree(dest)
        if kind == "dir":
            shutil.copytree(payload, dest)
        else:
            os.makedirs(dest)
            unpack(payload, dest)
    print(f"{len(staged)} example(s) in {out}")
    return 0


def selftest():
    with tempfile.TemporaryDirectory(prefix="gunkit_examples_") as tmp:
        rel = os.path.join(tmp, "rel")
        os.makedirs(rel)
        rows = []
        for mid, ver in (("camo_a", "1.0.0"), ("camo_b", "2.0.0")):
            name = os.path.join(rel, f"{mid}-{ver}.zip")
            with zipfile.ZipFile(name, "w") as z:
                z.writestr("manifest.json", '{"id": "%s", "name": "%s", "version": "%s", "tags": ["camo"]}' % (mid, mid, ver))
                z.writestr("assets/p.dtx", b"x" * 64)
            rows.append((mid, ver, hashlib.sha256(open(name, "rb").read()).hexdigest()))
        out = os.path.join(tmp, "out")
        assert fill(out, base=rel, rows=rows) == 0
        assert os.path.isfile(os.path.join(out, "camo_a", "assets", "p.dtx"))
        assert os.path.isfile(os.path.join(out, "camo_b", "manifest.json"))
        # a wrong hash unpacks nothing, not even the good one
        bad = [rows[0], (rows[1][0], rows[1][1], "0" * 64)]
        out2 = os.path.join(tmp, "out2")
        assert fill(out2, base=rel, rows=bad) == 1
        assert not os.path.exists(out2)
        # a missing asset fails
        assert fill(os.path.join(tmp, "out3"), base=rel, rows=[("camo_c", "1.0.0", "f" * 64)]) == 1
        # --from copies folders
        out4 = os.path.join(tmp, "out4")
        assert fill(out4, from_dir=out, rows=rows) == 0
        assert os.path.isfile(os.path.join(out4, "camo_a", "manifest.json"))
        # a zip with an unsafe path is refused
        evil = os.path.join(rel, "camo_e-1.0.0.zip")
        with zipfile.ZipFile(evil, "w") as z:
            z.writestr("../escape.txt", "no")
        sha = hashlib.sha256(open(evil, "rb").read()).hexdigest()
        try:
            fill(os.path.join(tmp, "out5"), base=rel, rows=[("camo_e", "1.0.0", sha)])
            raise AssertionError("an unsafe zip path was accepted")
        except ValueError:
            pass
        # the real list parses and names a release (release.yml fetches from it)
        real = read_list()
        assert len(real) == 5 and all(len(r[2]) == 64 for r in real), real
        rel = list_release()
        assert rel == ("argallo/DeCombat-mods", "v1"), rel
        assert release_base(rel) == "https://github.com/argallo/DeCombat-mods/releases/download/v1"
        # a list with a branch as its release is refused; a list whose release line is malformed too
        bad = os.path.join(tmp, "bad.txt")
        with open(bad, "w", encoding="utf-8") as f:
            f.write("release example/packs main\ncamo_a 1.0.0 " + "a" * 64 + "\n")
        try:
            release_base(list_release(bad))
            raise AssertionError("a branch as the list's release was accepted")
        except SystemExit as e:
            assert e.code == 2
        with open(bad, "w", encoding="utf-8") as f:
            f.write("release example/packs\n")
        try:
            read_list_full(bad)
            raise AssertionError("a malformed release line was accepted")
        except ValueError:
            pass
    print("gunkit_examples.py selftest: ok")
    return 0


def release_base(release):
    """The download folder of a (owner/repo, tag) release; exits 2 for a branch name or a bad shape."""
    if not release:
        print("error: the list names no release (a `release <owner/repo> <tag>` line) and none was given", file=sys.stderr)
        sys.exit(2)
    repo, tag = release
    if not re.match(r"^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$", repo):
        print(f"error: {repo!r} is not owner/repo", file=sys.stderr)
        sys.exit(2)
    if tag in ("main", "master", "latest", "HEAD", "develop", "dev", "trunk") or not re.match(r"^(v[0-9]|[0-9])[A-Za-z0-9._-]*$", tag):
        print(f"error: {tag!r} is not a release tag (a branch moves; a pack is its hash)", file=sys.stderr)
        sys.exit(2)
    return f"https://github.com/{repo}/releases/download/{tag}"


def main(argv=None):
    if argv is None:
        argv = sys.argv[1:]
    if argv[:1] == ["selftest"]:
        return selftest()
    ap = argparse.ArgumentParser(prog="gunkit_examples.py", description=__doc__.split("\n\n")[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out")
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--release", nargs=2, metavar=("OWNER/REPO", "TAG"), help="a release other than the list's own")
    g.add_argument("--base-url")
    g.add_argument("--from", dest="from_dir")
    a = ap.parse_args(argv)
    if a.release:
        base = release_base(tuple(a.release))
    elif a.base_url or a.from_dir:
        base = a.base_url
    else:
        base = release_base(list_release())
    return fill(a.out, base=base, from_dir=a.from_dir)


if __name__ == "__main__":
    sys.exit(main())

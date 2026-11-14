#!/usr/bin/env python3
"""The gate of the operator dashboard's front end (web/admin, docs/ADMIN_WEB.md section 8).

The lobby embeds a built script and stylesheet (crates/dc-lobby-server/src/admin_app), because the servers build with
cargo alone. This check makes sure the committed files are what the committed sources build, and that nothing in them
breaks the dashboard's rules:

  1. npm ci --ignore-scripts      the exact packages of package-lock.json, no install scripts
  2. npm run typecheck, npm test
  3. npm run build                writes app.js, app.css and THIRD_PARTY_NOTICES.txt into the crate
  4. the three files are byte for byte the committed ones (git diff / untracked)
  5. the size budget
  6. hygiene: no source map, no eval, no address of another site, no local path, no web font
  7. NOTICE.md names every bundled package with its version

Usage:
    python tools/admin_web_check.py              # everything (CI runs this)
    python tools/admin_web_check.py --no-install # skip npm ci (node_modules is already the lockfile's)
    python tools/admin_web_check.py --files-only # steps 5 to 7 on the files as they are (no Node needed)

Exit code 0 when every step passes. Provenance: ORIGINAL (DeCombat tooling).
"""
import gzip
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WEB = os.path.join(ROOT, "web", "admin")
OUT_REL = "crates/dc-lobby-server/src/admin_app"
OUT = os.path.join(ROOT, *OUT_REL.split("/"))
BUILT = ["app.js", "app.css", "THIRD_PARTY_NOTICES.txt"]

# The budget (bytes). The script holds React and the design library's shared code before the first screen; the
# stylesheet is the library's whole sheet, which is not cut down to what is used. Raise a number only with a reason.
# 2026-11-11: the script's numbers are the size the owner accepted for the finished app (about 350 000 gzipped); the
# admin web lets a browser keep both files for a build (DC_DEFAULTS.md LG-443).
JS_RAW_MAX = 1_150_000
JS_GZIP_MAX = 350_000
CSS_RAW_MAX = 340_000

# Addresses that may appear as text in the script: XML namespace names and React's error-message link. None is fetched.
URL_OK = ("http://www.w3.org/", "https://react.dev/errors/")


def fail(msg):
    print(f"admin_web_check: FAIL: {msg}")
    sys.exit(1)


def run(args, cwd):
    exe = shutil.which(args[0])
    if exe is None:
        fail(f"{args[0]} is not on PATH (Node.js {open(os.path.join(WEB, '.nvmrc')).read().strip()} is needed)")
    print("+ " + " ".join(args), flush=True)
    r = subprocess.run([exe] + args[1:], cwd=cwd)
    if r.returncode != 0:
        fail(f"{' '.join(args)} exited with {r.returncode}")


def git(args):
    r = subprocess.run(["git", "-C", ROOT] + args, capture_output=True, text=True)
    if r.returncode not in (0, 1):
        fail(f"git {' '.join(args)}: {r.stderr.strip()}")
    return r.returncode, r.stdout


def read(name):
    with open(os.path.join(OUT, name), "rb") as f:
        return f.read()


def check_same_as_committed():
    code, _ = git(["diff", "--quiet", "--", OUT_REL])
    _, untracked = git(["status", "--porcelain", "--", OUT_REL])
    if code != 0 or untracked.strip():
        _, stat = git(["status", "--short", "--", OUT_REL])
        fail(
            "the built files differ from the committed ones: commit what `npm run build` wrote, "
            "together with the source change\n" + stat
        )
    print("ok: the committed bundle is what the sources build")


def check_budget():
    js, css = read("app.js"), read("app.css")
    js_gz = len(gzip.compress(js, compresslevel=9, mtime=0))
    css_gz = len(gzip.compress(css, compresslevel=9, mtime=0))
    print(f"app.js  {len(js):>8} bytes, {js_gz:>7} gzip   (budget {JS_RAW_MAX} / {JS_GZIP_MAX})")
    print(f"app.css {len(css):>8} bytes, {css_gz:>7} gzip   (budget {CSS_RAW_MAX})")
    if len(js) > JS_RAW_MAX or js_gz > JS_GZIP_MAX:
        fail("app.js is over its budget")
    if len(css) > CSS_RAW_MAX:
        fail("app.css is over its budget")


def check_hygiene():
    js = read("app.js").decode("utf-8")
    css = read("app.css").decode("utf-8")
    notices = read(BUILT[2]).decode("utf-8")
    for name, text in (("app.js", js), ("app.css", css), (BUILT[2], notices)):
        if "\r" in text:
            fail(f"{name} holds a carriage return: the files are bytes, LF only")
        if re.search(r"[A-Za-z]:[\\/]+Users|/Users/|/home/[a-z]", text):
            fail(f"{name} holds a path of the machine that built it")
    for name, text in (("app.js", js), ("app.css", css)):
        if "sourceMappingURL" in text:
            fail(f"{name} refers to a source map")
        for url in re.findall(r"""(?:https?:)?//[A-Za-z0-9.-]+\.[A-Za-z]{2,}[^\s"'`)<>\\]*""", text):
            if not url.startswith(URL_OK):
                fail(f"{name} names another site: {url}")
    if re.search(r"(?<![A-Za-z0-9_$.])eval\s*\(", js) or re.search(r"new\s+Function\s*\(", js):
        fail("app.js uses eval or new Function: the content security policy forbids both")
    if re.search(r"@import|@font-face", css):
        fail("app.css imports a sheet or declares a web font: only the system font is used")
    if re.search(r"url\(\s*['\"]?(?!data:)", css):
        fail("app.css loads a file by url(): the policy allows no such request")
    print("ok: no source map, eval, other site, local path or web font in the bundle")


def check_notice():
    notices = read(BUILT[2]).decode("utf-8")
    packages = re.findall(r"^-{20,}\n(\S+) (\S+)\n", notices, flags=re.M)
    if not packages:
        fail(f"{BUILT[2]} lists no package")
    with open(os.path.join(ROOT, "NOTICE.md"), encoding="utf-8") as f:
        notice = f.read()
    missing = [f"{n} {v}" for n, v in packages if f"`{n}` {v}" not in notice]
    if missing:
        fail("NOTICE.md does not name these bundled packages with their version: " + ", ".join(missing))
    print(f"ok: NOTICE.md names the {len(packages)} bundled packages")


def main():
    args = sys.argv[1:]
    unknown = [a for a in args if a not in ("--no-install", "--files-only")]
    if unknown:
        print(__doc__)
        fail(f"unknown argument {unknown[0]}")
    if "--files-only" not in args:
        npm = "npm.cmd" if os.name == "nt" else "npm"
        if "--no-install" not in args:
            run([npm, "ci", "--ignore-scripts", "--no-audit", "--no-fund"], WEB)
        run([npm, "run", "typecheck"], WEB)
        run([npm, "test"], WEB)
        run([npm, "run", "build"], WEB)
        check_same_as_committed()
    check_budget()
    check_hygiene()
    check_notice()
    print("admin_web_check: ok")


if __name__ == "__main__":
    main()

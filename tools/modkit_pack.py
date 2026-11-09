#!/usr/bin/env python3
"""modkit_pack.py: the ModKit package (DC_DEFAULTS.md MOD-30): a zip with the prebuilt `dc-modkit` command-line tool
and a README, so mod authors and pack-repository workflows (tools/modrepo.py) need no Rust toolchain. One zip per
platform, built by .github/workflows/release.yml: Windows x86_64, macOS universal, Linux x86_64.

  python tools/modkit_pack.py --exe <dc-modkit or dc-modkit.exe> --platform win|mac|linux-x86_64 --name NAME [--out DIR]
  python tools/modkit_pack.py selftest

The zip holds `<NAME>/dc-modkit[.exe]` (executable bit set for macOS and Linux) and `<NAME>/README.md`. Nothing of the
game (ROM) and nothing of the building machine goes in. Standard library only, so every runner and machine runs it.
"""
# Provenance: ORIGINAL
import argparse
import os
import sys
import tempfile
import zipfile

README = """# DeCombat ModKit ({platform})

`{exe}` is the command-line tool for DeCombat mods: it writes a new mod folder, checks it and packs it into the
deterministic zip whose SHA-256 is the mod's identity (the same bytes give the same hash on every machine).

    {run} scaffold my_mod --kind client_visual     # a new mod folder
    {run} validate my_mod                          # the manifest, the entry script, the capabilities
    {run} lint my_mod                              # the pack rules (sizes, names, pictures)
    {run} pack my_mod                              # my_mod-<version>.zip and its sha256

The modding guide is docs/MODDING_GUIDE.md in the DeCombat repository; a pack repository (many mods, a release
workflow, the review rows) is set up with tools/modrepo.py there, whose workflow downloads this package.
{extra}
Check the zip against the release's SHA256SUMS before you run it. This package was built from the release's commit
by the release workflow; it reads only the folders you give it.
"""
MAC_EXTRA = """
macOS: the binary is universal (Apple Silicon and Intel) and signed ad hoc, not notarised. A download carries the
quarantine flag; clear it once with `xattr -d com.apple.quarantine dc-modkit` in the unpacked folder.
"""
LINUX_EXTRA = """
Linux: built on Ubuntu 22.04 (glibc 2.35); it runs on that and newer distributions.
"""


def pack(exe, platform, name, out):
    if not os.path.isfile(exe):
        print(f"no {exe}: build it first (cargo build --release -p dc-modkit)", file=sys.stderr)
        return 2
    if platform not in ("win", "mac", "linux-x86_64"):
        print(f"--platform {platform!r}: win, mac or linux-x86_64", file=sys.stderr)
        return 2
    exe_name = "dc-modkit.exe" if platform == "win" else "dc-modkit"
    run = exe_name if platform == "win" else "./dc-modkit"
    extra = {"mac": MAC_EXTRA, "linux-x86_64": LINUX_EXTRA}.get(platform, "")
    readme = README.format(platform={"win": "Windows x86_64", "mac": "macOS universal", "linux-x86_64": "Linux x86_64"}[platform],
                           exe=exe_name, run=run, extra=extra)
    os.makedirs(out, exist_ok=True)
    zpath = os.path.join(out, f"{name}.zip")
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        info = zipfile.ZipInfo(f"{name}/{exe_name}", date_time=(2026, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = (0o100755 if platform != "win" else 0o100644) << 16
        with open(exe, "rb") as f:
            z.writestr(info, f.read())
        r = zipfile.ZipInfo(f"{name}/README.md", date_time=(2026, 1, 1, 0, 0, 0))
        r.compress_type = zipfile.ZIP_DEFLATED
        r.external_attr = 0o100644 << 16
        z.writestr(r, readme)
    print(f"{zpath}: {exe_name} ({os.path.getsize(exe)} bytes) + README.md")
    return 0


def selftest():
    with tempfile.TemporaryDirectory(prefix="modkit_pack_") as tmp:
        exe = os.path.join(tmp, "dc-modkit")
        with open(exe, "wb") as f:
            f.write(b"\x7fELF fake")
        assert pack(exe, "linux-x86_64", "DeCombat-ModKit-linux-x86_64-v9", os.path.join(tmp, "out")) == 0
        with zipfile.ZipFile(os.path.join(tmp, "out", "DeCombat-ModKit-linux-x86_64-v9.zip")) as z:
            names = sorted(z.namelist())
            assert names == ["DeCombat-ModKit-linux-x86_64-v9/README.md", "DeCombat-ModKit-linux-x86_64-v9/dc-modkit"], names
            mode = z.getinfo("DeCombat-ModKit-linux-x86_64-v9/dc-modkit").external_attr >> 16
            assert mode & 0o111, oct(mode)
            assert z.read("DeCombat-ModKit-linux-x86_64-v9/dc-modkit") == b"\x7fELF fake"
            assert b"glibc 2.35" in z.read("DeCombat-ModKit-linux-x86_64-v9/README.md")
        assert pack(exe, "win", "DeCombat-ModKit-win-v9", os.path.join(tmp, "out")) == 0
        with zipfile.ZipFile(os.path.join(tmp, "out", "DeCombat-ModKit-win-v9.zip")) as z:
            assert "DeCombat-ModKit-win-v9/dc-modkit.exe" in z.namelist()
        assert pack(exe, "mac", "DeCombat-ModKit-mac-v9", os.path.join(tmp, "out")) == 0
        with zipfile.ZipFile(os.path.join(tmp, "out", "DeCombat-ModKit-mac-v9.zip")) as z:
            assert b"xattr -d com.apple.quarantine" in z.read("DeCombat-ModKit-mac-v9/README.md")
        assert pack(os.path.join(tmp, "missing"), "win", "x", tmp) == 2
        assert pack(exe, "beos", "x", tmp) == 2
    print("modkit_pack.py selftest: ok")
    return 0


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv[:1] == ["selftest"]:
        return selftest()
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", required=True, help="the built dc-modkit (dc-modkit.exe on Windows; a universal binary on macOS)")
    ap.add_argument("--platform", required=True, choices=["win", "mac", "linux-x86_64"])
    ap.add_argument("--name", required=True, help="zip base name, e.g. DeCombat-ModKit-win-<tag>")
    ap.add_argument("--out", default="out_modkit", help="folder for the zip (default out_modkit/)")
    a = ap.parse_args(argv)
    return pack(a.exe, a.platform, a.name, a.out)


if __name__ == "__main__":
    sys.exit(main())

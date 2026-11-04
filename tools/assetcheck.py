#!/usr/bin/env python3
"""assetcheck: what a contributor must not commit.

usage: assetcheck.py [--root DIR] [--max-mb N]

Refuses game files by extension, oversized files, private key blocks and lines holding a private marker. Scans the
tracked files of a git checkout, or every file of a folder that is not one. Output: 'ASSET FAIL kind file[:line] what',
then a summary line. Exit 1 on any failure. Python 3, standard library only.
"""
import argparse
import os
import re
import subprocess
import sys

# Game and engine formats, and programs: never in the tree.
BANNED_EXT = {
    '.rez', '.ltb', '.ltc', '.lta', '.ltu', '.spr', '.fxd', '.cres', '.arch', '.pcx', '.tga', '.dds',
    '.dll', '.exe', '.pdb', '.lib', '.sys', '.so', '.dylib',
}
# Formats our own tools also write: only where maps made with them live.
OWN_FORMATS = {'.dat', '.dtx'}
OWN_FORMAT_DIRS = ('examples/mods/', 'examples/mapkit/')
# The marker words are split so that this file does not trip over itself.
MARKERS = re.compile('DC-' + 'PRIVATE|DO-NOT-' + 'COMMIT')
KEY_BLOCK = re.compile('-----BEGIN (?:[A-Z0-9]+ )*PRIV' + 'ATE KEY-----')
MAX_TEXT = 8 * 1024 * 1024


def tracked(root):
    git = subprocess.run(['git', '-C', root, 'ls-files', '-z'], capture_output=True)
    if git.returncode == 0:
        return [f for f in git.stdout.decode('utf-8', 'replace').split('\0') if f]
    out = []
    for d, dirs, names in os.walk(root):
        dirs[:] = [x for x in dirs if x not in ('.git', 'target', '__pycache__')]
        out += [os.path.relpath(os.path.join(d, n), root).replace(os.sep, '/') for n in names]
    return out


def check(root, max_mb):
    fails = []
    files = tracked(root)
    limit = max_mb * 1024 * 1024
    for f in files:
        path = os.path.join(root, f)
        if not os.path.isfile(path):
            continue
        ext = os.path.splitext(f)[1].lower()
        if ext in BANNED_EXT:
            fails.append(('EXT', f, 'game or program file (%s)' % ext))
        elif ext in OWN_FORMATS and not f.startswith(OWN_FORMAT_DIRS):
            fails.append(('EXT', f, '%s only under %s' % (ext, ' or '.join(OWN_FORMAT_DIRS))))
        size = os.path.getsize(path)
        if size > limit:
            fails.append(('SIZE', f, '%.1f MB is over %d MB' % (size / 1048576, max_mb)))
        # these two name the markers on purpose
        if size > MAX_TEXT or f in ('tools/assetcheck.py', 'tools/README.md'):
            continue
        with open(path, 'rb') as fh:
            raw = fh.read()
        if b'\0' in raw[:8192]:
            continue
        for n, line in enumerate(raw.decode('utf-8', 'replace').splitlines(), 1):
            if MARKERS.search(line):
                fails.append(('MARKER', '%s:%d' % (f, n), 'line marked private'))
            if KEY_BLOCK.search(line) and '"' not in line and "'" not in line:
                fails.append(('KEY', '%s:%d' % (f, n), 'private key block'))
    return files, fails


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--root', default='.', help='checkout or folder to scan (default: current directory)')
    ap.add_argument('--max-mb', type=int, default=10, help='largest file allowed, in MB (default 10)')
    a = ap.parse_args()
    files, fails = check(a.root, a.max_mb)
    for kind, where, what in fails:
        print('ASSET FAIL %s %s %s' % (kind, where, what))
    print('assetcheck files=%d fail=%d' % (len(files), len(fails)))
    return 1 if fails else 0


if __name__ == '__main__':
    sys.exit(main())

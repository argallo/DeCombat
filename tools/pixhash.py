#!/usr/bin/env python3
"""Texel hashes of PNG frames (render determinism contract, docs/RENDER_DETERMINISM_CONTRACT.md, gate G0).

Hashes the intended texels of every PNG of a folder (a `dc-client --headless-frames N --out DIR` run of any build,
or the exact-lane PNGs of DC_EXACT_PNG) exactly as `dc_render::texel_hash` does: SHA-256 of
'dc-texels/1 rgba8 <w>x<h>\\n' followed by the RGBA8 texels, rows top to bottom, no padding. With a second folder it
compares the two runs file by file (two branches' binaries on the same scripted scene) and prints, per differing
image, the number of differing pixels, the largest channel difference and the first differing pixel.

Usage:
    python tools/pixhash.py DIR                 # one line per PNG: hash, size, file
    python tools/pixhash.py DIR_A DIR_B         # compare by file name; exit 1 when any image differs
    python tools/pixhash.py --json OUT DIR_A [DIR_B]   # also write the hashes / the comparison as JSON
    python tools/pixhash.py --tolerance 3 [--max-fraction 0.001] DIR_A DIR_B
                                                # hardware-coverage lane: an image passes when no channel differs by
                                                # more than 3, or at most that fraction of its pixels does

The PNG reader is our own (zlib + the five PNG filters; 8-bit greyscale, grey+alpha, RGB, RGBA and palette, not
interlaced), so the tool needs nothing beyond the Python standard library. It never writes images.
"""
# Provenance: ORIGINAL (PNG decoding from the public PNG specification, ISO/IEC 15948)
import argparse
import hashlib
import json
import os
import struct
import sys
import zlib

CONTRACT = 'dc-texels/1'


def read_png(path):
    """(width, height, RGBA8 bytes) of an 8-bit, non-interlaced PNG."""
    with open(path, 'rb') as f:
        data = f.read()
    if data[:8] != b'\x89PNG\r\n\x1a\n':
        raise ValueError('%s: not a PNG' % path)
    pos = 8
    idat = []
    palette = None
    trns = None
    w = h = depth = ctype = interlace = None
    while pos < len(data):
        n, kind = struct.unpack('>I4s', data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + n]
        pos += 12 + n
        if kind == b'IHDR':
            w, h, depth, ctype, _, _, interlace = struct.unpack('>IIBBBBB', body)
        elif kind == b'PLTE':
            palette = body
        elif kind == b'tRNS':
            trns = body
        elif kind == b'IDAT':
            idat.append(body)
        elif kind == b'IEND':
            break
    if depth != 8 or interlace != 0:
        raise ValueError('%s: only 8-bit non-interlaced PNGs (depth %s, interlace %s)' % (path, depth, interlace))
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    raw = zlib.decompress(b''.join(idat))
    stride = w * channels
    out = bytearray(h * stride)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        ft = raw[p]
        line = bytearray(raw[p + 1:p + 1 + stride])
        p += 1 + stride
        bpp = channels
        if ft == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xff
        elif ft == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xff
        elif ft == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xff
        elif ft == 4:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xff
        elif ft != 0:
            raise ValueError('%s: bad filter %d' % (path, ft))
        out[y * stride:(y + 1) * stride] = line
        prev = line
    if ctype == 6:
        return w, h, bytes(out)
    rgba = bytearray(w * h * 4)
    for i in range(w * h):
        if ctype == 2:
            r, g, b = out[i * 3:i * 3 + 3]
            a = 255
        elif ctype == 0:
            r = g = b = out[i]
            a = 255
        elif ctype == 4:
            r = g = b = out[i * 2]
            a = out[i * 2 + 1]
        else:
            k = out[i]
            r, g, b = palette[k * 3:k * 3 + 3]
            a = trns[k] if trns is not None and k < len(trns) else 255
        rgba[i * 4:i * 4 + 4] = bytes((r, g, b, a))
    return w, h, bytes(rgba)


def texel_hash(w, h, rgba):
    s = hashlib.sha256()
    s.update(('%s rgba8 %dx%d\n' % (CONTRACT, w, h)).encode('ascii'))
    s.update(rgba)
    return s.hexdigest()


def pngs(folder):
    return sorted(f for f in os.listdir(folder) if f.lower().endswith('.png'))


def compare(a, b, tol=0):
    """(differing pixels, largest channel difference, first differing (x, y, a_rgba, b_rgba), pixels over `tol`)."""
    (wa, ha, pa), (wb, hb, pb) = a, b
    if (wa, ha) != (wb, hb):
        return None
    n = 0
    over = 0
    worst = 0
    first = None
    for i in range(0, len(pa), 4):
        if pa[i:i + 4] != pb[i:i + 4]:
            n += 1
            d = max(abs(pa[i + k] - pb[i + k]) for k in range(4))
            worst = max(worst, d)
            if d > tol:
                over += 1
            if first is None:
                px = i // 4
                first = (px % wa, px // wa, tuple(pa[i:i + 4]), tuple(pb[i:i + 4]))
    return n, worst, first, over


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('dirs', nargs='+', help='one folder to hash, or two to compare')
    ap.add_argument('--json', help='write the result as JSON here')
    ap.add_argument('--tolerance', type=int, default=None,
                    help='hardware-coverage lane: largest channel difference an image may have and still pass')
    ap.add_argument('--max-fraction', type=float, default=0.0,
                    help='with --tolerance: fraction of pixels allowed beyond the tolerance (default 0)')
    a = ap.parse_args()
    if len(a.dirs) > 2:
        sys.exit('pixhash: one or two folders')
    result = {'contract': CONTRACT, 'images': {}}
    if len(a.dirs) == 1:
        for f in pngs(a.dirs[0]):
            w, h, px = read_png(os.path.join(a.dirs[0], f))
            hx = texel_hash(w, h, px)
            print('%s %dx%d %s' % (hx, w, h, f))
            result['images'][f] = {'sha256': hx, 'width': w, 'height': h}
        code = 0
    else:
        da, db = a.dirs
        names = sorted(set(pngs(da)) | set(pngs(db)))
        same = differ = missing = within = 0
        tol = a.tolerance
        for f in names:
            pa_, pb_ = os.path.join(da, f), os.path.join(db, f)
            if not (os.path.exists(pa_) and os.path.exists(pb_)):
                print('MISSING  %s (only in %s)' % (f, da if os.path.exists(pa_) else db))
                result['images'][f] = {'missing_in': 'B' if os.path.exists(pa_) else 'A'}
                missing += 1
                continue
            ia, ib = read_png(pa_), read_png(pb_)
            ha_, hb_ = texel_hash(*ia), texel_hash(*ib)
            if ha_ == hb_:
                print('SAME     %s %s' % (ha_[:16], f))
                result['images'][f] = {'sha256': ha_, 'same': True}
                same += 1
                continue
            differ += 1
            c = compare(ia, ib, tol or 0)
            if c is None:
                print('DIFFER   %s: sizes %dx%d vs %dx%d' % (f, ia[0], ia[1], ib[0], ib[1]))
                result['images'][f] = {'a': ha_, 'b': hb_, 'sizes_differ': True}
                continue
            n, worst, first, over = c
            total = ia[0] * ia[1]
            ok = tol is not None and over <= a.max_fraction * total
            within += ok
            print('%s %s %s vs %s: %d of %d pixels (%.3f %%), max channel diff %d, first at (%d, %d) %s vs %s'
                  % ('WITHIN  ' if ok else 'DIFFER  ', f, ha_[:16], hb_[:16], n, total, 100.0 * n / total, worst, first[0], first[1],
                     first[2], first[3]))
            result['images'][f] = {'a': ha_, 'b': hb_, 'pixels': n, 'of': total, 'largest_channel_diff': worst,
                                   'over_tolerance': over, 'within_tolerance': ok,
                                   'first': [first[0], first[1], list(first[2]), list(first[3])]}
        print('%d same, %d differ (%d of them within the tolerance), %d missing' % (same, differ, within, missing))
        result['summary'] = {'same': same, 'differ': differ, 'within_tolerance': within, 'missing': missing,
                             'tolerance': tol, 'max_fraction': a.max_fraction}
        code = 1 if (differ - within) or missing else 0
    if a.json:
        with open(a.json, 'w', encoding='utf-8') as f:
            json.dump(result, f, indent=1)
    return code


if __name__ == '__main__':
    sys.exit(main())

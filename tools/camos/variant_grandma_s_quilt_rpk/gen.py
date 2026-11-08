# Grandma's Quilt: procedural patchwork, tileable 512x512. Original art (numpy/Pillow only).
import sys, numpy as np
from PIL import Image
N = 512
P = 128            # patch size -> 4x4 patches per tile
rng = np.random.default_rng(int(sys.argv[1]) if len(sys.argv) > 1 else 7)

# palette (mid-tone, saturated), each with a ground and a contrasting accent
RED    = np.array([0.50, 0.09, 0.10]); RED_L  = np.array([0.62, 0.38, 0.30])
MUST   = np.array([0.46, 0.30, 0.05]); MUST_L = np.array([0.56, 0.45, 0.24])
TEAL   = np.array([0.04, 0.30, 0.31]); TEAL_L = np.array([0.32, 0.52, 0.48])
PLUM   = np.array([0.24, 0.08, 0.30]); PLUM_L = np.array([0.42, 0.30, 0.48])
CREAM  = np.array([0.66, 0.58, 0.42])
COLS = [(RED, RED_L), (MUST, MUST_L), (TEAL, TEAL_L), (PLUM, PLUM_L)]

yy, xx = np.mgrid[0:N, 0:N].astype(np.float32)

def fabric(kind, ground, accent, ox, oy):
    """Return an N x N x 3 image of a fabric (coordinates periodic over the patch grid)."""
    img = np.empty((N, N, 3), np.float32); img[:] = ground
    x = xx + ox; y = yy + oy
    if kind == 'gingham':
        s = 32.0  # 4 checks per patch
        a = (np.floor(x / s) % 2).astype(bool); b = (np.floor(y / s) % 2).astype(bool)
        mix = (a.astype(np.float32) + b.astype(np.float32)) / 2   # 0, .5, 1
        img = ground[None, None] * (1 - mix[..., None]) + accent[None, None] * mix[..., None]
    elif kind == 'dots':
        s = 42.667
        gx = (x % s) - s / 2; gy = (y % s) - s / 2
        # offset rows
        row = np.floor(y / s) % 2
        gx2 = ((x + s / 2) % s) - s / 2
        gx = np.where(row == 1, gx2, gx)
        m = (gx ** 2 + gy ** 2) < 11.5 ** 2
        img[m] = accent
    elif kind == 'stripes':
        s = 32.0   # 4 stripes per patch
        m = ((x + y) % s) < s * 0.45 if rng.random() < 0.0 else (x % s) < s * 0.45
        img[m] = accent
    elif kind == 'dstripes':
        s = 42.667
        m = ((x + y) % s) < s * 0.42
        img[m] = accent
    elif kind == 'floral':
        s = 42.667  # 3 flowers per patch side
        cx = (x % s) - s / 2; cy = (y % s) - s / 2
        r = np.hypot(cx, cy); th = np.arctan2(cy, cx)
        petal = r < (11 + 8 * np.abs(np.cos(2.5 * th)))
        img[petal] = accent
        img[r < 5.5] = CREAM * 0.9 if not np.allclose(accent, MUST_L) else RED
        # little leaves between flowers
        lx = ((x + s / 2) % s) - s / 2; ly = ((y + s / 2) % s) - s / 2
        leaf = (lx ** 2 / 30 + ly ** 2 / 6) < 1
        img[leaf & ~petal] = np.array([0.14, 0.32, 0.12])
    return img

kinds = ['gingham', 'dots', 'stripes', 'floral', 'dstripes']
out = np.zeros((N, N, 3), np.float32)
G = N // P
ci, ki = rng.integers(0, 4, (G, G)), rng.integers(0, 5, (G, G))
# avoid same colour as the left/up neighbour
for j in range(G):
    for i in range(G):
        while ci[j, i] == ci[j, (i - 1) % G] or ci[j, i] == ci[(j - 1) % G, i]:
            ci[j, i] = rng.integers(0, 4)
        while ki[j, i] == ki[j, (i - 1) % G]:
            ki[j, i] = rng.integers(0, 5)
tri = rng.random((G, G)) < 0.45      # half-square triangle patches
diag = rng.integers(0, 2, (G, G))

lx = xx % P; ly = yy % P
pi = (xx // P).astype(int); pj = (yy // P).astype(int)
for j in range(G):
    for i in range(G):
        cell = (pi == i) & (pj == j)
        g, a = COLS[ci[j, i]]
        f = fabric(kinds[ki[j, i]], g, a, rng.uniform(0, 30), rng.uniform(0, 30))
        out[cell] = f[cell]
        if tri[j, i]:
            c2 = (ci[j, i] + 1 + rng.integers(0, 3)) % 4
            k2 = (ki[j, i] + 1 + rng.integers(0, 4)) % 5
            g2, a2 = COLS[c2]
            f2 = fabric(kinds[k2], g2, a2, rng.uniform(0, 30), rng.uniform(0, 30))
            half = (lx > ly) if diag[j, i] == 0 else (lx + ly > P)
            m = cell & half
            out[m] = f2[m]

# seams: darker thin line on patch borders and the triangle diagonals
def dist_edge(v):
    return np.minimum(v % P, P - (v % P))
de = np.minimum(dist_edge(xx), dist_edge(yy))
dd = np.full((N, N), 1e9, np.float32)
for j in range(G):
    for i in range(G):
        if tri[j, i]:
            cell = (pi == i) & (pj == j)
            d = np.abs(lx - ly) / np.sqrt(2) if diag[j, i] == 0 else np.abs(lx + ly - P) / np.sqrt(2)
            dd = np.where(cell, d, dd)
seam = np.minimum(de, dd)
out = np.where((seam < 1.8)[..., None], out * 0.45, out)

# dashed stitching: running stitches 6 px in from each edge
STI = np.array([0.70, 0.64, 0.48])
def dash(t):  # along-the-line coordinate
    return (t % 14) < 8
inset = 7.0
sx = (np.abs((xx % P) - inset) < 1.6) | (np.abs((xx % P) - (P - inset)) < 1.6)
sy = (np.abs((yy % P) - inset) < 1.6) | (np.abs((yy % P) - (P - inset)) < 1.6)
inside_x = ((yy % P) > inset - 2) & ((yy % P) < P - inset + 2)
inside_y = ((xx % P) > inset - 2) & ((xx % P) < P - inset + 2)
st = (sx & inside_x & dash(yy)) | (sy & inside_y & dash(xx))
# stitching along diagonals, both sides
for j in range(G):
    for i in range(G):
        if tri[j, i]:
            cell = (pi == i) & (pj == j) & (np.minimum(lx, ly) > inset) & (np.maximum(lx, ly) < P - inset)
            if diag[j, i] == 0:
                d = (lx - ly) / np.sqrt(2); t = (lx + ly) / np.sqrt(2)
            else:
                d = (lx + ly - P) / np.sqrt(2); t = (lx - ly) / np.sqrt(2)
            st |= cell & (np.abs(np.abs(d) - 6.0) < 1.5) & dash(t)
# stitch shadow (1px down-right) for a raised look
sh = np.roll(np.roll(st, 1, 0), 1, 1) & ~st
out[sh] *= 0.6
out[st] = STI

out = np.clip(out * 0.92, 0, 1)
Image.fromarray((out * 255 + 0.5).astype(np.uint8)).save(sys.argv[2] if len(sys.argv) > 2 else 'paint.png')

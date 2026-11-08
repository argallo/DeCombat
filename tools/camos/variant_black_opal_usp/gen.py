# Black Opal paint for the USP: procedural, tileable (torus Voronoi + periodic noise).
import numpy as np
from PIL import Image
import sys, os
N = 512
rng = np.random.default_rng(563)
y, x = np.mgrid[0:N, 0:N] / N

def pnoise(octaves, seed):
    r = np.random.default_rng(seed)
    out = np.zeros((N, N))
    for f, a in octaves:
        for _ in range(4):
            kx, ky = r.integers(-f, f + 1, 2)
            if kx == 0 and ky == 0: kx = f
            out += a * np.sin(2 * np.pi * (kx * x + ky * y) + r.uniform(0, 2 * np.pi))
    return out / (np.abs(out).max() + 1e-9)

# warp field so the patches are irregular
wx = pnoise([(2, 1.0), (4, 0.5)], 1) * 0.05
wy = pnoise([(2, 1.0), (4, 0.5)], 2) * 0.05
px, py = (x + wx) % 1, (y + wy) % 1

K = int(sys.argv[1]) if len(sys.argv) > 1 else 52
pts = rng.random((K, 2))
d1 = np.full((N, N), 9.0); d2 = np.full((N, N), 9.0); idx = np.zeros((N, N), int)
for i, (cx, cy) in enumerate(pts):
    dx = np.abs(px - cx); dx = np.minimum(dx, 1 - dx)
    dy = np.abs(py - cy); dy = np.minimum(dy, 1 - dy)
    d = np.sqrt(dx * dx + dy * dy)
    closer = d < d1
    d2 = np.where(closer, d1, np.minimum(d2, d))
    idx = np.where(closer, i, idx); d1 = np.where(closer, d, d1)
edge = d2 - d1   # distance to cell border

pal = np.array([
    [0.06, 0.66, 0.16],  # electric green
    [0.04, 0.55, 0.58],  # teal
    [0.06, 0.26, 0.74],  # electric blue
    [0.66, 0.05, 0.52],  # magenta
    [0.76, 0.28, 0.02],  # orange fire
])
ci = rng.integers(0, len(pal), K)
cell_col = pal[ci]
cell_on = rng.random(K) < 0.56          # some cells stay black
cell_ang = rng.uniform(0, 2 * np.pi, K)
cell_pal2 = pal[(ci + rng.choice([-1, 1], K)) % len(pal)]   # neighbour hue: the patch shifts colour

col = cell_col[idx]; col2 = cell_pal2[idx]
# play of colour inside each patch: bands drifting towards the neighbour hue, with a bright sheen line
ang = cell_ang[idx]
s = np.cos(ang) * (((px - pts[idx, 0] + 0.5) % 1) - 0.5) + np.sin(ang) * (((py - pts[idx, 1] + 0.5) % 1) - 0.5)
s = s + 0.015 * pnoise([(6, 1.0)], 7)
t = 0.5 + 0.5 * np.sin(s * 2 * np.pi * 5)
fire = col * (1 - 0.6 * t[..., None]) + col2 * 0.6 * t[..., None]
sheen = (0.5 + 0.5 * np.sin(s * 2 * np.pi * 5 + 1.3)) ** 6
fire = fire * (0.8 + 0.5 * sheen[..., None])
# brightness: full to the edge between lit patches (harlequin opal), with a shimmer grain
fn = pnoise([(10, 1.0), (20, 0.7), (28, 0.4)], 3) * 0.5 + 0.5
on = cell_on[idx].astype(float)
inten = on * (0.62 + 0.38 * fn)
# thin bright seam where two lit patches meet (a flash line), no dark outlines
seam = np.clip(1 - edge / 0.006, 0, 1) * on
# a large-scale mask so whole regions go dark (black opal body)
big = pnoise([(1, 1.0), (2, 0.7), (3, 0.4)], 4) * 0.5 + 0.5
inten *= np.clip((big - 0.22) / 0.3, 0.2, 1.0)
fire = np.clip(fire * (1 + 0.35 * seam[..., None]), 0, 0.85)

base = np.array([0.05, 0.07, 0.17])
bn = pnoise([(3, 1.0), (6, 0.5)], 5)
basec = base * (1 + 0.25 * bn[..., None])
img = basec * (1 - inten[..., None]) + fire * inten[..., None]
img = np.clip(img, 0, 1)
out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(os.path.abspath(__file__)), 'paint.png')
Image.fromarray((img * 255).astype(np.uint8)).save(out)
print('wrote', out, img.mean(axis=(0, 1)))

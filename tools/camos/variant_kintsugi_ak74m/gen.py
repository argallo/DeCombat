# Kintsugi: red and black lacquer shards mended with gold seams. Procedural, tileable on the torus.
import os, numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(74)
y, x = np.mgrid[0:N, 0:N] / N
TAU = 2 * np.pi

def tnoise(seed, terms=6, fmax=5):
    r = np.random.default_rng(seed); a = np.zeros((N, N))
    for _ in range(terms):
        fx, fy = r.integers(-fmax, fmax + 1, 2)
        if fx == 0 and fy == 0: fx = 1
        a += np.sin(TAU * (fx * x + fy * y) + r.uniform(0, TAU)) / np.hypot(fx, fy)
    return a / np.abs(a).max()

def voronoi(seeds):
    # wrapped vectors pixel->seed, nearest index and distance to nearest bisector
    vx = (seeds[:, 0][:, None, None] - x[None]); vx -= np.round(vx)
    vy = (seeds[:, 1][:, None, None] - y[None]); vy -= np.round(vy)
    d2 = vx * vx + vy * vy
    i1 = np.argmin(d2, 0)
    v1x = np.take_along_axis(vx, i1[None], 0)[0]; v1y = np.take_along_axis(vy, i1[None], 0)[0]
    d1 = v1x * v1x + v1y * v1y
    ex = vx - v1x; ey = vy - v1y
    den = 2 * np.sqrt(ex * ex + ey * ey) + 1e-9
    bis = (d2 - d1) / den
    bis[i1[None] == np.arange(len(seeds))[:, None, None]] = 9
    i2 = np.argmin(bis, 0)
    return i1, bis.min(0), v1x, v1y, i2

# warp coordinates slightly so seams wander like real cracks
wx = tnoise(1, 8, 5) * 0.010 + tnoise(11, 14, 24) * 0.0035
wy = tnoise(2, 8, 5) * 0.010 + tnoise(12, 14, 24) * 0.0035
x0, y0 = x, y
x = (x0 + wx) % 1; y = (y0 + wy) % 1

# jittered grid of shards: 4x4 -> big shards that still read in third person
g = 4
big = [((i + rng.uniform(0.1, 0.9)) / g, (j + rng.uniform(0.1, 0.9)) / g)
       for i in range(g) for j in range(g) if (i, j) not in ((1, 2), (3, 0))]
# two impact points shatter into smaller shards
small = []
for cx, cy in ((1.5 / g, 2.5 / g), (3.5 / g, 0.5 / g)):
    for k in range(4):
        a = TAU * k / 4 + rng.uniform(-0.3, 0.3); r = rng.uniform(0.05, 0.10)
        small.append(((cx + r * np.cos(a)) % 1, (cy + r * np.sin(a)) % 1))
seeds = np.array(big + small)
idx, edge, vx1, vy1, idx_b = voronoi(seeds)
# fine side cracks: denser cells, only some of their edges are gilded
s2 = rng.uniform(0, 1, (40, 2))
idx2, edge2, _, _, _ = voronoi(s2)
x, y = x0, y0

# lacquer colours per shard: deep red and black, a few oxblood
red = np.array([0.38, 0.03, 0.05]); ox = np.array([0.26, 0.02, 0.04]); blk = np.array([0.058, 0.044, 0.052])
nk = len(seeds); kinds = rng.permutation(([0, 2, 1, 2, 0, 2] * nk)[:nk])
pal = np.array([red, ox, blk])[kinds]
col = pal[idx]
# gloss: each shard tilted differently, a soft sheen band across it
ang = rng.uniform(0, TAU, len(seeds))
tilt = (vx1 * np.cos(ang)[idx] + vy1 * np.sin(ang)[idx]) * g  # about -1..1 across a shard
sheen = np.clip(1 - np.abs(tilt - 0.25) * 1.6, 0, 1) ** 2
col = col * (0.88 + 0.18 * tilt[..., None].clip(-1, 1)) + sheen[..., None] * np.array([0.12, 0.04, 0.05])
col += tnoise(9, 10, 12)[..., None] * 0.015

# seams: thick main seams with width that swells and thins, thinner branch cracks
px = 1 / N
pairw = np.random.default_rng(7).uniform(3.8, 9.5, (len(seeds), len(seeds)))
pairw = np.maximum(pairw, pairw.T)
wmain = (pairw[idx, idx_b] + 2.0 * tnoise(3, 6, 4)) * px
wbr = (3.6 + 1.2 * tnoise(4, 6, 5)) * px
keep = (tnoise(5, 6, 3) > 0.15) & (idx2 % 3 != 0)
cov_main = np.clip((wmain - edge) / (1.5 * px) + 0.5, 0, 1)
cov_br = np.clip((wbr - edge2) / (1.5 * px) + 0.5, 0, 1) * keep
cov = np.maximum(cov_main, cov_br)
# gold: brighter at seam centre (rounded bead), slightly darker at the edges
t_main = np.clip(edge / wmain, 0, 1); t_br = np.clip(edge2 / wbr, 0, 1)
t = np.where(cov_main >= cov_br, t_main, t_br)
bead = 1 - t ** 2
gold_dk = np.array([0.40, 0.24, 0.05]); gold_lt = np.array([0.74, 0.52, 0.15])
gold = gold_dk + (gold_lt - gold_dk) * bead[..., None]
gold *= (0.92 + 0.10 * tnoise(6, 8, 14))[..., None]
# a thin dark lip where lacquer meets gold, so the seam reads against red
lip = np.clip(1 - np.abs(edge - wmain - 1.5 * px) / (2 * px), 0, 1) * (1 - cov_main)
col = col * (1 - 0.45 * lip[..., None])
out = col * (1 - cov[..., None]) + gold * cov[..., None]
img = (np.clip(out, 0, 1) * 255 + 0.5).astype(np.uint8)
Image.fromarray(img).save(os.path.join(D, "paint.png"))
Image.fromarray(np.tile(img, (2, 2, 1))).save(os.path.join(D, "tile_check.png"))
print("ok", img.mean(axis=(0, 1)))

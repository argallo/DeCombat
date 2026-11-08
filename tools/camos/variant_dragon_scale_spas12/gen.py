# Dragon Scale camo for the SPAS-12: procedural, tileable, original art.
import sys, os
import numpy as np
from PIL import Image

OUT = os.path.dirname(os.path.abspath(__file__))
N = 512
COLS = int(sys.argv[1]) if len(sys.argv) > 1 else 4   # scales per tile across
W = N // COLS                # scale width
H = W // 2                   # row pitch
ROWS = N // H                # even, so the offset alternates cleanly
R = W * 0.56                 # scale radius
rng = np.random.default_rng(7)
tint = rng.uniform(-1, 1, (ROWS, COLS * 2 + 2))  # per-scale variation, indexed modulo

y, x = np.mgrid[0:N, 0:N].astype(np.float64)
x += 0.5; y += 0.5

owner_k = np.full((N, N), -99)
own_dx = np.zeros((N, N)); own_dy = np.zeros((N, N)); own_t = np.ones((N, N))
own_id = np.zeros((N, N), dtype=int)
shadow = np.zeros((N, N))
base_row = np.floor(y / H).astype(int)
KMAX = int(np.ceil(R / H)) + 1

# scales from lower rows first; upper rows (larger k) lie on top
cand = []
for k in range(-2, KMAX + 1):
    j = base_row - k
    cy = j * H
    off = np.where(j % 2 == 0, 0.0, W / 2)
    i = np.round((x - off) / W)
    cx = i * W + off
    dx = x - cx; dy = y - cy
    d = np.hypot(dx, dy)
    cov = (d < R)
    cand.append((k, j, i, dx, dy, d, cov))

for k, j, i, dx, dy, d, cov in cand:
    m = cov
    owner_k[m] = k
    own_dx[m] = dx[m]; own_dy[m] = dy[m]; own_t[m] = d[m] / R
    own_id[m] = (j[m] % ROWS) * 1000 + (i[m].astype(int) % (COLS * 2))

# crease: shade where an overlapping scale (one row above the owner) ends just above the pixel
for k, j, i, dx, dy, d, cov in cand:
    above = k > owner_k
    gap = d - R
    s = np.clip(1 - gap / (0.16 * W), 0, 1) ** 1.6
    s = np.where(above & (gap >= 0), s, 0)
    shadow = np.maximum(shadow, s)

uncov = owner_k < -50
t = np.clip(own_t, 0, 1)
rowi = own_id // 1000; coli = own_id % 1000
var = tint[rowi % ROWS, coli % tint.shape[1]]

emer_dark = np.array([0.02, 0.24, 0.14])
emer_mid = np.array([0.05, 0.44, 0.26])
emer_lite = np.array([0.13, 0.58, 0.36])
gold = np.array([0.78, 0.56, 0.12])
gold_hi = np.array([0.95, 0.80, 0.36])
edge_dark = np.array([0.10, 0.12, 0.05])

# body: brighter in the middle and along a central ridge, darker toward the rim
ridge = np.exp(-(own_dx / (0.10 * W)) ** 2) * np.clip(own_dy / R, 0, 1) * (1 - t)
body_f = np.clip(0.75 - 0.6 * t + 0.25 * var * 0.4, 0, 1)
col = emer_dark + (emer_mid - emer_dark) * np.clip(body_f * 1.6, 0, 1)[..., None]
col = col + (emer_lite - emer_mid) * np.clip(ridge * 1.4 + 0.15 * var, 0, 1)[..., None]
# subtle hue shift per scale (teal <-> jade)
col[..., 2] += 0.04 * var
col[..., 0] += 0.02 * np.clip(-var, 0, 1)

# fine striations radiating from the scale root
ang = np.arctan2(own_dx, own_dy + 1e-6)
stri = 0.5 + 0.5 * np.cos(ang * 22)
col *= (0.93 + 0.07 * stri)[..., None]

# gold rim with a highlight in it and a dark hairline at the very edge
rim = np.clip((t - 0.80) / 0.06, 0, 1) * np.clip((0.985 - t) / 0.03, 0, 1)
rim_hi = np.exp(-((t - 0.88) / 0.025) ** 2)
gcol = gold + (gold_hi - gold) * rim_hi[..., None]
col = col * (1 - rim[..., None]) + gcol * rim[..., None]
hair = np.clip((t - 0.965) / 0.035, 0, 1)
col = col * (1 - hair[..., None]) + edge_dark * hair[..., None]

# crease shadow under each overlapping scale
col *= (1 - 0.72 * shadow)[..., None]
col[uncov] = emer_dark * 0.6

col = np.clip(col, 0, 1)
Image.fromarray((col * 255 + 0.5).astype(np.uint8)).save(os.path.join(OUT, "paint.png"))

# effect: a soft golden diagonal band (one per tile), strongest on the gold rims
u = (x + y) / N
band = ((np.cos(2 * np.pi * u) + 1) / 2) ** 5
spark = 0.45 + 0.55 * np.clip(rim + 0.3 * ridge, 0, 1)
fx = band * spark
fxc = np.stack([fx * 1.0, fx * 0.78, fx * 0.30], -1)
Image.fromarray((np.clip(fxc, 0, 1) * 255 + 0.5).astype(np.uint8)).save(os.path.join(OUT, "fx.png"))

# tile check preview: 2x2
p = Image.open(os.path.join(OUT, "paint.png"))
t2 = Image.new("RGB", (N * 2, N * 2))
for a in (0, N):
    for b in (0, N):
        t2.paste(p, (a, b))
t2.save(os.path.join(OUT, "tile2x2.png"))
print("ok", COLS, W, H, ROWS, R, "uncovered", int(uncov.sum()))

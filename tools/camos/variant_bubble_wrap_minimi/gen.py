# Bubble Wrap camo for the MINIMI. Procedural, tileable (torus distances), original art.
import os, sys
import numpy as np
from PIL import Image
d = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(553)
y, x = np.mgrid[0:N, 0:N].astype(np.float32) + 0.5
u, v = x / N, y / N

# palette (linear-ish 0..1), saturated cyan-blue, kept mid/dark so the bright receiver doesn't go white
film   = np.array([0.05, 0.23, 0.44])   # flat plastic between bubbles
dome   = np.array([0.09, 0.37, 0.60])   # bubble body
domehi = np.array([0.20, 0.54, 0.72])   # lit top-left of dome
shade  = np.array([0.02, 0.11, 0.28])   # shadow side / drop shadow
spec   = np.array([0.52, 0.80, 0.93])   # glossy highlight (small)
popc   = np.array([0.04, 0.19, 0.40])   # popped, flattened bubble

COLS, ROWS = 6, 6           # offset grid, even rows so it wraps
px, py = N / COLS, N / ROWS
R = 0.40 * px               # bubble radius in px

img = np.zeros((N, N, 3), np.float32) + film
# film texture: faint large-scale sheen (tileable)
sheen = 0.5 + 0.5 * np.sin(2 * np.pi * (u * 1 + v * 2)) * np.sin(2 * np.pi * (u * 2 - v * 1))
img = img * (0.9 + 0.15 * sheen[..., None])

def tor(dx):
    return (dx + N / 2) % N - N / 2

centers = []
for r in range(ROWS):
    for c in range(COLS):
        cx = (c + 0.5 + 0.5 * (r % 2)) * px
        cy = (r + 0.5) * py
        centers.append((cx, cy))
popped = {0 * COLS + 2, 1 * COLS + 5, 3 * COLS + 1, 4 * COLS + 4, 5 * COLS + 2}

L = np.array([-0.55, -0.65, 0.52]); L /= np.linalg.norm(L)  # light from top-left
for i, (cx, cy) in enumerate(centers):
    dx, dy = tor(x - cx), tor(y - cy)
    rr = np.sqrt(dx * dx + dy * dy)
    if i in popped:
        # flattened, crinkled disc: irregular outline, crease lines, a thin dark rim
        ang = np.arctan2(dy, dx)
        k = rng.uniform(0, 6.28, 3)
        rim = R * (0.95 + 0.07 * np.sin(5 * ang + k[0]) + 0.05 * np.sin(9 * ang + k[1]))
        m = rr < rim
        crinkle = np.abs(np.sin(7 * ang + 2.5 * np.sin(3 * ang + k[2]) + rr / R * 2.0))
        col = popc[None, None, :] * (0.85 + 0.25 * crinkle[..., None])
        # crease lines: sharp light/dark streaks
        streak = np.clip(1 - np.abs(np.sin(4 * ang + k[1] + rr / R)) * 8, 0, 1)
        col = col + streak[..., None] * (domehi - popc) * 0.5
        img[m] = col[m]
        ring = (rr >= rim - 3) & (rr < rim + 1.5)
        img[ring] = shade
        continue
    # drop shadow (offset down-right), soft
    sdx, sdy = tor(x - cx - 0.16 * R), tor(y - cy - 0.2 * R)
    sr = np.sqrt(sdx * sdx + sdy * sdy)
    sh = np.clip((R * 1.18 - sr) / (0.35 * R), 0, 1) * (rr > R * 0.97)
    img = img * (1 - 0.55 * sh[..., None]) + shade * 0.55 * sh[..., None] * 0.0
    m = rr < R
    nz = np.sqrt(np.clip(1 - (rr / R) ** 2, 0, 1))
    nx, ny = dx / R, dy / R
    lam = np.clip(nx * L[0] + ny * L[1] + nz * L[2], 0, 1)
    col = shade + (dome - shade) * np.clip(lam * 1.6, 0, 1)[..., None]
    col = col + (domehi - dome) * np.clip((lam - 0.55) / 0.45, 0, 1)[..., None]
    # rim: transparent edge darkens, then a thin bright fresnel line
    edge = np.clip((rr / R - 0.82) / 0.18, 0, 1)
    col = col * (1 - 0.35 * edge[..., None])
    # glossy highlight: a bold oval top-left plus a small secondary glint bottom-right
    hx, hy = dx + 0.38 * R, dy + 0.40 * R
    hl = np.clip(1 - np.sqrt((hx / (0.26 * R)) ** 2 + (hy / (0.17 * R)) ** 2), 0, 1) ** 0.6
    gx, gy = dx - 0.45 * R, dy - 0.42 * R
    gl = np.clip(1 - np.sqrt(gx * gx + gy * gy) / (0.09 * R), 0, 1) ** 0.8
    h = np.clip(hl + 0.7 * gl, 0, 1)[..., None]
    col = col * (1 - h) + spec * h
    img[m] = col[m]

# a wrinkle of plastic: a crease running diagonally (wraps on the torus: u + 2v)
t = (u * 1 + v * 2 + 0.04 * np.sin(2 * np.pi * (u * 2 - v))) % 1.0
dd = (t - 0.62)
dd = (dd + 0.5) % 1.0 - 0.5
w = 0.024
crease_lit = np.exp(-((dd + w) / w) ** 2)
crease_dark = np.exp(-((dd - w * 0.6) / (w * 0.7)) ** 2)
img = img * (1 - 0.5 * crease_dark[..., None]) + (domehi - img) * 0.45 * crease_lit[..., None]
# a second shallower fold
t2 = (u * 3 - v * 1) % 1.0
dd2 = ((t2 - 0.2) + 0.5) % 1.0 - 0.5
img = img * (1 - 0.3 * np.exp(-((dd2 - 0.008) / 0.009) ** 2)[..., None])
img = img + (domehi - img) * 0.25 * np.exp(-((dd2 + 0.01) / 0.01) ** 2)[..., None]

img = np.clip(img, 0, 1)
Image.fromarray((img * 255 + 0.5).astype(np.uint8)).save(os.path.join(d, "paint.png"))
t = Image.open(os.path.join(d, "paint.png"))
tc = Image.new("RGB", (N * 2, N * 2))
for a in range(2):
    for b in range(2):
        tc.paste(t, (a * N, b * N))
tc.save(os.path.join(d, "tile_check.png"))
print("ok")

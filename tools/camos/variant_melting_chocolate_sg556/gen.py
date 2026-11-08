# Melting Chocolate: milk-chocolate bar squares, bevelled, with melting drips and hazelnut chunks. Procedural, tileable.
import os, numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512; C = 4; cell = N / C
rng = np.random.default_rng(556)
y, x = np.mgrid[0:N, 0:N].astype(np.float32)
lx = x % cell; ly = y % cell
G = 8.0; B = 18.0   # groove half-width, bevel width

def col(*c): return np.array(c, np.float32)
face = col(0.19, 0.102, 0.056)
bev_hi = col(0.29, 0.165, 0.092)
bev_lo = col(0.09, 0.048, 0.028)
groove = col(0.08, 0.045, 0.028)

img = np.zeros((N, N, 3), np.float32)
dl, dt, dr, db = lx, ly, cell - lx, cell - ly
d = np.minimum(np.minimum(dl, dt), np.minimum(dr, db))
# smooth tiling noise
def tnoise(freqs, seed):
    r = np.random.default_rng(seed); out = np.zeros((N, N), np.float32)
    for f in freqs:
        for _ in range(3):
            a, b = r.integers(-f, f + 1, 2); ph = r.uniform(0, 6.283)
            out += np.cos(2 * np.pi * (a * x + b * y) / N + ph) / f
    return out / np.abs(out).max()
n1 = tnoise([3, 5, 8, 13], 1)
img[:] = face * (1 + 0.05 * n1[..., None])
# inner plateau rim: a faint second step inside the face
rim = (d > G + B + 10) & (d < G + B + 15)
near = np.argmin(np.stack([dl, dt, dr, db]), 0)
lit = (near == 0) | (near == 1)
img[rim & lit] = face * 0.86
img[rim & ~lit] = face * 1.12
# bevels (top/left catch light, bottom/right in shade), linear ramp toward the face
bv = (d >= G) & (d < G + B)
t = ((d - G) / B)[..., None]
img = np.where((bv & lit)[..., None], bev_hi * (1 - t) + face * t, img)
img = np.where((bv & ~lit)[..., None], bev_lo * (1 - t) + face * t, img)
# diagonal seams of the bevel corners (mitre)
img = np.where((d < G)[..., None], groove * (1 + 0.1 * n1[..., None]), img)

# --- hazelnut chunks on some faces ---
def torus(dx): return (dx + N / 2) % N - N / 2
cells = [(i, j) for i in range(C) for j in range(C)]
nut_cells = [cells[k] for k in rng.choice(len(cells), 6, replace=False)]
for (i, j) in nut_cells:
    cx = i * cell + cell * rng.uniform(0.35, 0.65); cy = j * cell + cell * rng.uniform(0.35, 0.65)
    R = rng.uniform(17, 21)
    dx, dy = torus(x - cx), torus(y - cy)
    ang = np.arctan2(dy, dx); rr = np.hypot(dx, dy)
    k = rng.uniform(0, 6.28, 3)
    edge = R * (1 + 0.18 * np.cos(3 * ang + k[0]) + 0.1 * np.cos(5 * ang + k[1]) + 0.07 * np.cos(2 * ang + k[2]))
    m = rr < edge
    ring = (rr >= edge) & (rr < edge + 3.5)
    img[ring] = img[ring] * 0.55   # crevice around the nut
    shade = np.clip(1.05 - 0.45 * ((dx + dy) / (1.4 * R)), 0.55, 1.4)
    nut = col(0.29, 0.175, 0.095) * shade[..., None] * (1 + 0.08 * n1[..., None])
    img = np.where(m[..., None], nut, img)
    hi = m & (np.hypot(dx + R * 0.35, dy + R * 0.35) < R * 0.28)
    img[hi] = col(0.34, 0.22, 0.13)

# --- melting drips from some bottom corners ---
gloss = col(0.17, 0.085, 0.045)
glint = col(0.36, 0.25, 0.19)
drips = []
for (i, j) in cells:
    if rng.random() < 0.45:
        side = rng.integers(0, 2)   # bottom-left or bottom-right corner
        cx = i * cell + (G + 12 if side == 0 else cell - G - 12) + rng.uniform(-4, 4)
        cy = (j + 1) * cell - G - 10
        drips.append((cx, cy, rng.uniform(45, 100), rng.uniform(11, 15)))
for (cx, cy, L, w) in drips:
    dx0 = torus(x - cx); dy0 = torus(y - cy)
    # sagging corner puddle
    sdf = np.hypot(dx0 / 1.25, dy0) - (w + 9)
    # the drip column, slightly wavy, narrowing, ending in a fat bulb
    s = np.clip(dy0 / L, 0, 1)
    wob = 3.5 * np.sin(dy0 / 12.0 + cx)
    wid = w * (1 - 0.35 * s)
    col_sdf = np.where((dy0 >= 0) & (dy0 <= L), np.abs(dx0 - wob) - wid, 1e3)
    bulb = np.hypot(dx0 - 3.5 * np.sin(L / 12.0 + cx), dy0 - L) - (w * 1.0)
    sdf = np.minimum(np.minimum(sdf, col_sdf), bulb)
    m = sdf < 0
    edge = (sdf >= 0) & (sdf < 3.5)
    img[edge] = img[edge] * 0.6   # dark outline / shadow under the drip
    # glossy body: darker toward its right edge, highlight streak on its left
    shade = np.clip(1.0 - 0.025 * (dx0 - wob), 0.7, 1.25)
    body = gloss * shade[..., None]
    img = np.where(m[..., None], body, img)
    streak = m & (np.abs(dx0 - wob + wid * 0.45) < 3.5) & (dy0 > -w) & (dy0 < L - 4)
    img[streak] = glint
    spot = np.hypot(dx0 - 3.5 * np.sin(L / 12.0 + cx) + w * 0.35, dy0 - L + w * 0.3) < w * 0.28
    img[spot] = glint

img = np.clip(img, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))
t2 = np.tile(img, (2, 2, 1))
Image.fromarray((t2 * 255).astype(np.uint8)).resize((512, 512)).save(os.path.join(D, "tile_check.png"))

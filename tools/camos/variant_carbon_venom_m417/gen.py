# Carbon Venom: procedural, tileable. Woven 2x2 carbon twill cracked by acid-green venom veins.
import os, sys
import numpy as np
from PIL import Image
A = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(417)
y, x = np.mgrid[0:N, 0:N].astype(np.float64) / N   # 0..1 on the torus

# ---- carbon twill: 16 tows per tile, 2x2 twill (period 4 divides 16) ----
T = 20
gx, gy = x * T, y * T
ci, cj = np.floor(gx).astype(int), np.floor(gy).astype(int)
fx_, fy_ = gx - ci, gy - cj
warp_up = ((ci + cj) % 4) < 2            # warp (vertical tow) on top
# along-tow bump (each float spans 2 cells: position inside the float)
ph = (ci + cj) % 4
float_pos_w = ((ph % 2) + fy_) / 2.0                 # warp float (2 cells) runs along y
float_pos_f = ((ph % 2) + fx_) / 2.0                 # weft float (2 cells) runs along x
float_pos_w = np.clip(float_pos_w, 0, 1); float_pos_f = np.clip(float_pos_f, 0, 1)
across_w = np.sin(np.pi * fx_)           # tow cross-section (rounded)
across_f = np.sin(np.pi * fy_)
along_w = np.sin(np.pi * float_pos_w) ** 0.6
along_f = np.sin(np.pi * float_pos_f) ** 0.6
shade_w = 0.55 + 0.45 * across_w ** 0.7 * along_w
shade_f = 0.55 + 0.45 * across_f ** 0.7 * along_f
# anisotropic sheen: warp tows catch more light than weft tows
sheen = np.where(warp_up, 1.00 * shade_w, 0.55 * shade_f)
# fine fibre striations along the tow
fib_w = 0.94 + 0.06 * np.sin(2 * np.pi * x * T * 7)
fib_f = 0.94 + 0.06 * np.sin(2 * np.pi * y * T * 7)
sheen = sheen * np.where(warp_up, fib_w, fib_f)
base_dark = np.array([0.048, 0.050, 0.054])
base_lit = np.array([0.215, 0.22, 0.232])
carbon = base_dark + (base_lit - base_dark) * sheen[..., None]

# ---- periodic value noise for domain warping ----
def pnoise(freq, seed):
    r = np.random.default_rng(seed).random((freq, freq))
    u, v = x * freq, y * freq
    i0, j0 = np.floor(u).astype(int) % freq, np.floor(v).astype(int) % freq
    i1, j1 = (i0 + 1) % freq, (j0 + 1) % freq
    a, b = u - np.floor(u), v - np.floor(v)
    a = a * a * (3 - 2 * a); b = b * b * (3 - 2 * b)
    return (r[j0, i0] * (1 - a) * (1 - b) + r[j0, i1] * a * (1 - b)
            + r[j1, i0] * (1 - a) * b + r[j1, i1] * a * b)
def fbm(seed, f0=4, oct=4):
    s, amp, tot = 0, 1.0, 0
    for k in range(oct):
        s = s + amp * pnoise(f0 * 2 ** k, seed + k); tot += amp; amp *= 0.5
    return s / tot - 0.5

def voronoi_edges(npts, seed, warp):
    p = np.random.default_rng(seed).random((npts, 2))
    wx = x + warp * fbm(seed + 100); wy = y + warp * fbm(seed + 200)
    d1 = np.full((N, N), 9.0); d2 = np.full((N, N), 9.0)
    for px, py in p:
        dx = np.abs(wx - px); dx = np.minimum(dx % 1.0, 1 - dx % 1.0)
        dy = np.abs(wy - py); dy = np.minimum(dy % 1.0, 1 - dy % 1.0)
        d = np.sqrt(dx * dx + dy * dy)
        d2 = np.where(d < d1, d1, np.minimum(d2, d)); d1 = np.minimum(d1, d)
    e = d2 - d1
    gxx = (np.roll(e, -1, 1) - np.roll(e, 1, 1)) * N / 2
    gyy = (np.roll(e, -1, 0) - np.roll(e, 1, 0)) * N / 2
    g = np.sqrt(gxx ** 2 + gyy ** 2)
    return e / np.maximum(g, 0.6)   # ~ true distance to the cell border (tile units)

# big bold veins + finer branching cracks
e1 = voronoi_edges(9, 11, 0.16)
e2 = voronoi_edges(26, 23, 0.10)
w1 = 0.011 * (0.7 + 0.6 * (fbm(300, 3, 3) + 0.5))   # vein width varies along the crack
core1 = np.clip(1 - e1 / w1, 0, 1)
halo1 = np.clip(1 - e1 / (w1 * 3.0), 0, 1)
# fine cracks only in part of the tile, fading out
mask2 = np.clip((fbm(400, 2, 3) - 0.02) * 6, 0, 1)
core2 = np.clip(1 - e2 / 0.0045, 0, 1) * mask2
halo2 = np.clip(1 - e2 / 0.012, 0, 1) * mask2
core = np.maximum(core1, 0.85 * core2) ** 0.8
halo = np.maximum(halo1, 0.8 * halo2)

venom_hot = np.array([0.34, 0.62, 0.04])
venom_deep = np.array([0.10, 0.30, 0.02])
# burnt seam around the vein (carbon darkened, slightly green-stained)
img = carbon * (1 - 0.35 * halo[..., None]) + np.array([0.01, 0.07, 0.0]) * halo[..., None]
vcol = venom_deep + (venom_hot - venom_deep) * core[..., None] ** 1.5
img = img * (1 - core[..., None]) + vcol * core[..., None]
img = np.clip(img, 0, 1)
Image.fromarray((img * 255 + 0.5).astype(np.uint8)).save(os.path.join(A, 'paint.png'))

# effect: veins only, black elsewhere
fxv = np.clip(core ** 1.3, 0, 1)
fxc = fxv[..., None] * np.array([0.55, 1.0, 0.12]) + (halo ** 3)[..., None] * np.array([0.03, 0.12, 0.0])
Image.fromarray((np.clip(fxc, 0, 1) * 255 + 0.5).astype(np.uint8)).save(os.path.join(A, 'fx.png'))

# tile check 2x2
t = Image.open(os.path.join(A, 'paint.png'))
c = Image.new('RGB', (N * 2, N * 2))
for i in range(2):
    for j in range(2): c.paste(t, (i * N, j * N))
c.save(os.path.join(A, 'tile_check.png'))

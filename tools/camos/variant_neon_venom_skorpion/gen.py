# Neon Venom: segmented carapace plates with violet seams. Procedural, tileable.
import os, numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
S = 512; N = 4; M = 2; A = 0.45      # rows, plates per row, arc depth (row units)
v, u = np.mgrid[0:S, 0:S] / S
def bulge(k, u):
    x = np.mod(u * M + 0.5 * (np.mod(k, 2)), 1.0)
    return 1 - (2 * x - 1) ** 2, x
k0 = np.floor(v * N)
# row k owns the pixel if it is above row k's rounded bottom edge and below row k-1's
K = np.full_like(u, np.nan)
for dk in (-1, 0, 1):
    k = k0 + dk
    b, _ = bulge(k, u)
    m = np.isnan(K) & (v * N < k + 1 + A * b)
    K[m] = k[m]
B, X = bulge(K, u)
bp, xp = bulge(K - 1, u)
top = K + A * bp                     # previous row's bottom (row units, absolute)
bot = K + 1 + A * B
T = v * N - top
H = bot - top
rowpx = S / N; colpx = S / M
d_top = T * rowpx
d_bot = (bot - v * N) * rowpx * np.sqrt(1 / (1 + (A * 4 * (2 * X - 1) * M / N) ** 2))
d_side = np.minimum(X, 1 - X) * colpx
d = np.minimum(np.minimum(d_top, d_bot), d_side)
bottom = H
# plate shading: each plate slides under the one above (shadow at its top) and its rounded
# lower half bulges toward the light, with a glossy band that follows the rounded edge
g = np.clip((bot - v * N) / H, 0, 1)        # 0 at own rounded edge, 1 at the tucked top
cx = 2 * X - 1
dome = (1 - cx ** 2) ** 0.5
occl = np.clip(d_top / (rowpx * 0.35), 0, 1) ** 0.8          # tucked under the plate above
light = (0.35 + 0.65 * dome) * (0.25 + 0.75 * occl) * (0.6 + 0.4 * (1 - g))
light *= 0.7 + 0.3 * np.clip(d / 10, 0, 1)                    # bevel at the edges
spec = np.exp(-((g - 0.42) / 0.09) ** 2) * np.exp(-(cx / 0.6) ** 2)
dark = np.array([0.035, 0.015, 0.06]); mid = np.array([0.125, 0.05, 0.205])
hi = np.array([0.40, 0.27, 0.56]); seam = np.array([0.44, 0.12, 0.70])
col = dark + (mid - dark) * np.clip(light, 0, 1)[..., None]
col = col + (hi - col) * np.clip(0.8 * spec, 0, 1)[..., None]
sw = np.clip(1 - (d - 3) / 3, 0, 1)                             # seam line ~6 px
col = col * (1 - sw[..., None]) + seam * sw[..., None]
Image.fromarray((np.clip(col, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))
# effect: black except glowing seams
g = np.clip(1 - (d - 2) / 3, 0, 1) + 0.45 * np.exp(-np.maximum(d - 3, 0) / 6)
g = np.clip(g, 0, 1)
fx = np.array([0.70, 0.28, 1.0]) * g[..., None]
Image.fromarray((np.clip(fx, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "fx.png"))
# tile check
p = Image.open(os.path.join(D, "paint.png")); t = Image.new("RGB", (S * 2, S * 2))
for i in range(2):
    for j in range(2): t.paste(p, (i * S, j * S))
t.save(os.path.join(D, "tile_check.png"))

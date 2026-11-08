# K7 "Cotton Candy": procedural, tileable (FFT noise is periodic by construction).
import os, sys
import numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(543)
fx = np.fft.fftfreq(N) * N
KX, KY = np.meshgrid(fx, fx)
K = np.sqrt(KX**2 + KY**2)

def noise(lo, hi, aniso=None, seed=None):
    r = np.random.default_rng(seed)
    spec = (r.normal(size=(N, N)) + 1j * r.normal(size=(N, N)))
    k = K.copy()
    if aniso is not None:
        ax, ay = aniso
        k = np.sqrt((KX * ax)**2 + (KY * ay)**2)
    band = np.exp(-((k - (lo + hi) / 2) / ((hi - lo) / 2 + 1e-6))**2) * (k > 0.5)
    f = np.real(np.fft.ifft2(spec * band))
    return (f - f.mean()) / (f.std() + 1e-9)

def warp(f, du, dv):
    # sample periodic field f at (y+dv, x+du), bilinear, wrapped
    y, x = np.mgrid[0:N, 0:N].astype(float)
    xs = (x + du) % N; ys = (y + dv) % N
    x0 = np.floor(xs).astype(int); y0 = np.floor(ys).astype(int)
    tx = xs - x0; ty = ys - y0
    x1 = (x0 + 1) % N; y1 = (y0 + 1) % N
    a = f[y0, x0] * (1 - tx) + f[y0, x1] * tx
    b = f[y1, x0] * (1 - tx) + f[y1, x1] * tx
    return a * (1 - ty) + b * ty

# domain warp fields
wu = noise(1.5, 3.5, seed=1) * 38
wv = noise(1.5, 3.5, seed=2) * 38
# big pink/blue swirl field (few fat shapes, reads in third person)
big = warp(noise(1.5, 3.0, seed=3) + 0.5 * noise(3, 6, seed=4), wu, wv)
# puff field: clumps of fluff
puff = warp(noise(4, 9, seed=5) + 0.5 * noise(9, 16, seed=6), wu * 0.7, wv * 0.7)
# wispy strands: anisotropic high-frequency noise, warped hard so strands curl
str1 = warp(noise(10, 22, aniso=(0.25, 1.0), seed=7), wu * 1.6, wv * 1.6)
str2 = warp(noise(10, 22, aniso=(1.0, 0.25), seed=8), wv * 1.6, wu * 1.6)

def sstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0, 1); return t * t * (3 - 2 * t)



pink_d = np.array([0.44, 0.11, 0.31]); pink = np.array([0.72, 0.24, 0.52]); pink_l = np.array([0.80, 0.40, 0.62])
blue_d = np.array([0.11, 0.25, 0.52]); blue = np.array([0.25, 0.52, 0.80]); blue_l = np.array([0.42, 0.63, 0.83])

# cumulus puffs: union of wrapped spheres, painter's order, colour from the swirled big field
yy, xx = np.mgrid[0:N, 0:N].astype(float)
col = np.zeros((N, N, 3)); col[:] = (pink_d + blue_d) / 2
r2 = np.random.default_rng(77)
# light gentle warp so puffs wobble, not liquid
gu = noise(3, 6, seed=31) * 6; gv = noise(3, 6, seed=32) * 6
XX = xx + gu; YY = yy + gv
puffs = []
for i in range(165):
    cx, cy = r2.uniform(0, N, 2)
    r = r2.uniform(26, 62) if i < 120 else r2.uniform(20, 36)
    puffs.append((cx, cy, r))
for cx, cy, r in puffs:
    dx = (XX - cx + N / 2) % N - N / 2
    dy = (YY - cy + N / 2) % N - N / 2
    d2 = (dx * dx + dy * dy) / (r * r)
    m = d2 < 1
    if not m.any(): continue
    z = np.sqrt(np.clip(1 - d2, 0, 1))
    nx, ny = dx / r, dy / r
    lam = np.clip(0.55 * z - 0.45 * nx - 0.55 * ny, 0, 1.2)   # light from upper left
    b = big[int(cy) % N, int(cx) % N]
    t = sstep(-0.25, 0.25, b)                                  # 0 pink .. 1 blue
    d_ = pink_d * (1 - t) + blue_d * t; m_ = pink * (1 - t) + blue * t; l_ = pink_l * (1 - t) + blue_l * t
    lam = lam[..., None]
    c = np.where(lam < 0.6, d_ + (m_ - d_) * (lam / 0.6), m_ + (l_ - m_) * np.clip((lam - 0.6) / 0.6, 0, 1))
    # soft fuzzy rim
    a = sstep(1.0, 0.55, d2)[..., None]
    col = col * (1 - a) + c * a

# soften everything (periodic gaussian blur via FFT): fluff, not plastic
G = np.exp(-(K ** 2) * (2 * np.pi * 2.2 / N) ** 2 / 2)[..., None]
col = np.real(np.fft.ifft2(np.fft.fft2(col, axes=(0, 1)) * G, axes=(0, 1)))
# spun-sugar fibre texture
fib = warp(noise(40, 80, aniso=(0.2, 1.0), seed=41), wu, wv)
col = col * (1 + 0.10 * np.tanh(fib[..., None]))
# wispy strands: thin curly filaments, contrasting colour
s = np.maximum(np.exp(-(str1 / 0.16) ** 2) * sstep(-0.3, 0.8, puff), np.exp(-(str2 / 0.16) ** 2) * sstep(-0.3, 0.8, -puff))
lumb = col.mean(-1, keepdims=True)
isblue = sstep(-0.02, 0.06, (col[..., 2] - col[..., 0]))[..., None]
strand_c = pink_l * isblue + blue_l * (1 - isblue)
s = s[..., None] * 0.7
col = col * (1 - s) + strand_c * s

# grade: deeper and more saturated so bright gun parts (x1.5) stay pink/blue, not pastel white
lum = col.mean(-1, keepdims=True)
col = (lum + (col - lum) * 1.35) * 0.74

img = (np.clip(col, 0, 1) * 255).astype(np.uint8)
Image.fromarray(img).save(os.path.join(D, "paint.png"))
Image.fromarray(np.tile(img, (2, 2, 1))).resize((512, 512)).save(os.path.join(D, "tile_check.png"))
print("mean", col.reshape(-1, 3).mean(0), "max", col.reshape(-1, 3).max(0))

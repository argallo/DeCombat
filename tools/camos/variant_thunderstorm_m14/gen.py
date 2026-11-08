# M14 "Thunderstorm": procedural, tileable storm clouds (paint) + sparse branching lightning (effect).
import os, math
import numpy as np
from PIL import Image, ImageDraw, ImageFilter
D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(1432)

def pnoise(n, seed):
    """Periodic value noise: n x n random grid, smooth interpolation, wraps on the torus."""
    r = np.random.default_rng(seed).random((n, n))
    t = np.arange(N) * n / N
    i0 = np.floor(t).astype(int); f = t - i0; f = f * f * (3 - 2 * f)
    i1 = (i0 + 1) % n
    a = r[i0][:, i0]; b = r[i0][:, i1]; c = r[i1][:, i0]; d = r[i1][:, i1]
    fy = f[:, None]; fx = f[None, :]
    return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy

def fbm(seed, base=3, oct=5):
    s = np.zeros((N, N)); amp = 1.0; tot = 0
    for k in range(oct):
        s += amp * pnoise(base * 2 ** k, seed + k); tot += amp; amp *= 0.5
    return s / tot

def warp(img, du, dv):
    yy, xx = np.mgrid[0:N, 0:N]
    y = (yy + dv).astype(int) % N; x = (xx + du).astype(int) % N
    return img[y, x]

# churning clouds: domain-warped fBm
w1 = fbm(10, 2, 4); w2 = fbm(20, 2, 4)
c = fbm(30, 4, 6)
c = warp(c, (w1 - 0.5) * 140, (w2 - 0.5) * 140)
c2 = warp(fbm(40, 4, 5), (w2 - 0.5) * 90, (w1 - 0.5) * 90)
c = (c - c.min()) / (c.max() - c.min())
c = np.clip((c - 0.5) * 2.3 + 0.5, 0, 1) ** 1.6          # punchy billows, biased dark
hue = (c2 - c2.min()) / (c2.max() - c2.min())
hue = np.clip((hue - 0.62) * 2.2 + 0.5, 0, 1)     # slate <-> purple mix

deep   = np.array([0.06, 0.07, 0.17])   # storm core
slate  = np.array([0.12, 0.18, 0.34])   # slate blue
purple = np.array([0.24, 0.11, 0.35])   # bruised purple
lilac  = np.array([0.20, 0.21, 0.38])   # lit cloud tops
mid = slate[None, None] * (1 - hue[..., None]) + purple[None, None] * hue[..., None]
t = c[..., None]
col = np.where(t < 0.5, deep + (mid - deep) * (t / 0.5), mid + (lilac - mid) * ((t - 0.5) / 0.5))
Image.fromarray((np.clip(col, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))

# lightning: few branching bolts, drawn on a 3x3 canvas and folded mod 512 so they tile
B = Image.new("L", (3 * N, 3 * N), 0); dr = ImageDraw.Draw(B)
def bolt(x, y, head, length, width, depth):
    pts = [(x, y)]; steps = int(length / 16)
    for s in range(steps):
        head += rng.normal(0, 0.08)
        ang = head + rng.normal(0, 0.55)          # zigzag around a steady heading
        seg = rng.uniform(10, 22)
        x += seg * math.cos(ang); y += seg * math.sin(ang)
        pts.append((x, y))
        if depth < 2 and s > 1 and rng.random() < (0.10 if depth == 0 else 0.05):
            bolt(x, y, head + rng.choice([-1, 1]) * rng.uniform(0.45, 0.8), length * rng.uniform(0.25, 0.45),
                 max(1, width - 1), depth + 1)
    dr.line(pts, fill=255, width=width, joint="curve")
starts = [(0.10, 0.05, 1.25), (0.60, 0.48, 1.9), (0.85, 0.15, 2.4), (0.30, 0.62, 0.9)]
for (sx, sy, h) in starts:
    bolt(N + sx * N, N + sy * N, h, rng.uniform(260, 340), 5, 0)
core = np.asarray(B, dtype=float) / 255
fold = np.zeros((N, N))
for i in range(3):
    for j in range(3):
        fold = np.maximum(fold, core[i * N:(i + 1) * N, j * N:(j + 1) * N])
def blur_t(a, r):
    big = np.tile(a, (3, 3)); im = Image.fromarray((big * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(r))
    return (np.asarray(im, dtype=float) / 255)[N:2 * N, N:2 * N]
glow = blur_t(fold, 7) * 2.2
coreb = blur_t(fold, 0.8)
fx = np.clip(coreb[..., None] * np.array([0.75, 0.92, 1.0]) + np.clip(glow, 0, 1)[..., None] * np.array([0.10, 0.35, 0.95]) * 0.6, 0, 1)
Image.fromarray((fx * 255).astype(np.uint8)).save(os.path.join(D, "fx.png"))
# tile check
p = Image.open(os.path.join(D, "paint.png")); f = Image.open(os.path.join(D, "fx.png"))
tc = Image.new("RGB", (2 * N, 2 * N))
for i in range(2):
    for j in range(2):
        tc.paste(Image.blend(p, Image.fromarray(np.clip(np.asarray(p, float) + np.asarray(f, float), 0, 255).astype(np.uint8)), 1.0), (i * N, j * N))
tc.save(os.path.join(D, "tile_check.png"))

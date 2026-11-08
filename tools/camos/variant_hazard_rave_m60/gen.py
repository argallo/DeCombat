import numpy as np
from PIL import Image
import os
D = os.path.dirname(os.path.abspath(__file__))
N = 512
y, x = np.mgrid[0:N, 0:N].astype(np.float64)
u, v = x / N, y / N
rng = np.random.default_rng(60)

def tnoise(octaves, base, seed):
    r = np.random.default_rng(seed)
    out = np.zeros((N, N))
    amp = 1.0
    for o in range(octaves):
        f = base * 2 ** o
        for _ in range(4):
            a, b = r.integers(-f, f + 1, 2)
            if a == 0 and b == 0:
                continue
            ph = r.uniform(0, 2 * np.pi)
            out += amp * np.cos(2 * np.pi * (a * u + b * v) + ph)
        amp *= 0.55
    out -= out.min(); out /= out.max()
    return out

K = 5  # stripe pairs per tile along the diagonal
s = (u + v) * K            # stripe coordinate
fr = s - np.floor(s)
# anti-aliased stripe mask: yellow when fr < 0.5
w = 1.2 * K * 2 / N
yel = np.clip((np.minimum(np.abs(fr - 0.25), 1) - 0.25) / -w * 0.5 + 0.5, 0, 1)
yel = np.clip(0.5 - (np.abs(fr - 0.25) - 0.25) / w, 0, 1)

Y = np.array([0.74, 0.56, 0.06])
B = np.array([0.15, 0.14, 0.13])
img = yel[..., None] * Y + (1 - yel[..., None]) * B

# grime: low-frequency tone variation
g = tnoise(4, 2, 1)
img *= (0.82 + 0.3 * g)[..., None]

# scuffs: worn patches where yellow wears to dark metal, black wears to grey
sc = tnoise(5, 3, 7)
wear = np.clip((sc - 0.72) / 0.12, 0, 1)
metal = np.array([0.36, 0.35, 0.33])
img = img * (1 - 0.75 * wear[..., None]) + metal * 0.75 * wear[..., None]

# scratches: thin random short lines, wrapped
scr = np.zeros((N, N))
for i in range(260):
    x0, y0 = rng.uniform(0, N, 2)
    ang = rng.uniform(0, np.pi)
    L = rng.uniform(8, 45)
    t = np.linspace(0, L, int(L * 2))
    xs = (x0 + np.cos(ang) * t).astype(int) % N
    ys = (y0 + np.sin(ang) * t).astype(int) % N
    scr[ys, xs] = np.maximum(scr[ys, xs], rng.uniform(0.4, 1.0))
img = img * (1 - 0.45 * scr[..., None]) + np.array([0.55, 0.53, 0.5]) * 0.45 * scr[..., None]

# fine grain
img *= (0.94 + 0.12 * rng.random((N, N)))[..., None]
img = np.clip(img, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(D, 'paint.png'))

# ---- effect: thin neon-green streaks running along the stripe edges, dashed along the stripe direction
d = (u - v)  # along-stripe coordinate (wraps with integer freq)
fx = np.zeros((N, N))
# edges at fr = 0 and fr = 0.5; distance in pixels across stripe
pix_per_s = N / K / np.sqrt(2)  # pixels per unit of s across the stripe
for edge, freq, seed in ((0.0, 3, 11), (0.5, 2, 12)):
    dd = (fr - edge + 0.5) % 1 - 0.5
    dist = np.abs(dd) * pix_per_s
    # place the light just inside the black band
    off = 9.0 if edge == 0.0 else -9.0
    dist = np.abs(dd * pix_per_s + off)
    line = 1.2 * np.exp(-(dist / 3.4) ** 2) + 0.45 * np.exp(-(dist / 10.0) ** 2)
    # dashes: different per stripe index so they don't line up
    idx = np.floor(s + 0.5 - edge)
    ph = np.array([0.0, 0.43, 0.81, 0.17, 0.62])[(idx.astype(int) * (1 if edge == 0 else 2)) % K]
    dash_c = (d * freq + ph) % 1
    dash = np.clip(1 - np.abs(dash_c - 0.5) / 0.3, 0, 1) ** 1.2
    # comet tail
    fx += line * dash
fx = np.clip(fx, 0, 1)
G = np.array([0.45, 1.0, 0.3])
fximg = fx[..., None] * G
Image.fromarray((np.clip(fximg, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, 'fx.png'))
print('ok', img.mean(axis=(0, 1)), fx.mean())

import numpy as np
from PIL import Image
import os
N = 512
rng = np.random.default_rng(521)
y, x = np.mgrid[0:N, 0:N] / N  # u = x, v = y
TAU = 2*np.pi

def pnoise(nterms, maxf, rng):
    out = np.zeros((N, N))
    for _ in range(nterms):
        fx, fy = rng.integers(-maxf, maxf+1, 2)
        if fx == 0 and fy == 0: continue
        amp = 1.0/np.hypot(fx, fy)
        out += amp*np.cos(TAU*(fx*x + fy*y) + rng.uniform(0, TAU))
    return out/np.abs(out).max()

# rind stripes: 5 dark stripes across u, wavy along v
phase = 5*x + 0.10*np.sin(TAU*(2*y) + 1.0) + 0.05*np.sin(TAU*(5*y + x)) + 0.06*pnoise(30, 6, rng)
s = np.sin(TAU*phase)
# ragged forked edges
s += 0.35*pnoise(60, 14, rng)
dark = (26, 80, 26)
light = (76, 140, 36)
mid = (78, 150, 52)
t = np.clip((s + 0.15)/0.25, 0, 1)[..., None]
img = np.array(light)*(1-t) + np.array(dark)*t
# fine speckle in the light rind
sp = pnoise(80, 40, rng)
img += (sp[..., None]*np.array([10, 14, 6]))

# flesh patches: blobby periodic noise
b = pnoise(40, 3, rng)*0.8 + 0.25*pnoise(60, 9, rng)
T = np.quantile(b, 0.62)
flesh = b > T
pale = (b > T-0.12) & ~flesh
white = (b > T-0.06) & ~flesh
img[pale] = (108, 152, 58)
img[white] = (140, 170, 88)
# flesh colour with juicy gradient: deeper red in the centre
depth = np.clip((b - T)/0.35, 0, 1)
fr = np.stack([190 - 30*depth, 38 - 22*depth, 48 - 22*depth], -1)
fr += pnoise(80, 30, rng)[..., None]*np.array([12, 8, 8])
img[flesh] = fr[flesh]

# seeds: teardrops scattered inside flesh (wrap-around)
seed = np.zeros((N, N), bool)
hi = np.zeros((N, N), bool)
cand = np.argwhere(b > T+0.07)
rng.shuffle(cand)
placed = []
for cy, cx in cand:
    if len(placed) > 120: break
    if any(min(abs(cy-py), N-abs(cy-py))**2 + min(abs(cx-px), N-abs(cx-px))**2 < 26**2 for py, px in placed):
        continue
    placed.append((cy, cx))
    ang = rng.uniform(0, np.pi)
    dy = (y*N - cy + N/2) % N - N/2
    dx = (x*N - cx + N/2) % N - N/2
    a = dx*np.cos(ang) + dy*np.sin(ang)
    c = -dx*np.sin(ang) + dy*np.cos(ang)
    L, W = 10.0, 5.5
    w = W*np.clip(0.55 + 0.45*a/L, 0.15, 1)  # teardrop: wide at +a
    m = (np.abs(a) < L) & (np.abs(c) < w*np.sqrt(np.clip(1 - (a/L)**2, 0, 1)) + 0.5)
    seed |= m
    hi |= m & (a > 2) & (a < 5) & (c > 0.5) & (c < 2.5)
img[seed] = (28, 22, 20)
img[hi] = (95, 85, 75)
img = np.clip(img, 0, 255).astype(np.uint8)
d = os.path.dirname(os.path.abspath(__file__))
Image.fromarray(img).save(os.path.join(d, 'paint.png'))
print(len(placed), 'seeds; flesh frac', flesh.mean())

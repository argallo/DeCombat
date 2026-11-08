# TAR-21 "Mirage": sunset dunes. Procedural, tileable on the torus (integer frequencies only).
import os, numpy as np
from PIL import Image
A = os.path.dirname(os.path.abspath(__file__))
N = 512
y, x = np.mgrid[0:N, 0:N] / N
TAU = 2 * np.pi

def pnoise(octaves, amp0, seed):
    r = np.random.default_rng(seed); f = np.zeros((N, N))
    for o in range(octaves):
        for _ in range(4):
            kx, ky = r.integers(-(o + 1) * 2, (o + 1) * 2 + 1, 2)
            if kx == 0 and ky == 0: continue
            f += amp0 / (o + 1) * np.sin(TAU * (kx * x + ky * y) + r.uniform(0, TAU))
    return f

# dunes: diagonal ridges bent by periodic noise; asymmetric profile (long sunlit slope, short slip face)
warp = pnoise(3, 0.10, 1)
t = (3 * x + 5 * y + warp) % 1.0
s = 0.74
v = np.where(t < s, 0.42 + 0.58 * (t / s) ** 0.8, 0.10 * (1 - (t - s) / (1 - s)) ** 0.7)
v = np.clip(v + 0.05 * pnoise(2, 0.5, 11), 0, 1)
lit = t < s

# wind-blown sand lines: fat, roughly parallel to the crests, wavering
warp2 = 0.6 * warp + pnoise(2, 0.06, 7)
rw = np.sin(TAU * (8 * x + 14 * y + 2.5 * warp2))
line = np.clip((rw - 0.45) / 0.2, 0, 1)            # bold bands
# crest glint: thin hot band right at the top of each dune
crest = np.clip(1 - np.abs(t - (s - 0.025)) / 0.025, 0, 1) * lit

violet = np.array([0.17, 0.06, 0.30]); plum = np.array([0.32, 0.08, 0.32])
rose = np.array([0.50, 0.12, 0.27]); orange = np.array([0.60, 0.20, 0.06])
gold = np.array([0.66, 0.30, 0.06])
stops = [(0.0, violet), (0.3, plum), (0.55, rose), (0.8, orange), (1.0, gold)]
img = np.zeros((N, N, 3))
for (a, ca), (b, cb) in zip(stops[:-1], stops[1:]):
    m = (v >= a) & (v <= b); w = ((v - a) / (b - a))[..., None]
    img[m] = (ca * (1 - w) + cb * w)[m]

L = line[..., None]
shadowline = np.array([0.40, 0.09, 0.26])      # rose-violet shadow inside each ripple on the lit slope
img = np.where(lit[..., None], img * (1 - 0.4 * L) + shadowline * 0.4 * L,
               img + L * np.array([0.10, 0.03, 0.10]))
img = img * (1 - 0.6 * crest[..., None]) + np.array([0.72, 0.36, 0.08]) * 0.6 * crest[..., None]
img = np.clip(img, 0, 1)
u8 = (img * 255).astype(np.uint8)
Image.fromarray(u8).save(os.path.join(A, 'paint.png'))
Image.fromarray(np.tile(u8, (2, 2, 1))).save(os.path.join(A, 'tile_check.png'))

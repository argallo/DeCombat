# Neon Tiger: black tiger stripes over a magenta/teal blend; fx = thin neon stripe edges. Procedural, tileable.
import os, sys
import numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(int(sys.argv[1]) if len(sys.argv) > 1 else 48)
y, x = np.mgrid[0:N, 0:N] / N  # 0..1 on the torus

def pnoise(oct_list):
    f = np.zeros((N, N))
    for (kmax, amp) in oct_list:
        for _ in range(6):
            kx, ky = rng.integers(-kmax, kmax + 1, 2)
            if kx == 0 and ky == 0: kx = 1
            ph = rng.uniform(0, 2 * np.pi)
            f += amp * np.sin(2 * np.pi * (kx * x + ky * y) + ph)
    return f / np.abs(f).max()

# colour blend magenta <-> teal, big soft areas
b = pnoise([(1, 1.0), (2, 0.4)])
t = np.clip(0.5 + 1.0 * b, 0, 1)
t = t * t * (3 - 2 * t); t = t * t * (3 - 2 * t)
mag = np.array([0.40, 0.03, 0.30])
teal = np.array([0.02, 0.28, 0.33])
base = mag[None, None] * (1 - t[..., None]) + teal[None, None] * t[..., None]
# a little mid tone variation (fur shading)
base *= (0.88 + 0.12 * pnoise([(4, 1.0)]))[..., None]

# tiger stripes: diagonal bands, wobbled, width-modulated so they taper to points and break into strokes
def anoise(kxm, kym, n=8):
    f = np.zeros((N, N))
    for _ in range(n):
        kx = rng.integers(-kxm, kxm + 1); ky = rng.integers(1, kym + 1) * rng.choice([-1, 1])
        f += rng.uniform(0.5, 1) * np.sin(2 * np.pi * (kx * x + ky * y) + rng.uniform(0, 2 * np.pi))
    return f / np.abs(f).max()
warp = 0.16 * pnoise([(1, 1.0), (2, 0.5)]) + 0.035 * pnoise([(5, 1.0)])
s = 5 * x + 1 * y + warp
d = np.abs((s % 1.0) - 0.5) * 2      # 0 at stripe centre, 1 between
w = 0.22 + 0.36 * anoise(3, 2)       # half-width, <0 breaks the stripe into long tapered strokes
s2 = 5 * x + 1 * y + 0.5 + warp * 1.2
d2 = np.abs((s2 % 1.0) - 0.5) * 2
w2 = 0.36 * anoise(4, 2) - 0.08      # short in-between strokes
m1 = w - d          # >0 inside stripe
m2 = w2 - d2
m = np.maximum(m1, m2)
inside = m > 0
img = base.copy()
img[inside] = np.array([0.035, 0.03, 0.045])
# soft-ish edge (anti alias)
aa = np.clip(m / 0.02 + 0.5, 0, 1)[..., None]
img = base * (1 - aa) + np.array([0.035, 0.03, 0.045]) * aa

Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))

# fx: thin neon halo just outside each stripe edge, coloured like the neighbouring base (brighter)
edge = np.exp(-((m + 0.045) / 0.04) ** 2) * (m < 0.005)   # halo just OUTSIDE the black stripe
neon = np.clip(base * 2.8 + 0.12, 0, 1)
fx = neon * edge[..., None]
Image.fromarray((np.clip(fx, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "fx.png"))
# tile check
p = Image.open(os.path.join(D, "paint.png")); f = Image.open(os.path.join(D, "fx.png"))
tc = Image.new("RGB", (2 * N, 2 * N))
for i in range(2):
    for j in range(2): tc.paste(p, (i * N, j * N))
tc.save(os.path.join(D, "tile_check.png"))
print("fx mean", fx.mean(), "stripe frac", inside.mean())

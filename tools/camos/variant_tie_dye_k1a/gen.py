# Tie-Dye: procedural, tileable spiral tie-dye (original art, numpy only).
import sys, numpy as np
from PIL import Image
N = 512
SCALE = float(sys.argv[1]) if len(sys.argv) > 1 else 0.9   # palette brightness multiplier
rng = np.random.default_rng(529)
y, x = np.mgrid[0:N, 0:N] / N
TAU = 2 * np.pi

def pnoise(oct_=4, base=2, amp=1.0):
    # periodic noise: sum of integer-frequency waves on the torus
    out = np.zeros((N, N))
    for o in range(oct_):
        f = base * 2 ** o
        for _ in range(3):
            kx, ky = rng.integers(-f, f + 1, 2)
            if kx == 0 and ky == 0: kx = f
            out += amp / (o + 1) * np.sin(TAU * (kx * x + ky * y) + rng.uniform(0, TAU))
    return out / oct_

# gentle domain warp (periodic), so the spirals look hand-dyed
wx = 0.035 * pnoise(3, 1); wy = 0.035 * pnoise(3, 1)
u = x + wx; v = y + wy
X = np.sin(TAU * u); Y = np.sin(TAU * v)
theta = np.arctan2(Y, X)                     # periodic; spirals around 4 points per tile
rho = np.sqrt(X * X + Y * Y)
ARMS = 1
phase = ARMS * theta / TAU + 1.5 * rho + 0.08 * pnoise(4, 2)
phase = phase % 1.0

# cyclic rainbow, kept mid-tone and saturated
pal = np.array([
    [0.62, 0.08, 0.14],  # red
    [0.66, 0.28, 0.04],  # orange
    [0.60, 0.50, 0.04],  # yellow
    [0.12, 0.48, 0.12],  # green
    [0.04, 0.38, 0.46],  # teal
    [0.10, 0.18, 0.58],  # blue
    [0.36, 0.10, 0.54],  # purple
    [0.58, 0.08, 0.38],  # magenta
]) * SCALE
K = len(pal)
t = phase * K
i0 = np.floor(t).astype(int) % K; i1 = (i0 + 1) % K
f = t - np.floor(t)
f = f * f * (3 - 2 * f)  # smooth band transitions, still visible bands
col = pal[i0] * (1 - f)[..., None] + pal[i1] * f[..., None]

# band edges a touch darker where dye pooled
edge = np.abs(np.sin(np.pi * t)) ** 0.35
col *= (0.80 + 0.20 * edge)[..., None]

# crinkled fold lines: thin ridges along a second, tighter spiral, broken up by noise
fold_phase = 7 * theta / TAU + 0.9 * rho + 0.22 * pnoise(5, 4)
ridge = np.abs(np.sin(np.pi * fold_phase))
crinkle = 0.5 + 0.5 * pnoise(5, 4)
fold = np.clip((ridge - 0.84) / 0.10, 0, 1) * np.clip(crinkle * 2.2 - 0.5, 0, 1)
fold_col = 0.45 * col + 0.30 * SCALE   # white-ish tint of the local dye, but mid-tone
col = col * (1 - fold[..., None] * 0.9) + fold_col * fold[..., None] * 0.9

# the dye's bleed at the spiral centres: a mid-tone pale blot
centre = np.clip(1 - rho / 0.18, 0, 1) ** 1.5
col = col * (1 - 0.55 * centre[..., None]) + np.array([0.48, 0.42, 0.46]) * SCALE * 0.55 * centre[..., None]

img = (np.clip(col, 0, 1) * 255).astype(np.uint8)
out = sys.argv[2] if len(sys.argv) > 2 else __file__.replace('gen.py', 'paint.png')
Image.fromarray(img).save(out)
Image.fromarray(np.tile(img, (2, 2, 1))).resize((512, 512)).save(out.replace('.png', '_tile.png'))
print('mean', img.reshape(-1, 3).mean(0) / 255)

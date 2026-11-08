# Molten Core camo for the AK-47: procedural, tileable basalt crust with lava cracks.
import numpy as np
from PIL import Image
import os
N = 512
rng = np.random.default_rng(510)
out = os.path.dirname(os.path.abspath(__file__))

yy, xx = np.mgrid[0:N, 0:N].astype(np.float32) / N

def tor_noise(octaves, base, seed):
    r = np.random.default_rng(seed)
    acc = np.zeros((N, N), np.float32); amp = 1.0; tot = 0
    for o in range(octaves):
        f = base * 2 ** o
        for _ in range(4):
            kx, ky = r.integers(-f, f + 1, 2)
            if kx == 0 and ky == 0: kx = 1
            ph = r.uniform(0, 2 * np.pi)
            acc += amp * np.sin(2 * np.pi * (kx * xx + ky * yy) + ph)
        tot += amp * 4; amp *= 0.55
    return acc / tot * 2.0

def voronoi(g, seed, warpx, warpy):
    # jittered g x g grid: seeds never bunch up, so no smeared lava fans
    r = np.random.default_rng(seed)
    gi, gj = np.mgrid[0:g, 0:g]
    pts = (np.stack([gi.ravel(), gj.ravel()], -1) + 0.15 + 0.7 * r.random((g * g, 2))) / g
    X = (xx + warpx) % 1.0; Y = (yy + warpy) % 1.0
    d1 = np.full((N, N), 9.0, np.float32); d2 = np.full((N, N), 9.0, np.float32)
    for px, py in pts:
        dx = np.abs(X - px); dx = np.minimum(dx, 1 - dx)
        dy = np.abs(Y - py); dy = np.minimum(dy, 1 - dy)
        d = np.sqrt(dx * dx + dy * dy)
        m = d < d1
        d2 = np.where(m, d1, np.minimum(d2, d)); d1 = np.where(m, d, d1)
    return d2 - d1, d1

wx = 0.03 * tor_noise(3, 2, 1); wy = 0.03 * tor_noise(3, 2, 2)
edge, _ = voronoi(6, 7, wx, wy)               # main plates
edge2, _ = voronoi(11, 9, wx * 1.5, wy * 1.5) # finer secondary cracks

width = 0.013 + 0.010 * (tor_noise(2, 3, 5) * 0.5 + 0.5)
core = np.clip(1.3 - edge / width, 0, 1)                # main crack (soft-edged)
halo = np.exp(-edge / 0.018)                            # heat bleeding into crust, tight
fine_mask = np.clip(0.35 + tor_noise(2, 2, 11), 0, 1)
fine = np.clip(1.2 - edge2 / 0.0065, 0, 1) * fine_mask
lava = np.clip(np.maximum(core, fine * 0.85), 0, 1)

rough = tor_noise(5, 4, 3)                              # crust surface
pits = np.clip(tor_noise(3, 8, 4) * 1.5, 0, 1)          # darker vesicular pits
ash = np.clip(tor_noise(3, 6, 6) * 1.5 - 0.4, 0, 1)     # cooler ash-grey flecks
base = 0.17 + 0.04 * rough - 0.05 * pits + 0.06 * ash
crust = np.stack([base * 1.02, base * 0.96, base * 1.0], -1)
crust[..., 0] += 0.16 * halo; crust[..., 1] += 0.04 * halo; crust[..., 2] -= 0.04 * halo
hot = np.clip(core * np.maximum(core, fine), 0, 1) ** 2
lava_col = np.stack([0.86 + 0.08 * hot, 0.25 + 0.24 * hot, 0.02 + 0.04 * hot], -1)
a = lava[..., None]
img = crust * (1 - a) + lava_col * a
img = np.clip(img, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(out, "paint.png"))
# tile check
t = np.tile(img, (2, 2, 1))
Image.fromarray((t * 255).astype(np.uint8)).resize((512, 512)).save(os.path.join(out, "tile_check.png"))

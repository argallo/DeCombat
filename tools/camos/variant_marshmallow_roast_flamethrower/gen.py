# Marshmallow Roast: procedural, tileable. Toasted marshmallows over campfire embers.
import numpy as np
from PIL import Image
import os, sys
N = 512
OUT = os.path.dirname(os.path.abspath(__file__))
rng = np.random.default_rng(int(sys.argv[1]) if len(sys.argv) > 1 else 7)
y, x = np.mgrid[0:N, 0:N].astype(np.float64)
u, v = x / N * 2 * np.pi, y / N * 2 * np.pi

def tnoise(octs):
    f = np.zeros((N, N))
    for fu, fv, a in octs:
        for _ in range(3):
            p1, p2 = rng.uniform(0, 2*np.pi, 2)
            ku, kv = rng.integers(-fu, fu+1), rng.integers(-fv, fv+1)
            if ku == 0 and kv == 0: ku = fu
            f += a * np.sin(ku*u + kv*v + p1) * np.cos(rng.integers(1, fu+1)*u - rng.integers(0, fv+1)*v + p2)
    return f

# --- embers background ---
n1 = tnoise([(2,2,1.0),(4,4,0.6),(7,7,0.35),(12,12,0.2)])
n1 = (n1 - n1.min()) / (n1.max() - n1.min())
n2 = tnoise([(5,5,1.0),(9,9,0.5)])
n2 = (n2 - n2.min()) / (n2.max() - n2.min())
# cracks: thin dark lines where n2 crosses 0.5
crack = np.exp(-((n2 - 0.5) / 0.035) ** 2)
deep = np.array([0.30, 0.06, 0.03])     # deep red coal
mid = np.array([0.52, 0.15, 0.04])      # orange-red
hot = np.array([0.62, 0.30, 0.05])      # hot orange
t = n1[..., None]
bg = np.where(t < 0.5, deep + (mid - deep) * (t / 0.5), mid + (hot - mid) * ((t - 0.5) / 0.5))
ash = np.array([0.16, 0.06, 0.04])
bg = bg * (1 - 0.75 * crack[..., None]) + ash * 0.75 * crack[..., None]
img = bg.copy()

def wrapd(a, c):
    d = (a - c + N/2) % N - N/2
    return d

# --- marshmallows on a jittered grid ---
cols, rows = 4, 4
cells = [(i, j) for i in range(cols) for j in range(rows)]
kinds = ['toast'] * 16
for k in rng.choice(16, 4, replace=False): kinds[k] = 'char'
for k in rng.choice([i for i in range(16) if kinds[i] == 'toast'], 3, replace=False): kinds[k] = 'gold'
order = rng.permutation(16)
for idx in order:
    i, j = cells[idx]
    cx = (i + 0.5 + (j % 2) * 0.5) * N / cols + rng.uniform(-14, 14)
    cy = (j + 0.5) * N / rows + rng.uniform(-10, 10)
    ang = rng.uniform(-0.6, 0.6) + (np.pi/2 if rng.random() < 0.35 else 0)
    hw, hh = rng.uniform(46, 54), rng.uniform(34, 40)  # half sizes of the pill (side view of a cylinder)
    r = hh * 0.75
    dx, dy = wrapd(x, cx), wrapd(y, cy)
    ca, sa = np.cos(ang), np.sin(ang)
    px, py = ca*dx + sa*dy, -sa*dx + ca*dy
    qx, qy = np.abs(px) - (hw - r), np.abs(py) - (hh - r)
    sd = np.hypot(np.maximum(qx, 0), np.maximum(qy, 0)) + np.minimum(np.maximum(qx, qy), 0) - r
    inside = sd < 0
    shadow = (sd > 0) & (sd < 9)
    # drop shadow / outline
    img[shadow] = img[shadow] * (0.35 + 0.65 * (sd[shadow, None] / 9))
    depth = np.clip(-sd / r, 0, 1)                      # 0 at rim, 1 well inside
    kind = kinds[idx]
    if kind == 'char':
        rim = np.array([0.08, 0.05, 0.04]); core = np.array([0.24, 0.15, 0.10])
        tk = np.clip(depth / 0.9, 0, 1) ** 1.5
    elif kind == 'gold':
        rim = np.array([0.50, 0.30, 0.08]); core = np.array([0.52, 0.43, 0.29])
        tk = np.clip(depth / 0.6, 0, 1) ** 1.0
    else:
        rim = np.array([0.36, 0.17, 0.05]); core = np.array([0.49, 0.40, 0.27])
        tk = np.clip(depth / 0.8, 0, 1) ** 1.3
    col = rim + (core - rim) * tk[..., None]
    # blister blotches of toast on the surface
    ph = rng.uniform(0, 6, 4)
    bl = (np.sin(px*0.07 + ph[0]) + np.sin(py*0.11 + ph[1]) + np.sin((px+py)*0.05 + ph[2]) + np.sin((px-py)*0.09 + ph[3])) / 4
    blot = np.clip((bl - 0.15) / 0.35, 0, 1) * (0.6 if kind != 'char' else 0.3) * np.clip(1.2 - depth, 0, 1)
    col = col * (1 - blot[..., None]) + rim * blot[..., None]
    # top-left highlight (puffy)
    hl = np.exp(-(((px + hw*0.25) / (hw*0.45)) ** 2 + ((py + hh*0.45) / (hh*0.28)) ** 2))
    col = col + (0.08 if kind != 'char' else 0.07) * hl[..., None]
    # warm under-glow from the fire on the bottom edge
    glow = np.clip((py / hh), 0, 1) ** 2 * np.clip(1 - depth*2, 0, 1)
    col = col + np.array([0.12, 0.04, 0.0]) * glow[..., None]
    img[inside] = col[inside]

img = np.clip(img, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(OUT, 'paint.png'))
t = np.tile(img, (2, 2, 1))
Image.fromarray((t * 255).astype(np.uint8)).resize((512, 512)).save(os.path.join(OUT, 'tile_check.png'))
print('ok')

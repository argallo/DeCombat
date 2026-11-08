# Neon Leopard: black leopard rosettes on hot pink, some with electric teal centres. Procedural, tileable.
import numpy as np, sys, os
from PIL import Image
OUT = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(530)
y, x = np.mgrid[0:N, 0:N] / N

def tnoise(seed, terms=6, fmax=5):
    r = np.random.default_rng(seed); v = np.zeros((N, N))
    for _ in range(terms):
        fx, fy = r.integers(-fmax, fmax + 1, 2)
        if fx == 0 and fy == 0: fx = 1
        v += np.sin(2*np.pi*(fx*x + fy*y) + r.uniform(0, 2*np.pi)) / np.hypot(fx, fy)
    return v / np.abs(v).max()

BASE = np.array([0.50, 0.07, 0.31])
DEEP = np.array([0.34, 0.04, 0.20])
TEAL = np.array([0.03, 0.38, 0.40])
BLACK = np.array([0.05, 0.03, 0.05])

# base pink with soft tonal variation
tone = tnoise(1, 8, 4)
img = BASE[None, None] * (1 + 0.08 * tone[..., None])

G = int(sys.argv[1]) if len(sys.argv) > 1 else 5
cells = [(i, j) for i in range(G) for j in range(G)]
warp_a = tnoise(2, 8, 9) * 0.012
warp_b = tnoise(3, 8, 9) * 0.012
xs, ys = x + warp_a, y + warp_b
ring = np.zeros((N, N), bool); centre = np.zeros((N, N), int)  # 0 none, 1 deep, 2 teal
spots = []
for k, (i, j) in enumerate(cells):
    cx = (i + 0.5 + rng.uniform(-0.28, 0.28) + (0.5 if j % 2 else 0)) / G
    cy = (j + 0.5 + rng.uniform(-0.22, 0.22)) / G
    r = rng.uniform(0.33, 0.42) / G
    spots.append((cx % 1, cy % 1, r, rng.random() < 0.30))
for (cx, cy, r, teal) in spots:
    dx = (xs - cx + 0.5) % 1 - 0.5; dy = (ys - cy + 0.5) % 1 - 0.5
    ang = np.arctan2(dy, dx)
    # irregular radius
    ph = rng.uniform(0, 2*np.pi, 3)
    rr = r * (1 + 0.12*np.sin(2*ang + ph[0]) + 0.08*np.sin(3*ang + ph[1]) + 0.05*np.sin(5*ang + ph[2]))
    d = np.hypot(dx * 1.0, dy * 1.08)
    w = r * (0.24 + 0.10*np.sin(4*ang + ph[1]))
    # break the ring into 3-5 blobs with gaps
    nseg = rng.integers(3, 6); off = rng.uniform(0, 2*np.pi)
    seg = np.cos(nseg * (ang + off) / 1.0 * 1) 
    gap = seg < -0.82
    m = (np.abs(d - rr) < w) & ~gap
    ring |= m
    inner = d < rr - w * 0.6
    centre[inner] = 2 if teal else 1
# free small solid spots between rosettes
for _ in range(G * G):
    cx, cy = rng.random(2); r = rng.uniform(0.07, 0.12) / G
    dx = (xs - cx + 0.5) % 1 - 0.5; dy = (ys - cy + 0.5) % 1 - 0.5
    near = min(np.hypot((cx-a+0.5)%1-0.5, (cy-b+0.5)%1-0.5) - rad for a, b, rad, _ in spots)
    if near < 0.55 / G * 0.5: continue
    ring |= np.hypot(dx, dy * 1.1) < r * (1 + 0.2 * tnoise(int(cx*1000), 4, 6)[0, 0])

img[centre == 1] = DEEP * (1 + 0.06 * tone[centre == 1, None])
img[centre == 2] = TEAL * (1 + 0.10 * tone[centre == 2, None])
img[ring] = BLACK
img = np.clip(img, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(OUT, "paint.png"))
# tile preview
Image.fromarray(np.tile((img*255).astype(np.uint8), (2, 2, 1))).resize((512, 512)).save(os.path.join(OUT, "tile2x2.png"))
print("ok")

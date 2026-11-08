# MP7 "Glitch" - procedural, tileable 512x512. Paint + effect layer.
import numpy as np, os, sys
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(int(sys.argv[1]) if len(sys.argv) > 1 else 7)
yy, xx = np.mgrid[0:N, 0:N]

MAG = np.array([0.72, 0.10, 0.58]); CYA = np.array([0.08, 0.60, 0.72]); LIM = np.array([0.45, 0.72, 0.08])
COLS = [MAG, CYA, LIM]

# --- base: dark slate with horizontal datamosh bands (rows of varying tone + horizontal smear)
img = np.zeros((N, N, 3)); img[:] = [0.12, 0.13, 0.17]
y = 0
while y < N:
    h = int(rng.integers(4, 40))
    tone = rng.uniform(-0.06, 0.07)
    tint = rng.normal(0, 0.012, 3)
    rows = (np.arange(y, y + h)) % N
    img[rows] += tone + tint
    y += h

def rect(a, x0, y0, w, h, col, mode="set", alpha=1.0):
    ys = (np.arange(y0, y0 + h)) % N; xs = (np.arange(x0, x0 + w)) % N
    sub = a[np.ix_(ys, xs)]
    if mode == "set": sub = sub * (1 - alpha) + np.asarray(col) * alpha
    a[np.ix_(ys, xs)] = sub

# --- displaced RGB-split blocks: a coloured block with offset magenta/cyan ghosts (channel split)
for i in range(20):
    w = int(rng.integers(70, 220)); h = int(rng.integers(14, 44))
    x0 = int(rng.integers(0, N)); y0 = int(i * N / 20 + rng.integers(-8, 8)) % N
    c = COLS[i % 3] * rng.uniform(0.8, 1.0)
    off = int(rng.integers(6, 16))
    # ghosts: pure-ish channel split either side
    rect(img, x0 - off, y0, w, h, [0.62, 0.06, 0.40], alpha=0.85)   # magenta ghost left
    rect(img, x0 + off, y0, w, h, [0.05, 0.50, 0.62], alpha=0.85)   # cyan ghost right
    rect(img, x0, y0, w, h, c)
    # inner datamosh stripes inside the block
    for k in range(int(rng.integers(1, 4))):
        sy = int(rng.integers(0, h)); sh = int(rng.integers(1, 4))
        rect(img, x0, y0 + sy, w, sh, c * 0.45 + 0.04)

# --- smaller slate "macroblocks" (compression squares) in darker/lighter slate
for i in range(40):
    s = int(rng.choice([16, 24, 32]))
    x0 = int(rng.integers(0, N // 8)) * 8; y0 = int(rng.integers(0, N // 8)) * 8
    t = rng.uniform(-0.07, 0.06)
    rect(img, x0, y0, s, s, np.clip([0.12 + t, 0.13 + t, 0.18 + t], 0, 1), alpha=0.9)

# --- horizontal row displacement (whole bands shifted sideways = datamosh tear)
y = 0
while y < N:
    h = int(rng.integers(6, 30))
    if rng.random() < 0.35:
        sh = int(rng.integers(-60, 60))
        rows = np.arange(y, y + h) % N
        img[rows] = np.roll(img[rows], sh, axis=1)
    y += h

# --- scanlines: every other pair of rows a touch darker, plus fine noise
scan = np.where((yy // 2) % 2 == 0, 1.0, 0.82)[..., None]
img = img * scan
img += rng.normal(0, 0.018, (N, N, 1))
img = np.clip(img, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))

# --- effect: mostly black, thin coloured glitch bars (with RGB-split neighbours), sparse
fx = np.zeros((N, N, 3))
FXC = [np.array([1.0, 0.15, 0.80]), np.array([0.15, 0.90, 1.0]), np.array([0.6, 1.0, 0.15])]
for i in range(32):
    w = int(rng.integers(60, 220)); h = int(rng.integers(5, 10))
    x0 = int(rng.integers(0, N)); y0 = int(i * N / 32 + rng.integers(-5, 5)) % N
    c = FXC[i % 3]
    # soft ends along the bar
    xs = np.arange(w); env = np.clip(np.minimum(xs, w - 1 - xs) / 18.0, 0, 1)
    ys = np.arange(y0, y0 + h) % N; xi = (x0 + xs) % N
    fx[np.ix_(ys, xi)] = np.maximum(fx[np.ix_(ys, xi)], c[None, None, :] * env[None, :, None])
    # 1px offset split line in another colour just below
    c2 = FXC[(i + 1) % 3]; ys2 = np.array([(y0 + h + 1) % N]); xi2 = (x0 + 8 + xs) % N
    fx[np.ix_(ys2, xi2)] = np.maximum(fx[np.ix_(ys2, xi2)], c2[None, None, :] * env[None, :, None] * 0.7)
Image.fromarray((np.clip(fx, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "fx.png"))
# tile check
p = Image.open(os.path.join(D, "paint.png")); t = Image.new("RGB", (N * 2, N * 2))
for a in (0, N):
    for b in (0, N): t.paste(p, (a, b))
t.save(os.path.join(D, "tile_check.png"))

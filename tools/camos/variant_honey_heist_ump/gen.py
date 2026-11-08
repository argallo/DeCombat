# Honey Heist camo for the UMP: procedural tileable honeycomb, honey drips and cartoon bees. Original art.
import numpy as np, os, sys
from PIL import Image
N = 512
out = os.path.dirname(os.path.abspath(__file__))
rng = np.random.default_rng(548)
yy, xx = (np.mgrid[0:N, 0:N].astype(np.float32) + 0.5) / N   # yy = v (down), xx = u

def tor_noise(octaves, base, seed):
    r = np.random.default_rng(seed); acc = np.zeros((N, N), np.float32); amp = 1.0; tot = 0
    for o in range(octaves):
        f = base * 2 ** o
        for _ in range(4):
            kx, ky = r.integers(-f, f + 1, 2)
            if kx == 0 and ky == 0: kx = 1
            acc += amp * np.sin(2 * np.pi * (kx * xx + ky * yy) + r.uniform(0, 2 * np.pi))
        tot += amp * 4; amp *= 0.55
    return acc / tot * 2.0

def wrap(a): return (a + 0.5) % 1.0 - 0.5

# hex lattice on the torus: COLS columns, ROWS (even) rows, odd rows shifted half a column
COLS, ROWS = 4, 4
cent = []
for j in range(ROWS):
    for i in range(COLS):
        cent.append(((i + 0.5 * (j % 2) + 0.25) / COLS, (j + 0.5) / ROWS))
cent = np.array(cent)
d1 = np.full((N, N), 9.0, np.float32); d2 = d1.copy(); idx = np.zeros((N, N), int)
DX = np.zeros((N, N), np.float32); DY = DX.copy()
for k, (cx, cy) in enumerate(cent):
    dx = wrap(xx - cx); dy = wrap(yy - cy)
    d = np.sqrt(dx * dx + dy * dy)
    m = d < d1
    d2 = np.where(m, d1, np.minimum(d2, d)); d1 = np.where(m, d, d1)
    idx = np.where(m, k, idx); DX = np.where(m, dx, DX); DY = np.where(m, dy, DY)
edge = d2 - d1                  # 0 on the wax walls
R = 1.0 / COLS / np.sqrt(3) * 1.07  # hex radius (rows a little stretched)
ncell = len(cent)
kind = rng.choice([0, 1, 2], ncell, p=[0.38, 0.47, 0.15])   # 0 capped, 1 honey, 2 empty
kind_px = kind[idx]
rr = d1 / R                     # 0 centre .. ~1 corner

grain = tor_noise(4, 6, 3)
# capped cells: domed pale-amber wax with a soft highlight
dome = np.clip(1 - rr, 0, 1)
cap = np.stack([0.66 + 0.10 * dome, 0.44 + 0.10 * dome, 0.13 + 0.06 * dome], -1)
ring = np.clip(1 - np.abs(rr - 0.55) / 0.06, 0, 1)[..., None]
cap = cap * (1 - 0.25 * ring)
cap += 0.025 * grain[..., None]
# honey cells: deep glossy amber, darker toward the walls, with a glint upper-left
deep = np.clip(1 - rr * 1.1, 0, 1)
honey = np.stack([0.46 + 0.22 * deep, 0.16 + 0.14 * deep, 0.02 + 0.02 * deep], -1)
gl = np.exp(-(((DX + 0.32 * R) ** 2 + (DY + 0.30 * R) ** 2) / (0.17 * R) ** 2))
gl2 = np.exp(-(((DX - 0.30 * R) ** 2 + (DY - 0.34 * R) ** 2) / (0.08 * R) ** 2)) * 0.6
glint = np.clip(gl + gl2, 0, 1)[..., None]
honey = honey * (1 - glint) + np.array([0.86, 0.74, 0.42]) * glint
# empty cells: shadowy brown hollow
empty = np.stack([0.26 + 0.08 * rr, 0.14 + 0.05 * rr, 0.04 + 0.02 * rr], -1)
img = np.where(kind_px[..., None] == 0, cap, np.where(kind_px[..., None] == 1, honey, empty))

# wax walls: dark outline at the rim, lighter golden ridge on top
w = edge / R
ridge = np.clip(1 - w / 0.13, 0, 1)
rim = np.clip(1 - np.abs(w - 0.19) / 0.09, 0, 1)
wall = np.array([0.74, 0.55, 0.18]) + 0.04 * grain[..., None]
img = img * (1 - rim[..., None] * 0.8) + np.array([0.12, 0.05, 0.01]) * rim[..., None] * 0.8
img = img * (1 - ridge[..., None]) + wall * ridge[..., None]

# thick honey drips running down (+v) from some honey cells
drip = np.zeros((N, N), np.float32); hl = np.zeros((N, N), np.float32)
honey_ids = np.where(kind == 1)[0]
for k in rng.choice(honey_ids, min(5, len(honey_ids)), replace=False):
    cx, cy = cent[k]
    x0 = cx + rng.uniform(-0.4, 0.4) * R
    length = rng.uniform(0.20, 0.36); wid = rng.uniform(0.026, 0.036)
    dx = wrap(xx - x0); t = wrap(yy - cy)        # t: distance below the cell centre
    t = np.where(t < -0.1, t + 1.0, t)
    tt = np.clip(t / length, 0, 1)
    wv = wid * (1 - 0.35 * tt + 0.12 * np.sin(tt * 9 + k))
    body = (np.abs(dx) < wv) & (t > 0) & (t < length)
    bulb = (dx / (wid * 1.6)) ** 2 + ((t - length) / (wid * 1.9)) ** 2 < 1
    s = (body | bulb).astype(np.float32)
    drip = np.maximum(drip, s)
    h = np.exp(-((dx + 0.45 * wv) / (0.22 * wv)) ** 2) * s
    hb = np.exp(-(((dx + 0.6 * wid) ** 2 + (t - length + 0.7 * wid) ** 2) / (0.5 * wid) ** 2)) * bulb
    hl = np.maximum(hl, np.maximum(h, hb))
# soft dark edge around each drip
from PIL import ImageFilter
dm = np.array(Image.fromarray((drip * 255).astype(np.uint8)).filter(ImageFilter.MaxFilter(7))) / 255.0
dm = np.maximum(dm, 0)
img = img * (1 - 0.5 * (dm - drip)[..., None] .clip(0, 1))
dcol = np.array([0.66, 0.33, 0.03])
img = img * (1 - drip[..., None]) + dcol * drip[..., None]
img = img * (1 - hl[..., None] * 0.8) + np.array([0.88, 0.70, 0.36]) * hl[..., None] * 0.8

# cartoon bees
def bee(img, bx, by, ang, L):
    ca, sa = np.cos(ang), np.sin(ang)
    dx = wrap(xx - bx); dy = wrap(yy - by)
    a = dx * ca + dy * sa; b = -dx * sa + dy * ca      # a along the body
    ax, bxw = L * 0.5, L * 0.32
    # wings (behind body), two lobes above
    for side, off in ((1, -0.12), (-1, 0.16)):
        wa = a - off * L; wb = b + 0.36 * L
        wing = (wa / (0.24 * L)) ** 2 + (wb / (0.17 * L)) ** 2
        wm = wing < 1
        wc = np.array([0.55, 0.62, 0.68]) + 0.12 * np.clip(1 - wing, 0, 1)[..., None] * np.array([1, 1, 1])
        img = np.where(wm[..., None], wc, img)
        wo = np.abs(wing - 1) < 0.12
        img = np.where(wo[..., None], np.array([0.1, 0.09, 0.08]), img)
    e = (a / ax) ** 2 + (b / bxw) ** 2
    bodym = e < 1
    stripe = np.sin(a / L * 2 * np.pi * 2.3 + 0.6) > 0.15
    col = np.where(stripe[..., None], np.array([0.10, 0.08, 0.05]), np.array([0.78, 0.58, 0.06]))
    sh = np.clip(1 - e, 0, 1) ** 0.5
    col = col * (0.75 + 0.35 * sh[..., None])
    img = np.where(bodym[..., None], col, img)
    out_ = (e >= 1) & (e < 1.22)
    img = np.where(out_[..., None], np.array([0.07, 0.05, 0.03]), img)
    # head
    ha = a - 0.56 * L
    hd = (ha ** 2 + b ** 2) / (0.21 * L) ** 2
    img = np.where((hd < 1)[..., None], np.array([0.12, 0.10, 0.07]), img)
    # eye + stinger
    ey = ((ha - 0.06 * L) ** 2 + (b + 0.06 * L) ** 2) / (0.07 * L) ** 2
    img = np.where((ey < 1)[..., None], np.array([0.82, 0.80, 0.74]), img)
    pu = ((ha - 0.08 * L) ** 2 + (b + 0.06 * L) ** 2) / (0.035 * L) ** 2
    img = np.where((pu < 1)[..., None], np.array([0.03, 0.03, 0.03]), img)
    st = (a < -0.5 * L) & (a > -0.68 * L) & (np.abs(b) < (a + 0.68 * L) * 0.35)
    img = np.where(st[..., None], np.array([0.08, 0.06, 0.04]), img)
    return img
L = 0.21
for bx, by, ang in [(0.20, 0.30, 0.5), (0.70, 0.22, -0.5), (0.52, 0.76, 2.6)]:
    img = bee(img, bx, by, ang, L)

# deepen for the bright UMP polymer: darker, a bit more saturated
DARK = float(sys.argv[1]) if len(sys.argv) > 1 else 0.6
lum = img.mean(-1, keepdims=True)
img = (lum + (img - lum) * 1.25) * DARK
img = np.clip(img, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(out, "paint.png"))
t = np.tile(img, (2, 2, 1))
Image.fromarray((t * 255).astype(np.uint8)).resize((512, 512)).save(os.path.join(out, "tile_check.png"))

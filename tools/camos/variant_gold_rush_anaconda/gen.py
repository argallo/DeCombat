import os, numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(556)
y, x = np.mgrid[0:N, 0:N] / N  # 0..1, torus coords

def pnoise(freq, seed, X=None, Y=None):
    """Periodic value noise with integer grid freq (tileable)."""
    X = x if X is None else X; Y = y if Y is None else Y
    r = np.random.default_rng(seed)
    g = r.random((freq, freq))
    fx, fy = (X % 1) * freq, (Y % 1) * freq
    x0, y0 = np.floor(fx).astype(int), np.floor(fy).astype(int)
    tx, ty = fx - x0, fy - y0
    tx = tx * tx * (3 - 2 * tx); ty = ty * ty * (3 - 2 * ty)
    x1, y1 = (x0 + 1) % freq, (y0 + 1) % freq
    x0 %= freq; y0 %= freq
    a = g[y0, x0] * (1 - tx) + g[y0, x1] * tx
    b = g[y1, x0] * (1 - tx) + g[y1, x1] * tx
    return a * (1 - ty) + b * ty

def fbm(base, octs, seed, X=None, Y=None):
    s, amp, tot = 0, 1, 0
    for o in range(octs):
        s = s + amp * pnoise(base * 2 ** o, seed + o, X, Y); tot += amp; amp *= 0.5
    return s / tot

# --- slate rock: cool dark grey with layered strata and grain
rock = fbm(4, 5, 10)
strata = 0.5 + 0.5 * np.sin(2 * np.pi * (3 * y + 1 * x) + 5 * fbm(3, 3, 40))
grain = fbm(32, 2, 70)
r_l = 0.55 * rock + 0.25 * strata + 0.2 * grain
r_l = (r_l - r_l.min()) / (r_l.max() - r_l.min())
slate_dark = np.array([0.07, 0.08, 0.10]); slate_lite = np.array([0.24, 0.26, 0.30])
img = slate_dark + (slate_lite - slate_dark) * r_l[..., None]

# --- gold veins: ridged noise, domain-warped, a thick main network + thinner branches
wx = fbm(3, 3, 100) - 0.5; wy = fbm(3, 3, 120) - 0.5
def ridged(base, seed, warp):
    n = fbm(base, 4, seed)
    return np.abs(n - 0.5)
# warp by resampling on torus
def warp_sample(field, ox, oy):
    ix = ((x + ox) * N).astype(int) % N; iy = ((y + oy) * N).astype(int) % N
    return field[iy, ix]
def isoline(field, width_px):
    gy_, gx_ = np.gradient(field)
    # periodic gradient fix at borders
    gx_ = (np.roll(field, -1, 1) - np.roll(field, 1, 1)) / 2
    gy_ = (np.roll(field, -1, 0) - np.roll(field, 1, 0)) / 2
    g = np.sqrt(gx_ ** 2 + gy_ ** 2) + 1e-6
    d = np.abs(field - 0.5) / g           # distance in px to the 0.5 isoline
    return np.clip(1 - d / width_px, 0, 1)
f1 = fbm(3, 3, 200, x + 0.18 * wx, y + 0.18 * wy)
f2 = fbm(5, 3, 300, x + 0.12 * wy, y + 0.12 * wx)
wmod = 0.55 + 0.9 * fbm(4, 2, 900)        # vein thickness swells and pinches
vein = np.clip(isoline(f1, 12 * wmod) * 1.6, 0, 1)    # thick main veins
vein2 = np.clip(isoline(f2, 4.5 * wmod) * 1.6, 0, 1) * 0.9
v1 = np.abs(f1 - 0.5) / 0.04
veinm = np.maximum(vein, vein2)

# --- nuggets: fat irregular blobs, scattered with wrap
nug = np.zeros((N, N))
for i in range(11):
    cx, cy = rng.random(), rng.random()
    rad = rng.uniform(0.03, 0.055)
    dx = (x - cx + 0.5) % 1 - 0.5; dy = (y - cy + 0.5) % 1 - 0.5
    ang = np.arctan2(dy, dx)
    ph = rng.random(4) * 6.28
    rr = rad * (1 + 0.16 * np.sin(2 * ang + ph[0]) + 0.10 * np.sin(3 * ang + ph[1]) + 0.07 * np.sin(5 * ang + ph[2]) + 0.04 * np.sin(7 * ang + ph[3]))
    d = np.sqrt(dx * dx + dy * dy) / rr
    nug = np.maximum(nug, np.clip((1 - d) * 6, 0, 1))
goldm = np.clip(np.maximum(veinm, nug), 0, 1)

# gold colour: rich, deep, with lumpy shading (hammered look) and a darker rim
lump = fbm(24, 3, 500)
gold_deep = np.array([0.30, 0.19, 0.05]); gold_mid = np.array([0.50, 0.35, 0.09]); gold_hi = np.array([0.62, 0.47, 0.17])
t = np.clip(lump * 1.6 - 0.3, 0, 1)[..., None]
gold = gold_deep + (gold_mid - gold_deep) * np.clip(t * 2, 0, 1) + (gold_hi - gold_mid) * np.clip(t * 2 - 1, 0, 1)
core = np.clip(goldm * 1.6 - 0.4, 0, 1)[..., None]   # darker edge, brighter core
gold = gold_deep * (1 - core) + gold * core
# dark halo in the rock around gold (quartz-free contact zone)
halo = np.clip(1 - v1 / 3, 0, 1)
img = img * (1 - 0.35 * halo[..., None] * (1 - goldm[..., None]))
a = np.clip(goldm * 2.2, 0, 1)[..., None]
img = img * (1 - a) + gold * a
Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))

# --- fx: sparse glints, mostly black; placed on gold where possible
fx = np.zeros((N, N))
gy, gx = np.nonzero(goldm > 0.8)
picks = rng.choice(len(gx), 20, replace=False)
for p in picks:
    cx, cy = gx[p] / N, gy[p] / N
    dx = (x - cx + 0.5) % 1 - 0.5; dy = (y - cy + 0.5) % 1 - 0.5
    s = rng.uniform(0.8, 1.3)
    d = np.sqrt(dx * dx + dy * dy)
    dot = np.exp(-(d / (0.011 * s)) ** 2) + 0.35 * np.exp(-(d / (0.024 * s)) ** 2)
    star = (np.exp(-(np.abs(dx) / (0.0045 * s))) * np.exp(-(np.abs(dy) / (0.04 * s)) ** 2)
            + np.exp(-(np.abs(dy) / (0.0045 * s))) * np.exp(-(np.abs(dx) / (0.04 * s)) ** 2))
    fx = np.maximum(fx, np.clip(dot + 1.0 * star, 0, 1))
fxc = np.array([1.0, 0.78, 0.35])
Image.fromarray((np.clip(fx[..., None] * fxc, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "fx.png"))
# tile check
p = Image.open(os.path.join(D, "paint.png")); t2 = Image.new("RGB", (1024, 1024))
for i in range(2):
    for j in range(2): t2.paste(p, (i * 512, j * 512))
t2.save(os.path.join(D, "tile_check.png"))

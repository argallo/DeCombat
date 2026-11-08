# Frostbite camo: procedural, tileable. Original art (numpy/Pillow only).
import sys, math, numpy as np
from PIL import Image, ImageDraw, ImageFilter
OUT = sys.argv[1]
N = 512
rng = np.random.default_rng(5241)

# --- faceted crystal: toroidal Voronoi with per-cell flat shading ---
K = 42
pts = rng.random((K, 2)) * N
yy, xx = np.mgrid[0:N, 0:N].astype(np.float32)
d1 = np.full((N, N), 1e9, np.float32); d2 = d1.copy(); idx = np.zeros((N, N), np.int32)
for k, (px, py) in enumerate(pts):
    dx = np.abs(xx - px); dx = np.minimum(dx, N - dx)
    dy = np.abs(yy - py); dy = np.minimum(dy, N - dy)
    d = np.sqrt(dx * dx + dy * dy)
    m = d < d1
    d2 = np.where(m, d1, np.minimum(d2, d)); idx = np.where(m, k, idx); d1 = np.where(m, d, d1)
edge = d2 - d1  # distance to cell border

# per-facet: a tilted plane gives a gradient across the facet (like a cut face catching light)
tone = rng.uniform(-1, 1, K); gx = rng.uniform(-1, 1, K); gy = rng.uniform(-1, 1, K)
cx = pts[idx, 0]; cy = pts[idx, 1]
ox = (xx - cx + N / 2) % N - N / 2; oy = (yy - cy + N / 2) % N - N / 2
facet = tone[idx] * 0.5 + (gx[idx] * ox + gy[idx] * oy) / 40.0
facet = np.clip(facet, -1, 1)

# palette: deep ice -> bright ice (all saturated blue, mid-tone)
deep = np.array([0.07, 0.20, 0.50]); mid = np.array([0.11, 0.32, 0.64]); hi = np.array([0.19, 0.44, 0.74])
t = (facet + 1) / 2
col = np.where(t[..., None] < 0.5, deep + (mid - deep) * (t[..., None] * 2), mid + (hi - mid) * ((t[..., None] - 0.5) * 2))
# a little cyan/teal hue variation per facet
hue = rng.uniform(-1, 1, K)[idx]
col[..., 1] += hue * 0.04; col[..., 0] -= hue * 0.03

# bright facet ridges (thin lighter line just inside borders) and dark seams at the border
ridge = np.exp(-((edge - 2.5) ** 2) / 1.5)
col = col + ridge[..., None] * np.array([0.08, 0.10, 0.08])

# --- cracks: deep blue, along some facet borders plus jagged random cracks ---
crack = Image.new('L', (N * 2, N * 2), 0)
cd = ImageDraw.Draw(crack)
S = 2
def wline(dr, a, b, w, v):
    for ox_ in (-N, 0, N):
        for oy_ in (-N, 0, N):
            dr.line([((a[0] + ox_) * S, (a[1] + oy_) * S), ((b[0] + ox_) * S, (b[1] + oy_) * S)], fill=v, width=w)
for _ in range(9):
    x, y = rng.random(2) * N
    ang = rng.uniform(0, 2 * math.pi)
    L = rng.uniform(120, 260)
    steps = int(L / 10)
    w = 6
    for s in range(steps):
        ang += rng.normal(0, 0.3)
        nx, ny = x + math.cos(ang) * 10, y + math.sin(ang) * 10
        wline(cd, (x % N, y % N), (x % N + (nx - x), y % N + (ny - y)), max(1, w), 255)
        if rng.random() < 0.18:  # side crack
            a2 = ang + rng.choice([-1, 1]) * rng.uniform(0.6, 1.2); bx, by = x, y
            for _ in range(rng.integers(2, 6)):
                a2 += rng.normal(0, 0.4)
                ex, ey = bx + math.cos(a2) * 8, by + math.sin(a2) * 8
                wline(cd, (bx % N, by % N), (bx % N + (ex - bx), by % N + (ey - by)), 2, 200)
                bx, by = ex, ey
        x, y = nx, ny
        if s > steps * 0.6: w = 4
crack = np.asarray(crack.resize((N, N), Image.LANCZOS), np.float32) / 255
seam = np.exp(-(edge ** 2) / 1.2) * (rng.random(K)[idx] > 0.35)
crk = np.clip(crack + seam * 0.8, 0, 1)
crackcol = np.array([0.03, 0.08, 0.30])
# halo of darker blue around cracks (ice fractured below the surface)
halo = np.asarray(Image.fromarray((crk * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(4)), np.float32) / 255
col = col * (1 - 0.35 * halo[..., None]) + np.array([0.0, 0.03, 0.10]) * halo[..., None]
col = col * (1 - crk[..., None]) + crackcol * crk[..., None]

# --- frost ferns: branching dendrites, pale icy blue (not white) ---
fern = Image.new('L', (N * S, N * S), 0)
fd = ImageDraw.Draw(fern)
def branch(x, y, ang, L, w, depth, curl=0.0):
    if depth == 0 or L < 3: return
    segs = 6
    sl = L / segs
    for i in range(segs):
        ang += curl + rng.normal(0, 0.05)
        nx, ny = x + math.cos(ang) * sl, y + math.sin(ang) * sl
        wline(fd, (x % N, y % N), (x % N + (nx - x), y % N + (ny - y)), max(1, int(w)), 255)
        # feathered side shoots on both sides, like frost on a window
        for side in (-1, 1):
            branch(nx, ny, ang + side * rng.uniform(0.9, 1.15), L * 0.32 * (1 - i / segs * 0.6), w * 0.65, depth - 1, curl * 1.5)
        x, y = nx, ny
        w = max(1, w * 0.88)
for _ in range(6):
    x, y = rng.random(2) * N
    base = rng.uniform(0, 2 * math.pi)
    for j in range(rng.integers(2, 4)):  # fronds fanning out from a seed point
        branch(x, y, base + j * rng.uniform(0.6, 1.3), rng.uniform(110, 170), 7, 3, rng.uniform(-0.12, 0.12))
fern = np.asarray(fern.resize((N, N), Image.LANCZOS), np.float32) / 255
fglow = np.asarray(Image.fromarray((fern * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(3)), np.float32) / 255
frostcol = np.array([0.38, 0.58, 0.84])
col = col + (np.array([0.32, 0.54, 0.82]) - col) * (fglow[..., None] * 0.5)
col = col * (1 - fern[..., None]) + frostcol * fern[..., None]

col = np.clip(col, 0, 1)
Image.fromarray((col * 255).astype(np.uint8)).save(OUT + '/paint.png')

# --- effect: sparse cold glints, mostly black ---
fx = np.zeros((N, N), np.float32)
for _ in range(16):
    gx_, gy_ = rng.random(2) * N
    r = rng.uniform(2.5, 4.5); arm = r * rng.uniform(3, 5)
    dx = (xx - gx_ + N / 2) % N - N / 2; dy = (yy - gy_ + N / 2) % N - N / 2
    core = np.exp(-(dx * dx + dy * dy) / (r * r))
    star = np.exp(-(dy * dy) / 0.8) * np.exp(-(dx * dx) / (arm * arm)) + np.exp(-(dx * dx) / 0.8) * np.exp(-(dy * dy) / (arm * arm))
    fx = np.maximum(fx, np.clip(core + 0.6 * star, 0, 1) * rng.uniform(0.6, 1.0))
fxc = fx[..., None] * np.array([0.55, 0.80, 1.0])
Image.fromarray((np.clip(fxc, 0, 1) * 255).astype(np.uint8)).save(OUT + '/fx.png')
print('ok')

# Crown Jewels camo for the L85A1: procedural, tileable 512x512 paint + fx.
import os, math
import numpy as np
from PIL import Image, ImageDraw, ImageFilter
A = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 2; M = N * SS
rng = np.random.default_rng(176)

def tnoise(freqs, amps, seed):
    r = np.random.default_rng(seed)
    y, x = np.mgrid[0:N, 0:N] / N
    out = np.zeros((N, N))
    for f, a in zip(freqs, amps):
        for _ in range(4):
            kx, ky = r.integers(-f, f + 1, 2)
            ph = r.uniform(0, 2 * np.pi)
            out += a * np.sin(2 * np.pi * (kx * x + ky * y) + ph)
    return out

# --- velvet base: deep purple, soft crushed sheen, fine pile
sheen = tnoise([2, 3, 5], [1.0, 0.6, 0.35], 1)
sheen = (sheen - sheen.min()) / (sheen.max() - sheen.min())
pile = rng.normal(0, 1, (N, N))
pile = np.array(Image.fromarray(((pile * 30) + 128).clip(0, 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(0.7)), float) / 255 - 0.5
v = 0.62 + 0.50 * sheen + 0.25 * pile
base = np.array([0.21, 0.045, 0.29])
vel = (base[None, None, :] * v[..., None]).clip(0, 1)

# --- gold layer, drawn supersampled with wrap
gold_mask = Image.new('L', (M, M), 0)
gem_masks = {k: Image.new('L', (M, M), 0) for k in ('ruby', 'sapphire', 'emerald')}
gm = ImageDraw.Draw(gold_mask)
gd = {k: ImageDraw.Draw(m) for k, m in gem_masks.items()}

def wrapped(fn):
    for ox in (-M, 0, M):
        for oy in (-M, 0, M):
            fn(ox, oy)

S = SS
gems = []
cols = [['ruby', 'sapphire'], ['emerald', 'ruby'], ['sapphire', 'emerald'], ['ruby', 'emerald']]
for j in range(4):
    for i in range(2):
        gems.append((i * 256 + 128 * (j % 2), j * 128, cols[j][i]))
R_GEM, R_BEZ = 38, 50

def line(pts, w):
    def f(ox, oy):
        gm.line([((x * S) + ox, (y * S) + oy) for x, y in pts], fill=255, width=int(w * S), joint='curve')
    wrapped(f)

def disc(cx, cy, r, d=gm, fill=255):
    def f(ox, oy):
        d.ellipse([cx * S - r * S + ox, cy * S - r * S + oy, cx * S + r * S + ox, cy * S + r * S + oy], fill=fill)
    wrapped(f)

def spiral(cx, cy, a0, r0, turns, sign, w):
    pts = []
    n = 60
    for k in range(n + 1):
        t = k / n
        ang = a0 + sign * t * turns * 2 * math.pi
        r = r0 * (1 - 0.8 * t)
        pts.append((cx + r * math.cos(ang), cy + r * math.sin(ang)))
    line(pts, w)
    disc(pts[-1][0], pts[-1][1], w * 0.9)

# curved trellis between diagonal neighbours (S-curves) with scroll curls at the midpoints
for (x, y, _) in gems:
    for dx, dy in ((128, 128), (-128, 128)):
        x1, y1 = x + dx, y + dy
        mx, my = (x + x1) / 2, (y + y1) / 2
        nx, ny = -dy / 181.0, dx / 181.0
        pts = []
        for k in range(41):
            t = k / 40
            off = 14 * math.sin(2 * math.pi * t)
            pts.append((x + dx * t + nx * off, y + dy * t + ny * off))
        line(pts, 7)
        # paired C-scrolls each side of the midpoint
        for sgn in (1, -1):
            cx, cy = mx + nx * 30 * sgn, my + ny * 30 * sgn
            ang0 = math.atan2(-ny * sgn, -nx * sgn)
            spiral(cx, cy, ang0, 24, 1.15, sgn, 6)
        disc(mx, my, 7)
# small curls between horizontally adjacent gems' gaps (centres of empty diamonds)
for j in range(4):
    for i in range(2):
        cx, cy = i * 256 + 128 * ((j + 1) % 2), j * 128
        for k in range(4):
            a = k * math.pi / 2 + math.pi / 4
            spiral(cx + 26 * math.cos(a), cy + 26 * math.sin(a), a + math.pi, 18, 1.0, 1 if k % 2 else -1, 5)
        disc(cx, cy, 9)

# bezels: ring + beads, then gems
for (x, y, c) in gems:
    disc(x, y, R_BEZ)
    for k in range(18):
        a = k * math.pi / 9
        disc(x + (R_BEZ + 1) * math.cos(a), y + (R_BEZ + 1) * math.sin(a), 4.5)
    disc(x, y, R_GEM, d=gd[c])

def down(img):
    return np.array(img.resize((N, N), Image.LANCZOS), float) / 255

gold_a = down(gold_mask)
gem_a = {k: down(m) for k, m in gem_masks.items()}

# gold shading: brushed metal + bevel from mask gradient
gb = np.array(Image.fromarray((gold_a * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(2.5)), float) / 255
gy, gx = np.gradient(gb)
bevel = (-gx - gy) * 6
gold_col = np.array([0.50, 0.34, 0.08])
gshade = (0.85 + bevel + 0.10 * tnoise([6], [1], 3) / 4)[..., None]
gold = (gold_col[None, None, :] * gshade).clip(0, 1)
img = vel * (1 - gold_a[..., None]) + gold * gold_a[..., None]

# gems: faceted look via radial facets around each centre
y, x = np.mgrid[0:N, 0:N].astype(float)
gemcol = {'ruby': np.array([0.62, 0.05, 0.12]), 'sapphire': np.array([0.08, 0.18, 0.66]),
          'emerald': np.array([0.04, 0.50, 0.22])}
facet = np.ones((N, N))
for (gx0, gy0, c) in gems:
    dxw = (x - gx0 + N / 2) % N - N / 2
    dyw = (y - gy0 + N / 2) % N - N / 2
    r = np.hypot(dxw, dyw); a = np.arctan2(dyw, dxw)
    inside = r < R_GEM + 2
    fac = 0.75 + 0.25 * np.cos(8 * a)  # 8 facets
    table = r < R_GEM * 0.45
    f = np.where(table, 1.10, fac * (1.05 - 0.45 * r / R_GEM))
    # highlight upper-left
    hl = np.exp(-((dxw + 12) ** 2 + (dyw + 12) ** 2) / 60.0) * 0.6
    facet = np.where(inside, f + hl, facet)
for k, col in gemcol.items():
    a = gem_a[k][..., None]
    gimg = (col[None, None, :] * facet[..., None]).clip(0, 1)
    img = img * (1 - a) + gimg * a

Image.fromarray((img.clip(0, 1) * 255).astype(np.uint8)).save(os.path.join(A, 'paint.png'))

# --- fx: sparse four-point star glints, mostly black
fx = np.zeros((N, N))
r2 = np.random.default_rng(7)
pts = [(gx0 - 14, gy0 - 14) for (gx0, gy0, _) in gems] + [tuple(r2.uniform(0, N, 2)) for _ in range(6)]
for (px, py) in pts:
    dxw = (x - px + N / 2) % N - N / 2
    dyw = (y - py + N / 2) % N - N / 2
    rr = np.hypot(dxw, dyw)
    core = np.exp(-rr ** 2 / 220.0)
    rays = (np.exp(-np.abs(dxw) / 3.5) + np.exp(-np.abs(dyw) / 3.5)) * np.exp(-rr / 40.0)
    fx = np.maximum(fx, core + 0.8 * rays)
fx = fx.clip(0, 1)
fxc = np.stack([fx, fx * 0.92, fx * 0.75], -1)
Image.fromarray((fxc * 255).astype(np.uint8)).save(os.path.join(A, 'fx.png'))

# tile check
p = Image.open(os.path.join(A, 'paint.png'))
t = Image.new('RGB', (2 * N, 2 * N))
for i in range(2):
    for j in range(2):
        t.paste(p, (i * N, j * N))
t.save(os.path.join(A, 'tile_check.png'))

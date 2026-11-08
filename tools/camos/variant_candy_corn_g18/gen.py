# Candy Corn camo for the G18: procedural, tileable 512x512.
import os, math
import numpy as np
from PIL import Image, ImageDraw
D = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 2; M = N * SS
rng = np.random.default_rng(1031)

GROUND = np.array([0.095, 0.035, 0.12])
GROUND2 = np.array([0.06, 0.022, 0.075])
YEL = np.array([0.52, 0.32, 0.02])
ORA = np.array([0.56, 0.115, 0.01])
CRM = np.array([0.45, 0.37, 0.255])
BAT = np.array([0.045, 0.025, 0.06])

yy, xx = np.mgrid[0:M, 0:M].astype(np.float32) / M  # 0..1 torus coords

# ground: purple-black with soft tileable mottling
g = np.zeros((M, M), np.float32)
for fx, fy, a in [(1, 2, 0.5), (3, 1, 0.3), (2, 3, 0.25), (5, 4, 0.12)]:
    ph = rng.uniform(0, 2 * np.pi)
    g += a * np.sin(2 * np.pi * (fx * xx + fy * yy) + ph)
g = (g - g.min()) / (g.max() - g.min())
img = GROUND2[None, None] * (1 - g[..., None]) + GROUND[None, None] * g[..., None]

# bats first (behind kernels): PIL mask, drawn at 9 wrap offsets
def bat_poly(cx, cy, s, rot):
    # wing outline in local units (span ~ 2 across), scalloped trailing edge
    pts = [(0, -0.28), (0.12, -0.40), (0.16, -0.22), (0.45, -0.38), (0.80, -0.42), (1.0, -0.20),
           (0.78, -0.02), (0.62, 0.12), (0.48, -0.02), (0.32, 0.10), (0.18, 0.0), (0.0, 0.30)]
    full = pts + [(-x, y) for x, y in reversed(pts[1:-1])]
    c, sn = math.cos(rot), math.sin(rot)
    return [(cx + s * (x * c - y * sn), cy + s * (x * sn + y * c)) for x, y in full]

bm = Image.new("L", (M, M), 0); bd = ImageDraw.Draw(bm)
rm = Image.new("L", (M, M), 0); rd = ImageDraw.Draw(rm)
bats = [(0.10, 0.22), (0.47, 0.60), (0.83, 0.12), (0.30, 0.88), (0.68, 0.38)]
for (bx, by) in bats:
    s = rng.uniform(0.062, 0.072) * M; rot = rng.uniform(-0.4, 0.4)
    for ox in (-M, 0, M):
        for oy in (-M, 0, M):
            bd.polygon(bat_poly(bx * M + ox, by * M + oy, s, rot), fill=255)
            rd.polygon(bat_poly(bx * M + ox, by * M + oy, s, rot), outline=255, width=5)
bmask = np.asarray(bm, np.float32)[..., None] / 255
img = img * (1 - bmask) + BAT[None, None] * bmask
rmask = np.asarray(rm, np.float32)[..., None] / 255
img = img * (1 - rmask) + np.array([0.30, 0.17, 0.40])[None, None] * rmask

# kernels: dart-throw on the torus
L = 0.19; Wd = 0.145  # length and base width as fraction of tile
cent = []
batc = np.array(bats)
tries = 0
while len(cent) < 15 and tries < 20000:
    tries += 1
    p = rng.uniform(0, 1, 2)
    def tdist(a, b):
        d = np.abs(a - b); d = np.minimum(d, 1 - d); return np.hypot(*d.T) if d.ndim > 1 else math.hypot(*d)
    if cent and np.min(tdist(np.array(cent), p)) < 0.205: continue
    if np.min(tdist(batc, p)) < 0.13: continue
    cent.append(p)
print("kernels", len(cent))

for (cx, cy) in cent:
    ang = rng.uniform(0, 2 * np.pi)
    sc = rng.uniform(0.9, 1.1)
    Lk, Wk = L * sc, Wd * sc
    dx = (xx - cx + 0.5) % 1 - 0.5; dy = (yy - cy + 0.5) % 1 - 0.5
    ca, sa = math.cos(ang), math.sin(ang)
    # local: v along kernel (base->tip), u across
    v = (dx * ca + dy * sa) / Lk + 0.45
    u = (-dx * sa + dy * ca) / (Wk / 2)
    vc = np.clip(v, 0, 1)
    hw = (1 - vc) ** 0.8 * 1.0
    hw = np.where(v < 0.1, np.sqrt(np.clip(1 - ((0.1 - v) / 0.16) ** 2, 0, 1)) * 1.0, hw)
    # rounded tip
    hw = np.minimum(hw, np.sqrt(np.clip(1 - ((v - 0.86) / 0.14).clip(0) ** 2, 0, 1)) * 0.32 + (v < 0.86) * 10)
    inside = (np.abs(u) < hw) & (v > -0.06) & (v < 1.0)
    if not inside.any(): continue
    un = np.clip(u / np.maximum(hw, 1e-3), -1, 1)
    col = np.where((v < 0.42)[..., None], YEL, np.where((v < 0.76)[..., None], ORA, CRM))
    # soft band blending
    t1 = np.clip((v - 0.40) / 0.04, 0, 1)[..., None]; t2 = np.clip((v - 0.74) / 0.04, 0, 1)[..., None]
    col = YEL * (1 - t1) + ORA * t1
    col = col * (1 - t2) + CRM * t2
    # rounded body shading + rim darkening
    shade = 0.72 + 0.33 * np.sqrt(np.clip(1 - un ** 2, 0, 1))
    rim = np.clip((1 - np.abs(un)) / 0.12, 0, 1) * np.clip((v + 0.06) / 0.05, 0, 1)
    shade = shade * (0.55 + 0.45 * rim)
    c = col * shade[..., None]
    # glossy sheen: a streak down one side and a spot near the base
    sheen = np.exp(-((un + 0.42) / 0.13) ** 2) * np.clip((v - 0.05) / 0.2, 0, 1) * np.clip((0.82 - v) / 0.2, 0, 1)
    spot = np.exp(-(((un + 0.25) / 0.2) ** 2 + ((v - 0.12) / 0.07) ** 2))
    gl = np.clip(0.55 * sheen + 0.45 * spot, 0, 1)[..., None]
    c = c * (1 - gl) + np.array([0.62, 0.56, 0.47]) * gl
    # thin dark outline via anti-aliased mask from distance to edge
    edge = (hw - np.abs(u)) * (Wk / 2) * M
    a = np.clip(edge / 1.5, 0, 1) * inside
    a = a[..., None]
    out_m = np.clip((hw * (Wk / 2) * M + 2.5 - np.abs(u) * (Wk / 2) * M) / 1.5, 0, 1) * ((v > -0.1) & (v < 1.02))
    img = img * (1 - 0.5 * out_m[..., None]) + 0 * out_m[..., None]
    img = img * (1 - a) + c * a

img = np.clip(img, 0, 1)
im = Image.fromarray((img * 255 + 0.5).astype(np.uint8)).resize((N, N), Image.LANCZOS)
im.save(os.path.join(D, "paint.png"))
# tile check
t = Image.new("RGB", (2 * N, 2 * N))
for i in range(2):
    for j in range(2): t.paste(im, (i * N, j * N))
t.save(os.path.join(D, "tile_check.png"))

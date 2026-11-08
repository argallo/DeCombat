# Koi Pond camo for the AUG A3: procedural, tileable (all motifs wrap on the torus).
import numpy as np
from PIL import Image
import sys, os
OUT = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 2; W = N * SS
rng = np.random.default_rng(int(sys.argv[1]) if len(sys.argv) > 1 else 7)
y, x = np.mgrid[0:W, 0:W].astype(np.float32) + 0.5
T = 2 * np.pi / W

def wrap(d):
    return (d + W / 2) % W - W / 2

# --- water: deep teal with tileable ripple bands and soft light caustics
def tor(fx, fy, ph):
    return np.sin(T * (fx * x + fy * y) + ph)
ripple = (0.5 * tor(3, 1, 0.3) + 0.35 * tor(-2, 4, 1.7) + 0.3 * tor(5, -3, 2.2)
          + 0.2 * tor(7, 6, 0.9) + 0.15 * tor(-9, 4, 4.0))
caus = np.abs(tor(11, 5, 0.0) + tor(-6, 12, 1.1) + tor(4, -13, 2.6))
caus = np.clip(1.0 - caus / 0.6, 0, 1) ** 2
deep = np.array([0.02, 0.15, 0.20]); shal = np.array([0.05, 0.27, 0.30])
t = np.clip(0.5 + 0.28 * ripple, 0, 1)[..., None]
img = deep * (1 - t) + shal * t
img = img + caus[..., None] * np.array([0.04, 0.08, 0.08])

# concentric ripple rings around a few points
for _ in range(5):
    cx, cy = rng.uniform(0, W, 2)
    r = np.hypot(wrap(x - cx), wrap(y - cy))
    R = rng.uniform(70, 130) * SS
    ring = (0.5 + 0.5 * np.cos(r / (9 * SS) * 2 * np.pi)) * np.clip(1 - r / R, 0, 1) ** 1.5
    img = img + ring[..., None] * np.array([0.03, 0.06, 0.06])

def blend(mask, col, a=1.0):
    global img
    m = np.clip(mask, 0, 1)[..., None] * a
    img = img * (1 - m) + np.asarray(col) * m

def smooth(d, w=1.2 * SS):  # signed distance -> coverage (inside < 0)
    return np.clip(0.5 - d / w, 0, 1)

# --- placement with spacing (toroidal), lily pads first
placed = []
def place(rad, tries=400):
    for _ in range(tries):
        c = rng.uniform(0, W, 2)
        if all(np.hypot(wrap(c[0] - p[0]), wrap(c[1] - p[1])) > rad + p[2] for p in placed):
            placed.append((c[0], c[1], rad)); return c
    return None

pads = []
for _ in range(4):
    R = rng.uniform(24, 32) * SS
    c = place(R + 6 * SS)
    if c is not None: pads.append((c, R))

fish = []
for i in range(15):
    L = rng.uniform(22, 29) * SS
    c = place(L * 0.75)
    if c is not None: fish.append((c, L, i))

# shadows under pads and fish (offset down-right), drawn before the motifs
for (c, R) in pads:
    d = np.hypot(wrap(x - c[0] - 5 * SS), wrap(y - c[1] - 6 * SS)) - R
    blend(np.clip(1 - (d + 4 * SS) / (12 * SS), 0, 1) * (d < 8 * SS), [0.01, 0.08, 0.11], 0.5)

def fish_local(c, ang):
    dx = wrap(x - c[0]); dy = wrap(y - c[1])
    ca, sa = np.cos(ang), np.sin(ang)
    return dx * ca + dy * sa, -dx * sa + dy * ca

fparams = []
for (c, L, i) in fish:
    ang = rng.uniform(0, 2 * np.pi)
    bend = rng.uniform(-0.6, 0.6)
    fparams.append((c, L, i, ang, bend))
    u, v = fish_local((c[0] + 4 * SS, c[1] + 5 * SS), ang)
    un = u / L; vb = v - bend * L * 0.35 * un ** 2
    wd = 0.26 * L * np.sqrt(np.clip(1 - ((un - 0.1) / 0.9) ** 2, 0, 1)) * (0.45 + 0.55 * np.clip(un + 0.8, 0, 1))
    blend(smooth(np.abs(vb) - wd - 2 * SS, 6 * SS) * (wd > 0), [0.01, 0.08, 0.11], 0.45)

# --- lily pads: green disc with a wedge notch, radial veins, a lighter rim
for (c, R) in pads:
    dx = wrap(x - c[0]); dy = wrap(y - c[1])
    r = np.hypot(dx, dy); a = np.arctan2(dy, dx)
    a0 = rng.uniform(-np.pi, np.pi)
    da = np.angle(np.exp(1j * (a - a0)))
    notch = (np.abs(da) < 0.28) & (r > R * 0.05)
    cover = smooth(r - R) * (~notch)
    veins = (np.abs(np.sin(a * 7 + 0.3)) < 0.08) & (r < R * 0.92) & (r > R * 0.12)
    shade = 0.85 + 0.15 * (r / R)
    col = np.array([0.19, 0.42, 0.13])[None, None, :] * shade[..., None]
    col = np.where(veins[..., None], np.array([0.30, 0.52, 0.19]), col)
    col = np.where((r > R - 3 * SS)[..., None], np.array([0.17, 0.38, 0.13]), col)
    m = np.clip(cover, 0, 1)[..., None]
    img = img * (1 - m) + col * m
    # a small pink-white bud on one pad
    if rng.random() < 0.6:
        bx, by = c[0] + R * 0.35 * np.cos(a0 + np.pi), c[1] + R * 0.35 * np.sin(a0 + np.pi)
        br = np.hypot(wrap(x - bx), wrap(y - by)); ba = np.arctan2(wrap(y - by), wrap(x - bx))
        petal = br - (9 + 4 * np.abs(np.cos(ba * 3))) * SS
        blend(smooth(petal), [0.78, 0.45, 0.56])
        blend(smooth(br - 3.5 * SS), [0.80, 0.66, 0.20])

# --- koi
ORANGE = [0.80, 0.34, 0.05]; WHITE = [0.66, 0.65, 0.62]; RED = [0.66, 0.08, 0.06]
GOLD = [0.85, 0.62, 0.15]; BLACK = [0.08, 0.08, 0.09]
for (c, L, i, ang, bend) in fparams:
    u, v = fish_local(c, ang)
    un = u / L
    vb = v - bend * L * 0.35 * un ** 2
    # body: rounded head at +1, tapering towards tail at -0.8
    prof = np.sqrt(np.clip(1 - ((un - 0.1) / 0.9) ** 2, 0, 1)) * (0.45 + 0.55 * np.clip(un + 0.8, 0, 1))
    wd = 0.29 * L * prof
    body = smooth(np.abs(vb) - wd) * np.clip(prof * 8, 0, 1)
    # tail fan behind -0.8
    tu = -(un + 0.78)
    tail_w = 0.08 * L + 0.55 * L * np.clip(tu, 0, 0.45)
    tail = smooth(np.abs(vb) - tail_w) * (tu > 0) * smooth((tu - 0.42) * L - 0.12 * L * np.abs(np.sin(vb / L * 6)))
    # pectoral fins
    fin = np.zeros_like(x)
    for s in (-1, 1):
        fu = un - 0.35; fv = s * vb - wd.max() * 0 - 0.22 * L
        fin = np.maximum(fin, smooth(np.hypot(fu * L * 1.4, fv - 0.06 * L) - 0.13 * L))
    kind = i % 3   # 0 orange kohaku-ish, 1 white with red (kohaku), 2 orange-and-white
    fincol = [0.80, 0.55, 0.42] if kind == 1 else [0.86, 0.52, 0.22]
    blend(fin * 0.85, fincol)
    blend(tail * 0.9, fincol)
    base = WHITE if kind == 1 else ORANGE
    blend(body, base)
    # patches: wrap-safe blobs in body coordinates
    pat = np.sin(un * 9 + i * 1.7) + 0.8 * np.sin(vb / L * 14 + i) + 0.6 * np.sin((un * 5 - vb / L * 8) + i * 2.3)
    if kind == 1:
        blend(body * smooth(-(pat - 0.4), 2.5), RED)
    elif kind == 2:
        blend(body * smooth(-(pat - 0.7), 2.5), WHITE)
    else:
        blend(body * smooth(-(pat - 1.2), 2.5), GOLD)
    # head spot (tancho) on some white fish
    if kind == 1 and i % 2 == 0:
        blend(smooth(np.hypot((un - 0.72) * L, vb) - 0.11 * L), RED)
    # dorsal highlight line and eyes
    blend(body * smooth(np.abs(vb) - 0.05 * L) * (un > -0.5) * (un < 0.7), [1, 1, 1], 0.18)
    for s in (-1, 1):
        blend(smooth(np.hypot((un - 0.84) * L, vb - s * 0.12 * L) - 1.6 * SS), BLACK)

img = np.clip(img, 0, 1)
im = Image.fromarray((img * 255).astype(np.uint8)).resize((N, N), Image.LANCZOS)
im.save(os.path.join(OUT, 'paint.png'))
print('ok', len(pads), 'pads', len(fish), 'fish')

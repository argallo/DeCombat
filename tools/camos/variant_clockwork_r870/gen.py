# Steampunk: brass and copper gears on dark oiled bronze. Procedural, tileable 512x512.
import os, numpy as np
from PIL import Image
d = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(870)
y, x = np.mgrid[0:N, 0:N].astype(np.float32)

def tnoise(octaves=5, base=2, seed=0):
    r = np.random.default_rng(seed); out = np.zeros((N, N), np.float32); amp = 1.0
    for o in range(octaves):
        f = base * (2 ** o)
        for _ in range(4):
            fx, fy = r.integers(-f, f + 1, 2)
            ph = r.uniform(0, 2 * np.pi)
            out += amp * np.cos(2 * np.pi * (fx * x + fy * y) / N + ph)
        amp *= 0.55
    out = (out - out.min()) / (out.max() - out.min()); return out

def wrap(a, c):
    return ((a - c + N / 2) % N) - N / 2

n1 = tnoise(5, 2, 1); n2 = tnoise(4, 3, 2); n3 = tnoise(3, 6, 3)
# background: dark oiled bronze with riveted plates (grid 128, offset rows)
bronze = np.array([0.16, 0.11, 0.075])
img = np.ones((N, N, 3), np.float32) * bronze
img *= (0.8 + 0.4 * n1)[..., None]
P = 128
row = (y // P).astype(int)
xs = (x + (row % 2) * P / 2) % N
px, py = xs % P, y % P
seam = (np.minimum(px, P - px) < 2.5) | (np.minimum(py, P - py) < 2.5)
img[seam] *= 0.45
# plate edge highlight
hl = ((px > 2.5) & (px < 5)) | ((py > 2.5) & (py < 5))
img[hl & ~seam] *= 1.35
# rivets on plates
for cxr in (12, P - 12):
    for cyr in (12, P - 12, P / 2):
        dxr = px - cxr; dyr = py - cyr; rr = np.sqrt(dxr ** 2 + dyr ** 2)
        m = rr < 6
        shade = np.clip(1.25 - (dxr + dyr) / 10, 0.6, 1.6)
        img[m] = (np.array([0.42, 0.30, 0.16]) * shade[m, None])
        img[(rr >= 6) & (rr < 7.5)] *= 0.5

brass = np.array([0.47, 0.31, 0.08])
copper = np.array([0.52, 0.21, 0.10])
darkbrass = np.array([0.36, 0.24, 0.07])
verd = np.array([0.22, 0.48, 0.40])

# gears placed in meshing chains: (parent, angle deg, R, teeth, colour, spokes)
import math
spec = [
    (None, (130, 140), 108, 18, brass, 5),
    (0, 40, 74, 13, copper, 4),
    (1, -35, 50, 9, darkbrass, 0),
    (1, 95, 58, 10, brass, 0),
    (0, 160, 46, 8, copper, 0),
    (0, -70, 42, 8, darkbrass, 0),
    (3, 20, 66, 12, copper, 4),
    (4, 110, 60, 11, brass, 0),
    (5, -110, 64, 12, brass, 5),
    (8, 170, 44, 8, copper, 0),
    (6, 30, 54, 10, darkbrass, 0),
]
gears = []
for i, (p, a, R, T, col, sp) in enumerate(spec):
    if p is None:
        cx, cy = a
    else:
        pcx, pcy, pR = gears[p][0], gears[p][1], gears[p][2]
        dist = (pR - pR * 0.07) + (R - R * 0.07)
        cx = (pcx + dist * math.cos(math.radians(a))) % N
        cy = (pcy + dist * math.sin(math.radians(a))) % N
    gears.append((cx, cy, R, T, col, sp, i * 0.37))
print([(round(g[0]),round(g[1]),g[2]) for g in gears])
for (cx, cy, R, T, col, spokes, rot) in gears:
    dx = wrap(x, cx); dy = wrap(y, cy)
    r = np.sqrt(dx ** 2 + dy ** 2); th = np.arctan2(dy, dx) + rot
    tooth = R * 0.14
    w = np.cos(T * th)
    prof = np.clip(w * 2.2, -1, 1) * 0.5 + 0.5          # trapezoid teeth
    rout = R - tooth + tooth * prof
    body = r < rout
    # drop shadow
    sh = (np.sqrt((dx - 6) ** 2 + (dy - 7) ** 2) < rout + 2) & ~body
    img[sh] *= 0.55
    # rim and spokes or plate with holes
    rim_in = R * 0.70
    hub = R * 0.30
    if spokes:
        ang = (th * spokes / (2 * np.pi)) % 1.0
        sp = np.abs(ang - 0.5) * 2 * np.pi * r / spokes   # distance along arc to spoke centre
        spoke = (np.abs(0.5 - ang) > 0.5 - (R * 0.09) / np.maximum(r * 2 * np.pi / spokes, 1))
        open_ = (r > hub) & (r < rim_in) & ~spoke
    else:
        # plate with round lightening holes
        nh = 5
        hr = (rim_in + hub) / 2
        open_ = np.zeros_like(body)
        for k in range(nh):
            a = 2 * np.pi * k / nh - rot
            hx, hy = hr * np.cos(a), hr * np.sin(a)
            open_ |= np.sqrt((dx - hx) ** 2 + (dy - hy) ** 2) < (rim_in - hub) * 0.38
    axle = r < R * 0.09
    solid = body & ~open_ & ~axle
    # shading: directional bevel on edges + brushed radial tone
    light = 1.0 + 0.18 * (-(dx + dy) / (r + 1e-3)) * np.clip((r / R) - 0.6, 0, 1) * 2.5
    edge = np.abs(r - rout) < 2.2
    inner = (np.abs(r - rim_in) < 2.0) & solid
    tone = 0.85 + 0.3 * n3
    c = col[None, None, :] * (light * tone)[..., None]
    img[solid] = c[solid]
    img[edge & body] *= 0.6
    img[inner] *= 0.65
    # hub ring
    hubm = (r < hub) & solid
    img[hubm] = (col * 1.15 * (1 + 0.15 * (-(dx + dy) / (r + 1e-3))[..., None]))[hubm]
    img[(np.abs(r - hub) < 1.8) & solid] *= 0.6
    # hub rivets
    for k in range(4):
        a = 2 * np.pi * k / 4 + rot + 0.4
        rx, ry = hub * 0.6 * np.cos(a), hub * 0.6 * np.sin(a)
        rm = np.sqrt((dx - rx) ** 2 + (dy - ry) ** 2) < max(2.5, R * 0.045)
        img[rm] = col * 0.55
    img[axle] = bronze * 0.4
    # verdigris in recesses: near the inner edges and the hub seam, noise-gated
    rec = solid & ((np.abs(r - rim_in) < R * 0.08) | (np.abs(r - hub) < R * 0.06) | (np.abs(r - rout) < R * 0.06))
    vm = rec & (n2 > 0.45)
    img[vm] = img[vm] * 0.45 + verd * 0.55

# overall patina speckle on background bronze
nf = tnoise(3, 12, 9)
bgv = (nf > 0.80) & (n2 > 0.6) & (img.sum(-1) < 0.6)
img[bgv] = img[bgv] * 0.5 + verd * 0.5 * 0.8
img = np.clip(img, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(d, "paint.png"))
t = Image.open(os.path.join(d, "paint.png")); tc = Image.new("RGB", (N * 2, N * 2))
for i in range(2):
    for j in range(2): tc.paste(t, (i * N, j * N))
tc.save(os.path.join(d, "tile_check.png"))

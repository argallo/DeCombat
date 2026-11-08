# Comic Pop camo for the MP5A4: pop-art comic print, procedurally drawn, tileable 512x512.
# Ben-Day halftone dots in yellow / red / cyan, thick black ink outlines, spiky burst shapes. No text.
import math, random, sys, os
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

N = 512
SS = 3                     # supersample
W = N * SS
OUT = os.path.dirname(os.path.abspath(__file__))
rng = random.Random(5160)

def c(r, g, b):
    return np.array([r, g, b], dtype=np.float32)

INK    = c(0.04, 0.03, 0.05)
PAPER  = c(0.50, 0.45, 0.28)   # warm newsprint, mid-tone
YEL    = c(0.70, 0.54, 0.03)
YEL_D  = c(0.62, 0.38, 0.02)
RED    = c(0.64, 0.06, 0.08)
RED_D  = c(0.45, 0.03, 0.08)
CYAN   = c(0.02, 0.40, 0.62)
CYAN_L = c(0.22, 0.54, 0.64)

# ---------- shapes ----------
def star(cx, cy, r_out, r_in, n, rot, jitter):
    pts = []
    for i in range(2 * n):
        a = rot + math.pi * i / n
        r = r_out if i % 2 == 0 else r_in
        r *= 1.0 + rng.uniform(-jitter, jitter)
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return pts

def scale_pts(pts, cx, cy, k):
    return [(cx + (x - cx) * k, cy + (y - cy) * k) for x, y in pts]

# label canvas (L): 0 background, 1 big outer, 2 big inner, 3 small burst, 4 cyan blob, 255 ink
lab = Image.new("L", (W, W), 0)
d = ImageDraw.Draw(lab)
INK_W = 9 * SS

def poly_wrapped(pts, fill, outline_w=0):
    for ox in (-W, 0, W):
        for oy in (-W, 0, W):
            p = [(x * SS + ox, y * SS + oy) for x, y in pts]
            if outline_w:
                d.line(p + [p[0], p[1]], fill=255, width=outline_w, joint="curve")
            else:
                d.polygon(p, fill=fill)

def ellipse_wrapped(cx, cy, r, fill, outline_w=0):
    for ox in (-W, 0, W):
        for oy in (-W, 0, W):
            b = [(cx - r) * SS + ox, (cy - r) * SS + oy, (cx + r) * SS + ox, (cy + r) * SS + oy]
            if outline_w:
                d.ellipse(b, outline=255, width=outline_w)
            else:
                d.ellipse(b, fill=fill)

shapes = []
# big bursts placed on a jittered 2x2 lattice so they spread evenly over the tile
for gx in range(2):
    for gy in range(2):
        cx = (gx + 0.5) * N / 2 + rng.uniform(-40, 40) + (gy * 64)
        cy = (gy + 0.5) * N / 2 + rng.uniform(-40, 40)
        R = rng.uniform(92, 112)
        n = rng.choice([11, 12, 13, 14])
        rot = rng.uniform(0, math.tau)
        outer = star(cx, cy, R, R * 0.62, n, rot, 0.16)
        inner = star(cx, cy, R * 0.55, R * 0.36, n, rot + math.pi / n, 0.12)
        shapes.append(("big", cx % N, cy % N, outer, inner))

# small bursts and dots in the gaps
smalls = []
tries = 0
while len(smalls) < 9 and tries < 4000:
    tries += 1
    cx, cy = rng.uniform(0, N), rng.uniform(0, N)
    ok = True
    for s in shapes:
        dx = min(abs(cx - s[1]), N - abs(cx - s[1])); dy = min(abs(cy - s[2]), N - abs(cy - s[2]))
        if math.hypot(dx, dy) < 128: ok = False
    for s in smalls:
        dx = min(abs(cx - s[0]), N - abs(cx - s[0])); dy = min(abs(cy - s[1]), N - abs(cy - s[1]))
        if math.hypot(dx, dy) < 70: ok = False
    if ok:
        smalls.append((cx, cy, rng.uniform(26, 40), rng.choice(["burst", "cburst", "blob"])))

# draw: fills first, then ink outlines on top of each shape in order (later shapes overlap earlier)
for cx, cy, r, kind in smalls:
    if kind == "burst":
        pts = star(cx, cy, r, r * 0.55, rng.choice([7, 8, 9]), rng.uniform(0, 6.3), 0.18)
        poly_wrapped(pts, 3); poly_wrapped(pts, 0, INK_W - 2 * SS)
    elif kind == "cburst":
        pts = star(cx, cy, r, r * 0.6, rng.choice([8, 9, 10]), rng.uniform(0, 6.3), 0.15)
        poly_wrapped(pts, 4); poly_wrapped(pts, 0, INK_W - 2 * SS)
    else:
        ellipse_wrapped(cx, cy, r * 0.75, 4); ellipse_wrapped(cx, cy, r * 0.75, 0, INK_W - 2 * SS)

for _, cx, cy, outer, inner in shapes:
    poly_wrapped(outer, 1)
    poly_wrapped(outer, 0, INK_W)
    poly_wrapped(inner, 2)
    poly_wrapped(inner, 0, INK_W - 2 * SS)

L = np.array(lab.resize((N, N), Image.NEAREST))  # label map
inkA = np.array(lab.point(lambda v: 255 if v == 255 else 0).resize((N, N), Image.LANCZOS), dtype=np.float32) / 255.0

# ---------- halftone (torus-periodic) ----------
y, x = np.mgrid[0:N, 0:N].astype(np.float32)

def halftone(freq, radius_field):
    # rotated 45 deg dot grid, periodic on the 512 torus because freq is a whole number
    a = 2 * np.pi * freq / N
    u = (x + y) * a / 2
    v = (x - y) * a / 2
    # distance to nearest dot centre in cell units
    fu = (u / np.pi) % 1.0 - 0.5
    fv = (v / np.pi) % 1.0 - 0.5
    dist = np.sqrt(fu * fu + fv * fv)
    # antialiased disc
    return np.clip((radius_field - dist) * 18 + 0.5, 0, 1)

def torus_wave(fx, fy, ph):
    return np.cos(2 * np.pi * (fx * x + fy * y) / N + ph)

shade = 0.5 + 0.25 * torus_wave(1, 2, 0.3) + 0.25 * torus_wave(-2, 1, 1.7)   # 0..1 slow field

img = np.zeros((N, N, 3), np.float32)

def put(mask, base, dot, dots):
    m = mask[..., None]
    col = base * (1 - dots[..., None]) + dot * dots[..., None]
    return img * (1 - m) + col * m

bg = (L == 0).astype(np.float32)
img = put(bg, PAPER, CYAN, halftone(28, 0.30 + 0.16 * shade))

# big burst outer: red with deeper red dots fading to the tips (radial)
for _, cx, cy, outer, inner in shapes:
    pass
m1 = (L == 1).astype(np.float32)
img = put(m1, RED, YEL, halftone(40, 0.10 + 0.18 * (1 - shade)))
m2 = (L == 2).astype(np.float32)
img = put(m2, YEL, RED, halftone(48, 0.08 + 0.16 * shade))
m3 = (L == 3).astype(np.float32)
img = put(m3, YEL, YEL_D, halftone(48, 0.20))
m4 = (L == 4).astype(np.float32)
img = put(m4, CYAN, CYAN_L, halftone(40, 0.22))

# ink on top, antialiased
img = img * (1 - inkA[..., None]) + INK * inkA[..., None]

out = Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8), "RGB")
out.save(os.path.join(OUT, "paint.png"))
# tiled preview to check seams
prev = Image.new("RGB", (N * 2, N * 2))
for i in range(2):
    for j in range(2):
        prev.paste(out, (i * N, j * N))
prev.save(os.path.join(OUT, "tile_preview.png"))
print("ok")

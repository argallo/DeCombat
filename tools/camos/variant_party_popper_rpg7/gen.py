# Party Popper camo for the RPG-7: procedural, original art (numpy + Pillow only).
import numpy as np, sys, os
from PIL import Image, ImageDraw
OUT = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(525)
y, x = np.mgrid[0:N, 0:N].astype(np.float32) / N

# Candy-cane diagonal stripes: integer frequency on the torus -> tileable.
K = 4                                  # stripe pairs across the tile
t = ((x + y) * K) % 1.0                # 0..1 within one stripe pair
band = np.floor((x + y) * K).astype(int) % 5
party = np.array([
    [0.62, 0.08, 0.36],   # hot pink
    [0.05, 0.46, 0.52],   # turquoise
    [0.64, 0.50, 0.04],   # party yellow
    [0.36, 0.12, 0.60],   # purple
    [0.68, 0.26, 0.05],   # orange
], np.float32)
cream = np.array([0.42, 0.34, 0.42], np.float32)
img = np.empty((N, N, 3), np.float32)
col = party[band]
# soft edge between colour stripe and cream stripe
edge = np.clip((np.abs(t - 0.5) - 0.20) / 0.02, 0, 1)[..., None]   # 1 = colour
img[:] = col * edge + cream * (1 - edge)
# thin dark-ish pinstripe in the cream band, like a candy cane
pin = np.exp(-((t - 0.0) ** 2 + 0) / 0.0004) + np.exp(-((t - 1.0) ** 2) / 0.0004)
img = img * (1 - 0.35 * pin[..., None])

pil = Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8))
# Draw on a 3x3 canvas and crop the centre so everything wraps.
big = Image.new("RGB", (N * 3, N * 3))
for i in range(3):
    for j in range(3):
        big.paste(pil, (i * N, j * N))
d = ImageDraw.Draw(big)
bright = [tuple(int(c * 255) for c in p) for p in
          [[0.70, 0.10, 0.40], [0.08, 0.55, 0.60], [0.72, 0.58, 0.05], [0.42, 0.15, 0.70],
           [0.75, 0.30, 0.06], [0.18, 0.58, 0.18], [0.60, 0.58, 0.62]]]

def wrapped(fn):
    for ox in (0, N, 2 * N):
        for oy in (0, N, 2 * N):
            fn(ox, oy)

# Curly streamers: sinusoid paths with loops (trochoid), wrapped.
for s in range(5):
    cx, cy = rng.uniform(0, N, 2)
    ang = rng.uniform(0, 2 * np.pi)
    c = bright[s % len(bright)]
    L = rng.uniform(220, 320)
    tt = np.linspace(0, 1, 400)
    r = 17
    loops = rng.integers(4, 7)
    px = tt * L + r * np.cos(tt * loops * 2 * np.pi) * 1.4
    py = 18 * np.sin(tt * np.pi * 1.3) + r * np.sin(tt * loops * 2 * np.pi)
    ca, sa = np.cos(ang), np.sin(ang)
    X = cx + px * ca - py * sa
    Y = cy + px * sa + py * ca
    pts = list(zip(X, Y))
    def draw(ox, oy, pts=pts, c=c):
        q = [(a + ox, b + oy) for a, b in pts]
        d.line(q, fill=(40, 30, 50), width=11, joint="curve")
        d.line(q, fill=c, width=7, joint="curve")
    wrapped(draw)

# Confetti flakes: small rotated rectangles, dots and triangles.
for k in range(80):
    cx, cy = rng.uniform(0, N, 2)
    c = bright[rng.integers(len(bright))]
    kind = rng.integers(3)
    a = rng.uniform(0, np.pi)
    if kind == 0:
        w, h = rng.uniform(9, 13), rng.uniform(18, 26)
        corners = np.array([[-w, -h], [w, -h], [w, h], [-w, h]]) / 2
    elif kind == 1:
        s_ = rng.uniform(12, 17)
        corners = np.array([[0, -s_], [s_ * 0.87, s_ * 0.5], [-s_ * 0.87, s_ * 0.5]]) * 0.8
    else:
        r_ = rng.uniform(6.5, 9)
        def draw(ox, oy, cx=cx, cy=cy, r_=r_, c=c):
            d.ellipse([cx + ox - r_ - 1.5, cy + oy - r_ - 1.5, cx + ox + r_ + 1.5, cy + oy + r_ + 1.5], fill=(40, 30, 50))
            d.ellipse([cx + ox - r_, cy + oy - r_, cx + ox + r_, cy + oy + r_], fill=c)
        wrapped(draw); continue
    R = np.array([[np.cos(a), -np.sin(a)], [np.sin(a), np.cos(a)]])
    p = corners @ R.T
    def draw(ox, oy, p=p, cx=cx, cy=cy, c=c):
        pp = [(cx + ox + u, cy + oy + v) for u, v in p]
        o = [(cx + ox + u * 1.35, cy + oy + v * 1.35) for u, v in p]
        d.polygon(o, fill=(40, 30, 50))
        d.polygon(pp, fill=c)
    wrapped(draw)

big.crop((N, N, 2 * N, 2 * N)).save(os.path.join(OUT, "paint.png"))

# Effect: mostly black, sparse bright confetti specks (drifts downward).
fx = Image.new("RGB", (N * 3, N * 3))
fd = ImageDraw.Draw(fx)
for k in range(40):
    cx, cy = rng.uniform(0, N, 2)
    c = bright[rng.integers(6)]
    a = rng.uniform(0, np.pi)
    w, h = rng.uniform(6, 8), rng.uniform(11, 15)
    R = np.array([[np.cos(a), -np.sin(a)], [np.sin(a), np.cos(a)]])
    p = (np.array([[-w, -h], [w, -h], [w, h], [-w, h]]) / 2) @ R.T
    for ox in (0, N, 2 * N):
        for oy in (0, N, 2 * N):
            fd.polygon([(cx + ox + u, cy + oy + v) for u, v in p], fill=c)
fx.crop((N, N, 2 * N, 2 * N)).save(os.path.join(OUT, "fx.png"))
print("ok")

# Grand Finale camo for the LAW: procedural fireworks over a navy night sky. Original art, numpy + Pillow only.
import os, math
import numpy as np
from PIL import Image, ImageDraw, ImageFilter
R = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(1964)

# --- sky: deep navy with soft tileable variation (integer frequencies on the torus)
y, x = np.mgrid[0:N, 0:N] / N
v = (0.5 + 0.25 * np.sin(2 * np.pi * (x + 2 * y) + 0.7) + 0.25 * np.sin(2 * np.pi * (3 * x - y) + 2.1))
navy_a = np.array([0.02, 0.035, 0.13]); navy_b = np.array([0.045, 0.07, 0.22])
img = navy_a + (navy_b - navy_a) * v[..., None]

PAL = {
    "gold":    np.array([0.78, 0.52, 0.06]),
    "magenta": np.array([0.72, 0.10, 0.56]),
    "cyan":    np.array([0.06, 0.56, 0.74]),
    "green":   np.array([0.22, 0.68, 0.12]),
}

def wrap_draw(fn):
    """Draw onto a 3N canvas at all 9 offsets and fold back, so every shape wraps cleanly."""
    big = Image.new("L", (3 * N, 3 * N), 0)
    d = ImageDraw.Draw(big)
    for ox in (0, N, 2 * N):
        for oy in (0, N, 2 * N):
            fn(d, ox, oy)
    a = np.asarray(big, np.float32) / 255.0
    out = np.zeros((N, N), np.float32)
    for i in range(3):
        for j in range(3):
            out = np.maximum(out, a[i * N:(i + 1) * N, j * N:(j + 1) * N])
    return out

def blur_wrap(m, r):
    t = np.tile(m, (3, 3))
    im = Image.fromarray((np.clip(t, 0, 1) * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(r))
    return np.asarray(im, np.float32)[N:2 * N, N:2 * N] / 255.0

def burst(cx, cy, rad, kind, rays, rot):
    def fn(d, ox, oy):
        X, Y = cx + ox, cy + oy
        for k in range(rays):
            a = rot + 2 * math.pi * k / rays
            pts = []
            steps = 18
            for s in range(steps + 1):
                t = 0.18 + 0.82 * s / steps
                px = X + math.cos(a) * t * rad
                py = Y + math.sin(a) * t * rad
                if kind == "willow":
                    py += 0.55 * rad * t * t      # trails droop like a willow
                pts.append((px, py))
            # trail thickens toward the tip
            for s in range(len(pts) - 1):
                w = 2 + int(5 * s / steps)
                d.line([pts[s], pts[s + 1]], fill=int(110 + 145 * s / steps), width=w)
            tx, ty = pts[-1]
            r = 7 if kind == "peony" else 6
            d.ellipse([tx - r, ty - r, tx + r, ty + r], fill=255)
        d.ellipse([X - 6, Y - 6, X + 6, Y + 6], fill=200)
    return fn

# bursts: (x, y, radius, colour, kind) laid out by hand for a balanced, crop-proof scatter
B = [
    (70, 80, 92, "gold", "peony"),
    (300, 60, 70, "cyan", "willow"),
    (440, 200, 88, "magenta", "peony"),
    (180, 250, 78, "green", "willow"),
    (60, 400, 74, "magenta", "willow"),
    (320, 380, 96, "gold", "willow"),
    (470, 470, 60, "cyan", "peony"),
    (200, 470, 52, "green", "peony"),
    (380, 150, 40, "gold", "peony"),
    (110, 170, 38, "cyan", "peony"),
]
for (cx, cy, rad, col, kind) in B:
    rays = 18 if rad > 60 else 12
    m = wrap_draw(burst(cx, cy, rad, kind, rays, rng.uniform(0, 1)))
    halo = blur_wrap(m, 6) * 0.55
    core = blur_wrap(m, 1.0)
    a = np.clip(core + halo, 0, 1)[..., None]
    c = PAL[col]
    img = img * (1 - a) + c * a
    # tiny hot centre at the tips, still mid-tone
    tip = np.clip(core - 0.85, 0, 1)[..., None] / 0.15
    img = img + (np.minimum(c + 0.18, 0.86) - img) * tip * 0.5

# faint background stars in the paint
stars = np.zeros((N, N), np.float32)
for _ in range(70):
    sx, sy = rng.integers(0, N, 2); stars[sy, sx] = 1.0
stars = blur_wrap(stars, 0.8) * 2.5
img = img + np.clip(stars, 0, 1)[..., None] * (np.array([0.45, 0.45, 0.55]) - img) * 0.6

Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8)).save(os.path.join(R, "paint.png"))

# --- effect: mostly black, sparse twinkling four-point sparkles
fx = np.zeros((N, N, 3), np.float32)
SP = [np.array([1.0, 0.82, 0.40]), np.array([1.0, 0.55, 0.9]), np.array([0.5, 0.9, 1.0]), np.array([0.65, 1.0, 0.5]),
      np.array([1.0, 1.0, 0.9])]
for i in range(22):
    sx, sy = rng.uniform(0, N, 2)
    size = rng.choice([12, 16, 20])
    def fn(d, ox, oy, sx=sx, sy=sy, size=size):
        X, Y = sx + ox, sy + oy
        d.polygon([(X - size, Y), (X - 2, Y - 2), (X, Y - size), (X + 2, Y - 2), (X + size, Y), (X + 2, Y + 2),
                   (X, Y + size), (X - 2, Y + 2)], fill=255)
        d.ellipse([X - 3, Y - 3, X + 3, Y + 3], fill=255)
    m = wrap_draw(fn)
    m = np.clip(blur_wrap(m, 1.0) + blur_wrap(m, 4) * 0.5, 0, 1)
    fx = np.maximum(fx, m[..., None] * SP[i % len(SP)])
Image.fromarray((np.clip(fx, 0, 1) * 255).astype(np.uint8)).save(os.path.join(R, "fx.png"))

# tiling check: 2x2
p = Image.open(os.path.join(R, "paint.png"))
t = Image.new("RGB", (2 * N, 2 * N))
for i in range(2):
    for j in range(2):
        t.paste(p, (i * N, j * N))
t.save(os.path.join(R, "tile_check.png"))

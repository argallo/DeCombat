# Kitty Chaos: paw prints, yarn balls with trailing strands, cat faces on lavender. Procedural, tileable.
import os, math
import numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 2; M = N * SS
rng = np.random.default_rng(7)

def C(r, g, b): return np.array([r, g, b], float) / 255.0
GROUND = C(86, 58, 140)
PINK = C(200, 34, 128); PINK_D = C(120, 14, 74)
TEAL = C(18, 138, 140); TEAL_D = C(6, 74, 80)
PURP = C(124, 40, 180); PURP_D = C(52, 14, 88)
VIO = C(150, 56, 196)
CAT = C(46, 16, 72); EYE = C(40, 170, 150)

yy, xx = np.mgrid[0:M, 0:M].astype(float) / SS + 0.5 / SS
img = np.ones((M, M, 3)) * GROUND

def wrapd(cx, cy):
    dx = (xx - cx + N / 2) % N - N / 2
    dy = (yy - cy + N / 2) % N - N / 2
    return dx, dy

def put(mask, col):
    img[mask] = col

def rot(dx, dy, a):
    c, s = math.cos(a), math.sin(a)
    return c * dx + s * dy, -s * dx + c * dy

def ellipse(dx, dy, rx, ry):
    return (dx / rx) ** 2 + (dy / ry) ** 2 <= 1

# subtle ground texture: faint diagonal "fur" bands (whole-number freqs)
band = 0.5 + 0.5 * np.sin(2 * math.pi * (3 * xx + 5 * yy) / N + 1.3 * np.sin(2 * math.pi * 2 * xx / N))
img *= (0.94 + 0.08 * band)[..., None]

def strand(points, col, w):
    # dense samples along the polyline; each paints a small wrapped window only
    R = int(math.ceil(w * SS)) + 1
    off = np.arange(-R, R + 1)
    for (x0, y0), (x1, y1) in zip(points[:-1], points[1:]):
        L = math.hypot(x1 - x0, y1 - y0); steps = max(2, int(L * 1.5))
        for t in np.linspace(0, 1, steps):
            px, py = (x0 + (x1 - x0) * t) * SS, (y0 + (y1 - y0) * t) * SS
            ix = (int(px) + off) % M; iy = (int(py) + off) % M
            ddx = (int(px) + off + 0.5 - px)[None, :]; ddy = (int(py) + off + 0.5 - py)[:, None]
            m = ddx * ddx + ddy * ddy < (w * SS) ** 2
            sub = img[np.ix_(iy, ix)]
            sub[m] = col
            img[np.ix_(iy, ix)] = sub

def wavy(x, y, ang, length, amp, k, seg=60):
    pts = []
    for i in range(seg + 1):
        s = length * i / seg
        off = amp * math.sin(k * s / length * 2 * math.pi)
        pts.append((x + math.cos(ang) * s - math.sin(ang) * off, y + math.sin(ang) * s + math.cos(ang) * off))
    return pts

def yarn(cx, cy, r, col, dark, tail_ang):
    # trailing strand first (under the ball)
    pts = wavy(cx, cy, tail_ang, r * 4.2, r * 0.45, 1.6)
    strand(pts, dark, 5.5)
    strand(pts, col, 3.2)
    dx, dy = wrapd(cx, cy)
    inside = dx * dx + dy * dy <= r * r
    put(inside, col)
    # wraps: several families of curved bands
    rr = np.sqrt(dx * dx + dy * dy) / r
    lines = np.zeros_like(dx, bool)
    for a, ph in [(0.3, 0.0), (1.4, 1.1), (2.4, 2.2)]:
        u, v = rot(dx, dy, a)
        f = np.sin((u / r) * 7.0 + 2.2 * (v / r) ** 2 + ph)
        lines |= np.abs(f) < 0.28
    put(inside & lines & (rr < 0.97), dark)
    ring = inside & (rr > 0.9)
    put(ring, dark)

def paw(cx, cy, s, a, col):
    dx, dy = wrapd(cx, cy)
    u, v = rot(dx, dy, a)
    m = ellipse(u, v - 0.25 * s, 0.62 * s, 0.5 * s)
    # heart-ish main pad: add two lobes
    m |= ellipse(u - 0.3 * s, v - 0.1 * s, 0.38 * s, 0.36 * s)
    m |= ellipse(u + 0.3 * s, v - 0.1 * s, 0.38 * s, 0.36 * s)
    for tx, ty in [(-0.78, -0.45), (-0.3, -0.92), (0.3, -0.92), (0.78, -0.45)]:
        m |= ellipse(u - tx * s, v - ty * s, 0.24 * s, 0.3 * s)
    put(m, col)

def cat(cx, cy, r, a, col, eye):
    dx, dy = wrapd(cx, cy)
    u, v = rot(dx, dy, a)
    head = ellipse(u, v, 1.1 * r, 0.92 * r)
    # ears: triangles
    def tri(px, py, p):
        (x1, y1), (x2, y2), (x3, y3) = p
        def side(ax, ay, bx, by): return (px - bx) * (ay - by) - (ax - bx) * (py - by)
        d1, d2, d3 = side(x1, y1, x2, y2), side(x2, y2, x3, y3), side(x3, y3, x1, y1)
        neg = (d1 < 0) | (d2 < 0) | (d3 < 0); pos = (d1 > 0) | (d2 > 0) | (d3 > 0)
        return ~(neg & pos)
    ears = tri(u, v, [(-1.0 * r, -0.3 * r), (-0.85 * r, -1.45 * r), (-0.2 * r, -0.75 * r)])
    ears |= tri(u, v, [(1.0 * r, -0.3 * r), (0.85 * r, -1.45 * r), (0.2 * r, -0.75 * r)])
    put(head | ears, col)
    # eyes: almond, with slit pupil
    for ex in (-0.42, 0.42):
        e = ellipse(u - ex * r, v + 0.02 * r, 0.24 * r, 0.17 * r)
        put(e, eye)
        put(e & ellipse(u - ex * r, v + 0.02 * r, 0.06 * r, 0.17 * r), col)
    # nose
    put(ellipse(u, v - 0.3 * r, 0.11 * r, 0.08 * r), PINK)
    # whiskers
    for sgn in (-1, 1):
        for k, (dyw, sl) in enumerate([(0.38, -0.12), (0.5, 0.0), (0.62, 0.12)]):
            x0 = sgn * 0.3 * r; x1 = sgn * 1.55 * r
            t = (u - x0) / (x1 - x0)
            yline = (dyw + sl * t) * r
            put((t > 0) & (t < 1) & (np.abs(v - yline) < 2.2), col)

# tone-on-tone small paws in the ground, so even a magnified slice still says "cat"
for gi in range(8):
    for gj in range(8):
        cx = (gi + 0.5 + 0.25 * (gj % 2)) * N / 8; cy = (gj + 0.5) * N / 8
        paw(cx % N, cy, 13, (gi * 1.7 + gj * 2.3) % 6.28, GROUND * 0.78)
# layout (tile coords 0..512); bold and big
# loose tangled strands that wrap round the whole tile (whole-number frequencies), under everything
def loop_h(y0, amp, k, ph, col, dark, w=3.4):
    pts = [(x, y0 + amp * math.sin(2 * math.pi * k * x / N + ph) + 0.4 * amp * math.sin(2 * math.pi * 3 * x / N)) for x in np.linspace(0, N, 220)]
    strand(pts, dark, w + 2.2); strand(pts, col, w)
def loop_v(x0, amp, k, ph, col, dark, w=3.4):
    pts = [(x0 + amp * math.sin(2 * math.pi * k * y / N + ph) + 0.4 * amp * math.sin(2 * math.pi * 3 * y / N + 1), y) for y in np.linspace(0, N, 220)]
    strand(pts, dark, w + 2.2); strand(pts, col, w)
loop_h(205, 38, 2, 0.4, PINK, PINK_D)
loop_h(470, 30, 1, 2.0, TEAL, TEAL_D)
loop_v(20, 34, 2, 1.2, C(150, 60, 200), PURP_D)
loop_v(250, 40, 1, 3.5, TEAL, TEAL_D)
yarn(120, 130, 62, PINK, PINK_D, 0.6)
yarn(380, 390, 58, TEAL, TEAL_D, 3.6)
yarn(400, 110, 44, PURP, PURP_D, 2.3)
yarn(470, 300, 38, PINK, PINK_D, 0.9)
cat(120, 400, 50, 0.15, CAT, EYE)
cat(300, 250, 42, -0.25, CAT, C(210, 60, 150))
cat(470, 190, 34, 0.4, CAT, C(230, 200, 60))
paws = [(250, 60, 36, 0.5, TEAL), (290, 130, 36, 0.7, TEAL),
        (40, 270, 34, -0.3, PINK), (200, 300, 30, 2.6, VIO),
        (185, 32, 30, 3.4, PINK), (240, 460, 34, 1.9, VIO),
        (480, 470, 30, 0.2, TEAL), (60, 30, 30, 2.9, VIO),
        (330, 470, 28, 1.2, PINK), (40, 170, 28, 4.0, TEAL), (200, 390, 26, 5.2, PINK), (350, 30, 26, 2.2, PINK)]
for p in paws: paw(*p)

out = Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8)).resize((N, N), Image.LANCZOS)
out.save(os.path.join(D, "paint.png"))
t = Image.new("RGB", (N * 2, N * 2))
for i in range(2):
    for j in range(2): t.paste(out, (i * N, j * N))
t.save(os.path.join(D, "tile_check.png"))

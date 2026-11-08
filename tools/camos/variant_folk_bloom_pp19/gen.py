# Folk Bloom: folk-painted lacquerware look, own procedural design. Tileable 512 px.
import os, math, random
import numpy as np
from PIL import Image, ImageDraw, ImageFilter
D = os.path.dirname(os.path.abspath(__file__))
T = 512; K = 4; S = T * K          # draw at 4x, wrap by drawing at 9 offsets
rng = random.Random(19)
BLACK = (22, 16, 14)
RED = (160, 24, 18); RED_D = (112, 14, 10); RED_L = (180, 40, 24)
GOLD = (150, 98, 22); GOLD_L = (176, 120, 32); GOLD_D = (112, 68, 14)
img = Image.new("RGB", (S, S), BLACK)
dr = ImageDraw.Draw(img)
OFF = [(dx, dy) for dx in (-S, 0, S) for dy in (-S, 0, S)]

def poly(pts, fill):
    for dx, dy in OFF:
        dr.polygon([(x * K + dx, y * K + dy) for x, y in pts], fill=fill)
def disc(x, y, r, fill):
    for dx, dy in OFF:
        X, Y = x * K + dx, y * K + dy
        dr.ellipse([X - r * K, Y - r * K, X + r * K, Y + r * K], fill=fill)
def stroke(pts, w0, w1, fill):
    # tapered brush stroke along a polyline (in tile px)
    n = len(pts)
    for i, (x, y) in enumerate(pts):
        t = i / max(1, n - 1)
        disc(x, y, (w0 + (w1 - w0) * t) / 2, fill)
def petal(cx, cy, ang, length, width, fill):
    pts = []
    for i in range(41):
        t = i / 40.0
        # teardrop: narrow at base, fat near 0.6, pointed tip
        w = width * math.sin(math.pi * t) ** 0.8 * (0.55 + 0.45 * t) * (1 - t ** 6)
        pts.append((t * length, w))
    pts = pts + [(x, -y) for x, y in reversed(pts)]
    c, s = math.cos(ang), math.sin(ang)
    poly([(cx + x * c - y * s, cy + x * s + y * c) for x, y in pts], fill)
def spiral(cx, cy, r0, turns, ang0, sign, w, fill):
    pts = []
    n = int(60 * turns)
    for i in range(n + 1):
        t = i / n
        a = ang0 + sign * t * turns * 2 * math.pi
        r = r0 * (1 - 0.85 * t)
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    stroke(pts, w, w * 0.35, fill)
def curve(p0, p1, p2, n=60):
    return [((1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t * t * p2[0],
             (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t * t * p2[1]) for t in np.linspace(0, 1, n)]
def leaf(x, y, ang, L, W, fill, vein=True):
    petal(x, y, ang, L, W, fill)
    if vein:
        c, s = math.cos(ang), math.sin(ang)
        stroke([(x + c * L * t, y + s * L * t) for t in np.linspace(0.15, 0.8, 20)], 3.0, 1.0, BLACK)
def flower(cx, cy, R, rot):
    n = 6
    for i in range(n):                      # gold back petals peeking between
        petal(cx, cy, rot + (i + 0.5) * 2 * math.pi / n, R * 0.95, R * 0.28, GOLD)
    for i in range(n):                      # big red petals
        a = rot + i * 2 * math.pi / n
        petal(cx, cy, a, R, R * 0.42, RED)
        petal(cx, cy, a, R * 0.72, R * 0.22, RED_L)     # brush highlight
        c, s = math.cos(a), math.sin(a)
        stroke([(cx + c * R * t, cy + s * R * t) for t in np.linspace(0.3, 0.75, 16)], 2.6, 1.0, RED_D)
    disc(cx, cy, R * 0.36, GOLD)
    disc(cx, cy, R * 0.25, RED_D)
    for i in range(8):                       # gold dots ring
        a = rot + i * 2 * math.pi / 8
        disc(cx + math.cos(a) * R * 0.30, cy + math.sin(a) * R * 0.30, R * 0.055, GOLD_L)
    disc(cx, cy, R * 0.11, GOLD_L)
def berries(cx, cy, r, n, rot):
    for i in range(n):
        a = rot + i * 2 * math.pi / n
        x, y = cx + math.cos(a) * r * 1.25, cy + math.sin(a) * r * 1.25
        disc(x, y, r, RED)
        disc(x - r * 0.3, y - r * 0.3, r * 0.32, RED_L)
    disc(cx, cy, r * 0.75, RED)
    disc(cx - r * 0.25, cy - r * 0.25, r * 0.25, GOLD_L)

# main flowers (big), placed so the torus is evenly filled
flowers = [(110, 120, 78, 0.2), (370, 250, 84, 0.9), (150, 400, 66, 0.5), (420, 470, 50, 1.4),
           (250, 300, 42, 0.0), (480, 150, 44, 0.6), (300, 30, 40, 1.1), (40, 500, 38, 0.3)]
# vines (gold) between them, drawn first, under everything
vines = [((110, 120), (250, 120), (370, 250)), ((370, 250), (290, 380), (150, 400)),
         ((150, 400), (40, 300), (110, 120)), ((370, 250), (470, 120), (110 + 512, 120)),
         ((150, 400), (300, 520), (420, 470)), ((420, 470), (470, 380), (370, 250)),
         ((110, 120), (60, 30), (150, 400 - 512))]
for p0, p1, p2 in vines:
    pts = curve(p0, p1, p2)
    stroke(pts, 9, 9, GOLD_D)
    stroke(pts, 6, 6, GOLD)
    # leaves and curls along the vine
    for t in (0.3, 0.55, 0.78):
        i = int(t * (len(pts) - 1))
        x, y = pts[i]; x2, y2 = pts[i + 1]
        a = math.atan2(y2 - y, x2 - x)
        side = 1 if rng.random() < 0.5 else -1
        leaf(x, y, a + side * 0.9, 46, 15, GOLD if t != 0.55 else RED_L)
    i = int(0.42 * (len(pts) - 1)); x, y = pts[i]; x2, y2 = pts[i + 1]
    a = math.atan2(y2 - y, x2 - x) - 1.3
    spiral(x + math.cos(a) * 26, y + math.sin(a) * 26, 26, 1.6, a + math.pi, 1, 6, GOLD)
# berry clusters in the gaps
for cx, cy, rot in [(200, 40, 0.3), (240, 200, 1.0), (40, 230, 0.7), (290, 450, 0.1), (480, 340, 0.5), (420, 60, 1.2), (60, 330, 0.4)]:
    berries(cx, cy, 13, 4, rot)
for cx, cy, R, rot in flowers:
    flower(cx, cy, R, rot)

small = img.resize((T, T), Image.LANCZOS)
a = np.asarray(small).astype(np.float32)
# soft "lacquer" brush texture: faint streaks so the black ground is not dead flat
yy, xx = np.mgrid[0:T, 0:T] / T
streak = 0.5 + 0.5 * np.sin(2 * math.pi * (3 * xx + 7 * yy) + 1.5 * np.sin(2 * math.pi * (2 * xx - yy)))
a *= (0.94 + 0.08 * streak)[..., None]
Image.fromarray(np.clip(a, 0, 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))
t2 = Image.new("RGB", (T * 2, T * 2))
for i in range(2):
    for j in range(2): t2.paste(small, (i * T, j * T))
t2.save(os.path.join(D, "tile_check.png"))

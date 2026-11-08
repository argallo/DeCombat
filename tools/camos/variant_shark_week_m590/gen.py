# Shark Week camo for the M590: procedural, tileable 512x512.
import numpy as np, os, math
from PIL import Image, ImageDraw
OUT = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 2; W = N * SS
rng = np.random.default_rng(590)
DIM = 0.74   # the M590 texture and lighting push mid-tones up hard; keep the whole palette deep

def c(r, g, b): return (int(r * 255), int(g * 255), int(b * 255))
DEEP  = (0.03, 0.10, 0.34)
MID   = (0.05, 0.20, 0.50)
CREST = (0.06, 0.27, 0.52)
FOAM  = c(0.34, 0.56, 0.70)
FIN   = c(0.32, 0.35, 0.41)
FIN_D = c(0.19, 0.21, 0.27)
HEAD  = c(0.32, 0.36, 0.42)
BELLY = c(0.48, 0.50, 0.54)
MOUTH = c(0.36, 0.05, 0.10)
TOOTH = c(0.86, 0.86, 0.84)
EYE   = c(0.03, 0.03, 0.05)
BUB   = c(0.36, 0.60, 0.78)

# --- background: wavy bands, integer frequencies so it wraps
y, x = np.mgrid[0:W, 0:W] / W
NB = 3
phase = y * NB + 0.10 * np.sin(2 * np.pi * (x * 3 + y)) + 0.05 * np.sin(2 * np.pi * (x * 7))
f = phase % 1.0
img = np.zeros((W, W, 3))
for i in range(3):
    t = np.clip((f - 0.0) / 0.55, 0, 1)
    band = DEEP[i] + (MID[i] - DEEP[i]) * t
    band = np.where(f > 0.78, CREST[i], band)
    img[..., i] = band
im = Image.fromarray((img * 255).astype(np.uint8))
d = ImageDraw.Draw(im)

def wrapped(fn, cx, cy, *a):
    for ox in (-W, 0, W):
        for oy in (-W, 0, W):
            fn(cx + ox, cy + oy, *a)

def crest_y(xx, k):
    # the y where phase == k + 0.78 (approx, solve iteratively)
    yy = (k + 0.78) / NB
    for _ in range(6):
        yy = (k + 0.78 - 0.10 * math.sin(2 * math.pi * (xx * 3 + yy)) - 0.05 * math.sin(2 * math.pi * xx * 7)) / NB
    return yy

# foam line at each crest (scalloped)
for k in range(NB):
    pts = []
    for s in range(0, W + 1, 4):
        xx = s / W
        pts.append((s, crest_y(xx, k) * W))
    for oy in (-W, 0, W):
        d.line([(px, py + oy) for px, py in pts], fill=FOAM, width=7 * SS)
    # scallop dots on the foam
    for s in range(0, W, 40 * SS):
        xx = s / W; py = crest_y(xx, k) * W
        r = 7 * SS
        wrapped(lambda cx, cy: d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=FOAM), s, py)

def fin(cx, cy, h, flip):
    s = -1 if flip else 1
    # curved shark fin: leading edge convex, trailing edge concave
    pts = []
    for t in np.linspace(0, 1, 24):   # leading edge from base-front to tip
        pts.append((cx - s * 0.75 * h + s * 1.20 * h * t ** 0.55, cy - h * t))
    for t in np.linspace(1, 0, 16):   # trailing edge hooked back
        pts.append((cx + s * 0.30 * h + s * 0.15 * h * t - s * 0.20 * h * math.sin(math.pi * t), cy - h * t))
    d.polygon(pts, fill=FIN)
    # shaded back half
    pts2 = [(cx + s * 0.02 * h, cy)] + [p for p in pts[20:]]
    d.polygon(pts2, fill=FIN_D)
    # foam splash at the base
    for j, dx in enumerate((-0.7, -0.35, 0.0, 0.35, 0.7)):
        r = (0.13 + 0.05 * (j % 2)) * h
        ex = cx + dx * h
        d.ellipse([ex - r, cy - r * 0.7, ex + r, cy + r * 0.7], fill=FOAM)

def grin(cx, cy, r):
    # cartoon shark face: grey head, pale belly jaw, huge toothy smile, one beady eye
    d.ellipse([cx - r, cy - r * 0.78, cx + r, cy + r * 0.78], fill=HEAD)
    d.chord([cx - r * 0.92, cy - r * 0.40, cx + r * 0.92, cy + r * 0.76], 0, 180, fill=BELLY)
    # mouth: crescent
    mx0, my0, mx1, my1 = cx - r * 0.78, cy - r * 0.42, cx + r * 0.78, cy + r * 0.58
    d.chord([mx0, my0, mx1, my1], 0, 180, fill=MOUTH)
    # top teeth along the straight upper edge
    mid = (my0 + my1) / 2
    nt = 7
    for i in range(nt):
        a = mx0 + (mx1 - mx0) * (i + 0.5) / nt
        hw = (mx1 - mx0) / nt / 2 * 0.95
        if abs(a - cx) > r * 0.70: continue
        d.polygon([(a - hw, mid), (a + hw, mid), (a, mid + r * 0.20)], fill=TOOTH)
    # bottom teeth pointing up from the curved lower edge
    for i in range(1, 6):
        ang = math.pi * i / 6
        bx = cx + (mx1 - mx0) / 2 * math.cos(ang) * 0.92
        by = mid + (my1 - mid) * math.sin(ang) * 0.92
        hw = r * 0.08
        d.polygon([(bx - hw, by), (bx + hw, by), (bx, by - r * 0.17)], fill=TOOTH)
    # eye + glint
    er = r * 0.13
    ex, ey = cx + r * 0.38, cy - r * 0.48
    d.ellipse([ex - er, ey - er, ex + er, ey + er], fill=EYE)
    g = er * 0.4
    d.ellipse([ex - er * 0.35 - g, ey - er * 0.35 - g, ex - er * 0.35 + g, ey - er * 0.35 + g], fill=TOOTH)
    # gills
    for k in range(3):
        gx = cx - r * 0.62 + k * r * 0.12
        d.arc([gx - r * 0.08, cy - r * 0.35, gx + r * 0.08, cy + r * 0.05], 100, 260, fill=FIN_D, width=int(r * 0.05))

def bubble(cx, cy, r):
    d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=BUB, width=max(3, int(r * 0.28)))
    g = r * 0.28
    d.ellipse([cx - r * 0.35 - g, cy - r * 0.35 - g, cx - r * 0.35 + g, cy - r * 0.35 + g], fill=BUB)

S = SS
# bubbles first (under the bigger motifs)
for (bx, by, br) in [(60, 40, 14), (78, 70, 9), (300, 150, 16), (318, 120, 10), (450, 300, 15),
                     (470, 270, 9), (150, 330, 13), (210, 470, 16), (230, 440, 9), (380, 420, 12),
                     (40, 230, 12), (500, 80, 11)]:
    wrapped(bubble, bx * S, by * S, br * S)

# fins ride on the crests
for (fx, k, h, fl) in [(0.08, 0, 104, False), (0.60, 0, 84, True), (0.36, 1, 108, False),
                       (0.90, 1, 80, False), (0.30, 2, 84, True), (0.62, 2, 104, False)]:
    py = crest_y(fx, k) * W
    wrapped(fin, fx * W, py + 4 * S, h * S, fl)

# grins in the open water between crests
for (gx, gy, gr) in [(0.30, 0.11, 74), (0.80, 0.46, 76), (0.08, 0.78, 70)]:
    wrapped(grin, gx * W, gy * W, gr * S)

im = im.resize((N, N), Image.LANCZOS)
im = Image.fromarray((np.asarray(im).astype(float) * DIM).clip(0, 255).astype(np.uint8))
im.save(os.path.join(OUT, "paint.png"))
print("ok")

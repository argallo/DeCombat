# Art Deco camo for the M1911 MEU: gold sunburst fans (scale tiling) and stepped chevron band on glossy black,
# a few emerald inlays. Drawn at 4x with PIL, wrapped in x, downsampled to a 512 tileable PNG.
import math, sys
from PIL import Image, ImageDraw
OUT = sys.argv[1] if len(sys.argv) > 1 else 'paint.png'
S = 4; T = 512; N = T * S
def c(r, g, b): return (int(r * 255), int(g * 255), int(b * 255))
BLACK = c(0.07, 0.065, 0.075)
GOLD = c(0.57, 0.385, 0.105)
GOLD_D = c(0.40, 0.26, 0.065)
EMER = c(0.02, 0.30, 0.17)
EMER_L = c(0.04, 0.38, 0.23)
im = Image.new('RGB', (N, N), BLACK)
d = ImageDraw.Draw(im)
R = 64; COLS = 4; DY = 32; ROWS = 12; BAND0 = ROWS * DY   # fans occupy v 0..384, band 384..512
def P(v): return v * S
def arc(cx, cy, r, w, col):
    d.arc([P(cx - r), P(cy - r), P(cx + r), P(cy + r)], 180, 360, fill=col, width=int(w * S))
def disc(cx, cy, r, col):
    d.pieslice([P(cx - r), P(cy - r), P(cx + r), P(cy + r)], 180, 360, fill=col)
def fan(cx, cy, emerald):
    disc(cx, cy, R, BLACK)
    if emerald:
        disc(cx, cy, R - 6, EMER); disc(cx, cy, R - 18, BLACK)
    arc(cx, cy, R, 6, GOLD)                    # bold outer rim
    arc(cx, cy, R - 18, 3, GOLD)              # thin inner rim
    nr = 7                                     # sunburst rays between hub and inner rim
    for i in range(1, nr):
        a = math.pi + math.pi * i / nr
        r0, r1 = 15, R - 19
        d.line([P(cx + r0 * math.cos(a)), P(cy + r0 * math.sin(a)), P(cx + r1 * math.cos(a)), P(cy + r1 * math.sin(a))],
               fill=GOLD, width=int(2.6 * S))
    for i in range(nr):                        # small gold drops between rays near the rim
        a = math.pi + math.pi * (i + 0.5) / nr
        rr = R - 27
        x, y = cx + rr * math.cos(a), cy + rr * math.sin(a)
        d.ellipse([P(x - 2.6), P(y - 2.6), P(x + 2.6), P(y + 2.6)], fill=GOLD_D)
    disc(cx, cy, 14, GOLD)                     # hub
    disc(cx, cy, 10, EMER_L if emerald else BLACK)
    disc(cx, cy, 5, GOLD)
for k in range(ROWS):
    cy = (k + 1) * DY
    off = (k % 2) * (R)
    for col in range(COLS):
        cx = col * 2 * R + off
        emerald = ((col * 2 + k) % 7) == 3
        for wx in (-T, 0, T):
            fan(cx + wx, cy, emerald)
# --- stepped chevron band, v 384..512 ---
d.rectangle([0, P(BAND0), N, N], fill=BLACK)
for y, w in ((BAND0, 6), (T - 6, 6)):
    d.rectangle([0, P(y), N, P(y + w)], fill=GOLD)
for y in (BAND0 + 10, T - 13):
    d.rectangle([0, P(y), N, P(y + 3)], fill=GOLD)
PER = 128; SH = 16; SV = 12
def zig(y0):
    pts = []
    for k in range(-1, 6):
        cx = k * PER + PER / 2
        for i in range(4, 0, -1):
            x = cx - i * SH
            pts += [(x, y0 + i * SV), (x, y0 + (i - 1) * SV)]
        pts += [(cx, y0)]
        for i in range(1, 5):
            x = cx + i * SH
            pts += [(x - SH + SH, y0 + (i - 1) * SV), (x, y0 + i * SV)] if False else [(cx + (i - 1) * SH + SH, y0 + (i - 1) * SV), (cx + i * SH, y0 + i * SV)]
    return pts
def zig2(y0):
    # stepped zigzag: apex flat tread, then down-steps both sides, meeting the next chevron at the valley
    pts = []
    for k in range(-1, 6):
        cx = k * PER + PER / 2
        x = cx - 4 * SH
        pts.append((x, y0 + 4 * SV))
        for i in range(4, 0, -1):
            pts.append((cx - i * SH, y0 + (i - 1) * SV + 0))
            pts.append((cx - (i - 1) * SH, y0 + (i - 1) * SV))
        # pts now at (cx, y0); mirror to the right
        for i in range(1, 5):
            pts.append((cx + (i - 1) * SH, y0 + i * SV))
            pts.append((cx + i * SH, y0 + i * SV))
    return pts
def poly(pts, col, w):
    pp = [(P(x), P(y)) for x, y in pts]
    d.line(pp, fill=col, width=int(w * S))
    h = w * S / 2
    for x, y in pp:
        d.rectangle([x - h, y - h, x + h, y + h], fill=col)
A = BAND0 + 22
for k in range(-1, 6):                        # emerald stepped pyramid under the innermost chevron
    cx = k * PER + PER / 2
    for i in range(3):
        y = A + 26 + 14 + i * SV
        w = 6 + i * SH
        d.rectangle([P(cx - w), P(y), P(cx + w), P(min(y + SV, T - 16))], fill=EMER)
for j, (dy, col, w) in enumerate(((0, GOLD, 6), (26, GOLD, 6))):
    poly(zig2(A + dy), col, w)
img = im.resize((T, T), Image.LANCZOS)
img.save(OUT)
print('wrote', OUT)

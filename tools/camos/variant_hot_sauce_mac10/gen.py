# Hot Sauce camo for the MAC-10: procedural, tileable. Run with the pilenv python.
import os, math, random
import numpy as np
from PIL import Image, ImageDraw, ImageFilter
D = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 4; M = N * SS
rng = random.Random(7)

# ground: orange to red, tileable (whole-number frequencies)
y, x = np.mgrid[0:N, 0:N] / N
t = 0.5 + 0.30 * np.sin(2 * np.pi * (x + y)) + 0.20 * np.sin(2 * np.pi * (2 * y - x) + 1.3)
t = np.clip(t, 0, 1)[..., None]
orange = np.array([0.55, 0.20, 0.04]); red = np.array([0.42, 0.06, 0.03])
ground = orange * t + red * (1 - t)
base = Image.fromarray((ground * 255).astype(np.uint8)).resize((M, M), Image.BICUBIC)

def wrapdraw(layer, fn):
    d = ImageDraw.Draw(layer)
    for ox in (-M, 0, M):
        for oy in (-M, 0, M):
            fn(d, ox, oy)

# flame licks: teardrops pointing "up" (-y), yellow-orange with a hot core
flames = Image.new("RGBA", (M, M), (0, 0, 0, 0))
def flame_poly(cx, cy, w, h, lean, k=40):
    pts = []
    for i in range(k + 1):
        a = i / k * 2 * math.pi
        s = math.sin(a); c = math.cos(a)
        # bottom round, top pointed
        yy = -c
        r = w * (0.5 + 0.5 * yy) ** 0.6 if yy > -1 else 0
        r = w * math.sqrt(max(0, (1 + c) / 2)) * (1 - 0.0) 
        px = cx + s * w * (0.25 + 0.75 * (1 + c) / 2) + lean * (1 - c) / 2 * h * 0.3
        py = cy - h * (1 - c) / 2 + h * 0.15 * c
        pts.append((px, py))
    return pts
flist = []
for i in range(26):
    flist.append((rng.uniform(0, M), rng.uniform(0, M), rng.uniform(22, 38) * SS, rng.uniform(55, 95) * SS, rng.uniform(-0.6, 0.6)))
def draw_flames(d, ox, oy):
    for cx, cy, w, h, lean in flist:
        d.polygon(flame_poly(cx + ox, cy + oy, w, h, lean), fill=(170, 70, 12, 255))
        d.polygon(flame_poly(cx + ox, cy + oy + h * 0.05, w * 0.55, h * 0.62, lean), fill=(176, 120, 22, 255))
wrapdraw(flames, draw_flames)
base = Image.alpha_composite(base.convert("RGBA"), flames)

# peppers
def bez(p0, p1, p2, t):
    return ((1-t)**2*p0[0] + 2*(1-t)*t*p1[0] + t*t*p2[0], (1-t)**2*p0[1] + 2*(1-t)*t*p1[1] + t*t*p2[1])

def pepper_geom(cx, cy, ang, L, bend):
    dx, dy = math.cos(ang), math.sin(ang); nx, ny = -dy, dx
    p0 = (cx - dx * L / 2, cy - dy * L / 2)
    p2 = (cx + dx * L / 2, cy + dy * L / 2)
    p1 = (cx + nx * L * bend, cy + ny * L * bend)
    return p0, p1, p2, (nx, ny)

NP = 10; GAP = 66
peppers = []
def samples(cx, cy, ang, L, bend):
    p0, p1, p2, _ = pepper_geom(cx, cy, ang, L, bend)
    return [bez(p0, p1, p2, k / 8) for k in range(9)] + [(p0[0] - (p1[0]-p0[0]) * 0.35, p0[1] - (p1[1]-p0[1]) * 0.35)]
def tdist(a, b):
    dx = abs(a[0] - b[0]) % M; dy = abs(a[1] - b[1]) % M
    return math.hypot(min(dx, M - dx), min(dy, M - dy))
placed = []
tries = 0
while len(peppers) < NP and tries < 20000:
    tries += 1
    cx, cy = rng.uniform(0, M), rng.uniform(0, M)
    ang = rng.uniform(0, 2 * math.pi); L = rng.uniform(128, 152) * SS
    bend = rng.uniform(0.22, 0.4) * (1 if rng.random() < 0.5 else -1)
    sm = samples(cx, cy, ang, L, bend)
    if all(tdist(a, b) > GAP * SS for a in sm for b in placed):
        green = len(peppers) % 2 == 1
        peppers.append((cx, cy, ang, L, bend, green)); placed += sm
print(len(peppers), "peppers")

R0 = 29 * SS
def draw_peppers(d, ox, oy, pass_):
    for cx, cy, ang, L, bend, green in peppers:
        p0, p1, p2, (nx, ny) = pepper_geom(cx + ox, cy + oy, ang, L, bend)
        if green:
            body, dark, hi = (40, 120, 22), (14, 48, 8), (150, 205, 110)
        else:
            body, dark, hi = (168, 14, 18), (60, 4, 6), (240, 150, 140)
        steps = 90
        for k in range(steps + 1):
            tt = k / steps
            px, py = bez(p0, p1, p2, tt)
            # fat near the stem end (t=0), pointed tip at t=1
            r = R0 * (1 - tt) ** 0.55 * (0.82 + 0.18 * math.sin(math.pi * min(1, tt * 2.2)))
            r = max(r, 2 * SS)
            if pass_ == 0:
                e = 5 * SS
                d.ellipse([px - r - e, py - r - e, px + r + e, py + r + e], fill=(30, 4, 2))
            elif pass_ == 1:
                d.ellipse([px - r, py - r, px + r, py + r], fill=dark)
            elif pass_ == 2:
                rr = r * 0.80
                # shade side darker: offset body toward the highlight side
                sx, sy = px - nx * r * 0.12 * (1 if bend > 0 else -1), py - ny * r * 0.12 * (1 if bend > 0 else -1)
                d.ellipse([sx - rr, sy - rr, sx + rr, sy + rr], fill=body)
            elif pass_ == 3 and 0.08 < tt < 0.75:
                s = -1 if bend > 0 else 1
                rr = r * 0.22 * (1 - tt * 0.6)
                hx, hy = px + nx * r * 0.45 * s, py + ny * r * 0.45 * s
                d.ellipse([hx - rr, hy - rr, hx + rr, hy + rr], fill=hi)
        if pass_ == 4:
            # calyx cap at the fat end and a curly stem
            tx, ty = bez(p0, p1, p2, 0.02)
            ax, ay = p1[0] - p0[0], p1[1] - p0[1]; al = math.hypot(ax, ay); ax, ay = ax / al, ay / al
            cap = R0 * 0.72
            d.ellipse([tx - cap, ty - cap * 0.95, tx + cap, ty + cap * 0.95], fill=(14, 40, 6))
            d.ellipse([tx - cap * 0.8, ty - cap * 0.75, tx + cap * 0.8, ty + cap * 0.75], fill=(46, 112, 22))
            # stem goes backwards out of the cap and curls
            pts = []
            sx, sy = tx - ax * cap * 0.6, ty - ay * cap * 0.6
            base_a = math.atan2(-ay, -ax)
            curl = 1 if bend > 0 else -1
            for k in range(40):
                q = k / 39
                a = base_a + curl * q * 3.6
                rad = (1 - q * 0.55) * 1.0
                sx += math.cos(a) * 1.9 * SS * rad * 1.4
                sy += math.sin(a) * 1.9 * SS * rad * 1.4
                pts.append((sx, sy))
            d.line(pts, fill=(14, 40, 6), width=int(12 * SS), joint="curve")
            d.line(pts, fill=(70, 140, 30), width=int(7 * SS), joint="curve")
            for (qx, qy), w in ((pts[0], 7.5), (pts[-1], 7.5)):
                d.ellipse([qx - w * SS / 2 * 2, qy - w * SS / 2 * 2, qx + w * SS / 2 * 2, qy + w * SS / 2 * 2], fill=(14, 40, 6))
                d.ellipse([qx - w * SS / 2 * 1.2, qy - w * SS / 2 * 1.2, qx + w * SS / 2 * 1.2, qy + w * SS / 2 * 1.2], fill=(70, 140, 30))

# soft drop shadow
shadow = Image.new("RGBA", (M, M), (0, 0, 0, 0))
wrapdraw(shadow, lambda d, ox, oy: draw_peppers(d, ox + 6 * SS, oy + 8 * SS, 0))
sa = np.array(shadow)[..., 3]
sh = Image.fromarray((sa * 0.45).astype(np.uint8))
sh = sh.filter(ImageFilter.GaussianBlur(6 * SS))  # blur edges at seam are minor
shl = Image.new("RGBA", (M, M), (40, 4, 0, 0)); shl.putalpha(sh)
base = Image.alpha_composite(base, shl)
layer = Image.new("RGBA", (M, M), (0, 0, 0, 0))
for p in range(5):
    wrapdraw(layer, lambda d, ox, oy, p=p: draw_peppers(d, ox, oy, p))
base = Image.alpha_composite(base, layer)
out = base.convert("RGB").resize((N, N), Image.LANCZOS)
out.save(os.path.join(D, "paint.png"))
# tile check
tc = Image.new("RGB", (N * 2, N * 2))
for i in range(2):
    for j in range(2): tc.paste(out, (i * N, j * N))
tc.save(os.path.join(D, "tile_check.png"))
print("ok")

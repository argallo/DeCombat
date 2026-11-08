# Peppermint: round swirl candies (candy red / pale pink pinwheel) packed on a mint-green ground. Procedural, tileable.
import os, sys
import numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 2; M = N * SS
NCOL = int(sys.argv[1]) if len(sys.argv) > 1 else 4      # candies per row in one tile
RAD = float(sys.argv[2]) if len(sys.argv) > 2 else 0.48  # radius as a fraction of the column spacing
rng = np.random.default_rng(7)

def c(*v): return np.array(v, dtype=np.float32)
MINT  = c(0.15, 0.42, 0.31)
MINT2 = c(0.11, 0.36, 0.26)
RED   = c(0.50, 0.03, 0.08)
PINK  = c(0.67, 0.30, 0.40)
RIM   = c(0.36, 0.03, 0.08)
GLOSS = c(0.80, 0.70, 0.72)

y, x = np.mgrid[0:M, 0:M].astype(np.float32) / M          # 0..1 torus
# mint ground with a soft, tileable mottling
g = 0.5 + 0.25 * np.sin(2*np.pi*(2*x + y)) * np.sin(2*np.pi*(3*y - x))
img = MINT2[None, None] * (1 - g[..., None]) + MINT[None, None] * g[..., None]

sx = 1.0 / NCOL; nrow = NCOL; sy = 1.0 / nrow             # offset rows (brick-hex packing)
R = RAD * sx
centres = []
for j in range(nrow):
    for i in range(NCOL):
        cx = (i + 0.5 * (j % 2) + rng.uniform(-0.04, 0.04)) * sx
        cy = (j + 0.5 + rng.uniform(-0.04, 0.04)) * sy
        centres.append((cx, cy, R * rng.uniform(0.93, 1.03), rng.uniform(0, 2*np.pi), rng.choice([-1, 1])))

def wrapd(a): return (a + 0.5) % 1.0 - 0.5
# shadows first, then candies
for cx, cy, r, ph, sg in centres:
    dx = wrapd(x - cx - 0.012); dy = wrapd(y - cy - 0.016)
    d = np.sqrt(dx*dx + dy*dy) / r
    sh = np.clip(1.15 - d, 0, 1) ** 1.5 * 0.55
    img *= (1 - sh[..., None])
aa = 1.5 / M
for cx, cy, r, ph, sg in centres:
    dx = wrapd(x - cx); dy = wrapd(y - cy)
    rr = np.sqrt(dx*dx + dy*dy)
    d = rr / r
    inside = np.clip((r - rr) / aa, 0, 1)
    if not inside.any(): continue
    ang = np.arctan2(dy, dx)
    # pinwheel: 6 stripe pairs with a gentle spiral twist
    t = np.sin(6 * (ang * sg + ph) + 3.0 * d)
    s = np.clip(t * 4, -1, 1) * 0.5 + 0.5           # crisp but antialiased stripes
    col = RED[None, None] * s[..., None] + PINK[None, None] * (1 - s[..., None])
    # small pink centre dot
    core = np.clip((0.16 - d) / 0.03, 0, 1)
    col = col * (1 - core[..., None]) + PINK[None, None] * core[..., None]
    # dome shading: lighter top-left, darker toward the rim
    shade = 1.0 + 0.18 * (-(dx + dy) / r) - 0.25 * d ** 3
    col = col * shade[..., None]
    # dark candy rim
    rim = np.clip((d - 0.88) / 0.06, 0, 1)
    col = col * (1 - rim[..., None]) + RIM[None, None] * rim[..., None]
    # glossy highlight: a crescent ellipse upper-left
    hx = (dx + 0.38 * r) / (0.30 * r); hy = (dy + 0.40 * r) / (0.15 * r)
    ca, sa = np.cos(-0.75), np.sin(-0.75)
    ex = hx * ca - hy * sa * 0.5; ey = hx * sa * 2 + hy * ca
    e = np.clip((1 - np.sqrt(((dx + 0.36*r)*ca + (dy + 0.36*r)*sa)**2 / (0.30*r)**2
                          + (-(dx + 0.36*r)*sa + (dy + 0.36*r)*ca)**2 / (0.12*r)**2)) * 3, 0, 1)
    col = col * (1 - 0.85 * e[..., None]) + GLOSS[None, None] * (0.85 * e[..., None])
    small = np.clip((1 - np.sqrt((dx - 0.05*r + 0.50*r)**2 + (dy + 0.05*r)**2) / (0.07*r)) * 3, 0, 1)
    col = col * (1 - 0.7 * small[..., None]) + GLOSS[None, None] * (0.7 * small[..., None])
    img = img * (1 - inside[..., None]) + col * inside[..., None]

out = (np.clip(img, 0, 1) * 255).astype(np.uint8)
im = Image.fromarray(out, "RGB").resize((N, N), Image.LANCZOS)
im.save(os.path.join(D, "paint.png"))
t = Image.new("RGB", (N * 2, N * 2))
for a in (0, N):
    for b in (0, N): t.paste(im, (a, b))
t.save(os.path.join(D, "tile_check.png"))
print("ok", NCOL, RAD)

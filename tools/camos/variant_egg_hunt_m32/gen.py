# Egg Hunt camo for the M32: decorated eggs in paper grass. Procedural, tileable.
import numpy as np, os, sys
from PIL import Image
OUT = os.path.dirname(os.path.abspath(__file__))
N = 1024  # supersampled; period N, downsampled to 512
rng = np.random.default_rng(int(sys.argv[1]) if len(sys.argv) > 1 else 7)
yy, xx = np.mgrid[0:N, 0:N].astype(np.float32)

def c(h): return np.array([int(h[i:i+2], 16) / 255 for i in (0, 2, 4)], np.float32)
PINK, TEAL, YEL, PUR = c("b8156a"), c("0a7472"), c("b4840a"), c("6420a0")
PINK_D, TEAL_D, YEL_D, PUR_D = c("7a0c42"), c("075a58"), c("8a5a04"), c("3a1068")
WHITE = c("a898b8")
G0, G1, G2 = c("0f3410"), c("1a5216"), c("286a20")

img = np.zeros((N, N, 3), np.float32)
# paper grass: base mid green + many slanted blades (tileable sines plus drawn strands)
t = (np.sin(2*np.pi*(xx*9/N + yy*2/N)) * 0.5 + 0.5)
t2 = (np.sin(2*np.pi*(xx*23/N - yy*3/N + 0.3)) * 0.5 + 0.5)
img[:] = G0 * (1 - t[..., None]) + G1 * t[..., None]
# blades: thin curvy strips
for i in range(260):
    bx, by = rng.uniform(0, N), rng.uniform(0, N)
    L = rng.uniform(80, 200); lean = rng.uniform(-0.5, 0.5); w = rng.uniform(4, 8)
    col = [G0 * 0.8, G1, G2, c("225e14")][rng.integers(4)]
    dx = (xx - bx + N/2) % N - N/2; dy = (yy - by + N/2) % N - N/2
    s = -dy / L  # 0..1 up the blade
    off = dx - lean * (-dy) - 0.15*L*s*s*np.sign(lean)
    ww = w * (1 - s)
    m = (s > 0) & (s < 1) & (np.abs(off) < ww)
    img[m] = col

# eggs placed on a jittered grid so they never overlap
EG = []
cols = [(PINK, PINK_D), (TEAL, TEAL_D), (YEL, YEL_D), (PUR, PUR_D)]
cells = [(i, j) for i in range(3) for j in range(3)]
k = 0
for (i, j) in cells:
    cx = (i + 0.5) * N / 3 + rng.uniform(-30, 30) + (j % 2) * N / 6
    cy = (j + 0.5) * N / 3 + rng.uniform(-25, 25)
    EG.append((cx % N, cy % N, rng.uniform(-0.6, 0.6), k % 4, (k * 3 + j) % 3, rng.integers(0, 4)))
    k += 1

for (cx, cy, ang, ci, deco, ci2) in EG:
    a, b = 130.0, 166.0  # half width / half height
    dx = (xx - cx + N/2) % N - N/2; dy = (yy - cy + N/2) % N - N/2
    ca, sa = np.cos(ang), np.sin(ang)
    u = ca*dx + sa*dy; v = -sa*dx + ca*dy  # v along egg's long axis (neg = top)
    vn = v / b
    un = u / (a * (1 - 0.16*(-vn)))  # narrower top
    r = un**2 + vn**2
    egg = r < 1
    base, dark = cols[ci]
    acc, accd = cols[(ci + 1 + ci2 % 3) % 4]
    if ci2 % 3 == 0: acc = WHITE * 0.85 + base * 0.15
    # shadow on grass under the egg
    sh = ((u/(a*1.08))**2 + ((v - 18)/(b*1.05))**2 < 1) & ~egg
    img[sh] *= 0.55
    e = np.broadcast_to(base, img.shape).copy()
    if deco == 0:  # bold horizontal stripes with a zigzag band
        band = np.floor((vn + 1) * 3.5) % 2 == 1
        e[band] = acc
        zz = np.abs(vn - (-0.02 + 0.13 * (2*np.abs(((un*3) % 1) - 0.5)*2 - 1))) < 0.07
        e[zz] = accd if ci2 % 2 else dark
    elif deco == 1:  # big zigzag stripes
        tri = 2*np.abs(((u/a*2.2) % 1) - 0.5)
        zv = (vn + 0.18*tri) * 3.2
        z = np.floor(zv) % 2 == 0
        e[z] = acc
        e[(np.abs(zv - np.round(zv)) < 0.09)] = dark
    else:  # fat polka dots on a stripe belt
        belt = np.abs(vn) < 0.2
        e[belt] = acc
        gu = (u / 52.0); gv = (v / 52.0) + 0.5*(np.floor(gu + 0.5) % 2)
        du = gu - np.round(gu); dv = gv - np.round(gv)
        dots = (du**2 + dv**2) < 0.34**2
        e[dots & ~belt] = acc
        e[dots & belt] = dark
    # shading: soft rim darker, highlight upper left
    shade = 1.0 - 0.35 * np.clip(r, 0, 1)**2
    hl = np.exp(-(((un + 0.35)**2 + (vn + 0.45)**2) / 0.05)) * 0.25
    e = e * shade[..., None] + hl[..., None]
    # dark outline
    outl = egg & (r > 0.93)
    e[outl] = dark * 0.7
    img[egg] = e[egg]

# a few grass blades in front of each egg's base (nestled)
for (cx, cy, ang, ci, deco, ci2) in EG:
    for q in range(4):
        bx = cx + rng.uniform(-110, 110); by = cy + 162 + rng.uniform(0, 20)
        L = rng.uniform(60, 120); lean = rng.uniform(-0.6, 0.6); w = rng.uniform(5, 9)
        col = [G1, G2, c("225e14")][rng.integers(3)]
        dx = (xx - bx + N/2) % N - N/2; dy = (yy - by + N/2) % N - N/2
        s = -dy / L
        off = dx - lean * (-dy)
        m = (s > 0) & (s < 1) & (np.abs(off) < w * (1 - s))
        img[m] = col

im = Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8)).resize((512, 512), Image.LANCZOS)
im.save(os.path.join(OUT, "paint.png"))
print("ok")

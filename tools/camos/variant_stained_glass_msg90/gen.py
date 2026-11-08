# MSG-90 "Stained Glass": toroidal Voronoi panes in four jewel tones, thick lead lines,
# mottled glass per pane. fx.png: a soft diagonal beam of tinted light, mostly black.
import os
import numpy as np
from PIL import Image

D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(91)

# --- points, Lloyd-relaxed on the torus
K = 18
pts = rng.random((K, 2))
yy, xx = np.mgrid[0:N, 0:N] / N
def nearest2(pts):
    dx = xx[None] - pts[:, 0, None, None]; dx -= np.round(dx)
    dy = yy[None] - pts[:, 1, None, None]; dy -= np.round(dy)
    d = np.hypot(dx, dy)
    o = np.argsort(d, axis=0)
    return o[0], o[1], d
for _ in range(3):
    i1, _, _ = nearest2(pts)
    new = []
    for k in range(K):
        m = i1 == k
        if not m.any():
            new.append(pts[k]); continue
        # circular mean on torus
        ax = np.angle(np.exp(2j*np.pi*xx[m]).mean()) / (2*np.pi) % 1
        ay = np.angle(np.exp(2j*np.pi*yy[m]).mean()) / (2*np.pi) % 1
        new.append([ax, ay])
    pts = np.array(new)
i1, i2, d = nearest2(pts)

# exact distance to the bisector of the nearest two sites
p1 = pts[i1]; p2 = pts[i2]
def wrapd(a, b):
    v = a - b; return v - np.round(v)
qx = xx; qy = yy
v12 = np.stack([wrapd(p2[..., 0], p1[..., 0]), wrapd(p2[..., 1], p1[..., 1])], -1)
vq = np.stack([wrapd(qx, p1[..., 0]), wrapd(qy, p1[..., 1])], -1)
L = np.linalg.norm(v12, axis=-1)
proj = (vq * v12).sum(-1) / L
edge = (L / 2 - proj) * N          # pixels to the lead line centre

# --- colour assignment: graph colouring so neighbours differ
adj = {k: set() for k in range(K)}
for a, b in zip(i1[edge < 2].ravel(), i2[edge < 2].ravel()):
    adj[int(a)].add(int(b)); adj[int(b)].add(int(a))
pal = np.array([
    [0.46, 0.04, 0.08],   # ruby
    [0.06, 0.12, 0.50],   # cobalt
    [0.04, 0.33, 0.15],   # emerald
    [0.54, 0.29, 0.03],   # amber
])
import sys
sys.setrecursionlimit(10000)
def solve():
    order = sorted(range(K), key=lambda k: -len(adj[k]))
    col = {}
    def bt(i):
        if i == K: return True
        k = order[i]
        used = {col[n] for n in adj[k] if n in col}
        cs = [c for c in rng.permutation(4) if c not in used]
        for c in cs:
            col[k] = int(c)
            if bt(i + 1): return True
            del col[k]
        return False
    return col if bt(0) else None
best = solve()
assert best is not None, "no 4-colouring"
cid = np.array([best[k] for k in range(K)])

# --- periodic noise (FFT filtered)
def pnoise(scale, seed):
    r = np.random.default_rng(seed).standard_normal((N, N))
    f = np.fft.fftfreq(N)[:, None]**2 + np.fft.fftfreq(N)[None, :]**2
    g = np.exp(-f * (N / scale)**2)
    n = np.real(np.fft.ifft2(np.fft.fft2(r) * g))
    return (n - n.mean()) / n.std()
mott = 0.6 * pnoise(18, 1) + 0.4 * pnoise(6, 2)
streak = pnoise(40, 3)

base = pal[cid[i1]]                       # N,N,3
# per-pane tone variation (each pane a slightly different batch of glass)
tone = 0.85 + 0.3 * np.random.default_rng(5).random(K)
base = base * tone[i1][..., None]
# mottled glass: brightness ripple plus slight hue drift
shade = 1 + 0.16 * mott + 0.08 * streak
# glass darkens toward the lead (light passes thinner near the came)
rim = np.clip(edge / 26, 0, 1)
shade *= 0.72 + 0.28 * rim**0.6
# per-pane light falloff: one side of each pane a little brighter (uneven glass)
ang = np.random.default_rng(7).random(K) * 2 * np.pi
gx = np.cos(ang)[i1] * vq[..., 0] + np.sin(ang)[i1] * vq[..., 1]
shade *= 1 + 1.6 * gx
glass = base * shade[..., None]

# roundels: a ringed disc of another colour in some panes (classic leaded roundel)
rr = np.random.default_rng(91)
dq = np.hypot(vq[..., 0], vq[..., 1]) * N   # px from own site
ring_d = np.full((N, N), 1e9)
inner = np.zeros((N, N), bool)
rcol = np.zeros((N, N), int)
for k in range(K):
    if rr.random() < 0.45:
        m = i1 == k
        R = min(edge[m].max() * 0.62, 46)
        if R < 18: continue
        c2 = [c for c in range(4) if c != cid[k]][rr.integers(3)]
        dd = dq[m]
        ring_d[m] = np.abs(dd - R)
        inner[m] = dd < R
        rcol[m] = c2
rim2 = np.clip((ring_d) / 14, 0, 1)
inner_glass = pal[rcol] * (1 + 0.2 * mott[..., None]) * (0.75 + 0.35 * rim2[..., None])
glass = np.where(inner[..., None], inner_glass, glass)

# --- lead lines: thick, dark, with a little rounded bevel
W = 10.0
lead_m = np.clip((W - edge) / 1.5 + 0.5, 0, 1)
bev = np.clip(1 - (edge / W)**2, 0, 1)
lead = np.stack([0.07, 0.07, 0.08]) * (0.75 + 0.6 * bev[..., None]**2)
img = glass * (1 - lead_m[..., None]) + lead * lead_m[..., None]
Wr = 5.5
ring_m = np.clip((Wr - ring_d) / 1.5 + 0.5, 0, 1)
rbev = np.clip(1 - (ring_d / Wr)**2, 0, 1)
rlead = np.stack([0.07, 0.07, 0.08]) * (0.75 + 0.6 * rbev[..., None]**2)
img = img * (1 - ring_m[..., None]) + rlead * ring_m[..., None]
lead_m = np.maximum(lead_m, ring_m)
img = np.clip(img, 0, 1)
Image.fromarray((img * 255 + 0.5).astype(np.uint8)).save(os.path.join(D, "paint.png"))

# --- effect: one soft diagonal beam (period 1 along u+v), tinted by the pane
s = (xx + yy) % 1.0
dist = np.minimum(s, 1 - s)
beam = np.exp(-(dist / 0.055)**2) + 0.35 * np.exp(-(dist / 0.14)**2)   # bright core + soft halo
tint = np.where(inner[..., None], pal[rcol], pal[cid[i1]]) * 2.0 + 0.15
fxv = tint * beam[..., None] * (0.85 + 0.15 * mott[..., None]) * (1 - lead_m[..., None])
fxv = np.clip(fxv, 0, 1)
Image.fromarray((fxv * 255 + 0.5).astype(np.uint8)).save(os.path.join(D, "fx.png"))

# tile check
t = np.tile(img, (2, 2, 1))
Image.fromarray((t * 255).astype(np.uint8)).resize((512, 512)).save(os.path.join(D, "tile_check.png"))
print("ok", np.bincount(cid, minlength=4))

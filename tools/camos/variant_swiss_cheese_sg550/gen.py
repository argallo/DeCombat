# Swiss Cheese camo for the SG550: golden Emmental with round holes, procedurally drawn, tileable.
import numpy as np, sys
from PIL import Image
N = 512
rng = np.random.default_rng(550)
scale = float(sys.argv[1]) if len(sys.argv) > 1 else 1.0
y, x = np.mgrid[0:N, 0:N].astype(np.float32)

# base cheese: warm golden yellow with gentle tileable mottling
base = np.array([0.63, 0.44, 0.04], np.float32) * scale
mott = np.zeros((N, N), np.float32)
for fx, fy, a in [(1, 2, 0.5), (3, 1, 0.3), (2, 3, 0.25), (5, 4, 0.15), (7, 6, 0.1)]:
    ph = rng.uniform(0, 2*np.pi)
    mott += a*np.sin(2*np.pi*(fx*x + fy*y)/N + ph)
mott /= np.abs(mott).max()
img = base[None, None, :] * (1 + 0.07*mott[..., None])

# holes: poisson-ish placement on the torus, sizes varied
holes = []
radii = [68, 58, 50, 44, 38, 34, 30, 27, 24, 22, 20, 18, 16, 14]
tries = 0
target = []
for r in radii:
    target += [r] * (1 if r > 35 else 2 if r > 20 else 3)
for r in target:
    for _ in range(4000):
        cx, cy = rng.uniform(0, N, 2)
        ok = True
        for (hx, hy, hr) in holes:
            dx = min(abs(cx-hx), N-abs(cx-hx)); dy = min(abs(cy-hy), N-abs(cy-hy))
            if (dx*dx+dy*dy) ** 0.5 < r + hr + 12:
                ok = False; break
        if ok:
            holes.append((cx, cy, r)); break

L = np.array([-0.6, -0.8]); L /= np.linalg.norm(L)  # light from upper-left
for (cx, cy, r) in holes:
    dx = (x - cx + N/2) % N - N/2
    dy = (y - cy + N/2) % N - N/2
    d = np.sqrt(dx*dx + dy*dy)
    m = d < r + 6
    if not m.any():
        continue
    dd = d[m]; ux = dx[m]/np.maximum(dd, 1e-3); uy = dy[m]/np.maximum(dd, 1e-3)
    facing = ux*L[0] + uy*L[1]   # +1 on side towards light
    col = img[m].copy()
    t = np.clip(dd / r, 0, 2)
    inside = dd < r
    # cavity: upper-left inner wall in shadow, lower-right wall catches light
    depth = 1 - t**2
    shade = 0.62 + 0.34*(-facing)*np.sqrt(np.clip(t, 0, 1)) - 0.10*depth
    cav = np.array([0.60, 0.33, 0.04], np.float32)[None, :] * scale * shade[:, None]
    cav[:, 2] = np.maximum(cav[:, 2], 0.02)
    # lit rim: thin band just outside the edge, brighter on the side away from the light (lower-right lip is lit... use lip facing light)
    rim_w = max(3.5, r*0.15)
    rimd = np.abs(dd - r)
    rim = np.clip(1 - rimd/rim_w, 0, 1)
    lip = np.clip(0.35 + 0.65*facing, 0, 1)   # lip towards the light is lit
    rimcol = np.array([0.74, 0.56, 0.10], np.float32) * scale
    # darker shadow lip on the far side (inside edge)
    aa = np.clip((r - dd) + 0.5, 0, 1)  # antialias inside
    col = col*(1-aa[:, None]) + cav*aa[:, None]
    k = (rim*lip)[:, None]*0.9
    col = col*(1-k) + rimcol[None, :]*k
    # soft ring of shadow outside on the light-away side (rim casts edge)
    out = (dd >= r) & (dd < r + 6)
    so = np.clip(1 - (dd - r)/6, 0, 1) * np.clip(-facing, 0, 1) * 0.25 * out
    col *= (1 - so[:, None])
    img[m] = col

img = np.clip(img, 0, 1)
Image.fromarray((img*255).astype(np.uint8)).save(__file__.rsplit('/', 1)[0] + '/paint.png')
print(len(holes), 'holes')

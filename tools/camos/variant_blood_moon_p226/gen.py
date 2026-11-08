# Blood Moon camo for the P226: cratered crimson lunar surface + drifting dark-red cloud wisps (effect).
# Everything procedural, tileable on the torus (integer frequencies / modulo distances).
import sys, numpy as np
from PIL import Image
D = sys.argv[1]
N = 512
rng = np.random.default_rng(226)
y, x = np.mgrid[0:N, 0:N] / N  # 0..1

def fnoise(fmin, fmax, seed, aniso=(1.0, 1.0)):
    r = np.random.default_rng(seed); out = np.zeros((N, N))
    for _ in range(60):
        fx = r.integers(-fmax, fmax + 1); fy = r.integers(-fmax, fmax + 1)
        f = np.hypot(fx, fy)
        if f < fmin or f > fmax: continue
        a = 1.0 / f ** 1.2
        out += a * np.cos(2 * np.pi * (fx * x * aniso[0] + fy * y * aniso[1]) + r.uniform(0, 2 * np.pi))
    out -= out.min(); out /= out.max(); return out

def fbm(seed, octs, base):
    o = np.zeros((N, N)); amp = 1; tot = 0
    for i in range(octs):
        o += amp * fnoise(base * 2 ** i, base * 2 ** (i + 1), seed + i); tot += amp; amp *= 0.5
    return o / tot

# --- base surface: highlands vs dark mare patches
mare = fbm(10, 3, 1)
mare = np.clip((mare - 0.45) / 0.10, 0, 1)
mare = mare * mare * (3 - 2 * mare)
grain = fbm(30, 3, 6)
high = np.array([0.33, 0.065, 0.05])
low = np.array([0.19, 0.028, 0.035])
col = high[None, None] * (1 - mare[..., None]) + low[None, None] * mare[..., None]
col *= (0.86 + 0.28 * grain)[..., None]

# --- craters (toroidal), big few + medium, lit from upper-left
L = np.array([-0.7, -0.7]); L /= np.linalg.norm(L)
crs = []
def place(n, rmin, rmax, tries=4000):
    for _ in range(tries):
        if n == 0: break
        cx, cy = rng.uniform(0, 1, 2); r = rng.uniform(rmin, rmax)
        ok = True
        for (ax, ay, ar) in crs:
            dx = min(abs(cx - ax), 1 - abs(cx - ax)); dy = min(abs(cy - ay), 1 - abs(cy - ay))
            if np.hypot(dx, dy) < (r + ar) * 0.95: ok = False; break
        if ok: crs.append((cx, cy, r)); n -= 1
place(4, 0.11, 0.15)
place(7, 0.055, 0.08)
place(10, 0.03, 0.042)
shade = np.zeros((N, N)); floor = np.zeros((N, N))
for (cx, cy, r) in crs:
    dx = (x - cx + 0.5) % 1 - 0.5; dy = (y - cy + 0.5) % 1 - 0.5
    d = np.hypot(dx, dy) / r
    nx = dx / (np.hypot(dx, dy) + 1e-9); ny = dy / (np.hypot(dx, dy) + 1e-9)
    facing = nx * L[0] + ny * L[1]  # +1 = rim side facing the light? (outer normal toward light)
    # floor: dark bowl inside 0.82
    fl = np.clip((0.86 - d) / 0.08, 0, 1)
    floor = np.maximum(floor, fl)
    # floor gradient: side away from light catches light
    shade += fl * (-facing) * d * 0.35
    if r > 0.1:
        pk = np.clip(1 - d / 0.16, 0, 1)
        shade += pk * (0.4 + 0.6 * facing) * 0.9 - np.clip(1 - np.abs(d-0.17)/0.06,0,1)*np.clip(-facing,0,1)*0.5
    # inner wall: far side (opposite light) lit, near side in shadow
    wall = np.clip(1 - np.abs(d - 0.9) / 0.14, 0, 1)
    shade += wall * (-facing) * 1.0
    # outer rim ring: bright crest
    rim = np.clip(1 - np.abs(d - 1.03) / 0.08, 0, 1)
    shade += rim * (0.55 + 0.45 * facing) * 0.9
    # ejecta halo slightly lighter
    ej = np.clip(1 - (d - 1.1) / 0.6, 0, 1) * (d > 1.1)
    shade += ej * 0.12
floorcol = np.array([0.12, 0.02, 0.022])
col = col * (1 - 0.85 * floor[..., None]) + floorcol * 0.85 * floor[..., None]
lit = np.array([0.56, 0.17, 0.07])   # rust-orange lit rim
dark = np.array([0.06, 0.01, 0.01])
s = np.clip(shade, -1, 1)[..., None]
col = np.where(s > 0, col * (1 - s) + lit * s, col * (1 + s) + dark * (-s))
col = np.clip(col * 0.76, 0, 1)
Image.fromarray((col * 255).astype(np.uint8)).save(D + '/paint.png')

# --- effect: thin dark-red cloud wisps, mostly black; stretched along u
w = np.zeros((N, N))
for k, (seed, th) in enumerate([(50, 0.0), (60, 0.0)]):
    n1 = fnoise(1, 3, seed, aniso=(1, 1))
    n2 = fnoise(2, 5, seed + 5)
    # streaks: bands in v warped by low noise -> wisps
    ph = np.pi * (3 * y + 0.6 * n1 + 0.25 * n2 + k * 0.5)
    band = np.abs(np.sin(ph))
    thin = np.clip(1 - band / 0.28, 0, 1) ** 1.2 + 0.3 * np.clip(1 - band / 0.6, 0, 1) ** 2
    mod = np.clip((fbm(seed + 9, 3, 1) - 0.4) / 0.35, 0, 1) * (0.55 + 0.45 * fbm(seed + 20, 2, 4))
    w += thin * mod * (0.7 if k else 1.0)
w = np.clip(w, 0, 1)
fxc = np.array([1.0, 0.21, 0.10])
fx = w[..., None] * fxc
Image.fromarray((np.clip(fx, 0, 1) * 255).astype(np.uint8)).save(D + '/fx.png')
print('craters', len(crs))

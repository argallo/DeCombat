# Disco Fever (UZI): mirror-tile disco ball paint + sparse coloured sparkles as effect light. Procedural only.
import numpy as np, os, sys
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512
T = int(sys.argv[1]) if len(sys.argv) > 1 else 32     # tiles across (512/T px each)
G = 2                                                 # grout px
rng = np.random.default_rng(1977)
s = N // T
y, x = np.mgrid[0:N, 0:N]
ty, tx = y // s, x // s
ly, lx = (y % s) / s, (x % s) / s

silver = np.array([0.46, 0.52, 0.66])
blue   = np.array([0.28, 0.42, 0.74])
violet = np.array([0.50, 0.28, 0.70])
pal = [silver, blue, violet]
tint = np.zeros((T, T, 3)); bright = rng.uniform(0.82, 1.18, (T, T))
for i in range(T):
    for j in range(T):
        a, b = rng.choice(3, 2, replace=False); m = rng.uniform(0, 1)
        tint[i, j] = pal[a] * (1 - m) + pal[b] * m
# a facet: each tile tilted differently -> a gradient across it
ang = rng.uniform(0, 2*np.pi, (T, T)); slope = rng.uniform(0.10, 0.28, (T, T))
g = (np.cos(ang[ty, tx]) * (lx - 0.5) + np.sin(ang[ty, tx]) * (ly - 0.5)) * 2 * slope[ty, tx]
col = tint[ty, tx] * (bright[ty, tx] + g)[..., None]
# a few tiles catch a bright reflection (corner highlight)
hl = rng.uniform(0, 1, (T, T)) < 0.10
hd = np.clip(1 - np.hypot(lx - 0.28, ly - 0.28) / 0.45, 0, 1) * hl[ty, tx]
col = col + (np.array([0.78, 0.80, 0.86]) - col) * (hd ** 1.5)[..., None] * 0.7
gm = ((y % s) < G) | ((x % s) < G)
col[gm] = [0.14, 0.13, 0.17]
col = np.clip(col * 0.88, 0, 1)   # a touch darker so the sparkle light reads
Image.fromarray((col * 255).astype(np.uint8)).save(os.path.join(D, 'paint.png'))

# effect: black with sparse small 4-point star sparkles, wrapped (tileable)
fx = np.zeros((N, N, 3))
cols = [np.array([1.0, 0.12, 0.85]), np.array([0.05, 0.95, 1.0]), np.array([1.0, 0.78, 0.10])]
n = int(sys.argv[2]) if len(sys.argv) > 2 else 26
for k in range(n):
    cx, cy = rng.uniform(0, N, 2); r = rng.uniform(28, 42); c = cols[k % 3]
    dx = (x - cx + N/2) % N - N/2; dy = (y - cy + N/2) % N - N/2
    core = np.exp(-(dx**2 + dy**2) / (2 * (r * 0.26)**2))
    L = r * 1.3; w = r * 0.13
    arm = np.exp(-np.abs(dx) / w) * np.clip(1 - np.abs(dy)/L, 0, 1)**1.5 + \
          np.exp(-np.abs(dy) / w) * np.clip(1 - np.abs(dx)/L, 0, 1)**1.5
    du, dv = (dx + dy) / 1.414, (dx - dy) / 1.414
    diag = np.exp(-np.abs(du) / (w*0.8)) * np.clip(1 - np.abs(dv)/(L*0.5), 0, 1)**1.5 + \
           np.exp(-np.abs(dv) / (w*0.8)) * np.clip(1 - np.abs(du)/(L*0.5), 0, 1)**1.5
    v = np.clip(1.2 * core + 0.9 * arm + 0.5 * diag, 0, 1)
    fx = np.maximum(fx, v[..., None] * c)
Image.fromarray((np.clip(fx, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, 'fx.png'))
print('ok', T, n)

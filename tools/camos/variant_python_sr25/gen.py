# Python camo for the SR25: chain of dark olive/black blotches outlined in acid yellow over lime, with scale texture.
# Procedural, tileable on the torus (all noise from FFT of a periodic grid, all distances wrapped).
import os, numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(25)
y, x = np.mgrid[0:N, 0:N] / N

def pnoise(cut, seed):
    r = np.random.default_rng(seed)
    f = np.fft.fftfreq(N) * N
    k = np.sqrt(f[None, :] ** 2 + f[:, None] ** 2)
    spec = (r.normal(size=(N, N)) + 1j * r.normal(size=(N, N))) * np.exp(-(k / cut) ** 2)
    spec[0, 0] = 0
    n = np.real(np.fft.ifft2(spec))
    return n / n.std()

# domain warp for organic blotch edges
wx = pnoise(5, 1) * 0.035 + pnoise(14, 2) * 0.008
wy = pnoise(5, 3) * 0.035 + pnoise(14, 4) * 0.008
X, Y = (x + wx) % 1, (y + wy) % 1

# jittered grid of blotch centres (few, big), stretched slightly into saddles
G = 4
pts = []
for i in range(G):
    for j in range(G):
        pts.append(((i + 0.5 + rng.uniform(-.3, .3)) / G, (j + 0.5 + rng.uniform(-.3, .3)) / G))
pts = np.array(pts)
d1 = np.full((N, N), 9.0); d2 = np.full((N, N), 9.0)
for px, py in pts:
    dx = (X - px + .5) % 1 - .5
    dy = (Y - py + .5) % 1 - .5
    d = np.sqrt((dx * 1.25) ** 2 + (dy * 0.85) ** 2)
    m = d < d1
    d2 = np.where(m, d1, np.minimum(d2, d))
    d1 = np.where(m, d, d1)
edge = d2 - d1            # 0 on cell borders, large deep inside cells

# chain links: thin ground gaps that pinch shut here and there so blotches join into a chain
gap = 0.052 + 0.024 * pnoise(3, 7)
gap = np.clip(gap, 0.008, 0.08)
outline = 0.026
inner = edge - gap        # >0 inside blotch

# scale texture: offset rows of round scales (hex-ish), tileable, 24 x 32 per tile
sc_u, sc_v = 24, 32
su = x * sc_u; sv = y * sc_v
row = np.floor(sv)
su2 = su + 0.5 * (row % 2)
fu = su2 - np.floor(su2) - .5
fv = sv - row - .3
sd = np.sqrt(fu ** 2 + (fv * 1.0) ** 2)
scale = np.clip(1 - sd * 1.6, 0, 1)            # bright centre of each scale
sline = np.clip((sd - 0.42) * 8, 0, 1)        # dark rim between scales

C = lambda r, g, b: np.array([r, g, b], float)
lime = C(.30, .46, .07); lime2 = C(.23, .37, .05)
acid = C(.60, .58, .03)
olive = C(.25, .28, .08); olive2 = C(.19, .22, .06); black = C(.04, .05, .02)

ground = lime2 + (lime - lime2) * np.clip(0.5 + 0.25 * pnoise(6, 9), 0, 1)[..., None]
img = ground.copy()
# yellow outline band
ob = (inner > -outline) & (inner <= 0)
img[ob] = acid
# blotch interior: black ring just inside the yellow, then a mottled olive fill
ring = 0.020
fill = olive2 + (olive - olive2) * np.clip(0.5 + 0.3 * pnoise(10, 11), 0, 1)[..., None]
blot = np.where((inner < ring)[..., None], black, fill)
img = np.where((inner > 0)[..., None], blot, img)

# apply scale texture everywhere: lift scale centres, darken rims
lum = 1 + 0.14 * scale[..., None] - 0.22 * sline[..., None]
img = np.clip(img * lum, 0, 1)
Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))
t2 = np.tile((img * 255).astype(np.uint8), (2, 2, 1))
Image.fromarray(t2).resize((512, 512)).save(os.path.join(D, "tile_check.png"))
print("ok", (inner > 0).mean(), ob.mean())

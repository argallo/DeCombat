# Royal Marble: amethyst / black marble with gold veins. Procedural, tileable
# (FFT-filtered noise is periodic; band phases use whole-number frequencies).
import numpy as np, sys
from PIL import Image
N = 512
fy = np.fft.fftfreq(N)[:, None] * N
fx = np.fft.fftfreq(N)[None, :] * N
R = np.sqrt(fx**2 + fy**2)

def lp(scale, seed):
    r = np.random.default_rng(seed)
    spec = np.fft.fft2(r.normal(size=(N, N)))
    f = np.real(np.fft.ifft2(spec * np.exp(-(R / scale) ** 2)))
    return (f - f.mean()) / f.std()

def fbm(seed, base=2.5, octaves=4):
    out = np.zeros((N, N)); a = 1.0; tot = 0
    for o in range(octaves):
        out += a * lp(base * 2 ** o, seed + o * 101); tot += a; a *= 0.5
    return out / tot

y, x = np.mgrid[0:N, 0:N] / N
def sample(f, u, v):  # bilinear, wrapping
    fu = u * N; fv = v * N
    j0 = np.floor(fu).astype(int); i0 = np.floor(fv).astype(int)
    a = fu - j0; b = fv - i0
    j0 %= N; i0 %= N; j1 = (j0 + 1) % N; i1 = (i0 + 1) % N
    return (f[i0, j0] * (1 - a) * (1 - b) + f[i0, j1] * a * (1 - b)
            + f[i1, j0] * (1 - a) * b + f[i1, j1] * a * b)

W = 0.06
xs = (x + W * fbm(1)) % 1
ys = (y + W * fbm(7)) % 1
turb = sample(fbm(20), xs, ys)

vw = float(sys.argv[1]) if len(sys.argv) > 1 else 5.0
amethyst = np.array([0.40, 0.13, 0.56])
deep     = np.array([0.24, 0.06, 0.36])
black    = np.array([0.06, 0.03, 0.09])
lilac    = np.array([0.50, 0.26, 0.62])
gold     = np.array([0.76, 0.54, 0.14])

band = 0.5 + 0.5 * np.sin(2 * np.pi * (2 * x + 3 * y) + 2.2 * turb)
t = band[..., None]
col = deep * (1 - t) + amethyst * t
crest = (np.clip((band - 0.85) / 0.15, 0, 1) ** 1.5)[..., None]
col = col * (1 - crest * 0.6) + lilac * crest * 0.6
band2 = 0.5 + 0.5 * np.sin(2 * np.pi * (-2 * x + 1 * y) + 2.6 * sample(fbm(40), xs, ys))
blk = (np.clip((band2 - 0.62) / 0.22, 0, 1) ** 1.2)[..., None]
col = col * (1 - blk * 0.92) + black * blk * 0.92

# gold veins: zero lines of warped sine fields
vf = np.sin(np.pi * (1 * x - 2 * y) * 2 + 2.4 * sample(fbm(60), xs, ys))
def grad(f):
    gx = (np.roll(f, -1, 1) - np.roll(f, 1, 1)) / 2; gy = (np.roll(f, -1, 0) - np.roll(f, 1, 0)) / 2
    return np.sqrt(gx**2 + gy**2) + 1e-4
def line(f, px):  # soft line of ~px pixels half-width along zero set of f
    d = np.abs(f) / np.maximum(grad(f), 0.004)
    return np.clip(1.3 - d / px, 0, 1) ** 0.8
vein = line(vf, vw)
vf2 = np.sin(2 * np.pi * (3 * x + 1 * y) + 2.8 * sample(fbm(80), xs, ys))
vein2 = line(vf2, vw * 0.45)
mask2 = np.clip(0.5 + 0.7 * lp(2, 90), 0, 1)
v = np.clip(np.maximum(vein, vein2 * mask2 * 0.9), 0, 1)[..., None]
col = col * (1 - v) + gold * v
img = (np.clip(col, 0, 1) * 255).astype(np.uint8)
Image.fromarray(img).save(sys.argv[2] if len(sys.argv) > 2 else 'paint.png')

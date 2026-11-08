"""The art of the SCAR-L Prism camo (examples/mods/variant_prism_scar): two tileable 512x512 PNGs, drawn here from noise,
with no game pixels in them.
paint:  broad rainbow smears banded like oil on water over most of the gun, with violet chrome in the gaps.
effect: the same smears on black (cyan, green, gold, orange, pink, blue), for a light effect (fx, VAR-4).
Every field uses whole-number frequencies on the torus, so both tile.

    python3 tools/camo_prism.py <out prefix> [seed, default 7]     (needs numpy and Pillow)

writes <prefix>_paint.png and <prefix>_fx.png. The pack's prism.dtx is the paint through dc-gunkit: set_pattern, set_flow
drift 0.03/0.01, swirl 0.1, 8 seconds (VAR-12), then save_pack with row 505. The effect picture is for a light effect
(set_effect with layer effect, VAR-4), which a flowing camo does not use."""
import sys
import numpy as np
from PIL import Image
N = 512
u, v = np.meshgrid(np.arange(N) / N, np.arange(N) / N)
TAU = 2 * np.pi
rng = np.random.default_rng(int(sys.argv[2]) if len(sys.argv) > 2 else 7)

def field(u, v, terms):
    out = np.zeros_like(u)
    for m, n, a in terms:
        out += a * np.sin(TAU * (m * u + n * v) + rng.uniform(0, TAU))
    return out

def warped(u, v, strength, terms, wterms):
    wu = field(u, v, wterms); wv = field(u, v, wterms)
    return field(u + strength * wu, v + strength * wv, terms)

def smooth(e0, e1, x):
    t = np.clip((x - e0) / (e1 - e0), 0, 1)
    return t * t * (3 - 2 * t)

STOPS = np.array([[0.10, 0.80, 1.00],   # cyan
                  [0.30, 0.95, 0.45],   # green
                  [1.00, 0.88, 0.20],   # gold
                  [1.00, 0.50, 0.15],   # orange
                  [1.00, 0.30, 0.70],   # pink
                  [0.40, 0.35, 1.00],   # blue
                  [0.10, 0.80, 1.00]])  # back to cyan

def palette(t):
    """A full rainbow: cyan, green, gold, orange, pink, blue, around and around."""
    t = (t % 1.0) * (len(STOPS) - 1)
    i = np.floor(t).astype(int); w = (t - i)[..., None]
    w = w * w * (3 - 2 * w)
    return STOPS[i] * (1 - w) + STOPS[np.minimum(i + 1, len(STOPS) - 1)] * w

flow = [(1, 1, 1.0), (2, -1, 0.6), (1, 3, 0.35), (3, 1, 0.25), (-2, 3, 0.2)]
warp = [(1, 0, 0.5), (0, 1, 0.5), (1, 2, 0.3), (2, 1, 0.25)]
f = warped(u, v, 0.18, flow, warp)            # the liquid's main sweep
g = warped(u, v, 0.25, flow[::-1], warp)      # a second, for the streaks
f = (f - f.min()) / (f.max() - f.min())
g = (g - g.min()) / (g.max() - g.min())

# chrome: violet, from deep to bright lilac in sweeping bands
sweep = 0.5 + 0.5 * np.sin(TAU * 2 * f)
deep = np.array([0.11, 0.03, 0.28]); lilac = np.array([0.42, 0.20, 0.74])
chrome = deep + (lilac - deep) * (sweep[..., None] ** 1.7)

# patches: broad smears where the second field is high, banded inside like oil on water
patch = smooth(0.28, 0.48, g)
hue = palette(g * 2.8 + 0.25 * f)
paint = chrome * (1 - 0.8 * patch[..., None]) + hue * 0.58 * patch[..., None]

# effect: the same smears, a little wider, on black; the game drifts them over the gun as light
wide = smooth(0.24, 0.48, g)
fx = palette(g * 2.8 + 0.25 * f + 0.1) * wide[..., None]

out = sys.argv[1]
Image.fromarray((np.clip(paint, 0, 1) * 255).astype(np.uint8)).save(out + "_paint.png")
Image.fromarray((np.clip(fx, 0, 1) * 255).astype(np.uint8)).save(out + "_fx.png")
print("wrote", out + "_paint.png", out + "_fx.png")

# Macaw: overlapping tropical parrot feathers, procedural and tileable (512 torus).
import os, numpy as np
from PIL import Image
N = 512
d = os.path.dirname(os.path.abspath(__file__))
rng = np.random.default_rng(552)
y, x = np.mgrid[0:N, 0:N].astype(np.float32)

SCARLET = np.array([0.50, 0.05, 0.04]); CRIMSON = np.array([0.34, 0.02, 0.05])
COBALT = np.array([0.05, 0.16, 0.55]); DEEPBLUE = np.array([0.04, 0.07, 0.34])
YELLOW = np.array([0.54, 0.36, 0.02]); GREEN = np.array([0.06, 0.32, 0.10])
# (base colour, tip colour) feather kinds
KINDS = [(SCARLET, CRIMSON), (SCARLET, COBALT), (YELLOW, GREEN), (COBALT, DEEPBLUE),
         (SCARLET, SCARLET), (YELLOW, YELLOW), (GREEN, COBALT)]
WEIGHTS = np.array([2.4, 2.6, 2.0, 2.4, 1.2, 1.0, 1.2]); WEIGHTS /= WEIGHTS.sum()

COLS, ROWS = 5, 4
L, WMAX = 300.0, 58.0
# balanced colours: scarlet, cobalt and yellow in equal measure, green as accents
COUNTS = [4, 4, 4, 4, 0, 2, 2]
ORDER = rng.permutation(np.repeat(np.arange(len(KINDS)), COUNTS))
best = np.full((N, N), -9.0, np.float32)
out = np.zeros((N, N, 3), np.float32)
for r in range(ROWS):
    for c in range(COLS):
        cx = (c + 0.5 * (r % 2)) * N / COLS + rng.uniform(-8, 8)
        cy = r * N / ROWS + rng.uniform(-6, 6)
        ang = np.radians(rng.uniform(-9, 9))
        base, tip = KINDS[ORDER[r * COLS + c]]
        dx = ((x - cx + N / 2) % N) - N / 2
        dy = ((y - cy + 96) % N) - 96
        along = dy * np.cos(ang) + dx * np.sin(ang)
        s = dx * np.cos(ang) - dy * np.sin(ang)
        t = along / L
        tc = np.clip(t, 0, 1)
        w = WMAX * np.sqrt(np.clip(np.sin(np.pi * tc ** 0.72), 0, 1))
        inside = (t > 0) & (t < 1) & (np.abs(s) < w)
        prio = t + rng.uniform(0, 0.08)
        win = inside & (prio > best)
        # colour: base -> tip gradient along the feather
        g = np.clip((tc - 0.72) / 0.12, 0, 1); g = (g * g * (3 - 2 * g))[..., None]
        col = base * (1 - g) + tip * g
        rel = np.abs(s) / np.maximum(w, 1)
        # barbs: fine chevrons pointing to the tip
        phase = along - np.abs(s) * 0.85
        barb = 0.78 + 0.22 * np.cos(2 * np.pi * phase / 9.0)
        shade = barb * (0.78 + 0.32 * tc)          # darker toward the hidden base
        shade = shade * np.where(rel > 0.78, 0.32, 1.0)   # dark outline
        shade = shade * (1.0 - 0.18 * rel ** 2)    # rounded vane
        shade = shade * np.where(s > 0, 0.82, 1.0)   # the two vanes catch light differently
        # splits: a couple of frayed gaps in the vane, following the barbs
        for k in range(2):
            sp = rng.uniform(0.62, 0.9) * L
            side = rng.choice([-1, 1])
            split = (side * s > 0) & (np.abs(phase - sp) < 1.8) & (rel > 0.2)
            shade = np.where(split, shade * 0.35, shade)
        col = col * shade[..., None]
        shaft = np.abs(s) < (3.2 - 1.8 * tc)
        col = np.where(shaft[..., None], col * 0.55 + np.array([0.46, 0.40, 0.30]) * 0.45, col)
        out[win] = col[win]
        best[win] = prio[win]
# gaps (none expected) get deep blue
out[best < -1] = DEEPBLUE
img = (np.clip(out, 0, 1) * 255).astype(np.uint8)
Image.fromarray(img).save(os.path.join(d, "paint.png"))
Image.fromarray(np.tile(img, (2, 2, 1))).save(os.path.join(d, "tile_check.png"))

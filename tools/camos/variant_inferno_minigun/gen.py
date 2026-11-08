# Inferno camo for the M134: procedural, tileable flame tongues over a char base.
import numpy as np
from PIL import Image
import os
N = 512
rng = np.random.default_rng(1349)
y, x = np.mgrid[0:N, 0:N] / N          # 0..1 on the torus
TAU = 2*np.pi

def fbm(octaves, base=2, amp_fall=0.55, seed=0):
    r = np.random.default_rng(seed)
    out = np.zeros((N, N)); a = 1.0; tot = 0
    for o in range(octaves):
        f = base * (2**o)
        for _ in range(4):
            kx = int(r.integers(-f, f+1)); ky = int(r.integers(-f, f+1))
            if kx == 0 and ky == 0: kx = f
            out += a*np.sin(TAU*(kx*x + ky*y) + r.uniform(0, TAU))
            tot += a
        a *= amp_fall
    return out / tot * 2.2

# domain warp (periodic)
wx = fbm(4, 2, seed=11) * 0.035
wy = fbm(4, 2, seed=12) * 0.03
xw = (x + wx) % 1.0
yw = (y + wy) % 1.0

ROWS = 3           # flame rows per tile
heat = np.zeros((N, N)); char = np.zeros((N, N)); ember = np.zeros((N, N))
for r_i in range(ROWS):
    xs = (xw + r_i * 0.37) % 1.0
    # tongues lick sideways more the higher they climb
    xs = (xs + 0.018*np.sin(TAU*(ROWS*2*yw) + TAU*(2*xs) + r_i)) % 1.0
    # wavy ground line for this row (integer frequencies -> tileable)
    yl = (r_i + 1) / ROWS + 0.025*np.sin(TAU*(3*xs) + r_i) + 0.012*np.sin(TAU*(8*xs) + 2*r_i)
    a = ((yl - yw) % 1.0) * ROWS          # 0 at the ground line, grows upward, wraps at ROWS
    # tongue heights: a few big tongues, many small ones
    big = np.abs(np.sin(TAU*(4*xs) + r_i*1.7)) ** 1.5
    mid = np.abs(np.sin(TAU*(11*xs) + 0.6 + r_i)) ** 2
    sml = np.abs(np.sin(TAU*(23*xs) + 1.9 + 2*r_i))
    h = 0.10 + 0.55*big + 0.22*mid + 0.08*sml + 0.06*fbm(3, 4, seed=30 + r_i)
    h = np.clip(h, 0.08, 0.85)
    v = np.clip(1.0 - a / h, 0, 1)
    heat = np.maximum(heat, v)
    # char ground just below the line
    c = 0.24 + 0.05*np.sin(TAU*(5*xs) + r_i)
    below = ROWS - a                       # distance below the line
    inchar = (below > 0) & (below < c)
    char = np.maximum(char, inchar.astype(float))
    # embers glowing at the top lip of the char
    ember = np.maximum(ember, np.where(inchar, np.clip(1 - below / 0.07, 0, 1), 0))
# char occludes the flames of the row below; a thin ember lip glows on it
heat = np.where(char > 0, 0.55 * ember, heat)

# flicker texture inside the flames
heat = heat * (0.85 + 0.15*fbm(3, 6, seed=77))
heat = np.clip(heat, 0, 1) ** 1.3

# colour ramp: char -> ember red -> red-orange -> orange -> yellow (never white)
stops = np.array([0.00, 0.12, 0.30, 0.52, 0.75, 1.00])
cols = np.array([
    [0.07, 0.035, 0.03],   # char
    [0.20, 0.04, 0.02],    # smoulder
    [0.58, 0.07, 0.02],    # deep red
    [0.78, 0.24, 0.02],    # red-orange
    [0.82, 0.46, 0.03],    # orange
    [0.82, 0.70, 0.10],    # saturated yellow
])
img = np.zeros((N, N, 3))
for c in range(3):
    img[..., c] = np.interp(heat, stops, cols[:, c])
# a little char grain in the base
grain = fbm(2, 24, seed=5)
img += (heat < 0.1)[..., None] * grain[..., None] * 0.015
img = np.clip(img, 0, 1)
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'paint.png')
Image.fromarray((img*255).astype(np.uint8)).save(out)
print('wrote', out, img.mean(axis=(0,1)))

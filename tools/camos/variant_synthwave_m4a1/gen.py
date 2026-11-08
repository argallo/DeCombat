# Synthwave camo for the M4A1: procedural, tileable, original art only.
# Tile = 3 x 3 cells (each row shifted a third). Each cell is a tiny 80s scene: banded sunset sky,
# a striped half-sun on a cyan horizon, and a neon grid floor below.
import numpy as np
from PIL import Image
import os
D = os.path.dirname(os.path.abspath(__file__))
N = 512
CELLS = 3
y, x = np.mgrid[0:N, 0:N].astype(np.float32)
u = x / N; v = y / N
row = np.floor(v * CELLS).astype(int)
cv = (v * CELLS) % 1.0
cu = (u * CELLS + row / 3.0) % 1.0     # shift a third per row; 3 rows wrap to a whole cell

H = 0.64   # horizon inside a cell
def over(img, col, a):
    a = np.clip(a, 0, 1)[..., None]
    return img * (1 - a) + np.asarray(col, np.float32) * a

# Sky: hard posterised bands, indigo at the top to hot pink at the horizon
sky = np.array([
    [0.15, 0.06, 0.34],
    [0.26, 0.07, 0.46],
    [0.40, 0.08, 0.52],
    [0.55, 0.09, 0.50],
    [0.68, 0.13, 0.42],
], np.float32)
si = np.clip(np.floor(cv / H * len(sky)).astype(int), 0, len(sky) - 1)
img = sky[si].copy()

# Sun: half disc on the horizon, yellow-orange on top grading to magenta, with slits that thicken downward
R = 0.37
dx = (cu - 0.5); dy = (cv - H)
r = np.sqrt(dx * dx + (dy * 1.0) ** 2)
above = cv < H
s = np.clip((cv - (H - R)) / R, 0, 1)        # 0 at sun top, 1 at horizon
sun_col = (np.array([0.80, 0.56, 0.12])[None, None, :] * (1 - s[..., None])
           + np.array([0.78, 0.18, 0.40])[None, None, :] * s[..., None])
sun_a = np.clip((R - r) * 110, 0, 1) * above
slit_phase = ((H - cv) / R * 6.0) % 1.0           # 6 stripes from horizon up
slit_w = 0.42 * s ** 1.3
slit = (slit_phase < slit_w) & (s > 0.35)
sun_a = sun_a * (~slit)
img = img * (1 - sun_a[..., None]) + sun_col * sun_a[..., None]
# soft halo around the sun (in paint, mid-tone only)
halo = np.clip(1 - (r - R) / 0.12, 0, 1) * (r > R) * above
img = over(img, [0.70, 0.22, 0.45], halo * 0.35)

# Ground: dark indigo with a neon grid (straight, no perspective)
ground = ~above
img[ground] = np.array([0.22, 0.06, 0.40])
def line(coord, cells, width):
    d = 0.5 - np.abs(((coord * cells) % 1.0) - 0.5)
    return np.clip(1 - d / width, 0, 1)
g = (cv - H) / (1 - H)                    # 0..1 down the ground
gy = line(g, 3, 0.10) * ground            # pink horizontals
gx = line(cu + 0.5, 4, 0.10) * ground     # cyan verticals, one passing under the sun's centre
img = over(img, [0.85, 0.22, 0.66], gy ** 1.5 * 0.6 + line(g, 3, 0.04) * ground)
img = over(img, [0.14, 0.68, 0.80], gx ** 1.5 * 0.6 + line(cu + 0.5, 4, 0.045) * ground)
# Bright cyan horizon line
hz = np.clip(1 - np.abs(cv - H) / 0.018, 0, 1)
img = over(img, [0.40, 0.85, 0.90], hz)
img = over(img, [0.14, 0.62, 0.78], np.clip(1 - np.abs(cv - H) / 0.05, 0, 1) * 0.5)

img = np.clip(img, 0, 1)
Image.fromarray((img * 255 + 0.5).astype(np.uint8)).save(os.path.join(D, 'paint.png'))

# Effect light: mostly black. Thin cyan scanlines inside a sweeping diagonal band, plus a pink glow
# along the horizon lines inside a second band. The studio drifts it slowly down the gun.
V = v
fx = np.zeros((N, N, 3), np.float32)
scan = np.clip(1 - np.abs(((V * 32) % 1.0) - 0.5) / 0.5, 0, 1) ** 14
band = (0.5 + 0.5 * np.sin(2 * np.pi * (V * 2 + u * 1))) ** 6
fx += scan[..., None] * band[..., None] * np.array([0.25, 0.85, 1.0])
hzg = np.clip(1 - np.abs(cv - H) / 0.06, 0, 1) ** 2
band2 = (0.5 + 0.5 * np.sin(2 * np.pi * (u * 2 - V * 1))) ** 8
fx += hzg[..., None] * band2[..., None] * np.array([1.0, 0.30, 0.85])
fx = np.clip(fx, 0, 1)
Image.fromarray((fx * 255 + 0.5).astype(np.uint8)).save(os.path.join(D, 'fx.png'))
print('ok')

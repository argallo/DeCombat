# Gummy Bears camo: procedural, tileable 512x512. Original art (numpy + Pillow only).
import os, numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
N = 512
rng = np.random.default_rng(7)
yy, xx = np.mgrid[0:N, 0:N].astype(np.float32) + 0.5

PAL = {  # base, rim (darker translucent edge)
    "red":    ((0.62, 0.08, 0.10), (0.30, 0.02, 0.05)),
    "orange": ((0.62, 0.22, 0.03), (0.32, 0.07, 0.02)),
    "green":  ((0.22, 0.56, 0.10), (0.06, 0.25, 0.04)),
    "yellow": ((0.56, 0.40, 0.03), (0.32, 0.17, 0.02)),
    "purple": ((0.40, 0.12, 0.52), (0.17, 0.04, 0.26)),
}
ORDER = ["red", "orange", "green", "yellow", "purple"]

def smin(a, b, k=0.035):
    h = np.clip(0.5 + 0.5 * (b - a) / k, 0, 1)
    return b * (1 - h) + a * h - k * h * (1 - h)

def ell(px, py, cx, cy, rx, ry):
    qx, qy = (px - cx) / rx, (py - cy) / ry
    return (np.sqrt(qx * qx + qy * qy) - 1.0) * min(rx, ry)

def bear_sdf(px, py):
    d = ell(px, py, 0, -0.28, 0.215, 0.195)                 # head
    d = smin(d, ell(px, py, -0.165, -0.44, 0.092, 0.092))  # ears
    d = smin(d, ell(px, py, 0.165, -0.44, 0.092, 0.092))
    d = smin(d, ell(px, py, 0, 0.11, 0.215, 0.255), 0.06)  # body
    for s in (-1, 1):                                      # arms, angled out
        ax, ay = px - s * 0.215, py - 0.01
        c, sn = np.cos(s * 0.6), np.sin(s * 0.6)
        rx_, ry_ = c * ax + sn * ay, -sn * ax + c * ay
        d = smin(d, ell(rx_, ry_, 0, 0, 0.075, 0.125))
        d = smin(d, ell(px, py, s * 0.14, 0.355, 0.105, 0.095))  # legs
    return d

# placement: jittered 4x4 grid, two layers so the pile overlaps
bears = []
for gy in range(3):
    for gx in range(3):
        cx = (gx + 0.5 + rng.uniform(-0.15, 0.15)) * N / 3
        cy = (gy + 0.5 + rng.uniform(-0.15, 0.15)) * N / 3
        bears.append((cx % N, cy % N, rng.uniform(0, 2 * np.pi), rng.uniform(215, 240), (gx + 2 * gy) % 5, 1.0))
for gy in range(2):
    for gx in range(2):
        cx = (gx + 0.25 + rng.uniform(-0.08, 0.08)) * N / 2 + N / 6
        cy = (gy + 0.25 + rng.uniform(-0.08, 0.08)) * N / 2 + N / 6
        bears.append((cx % N, cy % N, rng.uniform(0, 2 * np.pi), rng.uniform(205, 230), (3 * gx + gy + 2) % 5, 1.0))
rng.shuffle(bears)
back = []
for gy in range(4):
    for gx in range(4):
        cx = (gx + 0.5 + rng.uniform(-0.25, 0.25)) * N / 4 + 31
        cy = (gy + 0.5 + rng.uniform(-0.25, 0.25)) * N / 4 + 17
        back.append((cx % N, cy % N, rng.uniform(0, 2 * np.pi), rng.uniform(165, 190), (2 * gx + 3 * gy + 1) % 5, 0.6))
bears = back + bears

bg = np.array((0.20, 0.07, 0.12), np.float32)
img = np.ones((N, N, 3), np.float32) * bg
# faint sugar mottling on the gaps
img *= (0.85 + 0.15 * np.sin(2 * np.pi * (3 * xx / N + 2 * yy / N)) * np.sin(2 * np.pi * 5 * yy / N))[..., None]

L = np.array((-0.45, -0.6, 0.66)); L /= np.linalg.norm(L)
for cx, cy, ang, size, ci, dim in bears:
    dx = ((xx - cx + N / 2) % N) - N / 2
    dy = ((yy - cy + N / 2) % N) - N / 2
    c, s = np.cos(ang), np.sin(ang)
    px, py = (c * dx + s * dy) / size, (-s * dx + c * dy) / size
    d = bear_sdf(px, py) * size                 # pixels, negative inside
    # drop shadow onto the pile below
    sx, sy = dx - 9, dy - 13
    spx, spy = (c * sx + s * sy) / size, (-s * sx + c * sy) / size
    ds = bear_sdf(spx, spy) * size
    sh = np.clip(1 - (ds + 8) / 18, 0, 1) * 0.45
    img *= (1 - sh)[..., None]
    a = np.clip(0.5 - d, 0, 1)
    inside = np.clip(-d, 0, None)
    # height field -> normal, for a soft rounded gummy shape
    h = np.sqrt(np.clip(inside / 34.0, 0, 1)) * 34.0
    gy_, gx_ = np.gradient(h)
    nx, ny, nz = -gx_, -gy_, np.ones_like(h)
    nl = np.sqrt(nx * nx + ny * ny + nz * nz)
    nx, ny, nz = nx / nl, ny / nl, nz / nl
    diff = np.clip(nx * L[0] + ny * L[1] + nz * L[2], 0, 1)
    Hh = L + np.array((0, 0, 1.0)); Hh /= np.linalg.norm(Hh)
    spec = np.clip(nx * Hh[0] + ny * Hh[1] + nz * Hh[2], 0, 1) ** 40
    base, rim = (np.array(v, np.float32) for v in PAL[ORDER[ci]])
    t = np.clip(inside / 22.0, 0, 1)[..., None]       # darker translucent edge
    col = rim * (1 - t) + base * t
    col = col * (0.62 + 0.5 * diff[..., None])
    # inner glow (light passing through the gummy)
    col += (base * 0.25) * np.clip((inside - 30) / 40, 0, 1)[..., None]
    # glossy highlight: specular plus a fixed blob on head and belly, in light direction
    hx, hy = px * size, py * size
    for gx0, gy0, gr in ((-0.08, -0.33, 0.05), (-0.08, 0.02, 0.06)):
        # place the blob towards the light in screen space
        bx = (c * gx0 - s * gy0) * size  # back to screen offsets
        by = (s * gx0 + c * gy0) * size
        # shift toward light
        bx2, by2 = bx - 0.0, by - 0.0
        q = np.sqrt((dx - bx2 + 0.35 * gr * size) ** 2 + (dy - by2 + 0.35 * gr * size) ** 2) / (gr * size)
        blob = np.clip(1.0 - q, 0, 1) ** 0.6 * (inside > 4)
        spec = np.maximum(spec, blob * 0.85)
    col = col + (np.array((0.92, 0.88, 0.85)) - col) * spec[..., None] * 0.8
    col = col * (0.55 + 0.45 * np.clip(inside / 2.5, 0, 1))[..., None]
    col = col * dim
    img = img * (1 - a[..., None]) + col * a[..., None]

# roll so the thin strip the first-person stock and barrel sample (v 384..448) crosses mixed colours
img = np.roll(img, 88, axis=0)
out = (np.clip(img, 0, 1) * 255 + 0.5).astype(np.uint8)
Image.fromarray(out).save(os.path.join(D, "paint.png"))
Image.fromarray(np.tile(out, (2, 2, 1))).resize((512, 512)).save(os.path.join(D, "tile_check.png"))
print("ok", out.mean(axis=(0, 1)))

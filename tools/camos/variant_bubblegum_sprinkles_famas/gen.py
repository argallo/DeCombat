# Bubblegum Sprinkles: procedural, tileable 512x512 paint. Original art.
import numpy as np
from PIL import Image
import os, sys
OUT = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 2; M = N * SS
rng = np.random.default_rng(512)
y, x = np.mgrid[0:M, 0:M].astype(np.float32) / M  # torus coords 0..1

def tor(fx, fy, ph):
    return np.sin(2*np.pi*(fx*x + fy*y) + ph)

# bubblegum base: saturated mid pink with soft glossy mottling (whole-number freqs -> tileable)
base = np.array([0.74, 0.22, 0.50], np.float32)
m = 0.5*tor(2,1,0.3) + 0.35*tor(-1,3,1.7) + 0.25*tor(4,-3,2.2)
m = m/1.1
img = np.empty((M, M, 3), np.float32)
for c in range(3):
    img[..., c] = base[c] * (1 + 0.08*m)
# glossy blobby highlights (soft, slight)
g = np.clip((0.4*tor(3,2,0.9)+0.4*tor(-2,5,2.5)+0.3*tor(6,1,4.0)) - 0.55, 0, None)
img += g[..., None] * np.array([0.30, 0.22, 0.24])

def wrapd(a, b):
    d = a - b
    return d - np.round(d)

# teal drips: a wobbly blob of glaze with hanging drops, all wrapped on the torus
teal = np.array([0.10, 0.56, 0.58], np.float32)
tealhl = np.array([0.45, 0.82, 0.80], np.float32)
centres = [(0.15, 0.28), (0.64, 0.10), (0.42, 0.62)]
for k, (x0, y0) in enumerate(centres):
    width = rng.uniform(0.22, 0.27)
    dx = wrapd(x, x0); dy = wrapd(y, y0)
    mask = np.zeros_like(x, bool)
    hl = np.zeros_like(x, bool)
    # puddle top made of overlapping circles
    nc = 7
    for c in range(nc):
        cx = -width/2 + width*c/(nc-1)
        r = rng.uniform(0.026, 0.034)
        cy = rng.uniform(-0.008, 0.008)
        mask |= ((dx-cx)**2 + (dy-cy)**2) < r*r
    mask |= (np.abs(dx) < width/2) & (dy > -0.018) & (dy < 0.016)
    for d in range(rng.integers(3, 5)):
        cx = rng.uniform(-width/2+0.015, width/2-0.015)
        L = rng.uniform(0.06, 0.15)
        r = rng.uniform(0.013, 0.018)
        # stem tapers a bit then a round bulb
        t = np.clip(dy/L, 0, 1)
        sw = r*(0.95 - 0.25*t)
        stem = (np.abs(dx-cx) < sw) & (dy > 0) & (dy < L)
        bulb = ((dx-cx)**2 + (dy-L)**2) < (r*1.3)**2
        mask |= stem | bulb
        hl |= ((dx-cx+r*0.45)**2 + (dy-L+r*0.45)**2) < (r*0.38)**2
    mask_s = np.roll(np.roll(mask, 5, 0), 3, 1) & ~mask  # soft shadow below
    img[mask_s] *= 0.78
    img[mask] = teal
    # gloss: lighter band along the top of the puddle and a dot on each bulb
    top = mask & ~np.roll(mask, 14, 0)
    img[top] = img[top]*0.55 + tealhl*0.45
    img[hl & mask] = tealhl

# sprinkles: rounded rods, rainbow, random angles, wrapped
cols = np.array([
    [0.92, 0.20, 0.22],  # red
    [0.95, 0.55, 0.12],  # orange
    [0.95, 0.85, 0.15],  # yellow
    [0.25, 0.75, 0.25],  # green
    [0.20, 0.45, 0.92],  # blue
    [0.55, 0.25, 0.85],  # purple
    [0.95, 0.92, 0.85],  # cream white (few)
], np.float32)
# min-distance placement
pts = []
tries = 0
while len(pts) < 140 and tries < 20000:
    tries += 1
    p = rng.uniform(0, 1, 2)
    ok = True
    for q in pts:
        d = p - q; d -= np.round(d)
        if d @ d < 0.055**2: ok = False; break
    if ok: pts.append(p)
for i, (px, py) in enumerate(pts):
    ang = rng.uniform(0, np.pi)
    L = rng.uniform(0.020, 0.028); r = 0.0068
    ci = rng.choice(7, p=[0.17,0.16,0.17,0.17,0.16,0.13,0.04])
    col = cols[ci]
    ux, uy = np.cos(ang), np.sin(ang)
    # local window for speed
    R = int((L + r + 0.01) * M) + 2
    cx, cy = int(px*M), int(py*M)
    ii = (np.arange(cy-R, cy+R) % M); jj = (np.arange(cx-R, cx+R) % M)
    sub_x = x[np.ix_(ii, jj)]; sub_y = y[np.ix_(ii, jj)]
    dx = wrapd(sub_x, px); dy = wrapd(sub_y, py)
    t = np.clip(dx*ux + dy*uy, -L, L)
    ex = dx - t*ux; ey = dy - t*uy
    dist = np.sqrt(ex**2 + ey**2)
    a = np.clip((r - dist) / (1.0/M) * 0.7, 0, 1)
    # shading across the rod: lighter on one side for gloss, darker edge
    side = (ex*(-uy) + ey*ux) / r  # -1..1
    shade = 1.0 - 0.35*np.clip(dist/r, 0, 1)**2
    shade = shade + 0.25*np.clip(-side - 0.2, 0, 1)
    sub = img[np.ix_(ii, jj)]
    # soft drop shadow
    sh = np.clip((r*1.6 - np.sqrt((ex-0.002)**2 + (ey-0.003)**2)) / (2.0/M), 0, 1) * 0.25
    sub = sub * (1 - sh[..., None])
    rod = np.clip(col[None, None, :] * shade[..., None], 0, 1)
    sub = sub*(1-a[...,None]) + rod*a[...,None]
    img[np.ix_(ii, jj)] = sub

img = np.clip(img, 0, 1)
im = Image.fromarray((img*255+0.5).astype(np.uint8)).resize((N, N), Image.LANCZOS)
im.save(os.path.join(OUT, 'paint.png'))
# tiling check
Image.fromarray(np.tile(np.asarray(im), (2, 2, 1))).save(os.path.join(OUT, 'tile_check.png'))
print('ok', len(pts))

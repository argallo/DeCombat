# Autumn Fall: procedural maple leaves over a forest floor, tileable 512x512.
import numpy as np, sys
from PIL import Image
N = 512
rng = np.random.default_rng(int(sys.argv[1]) if len(sys.argv) > 1 else 7)
out = sys.argv[2] if len(sys.argv) > 2 else __file__.replace('gen.py', 'paint.png')

yy, xx = np.mgrid[0:N, 0:N].astype(np.float32)

# forest floor: tileable low-frequency noise from whole-number torus waves
def tnoise(octaves, amp0=1.0):
    f = np.zeros((N, N), np.float32)
    for o in range(octaves):
        for _ in range(4):
            kx, ky = rng.integers(-(2 << o), (2 << o) + 1, 2)
            ph = rng.uniform(0, 2*np.pi)
            f += amp0 / (o+1) * np.cos(2*np.pi*(kx*xx + ky*yy)/N + ph)
    return (f - f.min()) / (f.max() - f.min() + 1e-6)
n1 = tnoise(4)
floor = np.stack([0.15 + 0.05*n1, 0.08 + 0.03*n1, 0.035 + 0.015*n1], -1)
img = floor.copy()

def lobe_r(th):
    # 5 pointed lobes: top (pi/2), upper sides, lower sides; stem gap at bottom
    tips = [(np.pi/2, 1.00, 0.62), (np.pi/2 - 1.05, 0.90, 0.58), (np.pi/2 + 1.05, 0.90, 0.58),
            (np.pi/2 - 2.05, 0.62, 0.48), (np.pi/2 + 2.05, 0.62, 0.48)]
    r = np.full_like(th, 0.40)
    for a, h, w in tips:
        d = np.angle(np.exp(1j*(th - a)))
        r = np.maximum(r, 0.40 + (h-0.40) * np.clip(1 - np.abs(d)/w, 0, 1)**1.05)
    r *= 1 + 0.06*np.abs(np.sin(14*th))  # serrated edge
    return r

palette = [np.array(c) for c in [(0.44, 0.06, 0.03), (0.50, 0.19, 0.03), (0.50, 0.35, 0.025),
                                 (0.36, 0.05, 0.04), (0.54, 0.25, 0.03)]]

count = 19
for i in range(count):
    cx, cy = rng.uniform(0, N, 2)
    R = rng.uniform(80, 106)
    rot = rng.uniform(0, 2*np.pi)
    col = palette[i % len(palette)] * rng.uniform(0.85, 1.08)
    # toroidal offsets
    dx = (xx - cx + N/2) % N - N/2
    dy = (yy - cy + N/2) % N - N/2
    c, s = np.cos(rot), np.sin(rot)
    lx = (c*dx + s*dy) / R
    ly = (-s*dx + c*dy) / R
    rr = np.hypot(lx, ly)
    th = np.arctan2(ly, lx)
    edge = lobe_r(th)
    inside = rr < edge
    # stem
    stem = (np.abs(lx) < 0.035) & (ly < 0) & (ly > -0.55)
    # veins: rays to lobe tips
    vein = np.zeros_like(rr, bool)
    for a in [np.pi/2, np.pi/2-1.05, np.pi/2+1.05, np.pi/2-2.05, np.pi/2+2.05]:
        d = np.angle(np.exp(1j*(th - a)))
        vein |= (np.abs(d)*rr < 0.028 + 0.01*(1-rr)) & (rr < edge*0.9)
    # side veins off the main top ray
    shade = 0.82 + 0.25*(1 - np.clip(rr/edge, 0, 1))  # lighter centre
    leafc = col[None, None, :] * shade[..., None]
    leafc = np.where(vein[..., None], leafc*0.42, leafc)
    # thin dark rim
    rim = inside & (rr > edge*0.93)
    leafc = np.where(rim[..., None], leafc*0.6, leafc)
    # drop shadow beneath on floor already painted
    sh = (np.hypot(lx-0.07, ly+0.07) < edge*1.08) & ~inside
    img = np.where(sh[..., None], img*0.55, img)
    img = np.where(inside[..., None], leafc, img)
    img = np.where(stem[..., None], np.array([0.30, 0.13, 0.05])[None, None], img)

img = np.clip(img, 0, 1)
Image.fromarray((img*255).astype(np.uint8)).save(out)
print('wrote', out)

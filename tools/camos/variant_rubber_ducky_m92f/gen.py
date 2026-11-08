# Rubber Ducky camo for the M92F SE: procedural, tileable 512x512.
import os, math
import numpy as np
from PIL import Image, ImageDraw

D = os.path.dirname(os.path.abspath(__file__))
N = 512; SS = 2; M = N * SS          # draw at 2x, downsample
rng = np.random.default_rng(7)

# palette (8-bit), kept mid-tone and saturated
WATER_A = np.array([8, 50, 112]); WATER_B = np.array([16, 74, 140])
YELLOW = (234, 162, 8); YELLOW_SH = (184, 104, 6); OUTLINE = (64, 28, 4)
BEAK = (200, 72, 6); BEAK_SH = (150, 44, 4)
EYE = (18, 14, 20); EYE_HI = (230, 230, 220)
BUB_RIM = (96, 160, 200); BUB_HI = (190, 220, 236)

# --- tileable water: integer-frequency ripples on the torus
y, x = np.mgrid[0:M, 0:M] / M
w = (np.sin(2*np.pi*(3*x + 2*y)) + np.sin(2*np.pi*(-2*x + 4*y) + 1.3)
     + 0.6*np.sin(2*np.pi*(5*x - y) + 2.1))
w = (w - w.min()) / (w.max() - w.min())
w = np.round(w * 3) / 3                                  # soft banded bath water
bg = WATER_A[None, None] * (1 - w[..., None]) + WATER_B[None, None] * w[..., None]
img = Image.fromarray(bg.astype(np.uint8), "RGB").convert("RGBA")

def duck_sprite(s):
    """A side-view rubber duck facing right, drawn in a (3s x 3s) RGBA box."""
    S = int(3 * s); im = Image.new("RGBA", (S, S), (0, 0, 0, 0)); d = ImageDraw.Draw(im)
    o = int(0.07 * s)                                    # outline width
    cx, cy = S * 0.45, S * 0.62
    body = [cx - 1.05*s, cy - 0.55*s, cx + 0.95*s, cy + 0.55*s]
    head_c = (cx + 0.45*s, cy - 0.75*s); hr = 0.52*s
    head = [head_c[0]-hr, head_c[1]-hr, head_c[0]+hr, head_c[1]+hr]
    tail = [(cx - 0.85*s, cy - 0.15*s), (cx - 1.35*s, cy - 0.75*s), (cx - 0.55*s, cy - 0.45*s)]
    beak = [head_c[0] + 0.30*s, head_c[1] - 0.05*s, head_c[0] + 1.05*s, head_c[1] + 0.32*s]
    # outlines first (grown shapes)
    def grow(b, g): return [b[0]-g, b[1]-g, b[2]+g, b[3]+g]
    d.ellipse(grow(body, o), fill=OUTLINE); d.ellipse(grow(head, o), fill=OUTLINE)
    d.polygon(tail, fill=OUTLINE, outline=OUTLINE, width=2*o)
    d.ellipse(grow(beak, o), fill=OUTLINE)
    # fills
    d.polygon(tail, fill=YELLOW)
    d.ellipse(body, fill=YELLOW)
    d.ellipse(beak, fill=BEAK)
    d.ellipse([beak[0], (beak[1]+beak[3])/2, beak[2]-0.1*s, beak[3]], fill=BEAK_SH)
    d.ellipse(head, fill=YELLOW)
    # belly shade and wing
    d.chord([body[0]+0.1*s, cy - 0.1*s, body[2]-0.1*s, body[3]-0.04*s], 20, 160, fill=YELLOW_SH)
    d.chord([cx - 0.75*s, cy - 0.35*s, cx + 0.35*s, cy + 0.30*s], 0, 180, fill=YELLOW_SH)
    d.arc([cx - 0.75*s, cy - 0.35*s, cx + 0.35*s, cy + 0.30*s], 0, 180, fill=OUTLINE, width=max(2, o//2))
    # eye
    er = 0.13*s; ex, ey = head_c[0] + 0.12*s, head_c[1] - 0.12*s
    d.ellipse([ex-er, ey-er, ex+er, ey+er], fill=EYE)
    d.ellipse([ex-er*0.15, ey-er*0.75, ex+er*0.45, ey-er*0.15], fill=EYE_HI)
    # glossy highlight on head and back
    d.ellipse([head_c[0]-0.32*s, head_c[1]-0.38*s, head_c[0]-0.08*s, head_c[1]-0.16*s], fill=(246, 214, 70))
    d.ellipse([cx - 0.55*s, cy - 0.48*s, cx - 0.05*s, cy - 0.32*s], fill=(242, 204, 58))
    return im

def paste_wrapped(dst, spr, cx, cy):
    W, H = spr.size
    for dx in (-M, 0, M):
        for dy in (-M, 0, M):
            dst.alpha_composite(spr, (int(cx - W/2 + dx), int(cy - H/2 + dy)))

def bubble(dst, cx, cy, r):
    lay = Image.new("RGBA", (int(4*r)+4, int(4*r)+4), (0, 0, 0, 0)); d = ImageDraw.Draw(lay)
    c = lay.size[0] / 2
    d.ellipse([c-r, c-r, c+r, c+r], fill=(36, 104, 168, 150), outline=BUB_RIM + (255,), width=max(3, int(r*0.16)))
    d.ellipse([c-r*0.55, c-r*0.6, c-r*0.1, c-r*0.2], fill=BUB_HI + (255,))
    paste_wrapped(dst, lay, cx, cy)

# bubbles first (behind ducks), then ducks in a staggered lattice so every crop has one
for _ in range(12):
    bubble(img, rng.uniform(0, M), rng.uniform(0, M), rng.uniform(0.022, 0.05) * M)

s = 0.10 * M
spots = [(0.27, 0.27, False, -6), (0.77, 0.30, True, 8), (0.25, 0.80, True, 5), (0.76, 0.79, False, -9)]
for u, v, flip, rot in spots:
    spr = duck_sprite(s)
    if flip: spr = spr.transpose(Image.FLIP_LEFT_RIGHT)
    spr = spr.rotate(rot, resample=Image.BICUBIC, expand=True)
    paste_wrapped(img, spr, u * M, v * M)

# a few small bubbles in front, clustered near the ducks
for u, v in [(0.50, 0.12), (0.03, 0.55), (0.52, 0.62), (0.97, 0.05), (0.50, 0.95)]:
    bubble(img, u*M, v*M, 0.028*M)

out = img.convert("RGB").resize((N, N), Image.LANCZOS)
out.save(os.path.join(D, "paint.png"))
# tile check
t = Image.new("RGB", (2*N, 2*N))
for i in range(2):
    for j in range(2): t.paste(out, (i*N, j*N))
t.save(os.path.join(D, "tile_check.png"))

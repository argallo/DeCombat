# Abyss: deep-sea camo with bioluminescent jellyfish. Procedural, tileable 512x512.
import os, math
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

A = os.path.dirname(os.path.abspath(__file__))
N = 512
SS = 2            # supersample
M = N * SS
rng = np.random.default_rng(7)

def periodic_noise(n, octaves, seed):
    r = np.random.default_rng(seed)
    y, x = np.mgrid[0:n, 0:n] / n
    out = np.zeros((n, n))
    amp = 1.0
    for f in octaves:
        for _ in range(3):
            kx, ky = r.integers(-f, f + 1, 2)
            if kx == 0 and ky == 0: kx = f
            ph = r.uniform(0, 2 * math.pi)
            out += amp * np.cos(2 * math.pi * (kx * x + ky * y) + ph)
        amp *= 0.55
    out -= out.min(); out /= out.max()
    return out

def wrap_blur(img, radius):
    """Gaussian blur of an L/RGB image that wraps around the edges."""
    a = np.asarray(img)
    big = np.tile(a, (3, 3) + (1,) * (a.ndim - 2))
    b = np.asarray(Image.fromarray(big).filter(ImageFilter.GaussianBlur(radius)))
    h, w = a.shape[:2]
    return Image.fromarray(b[h:2 * h, w:2 * w])

OFFS = [(dx, dy) for dx in (-M, 0, M) for dy in (-M, 0, M)]

# Jellyfish: (cx, cy, bell radius, hue) in 512 tile units. Big, few, bold.
JELLIES = [
    (110, 120, 66, 'c'),
    (360, 70, 52, 'm'),
    (300, 300, 72, 'm'),
    (70, 400, 46, 'c'),
    (455, 330, 40, 'c'),
    (190, 250, 30, 'm'),
]
COL = {
    # bell fill, rim / tendril, glow (effect)
    'c': ((0.12, 0.42, 0.52), (0.22, 0.62, 0.68), (0.25, 1.0, 0.95)),
    'm': ((0.42, 0.13, 0.45), (0.62, 0.24, 0.62), (1.0, 0.30, 0.90)),
}

def draw_jelly(dr_fill, dr_rim, dr_glow, cx, cy, R, hue, seed, dr_bell, dr_rg):
    r = np.random.default_rng(seed)
    cx, cy, R = cx * SS, cy * SS, R * SS
    fill, rim, glow = (tuple(int(255 * v) for v in c) for c in COL[hue])
    ry = R * 0.78
    # tendrils first (behind the bell)
    nt = 7
    for i in range(nt):
        x0 = cx - R * 0.85 + (2 * R * 0.85) * i / (nt - 1)
        length = R * r.uniform(2.2, 3.2)
        amp = R * r.uniform(0.10, 0.22)
        ph = r.uniform(0, 6.28)
        waves = r.uniform(1.2, 2.2)
        pts = []
        for s in np.linspace(0, 1, 60):
            pts.append((x0 + amp * s * math.sin(ph + waves * 6.28 * s) - (x0 - cx) * 0.25 * s,
                        cy + 2 + length * s))
        for k in range(len(pts) - 1):
            w = max(1, int(SS * (3.2 * (1 - k / len(pts)) + 0.8)))
            for dx, dy in OFFS:
                p0 = (pts[k][0] + dx, pts[k][1] + dy); p1 = (pts[k + 1][0] + dx, pts[k + 1][1] + dy)
                dr_rim.line([p0, p1], fill=rim, width=w)
                dr_glow.line([p0, p1], fill=glow, width=max(1, w - SS))
    # oral arms: thicker frilly ribbons in the middle
    for j in (-1, 1):
        pts = []
        L = R * r.uniform(1.4, 1.9)
        for s in np.linspace(0, 1, 50):
            pts.append((cx + j * R * 0.12 + R * 0.18 * math.sin(3.0 * 6.28 * s + j) * s, cy + L * s))
        for k in range(len(pts) - 1):
            w = int(SS * (9 * (1 - k / len(pts)) + 3))
            for dx, dy in OFFS:
                p0 = (pts[k][0] + dx, pts[k][1] + dy); p1 = (pts[k + 1][0] + dx, pts[k + 1][1] + dy)
                dr_fill.line([p0, p1], fill=fill, width=w)
                dr_rim.line([p0, p1], fill=rim, width=max(1, w // 3))
    # bell: dome (upper half ellipse plus a slight skirt)
    for dx, dy in OFFS:
        bx0, by0, bx1, by1 = cx - R + dx, cy - ry + dy, cx + R + dx, cy + ry * 0.45 + dy
        dr_fill.chord([bx0, by0, bx1, by0 + 2 * ry], 180, 360, fill=fill)
        dr_bell.chord([bx0, by0, bx1, by0 + 2 * ry], 180, 360, fill=glow)
        dr_bell.rectangle([bx0 + R * 0.04, cy + dy - 1, bx1 - R * 0.04, cy + dy + ry * 0.18], fill=glow)
        dr_fill.rectangle([bx0 + R * 0.04, cy + dy - 1, bx1 - R * 0.04, cy + dy + ry * 0.18], fill=fill)
        # scalloped rim
        ns = 9
        for k in range(ns):
            sx = bx0 + R * 0.04 + (2 * R * 0.92) * (k + 0.5) / ns
            sr = R * 0.12
            dr_fill.ellipse([sx - sr, cy + dy + ry * 0.12 - sr, sx + sr, cy + dy + ry * 0.12 + sr], fill=fill)
            dr_rim.ellipse([sx - sr * 0.5, cy + dy + ry * 0.12 - sr * 0.5 + sr * 0.6,
                            sx + sr * 0.5, cy + dy + ry * 0.12 + sr * 0.5 + sr * 0.6], fill=rim)
            dr_rg.ellipse([sx - sr * 0.45, cy + dy + ry * 0.12 + sr * 0.15,
                             sx + sr * 0.45, cy + dy + ry * 0.12 + sr * 1.05], fill=glow)
        # bright outline of the dome
        dr_rim.arc([bx0, by0, bx1, by0 + 2 * ry], 180, 360, fill=rim, width=int(SS * 5))
        dr_rg.arc([bx0 + SS, by0 + SS, bx1 - SS, by0 + 2 * ry - SS], 185, 355, fill=glow, width=int(SS * 3))
        # inner gonads: four horseshoe blobs
        for k in range(4):
            gx = cx + dx + (k - 1.5) * R * 0.36
            gy = cy + dy - ry * 0.38 + abs(k - 1.5) * ry * 0.12
            gr = R * 0.13
            dr_rim.ellipse([gx - gr, gy - gr, gx + gr, gy + gr], outline=rim, width=int(SS * 4))
            dr_rg.ellipse([gx - gr * 0.6, gy - gr * 0.6, gx + gr * 0.6, gy + gr * 0.6], fill=glow)

def make():
    # background: navy to black-blue, periodic, with soft vertical light shafts
    n1 = periodic_noise(N, [1, 2, 4], 11)
    n2 = periodic_noise(N, [3, 6, 12], 23)
    t = np.clip(0.65 * n1 + 0.35 * n2, 0, 1)[..., None]
    navy = np.array([0.04, 0.075, 0.23]); deep = np.array([0.012, 0.022, 0.085])
    bg = deep + (navy - deep) * t ** 1.3
    x = np.arange(N) / N
    shafts = (0.5 + 0.5 * np.cos(2 * math.pi * 3 * x + 1.3 * np.cos(2 * math.pi * x))) ** 6
    bg += (shafts[None, :] * 0.05 * (0.4 + 0.6 * n1))[..., None] * np.array([0.3, 0.7, 1.0])

    fillL = Image.new('RGB', (M, M), (0, 0, 0)); fa = Image.new('L', (M, M), 0)
    rimL = Image.new('RGB', (M, M), (0, 0, 0)); ra = Image.new('L', (M, M), 0)
    glowL = Image.new('RGB', (M, M), (0, 0, 0))
    bellL = Image.new('RGB', (M, M), (0, 0, 0)); rimG = Image.new('RGB', (M, M), (0, 0, 0))
    # draw colour and coverage masks in parallel by drawing twice
    class Pair:
        def __init__(s, img, mask): s.d = ImageDraw.Draw(img); s.m = ImageDraw.Draw(mask)
        def __getattr__(s, name):
            f1, f2 = getattr(s.d, name), getattr(s.m, name)
            def both(*a, **k):
                f1(*a, **k)
                k2 = dict(k)
                for key in ('fill', 'outline'):
                    if key in k2 and k2[key] is not None: k2[key] = 255
                f2(*a, **k2)
            return both
    pf, pr = Pair(fillL, fa), Pair(rimL, ra)
    pg = ImageDraw.Draw(glowL)
    for i, (cx, cy, R, h) in enumerate(JELLIES):
        draw_jelly(pf, pr, pg, cx, cy, R, h, 100 + i, ImageDraw.Draw(bellL), ImageDraw.Draw(rimG))

    def down(im): return im.resize((N, N), Image.LANCZOS)
    f = np.asarray(down(fillL)) / 255.0; fA = np.asarray(down(fa))[..., None] / 255.0
    rr = np.asarray(down(rimL)) / 255.0; rA = np.asarray(down(ra))[..., None] / 255.0
    # translucent bell: fill at 85 percent over the background, rim opaque
    fcol = np.where(fA > 1e-3, f / np.maximum(fA, 1e-3), 0)
    rcol = np.where(rA > 1e-3, rr / np.maximum(rA, 1e-3), 0)
    img = bg * (1 - 0.85 * fA) + fcol * 0.85 * fA
    img = img * (1 - rA) + rcol * rA
    # faint halo around the jellies in the paint
    halo = np.asarray(wrap_blur(down(glowL), 10)) / 255.0
    img = img + 0.10 * halo
    # marine snow: a few bigger soft specks
    snow = Image.new('L', (N, N), 0); ds = ImageDraw.Draw(snow)
    for _ in range(70):
        sx, sy = rng.uniform(0, N, 2); sr = rng.uniform(1.0, 2.4)
        for dx in (-N, 0, N):
            for dy in (-N, 0, N):
                ds.ellipse([sx - sr + dx, sy - sr + dy, sx + sr + dx, sy + sr + dy], fill=int(rng.uniform(90, 180)))
    sn = np.asarray(snow)[..., None] / 255.0
    img = img + sn * 0.18 * np.array([0.6, 0.85, 1.0])
    img = np.clip(img, 0, 1)
    Image.fromarray((img * 255).astype(np.uint8)).save(os.path.join(A, 'paint.png'))

    # effect: soft bell halos, bright rims, dim tendrils, rising motes. Mostly black.
    g = np.asarray(down(glowL)) / 255.0
    bell = np.asarray(wrap_blur(down(bellL), 9)) / 255.0
    core = np.asarray(wrap_blur(down(rimG), 1.0)) / 255.0
    fx = 0.55 * bell + 0.75 * core + 0.22 * np.asarray(wrap_blur(Image.fromarray((g * 255).astype(np.uint8)), 1.5)) / 255.0
    # rising motes: bright heads with a fading trail below (they move up)
    mot = np.zeros((N, N, 3))
    yy, xx = np.mgrid[0:N, 0:N]
    for _ in range(26):
        mx, my = rng.uniform(0, N, 2)
        c = np.array([0.3, 1.0, 0.95]) if rng.random() < 0.55 else np.array([1.0, 0.35, 0.9])
        r0 = rng.uniform(2.0, 3.6); tl = rng.uniform(18, 40)
        ddx = (xx - mx + N / 2) % N - N / 2
        ddy = (yy - my + N / 2) % N - N / 2
        head = np.exp(-(ddx ** 2 + ddy ** 2) / (2 * r0 ** 2))
        trail = np.exp(-ddx ** 2 / (2 * (r0 * 0.5) ** 2)) * np.where((ddy > 0) & (ddy < tl), (1 - ddy / tl) ** 2, 0) * 0.45
        mot += (head + trail)[..., None] * c
    fx = fx + mot + sn * 0.35 * np.array([0.5, 0.9, 1.0])
    fx = np.clip(fx, 0, 1)
    Image.fromarray((fx * 255).astype(np.uint8)).save(os.path.join(A, 'fx.png'))
    # tile check
    t2 = Image.open(os.path.join(A, 'paint.png'))
    chk = Image.new('RGB', (2 * N, 2 * N))
    for i in range(2):
        for j in range(2): chk.paste(t2, (i * N, j * N))
    chk.save(os.path.join(A, 'tile_check.png'))

make()

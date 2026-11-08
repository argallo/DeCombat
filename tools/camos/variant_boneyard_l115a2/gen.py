# Boneyard camo generator (original procedural art, numpy + Pillow only)
import os, math
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

D = os.path.dirname(os.path.abspath(__file__))
N = 512
SS = 4            # supersample
M = N * SS
rng = np.random.default_rng(538)

# palette (0..1), kept mid-tone
GROUND = np.array([0.16, 0.022, 0.035])
GROUND_DK = np.array([0.075, 0.012, 0.018])
DUST = np.array([0.22, 0.15, 0.13])
IVORY = (0.43, 0.36, 0.245)
IVORY_SH = (0.30, 0.23, 0.145)
LINE = (0.07, 0.015, 0.02)
HOLE = (0.06, 0.01, 0.015)

def c8(c): return tuple(int(round(v * 255)) for v in c) + (255,)

# ---- tileable ground: wrapped noise blurred in Fourier space
def tnoise(scale, seed):
    r = np.random.default_rng(seed).standard_normal((N, N))
    f = np.fft.fft2(r)
    ky = np.fft.fftfreq(N)[:, None]; kx = np.fft.fftfreq(N)[None, :]
    g = np.exp(-(kx**2 + ky**2) * (scale**2) * 2 * math.pi**2)
    out = np.real(np.fft.ifft2(f * g))
    out = (out - out.mean()) / (out.std() + 1e-9)
    return out
n1 = tnoise(40, 1); n2 = tnoise(10, 2); n3 = tnoise(3, 3)
shade = np.clip(0.5 + 0.35 * n1, 0, 1)[..., None]
ground = GROUND_DK * (1 - shade) + GROUND * shade
dust = np.clip((0.55 * n2 + 0.45 * n3) - 0.75, 0, 1)[..., None] * 0.7
ground = ground * (1 - dust) + DUST * dust
base = Image.fromarray((np.clip(ground, 0, 1) * 255).astype(np.uint8)).resize((M, M), Image.BICUBIC).convert("RGBA")

# ---- motif sprites
def bone_sprite(L, T):
    """bone along x, length L, shaft thickness T (supersampled px)"""
    pad = int(T * 1.2)
    w, h = L + 2 * pad, int(T * 2.6) + 2 * pad
    im = Image.new("RGBA", (w, h), (0, 0, 0, 0)); dr = ImageDraw.Draw(im)
    cy = h / 2; x0 = pad + T * 0.9; x1 = w - pad - T * 0.9
    k = T * 0.78; off = T * 0.62
    ow = max(3, int(T * 0.16))
    for col, grow in ((LINE, ow), (IVORY, 0)):
        for x in (x0, x1):
            for s in (-1, 1):
                dr.ellipse([x - k - grow, cy + s * off - k - grow, x + k + grow, cy + s * off + k + grow], fill=c8(col))
        dr.rectangle([x0, cy - T / 2 - grow, x1, cy + T / 2 + grow], fill=c8(col))
    # shading line along lower edge of shaft
    dr.rectangle([x0 + k * 0.6, cy + T * 0.12, x1 - k * 0.6, cy + T / 2], fill=c8(IVORY_SH))
    for x in (x0, x1):
        dr.ellipse([x - k * 0.55, cy + off - k * 0.35, x + k * 0.55, cy + off + k * 0.75], fill=c8(IVORY_SH))
    return im

def skull_sprite(S):
    """cartoon skull, width ~S"""
    w, h = int(S * 1.25), int(S * 1.4)
    im = Image.new("RGBA", (w, h), (0, 0, 0, 0)); dr = ImageDraw.Draw(im)
    cx = w / 2; top = S * 0.12
    R = S * 0.5
    ow = max(4, int(S * 0.05))
    def shape(col, g):
        dr.ellipse([cx - R - g, top - g, cx + R + g, top + 2 * R * 0.92 + g], fill=c8(col))
        jw = R * 0.62
        dr.rounded_rectangle([cx - jw - g, top + R * 1.25 - g, cx + jw + g, top + R * 2.05 + g], radius=R * 0.25, fill=c8(col))
    shape(LINE, ow); shape(IVORY, 0)
    # cheek shadow
    dr.chord([cx - R, top + R * 0.2, cx + R, top + R * 1.84], 20, 160, fill=c8(IVORY_SH))
    dr.ellipse([cx - R * 0.86, top + R * 0.05, cx + R * 0.86, top + R * 1.55], fill=c8(IVORY))
    # eyes
    er = R * 0.3; ey = top + R * 1.0
    for s in (-1, 1):
        ex = cx + s * R * 0.4
        dr.ellipse([ex - er, ey - er * 1.05, ex + er, ey + er * 1.05], fill=c8(HOLE))
    # nose
    ny = top + R * 1.38; nw = R * 0.13
    dr.polygon([(cx - nw, ny + nw * 1.2), (cx + nw, ny + nw * 1.2), (cx, ny - nw * 0.6)], fill=c8(HOLE))
    # teeth
    ty0, ty1 = top + R * 1.68, top + R * 1.98
    tw = R * 0.62
    dr.line([(cx - tw * 0.8, ty0), (cx + tw * 0.8, ty0)], fill=c8(LINE), width=ow)
    for i in range(-2, 3):
        x = cx + i * tw * 0.3
        dr.line([(x, ty0 - R * 0.04), (x, ty1)], fill=c8(LINE), width=max(3, ow - 1))
    return im

# ---- placement on the torus with spacing
items = []
def place(kind, count, rad, tries=4000):
    n = 0
    for _ in range(tries):
        if n >= count: break
        p = rng.random(2) * N
        ok = True
        for (q, r2, _k, _a, _s) in items:
            d = np.abs(p - q); d = np.minimum(d, N - d)
            if math.hypot(*d) < (rad + r2): ok = False; break
        if ok:
            ang = rng.uniform(0, 360)
            if kind == "skull": ang = rng.uniform(-35, 35)
            items.append((p, rad, kind, ang, rng.uniform(0.9, 1.1)))
            n += 1
place("skull", 6, 52)
place("bone", 9, 46)
place("bone", 10, 30)   # smaller bones fill gaps

def paste_wrap(canvas, spr, cx, cy):
    w, h = spr.size
    for dx in (-M, 0, M):
        for dy in (-M, 0, M):
            x = int(cx - w / 2 + dx); y = int(cy - h / 2 + dy)
            if x > M or y > M or x + w < 0 or y + h < 0: continue
            canvas.alpha_composite(spr, (x, y)) if x >= 0 and y >= 0 else canvas.paste(spr, (x, y), spr)

# soft drop shadow layer
shadow = Image.new("RGBA", (M, M), (0, 0, 0, 0))
layer = Image.new("RGBA", (M, M), (0, 0, 0, 0))
order = sorted(items, key=lambda t: 0 if t[2] == "bone" else 1)
for (p, rad, kind, ang, sc) in order:
    if kind == "bone":
        L = int(rad * 2.3 * SS * sc); T = int(rad * 0.42 * SS)
        spr = bone_sprite(L, T)
    else:
        spr = skull_sprite(int(rad * 1.45 * SS * sc))
    spr = spr.rotate(ang, resample=Image.BICUBIC, expand=True)
    cx, cy = p[0] * SS, p[1] * SS
    a = spr.split()[3]
    sh = Image.new("RGBA", spr.size, (20, 0, 2, 0)); sh.putalpha(a.point(lambda v: v * 0.6))
    paste_wrap(shadow, sh, cx + 6 * SS, cy + 7 * SS)
    paste_wrap(layer, spr, cx, cy)

# blur shadow with wrap: tile 3x3, blur, crop
big = Image.new("RGBA", (3 * M, 3 * M))
for i in range(3):
    for j in range(3): big.paste(shadow, (i * M, j * M))
big = big.filter(ImageFilter.GaussianBlur(5 * SS))
shadow = big.crop((M, M, 2 * M, 2 * M))
out = base.copy(); out.alpha_composite(shadow); out.alpha_composite(layer)
out = out.convert("RGB").resize((N, N), Image.LANCZOS)
# grave-dust over motifs too (aged look)
a = np.asarray(out).astype(np.float32) / 255
grit = np.clip(n3 - 1.3, 0, 1)[..., None] * 0.25
a = a * (1 - grit) + DUST * grit
a = a * (0.93 + 0.07 * np.clip(0.5 + 0.4 * n2, 0, 1))[..., None]
Image.fromarray((np.clip(a, 0, 1) * 255).astype(np.uint8)).save(os.path.join(D, "paint.png"))
t = Image.open(os.path.join(D, "paint.png"))
tc = Image.new("RGB", (2 * N, 2 * N))
for i in range(2):
    for j in range(2): tc.paste(t, (i * N, j * N))
tc.save(os.path.join(D, "tile_check.png"))
print("ok")

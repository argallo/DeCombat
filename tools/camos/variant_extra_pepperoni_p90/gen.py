# Extra Pepperoni camo for the P90: procedural, tileable 512x512 (torus distances everywhere).
import numpy as np, os, sys
from PIL import Image
N = 512
rng = np.random.default_rng(80515)
out = os.path.dirname(os.path.abspath(__file__))
y, x = np.mgrid[0:N, 0:N].astype(np.float32)

def fnoise(scale, seed):
    # periodic noise: random spectrum with a gaussian low-pass, inverse FFT (tileable by construction)
    r = np.random.default_rng(seed)
    f = np.fft.fftfreq(N)[:, None]**2 + np.fft.fftfreq(N)[None, :]**2
    spec = (r.normal(size=(N, N)) + 1j*r.normal(size=(N, N))) * np.exp(-f * (N/scale)**2)
    n = np.real(np.fft.ifft2(spec)); n -= n.mean(); return n / (n.std() + 1e-9)

def tdist(cx, cy):
    dx = (x - cx + N/2) % N - N/2; dy = (y - cy + N/2) % N - N/2
    return dx, dy

def mix(base, col, a):
    a = np.clip(a, 0, 1)[..., None]; return base*(1-a) + np.asarray(col, np.float32)*a

def sstep(e0, e1, v):
    t = np.clip((v-e0)/(e1-e0), 0, 1); return t*t*(3-2*t)

# --- cheese: saturated warm yellow-orange, with flowing streaks and bubbles
n1 = fnoise(18, 1); n2 = fnoise(6, 2); n3 = fnoise(40, 3)
cheese_a = np.array([0.95, 0.70, 0.08], np.float32)   # bright melted yellow
cheese_b = np.array([0.92, 0.50, 0.04], np.float32)   # deeper orange
t = sstep(-0.6, 1.4, n1 + 0.5*n3)
img = cheese_a*(1-t[..., None]) + cheese_b*t[..., None]
# golden-brown toasted blisters
blis = sstep(1.3, 2.1, n2 + 0.3*n1)
img = mix(img, [0.66, 0.36, 0.04], blis*0.8)
# bubbles: small domed circles with bright rim highlight and darker shadow
bub = rng.integers(0, N, size=(70, 2)); brad = rng.uniform(7, 16, 70)
for (cx, cy), r in zip(bub, brad):
    dx, dy = tdist(cx, cy); d = np.sqrt(dx*dx + dy*dy)
    inside = sstep(r, r-1.5, d)
    img = mix(img, [0.99, 0.80, 0.18], inside*0.55)                 # lighter dome
    hl = sstep(r*0.5, r*0.15, np.sqrt((dx+r*0.35)**2 + (dy+r*0.35)**2))
    img = mix(img, [1.0, 0.92, 0.45], hl*0.75)                       # specular dot
    rim = sstep(1.8, 0.0, np.abs(d - r)) * sstep(-r, r, dx+dy)       # shadowed lower-right rim
    img = mix(img, [0.70, 0.38, 0.03], rim*0.7)

# --- pepperoni slices: placed by rejection sampling on the torus (two tiles across the gun -> ~10 per tile)
pep = []
tries = 0
while len(pep) < 11 and tries < 5000:
    tries += 1
    c = rng.uniform(0, N, 2); r = rng.uniform(42, 56)
    ok = True
    for (px, py, pr) in pep:
        ddx = (c[0]-px+N/2) % N - N/2; ddy = (c[1]-py+N/2) % N - N/2
        if np.hypot(ddx, ddy) < r + pr + 14: ok = False; break
    if ok: pep.append((c[0], c[1], r))
pmask = np.zeros((N, N), np.float32)
for (cx, cy, r) in pep:
    dx, dy = tdist(cx, cy)
    ang = np.arctan2(dy, dx)
    wob = 1 + 0.022*np.sin(3*ang + rng.uniform(0, 6.28)) + 0.012*np.sin(5*ang + rng.uniform(0, 6.28))
    d = np.sqrt(dx*dx + dy*dy) / wob
    # cheese pulled up around the slice: soft dark-orange halo
    img = mix(img, [0.80, 0.40, 0.04], sstep(r+9, r+1, d)*0.6)
    body = sstep(r, r-1.6, d)
    pmask = np.maximum(pmask, body)
    tex = fnoise(9, int(cx*7+cy))  # same spectrum for every slice is fine; cheap re-use below
    col = np.stack([0.64 + 0.06*tex, 0.11 + 0.03*tex, 0.07 + 0.02*tex], -1)
    img = img*(1-body[..., None]) + col*body[..., None]
    # cupped, crispy darker rim
    img = mix(img, [0.42, 0.06, 0.04], sstep(r-12, r-1.5, d)*body*1.0)
    # fat speckles (lighter pink-red flecks)
    for _ in range(18):
        a = rng.uniform(0, 6.28); rr = rng.uniform(0, r*0.75)
        fx, fy = cx + rr*np.cos(a), cy + rr*np.sin(a)
        ex, ey = tdist(fx, fy); fd = np.sqrt(ex*ex + ey*ey)
        img = mix(img, [0.92, 0.48, 0.36], sstep(rng.uniform(3.5, 6.0), 1.0, fd)*body*0.8)
    # grease shine: one soft highlight crescent upper-left
    sh = sstep(r*0.35, 0, np.sqrt((dx + r*0.38)**2 + (dy + r*0.38)**2))
    img = mix(img, [0.95, 0.45, 0.35], sh*body*0.45)
    # little orange grease pools in the cheese next to it
    for _ in range(2):
        a = rng.uniform(0, 6.28); rr = r + rng.uniform(10, 18)
        ex, ey = tdist(cx + rr*np.cos(a), cy + rr*np.sin(a)); fd = np.sqrt(ex*ex + ey*ey)
        img = mix(img, [0.90, 0.36, 0.03], sstep(6, 2.5, fd)*(1-pmask)*0.85)

# --- green pepper bits: thick curved arc strips
def arc_piece(cx, cy, R, w, a0, span, col, dark):
    global img
    dx, dy = tdist(cx, cy); d = np.sqrt(dx*dx + dy*dy); ang = np.arctan2(dy, dx)
    da = (ang - a0 + np.pi) % (2*np.pi) - np.pi
    along = sstep(span/2, span/2 - 0.12, np.abs(da))
    across = sstep(w/2, w/2 - 1.2, np.abs(d - R))
    m = along*across
    edge = sstep(w/2 - 2.5, w/2, np.abs(d - R)) * m  # darker skin edge
    img = mix(img, col, m)
    img = mix(img, dark, edge*0.8)
    hl = sstep(1.5, 0, np.abs(d - (R - w*0.18))) * along * across
    img = mix(img, [0.55, 0.85, 0.35], hl*0.6)
    return m

def free(cx, cy, rad):
    for (px, py, pr) in pep:
        ddx = (cx-px+N/2) % N - N/2; ddy = (cy-py+N/2) % N - N/2
        if np.hypot(ddx, ddy) < pr + rad: return False
    return True

placed = []
k = 0
while len(placed) < 14 and k < 4000:
    k += 1
    cx, cy = rng.uniform(0, N, 2)
    if not free(cx, cy, 14) or any(np.hypot((cx-a+N/2) % N-N/2, (cy-b+N/2) % N-N/2) < 40 for a, b in placed): continue
    placed.append((cx, cy))
    R = rng.uniform(18, 28); a0 = rng.uniform(0, 6.28)
    arc_piece(cx - R*np.cos(a0), cy - R*np.sin(a0), R, rng.uniform(11, 14), a0, rng.uniform(1.4, 2.2),
              [0.16, 0.55, 0.12], [0.06, 0.30, 0.05])

# --- basil: small dark-green leaves (pointed ellipse with a centre vein)
k = 0; nb = 0
while nb < 9 and k < 4000:
    k += 1
    cx, cy = rng.uniform(0, N, 2)
    if any(np.hypot((cx-a+N/2) % N-N/2, (cy-b+N/2) % N-N/2) < 26 for a, b in placed): continue
    placed.append((cx, cy)); nb += 1
    L = rng.uniform(20, 27); W = L*0.5; th = rng.uniform(0, 6.28)
    dx, dy = tdist(cx, cy)
    u = dx*np.cos(th) + dy*np.sin(th); v = -dx*np.sin(th) + dy*np.cos(th)
    half = W*np.sqrt(np.clip(1 - (u/L)**2, 0, 1))**1.3  # pointed tips
    m = sstep(0.5, -0.8, np.abs(v) - half) * (np.abs(u) < L)
    img = mix(img, [0.10, 0.42, 0.10], m)
    img = mix(img, [0.05, 0.25, 0.06], m*sstep(-0.3, -2.5, v)*0.5)          # one half shaded
    img = mix(img, [0.30, 0.62, 0.22], m*sstep(1.0, 0.0, np.abs(v))*(np.abs(u) < L*0.8)*0.8)  # vein

# --- overall scale toward mid-tones (the gun texture multiplies up to 1.5x)
gain = float(sys.argv[1]) if len(sys.argv) > 1 else 0.68
img = np.clip(img*gain, 0, 1)
Image.fromarray((img*255+0.5).astype(np.uint8)).save(os.path.join(out, "paint.png"))
print("pepperoni", len(pep), "mean", img.reshape(-1, 3).mean(0))

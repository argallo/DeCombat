# Pixel Arcade camo: chunky 8-bit sprites on deep navy. All original, procedural.
import os, sys
import numpy as np
from PIL import Image
D = os.path.dirname(os.path.abspath(__file__))
CELL = int(sys.argv[1]) if len(sys.argv) > 1 else 16   # px per big pixel
N = 512 // CELL                                         # cells per tile side

P = {
    '.': None,
    'N': (18, 22, 64),     # navy bg
    'n': (32, 40, 100),    # lighter navy dots
    'k': (14, 14, 40),     # outline
    'R': (190, 30, 60),    # heart red
    'r': (120, 16, 40),    # heart shade
    'Y': (200, 150, 20),   # coin gold
    'y': (140, 88, 10),    # coin shade
    'S': (220, 190, 40),   # star yellow
    's': (200, 110, 20),   # star shade
    'G': (40, 170, 70),    # green critter
    'g': (20, 105, 45),
    'C': (20, 150, 190),   # cyan critter
    'c': (12, 90, 130),
    'M': (180, 50, 170),   # magenta critter
    'm': (110, 25, 110),
    'O': (220, 100, 20),   # orange
    'W': (190, 195, 210),  # highlight / eye white
}

HEART = [
    ".kk.kk.",
    "kRRkRRk",
    "kRWRRRk",
    "kRRRRrk",
    ".kRRrk.",
    "..krk..",
    "...k...",
]
COIN = [
    ".kkkk.",
    "kYYYyk",
    "kYWyyk",
    "kYWyyk",
    "kYWyyk",
    "kYWyyk",
    "kYYyyk",
    ".kkkk.",
]
STAR = [
    "...k...",
    "..kSk..",
    "kkkSkkk",
    "kSSSSsk",
    ".kSSsk.",
    ".kSkSk.",
    "kSk.ksk",
    "kk...kk",
]
# invented critters
BUG = [   # green box-bug with antennae
    ".k...k.",
    "..k.k..",
    ".kkkkk.",
    "kGGGGGk",
    "kGWkWGk",
    "kGGGGGk",
    "kgGGGgk",
    ".kgkgk.",
    ".k.k.k.",
]
BLOB = [  # cyan one-eyed blob with little feet
    "..kkkk..",
    ".kCCCCk.",
    "kCCWWCCk",
    "kCCWkCCk",
    "kCCCCCCk",
    "kcCCCCck",
    ".kcckcck",
    "..kk.kk.",
]
BAT = [   # magenta flapper
    "k.......k",
    "kmk...kmk",
    "kMMkkkMMk",
    "kMMMMMMMk",
    ".kMWMWMk.",
    "..kMOMk..",
    "...kkk...",
]
MINI = [  # small heart, sized for the magazine island
    ".k.k.",
    "kRkRk",
    "kRWRk",
    ".kRk.",
    "..k..",
]
GEM = [
    ".kkkkk.",
    "kOWOOOk",
    ".kOOOk.",
    "..kOk..",
    "...k...",
]

grid = np.zeros((N, N, 3), np.uint8)
grid[:] = P['N']
rng = np.random.default_rng(7)
# dim background pixel dots (2-cell plus signs and single dots)
for _ in range(N * N // 40):
    x, y = rng.integers(0, N, 2)
    grid[y % N, x % N] = P['n']

def put(spr, cx, cy):
    for j, row in enumerate(spr):
        for i, ch in enumerate(row):
            c = P[ch]
            if c is not None:
                grid[(cy + j) % N, (cx + i) % N] = c

# placements in a 32x32 cell torus (scaled if CELL differs)
f = N / 32.0
items = [
    (HEART, 1, 1), (COIN, 11, 2), (BUG, 20, 0), (STAR, 27, 3),
    (BLOB, 4, 10), (GEM, 14, 12), (HEART, 22, 10), (COIN, 29, 12),
    (STAR, 9, 19), (BAT, 17, 18), (COIN, 0, 16), (MINI, 1, 26), (BUG, 27, 20),
    (GEM, 7, 29), (HEART, 13, 27), (BLOB, 22, 26), (STAR, 30, 28),
]
for spr, x, y in items:
    put(spr, int(x * f), int(y * f))

img = Image.fromarray(grid).resize((512, 512), Image.NEAREST)
img.save(os.path.join(D, 'paint.png'))
t = Image.new('RGB', (1024, 1024))
for a in (0, 512):
    for b in (0, 512):
        t.paste(img, (a, b))
t.save(os.path.join(D, 'tile_check.png'))
print('ok', N)

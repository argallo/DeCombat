"""Overclocked: a green circuit board (paint) with cyan pulses running along its buses (effect).

Everything is procedural. Tileable: every primitive is drawn at the 9 wrap offsets.
The effect pulses sit exactly on the rows of the horizontal buses, and the effect drifts in u only,
so the pulses slide along the bus lines instead of across the board.
"""
import os
import random

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

N = 512
HERE = os.path.dirname(os.path.abspath(__file__))
rng = random.Random(416)

BOARD = (22, 86, 46)
BOARD_DARK = (17, 68, 37)
COPPER = (168, 98, 40)
GOLD = (196, 150, 56)
CHIP = (40, 42, 46)
CHIP_EDGE = (70, 72, 78)
SILK = (190, 214, 196)
HOLE = (20, 40, 26)
CORE = (18, 46, 48)

# bus rows: groups of parallel horizontal lines spanning the whole width
BUS_GROUPS = [32, 160, 288, 416]   # top of each group
BUS_PITCH = 20
BUS_LINES = 2
BUS_W = 10

OFFS = [(dx, dy) for dx in (-N, 0, N) for dy in (-N, 0, N)]


def rect(d, x0, y0, x1, y1, fill):
    for dx, dy in OFFS:
        d.rectangle([x0 + dx, y0 + dy, x1 + dx, y1 + dy], fill=fill)


def disc(d, cx, cy, r, fill):
    for dx, dy in OFFS:
        d.ellipse([cx - r + dx, cy - r + dy, cx + r + dx, cy + r + dy], fill=fill)


def hline(d, x0, x1, y, w, fill):
    a, b = min(x0, x1), max(x0, x1)
    rect(d, a - w // 2, y - w // 2, b + w // 2, y + w // 2, fill)


def vline(d, x, y0, y1, w, fill):
    a, b = min(y0, y1), max(y0, y1)
    rect(d, x - w // 2, a - w // 2, x + w // 2, b + w // 2, fill)


def bus_rows():
    rows = []
    for g in BUS_GROUPS:
        for i in range(BUS_LINES):
            rows.append(g + i * BUS_PITCH)
    return rows


def paint():
    img = Image.new("RGB", (N, N), BOARD)
    d = ImageDraw.Draw(img)

    # faint solder-mask texture: slightly darker grid of blocks
    for gy in range(0, N, 64):
        for gx in range(0, N, 64):
            if rng.random() < 0.35:
                rect(d, gx, gy, gx + 63, gy + 63, BOARD_DARK)

    # buses
    for y in bus_rows():
        hline(d, 0, N, y, BUS_W, COPPER)

    # bands between bus groups: chips with pins, traces from pins to buses
    for gi, g in enumerate(BUS_GROUPS):
        top_bus = g + (BUS_LINES - 1) * BUS_PITCH   # lowest line of this group
        nxt = BUS_GROUPS[(gi + 1) % len(BUS_GROUPS)] + (N if gi == len(BUS_GROUPS) - 1 else 0)
        band0, band1 = top_bus + 8, nxt - 8          # free space between groups
        x = rng.randrange(0, 40)
        while x < N - 30:
            w = rng.choice([44, 56, 68, 80])
            h = rng.choice([28, 36, 44])
            if x + w > N - 8:
                break
            cy = (band0 + band1) // 2 + rng.randrange(-6, 7)
            y0, y1 = cy - h // 2, cy + h // 2
            # pins and traces: up to the band's top bus, down to the next group's top line
            npins = max(2, (w - 12) // 16)
            for p in range(npins):
                px = x + 10 + p * ((w - 20) // max(1, npins - 1))
                # up trace with a right-angle jog
                jog = rng.choice([-16, 0, 16])
                mid = y0 - rng.randrange(8, max(9, (y0 - top_bus) - 4))
                vline(d, px, y0, mid, 6, COPPER)
                hline(d, px, px + jog, mid, 6, COPPER)
                vline(d, px + jog, mid, top_bus, 6, COPPER)
                disc(d, px + jog, mid, 5, GOLD) if jog else None
                disc(d, px + jog, top_bus, 6, GOLD)
                # down trace, only some pins
                if rng.random() < 0.6:
                    jog = rng.choice([-16, 0, 16])
                    mid = y1 + rng.randrange(8, max(9, (nxt - y1) - 6))
                    vline(d, px, y1, mid, 6, COPPER)
                    hline(d, px, px + jog, mid, 6, COPPER)
                    vline(d, px + jog, mid, nxt, 6, COPPER)
                    disc(d, px + jog, nxt, 6, GOLD)
                # pin itself
                rect(d, px - 3, y0 - 5, px + 3, y0, GOLD)
                rect(d, px - 3, y1, px + 3, y1 + 5, GOLD)
            # chip body
            rect(d, x, y0, x + w, y1, CHIP_EDGE)
            rect(d, x + 2, y0 + 2, x + w - 2, y1 - 2, CHIP)
            disc(d, x + 7, y0 + 7, 2, SILK)   # pin-1 dot
            x += w + rng.randrange(24, 56)

    # buses drawn last: copper rails with a dark light-pipe core, where the effect pulses run
    for y in bus_rows():
        hline(d, 0, N, y, BUS_W + 6, COPPER)
        hline(d, 0, N, y, BUS_W, CORE)

    # small SMD parts (resistors) scattered in gaps: two gold pads + dark body
    for _ in range(26):
        sx, sy = rng.randrange(0, N), rng.randrange(0, N)
        # keep them off the bus lines
        if any(abs(sy - r) < 10 for r in bus_rows()):
            continue
        if rng.random() < 0.5:
            rect(d, sx, sy, sx + 5, sy + 6, GOLD)
            rect(d, sx + 6, sy, sx + 11, sy + 6, (60, 50, 40))
            rect(d, sx + 12, sy, sx + 17, sy + 6, GOLD)
        else:
            rect(d, sx, sy, sx + 6, sy + 5, GOLD)
            rect(d, sx, sy + 6, sx + 6, sy + 11, (60, 50, 40))
            rect(d, sx, sy + 12, sx + 6, sy + 17, GOLD)

    return img


def fx():
    a = np.zeros((N, N, 3), np.float32)
    cyan = np.array([0.3, 1.0, 1.0], np.float32)
    xs = np.arange(N)
    for li, y in enumerate(bus_rows()):
        # a few pulses per bus line, each a short comet with a fading tail
        line = np.zeros(N, np.float32)
        n = rng.choice([3, 4])
        for _ in range(n):
            head = rng.randrange(0, N)
            length = rng.randrange(56, 96)
            dist = (head - xs) % N           # tail trails to the left of the head
            tail = np.where(dist < length, 0.25 + 0.75 * np.clip(1 - dist / length, 0, 1) ** 1.2, 0)
            tail = np.where(dist < 4, 1.0, tail)
            line = np.maximum(line, tail)
        for dy in range(-BUS_W // 2 - 2, BUS_W // 2 + 3):
            fall = 1.0 if abs(dy) <= BUS_W // 2 else 0.3
            col = line[:, None] * cyan * fall
            col = col + (line[:, None] >= 0.999) * fall * np.array([0.5, 0.0, 0.0], np.float32)  # whiter head
            a[(y + dy) % N] = np.maximum(a[(y + dy) % N], col)
    img = Image.fromarray((np.clip(a, 0, 1) * 255).astype(np.uint8))
    return img


if __name__ == "__main__":
    paint().save(os.path.join(HERE, "paint.png"))
    fx().save(os.path.join(HERE, "fx.png"))
    print("ok")

#!/usr/bin/env python3
"""Generate 45 additional pixel-art propositions (06-50) for byteshaver.

Same workflow as generate.py: 16x16 grids at 32px cells -> SVG, transparent
background, visual QA sheet. Symbolism stays on the name (shaver, bytes) and
the function (batch image conversion / shrinking). Edit grids, re-run:
    python3 generate-more.py
"""
import zlib, struct, os

OUT = os.path.dirname(os.path.abspath(__file__))

PAL = {
    'K': '#333333',  # dark outline (frames, shaver body)
    'W': '#ffffff',  # white frame
    'B': '#4aa5e8',  # image sky blue
    'b': '#8ecdf5',  # light blue (clouds)
    'G': '#57b544',  # image grass green
    'g': '#3c8f38',  # dark green shade
    'Y': '#ffd93b',  # sun yellow
    'y': '#f0a92e',  # amber shade
    's': '#cbd5e1',  # steel
    'w': '#f1f5f9',  # steel highlight
    'S': '#7c8b9a',  # steel shade
    't': '#2dd4bf',  # teal (shaver body)
    'T': '#0d9488',  # teal shade
    'd': '#0b1220',  # near-black detail
    'n': '#4ade80',  # data green (bytes)
    'l': '#94a3b8',  # light slate
    'L': '#64748b',  # mid slate
    '.': None,
}
N = 16
CELL = 32
SIZE = N * CELL

GLYPHS = {
    'B': ["XXX.",
          "X..X",
          "XXX.",
          "X..X",
          "XXX."],
    '0': [".XX.",
          "X..X",
          "X..X",
          "X..X",
          ".XX."],
    '1': [".X.",
          "XX.",
          ".X.",
          ".X.",
          "XXX"],
}


class Grid:
    def __init__(self):
        self.g = [['.'] * N for _ in range(N)]

    def rect(self, x0, y0, x1, y1, c):
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                self.g[y][x] = c

    def hline(self, x0, x1, y, c):
        self.rect(x0, y, x1, y, c)

    def vline(self, x, y0, y1, c):
        self.rect(x, y0, x, y1, c)

    def put(self, x, y, c):
        self.g[y][x] = c

    def image(self, x0, y0, x1, y1, sun=(1, 1), sky='B', grass='G', frame='W'):
        """classic pixel image icon: dark outline, white frame, sky, hill, sun."""
        self.rect(x0, y0, x1, y1, 'K')
        self.rect(x0 + 1, y0 + 1, x1 - 1, y1 - 1, frame)
        ix0, iy0, ix1, iy1 = x0 + 2, y0 + 2, x1 - 2, y1 - 2
        if ix1 < ix0 or iy1 < iy0:
            return
        horizon = iy1 - max(1, (iy1 - iy0) // 3)
        self.rect(ix0, iy0, ix1, horizon - 1, sky)
        self.rect(ix0, horizon, ix1, iy1, grass)
        if grass == 'G':
            self.hline(ix0, ix1, horizon, 'g')
        if sun:
            sx, sy = ix0 + sun[0], iy0 + sun[1]
            self.rect(sx, sy, sx + 1, sy + 1, 'Y')
            self.put(sx + 1, sy + 1, 'y')

    def glyph(self, x, y, ch, c, scale=1):
        rows = GLYPHS[ch]
        for dy, row in enumerate(rows):
            for dx, cell in enumerate(row):
                if cell != 'X':
                    continue
                self.rect(x + dx * scale, y + dy * scale,
                          x + dx * scale + scale - 1, y + dy * scale + scale - 1, c)

    # ---- shaver parts ----------------------------------------------------
    def cartridge(self, x, y):
        """steel blade cartridge, side view, top-left at (x,y); 7x4."""
        self.hline(x, x + 5, y, 'w')
        self.rect(x, y + 1, x + 6, y + 2, 's')
        self.put(x + 1, y + 1, 'd'); self.put(x + 3, y + 1, 'd'); self.put(x + 5, y + 1, 'd')
        self.hline(x, x + 6, y + 3, 'S')

    def neck(self, x, y, n=2):
        for i in range(n):
            self.put(x - i, y + i, 'S')

    def handle_diag(self, x, y, n, c='t', stripes=True):
        """3-wide diagonal handle going down-left from (x,y)."""
        for i in range(n):
            xx = x - i
            yy = y + i
            self.hline(xx - 1, xx + 1, yy, c)
            self.put(xx + 1, yy, 'T' if c == 't' else 'y')
            if stripes and i in (2, 3):
                self.put(xx, yy, 'd' if c == 't' else 'K')

    def razor_diag(self, x, y, c='t', n=6):
        """full diagonal razor: cartridge head at (x,y), handle down-left."""
        self.cartridge(x, y)
        self.neck(x - 1, y + 4)
        self.handle_diag(x - 2, y + 5, n, c)

    def foil(self, x, y, w):
        """perforated foil head, w wide, 3 tall + frame row."""
        self.hline(x + 1, x + w - 2, y, 'w')
        self.rect(x, y + 1, x + w - 1, y + 2, 's')
        for i, xx in enumerate(range(x + 1, x + w - 1, 2)):
            self.put(xx, y + 1, 'S')
            if xx + 1 <= x + w - 2:
                self.put(xx + 1, y + 2, 'S')
        self.hline(x, x + w - 1, y + 3, 'S')

    def body(self, x, y, w, h, button=True, led=None, cap=True):
        """teal shaver body with dark outline; (x,y) top-left incl outline."""
        self.rect(x, y, x + w - 1, y + h - 1, 'K')
        self.rect(x + 1, y, x + w - 2, y + h - 2, 't')
        self.vline(x + w - 2, y, y + h - 2, 'T')
        if button:
            bw = max(2, (w - 2) // 2)
            bx = x + 1 + (w - 2 - bw) // 2
            by = y + (h - 2) // 2
            self.rect(bx, by, bx + bw - 1, by + 1, 'Y')
            self.put(bx + bw - 1, by + 1, 'y')
        if led:
            self.put(x + 2, y + 1, led)
        if cap:
            self.hline(x, x + w - 1, y + h - 1, 'K')

    def rows(self):
        return [''.join(r) for r in self.g]


# ---- 06-10: razor solo ----------------------------------------------------

def i06():
    "vertical razor, head up"
    g = Grid()
    g.hline(4, 11, 1, 'w')
    g.rect(4, 2, 11, 2, 's')
    g.put(5, 2, 'd'); g.put(7, 2, 'd'); g.put(9, 2, 'd')
    g.rect(4, 3, 11, 3, 's'); g.hline(4, 11, 4, 'S')
    g.put(7, 5, 'S'); g.put(8, 5, 'S')
    g.rect(6, 6, 9, 13, 't')
    g.vline(9, 6, 13, 'T')
    g.put(7, 9, 'd'); g.put(7, 10, 'd'); g.put(8, 9, 'd'); g.put(8, 10, 'd')
    g.rect(6, 14, 9, 14, 'K')
    return g.rows()


def i07():
    "horizontal razor, head left"
    g = Grid()
    g.vline(1, 4, 9, 'w')
    g.rect(2, 4, 3, 10, 's')
    g.put(2, 5, 'd'); g.put(2, 7, 'd'); g.put(2, 9, 'd')
    g.vline(4, 4, 10, 'S')
    g.put(5, 7, 'S'); g.put(5, 8, 'S')
    g.rect(6, 6, 13, 9, 't')
    g.hline(6, 13, 9, 'T')
    g.put(9, 7, 'd'); g.put(10, 7, 'd'); g.put(9, 8, 'd'); g.put(10, 8, 'd')
    g.rect(14, 6, 14, 9, 'K')
    return g.rows()


def i08():
    "diagonal razor, amber grip"
    g = Grid()
    g.razor_diag(8, 1, c='Y', n=6)
    return g.rows()


def i09():
    "classic safety razor, T shape"
    g = Grid()
    g.hline(3, 12, 1, 'w')
    g.rect(3, 2, 12, 3, 's')
    for x in range(4, 12, 2):
        g.put(x, 3, 'S')
    g.put(7, 4, 'S'); g.put(8, 4, 'S')
    g.put(7, 5, 'S'); g.put(8, 5, 'S')
    g.rect(6, 6, 9, 13, 't')
    g.vline(9, 6, 13, 'T')
    g.put(7, 9, 'd'); g.put(8, 10, 'd')
    g.rect(6, 14, 9, 14, 'K')
    return g.rows()


def i10():
    "razor with fresh cut trail"
    g = Grid()
    g.razor_diag(9, 0, c='t', n=5)
    for x, y in [(6, 6), (4, 8), (2, 10), (0, 12)]:
        g.put(x, y, 'w' if x % 2 else 's')
    g.put(7, 5, 'Y'); g.put(1, 13, 'Y')
    return g.rows()


# ---- 11-15: electric solo -------------------------------------------------

def i11():
    "electric shaver, green LED"
    g = Grid()
    g.foil(3, 1, 10)
    g.body(4, 5, 8, 9, button=True, led='n')
    return g.rows()


def i12():
    "mini trimmer, slim"
    g = Grid()
    g.foil(6, 1, 5)
    g.body(6, 5, 5, 10, button=True)
    g.rect(6, 15, 10, 15, 'K')
    return g.rows()


def i13():
    "angled electric shaver"
    g = Grid()
    g.hline(10, 13, 0, 'w')
    g.rect(9, 1, 13, 2, 's')
    g.put(10, 1, 'S'); g.put(12, 2, 'S')
    g.rect(9, 3, 13, 3, 'S')
    for i, y in enumerate(range(4, 11)):
        x = 9 - i
        g.hline(x, x + 2, y, 't')
        g.put(x + 2, y, 'T')
    g.put(9, 7, 'Y'); g.put(9, 8, 'Y')
    g.put(5, 10, 'K'); g.put(6, 11, 'K')
    return g.rows()


def i14():
    "foil head close-up"
    g = Grid()
    g.rect(2, 3, 13, 12, 's')
    g.hline(3, 12, 3, 'w')
    g.vline(2, 4, 11, 'w')
    g.hline(3, 13, 12, 'S')
    g.vline(13, 4, 11, 'S')
    for y in range(5, 12, 2):
        for x in range(4, 13, 2):
            g.put(x, y, 'S')
    g.put(13, 3, 'Y')
    return g.rows()


def i15():
    "electric shaver with cable"
    g = Grid()
    g.foil(4, 0, 8)
    g.body(4, 4, 8, 8, button=True)
    g.put(7, 12, 'S'); g.put(8, 13, 'S'); g.put(7, 14, 'S'); g.put(8, 15, 'S')
    return g.rows()


# ---- 16-22: razor + image -------------------------------------------------

def i16():
    "razor splitting an image along the diagonal"
    g = Grid()
    g.image(1, 3, 12, 13)
    for i, y in enumerate(range(4, 12)):
        x = 12 - i
        if x > 1:
            g.rect(x, y, 12, y, '.')
    for x, y in [(13, 2), (12, 3)]:
        g.put(x, y, 's')
    g.put(14, 1, 'w'); g.put(14, 3, 'Y'); g.put(13, 4, 'B')
    return g.rows()


def i17():
    "razor shaving the top-left corner (mirror)"
    g = Grid()
    g.image(4, 4, 14, 13, sun=(5, 1))
    g.rect(4, 4, 6, 5, '.')
    g.put(4, 6, '.')
    for x, y in [(2, 2), (3, 3), (4, 4), (5, 5)]:
        g.put(x, y, 's')
    g.put(1, 1, 'w'); g.put(1, 3, 'Y'); g.put(2, 4, 'B')
    return g.rows()


def i18():
    "razor slicing a strip off the right edge"
    g = Grid()
    g.image(1, 3, 10, 13, sun=(1, 1))
    for y in range(4, 13):
        g.put(11, y, 'B' if y < 8 else 'G')
        g.put(12, y, 's' if y % 2 else 'w')
    g.hline(10, 13, 1, 'w')
    g.rect(10, 2, 13, 2, 's')
    g.put(11, 2, 'd'); g.put(13, 2, 'd')
    g.rect(10, 3, 13, 3, 'S')
    return g.rows()


def i19():
    "big razor, tiny image"
    g = Grid()
    g.image(1, 9, 6, 14, sun=(1, 1))
    g.put(5, 9, '.'); g.put(6, 9, '.'); g.put(6, 10, '.')
    g.razor_diag(9, 0, c='t', n=5)
    g.put(12, 6, 'Y'); g.put(13, 8, 'B')
    return g.rows()


def i20():
    "blade shaving a polaroid"
    g = Grid()
    g.rect(1, 5, 12, 14, 'K')
    g.rect(2, 6, 11, 13, 'W')
    g.rect(2, 6, 11, 11, 'B')
    g.rect(2, 10, 11, 11, 'G')
    g.hline(2, 11, 10, 'g')
    g.rect(3, 7, 4, 8, 'Y'); g.put(4, 8, 'y')
    g.rect(10, 5, 12, 6, '.')
    g.put(12, 7, '.')
    g.cartridge(8, 1)
    g.put(14, 5, 'w')
    g.put(14, 7, 'Y'); g.put(13, 6, 'B')
    return g.rows()


def i21():
    "shaved pixels falling like clippings"
    g = Grid()
    g.image(0, 1, 9, 11, sun=(1, 1))
    g.rect(7, 1, 9, 2, '.')
    g.put(9, 3, '.')
    for x, y in [(11, 0), (10, 1), (9, 2)]:
        g.put(x, y, 's')
    g.put(12, 0, 'w')
    for x, y, c in [(12, 3, 'B'), (13, 5, 'Y'), (12, 7, 'G'), (13, 9, 'B'), (12, 11, 'G')]:
        g.put(x, y, c)
    return g.rows()


def i22():
    "razor shaving a stack of two photos"
    g = Grid()
    g.rect(4, 1, 13, 8, 'S')
    g.rect(5, 2, 12, 7, 's')
    g.image(1, 4, 10, 13, sun=(1, 1))
    g.rect(8, 4, 10, 5, '.')
    g.put(10, 6, '.')
    for x, y in [(12, 1), (11, 2), (10, 3), (9, 4)]:
        g.put(x, y, 's')
    g.put(13, 0, 'w'); g.put(13, 3, 'Y')
    return g.rows()


# ---- 23-28: electric + image ----------------------------------------------

def i23():
    "electric shaver on the left, image on the right"
    g = Grid()
    g.hline(1, 4, 0, 'w')
    g.rect(1, 1, 5, 2, 's')
    g.put(2, 1, 'S'); g.put(4, 2, 'S')
    g.rect(1, 3, 5, 3, 'S')
    g.body(1, 5, 5, 6, button=True)
    g.image(7, 5, 15, 14, sun=(2, 1))
    g.rect(7, 5, 8, 6, '.')
    g.put(6, 4, 'B'); g.put(5, 3, 'Y')
    return g.rows()


def i24():
    "electric shaver pushing a strip off the image"
    g = Grid()
    g.image(1, 3, 9, 13, sun=(1, 1))
    for y in range(4, 13):
        g.put(10, y, 'B' if y < 8 else 'G')
    g.foil(11, 1, 5)
    g.body(12, 5, 4, 8, button=True)
    g.put(10, 2, 'Y')
    return g.rows()


def i25():
    "electric shaver giving an image a haircut"
    g = Grid()
    g.hline(3, 12, 0, 'w')
    g.rect(2, 1, 13, 2, 's')
    g.put(3, 2, 'S'); g.put(5, 2, 'S'); g.put(7, 2, 'S'); g.put(9, 2, 'S'); g.put(11, 2, 'S')
    g.rect(2, 3, 13, 3, 'S')
    g.image(2, 5, 13, 14, sun=(1, 1))
    for x in range(3, 13):
        g.put(x, 4, 'w' if x % 2 else 's')
    return g.rows()


def i26():
    "bytes falling out of a shaved image"
    g = Grid()
    g.image(0, 4, 9, 13, sun=(1, 1))
    g.rect(7, 4, 9, 5, '.')
    g.put(9, 6, '.')
    g.hline(10, 13, 0, 'w')
    g.rect(9, 1, 13, 2, 's')
    g.put(10, 1, 'S'); g.put(12, 2, 'S')
    g.rect(9, 3, 13, 3, 'S')
    g.rect(10, 4, 13, 9, 'K')
    g.rect(11, 4, 13, 8, 't')
    g.vline(13, 4, 8, 'T')
    g.glyph(11, 10, '0', 'n')
    g.put(13, 9, 'n')
    g.put(12, 13, 'n')
    return g.rows()


def i27():
    "electric shaver shaving a photo stack"
    g = Grid()
    g.rect(5, 0, 14, 6, 'S')
    g.rect(6, 1, 13, 5, 's')
    g.image(2, 4, 11, 13, sun=(1, 1))
    g.rect(9, 4, 11, 5, '.')
    g.hline(11, 14, 1, 'w')
    g.rect(10, 2, 14, 3, 's')
    g.put(11, 2, 'd'); g.put(13, 2, 'd')
    g.rect(10, 3, 14, 3, 'S')
    g.put(14, 4, 'Y'); g.put(13, 5, 'B')
    return g.rows()


def i28():
    "mini trimmer, tiny image"
    g = Grid()
    g.image(0, 8, 7, 15, sun=(1, 1))
    g.rect(6, 8, 7, 9, '.')
    g.foil(9, 0, 5)
    g.body(9, 4, 5, 7, button=True)
    g.put(8, 6, 'Y'); g.put(8, 4, 'B')
    return g.rows()


# ---- 29-36: bytes / name ---------------------------------------------------

def i29():
    "B with its corner shaved off"
    g = Grid()
    g.glyph(3, 3, 'B', 'w', scale=2)
    g.put(9, 5, '.'); g.put(10, 5, '.'); g.put(10, 6, '.')
    for x, y in [(13, 1), (12, 2), (11, 3), (10, 4)]:
        g.put(x, y, 's')
    g.put(14, 0, 'w'); g.put(13, 3, 'Y')
    return g.rows()


def i30():
    "razor leaning on a B"
    g = Grid()
    g.glyph(1, 4, 'B', 't', scale=2)
    g.vline(1, 4, 13, 'T')
    g.cartridge(9, 0)
    g.neck(8, 4)
    g.hline(6, 8, 5, 't'); g.put(8, 5, 'T')
    return g.rows()


def i31():
    "B shedding byte bits"
    g = Grid()
    g.glyph(3, 2, 'B', 'w', scale=2)
    g.hline(3, 10, 11, 's')
    g.glyph(12, 8, '0', 'n')
    g.glyph(13, 2, '1', 'n')
    g.put(12, 14, 'n'); g.put(14, 13, 'n')
    return g.rows()


def i32():
    "byte chip getting a trim"
    g = Grid()
    g.rect(3, 4, 12, 12, 'K')
    g.rect(4, 5, 11, 11, 'd')
    g.glyph(6, 6, 'B', 'n')
    for x in range(5, 12, 2):
        g.put(x, 3, 's'); g.put(x, 13, 's')
    g.rect(12, 1, 15, 2, '.')
    for x, y in [(14, 0), (13, 1)]:
        g.put(x, y, 's')
    g.put(15, 1, 'w')
    g.put(12, 3, '.'); g.put(13, 3, '.')
    return g.rows()


def i33():
    "razor cutting a data stream"
    g = Grid()
    g.glyph(1, 5, '0', 'n')
    g.glyph(6, 5, '1', 'n')
    g.glyph(11, 5, '0', 'n')
    for x, y in [(14, 1), (13, 2), (12, 3), (11, 4), (10, 5), (9, 6), (8, 7), (7, 8), (6, 9), (5, 10)]:
        g.put(x, y, 's')
    g.put(15, 0, 'w')
    g.put(11, 11, '.'); g.put(12, 11, '.'); g.put(13, 11, '.'); g.put(14, 11, '.'); g.put(15, 11, '.')
    g.glyph(12, 11, '0', 'n'); g.glyph(8, 11, '1', 'n')
    return g.rows()


def i34():
    "B badge"
    g = Grid()
    g.rect(1, 1, 14, 14, 'K')
    g.rect(2, 2, 13, 13, 'W')
    g.rect(3, 3, 12, 12, '.')
    g.glyph(6, 5, 'B', 't', scale=1)
    g.put(3, 3, 'Y'); g.put(12, 12, 'y')
    return g.rows()


def i35():
    "razor giving the B a flat top"
    g = Grid()
    g.hline(2, 13, 1, 'w')
    g.rect(2, 2, 13, 2, 's')
    g.put(3, 2, 'd'); g.put(5, 2, 'd'); g.put(7, 2, 'd'); g.put(9, 2, 'd'); g.put(11, 2, 'd')
    g.rect(2, 3, 13, 3, 'S')
    g.glyph(4, 6, 'B', 'w', scale=2)
    g.put(6, 7, 'w'); g.put(5, 5, 'w')
    return g.rows()


def i36():
    "image shedding a 0/1 strip"
    g = Grid()
    g.image(1, 2, 10, 12, sun=(1, 1))
    for y in range(3, 12):
        g.put(12, y, 'n' if y % 2 else 's')
    g.glyph(11, 3, '1', 'n')
    g.put(11, 3, 'n'); g.put(11, 4, '.'); g.put(11, 5, '.')
    g.put(11, 6, 'n')
    g.hline(9, 13, 0, 'w')
    g.rect(9, 1, 13, 1, 's')
    g.put(10, 1, 'd'); g.put(12, 1, 'd')
    g.rect(9, 2, 13, 2, 'S')
    g.put(11, 4, '.'); g.put(11, 5, '.')
    return g.rows()


# ---- 37-42: conversion / before-after --------------------------------------

def i37():
    "big washed frame to small vivid tile"
    g = Grid()
    g.rect(0, 4, 6, 12, 'K')
    g.rect(1, 5, 5, 11, 'l')
    g.rect(1, 9, 5, 11, 'L')
    g.rect(8, 8, 9, 8, 'Y')
    g.put(10, 7, 'Y'); g.put(10, 9, 'Y')
    g.put(11, 8, 'Y')
    g.image(12, 6, 15, 10, sun=None)
    g.rect(13, 7, 14, 7, 'B')
    g.rect(13, 8, 14, 9, 'G')
    g.put(13, 7, 'Y')
    return g.rows()


def i38():
    "same picture, two formats"
    g = Grid()
    g.image(0, 4, 6, 11, sun=(1, 1))
    g.rect(7, 7, 8, 7, 'Y')
    g.put(9, 6, 'Y'); g.put(9, 8, 'Y'); g.put(9, 7, 'Y')
    g.image(10, 4, 15, 11, sun=(0, 1), sky='t', grass='s')
    return g.rows()


def i39():
    "image going blocky (pixelation = compression)"
    g = Grid()
    g.image(1, 2, 14, 13, sun=(1, 1))
    for y in range(4, 12, 2):
        for x in range(8, 13, 2):
            c = 'g' if y >= 8 else 'b'
            g.rect(x, y, x + 1, y + 1, c)
    g.vline(8, 3, 12, 'w')
    return g.rows()


def i40():
    "image squeezed between two arrows"
    g = Grid()
    g.rect(6, 0, 9, 0, 't')
    g.rect(4, 1, 11, 1, 't')
    g.rect(5, 2, 10, 2, 't')
    g.rect(6, 3, 9, 3, 't')
    g.image(4, 5, 11, 10, sun=(1, 1))
    g.rect(6, 11, 9, 11, 't')
    g.rect(5, 12, 10, 12, 't')
    g.rect(4, 13, 11, 13, 't')
    g.rect(6, 14, 9, 14, 't')
    return g.rows()


def i41():
    "many images become one"
    g = Grid()
    g.image(0, 0, 5, 4, sun=None)
    g.image(0, 5, 5, 9, sun=None)
    g.image(0, 10, 5, 14, sun=None)
    g.rect(6, 7, 8, 7, 'Y')
    g.put(9, 6, 'Y'); g.put(9, 8, 'Y'); g.put(9, 7, 'Y')
    g.image(10, 4, 15, 10, sun=(1, 1))
    return g.rows()


def i42():
    "image with a heavy down arrow (smaller output)"
    g = Grid()
    g.image(0, 1, 9, 10, sun=(1, 1))
    g.rect(12, 2, 14, 2, 'Y')
    g.rect(12, 3, 14, 3, 'Y')
    g.rect(12, 4, 14, 4, 'Y')
    g.rect(12, 5, 14, 5, 'Y')
    g.rect(11, 6, 15, 6, 'Y')
    g.rect(11, 7, 15, 7, 'y')
    g.rect(12, 8, 14, 8, 'y')
    g.rect(13, 9, 13, 9, 'y')
    return g.rows()


# ---- 43-45: batch / misc ----------------------------------------------------

def i43():
    "batch: four images, one getting shaved"
    g = Grid()
    g.image(0, 0, 6, 6, sun=(1, 1))
    g.image(0, 8, 6, 14, sun=(1, 1))
    g.image(8, 8, 14, 14, sun=(1, 1))
    g.image(8, 0, 14, 6, sun=(1, 1))
    g.rect(12, 0, 14, 1, '.')
    g.put(14, 2, '.')
    for x, y in [(13, 0), (12, 1)]:
        g.put(x, y, 's')
    g.put(14, 3, 'Y')
    return g.rows()


def i44():
    "razor sliding out of a photo stack"
    g = Grid()
    g.rect(4, 1, 14, 8, 'S')
    g.rect(5, 2, 13, 7, 's')
    g.image(1, 5, 11, 14, sun=(1, 1))
    g.rect(10, 9, 14, 10, 'w')
    g.rect(10, 11, 14, 11, 'S')
    g.put(11, 10, 'd'); g.put(13, 10, 'd')
    return g.rows()


def i45():
    "image zipped shut (compression)"
    g = Grid()
    g.image(1, 2, 14, 13, sun=(2, 1))
    for y in range(3, 13):
        g.put(7, y, 's' if y % 2 else 'S')
        g.put(8, y, 'S' if y % 2 else 's')
    g.rect(6, 0, 7, 1, 't')
    g.put(8, 0, 'T'); g.put(8, 1, 'T')
    return g.rows()




def i46():
    "clean & optimized image sparkle"
    g = Grid()
    g.image(1, 3, 11, 13, sun=(1, 1))
    g.put(14, 1, 'w'); g.put(14, 2, 'w'); g.put(13, 2, 'w'); g.put(15, 2, 'w')
    g.put(14, 3, 'w')
    g.put(12, 0, 'Y'); g.put(14, 5, 'Y')
    return g.rows()


def i47():
    "image with down arrow"
    g = Grid()
    g.image(1, 2, 11, 12, sun=(1, 1))
    g.rect(13, 3, 15, 5, 'Y')
    g.rect(13, 6, 15, 6, 'Y')
    g.rect(12, 7, 16 - 1, 7, 'Y')
    g.rect(13, 8, 15, 8, 'y')
    g.put(14, 9, 'y')
    return g.rows()


def i48():
    "razor with a pixel spray trail"
    g = Grid()
    g.razor_diag(9, 0, c='t', n=4)
    for x, y, c in [(4, 6, 'w'), (2, 8, 's'), (0, 10, 'w'), (6, 8, 'Y'), (3, 11, 'B'), (1, 13, 'G')]:
        g.put(x, y, c)
    return g.rows()


def i49():
    "B sliced in half"
    g = Grid()
    g.glyph(4, 1, 'B', 'w', scale=2)
    g.rect(2, 8, 13, 8, 's')
    g.put(3, 8, 'd'); g.put(5, 8, 'd'); g.put(7, 8, 'd'); g.put(9, 8, 'd'); g.put(11, 8, 'd')
    for y in range(9, 14):
        for x in range(4, 12):
            if g.g[y][x] == 'w':
                g.g[y][x] = 's'
    g.put(14, 7, 'Y'); g.put(1, 12, 'w')
    return g.rows()


def i50():
    "batch loop around an image"
    g = Grid()
    g.image(4, 4, 11, 11, sun=(1, 1))
    for x, y in [(3, 3), (2, 2), (1, 1), (7, 0), (11, 1), (13, 3), (14, 6), (14, 9),
                 (13, 12), (11, 14), (7, 15), (3, 14), (1, 12), (0, 9), (0, 6), (1, 4)]:
        g.put(x, y, 's' if (x + y) % 2 else 'w')
    g.put(15, 12, 'Y'); g.put(15, 13, 'Y'); g.put(14, 13, 'Y')
    g.put(12, 1, 'Y')
    return g.rows()


ICONS = {f"{i:02d}-{name}": fn for i, name, fn in [
    (6, 'razor-vertical', i06), (7, 'razor-horizontal', i07), (8, 'razor-amber', i08),
    (9, 'safety-razor', i09), (10, 'razor-cut-trail', i10),
    (11, 'electric-led', i11), (12, 'mini-trimmer', i12), (13, 'electric-angled', i13),
    (14, 'foil-closeup', i14), (15, 'electric-cable', i15),
    (16, 'razor-split', i16), (17, 'razor-mirror', i17), (18, 'razor-strip', i18),
    (19, 'razor-tiny-image', i19), (20, 'razor-polaroid', i20), (21, 'razor-clippings', i21),
    (22, 'razor-photo-stack', i22),
    (23, 'electric-image-left', i23), (24, 'electric-strip', i24), (25, 'electric-haircut', i25),
    (26, 'electric-bytes', i26), (27, 'electric-stack', i27), (28, 'trimmer-tiny-image', i28),
    (29, 'B-cut', i29), (30, 'B-razor', i30), (31, 'B-bits', i31), (32, 'byte-chip', i32),
    (33, 'data-stream-cut', i33), (34, 'B-badge', i34), (35, 'B-flat-top', i35),
    (36, 'image-byte-strip', i36),
    (37, 'big-to-small', i37), (38, 'two-formats', i38), (39, 'pixelation', i39),
    (40, 'squeeze-image', i40), (41, 'many-to-one', i41), (42, 'image-down', i42),
    (43, 'batch-shave', i43), (44, 'stack-slide', i44), (45, 'image-zip', i45),
    (46, 'image-sparkle', i46), (47, 'image-down-arrow', i47), (48, 'shave-trail', i48),
    (49, 'B-slice', i49), (50, 'batch-loop', i50),
]}

def svg_for(rows):
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE} {SIZE}" '
        f'width="{SIZE}" height="{SIZE}" shape-rendering="crispEdges">',
        '  <!-- pixel-art proposition: 16x16 grid, 32px cells, flat palette, transparent bg -->',
    ]
    for y, row in enumerate(rows):
        assert len(row) == N, f"row {y}: {len(row)}"
        x = 0
        while x < N:
            c = row[x]
            if c == '.':
                x += 1
                continue
            x0 = x
            while x < N and row[x] == c:
                x += 1
            parts.append(f'  <rect x="{x0*CELL}" y="{y*CELL}" width="{(x-x0)*CELL}" '
                         f'height="{CELL}" fill="{PAL[c]}"/>')
    parts.append('</svg>')
    return '\n'.join(parts) + '\n'


def write_png(path, w, h, px):
    raw = b''.join(b'\x00' + bytes(v for p in row for v in p) for row in px)

    def chunk(tag, data):
        c = struct.pack('>I', len(data)) + tag + data
        return c + struct.pack('>I', zlib.crc32(tag + data) & 0xffffffff)

    png = b'\x89PNG\r\n\x1a\n'
    png += chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0))
    png += chunk(b'IDAT', zlib.compress(raw, 9))
    png += chunk(b'IEND', b'')
    with open(path, 'wb') as f:
        f.write(png)


def hex2rgb(hx):
    return (int(hx[1:3], 16), int(hx[3:5], 16), int(hx[5:7], 16))


def render(rows, cell, bg=(0, 0, 0, 0)):
    img = [[bg] * (N * cell) for _ in range(N * cell)]
    for y, row in enumerate(rows):
        for x, c in enumerate(row):
            if c == '.':
                continue
            r, g, b = hex2rgb(PAL[c])
            for yy in range(y * cell, (y + 1) * cell):
                for xx in range(x * cell, (x + 1) * cell):
                    img[yy][xx] = (r, g, b, 255)
    return img


META = {
    '06-razor-vertical': ("Razor, vertical", "The hand razor head-up: bold slotted cartridge, teal grip handle with rubber stripes. Symmetric and calm; a strong taskbar silhouette."),
    '07-razor-horizontal': ("Razor, horizontal", "The same tool lying on its side — a wide, low mark that fills taskbar rectangles nicely."),
    '08-razor-amber': ("Razor, amber grip", "Diagonal razor with an amber grip — the warm-accent variant of the classic diagonal."),
    '09-safety-razor': ("Safety razor", "The classic T-shaped double-edge razor. Instantly reads 'shaver' even at 16px; monochrome-safe."),
    '10-razor-cut-trail': ("Razor, fresh cut", "The razor with a dashed trail and sparks — motion, something was just shaved."),
    '11-electric-led': ("Electric shaver, LED", "Foil shaver with a green charge LED next to the amber power button — the 'device' face of the brand."),
    '12-mini-trimmer': ("Mini trimmer", "A slim single-purpose trimmer: narrow foil, long body. The most minimal electric silhouette."),
    '13-electric-angled': ("Electric shaver, angled", "The electric shaver tilted 45° — same energy as the diagonal razor, foil head instead."),
    '14-foil-closeup': ("Foil close-up", "Just the perforated foil, magnified: an abstract steel grid with a shine. Abstract-logo territory."),
    '15-electric-cable': ("Electric shaver, cabled", "Shaver with its cable curling away — a bit of character for larger sizes."),
    '16-razor-split': ("Razor split", "An image sliced along the diagonal, the top-right half removed — the boldest 'cutting an image' statement."),
    '17-razor-mirror': ("Razor, mirrored cut", "The corner shave mirrored to the top-left; same story as the flagship, opposite sweep."),
    '18-razor-strip': ("Edge strip shave", "A thin strip being sliced off the image's right edge — shaving as a strip of pixels, not a corner."),
    '19-razor-tiny-image': ("Big razor, tiny image", "A tiny photo being nibbled by an oversized razor — playful size contrast, very legible at 16px."),
    '20-razor-polaroid': ("Polaroid shave", "The classic thick-framed instant photo losing its corner to a blade cartridge. Nostalgic and literal."),
    '21-razor-clippings': ("Shaved clippings", "The corner bite plus clippings falling like hair — sky, sun and grass pixels dropping away."),
    '22-razor-photo-stack': ("Photo stack shave", "Two stacked photos; the razor is shaving the top one. 'Batch' enters the story."),
    '23-electric-image-left': ("Electric trim, mirrored", "The electric shaver trimming an image from the left — mirror of the flagship combination."),
    '24-electric-strip': ("Electric edge strip", "The foil pushes a strip of image off the right edge — conversion as a conveyor."),
    '25-electric-haircut': ("Image haircut", "The shaver passes over the image's top edge leaving a fresh hairline — the gentlest shave of the set."),
    '26-electric-bytes': ("Shaving bytes off", "The combination that spells the name: the shaver trims the image and green 0/1 bytes fall out."),
    '27-electric-stack': ("Electric stack shave", "The electric shaver working through a stack of photos — batch processing, told in one frame."),
    '28-trimmer-tiny-image': ("Trimmer, tiny image", "Minimal pairing: a small trimmer and a small photo, corner to corner."),
    '29-B-cut': ("B, corner shaved", "The brand initial with its corner freshly shaved off, razor still resting in the cut."),
    '30-B-razor': ("B with razor", "A teal B leaning against a parked razor — name and tool side by side."),
    '31-B-bits': ("B shedding bits", "The B sheds green 0/1 bits — bytes leaving the file."),
    '32-byte-chip': ("Byte chip", "A memory-chip marked B getting a trim — bytes as hardware, shave as optimization."),
    '33-data-stream-cut': ("Data stream cut", "A row of 0/1 data with a razor cut through it; the tail bytes fall away."),
    '34-B-badge': ("B badge", "The initial framed as an app-icon badge — the most conventional mark of the B family."),
    '35-B-flat-top': ("B, flat top", "The razor gave the B a fresh flat-top haircut. A little humor, very readable."),
    '36-image-byte-strip': ("Image to bytes", "The image sheds a vertical strip of 0/1 bytes — pixels becoming data under the blade."),
    '37-big-to-small': ("Big to small", "A big washed-out frame becomes a small vivid tile — the whole product promise in three glyphs."),
    '38-two-formats': ("Two formats", "The same picture as two different tiles with a swap arrow between — conversion without words."),
    '39-pixelation': ("Pixelation", "One image, split in half: fine on the left, chunky blocks on the right — compression as coarser pixels."),
    '40-squeeze-image': ("Squeeze", "Chunky arrows pressing a classic image icon — compression as pressure, in image-icon colors."),
    '41-many-to-one': ("Many to one", "A column of photos collapses into a single tile — batch input, single output."),
    '42-image-down': ("Image, down", "The image icon with a heavy down arrow — 'smaller' as a single gesture."),
    '43-batch-shave': ("Batch shave", "Four images in a grid, the razor working on the last one — batch + shave in one mark."),
    '44-stack-slide': ("Stack slide", "The blade slides out from between a stack of photos — batch as a deck of images."),
    '45-image-zip': ("Image, zipped", "The image icon zipped shut down the middle — compression as a zipper."),
    '46-image-sparkle': ("Clean image", "The image icon with a sparkle — 'optimized', the quiet before/after."),
    '47-image-down-arrow': ("Image, down arrow", "A calm variant of the shrink story: image icon plus a soft amber down arrow."),
    '48-shave-trail': ("Shave trail", "The razor with a spray of image-colored pixels behind it — pure motion, minimal shapes."),
    '49-B-slice': ("B, sliced", "The B cut clean through; the lower half fades to steel. Edgy, typographic."),
    '50-batch-loop': ("Batch loop", "A dotted orbit circles the image — everything processed, nothing missed, batch as a loop."),

    '01-razor': ("Razor", "The tool alone: a big steel blade cartridge with bold slots and a chunky striped teal handle on a diagonal."),
    '02-electric-shaver': ("Electric shaver", "Wide perforated foil head, teal body, amber power button."),
    '03-razor-image': ("Razor shaving an image", "Classic image icon with its corner cleanly shaved off; trimmed pixels drift away in image colors."),
    '04-electric-image': ("Electric shaver trimming an image", "The foil head sits right on the image's corner mid-shave, pixels flying out from under it."),
    '05-torn-image': ("Torn image", "A nod to the old-browser missing-image icon: torn zigzag edge, razor resting where the tear starts."),
}


def build_index(names):
    cards = []
    for name in names:
        title, desc = META[name]
        cards.append(f'''<div class="card" id="c{name.split('-')[0]}">
  <h2>{name.split('-')[0]} — {title}</h2>
  <p class="note">{desc}</p>
  <div class="row">
    <span class="swatch"><span class="dark"><img src="{name}.svg" width="256"></span><br>256px dark</span>
    <span class="swatch"><span class="dark"><img src="{name}.svg" width="64"></span><br>64px</span>
    <span class="swatch"><span class="light"><img src="{name}.svg" width="64"></span><br>64px light</span>
    <span class="swatch"><span class="dark"><img src="{name}.svg" width="32"></span><br>32px</span>
    <span class="swatch"><span class="dark"><img src="{name}.svg" width="16"></span><br>16px</span>
  </div>
</div>''')
    return '''<!doctype html>
<!-- Preview contact sheet for the byteshaver pixel-art propositions.
     Open in a browser: docs/img/icon-proposals/shaver/index.html
     All propositions are pixel art: 16x16 grids at 32px cells, transparent
     background. Grids live in generate.py (01-05) and generate-more.py
     (06-50); edit and re-run. -->
<html lang="en">
<head>
<meta charset="utf-8">
<title>byteshaver pixel-art icon propositions</title>
<style>
  :root { color-scheme: dark light; }
  body { font-family: system-ui, sans-serif; margin: 2rem; background: #f1f5f9; color: #0f172a; }
  .card { background: #fff; border-radius: 14px; padding: 1.25rem; margin-bottom: 1.5rem;
          box-shadow: 0 1px 4px rgba(0,0,0,.12); }
  h1 { font-size: 1.4rem; } h2 { font-size: 1.05rem; margin: 0 0 .25rem; }
  p.note { color: #475569; margin: .2rem 0 .9rem; max-width: 70ch; }
  .row { display: flex; align-items: flex-end; gap: 1.5rem; flex-wrap: wrap; }
  .swatch { text-align: center; font-size: .72rem; color: #64748b; }
  .dark, .light { padding: .6rem; border-radius: 10px; display: inline-block; }
  .dark { background: #0f172a; } .light { background: #f8fafc; border: 1px solid #e2e8f0; }
  img { display: block; image-rendering: pixelated; }
  img.px16, img.px32, img.px64 { border-radius: 4px; }
</style>
</head>
<body>
<h1>byteshaver — pixel-art icon propositions</h1>
<p>Pixel art on a 16×16 grid (32px cells, flat palette, hard edges,
   <code>shape-rendering="crispEdges"</code>), transparent background — the tiles
   below just prove it works on both. Symbolism stays on the name (shaver,
   bytes) and the function (batch image conversion / shrinking). 01–05 are the
   first shaver set; 06–50 extend it. Regenerate via
   <code>generate.py</code> / <code>generate-more.py</code>.</p>
 ''' + '\n\n'.join(cards) + '\n\n</body>\n</html>\n'


def main():
    os.makedirs(OUT, exist_ok=True)
    names = list(ICONS)
    for name in names:
        with open(f"{OUT}/{name}.svg", "w") as f:
            f.write(svg_for(ICONS[name]()))
    with open(f"{OUT}/index.html", "w") as f:
        f.write(build_index(sorted(set(names) | set(META))))

    dark = hex2rgb('#0f172a') + (255,)
    light = hex2rgb('#f1f5f9') + (255,)
    per = 9
    rows_n = (len(names) + per - 1) // per
    gap = 8
    big, small = 4, 1
    W = max(per * N * big, len(names) * N * small)
    H = rows_n * N * big + gap * (rows_n - 1) + gap + N * small * 2
    sheet = [[(60, 60, 70, 255)] * W for _ in range(H)]

    def paste(img, x0, y0):
        for yy in range(len(img)):
            for xx in range(len(img[0])):
                sheet[y0 + yy][x0 + xx] = img[yy][xx]

    y = 0
    for r in range(rows_n):
        for c, name in enumerate(names[r * per:(r + 1) * per]):
            img = render(ICONS[name](), big)
            for yy in range(N * big):
                for xx in range(N * big):
                    if img[yy][xx][3] == 0:
                        a = (144, 144, 144, 255) if ((xx // big + yy // big) % 2 == 0) else (114, 114, 114, 255)
                        img[yy][xx] = a
            paste(img, c * N * big, y)
        y += N * big + gap
    for c, name in enumerate(names):
        img = render(ICONS[name](), small)
        for yy in range(N * small):
            for xx in range(N * small):
                if img[yy][xx][3] == 0:
                    img[yy][xx] = dark
        paste(img, c * N * small, y)
    write_png(os.path.join(OUT, 'qa-sheet-more.png'), W, H, sheet)
    print("OK:", len(names), "icons")


if __name__ == '__main__':
    main()

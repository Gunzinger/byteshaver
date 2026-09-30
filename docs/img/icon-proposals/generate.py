#!/usr/bin/env python3
"""Generate pixel-art icon proposals for byteshaver (16x16 grid -> SVG) + QA sheet."""
import zlib, struct, os

OUT = os.path.dirname(os.path.abspath(__file__))

PAL = {
    'd': '#020617',  # outline / near-black
    'g': '#334155',  # dark slate
    'G': '#64748b',  # mid slate
    'l': '#94a3b8',  # light slate
    's': '#cbd5e1',  # steel
    'w': '#f1f5f9',  # white
    't': '#2dd4bf',  # teal
    'T': '#0d9488',  # deep teal
    'a': '#fbbf24',  # amber
    'A': '#d97706',  # deep amber
}
BG = '#0b1220'
N = 16
CELL = 32
SIZE = N * CELL  # 512
RX = 112


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

    def stamp(self, x0, y0, rows):
        for dy, row in enumerate(rows):
            for dx, c in enumerate(row):
                if c != ' ':
                    self.put(x0 + dx, y0 + dy, c)

    def photo_card(self, x0, y0, x1, y1, sky='T', vivid=True):
        """pixel mini-landscape card with 1px outline; (x0,y0)-(x1,y1) inclusive."""
        self.rect(x0, y0, x1, y1, 'd')
        ix0, iy0, ix1, iy1 = x0 + 1, y0 + 1, x1 - 1, y1 - 1
        horizon = iy0 + (iy1 - iy0) * 2 // 3
        self.rect(ix0, iy0, ix1, horizon - 1, sky if vivid else 'l')
        self.rect(ix0, horizon, ix1, iy1, 'G' if vivid else 'g')
        if vivid:
            sw = min(2, (ix1 - ix0) // 3)
            self.rect(ix0 + 1, iy0 + 1, ix0 + sw, iy0 + 1 + sw - 1, 'a')

    def rows(self):
        return [''.join(r) for r in self.g]


ICONS = {}


def icon_razor_slice():
    """photo card sliced by a steel razor diagonal, corner drifting away."""
    g = Grid()
    g.photo_card(1, 3, 10, 13)
    # bite the top-right corner off the card (diagonal staircase)
    g.rect(8, 3, 10, 3, '.')
    g.rect(9, 4, 10, 4, '.')
    g.put(10, 5, '.')
    # razor blade: steel diagonal through the bite, extending past the card
    for x, y in [(12, 0), (11, 1), (10, 2), (9, 3), (8, 4)]:
        g.put(x, y, 's')
    g.put(8, 4, 'w')
    # shaved-off pixels drifting away + spark at the tip
    g.put(13, 0, 'l')
    g.put(14, 2, 'l')
    g.put(13, 2, 'a')
    return g.rows()


def icon_nested_shrink():
    """dashed pixel frames collapsing into a teal core, inward chevron."""
    g = Grid()
    # outer dashed ring 13x13
    for x in range(1, 14):
        if x % 2 == 1 or x < 4 or x > 10:
            g.put(x, 1, 'G'); g.put(x, 13, 'G')
    for y in range(1, 14):
        if y % 2 == 1 or y < 4 or y > 10:
            g.put(1, y, 'G'); g.put(13, y, 'G')
    # middle ring
    g.rect(4, 4, 11, 11, 'l')
    g.rect(5, 5, 10, 10, '.')
    g.put(4, 4, 'w'); g.put(11, 11, 'G')
    # teal core with shading
    g.rect(6, 6, 9, 9, 't')
    g.hline(6, 9, 9, 'T'); g.vline(9, 6, 9, 'T')
    g.put(6, 6, 'w')
    # inward chevron (amber), pointing down-left at the core
    g.put(13, 2, 'a'); g.put(12, 3, 'a')
    g.put(13, 4, 'a'); g.put(14, 3, 'a')
    return g.rows()


def icon_pixel_shave():
    """pure pixel grid, top-right blocks shaved off diagonally, teal accents."""
    g = Grid()
    blocks = [(1, 1), (5, 1), (9, 1), (13, 1),
              (1, 5), (5, 5), (9, 5), (13, 5),
              (1, 9), (5, 9), (9, 9), (13, 9),
              (1, 13), (5, 13), (9, 13), (13, 13)]
    cut_line = {(13, 1), (13, 5), (9, 5)}          # removed blocks (diagonal bite)
    keep = {(5, 1): 't', (9, 1): 't'}              # teal accent near cut
    for (bx, by) in blocks:
        if (bx, by) in cut_line:
            continue
        c = keep.get((bx, by), 'G')
        g.rect(bx, by, bx + 2, by + 2, c)
    # steel blade staircase hugging the bite
    g.put(13, 5, 's'); g.put(14, 5, 's')
    g.put(12, 7, 's'); g.put(13, 7, 's')
    g.put(11, 8, 's')
    # flying shaved pixels
    g.put(14, 1, 'l'); g.put(15, 3, 'w')
    return g.rows()


def icon_squeeze():
    """chunky arrows pressing a photo tile smaller."""
    g = Grid()
    # top arrow (down): shaft + stepped head
    g.rect(6, 1, 9, 1, 't')
    g.rect(4, 2, 11, 2, 't')
    g.rect(5, 3, 10, 3, 't')
    g.rect(6, 4, 9, 4, 't')
    # tile
    g.photo_card(4, 6, 11, 9)
    # bottom arrow (up): stepped head + shaft
    g.rect(6, 11, 9, 11, 't')
    g.rect(5, 12, 10, 12, 't')
    g.rect(4, 13, 11, 13, 't')
    g.rect(6, 14, 9, 14, 't')
    return g.rows()


def icon_blade_badge():
    """classic safety-razor blade, pixel silhouette."""
    g = Grid()
    # flat, wide blade body: light top edge, steel body, shaded base
    g.hline(3, 12, 3, 'w')
    g.rect(1, 4, 14, 10, 's')
    g.rect(2, 11, 13, 11, 'G')
    # center slot
    g.rect(5, 6, 10, 8, '.')
    g.hline(6, 9, 9, 's')
    # side notches
    g.put(1, 7, '.'); g.put(1, 8, '.')
    g.put(14, 7, '.'); g.put(14, 8, '.')
    # teal glint
    g.put(13, 4, 't')
    return g.rows()


def icon_before_after():
    """big faded frame -> chunky arrow -> small vivid tile."""
    g = Grid()
    # big faded photo
    g.photo_card(0, 3, 6, 12, vivid=False)
    # amber arrow to the result
    g.rect(7, 8, 8, 8, 'a')
    g.rect(9, 7, 9, 9, 'a')
    g.put(10, 8, 'a')
    # small vivid tile
    g.photo_card(11, 6, 14, 10)
    return g.rows()


ICONS = {
    '01-razor-slice': icon_razor_slice,
    '02-nested-shrink': icon_nested_shrink,
    '03-pixel-shave': icon_pixel_shave,
    '04-squeeze': icon_squeeze,
    '05-blade-badge': icon_blade_badge,
    '06-before-after': icon_before_after,
}


def svg_for(rows):
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE} {SIZE}" '
        f'width="{SIZE}" height="{SIZE}" shape-rendering="crispEdges">',
        '  <!-- pixel-art proposal: 16x16 grid, 32px cells, flat palette, crisp edges -->',
        f'  <rect width="{SIZE}" height="{SIZE}" rx="{RX}" fill="{BG}"/>',
    ]
    for y, row in enumerate(rows):
        assert len(row) == N
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


def render(rows, cell):
    bg = hex2rgb(BG) + (255,)
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


def main():
    os.makedirs(OUT, exist_ok=True)
    names = list(ICONS)
    for name in names:
        with open(f"{OUT}/{name}.svg", "w") as f:
            f.write(svg_for(ICONS[name]()))

    # QA sheet: big row (128px) + small row (32px)
    cell_big, cell_small = 8, 2
    W = len(names) * N * cell_big
    H = N * cell_big + 20 + N * cell_small
    bgc = (24, 24, 32, 255)
    sheet = [[bgc] * W for _ in range(H)]
    for i, name in enumerate(names):
        rows = ICONS[name]()
        big = render(rows, cell_big)
        small = render(rows, cell_small)
        x0 = i * N * cell_big
        y0 = N * cell_big + 20
        for y in range(N * cell_big):
            for x in range(N * cell_big):
                sheet[y][x0 + x] = big[y][x]
        for y in range(N * cell_small):
            for x in range(N * cell_small):
                sheet[y0 + y][x0 + x] = small[y][x]
    write_png(os.path.join(OUT, 'qa-sheet.png'), W, H, sheet)
    print("OK:", ', '.join(f"{n}.svg" for n in names))


if __name__ == '__main__':
    main()

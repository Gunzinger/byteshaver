#!/usr/bin/env python3
"""Generate pixel-art shaver icon propositions for byteshaver (16x16 grid -> SVG) + QA sheet.

Transparent background; the image-icon propositions use the classic pixel
"image file" palette (blue sky, green hill, yellow sun, white frame — the old
browser broken-image look). Edit the grids, re-run: python3 generate.py
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
    'd': '#0b1220',  # near-black detail (blade slots, grips)
    '.': None,
}
N = 16
CELL = 32
SIZE = N * CELL  # 512


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

    def image_frame(self, x0, y0, x1, y1, sun=(2, 2)):
        """classic pixel image icon: dark outline, white frame, blue sky,
        green hill, yellow sun. (x0,y0)-(x1,y1) inclusive."""
        self.rect(x0, y0, x1, y1, 'K')
        self.rect(x0 + 1, y0 + 1, x1 - 1, y1 - 1, 'W')
        ix0, iy0, ix1, iy1 = x0 + 2, y0 + 2, x1 - 2, y1 - 2
        horizon = iy1 - max(1, (iy1 - iy0) // 3)
        self.rect(ix0, iy0, ix1, horizon - 1, 'B')
        self.rect(ix0, horizon, ix1, iy1, 'G')
        self.hline(ix0, ix1, horizon, 'g')
        if sun:
            sx, sy = ix0 + sun[0], iy0 + sun[1]
            self.rect(sx, sy, sx + 1, sy + 1, 'Y')
            self.put(sx + 1, sy + 1, 'y')

    def rows(self):
        return [''.join(r) for r in self.g]


ICONS = {}


def icon_razor():
    """classic hand razor, diagonal: big steel cartridge top-right, chunky teal handle."""
    g = Grid()
    # cartridge (blade head), side view — big and bold
    g.hline(8, 13, 1, 'w')
    g.rect(8, 2, 14, 2, 's')
    g.put(9, 2, 'd'); g.put(11, 2, 'd'); g.put(13, 2, 'd')
    g.rect(8, 3, 14, 3, 's')
    g.hline(8, 14, 4, 'S')
    # neck
    g.put(7, 5, 'S'); g.put(6, 6, 'S')
    # handle: 3-wide diagonal down-left, teal with dark grip stripes
    for i, y in enumerate(range(6, 12)):
        x = 5 - i
        g.hline(x - 1, x + 1, y, 't')
        g.put(x + 1, y, 'T')                 # shade edge
        if i in (2, 3):
            g.put(x, y, 'd')                 # grip stripes
    g.put(0, 12, 'K'); g.put(1, 12, 'K')     # end cap
    return g.rows()


def icon_electric():
    """electric foil shaver, front view: wide steel foil head, teal body, amber button."""
    g = Grid()
    # foil head, wider than the body
    g.hline(4, 11, 1, 'w')
    g.rect(3, 2, 12, 3, 's')
    for x in range(4, 12, 2):
        g.put(x, 2, 'S'); g.put(x + 1, 3, 'S')
    # head frame
    g.rect(3, 4, 12, 4, 'S')
    # body with outline
    g.rect(4, 5, 11, 13, 'K')
    g.rect(5, 5, 10, 12, 't')
    g.vline(10, 5, 12, 'T')          # shading
    # power button
    g.rect(7, 8, 8, 9, 'Y')
    g.put(8, 9, 'y')
    # base cap
    g.rect(4, 13, 11, 13, 'K')
    return g.rows()


def icon_razor_image():
    """classic image icon having its corner shaved off by a razor."""
    g = Grid()
    g.image_frame(1, 4, 11, 13, sun=(1, 1))
    # bite the top-right corner (frame + picture)
    g.rect(9, 4, 11, 5, '.')
    g.put(11, 6, '.')
    # razor diagonal through the bite
    for x, y in [(13, 1), (12, 2), (11, 3), (10, 4), (9, 5)]:
        g.put(x, y, 's')
    g.put(14, 0, 'w')
    g.put(13, 0, 'd')
    # shaved-off image pixels drift away (in image colors)
    g.put(14, 2, 'Y'); g.put(13, 3, 'B'); g.put(15, 4, 'G')
    return g.rows()


def icon_electric_image():
    """electric shaver trimming the corner of a classic image icon."""
    g = Grid()
    # image, bottom-left
    g.image_frame(0, 5, 9, 14, sun=(1, 1))
    # bite at its top-right corner, directly under the foil
    g.rect(8, 5, 9, 6, '.')
    # electric shaver: foil head right on top of the bite, body to the right
    g.hline(9, 12, 0, 'w')
    g.rect(8, 1, 12, 2, 's')
    g.put(9, 1, 'S'); g.put(11, 2, 'S')
    g.rect(8, 3, 12, 3, 'S')
    # body
    g.rect(9, 4, 12, 10, 'K')
    g.rect(10, 4, 12, 9, 't')
    g.vline(12, 4, 9, 'T')
    g.rect(10, 7, 11, 8, 'Y')
    g.rect(9, 10, 12, 10, 'K')
    # trimmed image pixels flying out from under the foil
    g.put(7, 3, 'B'); g.put(6, 2, 'Y'); g.put(7, 1, 'G')
    return g.rows()


def icon_torn_image():
    """old-browser broken image: torn right edge, razor beside the tear."""
    g = Grid()
    g.image_frame(1, 3, 11, 13, sun=(1, 1))
    # tear the right edge into a zigzag
    for y in range(4, 14):
        if y % 2 == 0:
            g.rect(10, y, 11, y, '.')
    # razor that did the tearing, along the top-right tear
    for x, y in [(14, 0), (13, 1), (12, 2), (11, 3)]:
        g.put(x, y, 's')
    g.put(14, 1, 'w')
    # small scraps drifting
    g.put(13, 4, 'G'); g.put(15, 2, 'B'); g.put(15, 5, 'Y')
    return g.rows()


ICONS = {
    '01-razor': icon_razor,
    '02-electric-shaver': icon_electric,
    '03-razor-image': icon_razor_image,
    '04-electric-image': icon_electric_image,
    '05-torn-image': icon_torn_image,
}


def svg_for(rows, bg=None, rx=None):
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE} {SIZE}" '
        f'width="{SIZE}" height="{SIZE}" shape-rendering="crispEdges">',
        '  <!-- pixel-art proposition: 16x16 grid, 32px cells, flat palette, transparent bg -->',
    ]
    if bg:
        parts.append(f'  <rect width="{SIZE}" height="{SIZE}" rx="{rx}" fill="{bg}"/>')
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


def main():
    os.makedirs(OUT, exist_ok=True)
    names = list(ICONS)
    for name in names:
        with open(f"{OUT}/{name}.svg", "w") as f:
            f.write(svg_for(ICONS[name]()))

    # QA sheet: rows = checker 128, dark 64, light 64, checker 32, dark 16
    def checker_color(xx, yy, cell):
        a, b = (144, 144, 144, 255), (114, 114, 114, 255)
        return a if (xx // cell + yy // cell) % 2 == 0 else b

    dark = hex2rgb('#0f172a') + (255,)
    light = hex2rgb('#f1f5f9') + (255,)
    rows_cfg = [('checker', 8), ('dark', 4), ('light', 4), ('checker', 2), ('dark', 1)]
    gap = 8
    W = len(names) * N * 8
    H = sum(N * c for _, c in rows_cfg) + gap * (len(rows_cfg) - 1)
    sheet = [[(60, 60, 70, 255)] * W for _ in range(H)]
    y = 0
    for kind, cell in rows_cfg:
        for i, name in enumerate(names):
            img = render(ICONS[name](), cell)
            if kind == 'checker':
                for yy in range(N * cell):
                    for xx in range(N * cell):
                        if img[yy][xx][3] == 0:
                            img[yy][xx] = checker_color(xx, yy, cell)
            elif kind == 'dark':
                for yy in range(N * cell):
                    for xx in range(N * cell):
                        if img[yy][xx][3] == 0:
                            img[yy][xx] = dark
            elif kind == 'light':
                for yy in range(N * cell):
                    for xx in range(N * cell):
                        if img[yy][xx][3] == 0:
                            img[yy][xx] = light
            x0 = i * N * 8
            for yy in range(N * cell):
                for xx in range(N * cell):
                    sheet[y + yy][x0 + xx] = img[yy][xx]
        y += N * cell + gap
    write_png(os.path.join(OUT, 'qa-sheet.png'), W, H, sheet)
    print("OK:", ', '.join(f"{n}.svg" for n in names))


if __name__ == '__main__':
    main()

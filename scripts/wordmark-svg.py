#!/usr/bin/env python3
"""Make brand/logo-wordmark.svg: the ThinFloe logo plus the word "ThinFloe".

The word is drawn as outlines taken from Geo-Oblique.ttf, so the SVG needs no
font on the machine that renders it (the image build renders it with
rsvg-convert in a throw-away stage). Run by hand when the logo, the word or
the font changes, and commit the result:

    python3 -m venv /scratch/oetiker/claude-tmp/thinfloe-tests/venv-t8
    /scratch/oetiker/claude-tmp/thinfloe-tests/venv-t8/bin/pip install fonttools
    /scratch/oetiker/claude-tmp/thinfloe-tests/venv-t8/bin/python scripts/wordmark-svg.py

Layout follows the boot-screen mockup (brand-and-boot.html, screen B): the
logo, 12 px gap, the word at 36 px, when the logo is 60 px wide for a 420 unit
view box. The word's cap-height is centred on the logo.
"""
import os
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FONT = os.path.join(ROOT, "src/thinfloe-menu/assets/Geo-Oblique.ttf")
OUT = os.path.join(ROOT, "brand/logo-wordmark.svg")
WORD = "ThinFloe"

# Same constants as src/thinfloe-menu/src/brand.rs.
LOGO_T = "M278 388H625V447H433V632H360V447H278Z"
LOGO_F = "M498 502H624V562H570V632H498Z"
SLANT = "translate(0 632) skewX(-9) translate(0 -632)"

UNIT_PER_PX = 420 / 60          # mockup: 420 units drawn 60 px wide
GAP = 12 * UNIT_PER_PX
FONT_SIZE = 36 * UNIT_PER_PX
LOGO_CENTRE_Y = (388 + 632) / 2
LOGO_RIGHT = 625 + 244 * 0.1584   # the T top edge, after the 9 degree slant
LOGO_LEFT = 278
PAD = 20


def main():
    font = TTFont(FONT)
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    hmtx = font["hmtx"]
    scale = FONT_SIZE / font["head"].unitsPerEm
    cap = getattr(font["OS/2"], "sCapHeight", 0) or font["head"].unitsPerEm * 0.7
    baseline = LOGO_CENTRE_Y + cap * scale / 2

    x = LOGO_RIGHT + GAP
    paths = []
    for ch in WORD:
        name = cmap[ord(ch)]
        pen = SVGPathPen(glyphs, ntos=lambda v: ("%.2f" % v).rstrip("0").rstrip("."))
        glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, x, baseline)))
        d = pen.getCommands()
        if d:
            paths.append(d)
        x += hmtx[name][0] * scale
    word_end = x

    x0, y0 = LOGO_LEFT - PAD, 388 - PAD
    w, h = word_end + PAD - x0, 632 - 388 + 2 * PAD
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="%g %g %g %g" '
        'width="%g" height="%g">\n' % (x0, y0, w, h, round(w), round(h))
        + '<g fill="#fff">\n'
        + '<g transform="%s"><path d="%s"/><path d="%s"/></g>\n' % (SLANT, LOGO_T, LOGO_F)
        + '<path d="%s"/>\n' % "".join(paths)
        + "</g>\n</svg>\n"
    )
    with open(OUT, "w") as f:
        f.write(svg)
    print("wrote %s (%d x %d units)" % (OUT, round(w), round(h)), file=sys.stderr)


if __name__ == "__main__":
    main()

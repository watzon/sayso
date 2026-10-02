"""Draw the background of the release DMG window: a kraft board with an ink arrow.

The Finder draws the icon labels in black in light mode and in white in dark mode,
and the picture cannot change that. The board is a mid tone, so both label colors
have a contrast of about 4.5 to 1.

The window is 600 x 400 points. The icons sit at x = 150 and x = 450, y = 190
(see scripts/release.sh). The output is one TIFF with a 1x and a 2x image.

Usage: python3 assets/dmg/generate.py
"""
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

OUT = Path(__file__).parent
TEXTURE = OUT.parent / "textures" / "board-light.png"
W, H = 600, 400
BOARD = (139, 113, 86)
INK = (29, 27, 24)
SS = 4  # the drawing scale: 2x output, supersampled 2 times


def board(scale):
    w, h = W * scale, H * scale
    img = Image.new("RGB", (w, h), BOARD)
    tile = Image.open(TEXTURE).convert("RGBA")
    for x in range(0, w, tile.width):
        for y in range(0, h, tile.height):
            img.paste(tile, (x, y), tile)
    # Soft shade toward the edges, so the middle of the board looks lit.
    yy, xx = np.mgrid[0:h, 0:w]
    d = np.hypot((xx - w / 2) / (w / 2), (yy - h / 2) / (h / 2))
    shade = 1 - 0.10 * np.clip(d - 0.55, 0, 1) ** 1.5
    return Image.fromarray((np.asarray(img) * shade[..., None]).astype(np.uint8))


def arrow_points():
    """The Sayso wave (see assets/app/generate_icons.py), then a straight run to the tip."""
    segs = [((2, 11), (5, 11), (6, 7), (8, 7)), ((8, 7), (10, 7), (10, 15), (12, 15)),
            ((12, 15), (14, 15), (14, 5), (16, 5)), ((16, 5), (18, 5), (18, 11), (20, 11))]
    pts = []
    for p0, p1, p2, p3 in segs:
        for t in np.linspace(0, 1, 120):
            a, b, c, d = (1 - t) ** 3, 3 * (1 - t) ** 2 * t, 3 * (1 - t) * t ** 2, t ** 3
            pts.append((a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]))
    pts = np.array(pts)
    x = 236 + (pts[:, 0] - 2) * (104 / 18)
    y = 190 + (pts[:, 1] - 11) * 3.4
    run = np.linspace(340, 366, 60)
    return np.concatenate([x, run]), np.concatenate([y, np.full(60, 190.0)])


def ink_layer():
    layer = Image.new("L", (W * SS, H * SS), 0)
    draw = ImageDraw.Draw(layer)

    def stroke(xs, ys, r0, r1):
        n = len(xs)
        for i, (x, y) in enumerate(zip(xs, ys)):
            # A brush stroke: thin at the start, full in the middle, a little thinner at the end.
            t = i / max(n - 1, 1)
            r = (r0 + (r1 - r0) * np.sin(np.pi * min(t * 1.6, 0.5 + t / 2))) * SS
            draw.ellipse([x * SS - r, y * SS - r, x * SS + r, y * SS + r], fill=255)

    xs, ys = arrow_points()
    xs, ys = np.interp(np.linspace(0, len(xs) - 1, 4000), np.arange(len(xs)), xs), np.interp(np.linspace(0, len(ys) - 1, 4000), np.arange(len(ys)), ys)
    stroke(xs, ys, 1.5, 2.7)
    for dy in (-11, 11):
        t = np.linspace(0, 1, 400)
        stroke(366 - 13 * t, 190 + dy * t, 2.4, 1.6)
    return layer


def render(scale):
    img = board(SS).convert("RGB")
    ink = ink_layer()
    # A faint bleed around the stroke, as ink has on rough paper.
    bleed = ink.filter(ImageFilter.GaussianBlur(1.2 * SS)).point(lambda v: int(v * 0.25))
    for mask, strength in ((bleed, 1.0), (ink, 0.94)):
        img.paste(Image.new("RGB", img.size, INK), (0, 0), mask.point(lambda v: int(v * strength)))
    return img.resize((W * scale, H * scale), Image.LANCZOS)


def main():
    with tempfile.TemporaryDirectory() as tmp:
        one, two = Path(tmp) / "background.png", Path(tmp) / "background@2x.png"
        render(1).save(one, dpi=(72, 72))
        render(2).save(two, dpi=(144, 144))
        subprocess.run(["tiffutil", "-cathidpicheck", str(one), str(two), "-out", str(OUT / "background.tiff")], check=True)
    print("wrote background.tiff")


if __name__ == "__main__":
    main()

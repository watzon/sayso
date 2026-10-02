"""Generate tileable paper textures for Sayso.

Each output is an RGBA overlay. Paint it on top of a flat surface color, so one
texture works for every ink and for light and dark mode.

Usage: python3 assets/textures/generate.py
"""

from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

SIZE = 512
OUT = Path(__file__).parent


def periodic_noise(rng, size, sigma):
    """Gaussian-filtered white noise. FFT filtering makes it tile seamlessly."""
    white = rng.standard_normal((size, size))
    fy = np.fft.fftfreq(size)[:, None]
    fx = np.fft.fftfreq(size)[None, :]
    kernel = np.exp(-2 * (np.pi * sigma) ** 2 * (fx**2 + fy**2))
    field = np.real(np.fft.ifft2(np.fft.fft2(white) * kernel))
    return field / (np.abs(field).max() + 1e-9)


def fibers(rng, size, count, length, width):
    """Short curved fibers drawn with wrap-around so the tile stays seamless."""
    canvas = Image.new("L", (size * 3, size * 3), 128)
    draw = ImageDraw.Draw(canvas)
    for _ in range(count):
        x, y = rng.uniform(0, size, 2) + size
        angle = rng.uniform(0, np.pi)
        tone = 128 + int(rng.choice([-1, 1], p=[0.35, 0.65]) * rng.uniform(40, 90))
        points = []
        for _ in range(int(rng.uniform(0.5, 1.0) * length)):
            angle += rng.normal(0, 0.12)
            x += np.cos(angle)
            y += np.sin(angle)
            points.append((x, y))
        for ox in (-size, 0, size):
            for oy in (-size, 0, size):
                draw.line([(px + ox, py + oy) for px, py in points], fill=tone, width=width)
    canvas = canvas.filter(ImageFilter.GaussianBlur(0.6))
    tile = np.asarray(canvas, dtype=np.float64)[size : 2 * size, size : 2 * size]
    return (tile - 128) / 128


def flecks(rng, size, count):
    """Rare dark inclusions, like specks of bark in rag paper."""
    field = np.zeros((size, size))
    for _ in range(count):
        cx, cy = rng.integers(0, size, 2)
        r = rng.uniform(0.6, 1.6)
        yy, xx = np.ogrid[-3:4, -3:4]
        blob = np.exp(-(xx**2 + yy**2) / (2 * r**2)) * rng.uniform(0.6, 1.0)
        for dy in range(7):
            for dx in range(7):
                field[(cy + dy - 3) % size, (cx + dx - 3) % size] -= blob[dy, dx]
    return field


def overlay(luma, dark_rgb, light_rgb, strength):
    """Map a signed luminance field to an RGBA overlay."""
    luma = np.clip(luma, -1, 1)
    rgba = np.zeros((SIZE, SIZE, 4), dtype=np.uint8)
    darker = luma < 0
    rgba[darker, :3] = dark_rgb
    rgba[~darker, :3] = light_rgb
    rgba[..., 3] = np.clip(np.abs(luma) * strength * 255, 0, 255).astype(np.uint8)
    return Image.fromarray(rgba, "RGBA")


def unit(field):
    """Scale a field to unit standard deviation so the weights mean the same thing."""
    return field / (field.std() + 1e-9)


def paper_field(seed, mottle, grain, fiber, fiber_count, fleck_count):
    rng = np.random.default_rng(seed)
    field = (
        mottle * unit(periodic_noise(rng, SIZE, 9))
        + grain * unit(periodic_noise(rng, SIZE, 0.6))
        + fiber * unit(fibers(rng, SIZE, fiber_count, 70, 1))
        + 2.5 * flecks(rng, SIZE, fleck_count)
    )
    return np.tanh(field / 3)


def main():
    # Content sheet: almost clean. Text sits here, so the texture stays very soft.
    sheet = paper_field(seed=7, mottle=0.3, grain=1.0, fiber=0.35, fiber_count=120, fleck_count=4)
    overlay(sheet, (92, 74, 48), (255, 253, 248), 0.05).save(OUT / "sheet-light.png")
    overlay(sheet, (0, 0, 0), (236, 230, 218), 0.03).save(OUT / "sheet-dark.png")

    # Board (window background and sidebar): this carries the paper texture.
    board = paper_field(seed=11, mottle=0.5, grain=1.1, fiber=0.7, fiber_count=220, fleck_count=16)
    overlay(board, (84, 66, 40), (255, 250, 240), 0.14).save(OUT / "board-light.png")
    overlay(board, (0, 0, 0), (236, 230, 218), 0.055).save(OUT / "board-dark.png")


if __name__ == "__main__":
    main()

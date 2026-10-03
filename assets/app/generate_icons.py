"""Draw the menu bar template icon (the Sayso ink wave) as PNG.

Usage: python3 assets/app/generate_icons.py
"""
import struct
import zlib
from pathlib import Path

import numpy as np

OUT = Path(__file__).parent


def png(path, rgba):
    h, w, _ = rgba.shape
    raw = b"".join(b"\x00" + rgba[y].tobytes() for y in range(h))
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    path.write_bytes(data)


def wave(size, stroke):
    """The wordmark: M2,11 C5,11 6,7 8,7 C10,7 10,15 12,15 C14,15 14,5 16,5 C18,5 18,11 20,11 in a 22-unit box."""
    segs = [((2, 11), (5, 11), (6, 7), (8, 7)), ((8, 7), (10, 7), (10, 15), (12, 15)),
            ((12, 15), (14, 15), (14, 5), (16, 5)), ((16, 5), (18, 5), (18, 11), (20, 11))]
    pts = []
    for p0, p1, p2, p3 in segs:
        for t in np.linspace(0, 1, 200):
            a = (1 - t) ** 3; b = 3 * (1 - t) ** 2 * t; c = 3 * (1 - t) * t ** 2; d = t ** 3
            pts.append((a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]))
    pts = np.array(pts) * (size / 22.0)
    ss = 4  # supersample
    yy, xx = np.mgrid[0:size * ss, 0:size * ss] / ss
    dist = np.full(xx.shape, 1e9)
    for x, y in pts[::2]:
        dist = np.minimum(dist, (xx - x) ** 2 + (yy - y) ** 2)
    r = stroke * size / 22.0 / 2.0
    cov = np.clip(r + 0.5 / ss - np.sqrt(dist), 0, 1 / ss) * ss
    alpha = cov.reshape(size, ss, size, ss).mean(axis=(1, 3))
    rgba = np.zeros((size, size, 4), dtype=np.uint8)
    rgba[..., 3] = (alpha * 255).round().astype(np.uint8)
    return rgba


if __name__ == "__main__":
    png(OUT / "tray.png", wave(36, 2.6))
    print("wrote tray.png")


def app_icon(size=1024):
    """Rag paper square with the ink wave, macOS icon grid (824 px body on 1024)."""
    s = size
    body = int(s * 824 / 1024)
    off = (s - body) // 2
    radius = body * 0.225
    yy, xx = np.mgrid[0:s, 0:s].astype(np.float64)
    # Rounded-rect coverage.
    cx = np.clip(xx, off + radius, off + body - radius)
    cy = np.clip(yy, off + radius, off + body - radius)
    d = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2)
    cov = np.clip(radius - d + 0.5, 0, 1)
    # Paper: warm off-white with a soft top light and fine grain.
    rng = np.random.default_rng(3)
    grain = rng.normal(0, 1, (s, s)) * 2.2
    t = (yy - off) / body
    base = np.stack([247 - 10 * t, 244 - 11 * t, 238 - 13 * t], axis=-1) + grain[..., None]
    rgba = np.zeros((s, s, 4))
    rgba[..., :3] = base
    rgba[..., 3] = cov * 255
    # Ink wave, scaled into the body.
    ink = wave(body, 2.2).astype(np.float64)[..., 3] / 255.0
    ink_full = np.zeros((s, s))
    ink_full[off:off + body, off:off + body] = ink
    for i, v in enumerate((29, 27, 24)):
        rgba[..., i] = rgba[..., i] * (1 - ink_full) + v * ink_full
    # A soft bottom edge shadow inside the sheet for depth.
    edge = np.clip((yy - (off + body * 0.9)) / (body * 0.1), 0, 1) * cov
    rgba[..., :3] *= (1 - 0.06 * edge)[..., None]
    return np.clip(rgba, 0, 255).astype(np.uint8)


def iconset():
    import subprocess
    d = OUT / "AppIcon.iconset"
    d.mkdir(exist_ok=True)
    big = app_icon(1024)
    for n in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = n * scale
            step = 1024 // px if 1024 % px == 0 else None
            img = big.reshape(px, 1024 // px, px, 1024 // px, 4).mean(axis=(1, 3)).astype(np.uint8) if step else big
            name = f"icon_{n}x{n}{'@2x' if scale == 2 else ''}.png"
            png(d / name, img)
            # The colored tray icon on Linux, where panels can be light or dark.
            if px == 64:
                png(OUT / "icon-64.png", img)
    subprocess.run(["iconutil", "-c", "icns", str(d), "-o", str(OUT / "AppIcon.icns")], check=True)
    print("wrote AppIcon.icns")


if __name__ == "__main__" and "--app" in __import__("sys").argv:
    iconset()

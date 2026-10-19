"""Draw the menu bar template icon (the Sayso ink wave) as PNG.

Usage: python3 assets/app/generate_icons.py
"""
import struct
import zlib
from pathlib import Path

import numpy as np

OUT = Path(__file__).parent


def png_bytes(rgba):
    h, w, _ = rgba.shape
    raw = b"".join(b"\x00" + rgba[y].tobytes() for y in range(h))
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    return data


def png(path, rgba):
    path.write_bytes(png_bytes(rgba))


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


# The icns type of each iconset file. The 16 and 32 px images at 1x use the
# ARGB type, and the others hold the PNG as is.
ICNS_TYPES = {
    "icon_16x16.png": b"ic04", "icon_32x32.png": b"ic05",
    "icon_128x128.png": b"ic07", "icon_256x256.png": b"ic08", "icon_512x512.png": b"ic09",
    "icon_16x16@2x.png": b"ic11", "icon_32x32@2x.png": b"ic12", "icon_128x128@2x.png": b"ic13",
    "icon_256x256@2x.png": b"ic14", "icon_512x512@2x.png": b"ic10",
}


def icns_rle(channel):
    """The run-length coding of one channel in an icns ARGB entry."""
    out = bytearray()
    i, n = 0, len(channel)
    while i < n:
        run = 1
        while i + run < n and run < 130 and channel[i + run] == channel[i]:
            run += 1
        if run >= 3:
            out += bytes((run + 125, channel[i]))
            i += run
            continue
        # A literal run ends before the next run of three equal bytes.
        j = i + 1
        while j < n and j - i < 128 and not (j + 2 < n and channel[j] == channel[j + 1] == channel[j + 2]):
            j += 1
        out += bytes((j - i - 1,)) + bytes(channel[i:j])
        i = j
    return bytes(out)


def icns_argb(rgba):
    """An ARGB entry: the magic, then the coded alpha, red, green, and blue planes."""
    planes = (rgba[..., c].reshape(-1).tobytes() for c in (3, 0, 1, 2))
    return b"ARGB" + b"".join(icns_rle(plane) for plane in planes)


def write_icns(path, entries):
    """Pack the entries ((type, data) pairs) in an icns file. The images come from
    the iconset as they are. iconutil codes the 1024 px image again, and the result
    is 40 % larger than the PNG."""
    body = b"".join(kind + struct.pack(">I", 8 + len(data)) + data for kind, data in entries)
    path.write_bytes(b"icns" + struct.pack(">I", 8 + len(body)) + body)


def iconset():
    d = OUT / "AppIcon.iconset"
    d.mkdir(exist_ok=True)
    big = app_icon(1024)
    entries = []
    for n in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = n * scale
            step = 1024 // px if 1024 % px == 0 else None
            img = big.reshape(px, 1024 // px, px, 1024 // px, 4).mean(axis=(1, 3)).astype(np.uint8) if step else big
            name = f"icon_{n}x{n}{'@2x' if scale == 2 else ''}.png"
            data = png_bytes(img)
            (d / name).write_bytes(data)
            kind = ICNS_TYPES[name]
            entries.append((kind, icns_argb(img) if kind in (b"ic04", b"ic05") else data))
            # The colored tray icon on Linux, where panels can be light or dark.
            if px == 64:
                png(OUT / "icon-64.png", img)
    write_icns(OUT / "AppIcon.icns", entries)
    print("wrote AppIcon.icns")


if __name__ == "__main__" and "--app" in __import__("sys").argv:
    iconset()

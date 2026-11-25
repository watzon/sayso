"""Build AppIcon.ico for the Windows build from the 1024 px app icon.

Usage: python3 assets/app/generate_ico.py

Needs only the standard library. The source is AppIcon.iconset/icon_512x512@2x.png,
the macOS icon: a rounded square plate of 824 px on a 1024 px canvas, with 100 px of
empty padding on each side that macOS expects. Windows draws the icon edge to edge, so
the padding made the app look small in the taskbar. This crops the canvas to the plate
and scales it to each size, so the plate fills the frame.

The plate spans 100.5 to 924.5 px (the pixels at 100 and 924 are half covered). The crop uses
those fractional bounds, and every size is an area average of the source in
premultiplied alpha, so the edges stay anti-aliased and nothing is cut off. The sizes
are the ones in internal/winres IconSizes. Each entry is a PNG, which Windows Vista and
later read directly.
"""
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).parent
SOURCE = OUT / "AppIcon.iconset" / "icon_512x512@2x.png"
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
CROP_START = 100.5
CROP_END = 924.5


def read_png(data):
    """Decode an 8-bit RGBA PNG without interlacing into a list of rows of bytes."""
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    pos, idat = 8, b""
    while pos < len(data):
        n, tag = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + n]
        if tag == b"IHDR":
            w, h, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert (depth, color, interlace) == (8, 6, 0), "needs 8-bit RGBA, not interlaced"
        elif tag == b"IDAT":
            idat += body
        pos += 12 + n
    raw = zlib.decompress(idat)
    stride = w * 4
    rows, prev = [], bytearray(stride)
    for y in range(h):
        kind = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        if kind == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 255
        elif kind == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 255
        elif kind == 3:
            for i in range(stride):
                left = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + (left + prev[i]) // 2) & 255
        elif kind == 4:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                b = prev[i]
                c = prev[i - 4] if i >= 4 else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else b if pb <= pc else c
                line[i] = (line[i] + pred) & 255
        rows.append(line)
        prev = line
    return w, h, rows


def write_png(size, rows):
    raw = b"".join(b"\x00" + bytes(r) for r in rows)

    def chunk(tag, body):
        return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def taps(size):
    """For each output pixel, the (source index, weight) pairs of its area average."""
    step = (CROP_END - CROP_START) / size
    out = []
    for i in range(size):
        lo, hi = CROP_START + i * step, CROP_START + (i + 1) * step
        row = []
        for s in range(int(lo), int(hi) + 1):
            w = min(hi, s + 1) - max(lo, s)
            if w > 1e-9:
                row.append((s, w / step))
        out.append(row)
    return out


def scale(w, rows, size):
    """The cropped plate at size x size, area averaged in premultiplied alpha."""
    t = taps(size)
    # Premultiplied floats for the source rows the crop touches.
    first, last = int(CROP_START), int(CROP_END)
    pre = {}
    for y in range(first, last + 1):
        line = rows[y]
        p = [0.0] * (w * 4)
        for x in range(first, last + 1):
            a = line[x * 4 + 3] / 255.0
            p[x * 4] = line[x * 4] * a
            p[x * 4 + 1] = line[x * 4 + 1] * a
            p[x * 4 + 2] = line[x * 4 + 2] * a
            p[x * 4 + 3] = a
        pre[y] = p
    # Rows first, so the second pass has only size rows to cover.
    tall = []
    for taps_y in t:
        acc = [0.0] * (w * 4)
        for sy, wt in taps_y:
            src = pre[sy]
            for i in range(first * 4, (last + 1) * 4):
                acc[i] += src[i] * wt
        tall.append(acc)
    out = []
    for acc in tall:
        line = bytearray()
        for taps_x in t:
            r = g = b = a = 0.0
            for sx, wt in taps_x:
                i = sx * 4
                r += acc[i] * wt
                g += acc[i + 1] * wt
                b += acc[i + 2] * wt
                a += acc[i + 3] * wt
            if a > 1e-9:
                line += bytes((min(255, round(r / a)), min(255, round(g / a)), min(255, round(b / a)), min(255, round(a * 255))))
            else:
                line += b"\x00\x00\x00\x00"
        out.append(line)
    return out


def main():
    w, h, rows = read_png(SOURCE.read_bytes())
    assert (w, h) == (1024, 1024)
    entries = [(size, write_png(size, scale(w, rows, size))) for size in SIZES]
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = len(header) + 16 * len(entries)
    directory = b""
    images = b""
    for size, data in entries:
        # A size of 256 is written as 0.
        dim = 0 if size >= 256 else size
        directory += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset + len(images))
        images += data
    (OUT / "AppIcon.ico").write_bytes(header + directory + images)
    print(f"wrote AppIcon.ico with {len(entries)} sizes")


if __name__ == "__main__":
    main()

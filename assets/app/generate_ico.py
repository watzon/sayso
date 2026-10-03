"""Pack the app icon PNGs into AppIcon.ico for the Windows build.

Usage: python3 assets/app/generate_ico.py

Each entry is a PNG, which Windows Vista and later read directly, so this
needs only the standard library. The sizes come from AppIcon.iconset.
"""
import struct
from pathlib import Path

OUT = Path(__file__).parent
SOURCES = [
    (16, "icon_16x16.png"),
    (24, None),  # No 24 px source: Windows scales the 32 px entry.
    (32, "icon_32x32.png"),
    (48, None),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_256x256.png"),
]


def main():
    entries = [(size, (OUT / "AppIcon.iconset" / name).read_bytes()) for size, name in SOURCES if name]
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

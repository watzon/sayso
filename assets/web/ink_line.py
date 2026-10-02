"""Draw the long ink line of the website as a calligraphic stroke.

The line is one path that runs the length of the page and passes behind each
item. A broad-nib pen has a fixed angle, so its mark is thick when the pen
moves across the nib and thin when it moves along it. This script turns a
centerline into the outline of that mark and writes one SVG for each layout.

The centerlines use the coordinates of the Paper boards "Website — Home v2"
(desktop, 1400 px sheet), "Website — Home v2 · Phone" (374 px sheet), and
"Website — Download v2" (1400 px sheet, and 374 px for the phone).

Usage: python3 assets/web/ink_line.py
"""
import math
import re
from pathlib import Path

OUT = Path(__file__).parent
INK = "#1D1B18"
NIB_ANGLE = math.radians(40)

DESKTOP = (
    "M 330,452 C 520,478 860,426 1070,448 C 1210,462 1330,540 1310,650 C 1300,710 1270,740 1230,760 "
    "L 700,1400 L 700,1421 C 700,1540 356,1520 356,1630 L 356,1930 C 356,2040 1044,2000 1044,2110 "
    "L 1044,2410 C 1044,2520 356,2480 356,2590 L 356,2890 C 356,3000 1044,2960 1044,3070 L 1044,3370 "
    "C 1044,3480 700,3470 700,3580 L 700,3980 C 700,4046 530,4018 432,4056 C 352,4088 336,4190 374,4228 "
    "C 390,4244 410,4243 440,4243 C 526.7,4243 555.6,4219 613.4,4219 C 671.2,4219 671.2,4267 729,4267 "
    "C 786.8,4267 786.8,4207 844.6,4207 C 902.4,4207 902.4,4243 960.2,4243"
)

PHONE = (
    "M 40,240 C 110,252 230,228 326,240 C 356,244 366,300 364,370 C 362,440 350,490 326,506 "
    "L 187,700 L 187,709 C 187,740 126,742 126,766 C 126,790 187,778 187,800 "
    "L 187,1020 C 187,1052 30,1040 30,1092 L 30,1260 C 30,1322 187,1310 187,1360 "
    "L 187,1580 C 187,1612 30,1600 30,1652 L 30,1820 C 30,1882 187,1870 187,1920 "
    "L 187,2140 C 187,2172 30,2160 30,2212 L 30,2380 C 30,2442 187,2430 187,2480 "
    "L 187,2700 C 187,2732 30,2720 30,2772 L 30,2940 C 30,3002 187,2990 187,3040 "
    "L 187,3380 C 187,3412 30,3400 30,3452 L 30,3590 C 30,3680 28,3738 60,3760 "
    "C 102.3,3760 116.4,3748 144.7,3748 C 172.9,3748 172.9,3772 201.1,3772 "
    "C 229.3,3772 229.3,3742 257.6,3742 C 285.8,3742 285.8,3760 314,3760"
)


# The download page: a swash under the headline that points at the receipt.
DOWNLOAD = "M 92,486 C 250,520 500,452 650,480 C 730,496 772,532 806,588"

DOWNLOAD_PHONE = "M 22,236 C 90,250 210,224 296,238 C 322,242 336,250 344,264"


def centerline(d, step):
    """Points along the path, about `step` px apart."""
    tokens = re.findall(r"[MCL]|-?\d+\.?\d*", d)
    nums = lambda i, n: [float(t) for t in tokens[i:i + n]]
    points, pos, i = [], None, 0
    while i < len(tokens):
        cmd = tokens[i]
        if cmd == "M":
            pos = tuple(nums(i + 1, 2)); points.append(pos); i += 3
        elif cmd == "L":
            end = tuple(nums(i + 1, 2))
            n = max(1, int(math.dist(pos, end) / step))
            points += [(pos[0] + (end[0] - pos[0]) * k / n, pos[1] + (end[1] - pos[1]) * k / n) for k in range(1, n + 1)]
            pos = end; i += 3
        else:
            x1, y1, x2, y2, x3, y3 = nums(i + 1, 6)
            length = math.dist(pos, (x1, y1)) + math.dist((x1, y1), (x2, y2)) + math.dist((x2, y2), (x3, y3))
            n = max(4, int(length / step))
            for k in range(1, n + 1):
                t = k / n
                a, b, c, e = (1 - t) ** 3, 3 * (1 - t) ** 2 * t, 3 * (1 - t) * t ** 2, t ** 3
                points.append((a * pos[0] + b * x1 + c * x2 + e * x3, a * pos[1] + b * y1 + c * y2 + e * y3))
            pos = (x3, y3); i += 7
    return points


def outline(points, thin, thick, taper):
    """The outline of the pen mark around the centerline, as SVG path data."""
    n = len(points)
    angles = []
    for i in range(n):
        ax, ay = points[max(i - 1, 0)]
        bx, by = points[min(i + 1, n - 1)]
        angles.append(math.atan2(by - ay, bx - ax))
    widths = [thin + (thick - thin) * abs(math.sin(a - NIB_ANGLE)) for a in angles]
    # A pen does not change width in one step. Average over neighbors.
    for _ in range(3):
        widths = [(widths[max(i - 1, 0)] + widths[i] + widths[min(i + 1, n - 1)]) / 3 for i in range(n)]
    # The pen lands and lifts: thin at both ends.
    run = [0.0]
    for i in range(1, n):
        run.append(run[-1] + math.dist(points[i - 1], points[i]))
    for i in range(n):
        edge = min(run[i], run[-1] - run[i])
        if edge < taper:
            widths[i] *= 0.12 + 0.88 * (edge / taper) ** 0.7
    left, right = [], []
    for (x, y), a, w in zip(points, angles, widths):
        nx, ny = -math.sin(a) * w / 2, math.cos(a) * w / 2
        left.append((x + nx, y + ny))
        right.append((x - nx, y - ny))
    ring = left + right[::-1]
    return "M " + " L ".join(f"{x:.1f},{y:.1f}" for x, y in ring) + " Z"


def write(name, d, width, height, step, thin, thick, taper):
    path = outline(centerline(d, step), thin, thick, taper)
    svg = (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">'
        # The faint stroke is the bleed of ink into rough paper.
        f'<path d="{path}" fill="{INK}" stroke="{INK}" stroke-opacity="0.09" stroke-width="{thick * 0.45:.1f}" stroke-linejoin="round"/>'
        "</svg>\n"
    )
    (OUT / name).write_text(svg)
    print(f"wrote {name}: {len(svg) // 1024} KB")


if __name__ == "__main__":
    write("ink-line-desktop.svg", DESKTOP, 1400, 4440, step=9, thin=1.6, thick=13, taper=90)
    write("ink-line-download.svg", DOWNLOAD, 1400, 860, step=8, thin=1.6, thick=13, taper=90)
    write("ink-line-download-phone.svg", DOWNLOAD_PHONE, 374, 320, step=5, thin=1.2, thick=7.5, taper=40)
    write("ink-line-phone.svg", PHONE, 374, 4100, step=6, thin=1.2, thick=7.5, taper=50)

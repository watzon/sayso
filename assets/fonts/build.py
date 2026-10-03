"""Build the static font files Sayso ships.

GPUI cannot set variable font axes, and fonts loaded with `add_fonts` render
at weight 400 when they are variable (zed #64783). So we cut static instances.

Fraunces gets two optical sizes as two families:
- "Fraunces" (opsz 48) for display text, 22 px and up.
- "Fraunces Text" (opsz 16) for reading text, such as the live preview and rows.

Usage:
    python3 -m venv /tmp/fontvenv && /tmp/fontvenv/bin/pip install fonttools
    /tmp/fontvenv/bin/python assets/fonts/build.py /path/to/google-fonts-sources

The sources are the OFL files from github.com/google/fonts (ofl/fraunces,
ofl/instrumentsans, ofl/ibmplexmono).
"""

import shutil
import sys
from pathlib import Path

from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

OUT = Path(__file__).parent
WEIGHT_NAMES = {400: "Regular", 500: "Medium", 600: "SemiBold", 700: "Bold"}


def rename(font: TTFont, family: str, style: str) -> None:
    """Set name table entries so the instance is its own static face."""
    full = f"{family} {style}".replace(" Regular", "") if style != "Regular" else family
    ps = f"{family.replace(' ', '')}-{style.replace(' ', '')}"
    name = font["name"]
    for rec in list(name.names):
        if rec.nameID in (1, 2, 3, 4, 6, 16, 17, 25):
            name.removeNames(nameID=rec.nameID)
    # Typographic family/subfamily (16/17) carry the real weight names.
    legacy_style = "Italic" if "Italic" in style else "Regular"
    legacy_family = family if style in ("Regular", "Italic") else f"{family} {style.replace(' Italic', '')}"
    for plat, enc, lang in ((3, 1, 0x409), (1, 0, 0)):
        name.setName(legacy_family, 1, plat, enc, lang)
        name.setName(legacy_style, 2, plat, enc, lang)
        name.setName(f"{ps};sayso", 3, plat, enc, lang)
        name.setName(full, 4, plat, enc, lang)
        name.setName(ps, 6, plat, enc, lang)
        name.setName(family, 16, plat, enc, lang)
        name.setName(style, 17, plat, enc, lang)
    if "fvar" in font:
        del font["fvar"]
    # A static face needs no STAT. Fraunces's STAT names a "NonWonky" value
    # that is not elidable, and DirectWrite then calls the family
    # "Fraunces NonWonky", so Windows cannot find "Fraunces".
    if "STAT" in font:
        del font["STAT"]


def cut(src: Path, axes: dict, family: str, style: str, weight: int, italic: bool) -> None:
    font = TTFont(src)
    inst = instantiateVariableFont(font, axes, updateFontNames=False)
    rename(inst, family, style)
    os2 = inst["OS/2"]
    os2.usWeightClass = weight
    os2.fsSelection = (os2.fsSelection & ~0b1100001) | (0b1 if italic else 0) | (0b1000000 if style == "Regular" else 0)
    inst["head"].macStyle = (0b10 if italic else 0) | (0b1 if weight >= 700 else 0)
    out = OUT / f"{family.replace(' ', '')}-{style.replace(' ', '')}.ttf"
    inst.save(out)
    print("wrote", out.name)


def main(src_root: Path) -> None:
    fraunces = src_root / "fraunces" / "Fraunces[SOFT,WONK,opsz,wght].ttf"
    fraunces_i = src_root / "fraunces" / "Fraunces-Italic[SOFT,WONK,opsz,wght].ttf"
    for family, opsz in (("Fraunces", 48), ("Fraunces Text", 16)):
        for w in (400, 500, 600, 700):
            cut(fraunces, {"wght": w, "opsz": opsz, "SOFT": 0, "WONK": 0}, family, WEIGHT_NAMES[w], w, False)
        cut(fraunces_i, {"wght": 400, "opsz": opsz, "SOFT": 0, "WONK": 0}, family, "Italic", 400, True)

    instrument = src_root / "instrumentsans" / "InstrumentSans[wdth,wght].ttf"
    for w in (400, 500, 600, 700):
        cut(instrument, {"wght": w, "wdth": 100}, "Instrument Sans", WEIGHT_NAMES[w], w, False)

    for style in ("Regular", "Medium"):
        shutil.copy(src_root / "ibmplexmono" / f"IBMPlexMono-{style}.ttf", OUT / f"IBMPlexMono-{style}.ttf")

    licenses = OUT / "licenses"
    licenses.mkdir(exist_ok=True)
    for d in ("fraunces", "instrumentsans", "ibmplexmono"):
        shutil.copy(src_root / d / "OFL.txt", licenses / f"{d}-OFL.txt")


if __name__ == "__main__":
    main(Path(sys.argv[1]))

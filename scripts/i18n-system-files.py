#!/usr/bin/env python3
"""Write the translated system files from the `system` catalogs.

The catalogs in crates/sayso-i18n/locales/<locale>/system.<locale>.yml hold
the text that the system shows outside the app. This script writes:

- packaging/macos/<lproj>/InfoPlist.strings: the microphone and system audio prompts of macOS.
- The localized lines of packaging/linux/dev.sayso.Sayso.desktop.
- The localized lines of packaging/linux/dev.sayso.Sayso.metainfo.xml.

Only reviewed locales (draft: false in LOCALES, crates/sayso-i18n/src/lib.rs)
go into these files, so a release never shows a draft translation.

Usage: scripts/i18n-system-files.py [--check]
With --check, it changes nothing and fails if a file is out of date.
"""

import json
import re
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCALES_DIR = ROOT / "crates/sayso-i18n/locales"
LIB = ROOT / "crates/sayso-i18n/src/lib.rs"
MACOS = ROOT / "packaging/macos"
DESKTOP = ROOT / "packaging/linux/dev.sayso.Sayso.desktop"
METAINFO = ROOT / "packaging/linux/dev.sayso.Sayso.metainfo.xml"

# The name of each locale in each system's convention.
LPROJ = {"es": "es", "pt-BR": "pt-BR", "zh-CN": "zh-Hans", "ja": "ja", "de": "de", "fr": "fr", "it": "it", "ko": "ko", "ru": "ru"}
DESKTOP_LANG = {"es": "es", "pt-BR": "pt_BR", "zh-CN": "zh_CN", "ja": "ja", "de": "de", "fr": "fr", "it": "it", "ko": "ko", "ru": "ru"}
GENERATED = "/* Written by scripts/i18n-system-files.py from the system catalog. */\n"


def reviewed_locales():
    """The reviewed locales of LOCALES, without English."""
    text = LIB.read_text()
    entries = re.findall(r'Locale \{ code: "([^"]+)", name: "[^"]*", draft: (true|false) \}', text)
    if not entries:
        sys.exit("could not read LOCALES in " + str(LIB))
    return [code for code, draft in entries if draft == "false" and code != "en"]


def system_texts(locale):
    """The keys and texts under `system:` in a catalog."""
    path = LOCALES_DIR / locale / f"system.{locale}.yml"
    texts = {}
    for line in path.read_text().splitlines():
        m = re.match(r'^  ([a-z_]+): (".*")\s*$', line)
        if m:
            texts[m.group(1)] = json.loads(m.group(2))
    return texts


def strings_escape(text):
    return text.replace("\\", "\\\\").replace('"', '\\"')


def xml_escape(text):
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def info_plist_strings(texts):
    return (
        GENERATED
        + f'"NSMicrophoneUsageDescription" = "{strings_escape(texts["microphone_usage"])}";\n'
        + f'"NSAudioCaptureUsageDescription" = "{strings_escape(texts["system_audio_usage"])}";\n'
    )


def desktop_entry(current, translations):
    keys = {"GenericName": "generic_name", "Comment": "comment", "Keywords": "keywords"}
    out = []
    section = None
    for line in current.splitlines():
        if re.match(r"^(GenericName|Comment|Keywords|Name)\[[^\]]+\]=", line):
            continue
        out.append(line)
        if line.startswith("["):
            section = line
        key = line.split("=", 1)[0]
        if section == "[Desktop Entry]" and key in keys:
            for locale, texts in translations:
                out.append(f"{key}[{DESKTOP_LANG[locale]}]={texts[keys[key]]}")
        elif section == "[Desktop Action Toggle]" and key == "Name":
            for locale, texts in translations:
                out.append(f"Name[{DESKTOP_LANG[locale]}]={texts['toggle_action']}")
    return "\n".join(out) + "\n"


def metainfo(current, translations):
    out = []
    for line in current.splitlines():
        if 'xml:lang="' in line:
            continue
        out.append(line)
        indent = line[: len(line) - len(line.lstrip())]
        if line.strip().startswith("<summary>"):
            for locale, texts in translations:
                out.append(f'{indent}<summary xml:lang="{locale}">{xml_escape(texts["summary"])}</summary>')
        elif line.strip().startswith("<p>"):
            for locale, texts in translations:
                out.append(f'{indent}<p xml:lang="{locale}">{xml_escape(texts["description"])}</p>')
    return "\n".join(out) + "\n"


def main():
    check = "--check" in sys.argv[1:]
    translations = [(locale, system_texts(locale)) for locale in reviewed_locales()]

    wanted = {MACOS / f"{LPROJ[locale]}.lproj" / "InfoPlist.strings": info_plist_strings(texts) for locale, texts in translations}
    wanted[DESKTOP] = desktop_entry(DESKTOP.read_text(), translations)
    wanted[METAINFO] = metainfo(METAINFO.read_text(), translations)
    # Folders that an earlier run wrote for a locale that is no longer reviewed.
    stale = [d for d in MACOS.glob("*.lproj") if d / "InfoPlist.strings" not in wanted]

    outdated = [p for p, text in wanted.items() if not p.exists() or p.read_text() != text] + stale
    if check:
        for p in outdated:
            print(f"out of date: {p.relative_to(ROOT)} (run scripts/i18n-system-files.py)")
        sys.exit(1 if outdated else 0)
    for p, text in wanted.items():
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    for d in stale:
        shutil.rmtree(d)
    print(f"system files: {len(translations)} reviewed locale(s)")


if __name__ == "__main__":
    main()

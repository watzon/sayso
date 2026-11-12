#!/usr/bin/env python3
"""Tests for scripts/check-macho.py. They build small Mach-O files by hand.

    python3 scripts/test_check_macho.py
"""

import os
import struct
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "check-macho.py")

COREAUDIO = "/System/Library/Frameworks/CoreAudio.framework/Versions/A/CoreAudio"
LC_SYMTAB = 0x2
LC_DYSYMTAB = 0xB
LC_LOAD_DYLIB = 0xC
LC_LOAD_WEAK_DYLIB = 0x80000018
LC_VERSION_MIN_MACOSX = 0x24
LC_BUILD_VERSION = 0x32


def version(major, minor):
    return (major << 16) | (minor << 8)


def build_version(sdk, order):
    # platform 1 is macOS; minos 14.0; one tool entry.
    return struct.pack(order + "IIIIIIII", LC_BUILD_VERSION, 32, 1, version(14, 0), sdk, 1, 3, 0)


def version_min(sdk, order):
    return struct.pack(order + "IIII", LC_VERSION_MIN_MACOSX, 16, version(10, 13), sdk)


def symtab(stroff, order):
    return struct.pack(order + "IIIIII", LC_SYMTAB, 24, 0x1000, 10, stroff, 100)


def dysymtab(nindirect, order):
    fields = [0] * 18
    fields[12] = 0x2000  # indirectsymoff
    fields[13] = nindirect
    return struct.pack(order + "II18I", LC_DYSYMTAB, 80, *fields)


def dylib(command, name, order):
    raw = name.encode() + b"\0"
    size = (24 + len(raw) + 7) // 8 * 8
    body = struct.pack(order + "IIIIII", command, size, 24, 0, 0, 0) + raw
    return body + b"\0" * (size - len(body))


def thin(commands, order="<", cputype=0x0100000C):
    """A thin 64-bit Mach-O file with the given load commands."""
    blob = b"".join(commands)
    magic = struct.pack("<I" if order == "<" else ">I", 0xFEEDFACF)
    header = magic + struct.pack(order + "iiIIIII", cputype, 0, 6, len(commands), len(blob), 0, 0)
    return header + blob + b"\0" * 64


def fat(slices, order=">"):
    """A universal file. slices is a list of (cputype, thin bytes)."""
    magic = struct.pack(">I", 0xCAFEBABE) if order == ">" else struct.pack("<I", 0xCAFEBABE)
    head = magic + struct.pack(order + "I", len(slices))
    offset = 0x4000
    table, body = b"", b""
    for cputype, data in slices:
        table += struct.pack(order + "iiIII", cputype, 0, offset + len(body), len(data), 14)
        body += data + b"\0" * (-len(data) % 0x4000)
    out = head + table
    out += b"\0" * (offset - len(out))
    return out + body


def run_script(*args):
    return subprocess.run([sys.executable, "-I", SCRIPT, *args], capture_output=True, text=True)


class CheckMachO(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def write(self, name, data):
        path = os.path.join(self.tmp.name, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(data)
        return path

    def macho(self, sdk, stroff, nindirect, order="<", extra=(), cputype=0x0100000C):
        return thin(
            [build_version(sdk, order), symtab(stroff, order), dysymtab(nindirect, order), *extra],
            order,
            cputype,
        )

    # String table alignment.

    def test_old_sdk_with_odd_string_table_passes(self):
        path = self.write("old", self.macho(version(26, 5), 0x1004, 3))
        result = run_script(path)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("OK", result.stdout)
        self.assertIn("sdk 26.5", result.stdout)

    def test_new_sdk_with_odd_string_table_fails(self):
        path = self.write("odd", self.macho(version(27, 2), 0x1004, 485))
        result = run_script(path)
        self.assertEqual(result.returncode, 1)
        for text in (path, "sdk 27.2", "stroff 0x1004", "nindirectsyms 485", "mis-aligned LINKEDIT string pool"):
            self.assertIn(text, result.stderr)

    def test_new_sdk_with_aligned_string_table_passes(self):
        path = self.write("even", self.macho(version(27, 2), 0x1008, 484))
        result = run_script(path)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_sdk_27_0_is_the_limit(self):
        self.assertEqual(run_script(self.write("a", self.macho(version(27, 0), 0x1004, 1))).returncode, 1)
        self.assertEqual(run_script(self.write("b", self.macho(version(26, 99), 0x1004, 1))).returncode, 0)

    def test_big_endian_thin_file(self):
        odd = self.write("be-odd", self.macho(version(27, 0), 0x1004, 1, order=">"))
        even = self.write("be-even", self.macho(version(27, 0), 0x1000, 0, order=">"))
        self.assertEqual(run_script(odd).returncode, 1)
        self.assertEqual(run_script(even).returncode, 0)

    def test_version_min_command_gives_the_sdk(self):
        blob = thin([version_min(version(27, 1), "<"), symtab(0x1004, "<"), dysymtab(1, "<")])
        self.assertEqual(run_script(self.write("vm-odd", blob)).returncode, 1)
        blob = thin([version_min(version(12, 0), "<"), symtab(0x1004, "<"), dysymtab(1, "<")])
        self.assertEqual(run_script(self.write("vm-old", blob)).returncode, 0)

    def test_file_without_sdk_passes(self):
        blob = thin([symtab(0x1004, "<"), dysymtab(1, "<")])
        self.assertEqual(run_script(self.write("nosdk", blob)).returncode, 0)

    # Universal files.

    def test_fat_file_with_one_bad_slice_fails(self):
        good = self.macho(version(27, 2), 0x1008, 2)
        bad = self.macho(version(27, 2), 0x1004, 3, cputype=0x01000007)
        for order in (">", "<"):
            with self.subTest(fat_order=order):
                path = self.write(f"fat-bad-{order == '>'}", fat([(0x0100000C, good), (0x01000007, bad)], order))
                result = run_script(path)
                self.assertEqual(result.returncode, 1)
                self.assertIn("(x86_64)", result.stderr)
                self.assertIn("OK", result.stdout)
                self.assertIn("(arm64)", result.stdout)

    def test_fat_file_with_good_slices_passes(self):
        arm = self.macho(version(27, 2), 0x1008, 2)
        intel = self.macho(version(27, 2), 0x1008, 2, cputype=0x01000007)
        for order in (">", "<"):
            with self.subTest(fat_order=order):
                path = self.write(f"fat-good-{order == '>'}", fat([(0x0100000C, arm), (0x01000007, intel)], order))
                result = run_script(path)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(result.stdout.strip().splitlines()), 2)

    def test_java_class_file_is_not_a_fat_file(self):
        # Same first 4 bytes as a fat file; the next word is a Java version.
        path = self.write("Thing.class", bytes.fromhex("cafebabe00000034") + b"\0" * 200)
        result = run_script(path)
        self.assertEqual(result.returncode, 1)
        self.assertIn("not a Mach-O file", result.stderr)

    # CoreAudio.

    def test_strong_coreaudio_fails_with_the_flag(self):
        blob = self.macho(version(26, 5), 0x1000, 0, extra=[dylib(LC_LOAD_DYLIB, COREAUDIO, "<")])
        path = self.write("strong", blob)
        self.assertEqual(run_script(path).returncode, 0)
        result = run_script("--weak-coreaudio", path)
        self.assertEqual(result.returncode, 1)
        self.assertIn("LC_LOAD_DYLIB", result.stderr)
        self.assertIn("weak", result.stderr)

    def test_weak_coreaudio_passes_with_the_flag(self):
        blob = self.macho(version(26, 5), 0x1000, 0, extra=[dylib(LC_LOAD_WEAK_DYLIB, COREAUDIO, "<")])
        result = run_script("--weak-coreaudio", self.write("weak", blob))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_other_strong_libraries_pass_with_the_flag(self):
        blob = self.macho(
            version(26, 5), 0x1000, 0, extra=[dylib(LC_LOAD_DYLIB, "/usr/lib/libSystem.B.dylib", "<")]
        )
        result = run_script("--weak-coreaudio", self.write("system", blob))
        self.assertEqual(result.returncode, 0, result.stderr)

    # Files and folders.

    def test_folder_is_walked_and_other_files_are_skipped(self):
        self.write("app/Sayso.app/Contents/MacOS/Sayso", self.macho(version(27, 2), 0x1008, 2))
        self.write("app/Sayso.app/Contents/Info.plist", b"<plist/>")
        self.write("app/readme.txt", b"hello")
        self.write("app/empty", b"")
        result = run_script(os.path.join(self.tmp.name, "app"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(result.stdout.strip().splitlines()), 1)
        self.assertEqual(result.stderr, "")

    def test_folder_finds_a_bad_file_by_magic_not_name(self):
        self.write("out/libodd.txt", self.macho(version(27, 2), 0x1004, 1))
        self.write("out/good", self.macho(version(27, 2), 0x1008, 2))
        result = run_script(os.path.join(self.tmp.name, "out"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("libodd.txt", result.stderr)

    def test_folder_does_not_follow_links(self):
        self.write("real/prog", self.macho(version(27, 2), 0x1008, 2))
        os.symlink(os.path.join(self.tmp.name, "real"), os.path.join(self.tmp.name, "real", "loop"))
        result = run_script(os.path.join(self.tmp.name, "real"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(result.stdout.strip().splitlines()), 1)

    def test_text_file_named_directly_is_an_error(self):
        result = run_script(self.write("notes.txt", b"just text, long enough to read"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("not a Mach-O file", result.stderr)

    def test_missing_path_is_an_error(self):
        result = run_script(os.path.join(self.tmp.name, "nope"))
        self.assertEqual(result.returncode, 1)

    def test_folder_without_any_mach_o_file_is_an_error(self):
        self.write("none/a.txt", b"text")
        self.assertEqual(run_script(os.path.join(self.tmp.name, "none")).returncode, 1)

    def test_cut_off_file_is_an_error(self):
        blob = self.macho(version(27, 2), 0x1008, 2)
        result = run_script(self.write("cut", blob[:40]))
        self.assertEqual(result.returncode, 1)
        self.assertIn("broken Mach-O file", result.stderr)

    def test_no_arguments_is_a_usage_error(self):
        self.assertEqual(run_script().returncode, 2)


if __name__ == "__main__":
    unittest.main()

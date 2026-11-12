#!/usr/bin/env python3
"""Check Mach-O files before they ship.

    scripts/check-macho.py [--weak-coreaudio] FILE_OR_DIR...

A directory is walked, and every Mach-O file inside is checked. The script
finds a Mach-O file by its magic number, not by its name, so it also finds the
programs inside an .app bundle. A file that is not Mach-O is skipped in a
directory. A file that you name and that is not Mach-O is an error.

The script reads the load commands itself. It needs only Python 3 and does not
call otool. It reads thin and universal (fat) files, both byte orders.

Checks:

  string table  A file with SDK 27.0 or newer (LC_BUILD_VERSION or
                LC_VERSION_MIN_MACOSX) fails when LC_SYMTAB stroff is not a
                multiple of 8. The linker of Xcode 27 does not pad
                stroff = indirectsymoff + 4 * nindirectsyms. With an odd count
                of indirect symbols, stroff % 8 is 4, and dyld of macOS 27
                refuses to load the file ("mis-aligned LINKEDIT string pool").

  --weak-coreaudio
                A file fails when it links CoreAudio with LC_LOAD_DYLIB. The
                node addon must link it weak (LC_LOAD_WEAK_DYLIB), so that it
                loads on macOS 14.0 and 14.1, which lack the process tap API.

The script prints one line for each file (each slice, for a universal file) and
exits 0 when all pass. It exits 1 when one fails, and also when it finds no
Mach-O file at all, because that means a wrong path.
"""

import os
import struct
import sys

# Magic numbers, as the first 4 bytes read in big-endian order.
MH_MAGIC = 0xFEEDFACE
MH_CIGAM = 0xCEFAEDFE
MH_MAGIC_64 = 0xFEEDFACF
MH_CIGAM_64 = 0xCFFAEDFE
FAT_MAGIC = 0xCAFEBABE
FAT_CIGAM = 0xBEBAFECA
FAT_MAGIC_64 = 0xCAFEBABF
FAT_CIGAM_64 = 0xBFBAFECA

LC_REQ_DYLD = 0x80000000
LC_SYMTAB = 0x2
LC_LOAD_DYLIB = 0xC
LC_VERSION_MIN_MACOSX = 0x24
LC_LOAD_WEAK_DYLIB = 0x18 | LC_REQ_DYLD
LC_BUILD_VERSION = 0x32

COREAUDIO_PREFIX = "/System/Library/Frameworks/CoreAudio.framework/"
SDK_LIMIT = (27, 0)

CPU_NAMES = {
    7: "i386",
    0x01000007: "x86_64",
    12: "arm",
    0x0100000C: "arm64",
    0x0200000C: "arm64_32",
}

# A limit, so that a broken header cannot make the script read a huge block.
MAX_LOAD_COMMANDS_BYTES = 64 * 1024 * 1024


class MachOError(Exception):
    """The file looks like Mach-O but its structure is wrong."""


def version_text(value):
    """Format a Mach-O version number (xxxx.yy.zz in a 32-bit word)."""
    major, minor, patch = value >> 16, (value >> 8) & 0xFF, value & 0xFF
    return f"{major}.{minor}" + (f".{patch}" if patch else "")


def read_exact(f, offset, size, what):
    f.seek(offset)
    data = f.read(size)
    if len(data) != size:
        raise MachOError(f"the file ends inside {what}")
    return data


def thin_header(magic_bytes):
    """Return (is_64, byte_order) for the first 4 bytes of a thin file, or None."""
    magic = struct.unpack(">I", magic_bytes)[0]
    if magic == MH_MAGIC_64:
        return True, ">"
    if magic == MH_CIGAM_64:
        return True, "<"
    if magic == MH_MAGIC:
        return False, ">"
    if magic == MH_CIGAM:
        return False, "<"
    return None


def fat_slices(f, size):
    """Return a list of (offset, size) for a universal file, or None if the
    file is not a universal Mach-O file."""
    f.seek(0)
    head = f.read(8)
    if len(head) < 8:
        return None
    magic = struct.unpack(">I", head[:4])[0]
    if magic in (FAT_MAGIC, FAT_MAGIC_64):
        order = ">"
    elif magic in (FAT_CIGAM, FAT_CIGAM_64):
        order = "<"
    else:
        return None
    is_64 = magic in (FAT_MAGIC_64, FAT_CIGAM_64)
    (count,) = struct.unpack(order + "I", head[4:8])
    # A Java class file also starts with 0xCAFEBABE, and its next word is a
    # version number. A real fat file has a few slices.
    if count == 0 or count > 64:
        return None
    entry = (order + "iiQQII") if is_64 else (order + "iiIII")
    entry_size = struct.calcsize(entry)
    table = f.read(entry_size * count)
    if len(table) != entry_size * count:
        return None
    slices = []
    for i in range(count):
        fields = struct.unpack_from(entry, table, i * entry_size)
        offset, length = fields[2], fields[3]
        if offset + length > size or length < 28:
            return None
        f.seek(offset)
        if thin_header(f.read(4)) is None:
            # The slice is not Mach-O, so this is not a universal Mach-O file.
            f.seek(0)
            return None
        slices.append((offset, length))
    f.seek(0)
    return slices


def parse_thin(f, base, length):
    """Read the load commands of one thin Mach-O file at offset base.

    Return a dict with: cputype, sdk (major, minor) or None, sdk_text,
    symtab (stroff, nindirectsyms) or None, dylibs [(command, name)]."""
    kind = thin_header(read_exact(f, base, 4, "the header"))
    is_64, order = kind
    header_size = 32 if is_64 else 28
    header = read_exact(f, base, header_size, "the header")
    cputype = struct.unpack_from(order + "i", header, 4)[0] & 0xFFFFFFFF
    cpusubtype = struct.unpack_from(order + "i", header, 8)[0] & 0xFFFFFF
    ncmds, sizeofcmds = struct.unpack_from(order + "II", header, 16)
    if sizeofcmds > MAX_LOAD_COMMANDS_BYTES or header_size + sizeofcmds > length:
        raise MachOError("the load commands are larger than the file")
    commands = read_exact(f, base + header_size, sizeofcmds, "the load commands")

    info = {
        "cputype": cputype,
        "cpusubtype": cpusubtype,
        "sdk": None,
        "sdk_text": None,
        "symtab": None,
        "dylibs": [],
    }
    dysymtab_seen = False
    position = 0
    for _ in range(ncmds):
        if position + 8 > len(commands):
            raise MachOError("a load command is cut off")
        cmd, cmdsize = struct.unpack_from(order + "II", commands, position)
        if cmdsize < 8 or position + cmdsize > len(commands):
            raise MachOError(f"a load command has the wrong size ({cmdsize})")
        body = commands[position : position + cmdsize]
        if cmd == LC_SYMTAB:
            if cmdsize < 24:
                raise MachOError("LC_SYMTAB is too small")
            _symoff, _nsyms, stroff, _strsize = struct.unpack_from(order + "IIII", body, 8)
            info["symtab"] = {"stroff": stroff}
        elif cmd == 0xB:  # LC_DYSYMTAB
            if cmdsize < 80:
                raise MachOError("LC_DYSYMTAB is too small")
            indirectsymoff, nindirectsyms = struct.unpack_from(order + "II", body, 8 + 12 * 4)
            info["dysymtab"] = {"indirectsymoff": indirectsymoff, "nindirectsyms": nindirectsyms}
            dysymtab_seen = True
        elif cmd == LC_BUILD_VERSION:
            if cmdsize < 24:
                raise MachOError("LC_BUILD_VERSION is too small")
            _platform, _minos, sdk = struct.unpack_from(order + "III", body, 8)
            info["sdk"] = (sdk >> 16, (sdk >> 8) & 0xFF)
            info["sdk_text"] = version_text(sdk)
        elif cmd == LC_VERSION_MIN_MACOSX:
            if cmdsize < 16:
                raise MachOError("LC_VERSION_MIN_MACOSX is too small")
            _version, sdk = struct.unpack_from(order + "II", body, 8)
            # LC_BUILD_VERSION wins when both are present.
            if info["sdk"] is None:
                info["sdk"] = (sdk >> 16, (sdk >> 8) & 0xFF)
                info["sdk_text"] = version_text(sdk)
        elif cmd in (LC_LOAD_DYLIB, LC_LOAD_WEAK_DYLIB):
            if cmdsize < 24:
                raise MachOError("a dylib command is too small")
            (name_offset,) = struct.unpack_from(order + "I", body, 8)
            if name_offset >= cmdsize:
                raise MachOError("a dylib name is outside its command")
            name = body[name_offset:].split(b"\0", 1)[0].decode("utf-8", "replace")
            info["dylibs"].append((cmd, name))
        position += cmdsize
    if not dysymtab_seen:
        info["dysymtab"] = None
    return info


def check_slice(info, weak_coreaudio):
    """Return a list of failure messages for one slice (empty when it passes)."""
    problems = []
    sdk = info["sdk"]
    symtab = info["symtab"]
    if sdk is not None and sdk >= SDK_LIMIT and symtab is not None:
        stroff = symtab["stroff"]
        if stroff % 8 != 0:
            dysymtab = info["dysymtab"]
            count = dysymtab["nindirectsyms"] if dysymtab else "unknown"
            problems.append(
                f"sdk {info['sdk_text']}, stroff {stroff:#x} (stroff % 8 = {stroff % 8}), "
                f"nindirectsyms {count}. The string table is not aligned to 8 bytes. "
                "The linker of Xcode 27 does not pad it when the count of indirect symbols is odd. "
                "dyld of macOS 27 refuses such a file with SDK 27 or newer "
                '("mis-aligned LINKEDIT string pool"). Link it again with another linker version '
                "(Xcode 26), or set the SDK version below 27."
            )
    if weak_coreaudio:
        for cmd, name in info["dylibs"]:
            if cmd == LC_LOAD_DYLIB and name.startswith(COREAUDIO_PREFIX):
                problems.append(
                    f"CoreAudio is linked with LC_LOAD_DYLIB ({name}). The addon would not load "
                    "on macOS 14.0 and 14.1. Link it weak: -Wl,-weak_framework,CoreAudio."
                )
    return problems


def describe(info):
    sdk = f"sdk {info['sdk_text']}" if info["sdk_text"] else "no sdk"
    if info["symtab"] is None:
        return f"{sdk}, no symbol table"
    return f"{sdk}, stroff {info['symtab']['stroff']:#x}"


def check_file(path, weak_coreaudio=False):
    """Check one file.

    Return None when the file is not Mach-O. Otherwise return a list of
    (label, problems, description), one entry for each slice. A slice with an
    empty problems list passes. Raise MachOError for a broken file."""
    size = os.path.getsize(path)
    with open(path, "rb") as f:
        head = f.read(4)
        if len(head) < 4:
            return None
        f.seek(0)
        slices = fat_slices(f, size)
        if slices is not None:
            results = []
            for offset, length in slices:
                info = parse_thin(f, offset, length)
                cpu = CPU_NAMES.get(info["cputype"], f"cpu {info['cputype']:#x}")
                if info["cputype"] == 0x0100000C and info["cpusubtype"] == 2:
                    cpu = "arm64e"
                results.append((cpu, check_slice(info, weak_coreaudio), describe(info)))
            return results
        if thin_header(head) is None:
            return None
        info = parse_thin(f, 0, size)
        return [(None, check_slice(info, weak_coreaudio), describe(info))]


def files_in(path):
    """Yield every regular file below a directory, not following links."""
    for folder, dirs, names in os.walk(path):
        dirs.sort()
        for name in sorted(names):
            full = os.path.join(folder, name)
            if os.path.islink(full) or not os.path.isfile(full):
                continue
            yield full


def run(paths, weak_coreaudio, out=sys.stdout, err=sys.stderr):
    """Check the paths. Return the exit code."""
    failed = 0
    checked = 0

    def report(path, explicit):
        nonlocal failed, checked
        try:
            results = check_file(path, weak_coreaudio)
        except MachOError as error:
            print(f"FAIL {path}: broken Mach-O file: {error}", file=err)
            failed += 1
            return
        except OSError as error:
            if explicit:
                print(f"FAIL {path}: cannot read the file: {error}", file=err)
                failed += 1
            return
        if results is None:
            if explicit:
                print(f"FAIL {path}: this is not a Mach-O file", file=err)
                failed += 1
            return
        for label, problems, description in results:
            name = f"{path} ({label})" if label else path
            checked += 1
            if problems:
                failed += 1
                for problem in problems:
                    print(f"FAIL {name}: {problem}", file=err)
            else:
                print(f"OK   {name}: {description}", file=out)

    for path in paths:
        if os.path.isdir(path):
            for found in files_in(path):
                report(found, explicit=False)
        elif os.path.exists(path):
            report(path, explicit=True)
        else:
            print(f"FAIL {path}: no such file or folder", file=err)
            failed += 1
    if not failed and checked == 0:
        print("FAIL: no Mach-O file found in the given paths", file=err)
        return 1
    return 1 if failed else 0


def main(argv):
    weak = False
    paths = []
    for arg in argv:
        if arg == "--weak-coreaudio":
            weak = True
        elif arg in ("-h", "--help"):
            print(__doc__)
            return 0
        elif arg.startswith("--"):
            print(f"unknown option {arg}\nusage: check-macho.py [--weak-coreaudio] FILE_OR_DIR...", file=sys.stderr)
            return 2
        else:
            paths.append(arg)
    if not paths:
        print("usage: check-macho.py [--weak-coreaudio] FILE_OR_DIR...", file=sys.stderr)
        return 2
    return run(paths, weak)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

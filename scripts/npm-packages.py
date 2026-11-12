#!/usr/bin/env python3
"""Make the npm packages of Sayso from the files of a release.

    scripts/npm-packages.py engine  --version 0.15.0 --release-dir DIR --out OUT
                                    [--allow-missing] [--no-npm-check]
    scripts/npm-packages.py cleanup --version 0.15.0 --release-dir DIR --out OUT
                                    [--allow-missing] [--no-npm-check]
    scripts/npm-packages.py check-versions --version 0.15.0

The engine command reads the engine release files in DIR:

    sayso-engine-<version>-macos-arm64.tar.gz
    sayso-engine-<version>-macos-x86_64.tar.gz
    sayso-engine-<version>-linux-x86_64.tar.gz
    sayso-engine-<version>-linux-arm64.tar.gz
    sayso-engine-<version>-windows-x64.zip

(scripts/package-engine.sh and scripts/package-engine.ps1 make them.) It writes
one npm package folder for each platform into OUT, and the main package:

    OUT/engine-darwin-arm64     @justsayso/engine-darwin-arm64
    OUT/engine-darwin-x64       @justsayso/engine-darwin-x64
    OUT/engine-linux-x64-gnu    @justsayso/engine-linux-x64-gnu
    OUT/engine-linux-arm64-gnu  @justsayso/engine-linux-arm64-gnu
    OUT/engine-win32-x64-msvc   @justsayso/engine-win32-x64-msvc
    OUT/engine                  @justsayso/engine (a copy of packages/engine)

The files of a platform package are the files of the release file, so the npm
package has the program that was signed in the release. The README.md of the
release file moves to ENGINE.md, because the package gets its own README.md.

The cleanup command does the same for the cleanup runtime. It reads

    sayso-cleanup-<version>-<macos-arm64|macos-x86_64|linux-x86_64|linux-arm64>.tar.gz
    sayso-cleanup-<version>-windows-x64.zip

(scripts/package-cleanup.sh and scripts/package-cleanup.ps1 make them) and
writes OUT/cleanup-<platform> (@justsayso/cleanup-<platform>) and OUT/cleanup
(@justsayso/cleanup, a copy of packages/cleanup). The README.md of the release
file moves to RUNTIME.md. A platform package has all the builds of the release
file (on x64 for Linux and Windows, three CPU levels).

The check-versions command stops with exit code 1, and names each wrong file,
unless the version of the Cargo workspace and of every package in packages/ is
the given version, and so are the @justsayso/* dependencies between them.

A platform without a release file is an error, unless you give --allow-missing.
The script never publishes. When npm is installed, it runs `npm pack --dry-run`
for each package, to check what a publication would send.
"""

import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

SCOPE = "@justsayso"
LICENSE = "GPL-3.0-only"
REPOSITORY_URL = "git+https://github.com/watzon/sayso.git"
HOMEPAGE = "https://github.com/watzon/sayso"

# name of the npm package folder, os, cpu, libc, release platform, extension,
# and a name for people.
PLATFORMS = [
    {"name": "darwin-arm64", "os": "darwin", "cpu": "arm64", "libc": None, "release": "macos-arm64", "ext": "tar.gz", "title": "macOS (Apple silicon)"},
    {"name": "darwin-x64", "os": "darwin", "cpu": "x64", "libc": None, "release": "macos-x86_64", "ext": "tar.gz", "title": "macOS (Intel)"},
    {"name": "linux-x64-gnu", "os": "linux", "cpu": "x64", "libc": "glibc", "release": "linux-x86_64", "ext": "tar.gz", "title": "Linux x64 (glibc)"},
    {"name": "linux-arm64-gnu", "os": "linux", "cpu": "arm64", "libc": "glibc", "release": "linux-arm64", "ext": "tar.gz", "title": "Linux arm64 (glibc)"},
    {"name": "win32-x64-msvc", "os": "win32", "cpu": "x64", "libc": None, "release": "windows-x64", "ext": "zip", "title": "Windows x64"},
]

# The two kinds of package that a release file makes. "program" is the file
# that must be in every platform package (the others are the builds of the
# CPU levels, which only x64 for Linux and Windows has).
KINDS = {
    "engine": {
        "title": "engine",
        "description": "The speech engine of Sayso for {platform}.",
        "program": "sayso-engine",
        "builds": [],
        "doc": "ENGINE.md",
    },
    "cleanup": {
        "title": "cleanup runtime",
        "description": "The runtime of the local cleanup models of Sayso for {platform}.",
        "program": "sayso-cleanup",
        "builds": ["-avx", "-baseline"],
        "doc": "RUNTIME.md",
    },
}

SEMVER = re.compile(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$")


class Error(Exception):
    """An error with a message for the person who runs the script."""


def release_file_name(kind, version, platform):
    return f"sayso-{kind}-{version}-{platform['release']}.{platform['ext']}"


def program_names(kind, platform):
    """The programs that a platform package must have, with .exe on Windows."""
    info = KINDS[kind]
    suffixes = [""]
    # Only x64 for Linux and Windows has the builds of the CPU levels
    # (scripts/build-cleanup.sh, scripts/bundle-windows.ps1).
    if platform["cpu"] == "x64" and platform["os"] in ("linux", "win32"):
        suffixes += info["builds"]
    exe = ".exe" if platform["os"] == "win32" else ""
    return [f"{info['program']}{suffix}{exe}" for suffix in suffixes]


def check_sha256(path):
    """Compare the file with its .sha256 file, when that file exists."""
    sidecar = path + ".sha256"
    if not os.path.exists(sidecar):
        return False
    with open(sidecar, encoding="utf-8") as f:
        expected = f.read().split()[0].lower()
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    if digest.hexdigest() != expected:
        raise Error(f"the checksum of {path} does not match {sidecar}")
    return True


def safe_relative(name, top):
    """Return the path of an archive entry without the top folder.

    Return None for the top folder itself. Raise Error for an entry that is not
    below the top folder, or that leaves it."""
    parts = [p for p in name.replace("\\", "/").split("/") if p not in ("", ".")]
    if not parts or parts[0] != top:
        raise Error(f"the archive entry {name!r} is not in the folder {top}/")
    rest = parts[1:]
    if any(p == ".." for p in rest) or os.path.isabs(name):
        raise Error(f"the archive entry {name!r} leaves its folder")
    return os.path.join(*rest) if rest else None


def file_mode(mode):
    """Keep the exec bit, and nothing else. npm keeps only this bit."""
    return 0o755 if mode & 0o111 else 0o644


def extract_tar(path, top, dest):
    with tarfile.open(path, "r:gz") as archive:
        for member in archive:
            rel = safe_relative(member.name, top)
            if rel is None:
                continue
            target = os.path.join(dest, rel)
            if member.isdir():
                os.makedirs(target, exist_ok=True)
            elif member.isfile():
                os.makedirs(os.path.dirname(target), exist_ok=True)
                source = archive.extractfile(member)
                with open(target, "wb") as out:
                    shutil.copyfileobj(source, out)
                os.chmod(target, file_mode(member.mode))
            else:
                raise Error(f"the archive entry {member.name!r} is not a file or a folder")


def extract_zip(path, top, dest):
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            rel = safe_relative(info.filename, top)
            if rel is None:
                continue
            target = os.path.join(dest, rel)
            if info.is_dir():
                os.makedirs(target, exist_ok=True)
                continue
            unix_mode = (info.external_attr >> 16) & 0xFFFF
            if stat.S_ISLNK(unix_mode):
                raise Error(f"the archive entry {info.filename!r} is a link")
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with archive.open(info) as source, open(target, "wb") as out:
                shutil.copyfileobj(source, out)
            # A zip from Windows has no Unix mode. The program is a .exe, which
            # needs no exec bit, so it gets the same mode as other files.
            os.chmod(target, file_mode(unix_mode) if unix_mode else 0o644)


def readme_text(kind, package, version, platform):
    return (
        f"# {package}\n"
        f"The {KINDS[kind]['title']} of Sayso {version} for {platform['title']}. `{SCOPE}/{kind}` installs it for you, so do not install it by itself.\n"
        f"It is free software under {LICENSE}. The source is at {HOMEPAGE}.\n"
    )


def package_json(kind, package, version, platform, entries):
    data = {
        "name": package,
        "version": version,
        "description": KINDS[kind]["description"].format(platform=platform["title"]),
        "license": LICENSE,
        "repository": {"type": "git", "url": REPOSITORY_URL, "directory": f"packages/{kind}"},
        "homepage": HOMEPAGE,
        "os": [platform["os"]],
        "cpu": [platform["cpu"]],
    }
    if platform["libc"]:
        data["libc"] = [platform["libc"]]
    data["files"] = entries
    data["publishConfig"] = {"access": "public"}
    return data


def make_platform_package(kind, version, platform, archive, out):
    package = f"{SCOPE}/{kind}-{platform['name']}"
    dest = os.path.join(out, f"{kind}-{platform['name']}")
    if os.path.lexists(dest):
        shutil.rmtree(dest)
    os.makedirs(dest)
    top = f"sayso-{kind}-{version}"
    if archive.endswith(".zip"):
        extract_zip(archive, top, dest)
    else:
        extract_tar(archive, top, dest)
    if not os.listdir(dest):
        raise Error(f"{archive} has no files in {top}/")

    # The release README is the program's own: how to run it, and what it needs.
    # The package gets a short README, and the release README keeps its text.
    release_readme = os.path.join(dest, "README.md")
    if os.path.exists(release_readme):
        os.replace(release_readme, os.path.join(dest, KINDS[kind]["doc"]))
    with open(release_readme, "w", encoding="utf-8") as f:
        f.write(readme_text(kind, package, version, platform))
    os.chmod(release_readme, 0o644)

    entries = sorted(name + ("/" if os.path.isdir(os.path.join(dest, name)) else "") for name in os.listdir(dest))
    # npm adds package.json, README.md, and LICENSE to every package itself.
    with open(os.path.join(dest, "package.json"), "w", encoding="utf-8") as f:
        json.dump(package_json(kind, package, version, platform, entries), f, indent=2)
        f.write("\n")
    return dest


def make_main_package(kind, version, out):
    source = os.path.join(ROOT, "packages", kind)
    dest = os.path.join(out, kind)
    if not os.path.isdir(source):
        raise Error(f"{source} does not exist")
    if os.path.lexists(dest):
        shutil.rmtree(dest)
    shutil.copytree(source, dest, ignore=shutil.ignore_patterns("node_modules"))
    path = os.path.join(dest, "package.json")
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    data["version"] = version
    data["optionalDependencies"] = {f"{SCOPE}/{kind}-{p['name']}": version for p in PLATFORMS}
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    return dest


def npm_pack_check(folder, expect):
    """Run `npm pack --dry-run` for a folder. Return the list of file paths."""
    npm = shutil.which("npm")
    if npm is None:
        return None
    result = subprocess.run(
        [npm, "pack", "--dry-run", "--json", "--ignore-scripts", folder],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise Error(f"npm pack --dry-run failed for {folder}:\n{result.stderr.strip()}")
    files = [entry["path"] for entry in json.loads(result.stdout)[0]["files"]]
    for name in expect:
        if name not in files:
            raise Error(f"npm pack would not send {name} from {folder}")
    return files


def run_kind(kind, args):
    version = args.version
    if not SEMVER.match(version):
        raise Error(f"--version {version!r} is not a version like 0.15.0")
    release_dir = os.path.abspath(args.release_dir)
    out = os.path.abspath(args.out)
    if not os.path.isdir(release_dir):
        raise Error(f"--release-dir {release_dir} is not a folder")

    found, missing = [], []
    for platform in PLATFORMS:
        path = os.path.join(release_dir, release_file_name(kind, version, platform))
        (found if os.path.isfile(path) else missing).append((platform, path))
    if missing and not args.allow_missing:
        names = "\n".join(f"  {path}" for _, path in missing)
        raise Error(f"these release files are missing (use --allow-missing to skip them):\n{names}")
    if not found:
        raise Error(f"no {kind} release file of version {version} in {release_dir}")

    os.makedirs(out, exist_ok=True)
    for platform, path in found:
        verified = check_sha256(path)
        dest = make_platform_package(kind, version, platform, path, out)
        programs = program_names(kind, platform)
        for program in programs:
            if not os.path.exists(os.path.join(dest, program)):
                raise Error(f"{path} has no {program}")
        print(f"==> {SCOPE}/{kind}-{platform['name']}: {dest}" + (" (checksum ok)" if verified else ""))
        if not args.no_npm_check:
            files = npm_pack_check(dest, [*programs, "package.json", "README.md", "LICENSE"])
            if files is not None:
                print(f"    npm pack --dry-run: {len(files)} files")
    for platform, path in missing:
        print(f"==> skipped {SCOPE}/{kind}-{platform['name']}: no {os.path.basename(path)}")

    main = make_main_package(kind, version, out)
    print(f"==> {SCOPE}/{kind}: {main}")
    if not args.no_npm_check:
        files = npm_pack_check(main, ["package.json", "index.js", "index.d.ts"])
        if files is not None:
            print(f"    npm pack --dry-run: {len(files)} files")


def workspace_version():
    """The version in [workspace.package] of the root Cargo.toml."""
    path = os.path.join(ROOT, "Cargo.toml")
    with open(path, encoding="utf-8") as f:
        text = f.read()
    section = re.search(r"^\[workspace\.package\]\s*\n(.*?)(?=^\[|\Z)", text, re.M | re.S)
    match = section and re.search(r'^version\s*=\s*"([^"]+)"', section.group(1), re.M)
    if not match:
        raise Error(f"{path} has no version in [workspace.package]")
    return match.group(1)


DEPENDENCY_FIELDS = ("dependencies", "optionalDependencies", "peerDependencies")


def run_check_versions(args):
    version = args.version
    if not SEMVER.match(version):
        raise Error(f"--version {version!r} is not a version like 0.15.0")
    wrong = []

    found = workspace_version()
    if found != version:
        wrong.append(f"Cargo.toml: workspace version is {found}, not {version}")

    manifests = [os.path.join(ROOT, "packages", "sdk", "package.json")]
    manifests += sorted(glob.glob(os.path.join(ROOT, "packages", "sdk", "npm", "*", "package.json")))
    manifests += [os.path.join(ROOT, "packages", "engine", "package.json"), os.path.join(ROOT, "packages", "cleanup", "package.json")]
    for path in manifests:
        shown = os.path.relpath(path, ROOT)
        try:
            with open(path, encoding="utf-8") as f:
                data = json.load(f)
        except (OSError, ValueError) as error:
            wrong.append(f"{shown}: cannot read it ({error})")
            continue
        if data.get("version") != version:
            wrong.append(f"{shown}: version is {data.get('version')!r}, not {version}")
        for field in DEPENDENCY_FIELDS:
            for name, range_ in (data.get(field) or {}).items():
                if name.startswith(f"{SCOPE}/") and range_ != version:
                    wrong.append(f"{shown}: {field} {name} is {range_!r}, not {version}")
    if wrong:
        raise Error("these files do not have the version " + version + ":\n" + "\n".join(f"  {line}" for line in wrong))
    print(f"==> all versions are {version} ({len(manifests)} package.json files and Cargo.toml)")


def add_kind_command(commands, kind):
    command = commands.add_parser(kind, help=f"make the {KINDS[kind]['title']} packages from the {kind} release files")
    command.add_argument("--version", required=True, help="the version of the release, like 0.15.0")
    command.add_argument("--release-dir", required=True, help=f"the folder with the {kind} release files")
    command.add_argument("--out", required=True, help="the folder to write the packages to")
    command.add_argument("--allow-missing", action="store_true", help="skip a platform that has no release file")
    command.add_argument("--no-npm-check", action="store_true", help="do not run npm pack --dry-run")


def main(argv):
    parser = argparse.ArgumentParser(description="Make the npm packages of Sayso from the files of a release.")
    commands = parser.add_subparsers(dest="command", required=True)
    for kind in KINDS:
        add_kind_command(commands, kind)
    check = commands.add_parser("check-versions", help="check that all packages and the Cargo workspace have one version")
    check.add_argument("--version", required=True, help="the version that every file must have, like 0.15.0")
    args = parser.parse_args(argv)
    try:
        if args.command in KINDS:
            run_kind(args.command, args)
        else:
            run_check_versions(args)
    except Error as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

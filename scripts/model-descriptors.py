#!/usr/bin/env python3
"""Write the descriptors of the model store (docs/sdk.md, "Model store").

The catalog in crates/sayso-core says which sources each model needs
(`engine_catalog::descriptors::wanted`). This script pins each source and
hashes each file, and writes crates/sayso-core/src/engine_catalog/descriptors.json:

- A Hugging Face source on a branch gets the commit of the branch. The file
  list comes from the tree of that commit. A file in LFS has its SHA-256 and
  size in the tree, so it is not downloaded. Other files are downloaded and
  hashed, and checked against the git object id of the tree.
- An archive (.tar.bz2) is downloaded and read without extraction. The files
  are its regular files after the root rule of crates/sayso-engine/src/download.rs:
  one folder that holds everything is removed. An archive with a link, a
  device, a path with "..", or an absolute path is refused.
- A single file is downloaded and hashed.

A source that the data already pins keeps its commit, so a second run gives
the same data. With --update, branches are resolved again. A descriptor gets
revision 1, and its revision goes up by one when its artifacts change.

Downloads are untrusted data. They go into a new empty temporary folder,
archives are never extracted to disk, and nothing in them runs.

Usage (from the repository root):
  python3 -I scripts/model-descriptors.py [--wanted FILE] [--update] [--only NAME]... [--downloads DIR]

Without --wanted, the script runs:
  cargo run -q -p sayso-core --example descriptor_sources
--only NAME takes only the artifacts whose name contains NAME, and keeps
the data of the others. --downloads DIR keeps the downloaded archives and
files in DIR and uses them again in a later run.
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "crates/sayso-core/src/engine_catalog/descriptors.json")
HF = "https://huggingface.co"
COMMIT = re.compile(r"^[0-9a-f]{40}$")
USER_AGENT = "sayso-model-descriptors/1"


class Refused(Exception):
    """A source that the store cannot take."""


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--wanted", help="the output of the example descriptor_sources")
    parser.add_argument("--data", default=DATA, help="the data file to read and write")
    parser.add_argument("--update", action="store_true", help="resolve branches again")
    parser.add_argument("--only", action="append", default=[], help="only artifacts whose name contains this")
    parser.add_argument("--downloads", help="a folder that keeps downloads between runs")
    args = parser.parse_args()

    wanted = load_wanted(args.wanted)
    old = load_json(args.data) if os.path.exists(args.data) else {"artifacts": {}, "descriptors": []}

    sources = {}
    for w in wanted:
        if not w["parts"]:
            sys.exit(f"{w['model']} ({w['format']}) has no artifacts")
        for part in w["parts"]:
            name, source = part["artifact"], part["source"]
            if sources.setdefault(name, source) != source:
                sys.exit(f"the artifact {name} has two sources: {sources[name]} and {source}")

    keep = args.downloads
    if keep:
        os.makedirs(keep, exist_ok=True)
    temp = tempfile.mkdtemp(prefix="sayso-descriptors-")
    try:
        artifacts = {}
        for name in sorted(sources):
            if args.only and not any(o in name for o in args.only):
                if name in old["artifacts"]:
                    artifacts[name] = old["artifacts"][name]
                continue
            print(f"{name}", file=sys.stderr)
            try:
                artifacts[name] = make_artifact(sources[name], old["artifacts"].get(name), args.update, keep or temp)
            except Refused as why:
                sys.exit(f"{name}: {why}")
    finally:
        shutil.rmtree(temp, ignore_errors=True)

    descriptors = []
    old_descriptors = {(d["model"], d["format"]): d for d in old["descriptors"]}
    for w in sorted(wanted, key=lambda w: (w["model"], w["format"])):
        names = [p["artifact"] for p in w["parts"]]
        missing = [n for n in names if n not in artifacts]
        if missing:
            print(f"no data for {w['model']} ({w['format']}): {missing}", file=sys.stderr)
            continue
        parts = [{"role": p["role"], "artifact": p["artifact"]} for p in w["parts"]]
        revision = 1
        before = old_descriptors.get((w["model"], w["format"]))
        if before:
            same = [(p["role"], old["artifacts"].get(p["artifact"], {}).get("digest")) for p in before["parts"]] == [
                (p["role"], artifacts[p["artifact"]]["digest"]) for p in parts
            ]
            revision = before["revision"] if same else before["revision"] + 1
        descriptors.append({"model": w["model"], "format": w["format"], "revision": revision, "parts": parts})

    used = {p["artifact"] for d in descriptors for p in d["parts"]}
    artifacts = {k: v for k, v in artifacts.items() if k in used}
    with open(args.data, "w", encoding="utf-8") as f:
        f.write(render({"artifacts": artifacts, "descriptors": descriptors}))
    print(f"wrote {len(descriptors)} descriptors with {len(artifacts)} artifacts to {args.data}", file=sys.stderr)


def load_wanted(path):
    if path:
        return load_json(path)
    command = ["cargo", "run", "-q", "-p", "sayso-core", "--example", "descriptor_sources"]
    out = subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def load_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


# Artifacts


def make_artifact(source, before, update, downloads):
    kind = source["kind"]
    if kind == "hugging_face":
        pinned, files = hugging_face(source, before, update, downloads)
    elif kind == "archive":
        pinned = {"kind": "archive", "url": source["url"]}
        files = archive_files(fetch(source["url"], downloads))
    elif kind == "file":
        pinned = {"kind": "file", "url": source["url"]}
        path = fetch(source["url"], downloads)
        name = urllib.parse.unquote(source["url"].rsplit("/", 1)[1])
        files = [{"path": name, "size": os.path.getsize(path), "sha256": sha256_file(path)}]
    else:
        raise Refused(f"unknown source kind {kind}")
    files.sort(key=lambda f: f["path"].encode())
    if not files:
        raise Refused("the source has no files")
    return {"source": pinned, "digest": digest(files), "files": files}


def hugging_face(source, before, update, downloads):
    repo, rev, folder, include = source["repo"], source["rev"], source["folder"], source["include"]
    old = (before or {}).get("source", {})
    if COMMIT.match(rev):
        commit = rev
    elif not update and old.get("kind") == "hugging_face" and old.get("repo") == repo and old.get("folder", "") == folder:
        commit = old["revision"]
    else:
        commit = get_json(f"{HF}/api/models/{repo}/revision/{quote(rev)}")["sha"]
    if not COMMIT.match(commit):
        raise Refused(f"{repo} at {rev} has no commit id")
    pinned = {"kind": "hugging_face", "repo": repo, "revision": commit}
    if folder:
        pinned["folder"] = folder
    prefix = folder + "/" if folder else ""
    tree_url = f"{HF}/api/models/{repo}/tree/{commit}" + (f"/{quote(folder)}" if folder else "") + "?recursive=true"
    files = []
    for entry in get_list(tree_url):
        if entry.get("type") != "file" or not entry["path"].startswith(prefix):
            continue
        path = entry["path"][len(prefix):]
        if include and not any(path == i or path.startswith(i + "/") for i in include):
            continue
        check_path(path)
        lfs = entry.get("lfs")
        if lfs:
            sha, size = lfs["oid"], lfs["size"]
        else:
            url = f"{HF}/{repo}/resolve/{commit}/{quote(entry['path'])}"
            local = fetch(url, downloads)
            size = os.path.getsize(local)
            if size != entry["size"] or git_blob(local) != entry["oid"]:
                raise Refused(f"{entry['path']} does not match the tree of {repo} at {commit}")
            sha = sha256_file(local)
        files.append({"path": path, "size": size, "sha256": sha})
    for i in include:
        if not any(f["path"] == i or f["path"].startswith(i + "/") for f in files):
            raise Refused(f"{repo} at {commit} has no {prefix}{i}")
    return pinned, files


def archive_files(path):
    """The regular files of a .tar.bz2, after the root rule. Nothing is written to disk."""
    files, tops, dirs = {}, set(), set()
    with tarfile.open(path, mode="r|bz2") as tar:
        for member in tar:
            name = member.name
            while name.startswith("./"):
                name = name[2:]
            name = name.rstrip("/")
            if name in ("", "."):
                continue
            if member.issym() or member.islnk():
                raise Refused(f"the archive has a link: {member.name}")
            if name.startswith("/") or ".." in name.split("/"):
                raise Refused(f"the archive has a path outside it: {member.name}")
            tops.add(name.split("/")[0])
            if member.isdir():
                dirs.add(name)
                continue
            if not member.isfile():
                raise Refused(f"the archive has an entry that is not a file or a folder: {member.name}")
            if name in files:
                raise Refused(f"the archive has {name} twice")
            hasher = hashlib.sha256()
            size = 0
            stream = tar.extractfile(member)
            while chunk := stream.read(1 << 20):
                hasher.update(chunk)
                size += len(chunk)
            if size != member.size:
                raise Refused(f"{name} is incomplete")
            files[name] = {"path": name, "size": size, "sha256": hasher.hexdigest()}
    # One folder that holds everything is the artifact (download.rs, move_into_place).
    if len(tops) == 1:
        (top,) = tops
        if top in dirs or any(n.startswith(top + "/") for n in files):
            files = {n[len(top) + 1:]: dict(f, path=n[len(top) + 1:]) for n, f in files.items()}
    for name in files:
        check_path(name)
    return list(files.values())


# The digest, as in engine_catalog::descriptors::digest.


def check_path(path):
    parts = path.split("/")
    if not path or any(p in ("", ".", "..") for p in parts) or any(c in "\\:" or ord(c) < 32 or ord(c) == 127 for c in path):
        raise Refused(f"the path {path!r} is not a plain relative path")


def digest(files):
    seen = set()
    hasher = hashlib.sha256()
    for f in sorted(files, key=lambda f: f["path"].encode()):
        check_path(f["path"])
        if f["path"].lower() in seen:
            raise Refused(f"the path {f['path']!r} is in the list twice")
        seen.add(f["path"].lower())
        if not re.fullmatch(r"[0-9a-f]{64}", f["sha256"]):
            raise Refused(f"the hash of {f['path']!r} is not SHA-256")
        hasher.update(f["path"].encode() + b"\0" + str(f["size"]).encode() + b"\0" + f["sha256"].encode() + b"\n")
    return hasher.hexdigest()


# The network


def quote(path):
    return "/".join(urllib.parse.quote(p, safe="") for p in path.split("/"))


def request(url):
    headers = {"User-Agent": USER_AGENT}
    for attempt in range(4):
        try:
            return urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60)
        except urllib.error.HTTPError as e:
            if e.code < 500 and e.code != 429:
                raise
            why = e
        except OSError as e:
            why = e
        time.sleep(2 ** attempt)
    raise why


def get_json(url):
    with request(url) as response:
        return json.load(response)


def get_list(url):
    """A list from the Hub API, with every page."""
    items = []
    while url:
        with request(url) as response:
            items.extend(json.load(response))
            link = response.headers.get("Link") or ""
        match = re.search(r'<([^>]+)>;\s*rel="next"', link)
        url = match.group(1) if match else None
    return items


def fetch(url, folder):
    """Download `url` into `folder` once, and return the path."""
    path = os.path.join(folder, hashlib.sha256(url.encode()).hexdigest())
    if os.path.exists(path):
        return path
    partial = path + ".partial"
    with request(url) as response, open(partial, "wb") as out:
        expected = response.headers.get("Content-Length")
        shutil.copyfileobj(response, out, 1 << 20)
    if expected is not None and os.path.getsize(partial) != int(expected):
        os.remove(partial)
        raise Refused(f"the download of {url} is incomplete")
    os.replace(partial, path)
    return path


def sha256_file(path):
    hasher = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            hasher.update(chunk)
    return hasher.hexdigest()


def git_blob(path):
    """The git object id of a file, which the Hub tree gives for files outside LFS."""
    hasher = hashlib.sha1()
    hasher.update(f"blob {os.path.getsize(path)}\0".encode())
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            hasher.update(chunk)
    return hasher.hexdigest()


# Output


def render(data):
    """JSON with one line for each file, so a change shows as a small diff."""
    one = lambda v: json.dumps(v, ensure_ascii=False)
    lines = ["{", '  "artifacts": {']
    names = sorted(data["artifacts"])
    for i, name in enumerate(names):
        a = data["artifacts"][name]
        lines.append(f"    {one(name)}: {{")
        lines.append(f'      "source": {one(a["source"])},')
        lines.append(f'      "digest": {one(a["digest"])},')
        lines.append('      "files": [')
        for j, f in enumerate(a["files"]):
            lines.append(f"        {one(f)}" + ("," if j + 1 < len(a["files"]) else ""))
        lines.append("      ]")
        lines.append("    }" + ("," if i + 1 < len(names) else ""))
    lines.append("  },")
    lines.append('  "descriptors": [')
    for i, d in enumerate(data["descriptors"]):
        lines.append(f"    {one(d)}" + ("," if i + 1 < len(data["descriptors"]) else ""))
    lines.append("  ]")
    lines.append("}")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    main()

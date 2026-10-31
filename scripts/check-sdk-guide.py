#!/usr/bin/env python3
"""Check that the Rust code in docs/sdk-guide/ is the code that CI compiles.

Every ```rust block in the guide needs a line above it with a tag:

  <!-- check: file crates/sayso-sdk/examples/scribe.rs -->      the block is the whole file
  <!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->    the block is lines of the file, in a row
  <!-- check: lines crates/sayso-sdk/examples/scribe.rs -->      each line of the block is a line of the file
  <!-- check: unchecked -->                                      a fragment that nothing compiles

The examples compile with `cargo clippy --workspace --all-targets`. Run this script from any folder.
"""
import re
import sys
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUIDE = ROOT / "docs" / "sdk-guide"
TAG = re.compile(r"^<!-- check: (file|verbatim|lines|unchecked)(?: (\S+))? -->$")


def blocks(text):
    """Yield (line number, tag line, code) for each rust block."""
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        if re.match(r"^\s*```rust\s*$", lines[i]):
            start = i
            tag = lines[i - 1].strip() if i > 0 else ""
            body = []
            i += 1
            while i < len(lines) and not re.match(r"^\s*```\s*$", lines[i]):
                body.append(lines[i])
                i += 1
            yield start + 1, tag, textwrap.dedent("\n".join(body))
        i += 1


def main():
    problems = []
    checked = 0
    for page in sorted(GUIDE.glob("*.md")):
        for number, tag, code in blocks(page.read_text()):
            where = f"{page.relative_to(ROOT)}:{number}"
            found = TAG.match(tag)
            if not found:
                problems.append(f"{where}: a rust block needs a tag above it, for example <!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->")
                continue
            kind, name = found.groups()
            if kind == "unchecked":
                continue
            source = (ROOT / name).read_text() if name and (ROOT / name).is_file() else None
            if source is None:
                problems.append(f"{where}: there is no file {name}")
                continue
            checked += 1
            if kind == "file":
                if code.strip("\n") != source.strip("\n"):
                    problems.append(f"{where}: the block is not the whole file {name}")
            elif kind == "verbatim":
                if code.strip("\n") not in source:
                    problems.append(f"{where}: the block is not in {name} line for line")
            else:
                have = {line.strip() for line in source.split("\n")}
                for line in code.split("\n"):
                    if line.strip() and line.strip() not in have:
                        problems.append(f"{where}: this line is not in {name}: {line.strip()}")
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1
    print(f"docs/sdk-guide: {checked} code blocks match the examples that CI compiles")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Check that the Rust and TypeScript code in docs/sdk-guide/ is the code that CI compiles.

Every ```rust block and every ```ts block in the guide needs a line above it with a tag:

  <!-- check: file crates/sayso-sdk/examples/scribe.rs -->      the block is the whole file
  <!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->    the block is lines of the file, in a row
  <!-- check: lines crates/sayso-sdk/examples/scribe.rs -->      each line of the block is a line of the file
  <!-- check: unchecked -->                                      a fragment that nothing compiles

The path of a ```ts block must be a file in packages/sdk/examples/.

The Rust examples compile with `cargo clippy --workspace --all-targets`. The TypeScript examples
compile with `pnpm --filter @justsayso/sdk check:examples` (`tsc -p packages/sdk/examples`).
Run this script from any folder.
"""
import re
import sys
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUIDE = ROOT / "docs" / "sdk-guide"
TAG = re.compile(r"^<!-- check: (file|verbatim|lines|unchecked)(?: (\S+))? -->$")
TS_EXAMPLES = "packages/sdk/examples/"
EXAMPLES = {
    "rust": "<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->",
    "ts": f"<!-- check: verbatim {TS_EXAMPLES}tutorial.ts -->",
}


def blocks(text, language):
    """Yield (line number, tag line, code) for each block of the language."""
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        if re.match(rf"^\s*```{language}\s*$", lines[i]):
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
        text = page.read_text()
        found_blocks = [(language, *block) for language in EXAMPLES for block in blocks(text, language)]
        for language, number, tag, code in sorted(found_blocks, key=lambda found: found[1]):
            where = f"{page.relative_to(ROOT)}:{number}"
            found = TAG.match(tag)
            if not found:
                problems.append(f"{where}: a {language} block needs a tag above it, for example {EXAMPLES[language]}")
                continue
            kind, name = found.groups()
            if kind == "unchecked":
                continue
            if language == "ts" and not (name or "").startswith(TS_EXAMPLES):
                problems.append(f"{where}: the file of a ts block must be in {TS_EXAMPLES}")
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

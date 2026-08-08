#!/usr/bin/env python3
"""Report cyrup doc-comment citations that cannot resolve at a given upstream tag.

Doc comments carry this port's provenance — they are how parity is audited — so a citation that
points at the wrong line is a broken index, not a cosmetic slip. This finds the provable failures.

The check is deliberately weak: it only proves a citation is *impossible* (past EOF). A citation
that lands inside the file may still be wrong, because deleting code upstream shifts every line
below it. Treat a clean run as "no proven breakage", never as "citations verified".

The deeper issue this surfaced (2026-08-08): a BARE `index.ts:1447` is correct when the citing code
ports v0.7.1 and wrong when it ports v0.8.0, and a crate mid-upgrade contains both. Citations should
name their tag — `v0.8.0 index.ts:1203-1205` — or they cannot be checked at all.

    ./check-citations.py cyrup-permission-system pi-permission-system v0.8.0 index.ts
"""
import collections
import pathlib
import re
import subprocess
import sys

WS = pathlib.Path("/home/d0m17bw/workspace")


def main() -> int:
    if len(sys.argv) != 5:
        print(__doc__)
        return 2
    crate, repo, tag, upstream_file = sys.argv[1:5]

    proc = subprocess.run(
        ["git", "-C", str(WS / repo), "show", f"{tag}:src/{upstream_file}"],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        print(f"cannot read {repo} {tag}:src/{upstream_file}\n{proc.stderr}", file=sys.stderr)
        return 2
    eof = len(proc.stdout.split("\n"))

    pattern = re.compile(re.escape(upstream_file) + r":(\d+)")
    # A citation already qualified with this tag is self-describing; one qualified with a DIFFERENT
    # tag is deliberate history and must not be reported against this tag.
    qualified_other = re.compile(r"v\d+\.\d+\.\d+\s+(?:pi\s+)?`?" + re.escape(upstream_file))

    total = 0
    per_file: collections.Counter = collections.Counter()
    broken: list[tuple[str, int, int, str]] = []

    root = WS / "cyrup" / "crates" / crate / "src"
    for path in sorted(root.rglob("*.rs")):
        for lineno, text in enumerate(path.read_text().splitlines(), 1):
            for m in pattern.finditer(text):
                total += 1
                per_file[path.name] += 1
                cited = int(m.group(1))
                if cited > eof:
                    tagged = "" if qualified_other.search(text) else " (untagged)"
                    broken.append((path.name, lineno, cited, text.strip()[:100] + tagged))

    print(f"{repo} {tag}:src/{upstream_file} has {eof} lines")
    print(f"{total} citations across {len(per_file)} files in {crate}")
    print(f"{len(broken)} cite a line PAST EOF — provably unresolvable at {tag}\n")
    for name, lineno, cited, text in broken:
        print(f"  {name}:{lineno}  cites :{cited}  {text}")

    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main())

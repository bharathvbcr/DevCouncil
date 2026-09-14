#!/usr/bin/env python3
"""Assert REPORT.md's tables still agree with the raw measurements.

A report drifts from its data the moment someone edits a number by hand. This
re-renders every table from comparison.json and requires each data row to appear
verbatim in REPORT.md, so the check fails loudly rather than a stale figure
surviving into the published claim.

Thousands separators are normalised away because the report adds them for
readability; nothing else is.
"""

import pathlib
import subprocess
import sys

N = pathlib.Path(__file__).resolve().parent


RENDERERS = ("render_tables.py", "render_semantics.py")
HEADERS = ("| repo", "| Tool", "| Compared with", "| Native surface")


def main():
    report = (N / "REPORT.md").read_text().replace(",", "")
    total, missing = 0, []
    for renderer in RENDERERS:
        gen = subprocess.run([sys.executable, str(N / renderer)],
                             capture_output=True, text=True, check=True).stdout
        rows = [l.strip() for l in gen.splitlines()
                if l.startswith("| ") and "---" not in l
                and not l.startswith(HEADERS)]
        total += len(rows)
        missing += [(renderer, r) for r in rows if r.replace(",", "") not in report]

    print("generated data rows: %d across %d renderers" % (total, len(RENDERERS)))
    if missing:
        print("ROWS NOT PRESENT IN REPORT.md: %d" % len(missing))
        for renderer, m in missing:
            print("   [%s] %s" % (renderer, m))
        return 1
    print("every generated row appears in REPORT.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

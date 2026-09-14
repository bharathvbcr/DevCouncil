#!/usr/bin/env python3
"""Render the semantics section's tables from semantics-audit.json and staleness.json."""

import json
from pathlib import Path

OUT = Path(__file__).resolve().parent
AUDIT = json.loads((OUT / "semantics-audit.json").read_text())
STALE = {r["tool"]: r for r in json.loads((OUT / "staleness.json").read_text())}
TOOLS = ("devmap", "codegraph", "cbm", "gitnexus", "graphify", "gortex")
NAME = {"devmap": "**DevMap 0.2.2**", "codegraph": "CodeGraph", "cbm": "CBM",
        "gitnexus": "GitNexus", "graphify": "Graphify", "gortex": "Gortex"}
COLS = ["search/Rust", "search/Go", "search/Python",
        "callers/Rust", "callers/Go", "callers/Python"]
HEAD = ["Rust search", "Go search", "Python search",
        "Rust callers", "Go callers", "Python callers"]


def correctness():
    lines = ["| Tool | Exact definitions | Caller pairs | Edits visible | Deleted probe absent | Revert-to-original |",
             "|---|---:|---:|---:|---:|---|"]
    for t in TOOLS:
        mine = [a for a in AUDIT["answers"] if a["tool"] == t]
        defs_ok = sum(1 for a in mine for s in a["samples"] if s["exact_definition"])
        defs_n = sum(len(a["samples"]) for a in mine)
        pairs, total = set(), 0
        for a in mine:
            total += len(a["expected"])
            if a["samples"]:
                pairs |= set(a["samples"][0]["matched"])
        edits = [c for c in AUDIT["controls"] if c["tool"] == t and c["expected"] == "present"]
        neg = [c for c in AUDIT["controls"] if c["tool"] == t and c["expected"] == "absent"]
        if t not in STALE:
            verdict = "*not measured*"
        elif STALE[t].get("stale"):
            verdict = "**stale**" + (", forced rebuild clears it"
                                     if STALE[t].get("present_after_forced_rebuild") is False
                                     else ", not cleared")
        else:
            verdict = "clean"
        lines.append("| %s | %d/%d | %s | %d/%d | %d/%d | %s |" % (
            NAME[t], defs_ok, defs_n,
            ("**%d/%d**" % (len(pairs), total)) if len(pairs) == total
            else "%d/%d" % (len(pairs), total),
            sum(1 for c in edits if c["passed"]), len(edits),
            sum(1 for c in neg if c["passed"]), len(neg), verdict))
    return "\n".join(lines)


def latency():
    lat = AUDIT["query_latency_seconds"]
    lines = ["| Tool | " + " | ".join(HEAD) + " |", "|---|" + "---:|" * len(HEAD)]
    for t in TOOLS:
        cells = []
        for c in COLS:
            v = lat[t].get(c)
            ms = v["median"] * 1000
            cells.append(("**%.1f ms**" if t == "devmap" else "%.1f ms") % ms)
        lines.append("| %s | %s |" % (NAME[t], " | ".join(cells)))
    rg = AUDIT["ripgrep_seconds"]
    lines.append("| ripgrep (text) | %s | — | — | — |" % " | ".join(
        "%.1f ms" % (rg[k]["median"] * 1000) for k in ("Rust", "Go", "Python")))
    return "\n".join(lines)


def factors():
    lat = AUDIT["query_latency_seconds"]
    base = {c: lat["devmap"][c]["median"] for c in COLS}
    lines = ["| Compared with | " + " | ".join(HEAD) + " |", "|---|" + "---:|" * len(HEAD)]
    for t in TOOLS:
        if t == "devmap":
            continue
        cells = []
        for c in COLS:
            f = lat[t][c]["median"] / base[c]
            cells.append("%.1f× faster" % f if f >= 1 else "%.2f× slower" % (1 / f))
        lines.append("| %s | %s |" % (NAME[t].replace("**", ""), " | ".join(cells)))
    return "\n".join(lines)


def missing():
    lines = []
    for t in TOOLS:
        miss = sorted({m for a in AUDIT["answers"] if a["tool"] == t
                       for s in a["samples"][:1] for m in s["missing"]})
        lines.append("- %s: %s" % (NAME[t].replace("**", ""),
                                   "; ".join("`%s`" % m for m in miss) if miss
                                   else "none in this five-pair sample."))
    return "\n".join(lines)


def main():
    print("## CORRECTNESS\n");  print(correctness())
    print("\n## MISSING\n");    print(missing())
    print("\n## LATENCY\n");    print(latency())
    print("\n## FACTORS\n");    print(factors())


if __name__ == "__main__":
    main()

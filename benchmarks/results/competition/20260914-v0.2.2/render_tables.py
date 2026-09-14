#!/usr/bin/env python3
"""Render REPORT.md's tables from comparison.json, so no number is hand-copied.

Every table in the report is emitted here. Transcribing a timing by hand into
prose is how a report starts disagreeing with its own raw data.
"""

import json
from pathlib import Path

OUT = Path(__file__).resolve().parent
C = json.loads((OUT / "comparison.json").read_text())
REPOS = ("DevPrism", "DevCouncil", "GitPulse", "scholarlm")
COMPETITORS = ("codegraph", "cbm", "gitnexus", "graphify")
NAME = {"devmap": "DevMap", "codegraph": "CodeGraph", "cbm": "CBM",
        "gitnexus": "GitNexus", "graphify": "Graphify", "gortex": "Gortex"}


def stage_table(stage):
    lines = ["| repo | DevMap | " + " | ".join(NAME[t] for t in COMPETITORS) + " |",
             "|---|---:|" + "---:|" * len(COMPETITORS)]
    for repo in REPOS:
        cells = C["timings"][repo]
        base = cells["devmap"][stage]["min"]
        # Bold marks the winner, so DevMap is only bold when nothing beats it.
        devmap_wins = all(cells[t][stage]["min"] >= base for t in COMPETITORS)
        row = ["| %s" % repo,
               ("**%.3fs**" if devmap_wins else "%.3fs") % base]
        for tool in COMPETITORS:
            v = cells[tool][stage]["min"]
            best = v < base
            factor = v / base
            shown = ("%.0f" % factor if factor >= 10 else
                     "%.1f" % factor if factor >= 1 else "%.2f" % factor)
            cell = "%.3fs ×%s" % (v, shown)
            row.append("**%s**" % cell if best else cell)
        lines.append(" | ".join(row) + " |")
    return "\n".join(lines)


def rss_table():
    tools = ("devmap", "codegraph", "gitnexus", "graphify")
    lines = ["| repo | " + " | ".join(NAME[t] for t in tools) + " |",
             "|---|" + "---:|" * len(tools)]
    for repo in REPOS:
        vals = {t: C["timings"][repo][t]["cold"]["peak_rss_bytes"] for t in tools}
        low = min(vals.values())
        row = ["| %s" % repo]
        for tool in tools:
            mb = "%.0f" % (vals[tool] / 1048576)
            row.append("**%s**" % mb if vals[tool] == low else mb)
        lines.append(" | ".join(row) + " |")
    return "\n".join(lines)


def index_table():
    tools = ("devmap", "codegraph", "graphify", "gitnexus", "gortex")
    lines = ["| repo | " + " | ".join(NAME[t] for t in tools) + " |",
             "|---|" + "---:|" * len(tools)]
    for repo in REPOS:
        vals = {t: C["index_bytes"][repo].get(t) for t in tools}
        present = [v for v in vals.values() if v]
        low = min(present) if present else None
        row = ["| %s" % repo]
        for tool in tools:
            v = vals[tool]
            if not v:
                row.append("—")
                continue
            mb = "%.0f" % (v / 1048576)
            row.append("**%s**" % mb if v == low else mb)
        lines.append(" | ".join(row) + " |")
    return "\n".join(lines)


def corpus_table():
    lines = ["| repo | commit | files | symbols | edges |", "|---|---|---:|---:|---:|"]
    for repo in REPOS:
        g = C["devmap_graph"][repo]
        lines.append("| %s | `%s` | %s | %s | %s |" % (
            repo, C["corpus_commits"][repo][:12],
            "{:,}".format(g["devmap_files_indexed"]),
            "{:,}".format(g["devmap_symbols"]), "{:,}".format(g["devmap_edges"])))
    return "\n".join(lines)


def gortex_table():
    lines = ["| repo | query-ready | enrichment complete | files reindexed | database | load |",
             "|---|---:|---:|---:|---:|---:|"]
    by_repo = {g["repo"]: g for g in C["gortex_gates"]}
    for repo in REPOS:
        g = by_repo.get(repo)
        if not g:
            continue
        lines.append("| %s | %.2fs | %.2fs | %s | %.0f MB | %.1f |" % (
            repo, g["query_ready_seconds"], g["enriched_seconds"],
            "{:,}".format(g["files_reindexed"]),
            g["database_bytes"] / 1048576, g["load_average"][0]))
    return "\n".join(lines)


def cbm_rss_line():
    seen = C["caveats"]["cbm_cold_peak_rss_mb"]
    return ", ".join("%s %s MB" % (r, seen[r]) for r in REPOS if r in seen)


def main():
    print("## CORPUS\n");        print(corpus_table())
    for stage in ("cold", "warm", "touch"):
        print("\n## %s\n" % stage.upper()); print(stage_table(stage))
    print("\n## RSS\n");         print(rss_table())
    print("\n## INDEX\n");       print(index_table())
    print("\n## GORTEX\n");      print(gortex_table())
    print("\n## CBM RSS\n");     print(cbm_rss_line())


if __name__ == "__main__":
    main()

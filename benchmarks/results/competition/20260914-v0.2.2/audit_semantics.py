#!/usr/bin/env python3
"""Score the semantics campaign: definitions, callers, edit visibility, freshness.

The per-tool response parsers are carried over from the build-48cd3c7 run's
`audit_results.py`, because a parser rewritten from scratch would silently
re-score the earlier report's numbers under different rules.

One deliberate change: that version used bare `assert`, so a single unparsable
response aborted the whole audit. Here a response that cannot be parsed is
recorded as `unparsed` and counted against the tool's total separately. A check
that could not run must never report what a check that ran and passed reports.
"""

import json
import re
import statistics
from pathlib import Path

OUT = Path(__file__).resolve().parent
TOOLS = ("devmap", "codegraph", "cbm", "gitnexus", "graphify", "gortex")
GORTEX_PREFIX = "devcouncil-bench/"
REPS = 5
NEVER_EXISTED = "devmap_competition_nonexistent_8aef7e52"
EDIT_FILE = "benchmarks/tasks.py"

PATHS = {
    "db_size_gate_bytes": "rust/devmap-extract/src/model.rs",
    "loadBounded": "backend/go_orchestrator/repomap/repomap.go",
    "describe_corpus": "benchmarks/map_bench.py",
}
LANGUAGE = {"db_size_gate_bytes": "Rust", "loadBounded": "Go",
            "describe_corpus": "Python"}
EXPECTED = {
    "db_size_gate_bytes": {
        "rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes",
        "rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor"},
    "loadBounded": {
        "backend/go_orchestrator/repomap/repomap.go::Load",
        "backend/go_orchestrator/repomap/compact_test.go::TestAnInternedGraphIsHeldToTheSameBound"},
    "describe_corpus": {"benchmarks/map_bench.py::main"},
}


def read(label):
    return (OUT / (label + ".stdout")).read_text()


def found(tool, text, name, path):
    """Did the tool return this exact symbol at this exact source path?"""
    if tool == "graphify":
        return (text.startswith("Node: " + name + "()\n")
                and re.search(r"^  Source:\s+" + re.escape(path) + r" L\d+", text, re.M)
                is not None)
    d = json.loads(text)
    if tool == "devmap":
        if d["resolution"] != "Available":
            raise ValueError("devmap resolution=%s" % d["resolution"])
        return any(x["symbol_name"] == name and x["file_path"] == path for x in d["items"])
    if tool == "gitnexus":
        if "error" in d:
            return False
        return (d.get("status") == "found" and d["symbol"]["name"] == name
                and d["symbol"]["filePath"] == path)
    if tool == "codegraph":
        return any(x["node"]["name"] == name and x["node"]["filePath"] == path for x in d)
    if tool == "cbm":
        return any(g.get("file") == path and row[d["cols"].index("name")] == name
                   for g in d["groups"] for row in g["rows"])
    if tool == "gortex":
        return any(x["name"] == name
                   and x["file_path"].removeprefix(GORTEX_PREFIX) == path
                   for x in d["results"])
    raise ValueError(tool)


def cbm_locations():
    d = json.loads(read("cbm-caller-locations"))
    lookup = {}
    for g in d["groups"]:
        for row in g["rows"]:
            name = row[d["cols"].index("name")]
            lookup.setdefault((g["qn_prefix"], name), set()).add(g["file"] + "::" + name)
    return lookup


def callers(tool, text, target):
    """The set of `file::symbol` the tool reports as calling `target`."""
    if tool == "graphify":
        if not text.startswith("Affected nodes for " + target + "()"):
            raise ValueError("graphify: unexpected header")
        return {m[2] + "::" + m[1]
                for m in re.finditer(r"^- (.+)\(\) \[calls\] (.+):L\d+\s*$", text, re.M)}
    d = json.loads(text)
    if tool == "devmap":
        defs = [x for x in d["definitions"]["items"]
                if x["symbol_name"] == target and x["file_path"] == PATHS[target]]
        if len(defs) != 1:
            raise ValueError("devmap: %d definitions" % len(defs))
        return {x["source_symbol"] for x in defs[0]["callers"]["items"]
                if x["edge_kind"] == "Calls"}
    if tool == "gitnexus":
        return {x["filePath"] + "::" + x["name"]
                for x in d.get("incoming", {}).get("calls", [])}
    if tool == "codegraph":
        return {x["filePath"] + "::" + x["name"] for x in d["callers"]}
    if tool == "gortex":
        want = GORTEX_PREFIX + PATHS[target] + "::" + target
        return {x["from"].removeprefix(GORTEX_PREFIX) for x in d["edges"]
                if x["kind"] == "calls" and x["to"] == want}
    if tool == "cbm":
        c = d.get("callers", {})
        lookup = cbm_locations()
        result = set()
        for g in c.get("groups", []):
            for row in g["rows"]:
                name = row[c["cols"].index("name")]
                locations = lookup.get((g["qn_prefix"], name), set())
                if len(locations) != 1:
                    raise ValueError("cbm: %s resolved to %s" % (name, locations))
                result.update(locations)
        return result
    raise ValueError(tool)


def flags(value, path=""):
    """Truncation / incompleteness markers a tool volunteers about its own answer."""
    watched = ("truncated", "has_more", "walk_incomplete", "hidden", "nodes_omitted",
               "text_matched_suppressed", "_truncated_by_budget", "next_cursor")
    result = []
    if isinstance(value, dict):
        for k, v in value.items():
            if k in watched and v:
                result.append({"path": path + "." + k, "value": v})
            result += flags(v, path + "." + k)
    elif isinstance(value, list):
        for i, v in enumerate(value):
            result += flags(v, path + "[%d]" % i)
    return result


def main():
    rows = [json.loads(l) for l in
            (OUT / "semantics-measurements.jsonl").read_text().splitlines() if l.strip()]
    records = {r["label"]: r for r in rows}

    answers, controls, limits, unparsed = [], [], [], []

    for tool in TOOLS:
        for target, path in PATHS.items():
            samples = []
            for i in range(1, REPS + 1):
                a, b = ("search-%s-%s-%d" % (tool, target, i),
                        "callers-%s-%s-%d" % (tool, target, i))
                ok = records[a]["measurement_ok"] and records[b]["measurement_ok"]
                try:
                    exact = found(tool, read(a), target, path)
                    got = callers(tool, read(b), target)
                except Exception as exc:
                    unparsed.append({"tool": tool, "target": target, "iteration": i,
                                     "error": "%s: %s" % (type(exc).__name__, exc)})
                    continue
                samples.append({"iteration": i, "command_ok": ok,
                                "exact_definition": exact, "returned": sorted(got),
                                "matched": sorted(got & EXPECTED[target]),
                                "missing": sorted(EXPECTED[target] - got),
                                "additional_unjudged": sorted(got - EXPECTED[target])})
                if i == 1 and tool != "graphify":
                    for label in (a, b):
                        try:
                            limits.extend({"label": label, **f}
                                          for f in flags(json.loads(read(label))))
                        except json.JSONDecodeError:
                            pass
            consistent = bool(samples) and all(
                s["returned"] == samples[0]["returned"]
                and s["exact_definition"] == samples[0]["exact_definition"]
                for s in samples)
            answers.append({"tool": tool, "target": target,
                            "language": LANGUAGE[target],
                            "expected": sorted(EXPECTED[target]),
                            "scored_samples": len(samples), "consistent": consistent,
                            "samples": samples})

        for i in (1, 2, 3):
            label = "validate-edit-%s-%d" % (tool, i)
            name = "devmap_competition_revision_%d" % i
            try:
                passed = records[label]["measurement_ok"] and found(
                    tool, read(label), name, EDIT_FILE)
            except Exception as exc:
                passed = None
                unparsed.append({"tool": tool, "label": label, "error": str(exc)})
            controls.append({"tool": tool, "label": label, "expected": "present",
                             "passed": passed})

        for name in ("devmap_competition_revision_3", NEVER_EXISTED):
            label = "negative-%s-%s" % (tool, name)
            try:
                absent = not found(tool, read(label), name, EDIT_FILE)
                passed = records[label]["measurement_ok"] and absent
            except Exception as exc:
                passed = None
                unparsed.append({"tool": tool, "label": label, "error": str(exc)})
            controls.append({"tool": tool, "label": label, "expected": "absent",
                             "passed": passed})

    # GitNexus stale-index recovery, if the follow-up ran.
    recovery = {}
    for label in ("gitnexus-repeat-stale-query", "gitnexus-after-force-query"):
        if (OUT / (label + ".stdout")).exists():
            try:
                recovery[label] = found("gitnexus", read(label),
                                        "devmap_competition_revision_3", EDIT_FILE)
            except Exception as exc:
                recovery[label] = "unparsed: %s" % exc

    latency = {}
    for tool in TOOLS:
        latency[tool] = {}
        for op in ("search", "callers"):
            for target in PATHS:
                vals = [records["%s-%s-%s-%d" % (op, tool, target, i)]["seconds"]
                        for i in range(1, REPS + 1)
                        if records["%s-%s-%s-%d" % (op, tool, target, i)]["measurement_ok"]]
                if vals:
                    latency[tool]["%s/%s" % (op, LANGUAGE[target])] = {
                        "median": statistics.median(vals), "min": min(vals),
                        "max": max(vals), "n": len(vals)}
    rg = {}
    for target in PATHS:
        vals = [records["text-ripgrep-%s-%d" % (target, i)]["seconds"]
                for i in range(1, REPS + 1)
                if "text-ripgrep-%s-%d" % (target, i) in records]
        if vals:
            rg[LANGUAGE[target]] = {"median": statistics.median(vals), "n": len(vals)}

    doc = {"answers": answers, "controls": controls, "coverage_flags": limits,
           "unparsed": unparsed, "gitnexus_recovery": recovery,
           "query_latency_seconds": latency, "ripgrep_seconds": rg}
    (OUT / "semantics-audit.json").write_text(json.dumps(doc, indent=1) + "\n")

    print("=== definitions and callers (default responses) ===")
    print("%-11s %14s %10s %14s %12s" % ("tool", "exact defs", "callers",
                                         "edits visible", "deleted gone"))
    for tool in TOOLS:
        mine = [a for a in answers if a["tool"] == tool]
        defs_ok = sum(1 for a in mine for s in a["samples"] if s["exact_definition"])
        defs_n = sum(len(a["samples"]) for a in mine)
        pairs = set()
        expected_total = 0
        for a in mine:
            expected_total += len(a["expected"])
            if a["samples"]:
                pairs |= set(a["samples"][0]["matched"])
        edits = [c for c in controls if c["tool"] == tool and c["expected"] == "present"]
        neg = [c for c in controls if c["tool"] == tool and c["expected"] == "absent"]
        print("%-11s %9d/%-4d %6d/%-3d %11d/%-2d %9d/%-2d" % (
            tool, defs_ok, defs_n, len(pairs), expected_total,
            sum(1 for c in edits if c["passed"]), len(edits),
            sum(1 for c in neg if c["passed"]), len(neg)))

    print("\n=== missing caller pairs ===")
    for tool in TOOLS:
        miss = sorted({m for a in answers if a["tool"] == tool
                       for s in a["samples"][:1] for m in s["missing"]})
        print("  %-11s %s" % (tool, "; ".join(miss) if miss else "none"))

    print("\n=== query latency, median ms (5 reps) ===")
    cols = ["search/Rust", "search/Go", "search/Python",
            "callers/Rust", "callers/Go", "callers/Python"]
    print("%-11s %s" % ("tool", " ".join("%13s" % c for c in cols)))
    for tool in TOOLS:
        cells = []
        for c in cols:
            v = latency[tool].get(c)
            cells.append("%13s" % ("%.1f" % (v["median"] * 1000) if v else "-"))
        print("%-11s %s" % (tool, " ".join(cells)))
    print("ripgrep literal: %s" % ", ".join(
        "%s %.1f ms" % (k, v["median"] * 1000) for k, v in rg.items()))

    if unparsed:
        print("\nUNPARSED RESPONSES (%d) -- not scored as pass or fail:" % len(unparsed))
        for u in unparsed[:12]:
            print("  ", u)
    if recovery:
        print("\nGitNexus deleted probe after ordinary refresh: %s; after forced rebuild: %s"
              % (recovery.get("gitnexus-repeat-stale-query"),
                 recovery.get("gitnexus-after-force-query")))
    print("\nwrote semantics-audit.json")


if __name__ == "__main__":
    main()

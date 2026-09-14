#!/usr/bin/env python3
"""The harder staleness case: revert a file to content the tool already indexed.

`semantics.py` removes one of three appended probes, leaving the file with
content the tool has never seen. That is the easy case -- any change-detector
notices it. The build-48cd3c7 run instead restored the file to its ORIGINAL
bytes, and found GitNexus still answering with the deleted symbol.

That difference matters: a tool keyed on content hash or mtime can conclude
"identical to the cold generation, nothing to do" and skip re-analysis, leaving
the symbols it learned from the edited generation behind. This script runs that
exact sequence per tool so the two reports are comparing the same scenario:

    cold (already done by semantics.py)
      -> append probe, refresh        (probe must now be present)
      -> restore ORIGINAL bytes, refresh
      -> query probe                  (a correct index answers "absent")
      -> refresh again, query again   (a second chance to notice)

A tool that answers "present" at either query is serving a symbol that no longer
exists in the source.
"""

import json
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent
sys.path.insert(0, str(OUT))
import semantics as S  # noqa: E402
import audit_semantics as A  # noqa: E402

PROBE = "devmap_competition_revert_probe"


def run(tool):
    corpus = S.corpus_for(tool)
    if not corpus.exists():
        raise SystemExit("%s: run semantics.py %s first" % (tool, tool))
    target = corpus / S.EDIT_FILE
    original = target.read_bytes()
    outcome = {"tool": tool}
    try:
        target.write_bytes(original + S.probe_source(PROBE).encode())
        S.record("revert-%s-add" % tool, S.index_argv(tool, corpus, cold=False), corpus)
        S.record("revert-%s-present" % tool,
                 S.search_argv(tool, corpus, PROBE, S.EDIT_FILE), corpus)

        target.write_bytes(original)  # byte-identical to the cold generation
        S.record("revert-%s-refresh1" % tool,
                 S.index_argv(tool, corpus, cold=False), corpus)
        S.record("revert-%s-query1" % tool,
                 S.search_argv(tool, corpus, PROBE, S.EDIT_FILE), corpus)
        S.record("revert-%s-refresh2" % tool,
                 S.index_argv(tool, corpus, cold=False), corpus)
        S.record("revert-%s-query2" % tool,
                 S.search_argv(tool, corpus, PROBE, S.EDIT_FILE), corpus)

        for key, label in (("present_after_add", "revert-%s-present" % tool),
                           ("present_after_1_refresh", "revert-%s-query1" % tool),
                           ("present_after_2_refreshes", "revert-%s-query2" % tool)):
            try:
                outcome[key] = A.found(tool, A.read(label), PROBE, S.EDIT_FILE)
            except Exception as exc:
                outcome[key] = "unparsed: %s" % exc
    finally:
        target.write_bytes(original)

    # The add must have worked, or the staleness question was never asked.
    outcome["probe_was_indexed"] = outcome.get("present_after_add") is True
    outcome["stale"] = (outcome["probe_was_indexed"]
                        and (outcome.get("present_after_1_refresh") is True
                             or outcome.get("present_after_2_refreshes") is True))

    # A stale index is only reportable as recoverable if the recovery is measured.
    if outcome["stale"] and tool == "gitnexus":
        S.record("revert-%s-force" % tool,
                 [S.TOOLS_BIN["gitnexus"], "analyze", corpus, "--index-only",
                  "--name", S.PROJECT, "--force"], corpus)
        S.record("revert-%s-query-after-force" % tool,
                 S.search_argv(tool, corpus, PROBE, S.EDIT_FILE), corpus)
        try:
            outcome["present_after_forced_rebuild"] = A.found(
                tool, A.read("revert-%s-query-after-force" % tool), PROBE, S.EDIT_FILE)
        except Exception as exc:
            outcome["present_after_forced_rebuild"] = "unparsed: %s" % exc
        target.write_bytes(original)
    return outcome


def main():
    tools = S.TOOLS if len(sys.argv) < 2 or sys.argv[1] == "all" else (sys.argv[1],)
    results = []
    for tool in tools:
        r = run(tool)
        results.append(r)
        print("%-11s indexed=%-5s after1=%-5s after2=%-5s  %s" % (
            tool, r["probe_was_indexed"], r.get("present_after_1_refresh"),
            r.get("present_after_2_refreshes"),
            "STALE" if r["stale"] else
            ("inconclusive: probe never indexed" if not r["probe_was_indexed"] else "clean")),
            flush=True)
    (OUT / "staleness.json").write_text(json.dumps(results, indent=1) + "\n")
    print("\nwrote staleness.json")


if __name__ == "__main__":
    main()

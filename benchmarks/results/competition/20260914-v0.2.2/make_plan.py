#!/usr/bin/env python3
"""Write plan.json and tool-paths.json, pinning every input this run depends on.

Adapted from the 20260913-multirepo run. Two things changed and both are
recorded rather than assumed:

  * DevMap is v0.2.2 (build 1d680486), not the v0.2.1 build 9d8e6db the prior
    run measured.
  * The competitor executables were deleted by the 2026-09-13 cleanup pass and
    reinstalled by `install_tools.py`, which verified each one byte-for-byte
    against the digest the build-48cd3c7 run recorded. They are the SAME builds,
    at different paths -- `tool_identities` carries the digests so that claim is
    checkable rather than asserted.

The corpora are fresh snapshots at each repository's current HEAD. Three of the
four moved since 2026-09-13, so competitor timings are NOT a controlled A/B
against that report -- see REPORT.md.
"""

import hashlib
import json
import os
import subprocess
from pathlib import Path

OUT = Path(__file__).resolve().parent
SCRATCH = Path("/Users/bharath/Code/devtools/DevCouncil/.devcouncil/benchmarks/20260914-v0.2.2")
SNAPSHOT_ROOT = Path("/private/tmp/claude-501/-Users-bharath-Code-devtools-DevCouncil/"
                     "8aef7e52-66bf-445f-af67-d8ff23d20183/scratchpad/corpus")

SOURCES = {
    "DevCouncil": "/Users/bharath/Code/devtools/DevCouncil",
    "GitPulse": "/Users/bharath/Code/devtools/GitPulse",
    "DevPrism": "/Users/bharath/Code/devtools/DevPrism",
    "scholarlm": "/Users/bharath/Code/scholarlm",
}

# Same files the 20260913-multirepo run and the map benchmark touched, so the
# edit stage keeps describing the same edit across reports. Verified present at
# the new HEADs before this plan was written.
EDIT_TARGETS = {
    "DevCouncil": "rust/devmap-cli/tests/file_liveness_reaches_every_artifact.rs",
    "DevPrism": "apps/desktop/src/components/settings-toggle-row.tsx",
    "GitPulse": "src-tauri/vendored/devmap-extract/src/langdecl/kotlin.rs",
    "scholarlm": "backend/go_orchestrator/internal/wisdev/temporal_activities.go",
}

TOOLS = dict(json.loads((OUT / "installed-tools.json").read_text()))
TOOLS["devmap"] = str(SCRATCH / "devmap")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def git(repo, *args):
    return subprocess.run(["git", "--git-dir", str(Path(repo) / ".git"), *args],
                          capture_output=True, text=True, check=True).stdout.strip()


def main():
    snapshots, commits = {}, {}
    for repo in SOURCES:
        snap = SNAPSHOT_ROOT / repo
        snapshots[repo] = str(snap)
        commits[repo] = git(snap, "rev-parse", "HEAD")
        target = snap / EDIT_TARGETS[repo]
        if not target.is_file():
            raise SystemExit("%s: edit target %s does not exist in the snapshot"
                             % (repo, EDIT_TARGETS[repo]))

    identities = {}
    for tool, path in TOOLS.items():
        identities[tool] = {
            "path": path,
            "exists": os.path.exists(path),
            "sha256": digest(path) if os.path.exists(path) else None,
        }
    missing = [t for t, i in identities.items() if not i["exists"]]
    if missing:
        raise SystemExit("missing executables: %s" % ", ".join(sorted(missing)))

    plan = {
        "benchmark": "devmap v0.2.2 cross-repository competitor comparison",
        "scratch": str(SCRATCH),
        "devmap_binary": TOOLS["devmap"],
        "devmap_version": subprocess.run([TOOLS["devmap"], "--version"],
                                         capture_output=True, text=True).stdout.strip(),
        "snapshots": snapshots,
        "corpus_commits": commits,
        "edit_targets": EDIT_TARGETS,
        "sources": SOURCES,
        "tool_identities": identities,
        "prior_run": "20260913-multirepo",
        "host": subprocess.run(["sysctl", "-n", "hw.model", "hw.ncpu"],
                               capture_output=True, text=True).stdout.split(),
        "note": "Each tool indexes its own APFS clone of the pinned snapshot. "
                "No working checkout and no live product index is read or written.",
    }
    (OUT / "plan.json").write_text(json.dumps(plan, indent=2) + "\n")
    (OUT / "tool-paths.json").write_text(json.dumps(TOOLS, indent=2) + "\n")

    print(plan["devmap_version"])
    for tool, ident in sorted(identities.items()):
        print("%-10s %s %s" % (tool, "OK " if ident["exists"] else "MISSING",
                               (ident["sha256"] or "")[:16]))
    for repo in sorted(commits):
        print("%-12s %s" % (repo, commits[repo][:12]))


if __name__ == "__main__":
    main()

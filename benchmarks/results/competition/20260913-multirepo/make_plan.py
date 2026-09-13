#!/usr/bin/env python3
"""Write plan.json and tool-paths.json, pinning every input this run depends on."""

import hashlib
import json
import os
import subprocess
from pathlib import Path

OUT = Path(__file__).resolve().parent
SCRATCH = Path("/Users/bharath/Code/devtools/DevCouncil/.devcouncil/benchmarks/20260913-multirepo")
SNAPSHOT_ROOT = Path("/private/tmp/claude-501/-Users-bharath-Code-devtools-DevCouncil/"
                     "372a7af4-05a3-473b-9f5d-9386945f80c7/scratchpad/corpus")
EXPANDED = Path("/Users/bharath/Code/devtools/DevCouncil/.devcouncil/benchmarks/"
                "20260912-expanded/tools")

SOURCES = {
    "DevCouncil": "/Users/bharath/Code/devtools/DevCouncil",
    "GitPulse": "/Users/bharath/Code/devtools/GitPulse",
    "DevPrism": "/Users/bharath/Code/devtools/DevPrism",
    "scholarlm": "/Users/bharath/Code/scholarlm",
}

# Same files the map benchmark touched, so both runs describe the same edit.
EDIT_TARGETS = {
    "DevCouncil": "rust/devmap-cli/tests/file_liveness_reaches_every_artifact.rs",
    "DevPrism": "apps/desktop/src/components/settings-toggle-row.tsx",
    "GitPulse": "src-tauri/vendored/devmap-extract/src/langdecl/kotlin.rs",
    "scholarlm": "backend/go_orchestrator/internal/wisdev/temporal_activities.go",
}

TOOLS = {
    "devmap": str(SCRATCH / "devmap"),
    "codegraph": str(EXPANDED / "codegraph/codegraph-darwin-arm64/bin/codegraph"),
    "cbm": str(EXPANDED / "codebase-memory-mcp/codebase-memory-mcp"),
    "gortex": str(EXPANDED / "gortex/gortex"),
    "graphify": str(EXPANDED / "graphify-venv/bin/graphify"),
    "gitnexus": "/opt/homebrew/bin/gitnexus",
}


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

    identities = {}
    for tool, path in TOOLS.items():
        identities[tool] = {
            "path": path,
            "exists": os.path.exists(path),
            "sha256": digest(path) if os.path.exists(path) else None,
        }

    plan = {
        "benchmark": "devmap cross-repository competitor comparison",
        "scratch": str(SCRATCH),
        "devmap_binary": TOOLS["devmap"],
        "snapshots": snapshots,
        "corpus_commits": commits,
        "edit_targets": EDIT_TARGETS,
        "sources": SOURCES,
        "tool_identities": identities,
        "host": subprocess.run(["sysctl", "-n", "hw.model", "hw.ncpu"],
                               capture_output=True, text=True).stdout.split(),
        "note": "Each tool indexes its own APFS clone of the pinned snapshot. "
                "No working checkout and no live product index is read or written.",
    }
    (OUT / "plan.json").write_text(json.dumps(plan, indent=2) + "\n")
    (OUT / "tool-paths.json").write_text(json.dumps(TOOLS, indent=2) + "\n")

    for tool, ident in identities.items():
        print("%-10s %s %s" % (tool, "OK " if ident["exists"] else "MISSING",
                               (ident["sha256"] or "")[:16]))
    for repo in sorted(commits):
        print("%-12s %s" % (repo, commits[repo][:12]))


if __name__ == "__main__":
    main()

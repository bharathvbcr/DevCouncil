# Native cutover follow-ups

Open work left after the Python → Go/Rust cut (Phase 7, 2026-09-10) and the
2026-09-11 Python-vs-native audits. Retirement **decisions** stay in
[PHASE7_LONG_TAIL.md](PHASE7_LONG_TAIL.md). This file is the **todo**.

Canonical rows live in `.devcouncil/state.sqlite` (`REQ-P7-*`, `TASK-P7-*`,
`GAP-P7-*`).

`dcstore ready` (2026-09-11): `TASK-P7-1`, `TASK-P7-2`, `TASK-P7-7`,
`TASK-P7-8`, `TASK-P7-9`, `TASK-P7-11`.

Do not reopen `TASK-RP-1` … `TASK-RP-6` (DevMap Phase-1 port; all `done`).

---

## Ready

| ID | Work | Gaps |
|----|------|------|
| **TASK-P7-1** | Wire Go `devcouncil verify` / MCP `verify_task` to `dcverify` the way Manvi `runRigor` does (stub / secret / coverage), or keep `RigorApplied` empty and stop claiming those gates. Replace `AllowedNextToolsForVerify` with the eight tools `Registry.Specs()` advertises. | `GAP-P7-DCVERIFY-UNWIRED` (blocking, critical), `GAP-P7-NEXT-TOOLS-DRIFT` |
| **TASK-P7-2** | Honor `--sandbox docker\|nix` or stop advertising it. `DefaultRunCommand` is always `/bin/sh -c` in the project root. | `GAP-P7-SANDBOX-ADVERTISED` (blocking, high) |
| **TASK-P7-7** | Add `--gap-type`, `--description`, `--blocking`, `--severity`, `--file`, `--evidence`, `--recommended-fix` to `dcstore gap-upsert` `KNOWN_FLAGS`. The handler exists; the CLI fatals first. | `GAP-P7-DCSTORE-GAP-UPSERT` (blocking, high) |
| **TASK-P7-8** | Implement remaining `integrate` adapters **or** drop those host names. Honor `--write-gate` **or** remove the flag and the “containment mode” label. Today: Cursor (mcp.json + rule), Claude (`.mcp.json` only), Codex (comment-only toml). Five other names write a stub receipt; `devmap integrate` accepts only `cursor\|claude\|codex`. | `GAP-P7-INTEGRATE-STUB`, `GAP-P7-WRITE-GATE-DISCARDED` (blocking, high) |
| **TASK-P7-9** | Rewrite packaged `devcouncil.md` / hero-loop / verification skills and policy `NoTaskAllowedCommands` to live Go MCP, Manvi, and DevMap commands. `dev checkout` / `dev status` / slash `/devcouncil:*` are unknown (exit 2). | `GAP-P7-SKILLS-PYTHON-TOOLS`, `GAP-P7-POLICY-CLI-DRIFT` |
| **TASK-P7-11** | Add a product command that inserts tasks / requirements, or document that sqlite plant is the only path. `dcstore` has `task` / `ready` / `list`, not create. | `GAP-P7-NO-TASK-CREATE` (blocking, high) |

## Done (docs honesty)

| ID | Closed |
|----|--------|
| TASK-P7-3 | Product docs no longer claim Go invoke `dcverify` or isolate docker/nix |
| TASK-P7-4 | PHASE7 names GitHub Checks, SCA, claims, executors |
| TASK-P7-5 | `docs/model-routing.md` points at Manvi |
| TASK-P7-6 | Host MCP eight-tool surface vs Manvi vs DevMap |
| TASK-P7-10 | Second-pass stale product docs (2026-09-11): code-graph CLI, corpus, integrate hosts, hero-loop, `--write-gate`, `devmap mcp` |

## Requirements

| ID | Priority | Means |
|----|----------|--------|
| REQ-P7-1 | critical | Host verify must not claim `dcverify` rigor it does not run. Sandbox must isolate or not be advertised. `gap-upsert` must accept the flags its handler reads. |
| REQ-P7-2 | high | Phase 7 retirement ledger names every deleted product surface. |
| REQ-P7-3 | high | Advertised integrate hosts, `--write-gate`, packaged skills, policy allowlist, and task create match the live binaries. |

## Retired capabilities (named, not scheduled)

These have no DevCouncil successor. They are gaps for the ledger, not ready tasks.

| Gap | Notes |
|-----|--------|
| `GAP-P7-SCA-LOST` | GitPulse Insights Health runs `pip-audit` / `npm audit` / `cargo-audit` / `govulncheck` on open — related job, not a verify gate. |
| `GAP-P7-GITHUB-LOST` | GitPulse checks out PRs and reads Dependabot / code scanning; it does not post a Checks API run from `devcouncil verify`. |
| `GAP-P7-CLAIMS-LOST` | Transcript claim lie-detector. |
| `GAP-P7-DAP-LOST` / `GAP-P7-LSP-CUT` / `GAP-P7-GEPA-LOST` / `GAP-P7-OKF-LOST` / `GAP-P7-EXECUTORS-LOST` | Explicit retire. |
| `GAP-P7-FAISS-WEAKER` | `devmap search --semantic` is lexical name similarity. |
| `GAP-P7-MCP-HOST-THIN` | Host MCP is 8 tools. Thick successor is **Manvi**, not `devcouncil mcp`. |
| `GAP-P7-DEVMAP-MCP-CLI` | `cypher` / `pdg` / `route_map` are CLI-only. |

## Historical docs

Port ledgers, dated audits, and qualification dumps live under
[archive/](archive/README.md). Live DevMap pages: [devmap/README.md](devmap/README.md),
[devmap/DIVERGENCES.md](devmap/DIVERGENCES.md), [devmap/HOST_INTEGRATION.md](devmap/HOST_INTEGRATION.md),
[devmap/AGENTIC_WORKTREE_DESIGN.md](devmap/AGENTIC_WORKTREE_DESIGN.md).

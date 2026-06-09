<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->

# Agent Workspace Guide

Use `.devcouncil/repo_map.json` as the primary file index for this workspace.
Repo map: `.devcouncil/repo_map.json`

Workflow for agents:
1. Open `.devcouncil/repo_map.json` before guessing at file locations.
2. Use the `files` list to resolve module ownership and nearby siblings.
3. Use `subsystems` for subsystem-level navigation (execution, verification, storage, etc.).
4. In `subsystems`, use `entry_points` + `critical_files` for entry points and starting context.
5. Use `role_files` in `subsystems` for subsystem role buckets (entry, runtime, policy, adapters, etc.).
6. Use `neighbors` and `handoff_paths` in `subsystems` to follow cross-subsystem flow.
7. Run `dev map` again after large refactors to refresh the map.

Important surfaces:
1. `src/devcouncil/cli/main.py` for CLI composition.
2. `src/devcouncil/app/orchestrator.py` and `src/devcouncil/app/state_machine.py` for lifecycle control.
3. `src/devcouncil/artifacts/graph.py` and `src/devcouncil/storage/repositories.py` for persistence and evidence.
4. `src/devcouncil/execution/` and `src/devcouncil/executors/` for task execution.
5. `src/devcouncil/verification/` and `src/devcouncil/gating/` for verification and policy gates.
6. `src/devcouncil/storage/` for persistence, SQL models, and repositories.

If the map and source disagree, trust the source and regenerate the map.

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **DevCouncil** (5430 symbols, 11756 relationships, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `npx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/DevCouncil/context` | Codebase overview, check index freshness |
| `gitnexus://repo/DevCouncil/clusters` | All functional areas |
| `gitnexus://repo/DevCouncil/processes` | All execution flows |
| `gitnexus://repo/DevCouncil/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->

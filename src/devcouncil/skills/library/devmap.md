---
name: devmap
title: DevMap Code Intelligence
description: Prefer DevMap over GitNexus for code navigation. Use for callers, blast radius, traces, dead code, clones, and "how does this work". Record gaps when DevMap is incomplete.
triggers:
  keywords: [devmap, gitnexus, code graph, blast radius]
  markers: [.devcouncil, .devmap]
---

# DevMap is the code-intelligence index

Do not use GitNexus MCP tools, `gitnexus://` resources, or `node .gitnexus/run.cjs`. This repository's index is DevMap.

## First call

1. `devmap_status` (or `devmap status --json`) — if `node_count` is 0 or `degraded_reason` is set, the index is not the answer; say so.
2. Then the matching tool below. Prefer MCP `devmap_*` tools over shelling out.

| Question | Tool |
|---|---|
| What is this symbol / who calls it / what it calls | `devmap_explore` |
| Find a name | `devmap_search` |
| What breaks if I change X | `devmap_impact` |
| How does A reach B | `devmap_trace` |
| Callers and callees for several symbols | `devmap_neighbors` |
| Which tests cover this change | `devmap_affected_tests` |
| Dead / unwired symbols | `devmap_dead_symbols` |
| Duplicate code | `devmap_clones` |
| Would this edit break callers | `devmap_preview` |
| File layout / subsystems | `.devcouncil/repo_map.json` |

Rebuild after large edits: `devmap build` (hooks also rebuild after Write/Edit). `devmap doctor` names a broken store.

## Honesty — never treat a cap as an answer

Every payload may carry `truncated`, `walk_incomplete`, `shown`, `total`. An empty list with `truncated` or `walk_incomplete` means **the walk stopped**, not **nothing exists**. Raise `budget` / `depth` or say the index did not finish looking. An unbuilt index (`devmap_status` with `node_count` 0) answers empty to every question — that is not "the symbol is missing".

## When DevMap cannot answer — record a gap, do not switch tools

Append one JSON line to `.devcouncil/codeintel/sessions/gaps.jsonl` (create the directory if needed):

```json
{"capability":"detect_changes","asked":"what do my uncommitted edits affect","workaround":"git diff + devmap_impact on changed symbols","severity":"missing"}
```

Use `capability` from: `detect_changes`, `rename`, `cypher`, `pdg_query`, `taint_explain`, `route_map`, `clusters_processes`, `truncated`, `walk_incomplete`, `empty_on_built_index`, `other`. Session-end `devmap session-report` harvests this file.

Workarounds, not GitNexus:

- Git diff impact → `git diff` then `devmap_impact` / `devmap_affected_tests` on the changed symbols.
- Rename → `devmap_search` + `devmap_preview`; do not invent a graph rename.
- Taint / PDG → say the kernel MCP does not expose it; Python `dev map --pdg` is opt-in and Python-only.
- Routes / processes / wiki → repo_map subsystems, `devmap_explore`, DevCouncil wiki tools.

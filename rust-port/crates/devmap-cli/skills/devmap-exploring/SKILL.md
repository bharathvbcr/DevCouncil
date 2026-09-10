---
name: devmap-exploring
description: Use when the user asks how code works, wants architecture, callers, or an execution flow. Examples: "How does X work?", "What calls this function?", "Show me the auth flow"
---

# Exploring with DevMap

1. `devmap_status` — refuse to interpret empty answers if the index is unbuilt.
2. `devmap_explore` on the symbol or concept. One call: definitions, callers, callees, blast radius.
3. `devmap_trace` when the question is "how does A reach B?".
4. Read the source files the tool named. The graph is a map, not the code.

If `truncated` or `walk_incomplete` is set, raise `budget`/`depth` or say the neighbourhood is partial. Do not call GitNexus `query` / `context` / `gitnexus://` resources.

Layout questions (which folder owns this): `.devcouncil/repo_map.json` subsystems, `entry_points`, `critical_files`.

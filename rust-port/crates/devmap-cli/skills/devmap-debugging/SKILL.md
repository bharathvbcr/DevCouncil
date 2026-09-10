---
name: devmap-debugging
description: Use when debugging a bug, tracing an error, or asking why something fails. Examples: "Why is X failing?", "Where does this error come from?", "Trace this bug"
---

# Debugging with DevMap

1. `devmap_search` / `devmap_explore` on the error text or the suspect symbol.
2. Callers (`explore` / `neighbors`) — who can reach the throw / the bad return.
3. `devmap_trace` from an entry point to the suspect when the chain is the question.
4. Read the source. Confirm the cause in the file, not only in the graph.

No GitNexus `query` / `context` / `cypher` / `detect_changes`. Recent-regression workaround: `git log` / `git diff` plus `devmap_impact` on the changed symbols. If the index cannot see a dynamic/reflected call, say `walk_incomplete` and record a gap.

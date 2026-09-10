---
name: devmap-refactoring
description: Use when renaming, extracting, splitting, moving, or restructuring code. Examples: "Rename this function", "Extract this into a module", "Move this to a separate file"
---

# Refactoring with DevMap

1. `devmap_explore` / `devmap_impact` on the symbol being moved.
2. `devmap_search` for every name that must change with it.
3. `devmap_preview` on the edited file before write — callers the edit would break.
4. After the edit, `devmap_affected_tests` and run those tests.

There is no graph-coordinated `rename` tool. Do not invent one and do not use GitNexus `rename`. Search + preview + tests is the path; record a `rename` gap if a multi-file graph rename is what was needed.

Check `unwired_candidates` in `repo_map.json` before creating a new module — wire what you add to a real caller.

---
name: devmap-impact
title: Impact analysis with DevMap
description: Use when the user wants to know what will break if they change something, or needs safety analysis before editing. Examples: "Is it safe to change X?", "What depends on this?", "What will break?"
triggers:
  keywords: [blast radius, impact analysis, devmap impact]
  markers: [.devcouncil, .devmap]
---

# Impact analysis with DevMap

1. `devmap_status` — a missing index is not "nothing depends on this".
2. `devmap_impact` on the symbol or file. Read `walk_incomplete` before calling the blast radius complete.
3. `devmap_affected_tests` for the tests to run.
4. `devmap_preview` with the proposed file contents before writing a risky edit.

Depth 1 edges are direct callers (will break). Deeper layers are transitive. A result with `truncated: true` is a partial blast radius.

There is no `detect_changes`. Workaround: map `git diff` paths to symbols, then `devmap_impact` / `devmap_affected_tests`. Record a `detect_changes` gap if that workaround is what you actually needed.

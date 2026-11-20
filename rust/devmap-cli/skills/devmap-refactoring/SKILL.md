---
name: devmap-refactoring
description: Use DevMap to plan and verify renames, extractions, moves, splits, and
  structural code changes.
---

# Refactoring with DevMap

Resolve this checkout with `devmap paths --json`, then check `devmap status --json`. Read the returned `repo_map` path; do not assume `.devcouncil` rather than `.devmap`. A missing, stale, or partially parsed index is not complete evidence. Rebuild with `devmap build --manifest` when needed and authorized, then recheck status.

Lead with the CLI: `devmap <cmd> … --json --root <absolute repo path>` works in any host and reads this checkout's own store. The `devmap_*` MCP tools named below are the alternative when the MCP server is connected and targets this checkout; on those calls (and `gitpulse_codeintel_*`) always pass `repo_path` (the absolute repository path) and check `repository.root` in the envelope before trusting the answer — Cursor shares one `devmap mcp` process across tabs. A host need not expose every CLI capability or MCP tool; check `devmap --help` and the tool schemas.

1. Explore and assess impact with `devmap explore <name> --json` and `devmap impact <target> --json`.
2. Find all references using symbol search and direct source inspection, including registries, generated bindings, and dynamic calls the graph may miss.
3. Use the installed preview interface where supported. Do not invent a graph-coordinated rename command; use language tooling or a suitable available fallback when required.
4. Inspect the resolved map's unwired candidates before adding a module, and verify its real callers after the edit.
5. Review the complete diff and run candidate tests from `devmap affected <target> --json` plus the repository's required checks.

Read `shown`, `total`, `truncated`, and `walk_incomplete`; say which references remain unverified. Prefer DevMap where it answers the question and explain any fallback. Explicit user and repository requirements take precedence.

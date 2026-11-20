---
name: devmap-exploring
description: Use DevMap to explain code structure, architecture, callers, and execution
  paths.
---

# Exploring with DevMap

Resolve this checkout with `devmap paths --json`, then check `devmap status --json`. Read the returned `repo_map` path; do not assume `.devcouncil` rather than `.devmap`. A missing, stale, or partially parsed index is not complete evidence. Rebuild with `devmap build --manifest` when needed and authorized, then recheck status.

Lead with the CLI: `devmap <cmd> … --json --root <absolute repo path>` works in any host and reads this checkout's own store. The `devmap_*` MCP tools named below are the alternative when the MCP server is connected and targets this checkout; on those calls (and `gitpulse_codeintel_*`) always pass `repo_path` (the absolute repository path) and check `repository.root` in the envelope before trusting the answer — Cursor shares one `devmap mcp` process across tabs. A host need not expose every CLI capability or MCP tool; check `devmap --help` and the tool schemas.

1. When you know the behaviour but not the name, run `devmap ask --evidence "<question>" --json` (MCP `devmap_ask_evidence`): ranked files with their source, the call edges between the hits, and the tests that reach them. Read its source before opening the same files again.
2. Run `devmap explore <name> --json` for definitions, callers, callees, and blast radius.
3. Use `devmap trace <from> <to> --json` for a specific entry-to-symbol path.
4. Use the resolved map's subsystems, entry points, and critical files for ownership.
5. Confirm the explanation against source and call sites. Graph paths describe static relationships, not proof that a path executed.

Read `shown`, `total`, `truncated`, and `walk_incomplete`. Increase query bounds only within supported limits. When DevMap cannot answer, explain the limitation and use source inspection or a suitable available fallback, subject to user and repository instructions.

---
name: devmap-impact
description: Use DevMap before changing code to identify callers, dependencies, candidate
  tests, and the limits of blast-radius evidence.
---

# Impact analysis with DevMap

Resolve this checkout with `devmap paths --json`, then check `devmap status --json`. Read the returned `repo_map` path; do not assume `.devcouncil` rather than `.devmap`. A missing, stale, or partially parsed index is not complete evidence. Rebuild with `devmap build --manifest` when needed and authorized, then recheck status.

Lead with the CLI: `devmap <cmd> … --json --root <absolute repo path>` works in any host and reads this checkout's own store. The `devmap_*` MCP tools named below are the alternative when the MCP server is connected and targets this checkout; on those calls (and `waylode_codeintel_*` (or the old `gitpulse_codeintel_*`)) always pass `repo_path` (the absolute repository path) and check `repository.root` in the envelope before trusting the answer — Cursor shares one `devmap mcp` process across tabs. A host need not expose every CLI capability or MCP tool; check `devmap --help` and the tool schemas.

1. Run `devmap impact <target> --json` on the exact symbol or file.
2. Inspect direct callers first, then transitive paths. Explain what may change; a dependency does not by itself prove a break.
3. Run `devmap affected <target> --json` to locate candidate tests and inspect their assertions. These are graph-reachable tests, not a coverage guarantee.
4. Use `devmap preview --help` or the exposed preview schema to check proposed source when useful.
5. Review `git diff` against the intended scope and run relevant tests plus required repository checks.

Read `shown`, `total`, `truncated`, and `walk_incomplete`. Never turn an incomplete or unavailable answer into a zero-risk verdict. For capabilities not exposed by this host, state the gap and use source inspection or an available fallback; follow user and repository requirements.

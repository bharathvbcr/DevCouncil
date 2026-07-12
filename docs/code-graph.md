# Repository Map & Code Graph

DevCouncil builds a deterministic repository map and a symbol-level knowledge graph without calling an LLM. Agents use these artifacts to navigate, find callers, and spot unwired or dead code before inventing new modules.

## Artifacts

| Path | Role |
| :--- | :--- |
| `.devcouncil/repo_map.json` | File inventory, subsystems, entry roots, unwired/unreachable/dead-symbol candidate lists, reverse-import dependents |
| `.devcouncil/graph/code_graph.json` | Symbol nodes + edges (imports, named imports, calls, inherits, contains) and tiered `dead_code` |
| `.devcouncil/graph/graph.html` | Self-contained interactive visualizer (`dev graph html`; **not** written by default on `dev map`) |
| `AGENTS.md` / `CLAUDE.md` | Marker-guarded workspace guides kept in sync with the map |

## Build / refresh

```bash
dev map                     # Full rebuild (liveness on by default)
dev map --if-stale          # No-op when fingerprints still match
dev map --no-liveness       # Skip entry/unwired/unreachable/dead lists
dev map --lsp-refs          # Confirm dead symbols via live LSP references
dev map --wiki / --no-wiki  # Refresh codebase-wiki skeletons after map (on by default)
dev map --scan-deps         # Opt-in SCA (pip-audit / npm audit / osv-scanner) → dependency_risks
dev map --watch             # Debounced incremental refresh on edits
```

Freshness uses git HEAD, a tracked-file hash, and a content fingerprint so plain edits mark the map stale. Fingerprint / git errors fail closed (treat as stale). Post-tool-use hooks and `dev map --watch` refresh incrementally; incremental extract still verifies parse-cache sha256 so a concurrent edit to an unlisted path cannot stamp a fresh fingerprint over stale symbols.

HTML visualizer: set `indexing.write_graph_html: true` in config if you want `dev map` to also write `graph.html`. Otherwise use `dev graph html` / `dev graph view` explicitly.

## Query the graph

```bash
dev graph query build_code_graph   # definition + callers/callees/importers
dev graph trace path/a.py path/b.py
dev graph dead                     # full dead-code report (uncapped)
dev graph dead --min-confidence inferred
dev graph check                    # god nodes + circular imports
dev graph process                  # BFS call-flows from entry roots
dev graph impact src/foo.py        # blast radius
dev graph impact --diff            # blast radius for working-tree changes
dev graph html                     # write graph.html
dev graph view                     # serve/open the HTML
dev graph export -o out.graphml    # GraphML (or --format okf)
```

## Dead-code confidence tiers

| Tier | Meaning |
| :--- | :--- |
| `extracted` | No inbound call/import edges in the resolved graph |
| `inferred` | Only callers are themselves dead (transitive island) |
| `ambiguous` | Graph-dead but token-scan or name-only refs suggest a possible false positive |

`dev map` stores a **capped** (200) `dead_symbol_candidates` list for agents; `dev graph dead` reports the **uncapped** graph tiers with reasons. Prefer reviewing `ambiguous` before deleting anything. If graph assemble fails, the map omits dead-symbol candidates rather than falling back to a token-only flood.

## Liveness lists on the map

- **entry_roots** — configured + convention entry points (CLI mains, `__main__`, etc.)
- **unwired_candidates** — code files with no non-test importers (capped at 200)
- **unreachable_files** — not reachable via imports from production entry roots (capped at 200)
- **dead_symbol_candidates** — legacy intersection of graph-dead ∩ token-scan (capped at 200)

### Map ↔ verify asymmetry (same-task island)

Map unwired lists treat **any** non-test importer as clearing unwired. Verify’s unwired gate for *new* files is stricter: a file imported only by other files added in the same task still fails (same-task island rule) until a **pre-existing** non-test caller imports it. Map lists are navigation hints; verify is the gate.

Verification can also enforce wiring / stale-map / dead-symbol / liveness-ratchet checks when those gates are enabled.

## Agent workflow

1. Open `.devcouncil/repo_map.json` before guessing file locations.
2. Use `subsystems` → `entry_points` / `critical_files` / `role_files` / `neighbors`.
3. Check `unwired_candidates` / `unreachable_files` / `dead_symbol_candidates` before adding modules.
4. Use `dev graph query` / `trace` / `dead` for symbol-level navigation.
5. Run `dev map` after large refactors (or rely on hooks / `--watch`).

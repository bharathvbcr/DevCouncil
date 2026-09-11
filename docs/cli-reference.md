# CLI Command Reference

This is the comprehensive reference for DevCouncil's native binaries: the Go host orchestrator (`devcouncil` / `dev`) and the Rust analysis components (`devmap`, `dcstore`, `dcverify`, `dcgrep`).

**Platforms:** macOS, Linux, Windows.  
**Maturity:** Stable / Preview labels live in [project-status.md](project-status.md).

---

## 1. Go Host Orchestrator (`devcouncil` / `dev`)

The same binary is installed as both `devcouncil` and `dev`.

```bash
Usage:
  devcouncil mcp                                       # Run the MCP stdio server
  devcouncil install [names…] [--list] [--json] [--prefix DIR] [--dry-run]
  devcouncil uninstall [names…] [--yes] [--prefix DIR]
  devcouncil disable NAME [--prefix DIR]
  devcouncil enable NAME [--prefix DIR]
  devcouncil gate status [--json] [--project-root DIR]
  devcouncil gate set --mode off|advisory|enforce
  devcouncil integrate HOST [options]                  # Configure coding agent integrations
  devcouncil integrate uninstall --target hooks        # Uninstall integration hooks
  devcouncil skills list                               # List embedded agent skills
  devcouncil skills scaffold [options]                 # Distribute skills into project
  devcouncil verify TASK_ID [options]                  # Deterministically verify a task diff
  devcouncil map [devmap args…]                        # Exec `devmap` (bare: build --manifest)
  devcouncil graph …                                   # Alias of map
  devcouncil ast …                                     # Exec `devmap ast`
```

First-time / standalone (no host binary yet):

```bash
bash scripts/install.sh --only=devmap
bash scripts/install.sh --help
```

There is no `uv` / Python install path.

### Host MCP Server

```bash
devcouncil mcp
# or: devcouncil mcp-server
```

Runs the Model Context Protocol (MCP) stdio server. Exposes tools for task management, atomic leases, diff inspection, policy write checks, and task verification to connected agents (Claude Code, Cursor, Codex, Antigravity).

### Agent Host Integrations

```bash
devcouncil integrate HOST [--apply|--check|--dry-run] [--project-root DIR] [--write-gate]
```

Supported hosts: `cursor`, `claude`, `codex`, `gemini`, `opencode`, `warp`, `aider`, `antigravity`.

- `--apply`: Write configuration files to the target repository.
- `--check`: Read-only verification that integration files match expected content.
- `--dry-run`: Print planned actions without modifying files.
- `--write-gate`: Install blocking PreToolUse write gates (Claude Code, Cursor).
- `--project-root DIR`: Specify target project directory (defaults to `$DEVCOUNCIL_PROJECT_ROOT` or `pwd`).

```bash
# Example usage:
devcouncil integrate cursor --apply
devcouncil integrate claude --apply --write-gate
devcouncil integrate antigravity --apply
devcouncil integrate claude --check
devcouncil integrate uninstall --target hooks --apply
```

### Agent Skills Distribution

```bash
devcouncil skills list
devcouncil skills scaffold [--skill NAME] [--project-root DIR] [--dry-run] [--check]
```

- `devcouncil skills list`: List packaged domain and code intelligence skills embedded in the binary.
- `devcouncil skills scaffold`: Install skills into `.agents/skills`, `.claude/skills`, and `.cursor/skills`.
- `--skill NAME`: Scaffold an individual skill (e.g. `--skill core-engineering` or `--skill devmap`).
- `--check`: Verify existing skill files match current versions without modifying them.

### Task Verification

```bash
devcouncil verify TASK_ID [--json] [--mode off|advisory|enforce] [--sandbox local|docker|nix] [--project-root DIR]
```

Verifies code changes associated with `TASK_ID` using the deterministic verification engine (`dcverify`).
- Checks planned file scope, diff validity, anti-laziness/stubs, and test execution.
- `--mode`: `off` (default when unset) skips quality verification; `advisory` still blocks hard-safety gaps; `enforce` blocks every `Blocking` gap. Hard-safety write policy is unchanged.
- `--json`: Output machine-readable verification results and typed `next_actions` for agent self-repair.
- `--sandbox local|docker|nix`: Run verification commands in a sandbox container or local environment.

### DevMap Shorthands

```bash
dev map [args...]      # Runs `devmap`. Bare invocation runs: devmap build --manifest
dev graph [args...]    # Alias for `dev map`
dev ast [args...]      # Execs `devmap ast`
```

---

## 2. Code Intelligence Engine (`devmap`)

High-performance multi-language code graph and symbol analysis engine (extract → resolve → analyze → store → query → serve).

```bash
# Build & Lifecycle
devmap build --manifest     # Generate repo_map.json and code_graph.json
devmap build --watch        # Rebuild on filesystem events (debounced)
devmap build --if-stale     # Build only if source files changed since last generation
devmap build --full         # Force a cold rebuild (re-parse all files)
devmap status [--json]      # Report store schema, page stats, generation, freshness
devmap doctor [--fix]       # Check store health, orphan entries, and repair issues
devmap paths [--json]       # Print resolved store, cache, and artifact paths

# Code Exploration & Navigation
devmap query <SYMBOL>       # 360° symbol view: definitions, callers, callees, importers
devmap trace <SRC> <DST>    # Shortest dependency or call path between two graph nodes
devmap impact <PATH...>     # Blast radius & reverse dependents for paths
devmap dead [--json]        # Unreferenced code with confidence tiers (extracted|inferred|ambiguous)
devmap search <QUERY>       # FTS5 symbol and path search over current generation
devmap cypher '<QUERY>'     # Execute supported Cypher queries against the SQLite graph store

# Visualizers & Servers
devmap view                 # Serve and open interactive graph visualizer locally
devmap demo                 # Output standalone demo.html visualizer
devmap map-html             # Generate subsystem map HTML (.devcouncil/map.html)
devmap graph-html           # Generate symbol-level graph HTML (.devcouncil/graph/graph.html)
devmap serve --mcp          # Run standalone DevMap MCP server over stdio
```

---

## 3. Analysis & State Suite (`dcstore`, `dcverify`, `dcgrep`)

### Task & Lease Store (`dcstore`)

Provides atomic mutual-exclusion leases and persistent SQLite task storage:

```bash
dcstore --db <PATH> health
dcstore --db <PATH> acquire --task <ID> --owner <OWNER> --client-id <CLIENT> --ttl-seconds <N>
dcstore --db <PATH> renew --task <ID> --token <TOKEN> --ttl-seconds <N>
dcstore --db <PATH> release --task <ID> --token <TOKEN>
dcstore --db <PATH> status --task <ID>
dcstore --db <PATH> next-task
```

### Deterministic Verifier (`dcverify`)

Diff parser, scope classifier, and rigor gate evaluator:

```bash
dcverify health
dcverify check [--planned <FILE>] [--coverage <LCOV>] [--root <DIR>] < unified.diff
dcverify evidence-check --contract <PATH> --bundle <PATH> [options]
```

### Ripgrep Engine Search (`dcgrep`)

Ignore-aware high-speed repository search with optional trigram index:

```bash
dcgrep health
dcgrep search < request.json
dcgrep files < request.json
dcgrep index < request.json
```

---

## 4. Retired Commands

With Phase 7 and the transition to native Go and Rust binaries, legacy Python CLI commands have been retired. Invoking them via the Go binary exits 2 with `unknown command`:

| Retired Command | Historical Function | Current Architecture / Replacement |
|---|---|---|
| `dev setup` / `dev init` | Python repo setup & doctor | Run `devmap build --manifest` + `devcouncil integrate <host> --apply`. |
| `dev plan` / `dev approve` | Multi-agent LLM debate & tasks | Managed by upstream agent harnesses (e.g. **Manvi**) or interactive prompts. |
| `dev run` / `dev e2e` / `dev go` | Python subprocess coding agent runner | Run agents natively via MCP (**Hero Loop**) or via **Manvi**. |
| `dev prompt` / `dev handoff` | Formatted text prompt generation | Handled over MCP via task checkout contexts and skills. |
| `dev check` / `dev check --verify` | Python LLM audit / demo check | Use `devcouncil verify TASK_ID` or `dcverify check`. |
| `dev wiki` / `dev okf` / `dev design` | Markdown wiki & OKF bundle generator | Replaced by `devmap build --guides` (`AGENTS.md`, `CLAUDE.md`). |
| `dev dashboard` | Textual dashboard UI | Replaced by DevMap visualizers (`devmap view`) and Manvi TUI. |
| `dev cost` / `dev doctor` | Python model pricing ledger & env checks | Health checks via `devmap doctor` and binary health flags. |

For detailed rationale on the retirement decisions, see [PHASE7_LONG_TAIL.md](PHASE7_LONG_TAIL.md).

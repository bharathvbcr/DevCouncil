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

Runs the Model Context Protocol (MCP) stdio server. Eight tools: checkout / renew / release / next_task / get_diff / verify_task / get_gaps / policy_check_write. What `integrate` installs into Cursor/Claude. Filesystem, grep, git, and `dcverify` rigor live on **Manvi**, not here.

### Agent Host Integrations

```bash
devcouncil integrate HOST [--apply|--check|--dry-run] [--project-root DIR] [--write-gate]
```

`Hosts` lists `cursor`, `claude`, `codex`, `gemini`, `opencode`, `warp`, `aider`, `antigravity`. Only **cursor / claude / codex** have adapters (TASK-P7-8). The rest write a stub receipt; `devmap integrate` then refuses those names.

- `--apply`: Write configuration files (or a stub receipt) to the target repository.
- `--check`: Read-only verification that integration files match expected content.
- `--dry-run`: Print planned actions without modifying files.
- `--write-gate`: Parsed and labeled "containment mode", then discarded. Does not write `.cursor/hooks.json` or Claude PreToolUse hooks.
- `--project-root DIR`: Specify target project directory (defaults to `$DEVCOUNCIL_PROJECT_ROOT` or `pwd`).

```bash
# Working adapters today:
devcouncil integrate cursor --apply
devcouncil integrate claude --apply
devcouncil integrate claude --check

# Removes leftover Python-era hook files if any remain. Integrate never writes those files now.
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

Verifies the working-tree diff for `TASK_ID` through Go `verify.Run()`: no-work, planned-file scope, orphan diffs, dependency-risk, and expected-test / allowed-command execution. This command does **not** spawn `dcverify`. Stub, secret, and coverage rigor live in that binary; Manvi `runRigor` is the current caller. `dcverify` remains a separate CLI (`scripts/install-components.sh`).
- `--mode`: `off` (default when unset) skips quality verification; `advisory` still blocks hard-safety gaps; `enforce` blocks every `Blocking` gap. Hard-safety write policy is unchanged.
- `--json`: Output machine-readable verification results and typed `next_actions` for agent self-repair.
- `--sandbox`: Copied onto the report. Only local execution is implemented (`/bin/sh -c` in the project root). `docker` and `nix` are accepted as labels and do **not** isolate (TASK-P7-2). Usage text still lists them; do not treat that as a working sandbox.

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
devmap build --full         # Cold rebuild
devmap serve                # Watcher + IPC (not `build --watch`)
devmap status [--json]
devmap doctor               # No `--fix`
devmap paths [--json]
devmap history
devmap repair

# Code Exploration & Navigation
devmap search <QUERY> [--semantic]
devmap explore <NAME>
devmap trace <SRC> <DST>
devmap impact <TARGET>
devmap dead [--json]
devmap cypher '<QUERY>'
devmap affected <TARGET>
devmap pdg <FILE> [--taint]
devmap ast …

# Visualizers & Servers
devmap map-html             # Subsystem map (.devcouncil/map.html)
devmap html                 # Symbol graph HTML
devmap mcp                  # DevMap MCP stdio (not `serve --mcp`)
devmap export               # GraphML
```

Unknown: `query`, `view`, `demo`, `graph-html`, `doctor --fix`, `build --watch`, `build --if-stale`.

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
| `dev status` / `dev checkout` / `dev tasks` / `dev doctor` | Python orientation and lease bootstrap | Go unknown command (exit 2). Policy `NoTaskAllowedCommands` still allowlists them (TASK-P7-9). Use `devcouncil mcp` tools / `devmap status` / `devmap doctor`. |
| `dev check` / `dev check --verify` | Python LLM audit / demo check | Use `devcouncil verify TASK_ID` or `dcverify`. |
| `dev wiki` / `dev okf` / `dev design` | Markdown wiki & OKF bundle generator | Replaced by `devmap build --guides` (`AGENTS.md`, `CLAUDE.md`). |
| `dev dashboard` | Textual dashboard UI | Replaced by DevMap visualizers (`devmap view`) and Manvi TUI. |
| `dev cost` / `dev doctor` | Python model pricing ledger & env checks | Health checks via `devmap doctor` and binary health flags. |

For detailed rationale on the retirement decisions, see [PHASE7_LONG_TAIL.md](PHASE7_LONG_TAIL.md). Open follow-ups: [TODO.md](TODO.md).

# CLI Command Reference

[Documentation index](README.md) · [Quickstart](quickstart.md)

Task verification defaults to `off`; for an existing task use explicit
`--mode enforce` when a strict verdict is required. Read skipped reasons and
coverage metadata. Installed `devmap COMMAND --help` and `devcouncil --help` are flag authorities.
The JSON analysis components do not expose conventional `--help`; their native
command parsers and [Rust guide](../rust/README.md) document those contracts.

This is the comprehensive reference for DevCouncil's native binaries: the Go host orchestrator (`devcouncil` / `dev`) and the Rust analysis components (`devmap`, `dcstore`, `dcverify`, `dcgrep`).

**Platforms:** macOS, Linux, Windows.  
**Maturity:** Stable / Preview labels live in [project-status.md](project-status.md).

## Interactive command output

Finite `devmap` commands and the Go host's installation, component, skills,
integration, verification, and gate commands share an immediate loader, an
80 ms animation cadence, and a completion summary. Labels describe the current
operation; an orbit means work is active, not a fabricated percentage or ETA.
DevMap build retains its measured five-stage trail and file counters.

- `--progress auto` is the default: animate interactive stderr, with decorated
  result details only when stdout is also a terminal.
- `--progress always` also emits plain activity lines in redirected logs. With
  `--json`, progress stays on stderr and stdout remains a single JSON document.
- `--progress never` suppresses decoration and activity, retaining diagnostics.
- `NO_COLOR=1` removes color. `TERM=dumb` uses plain output. Non-UTF-8 locales
  use ASCII. Windows uses the plain fallback; cross-compilation is not native
  terminal qualification.
- `devcouncil map`, `graph`, and `ast` forward presentation flags to DevMap.
  MCP, hook payloads, raw GraphML, and generated configuration retain their
  protocol format. The JSON-only components never animate.

Optional output has bounded queues and shutdown waits. A full or disconnected
stderr must not block an independent primary result stream. Primary stdout
retains normal backpressure. Errors writing a result fail the command; the
three JSON components return exit 1 for transport failure without a panic.

`devcouncil install devmap --dry-run --json` emits the planned command receipt
without executing it. `devmap repair --fts --pending --page-size --json`
returns one combined receipt; `--schema` remains exclusive. Raw GraphML
(`devmap export --out -`) cannot also request `--json`.

See [CLI presentation audit](CLI_PRESENTATION_AUDIT.md) for reproductions,
verification commands, measured coverage, and platform limits.

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

Runs the Model Context Protocol (MCP) stdio server. Eight tools: checkout / renew / release / next_task / get_diff / verify_task / get_gaps / policy_check_write. What `integrate` installs into Cursor/Claude. Arbitrary filesystem/patch/shell tools are not exposed by this host. Its verification gateway does invoke `dcverify` for rigor; see [the task loop contract](hero-loop.md).

### Agent Host Integrations

```bash
devcouncil integrate HOST [--apply|--check|--dry-run] [--project-root DIR]
```

`Hosts` lists `cursor`, `claude`, `codex`, `opencode`, `warp` and
`antigravity`, matching DevMap's accepted names. Outputs differ by host. The
Go Codex adapter remains comment-only; the Rust integrator installs DevMap's
user-level Codex MCP registration. Other adapters merge their owned entries
into host documents. See the [integration matrix](coding-cli-integration.md)
for the separate Go/Rust surfaces, hooks and user-level changes. Legacy
`gemini` and `aider` targets are refused by these adapters.

- `--apply`: Write configuration files to the target repository.
- `--check`: Read-only verification that integration files match expected content.
- `--dry-run`: Print planned actions without modifying files.
- `--write-gate`: **Removed.** It named a pre-tool-use gate that only DevCouncil's retired lifecycle hooks installed; nothing enforced it. Passing it exits 2 with that explanation rather than "unknown flag". `execution.hook_gate.mode` and its `contain` mode are gone for the same reason, along with `gate set --hook`. A copy left in an existing `config.yaml` is inert. Use `gates.mode` for verification and the `devcouncil_*` MCP policy tools for write scope.
- `--project-root DIR`: Specify target project directory (defaults to `$DEVCOUNCIL_PROJECT_ROOT` or `pwd`).

```bash
# Working adapters today:
devcouncil integrate cursor --apply
devcouncil integrate claude --apply
devcouncil integrate claude --check

# Removes only recognized legacy registrations, with backups. Other tools are preserved.
devcouncil integrate uninstall --target hooks --apply
```

### Retired hook compatibility and cleanup

```bash
dev hook status --project-root /path/to/project --client claude
dev hook disable --project-root /path/to/project --client claude --dry-run
dev disable hooks --project-root /path/to/project --client claude
```

Legacy `dev hook EVENT` calls are silent no-ops: no stdin reads, policy decisions,
project access, index builds, or child processes. This also covers cached commands
and unknown future event arguments. This behavior does not claim verification ran.

`status` and `integrate uninstall --target hooks --check` are read-only and exit 1
when registrations remain or inspection fails. Read the receipt to distinguish
those states. `--dry-run` returns a preview without creating backups or locks;
`disable` applies cleanup. Conflicting modes are refused. The `--client` filter
accepts Claude, Cursor, Codex, Gemini, Grok, OpenCode, or `all` (the default).
Use a home directory as the explicit root to inspect user settings separately.

Cleanup scans eight known paths in the selected root. It preserves unrelated
hooks, numbers, permissions, MCP registrations and OpenCode plugins. All files are
validated before applying changes; malformed, oversized, duplicate-key, symlinked
or nonregular files cause a refusal. Apply records exact backups in the receipt.
Independent cleanup writers serialize with a lock; intervening edits are checked
before each replacement. A stale lock is an error and is never stolen automatically.

Compound/custom hook commands whose ownership cannot be proven are preserved.
Plugin caches and Git map-refresh hooks are outside this lifecycle cleanup scope.
Reload the host session to discard cached registrations. `dev hook enable` reports
that lifecycle hooks are retired. DevMap's separate hooks remain available for
index maintenance. See [the migration audit](hook-migration-audit.md) for test
coverage and remaining platform limits.

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
devcouncil verify TASK_ID [--json] [--mode off|advisory|enforce] [--sandbox local|docker|nix] [--coverage PATH] [--project-root DIR]
```

Verifies the working-tree diff for `TASK_ID` through Go `verify.Run()`: no-work, planned-file scope, orphan diffs, dependency-risk, and expected-test / allowed-command execution. It also spawns **`dcverify`** for the rigor gates — stub detection (`stub_detected`), secret scanning (`security_risk`), and diff↔coverage (`diff_not_exercised`).
- `--coverage`: Path to a coverage profile (Go `-coverprofile` output, or LCOV) for the same change. Without it the diff↔coverage gate does not run and `coverage_skipped_reason` says `coverage profile not supplied`; the stub and secret gates run either way. A path that names no file is an error, not an unmeasured run.
- `rigor_applied` / `rigor_skipped_reason`: Exactly one is populated. `rigor_applied` names the gates that ran (`secret_scan`, `stub_detection`, and `diff_coverage` when a profile was given). When `dcverify` is not installed, `rigor_applied` is empty and `rigor_skipped_reason` names what to install. If `dcverify` **is** installed and fails, that is a blocking `rigor_check_unavailable` gap rather than a skip — a credential scan that could not run must not read like one that ran and found nothing.
- `dcverify` is discovered on `PATH`, excluding any candidate inside the repository under analysis (`proc.LookPathOutside`); `MANVI_VERIFY_BINARY` overrides discovery. It remains a separately installed component (`devcouncil install --only=dcverify`).
- `--mode`: `off` (default when unset) skips quality verification and reports `status: "skipped"`, `passed: false`, and `verification_skipped: true`; `advisory` still blocks hard-safety gaps; `enforce` blocks every `Blocking` gap. Hard-safety write policy is unchanged.
- `--json`: Output machine-readable verification results and typed `next_actions` for agent self-repair. Skipped tasks increment `completed_without_verification`, not `verified_tasks`. Completion and verification remain separate outcomes.
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
devmap deps <FILE> [--min-rung deterministic|high|speculative]
devmap trace <SRC> <DST>
devmap impact <TARGET> [--layers]
devmap neighbors <TARGETS…> [--depth N]
devmap affected <TARGETS…>
devmap dead [--json]
devmap clones [--kind exact|structural]
devmap preview --file <FILE> [--content <PATH|->]

# Git + Code Graph Bidirectional Analysis
devmap blast [--since <REV>] [--at <LOC>]   # Forward change impact (inbound edges)
devmap suspects <SYMPTOM> --since <REV>     # Backward regression attribution (outbound edges)

# Structural & Route Analysis
devmap cypher '<QUERY>'
devmap pdg <FILE> [--taint]
devmap routes [--filter <PATH>]
devmap shape-check [--filter <PATH>]
devmap api-impact <ROUTE>
devmap ast <QUERY> [--kind <KIND>] [--language <LANG>]

# Session Insights & Gaps
devmap gap-record --tool <TOOL> --gap-id <ID> --reason <REASON>
devmap session-report [--last]

# Visualizers & Servers
devmap map-html             # Subsystem map (.devmap/map.html or .devcouncil/map.html)
devmap html                 # Symbol graph HTML
devmap mcp                  # DevMap MCP stdio (13 read-only tools)
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
dcstore --db <PATH> task --task <ID>
dcstore --db <PATH> ready
```

### Deterministic Verifier (`dcverify`)

Diff parser, scope classifier, and rigor gate evaluator:

```bash
dcverify health
dcverify check --planned "src/service.go" --root /path/to/repo < unified.diff
# Optional: --coverage /path/to/profile (Go coverprofile or LCOV)
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
| `dev check` / `dev check --verify` | Python LLM audit / demo check | Use `devcouncil verify TASK_ID --mode enforce --json` for an existing task, or `dcverify`. |
| `dev wiki` / `dev okf` / `dev design` | Markdown wiki & OKF bundle generator | Managed guides are available with `devmap build --manifest --guides`; they do not replace the full wiki/OKF feature set. |
| `dev dashboard` | Textual dashboard UI | Use DevMap HTML exports (`devmap html`, `devmap map-html`) or the consuming harness UI. |
| `dev cost` / `dev doctor` | Python model pricing ledger & env checks | Health checks via `devmap doctor` and binary health flags. |

For detailed rationale on the retirement decisions, see [PHASE7_LONG_TAIL.md](PHASE7_LONG_TAIL.md). Open follow-ups: [TODO.md](TODO.md).

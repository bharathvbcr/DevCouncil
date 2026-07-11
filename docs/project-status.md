# Project Status

DevCouncil is early-stage and under active development. Public commands are grouped by maturity so users can distinguish stable daily workflow surfaces from preview integrations.

Status labels:

- **Stable**: intended for normal task planning, execution, verification, and reporting.
- **Preview**: usable, but API/output shape and behavior may still change.
- **Experimental**: available for local trials; keep it behind explicit user choice and DevCouncil verification gates.

**Flagship path — the Claude Code hero loop.** The certified end-to-end experience is the
autonomous Claude Code + MCP closed loop: `checkout_task → implement → verify_task →
next_actions → self-repair`, gated by deterministic verification including the
diff↔coverage check. See [hero-loop.md](hero-loop.md). The underlying verifier, the
diff↔coverage gate, the typed next-actions contract, and the lite `dev check --verify`
on-ramp are Stable; the **certified Claude Code MCP closed loop** (checkout → write →
verify → repair → release) is Stable — see [certified-paths.md](certified-paths.md).
Other coding CLI hooks remain Preview/Experimental as noted below.

| Area | Status |
| :--- | :--- |
| **CLI & Storage** | Stable: SQLite + SQLModel, covered by unit tests and mypy |
| **Artifact Graph** | Stable: coverage engine and report generation |
| **Council Debate** | Stable: multi-agent planning, critique, arbitration |
| **Manual Executor** | Stable: sidecar mode |
| **Coding CLI Executors** | Preview: Codex, Gemini, Claude, OpenCode, Antigravity, Warp, Cursor Agent, Aider, Copilot, Goose, Amp, Qwen, Crush, and configured CLI agents |
| **Ollama (local provider)** | Stable: offline planning and council roles via local Ollama models; no API key required |
| **Engineering Skills** | Stable: `dev skills` listing/scaffolding; skills embedded in `dev prompt` and planning context |
| **OKF & design.md** | Preview: `dev okf export`/`ingest`/`validate`/`html`; `dev design lint`/`export`/`check`; OKF ↔ skills bridge |
| **CI Scaffolding** | Preview: `dev scaffold-ci` writes a starter GitHub Actions workflow from configured commands |
| **Cost & Run Telemetry** | Stable: `dev cost show` reads local model-call ledger; `dev runs list`/`show` inspects coding-agent run manifests |
| **Security Scanning** | Stable: secret redaction and detection |
| **Diff↔Coverage Gate** | Stable: proves the changed lines were exercised by tests; signal-first, opt-in blocking (`verification.diff_coverage`) |
| **Next-Actions Contract** | Stable: typed, machine-routable repair steps from `dev verify --json`, `dev check --json`, and MCP `verify_task` |
| **Lite Check (`dev check --verify`)** | Stable: deterministic working-tree evidence gate with no planning and no provider keys |
| **Repair Loop** | Preview: LLM-driven repair inference; `dev go`/`dev e2e` now drive a bounded, attempt-accounted self-repair loop (correction manifest + re-run, capped by `execution.max_repair_attempts`, with no-progress detection) for automated executors. The correction manifest is task-scoped — failed evidence is filtered to the task (no chasing another task's failures) and the repair plan's concrete files/tests are merged into the repair scope |
| **Native Executor** | Experimental: exposed as `native` / `native-preview`; completion still requires verification |
| **MCP Server (Claude Code hero loop)** | Stable: certified closed loop with lease-gated writes, typed next-actions, renew/list leases, golden e2e fixtures |
| **Multi-agent Campaign (`dev campaign`)** | Preview: parallel dependency-wave dispatch, Reviewer QC gate, per-task leases, cost budget + dashboard progress. Tasks that share writable `planned_files` are serialized when `--max-parallel` > 1 (one git working tree). |
| **Coding CLI Hooks** | Experimental / starter: Codex, Gemini, OpenCode; Claude hooks are part of the certified path |
| **GitHub PR Checks** | Preview: `dev report --github` |
| **GitHub/GitLab PR Comments** | Preview: `dev report --github-pr-comment`, `dev report --gitlab-pr-comment` |
| **LSP / AST Indexing** | Preview: `dev lsp inspect`, `dev ast match` |
| **Live Dashboard** | Preview: `dev dashboard --open`, status panels, recent runs, and guarded local integration apply controls |

## Watch mode (`dev check --watch`)

**Status:** Preview — incremental gate selection re-runs only the lint/typecheck/test
commands affected by each save, using a content-hash cache to skip unchanged inputs.

**Known limitations:**

- **Narrowed type-checking** — mypy/pyright gates are scoped to touched files plus
  *direct import dependents* from `.devcouncil/repo_map.json`. Transitive or
  dynamic-import type errors in other files can still be missed. Run full
  `dev verify` (or `dev check --verify`) before commit when types matter.
- **Config edits** — changing `pyproject.toml`, `ruff.toml`, `mypy.ini`, `tsconfig.json`,
  and similar project config files now re-runs the matching stack gates (with the config
  file in the cache inputs), but lockfile-only or toolchain-version changes outside that
  set may still require a manual full verify.

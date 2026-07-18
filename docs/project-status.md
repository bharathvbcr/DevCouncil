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
verify → repair → release) is Stable — see [hero-loop.md](hero-loop.md#certified-path-stable).
Other coding CLI hooks remain Preview as noted below.

| Area | Status |
| :--- | :--- |
| **CLI & Storage** | Stable: SQLite + SQLModel, covered by unit tests and mypy; `dev tasks` shows active lease owners |
| **Artifact Graph** | Stable: coverage engine and report generation |
| **Council Debate** | Stable: multi-agent planning, critique, arbitration |
| **Manual Executor** | Stable: sidecar mode |
| **Coding CLI Executors** | Preview: Codex, Claude, OpenCode, Antigravity, Warp, Cursor Agent, Aider, Copilot, Goose, Amp, Qwen, Crush, and configured CLI agents. **Gemini CLI is deprecated** (compat via `--executor gemini`; migrate to Antigravity). |
| **Ollama (local provider)** | Stable: offline planning and council roles via local Ollama models; no API key required |
| **Engineering Skills** | Stable: `dev skills` listing/scaffolding; skills embedded in `dev prompt` and planning context |
| **OKF & design.md** | Preview: `dev okf export`/`ingest`/`validate`/`html`; `dev design lint`/`export`/`check`; OKF ↔ skills bridge |
| **CI Scaffolding** | Preview: `dev scaffold-ci` writes starter GitHub Actions workflows from configured commands (Python/Node/Go/Rust stacks); `dev scaffold-ci --evidence` adds verify → artifact upload with PR + push diff bases (`VERIFY_BASE`). Dogfooded in this repo |
| **One-command onboarding (`dev boot`)** | Preview: `dev boot "goal"` runs setup, applies `dev integrate --apply` (unless `--skip-integrations`), optional `--scaffold-ci` / `--scaffold-ci-evidence`, then `dev go` |
| **Cost & Run Telemetry** | Stable: `dev cost show` reads local model-call ledger; `dev runs list`/`show` inspects coding-agent run manifests |
| **Security Scanning** | Stable: secret redaction and detection |
| **Diff↔Coverage Gate** | Stable: proves the changed lines were exercised by tests; signal-first, opt-in blocking (`verification.diff_coverage`) |
| **Next-Actions Contract** | Stable: typed, machine-routable repair steps from `dev verify --json`, `dev check --json`, and MCP `verify_task` |
| **Lite Check (`dev check --verify`)** | Stable: deterministic working-tree evidence gate with no planning and no provider keys |
| **Repair Loop (deterministic)** | Stable: `dev go`/`dev e2e` drive a bounded, attempt-accounted self-repair loop — correction manifest from blocking gaps + next-actions, capped by `execution.max_repair_attempts`, with no-progress fingerprint detection. Task-scoped failed evidence; repair plan files/tests merged into scope. |
| **LLM repair inference** | Preview: optional `RepairService` sharpens correction-manifest root cause when a provider key is configured; not required for the deterministic loop |
| **Native Executor** | Preview: `native` / `native-preview` — lease-gated writes, shared verify/next-actions loop; completion still requires DevCouncil verification |
| **MCP Server (Claude Code hero loop)** | Stable: certified closed loop with lease-gated writes, typed next-actions, renew/list leases, golden e2e fixtures |
| **Multi-agent Campaign (`dev campaign`)** | Preview: parallel dependency-wave dispatch, Reviewer QC gate, per-task leases, cost budget + dashboard progress. Tasks that share writable `planned_files` are serialized when `--max-parallel` > 1 (one git working tree). |
| **Coding CLI Hooks** | Preview: unified stop gate on Claude/Codex Stop+SubagentStop (`execution.stop_gate`; `assist` seeded on integrate when unset). Cursor/Grok pre/post hooks only (no Stop gate). Gemini hooks deprecated (explicit `--tool gemini` only). |
| **Stop gate & claim checks** | Preview: map completion claims → independent command/filesystem checks; combine with optional active-task verify. See [coding-cli-integration.md](coding-cli-integration.md#stop-gate-assist-vs-block-executionstop_gate). |
| **Corpus side index** | Preview: `dev corpus build`/`query`/`status`; optional rigor gates `corpus_stale`, `doc_code_ref`, `acceptance_corpus` (soft by default). See [corpus.md](corpus.md). |
| **PDG / CFG / taint** | Preview: opt-in Python intra-procedural analysis (`dev map --pdg`, `dev graph pdg-query` / `explain`). Off by default. See [code-graph.md](code-graph.md). |
| **GitHub PR Checks** | Preview: `dev report --github` |
| **GitHub/GitLab PR Comments** | Preview: `dev report --github-pr-comment`, `dev report --gitlab-pr-comment` |
| **LSP / AST Indexing** | Preview: `dev lsp inspect`, `dev ast match`; optional live refs via `indexing.lsp_refs` / `dev map --lsp-refs` |
| **Repo Map & Code Graph** | Stable: `dev map` (liveness, incremental `--watch` / `--if-stale`) + `dev graph query|trace|dead|check|process|impact|html|view|demo|export|ingest|search|cypher` — sample UI via `dev graph demo` (see [code-graph.md](code-graph.md)). Opt-in PDG/taint and live LSP refs remain Preview (separate rows). **Known limits:** compatibility JSON may be skipped when over `indexing.graph_json_max_bytes` (store stays committed; `dev graph doctor` flags degraded export); semantic embeddings are opt-in and generation-filtered with a soft scan cap; Louvain communities abort after 15s. MCP graph tools wait for sync freshness and mark `stale` when pending. |
| **Live Dashboard** | Stable: local-only operator UI via `dev dashboard --open` — status panels, blocking-first gaps table, recent runs; loopback + token-guarded apply controls |

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

## Near-term focus

- Promote more Preview surfaces (hooks, corpus) once API shapes settle
- PR-diff verify in CI scaffold
- Broader Stop-gate support beyond Claude/Codex where host APIs allow
- Deeper corpus ↔ acceptance criterion linking

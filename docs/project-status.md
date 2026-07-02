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
on-ramp are Stable; the MCP transport and hooks that carry the loop are still maturing
(Preview/Experimental as noted below).

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
| **MCP Server** | Preview: typed argument validation, read tools, and the autonomous verify/next-actions closed loop; resumable repair contract (persisted gap routing fields + `get_gaps`/`get_next_actions` read tools), TTL-expiring leases with `renew`/`list`, and a lease-gated, policy-checked write path (`write_file`/`apply_patch`) so a pure-MCP agent can close the loop end to end |
| **Coding CLI Hooks** | Experimental / starter: Codex, Gemini, Claude, Cursor (`.cursor/hooks.json`), OpenCode (bundled plugin) |
| **GitHub PR Checks** | Preview: `dev report --github` |
| **GitHub/GitLab PR Comments** | Preview: `dev report --github-pr-comment`, `dev report --gitlab-pr-comment` |
| **LSP / AST Indexing** | Preview: `dev lsp inspect`, `dev ast match` |
| **Live Dashboard** | Preview: `dev dashboard --open`, status panels, recent runs, and guarded local integration apply controls |

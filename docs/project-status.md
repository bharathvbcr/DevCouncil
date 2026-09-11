# Project Status

DevCouncil is **components and modules**. Public commands and components are grouped by maturity to distinguish stable daily workflow surfaces from preview features and retired legacy subsystems. Manvi wraps these modules into a harness; host apps such as GitPulse take only the components they need.

Maturity labels:
- **Stable**: Production-ready for daily development, verified by deterministic tests and golden fixtures.
- **Preview**: Fully functional; API, flag names, or output formats may undergo refinement.
- **Retired**: Legacy subsystems replaced or removed during the transition to compiled Go and Rust binaries.

---

## Current Subsystems Matrix

| Subsystem | Binary / Implementation | Status | Description |
|---|---|---|---|
| **Host Orchestrator** | `devcouncil` / `dev` (Go) | **Stable** | Static native binary providing the MCP server (`mcp`), host integration (`integrate`), skills distribution (`skills`), and task verification (`verify`). |
| **Code Intelligence** | `devmap` (Rust) | **Stable** | Compiler-grade code graph, 36+ tree-sitter extractors, symbol resolution, blast-radius impact analysis, dead code detection, and workspace guide generation. |
| **Task & Lease Store** | `dcstore` (Rust) | **Stable** | SQLite-backed task repository with atomic mutual-exclusion leases for safe concurrent agent building. |
| **Deterministic Verifier** | `dcverify` (Rust) | **Stable** | Unified-diff parsing, declared planned-file scope enforcement, anti-laziness stub detection, and typed `next_actions` repair signals. |
| **Search Engine** | `dcgrep` (Rust) | **Stable** | Ripgrep-powered ignore-aware search engine with optional trigram indexing (`tgrep-core`). |
| **Hero Loop (MCP Closed Loop)** | Go Host MCP + thin `verify.Run()` | **Preview** | Checkout → implement → verify → repair → release over MCP. Host `devcouncil_verify_task` does **not** spawn `dcverify`; Manvi `runRigor` does. See TASK-P7-1. |
| **Agent Integrations** | `devcouncil integrate` | **Preview** | Cursor: `.cursor/mcp.json` + rule. Claude: `.mcp.json` only. Codex: comment-only toml. Antigravity / OpenCode / Warp / Aider / Gemini: stub receipt. `--write-gate` is ignored. See TASK-P7-8. |
| **Engineering Skills** | `devcouncil skills` | **Preview** | Embed and scaffold still ship Python-era hero-loop / verification contracts (TASK-P7-9). Domain skills (`core-engineering`, …) are the native set. |
| **Sandboxed Verification** | `devcouncil verify --sandbox` | **Not implemented** | Flag is accepted and copied onto the report. Commands still run via `/bin/sh -c` in the project root. `docker` / `nix` do not isolate. See TASK-P7-2. |

---

## Retired Subsystems (Phase 7 Migration)

As part of the consolidation into standalone native binaries, the following legacy Python orchestrator components were retired:

| Legacy Surface | Historical Function | Current Architecture / Successor |
|---|---|---|
| **Python CLI Launcher** | Typer/Click `dev` commands | Replaced by native Go `devcouncil` / `dev` binary and Node.js npm shim. |
| **Council Debate & Planning** | Multi-agent LLM debate (`dev plan`, `dev approve`) | Transitioned to upstream agent harnesses (e.g. **Manvi**). |
| **Agent Hub & Campaign** | Subprocess runner (`dev run`, `dev e2e`, `dev campaign`) | Handled over MCP via **Hero Loop** or orchestrated via **Manvi**. |
| **Codebase Wiki & OKF** | Markdown wiki & OKF bundle generator (`dev wiki`, `dev okf`) | Replaced by `devmap build --guides` (`AGENTS.md`, `CLAUDE.md`). |
| **Textual Dashboard** | Terminal dashboard (`dev dashboard`) | Replaced by DevMap visualizers (`devmap view`) and Manvi TUI. |
| **Corpus Side Index** | Doc/PDF/image index (`dev corpus`) | Consolidated into `devmap search` and standard repository mapping. |
| **Legacy Provider Routing** | Provider cost ledger & routing (`dev cost`, `dev setup`) | LLM routing is owned by upstream agent harnesses (e.g. **Manvi**). |
| **GitHub Checks + PR comments** | `integrations/github.py`, `reporting/github_check.py` | No Checks API writer in the Go host. GitPulse reads Dependabot / code scanning and can check out a PR; it does not post a check run from `devcouncil verify`. |
| **Offline SCA** | `repo/sca.py` (`pip-audit` / `npm audit` / `osv-scanner`) | No DevCouncil verify gate. GitPulse Insights Health (`src-tauri/src/analyzer/deps.rs`) runs `pip-audit` / `npm audit` / `cargo-audit` / `govulncheck` as a related job when a repo opens. |
| **Claim lie-detector** | `verification/claims/` | Swept up in the verification hard-cut; no mapper in Go verify. |
| **DAP debug broker** | `devcouncil_debug_*` (8 tools) | PHASE7: unused adjunct; not rebuilt. |
| **OpenHands / mini-SWE / Claude SDK executors** | `executors/` adapters | Manvi loop / hero-loop skill; those adapters were not transcribed. |

For complete rationale and architectural decisions regarding the retired surfaces, see [PHASE7_LONG_TAIL.md](PHASE7_LONG_TAIL.md). Open follow-ups are in [TODO.md](TODO.md).

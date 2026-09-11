# Project Status

DevCouncil is under active development. Public commands and components are grouped by maturity to distinguish stable daily workflow surfaces from preview features and retired legacy subsystems.

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
| **Hero Loop (MCP Closed Loop)** | Go Host + `dcverify` | **Stable** | Autonomous closed loop for Claude Code and Cursor over MCP (`checkout → implement → verify → repair → release`). |
| **Agent Integrations** | `devcouncil integrate` | **Stable** | Native configurations for Cursor, Claude Code, Codex, Antigravity, OpenCode, and Warp. |
| **Engineering Skills** | `devcouncil skills` | **Stable** | Embedding and scaffolding of verified skills (`core-engineering`, `devmap`, etc.) into target repositories. |
| **Sandboxed Verification** | `devcouncil verify --sandbox` | **Preview** | Execution of verification commands inside isolated Docker or Nix containers. |

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

For complete rationale and architectural decisions regarding the retired surfaces, see [PHASE7_LONG_TAIL.md](PHASE7_LONG_TAIL.md).

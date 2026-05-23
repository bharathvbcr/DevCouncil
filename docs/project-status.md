# Project Status

DevCouncil is early-stage and under active development. Public commands are grouped by maturity so users can distinguish stable daily workflow surfaces from preview integrations.

Status labels:

- **Stable**: intended for normal task planning, execution, verification, and reporting.
- **Preview**: usable, but API/output shape and behavior may still change.
- **Experimental**: available for local trials; keep it behind explicit user choice and DevCouncil verification gates.

| Area | Status |
| :--- | :--- |
| **CLI & Storage** | Stable: SQLite + SQLModel, covered by unit tests and mypy |
| **Artifact Graph** | Stable: coverage engine and report generation |
| **Council Debate** | Stable: multi-agent planning, critique, arbitration |
| **Manual Executor** | Stable: sidecar mode |
| **Coding CLI Executors** | Preview: Codex, Gemini, Claude, OpenCode, Antigravity, Warp, Cursor Agent, Aider, and configured CLI agents |
| **Security Scanning** | Stable: secret redaction and detection |
| **Repair Loop** | Preview: LLM-driven repair inference |
| **Native Executor** | Experimental: exposed as `native` / `native-preview`; completion still requires verification |
| **MCP Server** | Preview: typed argument validation and read-oriented tools, still evolving |
| **Coding CLI Hooks** | Experimental / starter: Codex, Gemini, Claude, Cursor (`.cursor/hooks.json`), OpenCode (bundled plugin) |
| **GitHub PR Checks** | Preview: `dev report --github` |
| **GitHub/GitLab PR Comments** | Preview: `dev report --github-pr-comment`, `dev report --gitlab-pr-comment` |
| **LSP / AST Indexing** | Preview: `dev lsp inspect`, `dev ast match` |
| **Live Dashboard** | Preview: `dev dashboard --open` |

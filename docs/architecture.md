# DevCouncil Architecture

DevCouncil is a gated orchestrator for AI-assisted software development. It ensures that AI-generated work proves it satisfies the original intent.

## Core Components

- **CLI**: Typer-based `dev` / `devcouncil` command surface for local terminal workflows.
- **Orchestrator & State Machine**: Manages transitions between planning, execution, and verification phases. See [gating state machine](architecture/gating-state-machine.md).
- **Artifact Graph**: Directed graph linking requirements, tasks, files, evidence, and gaps. See [artifact graph](architecture/artifact-graph.md).
- **Planning Council**: Multi-agent LLM debate for planning and critique.
- **Executors**: Adapters to run tasks via manual sidecar, mini-SWE-agent, OpenHands, native-preview, coding CLI execution, and registered bring-your-own CLI agents. See [executor adapters](executor-adapters.md).
- **Verifier & Gating**: Git cleanliness, authorized file modifications, test evidence, diff↔coverage, and secret scanning. Map/graph checks also surface unwired files, stale maps, wiring gaps, and dead-symbol candidates.
- **Repository map & code graph**: `dev map` writes `.devcouncil/repo_map.json` (subsystems, entry roots, liveness lists) and a symbol-level `.devcouncil/graph/code_graph.json`. Query with `dev graph query|trace|dead|check|process|impact|html`. See [code-graph.md](code-graph.md).
- **Knowledge formats**: Open Knowledge Format (OKF) export/ingest and `design.md` design-system lint/check, injected into planning and coding prompts. See [knowledge formats](knowledge-formats.md).
- **Engineering skills**: Bundled domain skills scaffolded into `.claude/skills/` and embedded in `dev prompt` output.

## Detailed Architecture Docs

| Topic | Document |
| :--- | :--- |
| Component map and module layout | [architecture/codebase.md](architecture/codebase.md) |
| Artifact graph nodes and persistence | [architecture/artifact-graph.md](architecture/artifact-graph.md) |
| Phase and task state machine | [architecture/gating-state-machine.md](architecture/gating-state-machine.md) |
| Orchestration flow | [architecture/orchestration.md](architecture/orchestration.md) |
| Executor adapters | [executor-adapters.md](executor-adapters.md) |
| Coding CLI integration tiers | [integration-tiers.md](integration-tiers.md) |
| Gating policy | [gating-policy.md](gating-policy.md) |
| Hero loop (Claude Code + MCP) | [hero-loop.md](hero-loop.md) |
| Repo map & code graph | [code-graph.md](code-graph.md) |

## Executors

Headless coding CLI adapters (all post-run verified by DevCouncil):

`codex`, `gemini`, `claude`, `opencode`, `antigravity`, `warp`, `cursor`, `aider`, `copilot`, `goose`, `amp`, `qwen`, `crush`, plus configured custom CLI agent names and their aliases.

Other execution paths:

- `manual` — sidecar prompts pasted into any coding tool
- `mini` — mini-SWE-agent
- `openhands` — OpenHands task API
- `native-preview` / `native` — built-in preview loop (experimental)

## Lite verification

`dev check --verify` runs the deterministic evidence gate against the current working tree without planning or provider keys. It shares the verifier, diff↔coverage gate, and typed next-actions contract used by `dev verify` and MCP `verify_task`. See [hero-loop.md](hero-loop.md).

## Cost and run telemetry

Model-call cost is recorded locally in `.devcouncil/model_calls.jsonl` and surfaced through `dev cost show`, `dev status`, reports, and the dashboard. Coding-agent runs write manifests under `.devcouncil/runs/<run-id>/` inspectable via `dev runs list` and `dev runs show`.

# DevCouncil: The Gated AI Orchestrator

<p align="center">
  <img src="src/devcouncil/assets/devcouncil-logo.svg" alt="DevCouncil Logo" width="300">
</p>

[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Python 3.12+](https://img.shields.io/badge/python-3.12+-blue.svg)](https://www.python.org/downloads/)
[![uv](https://img.shields.io/badge/managed%20by-uv-purple.svg)](https://github.com/astral-sh/uv)

**"DevCouncil should not merely generate code. It should make AI-generated work prove that it satisfied the original intent."**

DevCouncil is a high-integrity command-line orchestration platform for AI-assisted software development. It turns AI implementation from a black-box generation task into a gated engineering workflow where every change is authorized, verified, and traceable back to a requirement.

DevCouncil does not replace coding agents. It sits beside tools like Codex CLI, Gemini CLI, Claude Code, Warp/Oz, Cursor, Aider, and bring-your-own prompt-taking CLIs, then owns the plan, task scope, verification loop, repair prompts, and evidence trail.

## Documentation

- [Quickstart](docs/quickstart.md): shortest install-to-first-task path.
- [Daily workflow](docs/workflow.md): manual sidecar loop, verification, repair, and rollback.
- [Coding CLI integration](docs/coding-cli-integration.md): Codex, Gemini, Claude Code, Cursor, Aider, MCP, hooks, and automated executors.
- [CLI command reference](docs/cli-reference.md): available `dev` commands.
- [Architecture](docs/architecture.md): components, artifact graph, state machine, and gated execution.
- [Executor adapters](docs/executor-adapters.md): manual, coding CLI, native-preview, Mini-SWE, and OpenHands execution paths.
- [Live review](docs/live-review.md): `dev watch` session review, cards, signals, and blocking behavior.
- [Model routing](docs/model-routing.md): provider selection, role models, OpenRouter, and Vertex AI setup.
- [Security model](docs/security.md): redaction, permissions, allowlists, and local state.
- [Project status](docs/project-status.md): current maturity by subsystem.
- [Roadmap](docs/roadmap.md): planned work.

## Why DevCouncil Exists

Standard AI coding agents are good at producing the happy path, but they often fail in expensive ways when complexity grows:

- **Requirement omission:** agents lose track of original product or PRD constraints across chat turns.
- **Architecture drift:** agents add dependencies or change design patterns without explicit authorization.
- **Unverified success:** agents claim tests passed without proving that the new logic was exercised.
- **Hidden assumptions:** important decisions stay buried in transient chat history instead of durable project artifacts.

**DevCouncil makes evidence, not model confidence, the final authority.**

It creates a persistent **Requirement -> Task -> Diff -> Evidence** graph, blocks completion when evidence is missing, detects unauthorized changes, and produces a final report that can be reviewed like an engineering artifact.

## Quickstart

Run DevCouncil commands in a normal terminal from the root of the repository you want DevCouncil to manage. Do not run these commands inside a coding CLI chat.

Install `uv` first if it is missing:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
```

On macOS or Linux:

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

Install DevCouncil from npm:

```bash
npm install -g devcouncil
devcouncil --help
dev --help
```

Start the first gated workflow from your target repository:

```bash
cd path/to/your/project
dev setup
dev plan "Describe the implementation goal"
dev tasks
dev run TASK-001 --executor manual
dev prompt TASK-001
dev verify TASK-001
```

On a fresh interactive setup, DevCouncil can configure supported coding CLI integrations immediately; pass `--skip-integrations` if you want to defer that step.

Paste only the output from `dev prompt TASK-001` into Codex, Gemini, Claude Code, Warp, Cursor, Aider, or another coding tool. Keep `dev setup`, `dev plan`, `dev run`, and `dev verify` in the terminal at the repository root.

For an automated end-to-end run with a supported coding CLI installed:

```bash
dev e2e "Describe the implementation goal" --executor codex
dev e2e "Describe the implementation goal" --executor warp
dev go "Describe the implementation goal" --executor codex
```

`dev e2e` is the explicit one-command integration target for coding agents. It initializes local DevCouncil state if needed, plans the goal, runs each approved task through the selected executor, verifies the resulting diff, and prints the final report. If `--executor` is omitted, DevCouncil uses `execution.default_executor` from `.devcouncil/config.yaml`. `dev go` is kept as a shorter alias for the same flow.

For machine-readable agent handoff, write the final report to a stable file:

```bash
dev e2e "Describe the implementation goal" --executor codex --agent
dev e2e "Describe the implementation goal" --executor codex --json --report-file .devcouncil/reports/latest.json
```

`--agent` enables JSON output and writes `.devcouncil/reports/latest.json`. Fresh projects default to manual sidecar mode, so pass an automated executor or set `execution.default_executor` before using `dev e2e` without `--executor`.

See the full [quickstart](docs/quickstart.md) for installation variants, API-key setup, and first-run guidance.

Register any local CLI that accepts prompts. `dev agents` is the first-class agent hub; `dev integrate cli-agent` remains available for older scripts:

```bash
dev agents add opencode --command opencode --arg run --input-mode prompt-file --prompt-arg=--prompt-file --supports-mcp
dev agents
dev agents doctor
dev agents run TASK-001 --agent opencode --profile default
```

## Feature Set

DevCouncil is an application layer around coding agents. It does not just emit prompts; it owns the workflow state, validates task scope, records evidence, and produces release-style reports.

### Workflow Features

- **Repository onboarding:** `dev setup` initializes `.devcouncil/`, runs environment checks, offers integration setup, and prints the next useful commands.
- **Repository mapping:** `dev map` writes `.devcouncil/repo_map.json`, identifies important files and subsystems, filters generated/temp files, and keeps managed `AGENTS.md` / `CLAUDE.md` workspace guides synchronized.
- **Planning council:** `dev plan` turns a goal into requirements, acceptance criteria, assumptions, critique findings, and executable tasks.
- **Task graph:** `dev tasks` and `dev show TASK-001` expose requirement links, acceptance-criterion links, planned files, expected tests, allowed commands, forbidden changes, and status.
- **Scoped task prompts:** `dev prompt TASK-001` creates a constrained implementation prompt for sidecar agents, including file scope, verification expectations, and forbidden changes.
- **Execution:** `dev run TASK-001` supports manual sidecar mode, built-in coding CLI executors, external executors, and registered custom CLI agents.
- **One-command flow:** `dev e2e "goal"` and `dev go "goal"` can initialize state, plan, run approved tasks, verify the diff, and generate a report.
- **Verification:** `dev verify TASK-001` captures the diff, runs expected evidence commands, checks planned-file compliance, detects orphan changes, flags unplanned dependency edits, scans for secrets, and links evidence to acceptance criteria.
- **Repair:** `dev repair` converts blocking gaps into focused follow-up work instead of leaving failures as vague test output.
- **Rollback:** `dev rollback TASK-001` uses task checkpoints to revert scoped work when a task needs to be backed out.
- **Reporting:** `dev report` emits a requirements coverage table, evidence summary, blocking gaps, and live-review blockers; JSON and PR-comment paths are available for automation.

### App Surfaces

- **CLI:** `dev` and `devcouncil` expose the same Typer command surface for local terminal workflows.
- **Agent hub:** `dev agents` lists built-in and custom agents, `dev agents add` registers prompt-taking CLIs, `dev agents doctor` checks wiring, and `dev agents run` executes a task through a named agent/profile.
- **Integration hub:** `dev integrate all --apply` configures supported coding CLI and MCP integrations; targeted setup exists for Codex, Gemini, Claude Code, Cursor, Warp/Oz, hooks, and custom CLI agents.
- **MCP server:** `dev mcp-server` exposes DevCouncil context and workflow tools over stdio for MCP-capable clients.
- **Live review:** `dev watch` tracks review cards, signals, blocking feedback, and repair guidance while a session is active.
- **Trace viewer:** `dev trace tail --follow` streams local DevCouncil trace events for execution, verification, and agent handoff.
- **Dashboard:** `dev dashboard` serves a local status dashboard for project state and live workflow visibility.
- **Config editor:** `dev config` and `dev config models` inspect/update provider, model, executor, and command configuration.
- **Artifact tools:** `dev artifacts validate` checks stored graph integrity.
- **Code intelligence:** `dev lsp inspect` checks optional language-server readiness, and `dev ast match` searches code structurally.
- **Doctor:** `dev doctor` validates local dependencies, commands, and environment prerequisites before a workflow fails deeper in execution.

### Agent And Executor Support

DevCouncil works with human-in-the-loop sidecar sessions and automated prompt handoff:

- **Manual sidecar:** paste `dev prompt TASK-001` into any agent, then run `dev verify TASK-001`.
- **Built-in coding CLI adapters:** `codex`, `gemini`, `claude`, `warp`, and aliases such as `codex-cli`, `gemini-cli`, `claude-code`, `warp-cli`, `oz`, and `oz-cli`.
- **Custom CLI agents:** register any prompt-taking command with stdin, argument, or prompt-file handoff.
- **Execution profiles:** custom agents can use profiles such as `default`, `yolo`, and `prod` to adjust prompt constraints while DevCouncil still verifies the final diff.
- **External automated adapters:** `mini`, `openhands`, `native-preview`, and `native` are available when the corresponding local executor is configured.
- **Hook-aware clients:** `dev integrate hooks --apply` installs native write/shell tool hooks for supported clients so DevCouncil policy can block unauthorized actions before verification.

### Gates And Evidence

DevCouncil blocks completion on concrete gaps rather than model confidence:

- **Plan approval gates:** requirements must have acceptance criteria, acceptance criteria need verification methods, tasks must map to known requirements and acceptance criteria, high-impact assumptions must be resolved, and high/critical critique findings must be closed.
- **Task readiness gates:** the working tree must be clean for the task, planned files must be declared, and each task needs allowed commands plus expected verification evidence.
- **Diff gates:** verification detects files changed outside the planned task scope, dependency-file edits made without authorization, deleted/added files, and untracked file diffs.
- **Evidence gates:** passing evidence commands are linked back to acceptance criteria; missing passing evidence becomes a blocking gap.
- **Security gates:** secret scanning runs over captured diffs, and command output is redacted before it is written to logs.
- **Live-review gates:** unresolved critical review cards can block task verification and appear in reports.

### Providers, Models, And Cost Tracking

- **Providers:** OpenRouter and Vertex AI are supported through local configuration and secrets.
- **Role models:** planner, critic, arbiter, reviewer, and repair roles can share one model or use per-role overrides.
- **Structured repair:** model routing includes JSON repair paths for structured planning and review outputs.
- **Model defaults:** packaged YAML defaults ship with the tool so installed CLI environments do not depend on source-tree-only files.
- **Telemetry:** local trace and cost data feed `dev status`, reports, and dashboard surfaces.

### Reports And Automation Outputs

- **Markdown reports:** include verdict, coverage summary, requirement/task mapping, blocking gaps, and live-review status.
- **JSON reports:** `--json` and `--report-file` support machine-readable handoff to other automation.
- **Agent preset:** `--agent` writes `.devcouncil/reports/latest.json` for stable downstream consumption.
- **PR comments:** `dev report --github-pr-comment` and `dev report --gitlab-pr-comment` can publish verification summaries to pull/merge requests.
- **GitHub checks:** preview GitHub report/check surfaces are available for repository automation.

### Local State And Files

DevCouncil stores local workflow state in the target repository:

- `.devcouncil/config.yaml`: provider, executor, command, integration, and workflow settings.
- `.devcouncil/secrets.env`: local provider secrets such as API keys or Vertex AI project/location values.
- `.devcouncil/repo_map.json`: generated repository map and subsystem navigation index.
- `.devcouncil/state.sqlite`: SQLite state for requirements, assumptions, tasks, evidence, gaps, critique findings, and project phase history.
- `.devcouncil/checkpoints/`: task snapshots used by verification and rollback.
- `.devcouncil/logs/`: redacted stdout/stderr from verification commands.
- `.devcouncil/runs/<run-id>/agent-run.json`: prompt, executor, profile, exit status, and run metadata for automated agent executions.
- `.devcouncil/reports/latest.json`: optional machine-readable report generated by `dev e2e --agent`.
- `.devcouncil/integrations/`: generated integration files such as Warp/Oz MCP JSON.

### Maturity

The stable daily workflow is planning, manual sidecar execution, verification, repair, rollback, and reporting. Coding CLI executors, MCP, live review, dashboard, PR comments, LSP/AST tools, and GitHub check surfaces are preview features. Native autonomous execution is experimental and still requires DevCouncil verification before work is considered complete.

## Core Flow

DevCouncil's recommended default is **Manual Sidecar Mode**:

1. DevCouncil plans the work and creates a task graph.
2. You ask DevCouncil for one constrained task prompt.
3. You paste that prompt into your coding CLI or agent.
4. The agent edits the repository.
5. DevCouncil verifies the resulting diff against task constraints.
6. If verification fails, DevCouncil creates a focused repair loop.

The detailed task-by-task workflow lives in [docs/workflow.md](docs/workflow.md).

## How The Repo Runs

```mermaid
flowchart TD
    user["User runs dev/devcouncil"] --> cli["Typer CLI\nsrc/devcouncil/cli/main.py"]
    cli --> config["Config + secrets\n.devcouncil/config.yaml\n.devcouncil/secrets.env"]
    cli --> map["Repo map\nsrc/devcouncil/indexing/repo_mapper.py"]
    cli --> planning["Planning commands\ndev plan / dev prompt / dev tasks"]

    config --> providers["Model providers\nOpenRouter or Vertex AI"]
    providers --> router["ModelRouter\nrole models, cache, telemetry, structured JSON repair"]
    router --> planning

    planning --> storage["SQLite + repositories\nrequirements, tasks, gaps, evidence, state"]
    storage --> artifactGraph["Artifact graph\nRequirement -> Task -> Diff -> Evidence"]
    artifactGraph --> gates["Gate policy\nplanned files, commands, secret checks"]

    gates --> manual["Manual sidecar\ndev prompt + user agent edits"]
    gates --> coding["Coding CLI executor\nCodex, Gemini, Claude, Warp, custom CLIs"]
    gates --> native["Native preview executor\nLLM router + TaskRunner"]
    gates --> external["Mini-SWE / OpenHands adapters"]

    coding --> runlog["Run artifacts\nprompt file, redacted logs, manifest, trace events"]
    native --> runlog
    external --> runlog
    manual --> diff["Repository diff"]
    runlog --> diff

    diff --> verify["Verifier\ndev verify / automatic post-run verification"]
    verify --> evidence["Evidence + gaps"]
    evidence --> storage
    evidence --> repair["Repair loop\ndev repair / dev watch repair"]
    evidence --> report["Reports\ndev report, JSON, GitHub/GitLab comments"]

    cli --> mcp["MCP server\ndev mcp-server"]
    mcp --> storage
    mcp --> artifactGraph
    mcp --> repair

    cli --> live["Live review\ndev watch"]
    live --> cards["Cards + signals\nblocking review feedback"]
    cards --> report
```

## Install From Source

For local development inside this checkout:

```bash
uv sync
uv run dev --help
```

For a global install from this repository:

```bash
uv tool install --force .
dev --help
devcouncil --help
```

## Project Shape

DevCouncil implements a 7-phase software-team workflow:

1. Goal analysis and repository mapping.
2. Requirements drafting.
3. Council debate and task arbitration.
4. Gated execution with scoped files and commands.
5. Deterministic verification.
6. Repair-loop generation.
7. Evidence reporting.

Read [docs/architecture.md](docs/architecture.md) for the artifact graph, gating state machine, and component layout.

## Contributions

Project ideas and execution patterns come from the open-source ecosystem:

- [Sage](https://github.com/usetig/sage): peer-review-first model for planning and critique.
- [karpathy/llm-council](https://github.com/karpathy/llm-council): for the multi-LLM peer-review pattern.
- [GPT Pilot](https://github.com/Pythagora-io/gpt-pilot): for role-based software-team concept.
- [astral-sh/uv](https://github.com/astral-sh/uv): for reproducible Python package/runtime workflows.
- [OpenHands](https://github.com/All-Hands-AI/OpenHands): for workspace-aware agent execution patterns.
- [mini-SWE-agent](https://github.com/SWE-agent/mini-swe-agent): for lightweight execution loop inspiration.
- [SWE-agent](https://github.com/SWE-agent/SWE-agent): for full-spectrum autonomous SWE-style tasking patterns.
- [GitNexus](https://github.com/abhigyanpatwari/GitNexus): for structural codebase awareness.
- [graphify](https://github.com/safishamsi/graphify): for knowledge-graph-based coordination concepts.

## License

Licensed under the **Apache License, Version 2.0**. See [LICENSE](LICENSE) for details.

---

**"Trust the model, but verify the graph."**

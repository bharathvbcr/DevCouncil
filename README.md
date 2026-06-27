# DevCouncil: The Gated AI Orchestrator

<p align="center">
  <img src="https://raw.githubusercontent.com/bharathvbcr/DevCouncil/main/src/devcouncil/assets/devcouncil_logo_premium.png" alt="DevCouncil Logo" width="300">
</p>

[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Python 3.12+](https://img.shields.io/badge/python-3.12+-blue.svg)](https://www.python.org/downloads/)
[![uv](https://img.shields.io/badge/managed%20by-uv-purple.svg)](https://github.com/astral-sh/uv)

**"DevCouncil should not merely generate code. It should make AI-generated work prove that it satisfied the original intent."**

DevCouncil is a high-integrity command-line orchestration platform for AI-assisted software development. It turns AI implementation from a black-box generation task into a gated engineering workflow where every change is authorized, verified, and traceable back to a requirement.

DevCouncil does not replace coding agents. It sits beside tools like Codex CLI, Gemini CLI, Claude Code, OpenCode, Google Antigravity CLI, Warp/Oz, Cursor, Aider, and bring-your-own prompt-taking CLIs, then owns the plan, task scope, verification loop, repair prompts, and evidence trail.

## Documentation

- [Quickstart](docs/quickstart.md): shortest install-to-first-task path.
- [Daily workflow](docs/workflow.md): manual sidecar loop, verification, repair, and rollback.
- [Coding CLI integration](docs/coding-cli-integration.md): Codex, Gemini, Claude Code, OpenCode, Antigravity, Cursor, Aider, MCP, hooks, and automated executors.
- [Integration tiers](docs/integration-tiers.md): headless executor vs MCP-only vs sidecar definitions.
- [CLI command reference](docs/cli-reference.md): available `dev` commands.
- [Architecture](docs/architecture.md): components, artifact graph, state machine, and gated execution.
- [Executor adapters](docs/executor-adapters.md): manual, coding CLI, native-preview, Mini-SWE, and OpenHands execution paths.
- [Live review](docs/live-review.md): `dev watch` session review, cards, signals, and blocking behavior.
- [Model routing](docs/model-routing.md): provider selection, role models, OpenRouter, Vertex AI, Doubleword, and Ollama (local) setup.
- [Knowledge formats](docs/knowledge-formats.md): Open Knowledge Format (OKF) export/ingest/browse (`dev okf html` renders a bundle as a self-contained static HTML site) and design.md design-system lint/export plus `dev design check` (a CI-friendly gate that fails on hardcoded color/spacing/typography literals bypassing the tokens), injected as planning and coding context, plus the bidirectional OKF <-> engineering-skills bridge (`dev okf export --skills` / `dev okf ingest`).
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

### Run locally on macOS (Apple Silicon + Ollama)

DevCouncil runs fully offline against [Ollama](https://ollama.com) — no API key, no per-token cost. It is Apple-Silicon-aware: `dev setup --provider ollama` sizes the default local model to your Mac's unified memory, and `dev doctor` reports the chip/RAM, pings the Ollama server, and flags a too-small context window.

```bash
brew install ollama && ollama serve
ollama pull qwen2.5-coder:32b     # use the size `dev doctor` recommends for your RAM
export OLLAMA_NUM_CTX=16384       # large planning prompts need a raised context window
dev setup --provider ollama      # auto-selects the model for your RAM
```

See [Model routing → macOS / Apple Silicon](docs/model-routing.md) for the RAM-to-model table.

Paste only the output from `dev prompt TASK-001` into Codex, Gemini, Claude Code, OpenCode, Antigravity, Warp, Cursor, Aider, or another coding tool. Keep `dev setup`, `dev plan`, `dev run`, and `dev verify` in the terminal at the repository root.

For an automated end-to-end run with a supported coding CLI installed:

```bash
dev e2e "Describe the implementation goal" --executor codex
dev e2e "Describe the implementation goal" --executor antigravity
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

OpenCode and Google Antigravity CLI are built-in executors and MCP integrations:

```bash
dev integrate opencode --apply
dev run TASK-001 --executor opencode
dev agents run TASK-001 --agent opencode --profile default
dev integrate antigravity --apply
dev run TASK-001 --executor antigravity
dev agents run TASK-001 --agent agy --profile default
```

Register any other local CLI that accepts prompts. `dev agents` is the first-class agent hub; `dev integrate cli-agent` remains available for older scripts:

```bash
dev agents add myagent --command myagent --arg run --input-mode prompt-file --prompt-arg=--prompt-file --supports-mcp
dev agents
dev agents doctor
dev agents run TASK-001 --agent myagent --profile default
```

GEPA prompt-profile optimization is available for the agent hub:

```bash
dev agents optimize --agent codex --profile yolo --evals .devcouncil/evals/agent-profile.jsonl --dry-run
dev agents optimize --agent codex --profile yolo --evals .devcouncil/evals/agent-profile.jsonl --apply
```

## Feature Set

DevCouncil is an application layer around coding agents. It does not just emit prompts; it owns the workflow state, validates task scope, records evidence, and produces release-style reports.

### Workflow Features

- **Repository onboarding:** `dev setup` initializes `.devcouncil/`, generates the repo map + `AGENTS.md`/`CLAUDE.md` guides, scaffolds applicable engineering skills, runs environment checks, offers integration setup, and prints the next useful commands. Use `--skip-map` / `--skip-skills` to opt out, or `--scaffold-ci` to also write a starter GitHub Actions workflow.
- **Repository mapping:** `dev map` writes `.devcouncil/repo_map.json`, identifies important files and subsystems, filters generated/temp files, and keeps managed `AGENTS.md` / `CLAUDE.md` workspace guides synchronized. Subsystems, entry points, neighbors, and important surfaces are now inferred generically for **any** repository — grouped from the directory tree and ranked by an import-graph in-degree — so the map (and the structural context it feeds into prompts) is meaningful outside DevCouncil's own tree, not just within it. The map records the git HEAD and tracked-file fingerprint it was built from; when prompts reuse a map that has fallen behind the current code, they flag it as stale (run `dev map` to refresh) rather than silently feeding wrong structure. The map is also generated automatically on first init.
- **Engineering skills:** `dev skills` lists the bundled skills and shows which apply to the repository; `dev skills scaffold` writes them into `.claude/skills/<name>/SKILL.md`. A merged always-on `core-engineering` skill (think-before-coding, simplicity, surgical changes, goal-driven execution, evidence-grounded communication) plus domain skills (Android, iOS, Windows, web, AI training) that brief the agent on current SDKs, deprecations, and tooling before coding. Applicable skills are also embedded into `dev prompt` output.
- **CI scaffolding:** `dev scaffold-ci` writes a starter `.github/workflows/devcouncil.yml` derived from the configured test/lint/typecheck commands, filtered to the detected language stack; it never overwrites existing CI unless `--force`.
- **Planning council:** `dev plan` turns a goal into requirements, acceptance criteria, assumptions, critique findings, and executable tasks.
- **Task graph:** `dev tasks` and `dev show TASK-001` expose requirement links, acceptance-criterion links, planned files, expected tests, allowed commands, forbidden changes, dependencies, and status. Tasks can declare `depends_on`; the plan gate rejects unknown dependencies and cycles, and `dev go`/`dev e2e` run tasks in topological order and skip a task whose prerequisites didn't complete (rather than letting it fail spuriously and burn its repair budget).
- **Scoped task prompts:** `dev prompt TASK-001` creates a constrained implementation prompt for sidecar agents, including file scope, verification expectations, and forbidden changes. The prompt now embeds the current (secret-redacted) contents of each planned file with a top-level symbol outline, structural orientation (from the code-review graph when available, otherwise the generated `repo_map.json`), and a **dependents (blast-radius) list** — the files that import each file being changed, from the map's precomputed reverse-import index — so the agent edits in place and keeps call sites working instead of starting blind. A central prompt budget keeps the core (goal/scope/instructions) always present and fits the optional context sections in priority order (file contents > structural > dependents > skills), dropping the lowest-priority ones with an explicit marker rather than overflowing silently.
- **Execution:** `dev run TASK-001` supports manual sidecar mode, built-in coding CLI executors, external executors, and registered custom CLI agents.
- **One-command flow:** `dev e2e "goal"` and `dev go "goal"` can initialize state, plan, run approved tasks, verify the diff, and generate a report. With an automated executor the run is now a **closed loop**: a task that fails verification is re-driven through a bounded self-repair loop (a correction manifest is written and the executor re-run) until it verifies or the `execution.max_repair_attempts` budget is spent, with no-progress detection that stops early when the same blocking gaps reappear.
- **Verification:** `dev verify TASK-001` captures the diff, runs expected evidence commands, checks planned-file compliance, detects orphan changes, flags unplanned dependency edits, scans for secrets, and links evidence to acceptance criteria. An **empty diff can no longer pass** a task that declares files to create or modify (work that committed earlier is still recognized via the task checkpoint), and the result reports the rigor it ran at (`verification_mode` compiled vs coarse, `diff_empty`, `coverage_measured`/`coverage_skipped_reason`) plus a distinct `advisory_actions` list so an agent never mistakes "passed" for "proven." `dev verify` exits non-zero when a task is blocked so shell-driven agents can gate on `$?`.
- **Repair:** `dev repair` converts blocking gaps into focused follow-up work instead of leaving failures as vague test output.
- **Rollback:** `dev rollback TASK-001` uses task checkpoints to revert scoped work when a task needs to be backed out.
- **Reporting:** `dev report` emits a requirements coverage table, evidence summary, blocking gaps, and live-review blockers; JSON and PR-comment paths are available for automation.

### App Surfaces

- **CLI:** `dev` and `devcouncil` expose the same Typer command surface for local terminal workflows.
- **Agent hub:** `dev agents` lists built-in and custom agents, `dev agents add` registers prompt-taking CLIs, `dev agents doctor` checks wiring, `dev agents run` executes a task through a named agent/profile, and `dev agents optimize` uses GEPA to tune profile preambles from offline eval examples.
- **Integration hub:** `dev integrate all --apply` configures supported coding CLI and MCP integrations; targeted setup exists for Codex, Gemini, Claude Code, OpenCode, Antigravity, Cursor, Warp/Oz, hooks, and custom CLI agents. `dev integrate check` now reports each client's **enforcement posture** — `pre-action` (a native hook blocks unauthorized writes before they happen) vs `verify-only` (forbidden changes are caught only after the fact by verification) — so the containment guarantee isn't overstated for clients without a pre-action gate.
- **MCP server:** `dev mcp-server` exposes DevCouncil context and workflow tools over stdio for MCP-capable clients. `devcouncil_verify_task` now runs DevCouncil's strong compiled per-criterion checks when a provider key is configured (falling back to a clearly-labeled `coarse` mode otherwise), refuses to pass on an empty diff, and returns `verification_mode`, `diff_empty`, `coverage_measured`/`coverage_skipped_reason`, and an `advisory_actions` array alongside the blocking `next_actions`. Cheap, re-verify-free read tools — `devcouncil_get_gaps` and `devcouncil_get_next_actions` — let a reconnecting agent resume outstanding work from persisted gaps (which now carry `file`/`line`/`suggested_command`/`acceptance_criterion_id`). Task leases expire on a config-driven TTL so a crashed agent's task frees itself, with `devcouncil_renew_lease` and `devcouncil_list_leases` for long runs and fleet supervision; a partial-unique DB index enforces a single active lease per task, so concurrent checkouts can't both win the writer slot. A pure-MCP agent can now make the change itself through lease-gated write tools — `devcouncil_write_file` and `devcouncil_apply_patch` — which policy-check every target path *before* it lands (out-of-scope, protected, or escaping paths are rejected; a patch with any out-of-scope target is rejected whole, never partially applied), write atomically, and record a `FileChangeEvent` for provenance. The corpus is also browsable as MCP **resources** (`devcouncil://report`, `devcouncil://tasks`, `devcouncil://gaps`, `devcouncil://cards`, `devcouncil://task/{id}`) so a host can read project state without a tool call. `devcouncil_get_task_provenance` then exposes that audit trail — gated file changes, verification runs, diff-coverage evidence, and the latest correction manifest — so what happened on disk is inspectable. The diff↔coverage proof is now also retained across graph reloads (it was previously dropped), so reports and `dev status` reflect whether the changed lines were actually exercised.
- **Live review:** `dev watch` tracks review cards, signals, blocking feedback, and repair guidance while a session is active.
- **Trace viewer:** `dev trace tail --follow` streams local DevCouncil trace events for execution, verification, and agent handoff.
- **Dashboard:** `dev dashboard --open` serves a local status dashboard and opens it in the default browser for project state and live workflow visibility.
- **Agent-consumable CLI:** machine output for shell-driven agents — `dev prompt --json` (`{ok, task_id, prompt}`), `dev handoff --json` (`{ok, manifest_path, run_id, next_command}` to chain `dev run`), `dev verify` exits non-zero when blocked, and `dev status`/`dev report` accept `--fail-on-blocking` to exit non-zero on outstanding blocking gaps so a loop can gate on `$?`.
- **Config editor:** `dev config` and `dev config models` inspect/update provider, model, executor, and command configuration.
- **Artifact tools:** `dev artifacts validate` checks stored graph integrity.
- **Code intelligence:** `dev lsp inspect` checks optional language-server readiness, and `dev ast match` searches code structurally.
- **Doctor:** `dev doctor` validates local dependencies, commands, and environment prerequisites before a workflow fails deeper in execution.

### Agent And Executor Support

DevCouncil works with human-in-the-loop sidecar sessions and automated prompt handoff:

- **Manual sidecar:** paste `dev prompt TASK-001` into any agent, then run `dev verify TASK-001`.
- **Built-in coding CLI adapters:** `codex`, `gemini`, `claude`, `opencode`, `antigravity`, `warp`, `cursor`, `aider`, and aliases such as `codex-cli`, `gemini-cli`, `claude-code`, `opencode-cli`, `antigravity-cli`, `agy`, `agy-cli`, `warp-cli`, `oz`, `cursor-agent`, and `cursor-cli`.
- **Custom CLI agents:** register any prompt-taking command with stdin, argument, or prompt-file handoff.
- **Execution profiles:** custom agents can use profiles such as `default`, `yolo`, and `prod` to adjust prompt constraints while DevCouncil still verifies the final diff.
- **External automated adapters:** `mini`, `openhands`, `native-preview`, and `native` are available when the corresponding local executor is configured.
- **Hook-aware clients:** `dev integrate hooks --apply` installs write/shell hooks for Codex, Gemini, Claude, Cursor, and OpenCode so DevCouncil policy can block unauthorized actions before verification. The post-task hook can run deterministic verification of the active task and record gaps (enable `execution.verify_on_post_task`; off by default to keep hooks fast). File-write policy uses one shared path normalizer across the hook and task-policy paths that resolves every target and **denies anything outside the project root** (so the path that's checked is the path that's enforced), and the pre-tool-use hook is fail-closed: an unparseable or error payload is surfaced (and blocked under `--strict`/`DEVCOUNCIL_HOOK_STRICT`) rather than silently allowed.

### Gates And Evidence

DevCouncil blocks completion on concrete gaps rather than model confidence:

- **Plan approval gates:** requirements must have acceptance criteria, acceptance criteria need verification methods, tasks must map to known requirements and acceptance criteria, high-impact assumptions must be resolved, and high/critical critique findings must be closed.
- **Task readiness gates:** the working tree must be clean for the task, planned files must be declared, and each task needs allowed commands plus expected verification evidence.
- **Diff gates:** verification detects files changed outside the planned task scope, dependency-file edits made without authorization, deleted/added files, and untracked file diffs.
- **Evidence gates:** passing evidence commands are linked back to acceptance criteria; missing passing evidence becomes a blocking gap.
- **Security gates:** secret scanning runs over captured diffs, and command output is redacted before it is written to logs.
- **Live-review gates:** unresolved critical review cards can block task verification and appear in reports.

### Providers, Models, And Cost Tracking

- **Providers:** OpenRouter, Vertex AI, Doubleword, and Ollama (local, no key) are supported through local configuration and secrets.
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
- `.devcouncil/secrets.env`: local provider secrets such as API keys or Vertex AI project/location values. Git-ignored; copy `.devcouncil/secrets.env.example` and fill in real values. Environment variables take precedence over this file.
- `.devcouncil/repo_map.json`: generated repository map and subsystem navigation index.
- `.devcouncil/state.sqlite`: SQLite state for requirements, assumptions, tasks, evidence, gaps, critique findings, and project phase history.
- `.devcouncil/checkpoints/`: task snapshots used by verification and rollback.
- `.devcouncil/logs/`: redacted stdout/stderr from verification commands.
- `.devcouncil/runs/<run-id>/agent-run.json`: prompt, executor, profile, exit status, and run metadata for automated agent executions.
- `.devcouncil/reports/latest.json`: optional machine-readable report generated by `dev e2e --agent`.
- `.devcouncil/integrations/` and `.agents/`: generated integration files such as Warp/Oz MCP JSON and Antigravity MCP config.

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

    config --> providers["Model providers\nOpenRouter, Vertex AI, Doubleword, or Ollama"]
    providers --> router["ModelRouter\nrole models, cache, telemetry, structured JSON repair"]
    router --> planning

    planning --> storage["SQLite + repositories\nrequirements, tasks, gaps, evidence, state"]
    storage --> artifactGraph["Artifact graph\nRequirement -> Task -> Diff -> Evidence"]
    artifactGraph --> gates["Gate policy\nplanned files, commands, secret checks"]

    gates --> manual["Manual sidecar\ndev prompt + user agent edits"]
    gates --> coding["Coding CLI executor\nCodex, Gemini, Claude, OpenCode, Antigravity, Warp, custom CLIs"]
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

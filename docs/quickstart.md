# DevCouncil Quickstart

This is the shortest path for a new developer who wants to install DevCouncil, initialize a repository, connect a coding CLI, and run the first gated task.

Run DevCouncil commands in a normal terminal from the root of the repository you want DevCouncil to manage. Do not run these commands inside the coding CLI chat. Later, you paste the generated `dev prompt TASK-ID` output into Codex, Gemini, Claude Code, OpenCode, Antigravity, Warp, Cursor, Aider, or another registered CLI agent.

## Where To Run Commands

| Place | Run |
| :--- | :--- |
| Terminal at the target repo root | `dev setup`, `dev plan`, `dev run`, `dev prompt`, `dev verify` |
| Coding CLI chat | Only paste the generated `dev prompt TASK-ID` output |
| Terminal outside the target repo | Add `--project-root path/to/project` |

## 1. Install

DevCouncil is a Python CLI distributed through an npm wrapper. The wrapper delegates to `uv`, so install `uv` first if it is missing.

Windows:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
```

macOS or Linux:

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

For normal use, install DevCouncil from npm:

```bash
npm install -g devcouncil
devcouncil --help
dev --help
```

From a local checkout:

```bash
uv tool install --force .
devcouncil --help
```

For local npm wrapper testing before publishing a new package version:

```bash
npm install -g .
devcouncil --help
```

For local development inside this repo:

```bash
uv sync
uv run dev --help
```

## 2. Initialize A Project

Run this from the repository you want DevCouncil to manage:

```bash
cd path/to/your/project
dev setup
```

`dev setup` creates `.devcouncil/` if needed, runs the environment doctor, asks whether to configure coding CLI integrations on first run, and prints the next task commands.
If the configured model provider API key is missing, interactive terminals prompt for it and save it to local `.devcouncil/secrets.env`.
For non-interactive setup, pass it directly:

```bash
dev setup --api-key YOUR_KEY
```

To set the provider at the same time:

```bash
dev setup --provider openrouter --api-key YOUR_KEY
```

For Doubleword usage with its drop-in OpenAI-compatible API:

```bash
dev setup --provider doubleword --api-key YOUR_KEY
```

To choose models during initialization, pass one model for every role, then add per-role overrides only where needed:

```bash
dev setup --provider vertexai --model YOUR_DEFAULT_MODEL --role-model critic_a=YOUR_CRITIC_MODEL
```

Current model-backed DevCouncil commands support the `openrouter`, `vertexai`, and `doubleword` providers.

Vertex AI uses a Google Cloud access token plus project configuration:

```bash
export VERTEXAI_PROJECT=your-gcp-project
export VERTEXAI_LOCATION=global
dev setup --provider vertexai --api-key "$(gcloud auth print-access-token)"
```

You can also store Vertex project settings locally:

```bash
dev setup --provider vertexai --vertex-project your-gcp-project --vertex-location global --api-key "$(gcloud auth print-access-token)"
```

If `VERTEXAI_ACCESS_TOKEN` is not configured, DevCouncil can use `gcloud auth print-access-token` automatically after `gcloud auth login`.

Most other entry commands (`dev map`, `dev plan`, `dev run`, `dev status`, `dev verify`, etc.) now auto-initialize the project state if `.devcouncil/` is missing.

To preview the interactive code-graph HTML UI (synthetic import graph, no `dev map` required):

```bash
dev graph demo
```

This writes `.devcouncil/graph/demo.html` and opens it in your default browser.

To preview coding CLI integration commands:

```bash
dev setup --integrate
```

Fresh interactive setup prompts to apply supported coding CLI integrations immediately. Use `dev setup --skip-integrations` to defer that step.

`dev run --executor <client>` can be used for supported direct CLI execution modes (`codex`, `gemini`, `claude`, `opencode`, `antigravity`, `warp`, configured custom CLI agents, and their aliases), and it now performs verification after the tool exits.

To apply supported MCP integrations for detected clients:

```bash
dev setup --integrate --apply
```

If your coding CLI launches MCP servers from a different directory, pass the target repository explicitly:

```bash
dev setup --integrate --apply --project-root path/to/project
```

## 3. Run The First Task

For a coding agent or CI-style integration with a supported executor installed, use the single end-to-end command:

```bash
dev e2e "Add password reset with expiring single-use tokens" --executor codex
```

This is equivalent to `dev go`: it auto-initializes DevCouncil state if needed, plans the goal, executes approved tasks with the selected executor, verifies each task, and prints the final report. If `--executor` is omitted, DevCouncil uses `execution.default_executor` from `.devcouncil/config.yaml`. Use `--executor gemini`, `--executor claude`, `--executor opencode`, `--executor antigravity`, `--executor warp`, `--executor native-preview`, `--executor mini`, or `--executor openhands` when that executor is installed and configured.

For coding agents that should avoid scraping terminal output, write the final JSON report to a file:

```bash
dev e2e "Add password reset with expiring single-use tokens" --executor codex --agent
dev e2e "Add password reset with expiring single-use tokens" --executor codex --json --report-file .devcouncil/reports/latest.json
```

Create a plan:

```bash
dev plan "Add password reset with expiring single-use tokens"
```

Pick one task:

```bash
dev tasks
dev show TASK-001
dev run TASK-001 --executor manual
```

Generate the constrained prompt and give it to your coding CLI:

```bash
dev prompt TASK-001
```

Paste the output of that command into your coding CLI, or run directly through DevCouncil:

```bash
dev run TASK-001 --executor codex
dev run TASK-001 --executor gemini
dev run TASK-001 --executor claude
dev run TASK-001 --executor opencode
dev run TASK-001 --executor antigravity
dev run TASK-001 --executor warp
```
Aliases such as `codex-cli`, `gemini-cli`, `claude-code`, `claude-cli`, `opencode-cli`, `open-code`, `antigravity-cli`, `agy`, `agy-cli`, `warp-cli`, `oz`, and `oz-cli` are also accepted for direct DevCouncil execution mode.

To bring your own CLI agent:

```bash
dev agents add myagent --command myagent --arg run --input-mode stdin
dev agents doctor
dev agents run TASK-001 --agent myagent --profile default
```

Use `--profile yolo` for faster local runs that are still verified by DevCouncil, or `--profile prod` for restrictive prompts in high-risk repositories. The older `dev integrate cli-agent ... --apply` command still writes the same agent registry for compatibility.

Keep running DevCouncil verification commands in the same terminal at the repository root.
`dev run` executes coding-client adapters and automatically verifies changes after each run.

After the coding CLI edits files, verify the task:

```bash
dev verify TASK-001
```

Optional live inspection surfaces:

```bash
dev dashboard --open
dev lsp inspect
dev ast match "target_symbol"
```

To publish the final report back to a review thread, set the provider environment variables and run one of:

```bash
dev report --github-pr-comment
dev report --gitlab-pr-comment
```

If verification finds gaps, generate repair work:

```bash
dev repair
dev tasks
dev prompt REPAIR-001
```

## Daily Loop

```bash
dev tasks
dev run TASK-002 --executor manual
dev prompt TASK-002
dev verify TASK-002
dev report
```

Keep DevCouncil and the coding CLI in the same repository root whenever possible.

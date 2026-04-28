# DevCouncil Quickstart

This is the shortest path for a new developer who wants to install DevCouncil, initialize a repository, connect a coding CLI, and run the first gated task.

Run DevCouncil commands in a normal terminal from the root of the repository you want DevCouncil to manage. Do not run these commands inside the coding CLI chat. Later, you paste the generated `dev prompt TASK-ID` output into Codex, Gemini, Claude Code, Cursor, or Aider.

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

Current model-backed DevCouncil commands support the `openrouter` provider.

Most other entry commands (`dev map`, `dev plan`, `dev run`, `dev status`, `dev verify`, etc.) now auto-initialize the project state if `.devcouncil/` is missing.

To preview coding CLI integration commands:

```bash
dev setup --integrate
```

Fresh interactive setup prompts to apply supported coding CLI integrations immediately. Use `dev setup --skip-integrations` to defer that step.

`dev run --executor <client>` can be used for supported direct CLI execution modes (`codex`, `gemini`, `claude` and their aliases), and it now performs verification after the tool exits.

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

This is equivalent to `dev go`: it auto-initializes DevCouncil state if needed, plans the goal, executes approved tasks with the selected executor, verifies each task, and prints the final report. If `--executor` is omitted, DevCouncil uses `execution.default_executor` from `.devcouncil/config.yaml`. Use `--executor gemini`, `--executor claude`, `--executor native`, `--executor mini`, or `--executor openhands` when that executor is installed and configured.

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
```
Aliases such as `codex-cli`, `gemini-cli`, `claude-code`, and `claude-cli` are also accepted for direct DevCouncil execution mode.

Keep running DevCouncil verification commands in the same terminal at the repository root.
`dev run` executes coding-client adapters and automatically verifies changes after each run.

After the coding CLI edits files, verify the task:

```bash
dev verify TASK-001
```

Optional live inspection surfaces:

```bash
dev dashboard
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

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

DevCouncil is a Python CLI with an optional npm wrapper. Install `uv` first if it is missing.

Windows:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
```

macOS or Linux:

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

From a local checkout:

```bash
uv tool install --force .
devcouncil --help
```

For local npm wrapper testing before the package is published to npm:

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

`dev setup` creates `.devcouncil/` if needed, runs the environment doctor, and prints the next task commands.

To preview coding CLI integration commands:

```bash
dev setup --integrate
```

To apply supported MCP integrations for detected clients:

```bash
dev setup --integrate --apply
```

If your coding CLI launches MCP servers from a different directory, pass the target repository explicitly:

```bash
dev setup --integrate --apply --project-root path/to/project
```

## 3. Run The First Task

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

Paste the output of that command into your coding CLI. Keep running DevCouncil verification commands in the same terminal at the repository root.

After the coding CLI edits files, verify the task:

```bash
dev verify TASK-001
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

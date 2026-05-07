# Coding CLI Integration

DevCouncil works with any tool that can accept a prompt and edit files in the same repository.

## Compatibility Matrix

| Tool | Manual sidecar prompts | Headless prompt handoff | DevCouncil MCP tools | Write-blocking hooks |
| :--- | :---: | :---: | :---: | :---: |
| **Codex CLI** | Supported | Supported via `codex exec` | Supported via `codex mcp` | Native via `dev integrate hooks` |
| **Gemini CLI** | Supported | Supported via `gemini -p` or stdin | Supported via `gemini mcp` | Native via `dev integrate hooks` |
| **Claude Code** | Supported | Tool-dependent | Supported via `claude mcp` | Native via `dev integrate hooks` |
| **Warp / Oz** | Supported | Supported via `oz agent run` | Supported via Warp/Oz MCP JSON | Verification-gated sidecar |
| **Cursor** | Supported | Tool-dependent | Supported via `cursor --add-mcp` | Verification-gated sidecar |
| **Aider** | Supported | Prompt/stdin friendly | Not a primary path | Verification-gated sidecar |
| **Bring your own CLI** | Supported | Supported through configurable stdin, argument, or prompt-file handoff | Tool-dependent | Verification-gated sidecar |

## Fast Integration Setup

Preview coding CLI integrations:

```bash
dev setup --integrate
```

Apply supported MCP integrations and native hooks for installed clients:

```bash
dev setup --integrate --apply
```

Configure every coding CLI with first-party setup support:

```bash
dev integrate all --apply
# If your install only exposes the setup flow:
dev setup --integrate --apply
```

Preview exact setup commands without changing client config:

```bash
dev integrate all
```

Verify that DevCouncil is ready to expose MCP tools:

```bash
dev integrate check
```

`dev integrations` is an alias for `dev integrate`.

Set up one first-party integration at a time, or install native hooks separately:

```bash
dev integrate codex --apply
dev integrate gemini --apply
dev integrate claude --apply
dev integrate cursor --apply
dev integrate warp --apply
dev integrate hooks --apply
```

Register an arbitrary prompt-taking CLI:

```bash
dev integrate cli-agent opencode --command opencode --arg run --input-mode prompt-file --prompt-arg=--prompt-file --apply
dev run TASK-001 --executor opencode
```

If a configured MCP client launches tools from a different directory, point it at the target repository:

```bash
dev integrate all --apply --project-root path/to/project
```

## Codex CLI

Manual sidecar flow:

```bash
cd path/to/project
dev run TASK-001 --executor manual
dev prompt TASK-001
```

Paste the generated prompt into Codex CLI. After Codex finishes:

```bash
dev verify TASK-001
```

Headless handoff:

```bash
dev prompt TASK-001 | codex exec -
dev verify TASK-001
```

MCP setup:

```bash
dev integrate codex --apply
```

If Codex launches MCP servers outside the target repository root, set `DEVCOUNCIL_PROJECT_ROOT` to the repository path in the MCP server environment.

## Gemini CLI

Manual sidecar flow:

```bash
cd path/to/project
dev run TASK-001 --executor manual
dev prompt TASK-001
```

Paste the prompt into Gemini CLI, then verify:

```bash
dev verify TASK-001
```

Headless handoff:

```bash
dev prompt TASK-001 | gemini
dev verify TASK-001
```

Or:

```bash
gemini -p "$(dev prompt TASK-001)"
```

MCP setup:

```bash
dev integrate gemini --apply
```

If Gemini launches MCP servers outside the target repository root, configure the server with `DEVCOUNCIL_PROJECT_ROOT` pointing at the repository that contains `.devcouncil/`.

## Claude Code

Start Claude Code in the same repository, then paste the generated task prompt:

```bash
cd path/to/project
dev run TASK-001 --executor manual
dev prompt TASK-001
```

After Claude Code finishes:

```bash
dev verify TASK-001
```

DevCouncil also includes a native hook installer for hook-capable coding CLIs:

```bash
dev integrate hooks --apply
```

This writes project-local hook config for Codex CLI, Gemini CLI, and Claude Code. The hooks call `devcouncil hook pre-tool-use` before write/shell tools and `devcouncil hook post-tool-use` after tool execution, with `DEVCOUNCIL_PROJECT_ROOT` carried through the generated command.

The lower-level hook command group remains available:

```bash
dev hook --help
```

For direct execution flow from inside DevCouncil, run:

```bash
dev run TASK-001 --executor codex
dev run TASK-001 --executor gemini
dev run TASK-001 --executor claude
```

Those modes launch the corresponding client with the task prompt and return to DevCouncil for checkpointing and verification.

The intended hook integration is to call `dev hook pre-tool-use` before file-writing tools and block unauthorized writes with a non-zero exit. DevCouncil normalizes Claude, Codex, and Gemini tool-event payload shapes before applying the same policy.

MCP setup:

```bash
dev integrate claude --apply
```

Use `--scope local`, `--scope project`, or `--scope user` to choose where Claude Code stores the MCP server registration. DevCouncil defaults to `local`.

## Cursor

Use DevCouncil as the planning and verification shell around Cursor:

```bash
dev run TASK-001 --executor manual
dev prompt TASK-001
```

Paste the prompt into Cursor Chat or Agent mode and instruct Cursor to stay within the prompt's allowed files. When Cursor finishes:

```bash
dev verify TASK-001
```

If Cursor changes files outside the task scope, DevCouncil verification should flag the unauthorized diff.

MCP setup:

```bash
dev integrate cursor --apply
```

The command registers `devcouncil mcp-server` through Cursor's `--add-mcp` option with `DEVCOUNCIL_PROJECT_ROOT` set to the repository that contains `.devcouncil/`.

## Warp / Oz

DevCouncil supports Warp in two modes:

- Warp local agents can use DevCouncil through the generated MCP JSON file.
- The Oz CLI can run DevCouncil tasks directly with `dev run --executor warp` or `dev run --executor oz`.

MCP setup:

```bash
dev integrate warp --apply
```

This writes `.devcouncil/integrations/warp-mcp.json`:

```json
{
  "mcpServers": {
    "devcouncil": {
      "command": "devcouncil",
      "args": ["mcp-server"],
      "env": {
        "DEVCOUNCIL_PROJECT_ROOT": "/path/to/project"
      },
      "working_directory": "/path/to/project"
    }
  }
}
```

Direct execution:

```bash
dev run TASK-001 --executor warp
dev run TASK-001 --executor oz
```

Optional Warp/Oz execution settings can live in `.devcouncil/config.yaml`:

```yaml
integrations:
  warp:
    enabled: true
    command: oz
    run_mode: local
    profile: your-profile-id
    model: your-model-id
    share:
      - team:view
```

For cloud runs, set `run_mode: cloud` and `environment: <environment-id>`. DevCouncil still verifies the local working tree after the executor returns, so cloud workflows should sync changes back before verification.

## Bring Your Own CLI

DevCouncil can register any local CLI as an agent when the tool can receive a prompt through stdin, a command-line argument, or a prompt file. Registered agents are listed with `dev agents`, checked with `dev agents doctor`, and run with `dev agents run`.

Examples:

```bash
# stdin prompt
dev agents add myagent --command myagent --arg run --input-mode stdin

# prompt argument
dev agents add myagent --command myagent --arg run --input-mode argument --prompt-arg=--prompt

# prompt file
dev agents add myagent --command myagent --arg run --input-mode prompt-file --prompt-arg=--prompt-file

# MCP-capable agent
dev agents add myagent --command myagent --arg run --input-mode prompt-file --prompt-arg=--prompt-file --supports-mcp --help-arg --help
```

Then run:

```bash
dev agents run TASK-001 --agent myagent --profile default
dev agents run TASK-001 --agent myagent --profile yolo
dev agents run TASK-001 --agent myagent --profile prod
```

If `--profile` is omitted, DevCouncil uses the agent's configured `default_profile`. `default` is balanced local execution, `yolo` lets the agent move faster while DevCouncil still verifies the final diff, and `prod` adds restrictive prompt guidance for high-risk repositories. Built-in names and aliases such as `codex`, `claude`, `gemini`, `warp`, and `oz` are reserved for DevCouncil's built-in adapters.

The generated task prompt is written to `.devcouncil/<TASK-ID>-<executor>-task.md`, and each agent launch writes `.devcouncil/runs/<run-id>/agent-run.json` plus trace events for start, finish, failure, and verification.

Compatibility path:

```bash
dev integrate cli-agent myagent --command myagent --arg run --input-mode stdin --apply
dev run TASK-001 --executor myagent --profile default
```

## Aider

Start Aider in the target repository:

```bash
cd path/to/project
aider
```

Paste the output from:

```bash
dev prompt TASK-001
```

After Aider commits or leaves a working-tree diff:

```bash
dev verify TASK-001
```

If you want DevCouncil to inspect the live working tree before committing, verify before creating the final commit.

## Automated Executors

Manual sidecar mode is the recommended default because it works with any coding CLI and keeps the human in control of the agent session.

DevCouncil also has additional automated executor adapters:

```bash
dev run TASK-001 --executor mini
dev run TASK-001 --executor openhands
dev run TASK-001 --executor native-preview
```

Use these only when the target executor is installed and configured locally. Automated executor mode lets DevCouncil launch the implementation loop itself, capture the post-run diff, and verify the task automatically.

The live executor adapter values are `manual`, `mini`, `openhands`, `native-preview`, `native`, `codex`, `gemini`, `claude`, `warp`, and configured custom CLI names.
`codex-cli`, `gemini-cli`, `claude-code`, `claude-cli`, `warp-cli`, `oz`, and `oz-cli` are accepted aliases for their canonical names.

Direct `dev run --executor <coding-client>` execution now runs the selected coding CLI and automatically runs verification after the tool returns.

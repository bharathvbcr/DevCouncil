# Coding CLI Integration

DevCouncil works with any tool that can accept a prompt and edit files in the same repository.

## Compatibility Matrix

| Tool | Manual sidecar prompts | Headless prompt handoff | DevCouncil MCP tools | Write-blocking hooks |
| :--- | :---: | :---: | :---: | :---: |
| **Codex CLI** | Supported | Supported via `codex exec` | Supported via `codex mcp` | Native via `dev integrate hooks` |
| **Gemini CLI** | Supported | Supported via `gemini -p` or stdin | Supported via `gemini mcp` | Native via `dev integrate hooks` |
| **Claude Code** | Supported | Tool-dependent | Supported via `claude mcp` | Native via `dev integrate hooks` |
| **Cursor** | Supported | Tool-dependent | Supported via `cursor --add-mcp` | Verification-gated sidecar |
| **Aider** | Supported | Prompt/stdin friendly | Not a primary path | Verification-gated sidecar |

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
dev integrate hooks --apply
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
dev run TASK-001 --executor native
```

Use these only when the target executor is installed and configured locally. Automated executor mode lets DevCouncil launch the implementation loop itself, capture the post-run diff, and verify the task automatically.

The live executor adapter values are `manual`, `mini`, `openhands`, `native`, `codex`, `gemini`, and `claude`.
`codex-cli`, `gemini-cli`, `claude-code`, and `claude-cli` are accepted aliases for their canonical names.

Direct `dev run --executor <coding-client>` execution now runs the selected coding CLI and automatically runs verification after the tool returns.

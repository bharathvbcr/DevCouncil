# Coding CLI Integration Tiers

DevCouncil assigns each supported coding CLI to one of three integration tiers. Use this page to choose the right workflow and to see what deeper parity would require.

## Tier 1 — Headless executor

**Tools:** Codex CLI, Gemini CLI, Claude Code, OpenCode, Google Antigravity CLI, Warp/Oz, Cursor Agent (`cursor-agent`), Aider, GitHub Copilot CLI, Goose, Amp (Sourcegraph), Qwen Code, Crush (Charm)

**Capabilities:**

- `dev run TASK-001 --executor <client>` launches the tool non-interactively
- Task prompt written to `.devcouncil/<TASK>-<client>-task.md` when needed
- Post-run `dev verify` runs automatically
- Run manifest and trace events under `.devcouncil/runs/`

**Setup:** `dev integrate <client> --apply` for MCP where supported; executors work once the CLI is on `PATH`.

## Tier 2 — MCP companion (no headless executor)

**Tools:** None by default (Cursor is Tier 1 for `cursor-agent` and Tier 2 for editor-only workflows)

**Capabilities:**

- `dev integrate cursor --apply` writes `.cursor/mcp.json`
- Agent calls DevCouncil MCP tools during an interactive session
- Human pastes `dev prompt TASK-001` or uses the editor agent UI

**Setup:** `dev integrate cursor --apply` (MCP only if you stay in the editor without `cursor-agent`).

## Tier 3 — Sidecar only

**Tools:** Bring-your-own CLI (until registered), any tool without a first-party adapter

**Capabilities:**

- `dev run TASK-001 --executor manual` + `dev prompt TASK-001` pasted into the tool
- `dev verify TASK-001` after edits
- Optional registration: `dev integrate cli-agent NAME ... --apply`

**Setup:** No MCP or hooks required; verification is always the completion gate.

## Hooks and policy

| Tier | Native write/shell hooks (`dev integrate hooks --apply`) |
| :--- | :--- |
| Codex, Gemini, Claude | Yes — project hook JSON + `devcouncil hook pre-tool-use` |
| Cursor | Yes — `.cursor/hooks.json` (`preToolUse` / `postToolUse`) |
| OpenCode | Yes — bundled plugin via `dev integrate hooks` |
| Antigravity / Warp / Aider / Copilot / Goose / Amp / Qwen / Crush (executor only) | Verification-gated; hooks optional |
| Unregistered BYO CLI | Verification-gated only |

## Target parity (project decision)

| Tool | Target tier | Notes |
| :--- | :--- | :--- |
| Codex, Gemini, Claude, OpenCode, Antigravity, Warp | Tier 1 | Shipped |
| Cursor | Tier 1 via `cursor-agent --print --trust` | Shipped |
| Aider | Tier 1 via `aider --yes --message` | Shipped |
| Copilot, Goose, Amp, Qwen, Crush | Tier 1 via headless CLI adapters | Shipped |
| Editor-only Cursor | Tier 2 | MCP + manual prompt |
| Custom CLIs | Tier 3 → Tier 1 when registered | `dev integrate cli-agent` |

See [coding-cli-integration.md](coding-cli-integration.md) for commands and examples.

## Automation helpers

| Feature | Command / config |
| :--- | :--- |
| Auto-pick coding CLI when `default_executor` is `manual` | Used by `dev go` — first CLI on PATH in probe order |
| Custom probe order | `execution.coding_cli_probe_order` in `.devcouncil/config.yaml` |
| Live CLI output | `dev run TASK --executor codex --stream` or `execution.stream_cli_output: true` |
| Cursor session resume | `execution.cursor_resume_mode: project` or `task` (uses `cursor-agent create-chat` + `--resume`) |
| Strict integration doctor | `dev integrate check --strict` |
| Pick best executor on this machine | `dev integrate recommend` |
| View tier matrix | `dev integrate matrix` |
| Fast integration snapshot | `dev integrate status` / `dev integrate status --json` (includes `capabilities`) |
| Streamed run transcript | `.devcouncil/runs/<run-id>/transcript.txt` when `--stream` is used |
| CI integration report | `dev integrate check --json` (`ok`, `failures`, `recommended_executor`, `checks`) |
| CI report file | `dev integrate check --report-file` / `-o` / `--output` |
| Run artifact manifests | `.devcouncil/runs/<run-id>/agent-run.json` with completion metadata |
| Dashboard integration panel | `dev dashboard --open` — `/api/status` includes `integrations` and `recent_runs` |
| Dashboard apply/fix controls | `dev dashboard --open` local-only controls backed by the same integration service as `dev integrate` |

MCP exposes read-only integration status through `devcouncil_integration_status`; it does not apply integration changes.

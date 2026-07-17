# Coding CLI Integration

DevCouncil works with any tool that can accept a prompt and edit files in the same repository.

## Integration tiers

Each supported coding CLI falls into one of three tiers:

| Tier | Meaning | Examples |
| :--- | :--- | :--- |
| **1 — Headless executor** | `dev run TASK --executor <client>` launches non-interactively; post-run `dev verify`; run manifests under `.devcouncil/runs/` | Claude Code, Codex, OpenCode, Antigravity, Warp, Cursor Agent, Grok, Aider, Copilot, Goose, Amp, Qwen, Crush |
| **2 — MCP companion** | Interactive MCP tools without a headless executor (editor-only Cursor) | Cursor MCP via `.cursor/mcp.json` |
| **3 — Sidecar only** | `dev prompt` paste + `dev verify`; optional `dev integrate cli-agent` | Unregistered BYO CLIs |

**Gemini CLI is deprecated** — migrate to Antigravity; explicit `--executor gemini` remains compat only.

Setup: `dev integrate <client> --apply` for MCP where supported; executors work once the CLI is on `PATH`. Helpers: `dev integrate recommend`, `dev integrate matrix`, `dev integrate status`, `dev integrate check --strict`. Auto-pick when `default_executor` is `manual` uses `execution.coding_cli_probe_order`.

### Hooks and policy by client

| Client | Native write/shell hooks |
| :--- | :--- |
| Claude | Yes — opt-in blocking PreToolUse write gate + lifecycle/Stop hooks |
| Codex | Advisory PreToolUse + blocking Stop/SubagentStop; sandbox + verification are the write boundary |
| Cursor | Yes — `.cursor/hooks.json` |
| Grok Build | Yes — `.grok/hooks/devcouncil.json` (requires `/hooks-trust`) |
| OpenCode | Yes — bundled plugin via `dev integrate hooks` |
| Antigravity / Warp / Aider / Copilot / Goose / Amp / Qwen / Crush | Verification-gated; hooks optional |
| Unregistered BYO | Verification-gated only |

### Executor hardening parity

Safety policy is the same across coding CLIs; enforcement differs by runtime. Hook-aware clients (`dev run --executor codex|claude`, etc.) evaluate policy against the active task: writes are denied with no active run or outside planned files. Claude can block unplanned writes, secret paths, force pushes, `--no-verify` / `--no-gpg-sign`, and protected-branch hard resets before execution. Codex PreToolUse is advisory, so DevCouncil combines hook guidance with Codex sandbox and post-action verification. Claude/Codex **Stop** / **SubagentStop** run the unified stop gate. Clients without hooks get the task contract in prompts and fail non-compliant output at verification.

## Compatibility Matrix

| Tool | Manual sidecar prompts | Headless prompt handoff | DevCouncil MCP tools | Write-blocking hooks |
| :--- | :---: | :---: | :---: | :---: |
| **Claude Code** | Supported | Tool-dependent | Tools + resources + prompts via `claude mcp` | Assistive hooks + slash commands, subagents, output style, statusline, installable plugin (opt-in `--write-gate` for blocking containment) |
| **Codex CLI** | Supported | Supported via `codex exec` | Supported via `codex mcp` | Advisory PreToolUse + native Stop gate; sandbox and verify contain writes |
| **Gemini CLI** | Deprecated | Deprecated (compat only) | Deprecated — use Antigravity | Explicit `--tool gemini` only |
| **OpenCode** | Supported | Supported via `opencode run --file` | Supported via project `opencode.json` | Native via `dev integrate hooks` (bundled plugin) |
| **Google Antigravity CLI** | Supported | Supported via `agy --print` | Supported via project `.agents/mcp_config.json` | Verification-gated sidecar |
| **Warp / Oz** | Supported | Supported via `oz agent run` | Supported via Warp/Oz MCP JSON | Verification-gated sidecar |
| **Cursor** | Supported | Supported via `agent`/`cursor-agent --print --trust` (yolo adds `--force`; JSON output) | Supported via project `.cursor/mcp.json` | Native via `dev integrate hooks` (`.cursor/hooks.json`) |
| **Grok Build** | Supported | Supported via `grok -p` with `--directory` | Supported via `grok mcp add` or `.grok/config.toml` | Native via `dev integrate hooks --tool grok` (`.grok/hooks/devcouncil.json`; trust with `/hooks-trust`) |
| **Aider** | Supported | Supported via `aider --yes --message` | Not a primary path | Verification-gated sidecar |
| **GitHub Copilot CLI** | Supported | Supported via `copilot --allow-all-tools -p` | Tool-managed MCP config | Verification-gated sidecar |
| **Goose** | Supported | Supported via `goose run -i <prompt-file>` | Tool-managed extensions | Verification-gated sidecar |
| **Amp (Sourcegraph)** | Supported | Supported via `amp -x` | Tool-managed MCP config | Verification-gated sidecar |
| **Qwen Code** | Supported | Supported via stdin (Gemini-CLI compatible) | Tool-managed MCP config | Verification-gated sidecar |
| **Crush (Charm)** | Supported | Supported via `crush run` | Tool-managed MCP config | Verification-gated sidecar |
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
dev integrate check --strict   # Fail on real integration defects (optional coding CLIs stay warnings)
dev integrate check --json     # Machine-readable report (ok, recommended_executor, checks)
dev integrate check --report-file report.json  # Write the same JSON report to a file
dev integrate check -o report.json             # Alias for --report-file / --output
dev integrate status           # Fast PATH + config snapshot (no MCP probe)
dev integrate status --json
dev integrate recommend        # Best executor for this machine
dev integrate matrix           # Built-in tier/capability table
dev integrate all --apply --strict  # Apply all integrations, then strict check
```

`dev integrations` is an alias for `dev integrate`.

## Dashboard integration controls

`dev dashboard --open` serves a local dashboard with integration diagnostics and guarded apply/fix controls.

The dashboard can:

- run the same readiness check as `dev integrate check --json`
- apply project-local MCP files for Cursor, OpenCode, Antigravity, and Warp/Oz
- install supported hook files for Codex, Claude, Cursor, Grok, and OpenCode (Gemini excluded from `--tool all`; use Antigravity instead)
- re-run status after every action

Dashboard mutations are local-only. The server accepts apply requests only from loopback clients and requires a per-server token embedded in the served page. The API accepts only known integration targets, not arbitrary shell commands.

Set up one first-party integration at a time, or install native hooks separately:

```bash
dev integrate claude --apply
dev integrate codex --apply
dev integrate gemini --apply
dev integrate opencode --apply
dev integrate antigravity --apply
dev integrate cursor --apply
dev integrate warp --apply
dev integrate aider --apply
dev integrate hooks --apply
```

OpenCode is built in and uses an attached prompt file so large DevCouncil task prompts do not become giant command-line arguments:

```bash
dev run TASK-001 --executor opencode
dev agents run TASK-001 --agent opencode --profile default
```

Google Antigravity CLI is also built in. DevCouncil writes the full task prompt to a task file and launches `agy --print` with a short instruction to read that file:

```bash
dev integrate antigravity --apply
dev run TASK-001 --executor antigravity
dev agents run TASK-001 --agent agy --profile default
```

Register an arbitrary prompt-taking CLI:

```bash
dev integrate cli-agent myagent --command myagent --arg run --input-mode stdin --apply
dev run TASK-001 --executor myagent
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

This is a one-shot MCP + native-hook install. The MCP registration persists the
checkout's project-venv `dev` executable instead of a potentially stale global
package. Codex hooks use second-based timeouts and the canonical
`[features].hooks = true` flag. After installation, open Codex in the project and
run `/hooks` to review and trust the exact generated hook definitions.

Codex's current PreToolUse output contract supports `systemMessage`, but not a
portable deny decision. DevCouncil therefore reports this posture as
`advisory+verify`: successful hooks are silent, warnings use `systemMessage`, the
executor maps `plan` to a read-only sandbox and normal automation to
`workspace-write`, and the lease/scope verifier remains authoritative. Stop and
SubagentStop can block completion through Codex's native `continue: false` and
`stopReason` fields.

If Codex launches MCP servers outside the target repository root, set `DEVCOUNCIL_PROJECT_ROOT` to the repository path in the MCP server environment.

### Stop gate: `assist` vs `block` (`execution.stop_gate`)

Claude and Codex install **Stop** / **SubagentStop** hooks that run the unified stop gate
(claim checks against configured commands plus optional active-task verify). Cursor and Grok
hooks do **not** wire Stop handlers — their integrate paths leave `stop_gate.mode` at the
code default (`off`).

| Setting | Behavior |
| --- | --- |
| `off` | Stop hooks are no-ops (code default; safe when hooks are not installed). |
| `assist` | Failures surface as `systemMessage` / Codex `stopReason`; the agent may continue. **Seeded on `dev integrate claude/codex --apply`** (and `dev integrate hooks --apply --tool claude\|codex`) when `mode` is unset. |
| `block` | Failures return a blocking decision (Claude `{"decision":"block"}`; Codex `continue: false`). Opt in after hooks are trusted — DevCouncil does **not** default to `block`. |

```yaml
# .devcouncil/config.yaml — promote to block after dogfooding assist
execution:
  stop_gate:
    mode: block          # assist | block | off
    check_claims: true
    verify_active_task: true
    per_command_timeout: 90
    total_timeout: 120
    max_blocks: 2
    verify_cache_minutes: 5
```

Bare repos without Stop hooks stay on `off` unless you set `mode` explicitly.
Override for a session with `DEVCOUNCIL_STOP_GATE=off|assist|block`.

#### Claim checks (`check_claims`)

When enabled, the stop gate extracts the last assistant turn, maps it to up to five
assertions (`verification/claims/`), and runs independent probes:

| Kind | Typical phrasing | Check |
| --- | --- | --- |
| `tests_pass` | "tests pass", "suite is green" | Configured test command(s) |
| `build_succeeds` | "build succeeds", "compiles cleanly" | Configured build command(s) |
| `lint_clean` | "lint passes", "no type errors" | Configured lint/typecheck command(s) |
| `file_created` / `file_updated` | "created `path`", "updated `path`" | Filesystem probe |
| `command_succeeded` | "ran `cmd` successfully" | Re-run quoted command under budget |
| `generic_done` | "done", "task is complete" | Weak signal; suppressed when specific assertions exist |

Hedged or negated sentences are skipped. Claim commands reuse the project `commands`
block in config. Maturity: **Preview** — see [project-status.md](project-status.md).

#### Active-task verify (`verify_active_task`)

When a leased/active task exists, the stop gate runs (or reuses a short TTL cache of)
`dev verify` and surfaces blocking gaps + next-actions in the stop message. Combined
with claim checks, this is the completion lie-detector for Stop hooks.

## Gemini CLI (deprecated)

Google replaced the consumer Gemini CLI with **Antigravity CLI**. Prefer:

```bash
dev integrate antigravity --apply
dev run TASK-001 --executor antigravity
```

The Gemini adapter remains for explicit `--executor gemini` / `gemini-cli` compatibility only. It is excluded from `dev integrate all`, default probe order, and `dev integrate hooks --tool all`.

Legacy manual flow (deprecated):

```bash
dev prompt TASK-001 | gemini
dev verify TASK-001
```

Explicit MCP/hooks (deprecated, prints a migration banner):

```bash
dev integrate gemini --apply
dev integrate hooks --apply --tool gemini
```

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

This writes project-local hook config for Claude Code, Codex CLI, Cursor (`.cursor/hooks.json`), Grok (`.grok/hooks/devcouncil.json` — run `/hooks-trust` in Grok after apply), and OpenCode (bundled `.devcouncil/integrations/opencode_devcouncil_plugin.mjs`). Gemini hooks are available only via explicit `--tool gemini` (deprecated). Generated commands use the checkout's resolved project `dev` executable and carry an explicit project root.

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

Claude's opt-in write gate calls `dev hook pre-tool-use` before file-writing tools and blocks unauthorized writes with a non-zero exit. Codex receives the same policy evaluation but its current PreToolUse schema is advisory; do not treat it as a blocking boundary. DevCouncil's lease-gated MCP writes and post-run verification remain blocking boundaries for both clients.

MCP setup:

```bash
dev integrate claude --apply
```

Use `--scope local`, `--scope project`, or `--scope user` to choose where Claude Code stores the MCP server registration. DevCouncil defaults to `local`.

### Complete Claude Code integration

`dev integrate claude --apply` is a one-shot that installs the entire Claude Code surface, not just the MCP server:

- **MCP server** — registered via `claude mcp add`, exposing DevCouncil's tools, resources, and **prompts** (the prompts surface as `/mcp__devcouncil__*` slash commands, e.g. `/mcp__devcouncil__implement_next_task`).
- **Assistive hooks** — `Stop`, `SessionStart`, `UserPromptSubmit`, `SessionEnd`, `PreCompact`, `PostCompact`, `SubagentStop`, and `Notification` are wired into `.claude/settings.local.json`. `SessionStart`/`UserPromptSubmit` inject a live DevCouncil status snapshot as context. When Claude compacts context, **`SessionStart` with `source=compact`** (matcher includes `compact|clear`) injects a **slim** `compact_briefing()` built from the on-disk snapshot — not the full status dump. **`PreCompact`** atomically writes `.devcouncil/state/compact_snapshot.json` (task, gaps, stop-gate summary) and may emit a user-only `systemMessage` toast; it does **not** inject model context. **`PostCompact`** is trace-only (no `additionalContext`). **`UserPromptSubmit`** skips redundant status when a compact briefing fired within ~60s (`execution.skip_prompt_status_after_compact_seconds`, default 60). **`Stop` / `SubagentStop`** run the unified **stop gate** (`execution.stop_gate`): claim checks against configured commands plus optional active-task verify. In `assist` mode failures surface as `systemMessage`; in `block` mode Claude receives `{"decision":"block","reason":...}`. **`post_task`** is a deprecated alias for the same handler; **`execution.verify_on_post_task`** is a deprecated alias for `stop_gate.mode != off` with `verify_active_task`.
- **Slash commands** — `.claude/commands/devcouncil/*.md` (`/devcouncil:status`, `/devcouncil:next`, `/devcouncil:verify`, `/devcouncil:repair`, `/devcouncil:plan`, `/devcouncil:review`, `/devcouncil:report`).
- **Subagents** — `.claude/agents/devcouncil-implementer.md`, `devcouncil-verifier.md`, and `devcouncil-reviewer.md`, each scoped to the relevant DevCouncil MCP tools.
- **Output style** — `.claude/output-styles/devcouncil.md` for evidence-first engineering discipline.
- **Skills** — the applicable engineering skills scaffolded into `.claude/skills/`.
- **Statusline + permissions** — a `statusLine` showing phase/tasks/gaps and an allow-list for the read-only `dev` commands the slash commands shell out to, merged into `.claude/settings.local.json` (existing keys are preserved).

#### Compaction continuity — deferred

The shipped path is SessionStart(`source=compact`) + PreCompact disk snapshot; these items were scoped out of the hardening pass:

- **Blocking PreCompact when gaps exist** — PreCompact can `decision:block`, but blocking compaction on open gaps risks stalling long sessions; durable graph + slim briefing is the default.
- **InstructionsLoaded(compact) beyond optional trace** — fires when CLAUDE.md reloads after compact; observability only (no model context channel).
- **Cursor/Grok lifecycle** — no PreCompact/PostCompact/SessionStart(compact) host events yet; parity waits on host support.
- **Stop-hook invisible `additionalContext`** — orthogonal to compaction; stop gate uses `systemMessage` / `decision:block`, not hidden model context.
- **Full transcript archival on PreCompact** — snapshot already carries task, gaps, and stop-gate summary; full transcript archive is heavier and may lag pre-summary text.

#### Anthropic advisor tool

Claude Code can pair a faster main model with a stronger **advisor** that Claude consults
mid-task (server-side Anthropic API tool — not live review, not the planning council, not
`opusplan`). Requires Claude Code ≥ 2.1.98 (Fable main/advisor needs ≥ 2.1.170) and the
Anthropic API. Soft pairing skips clear mismatches (haiku advisor, weaker family,
fable+non-fable); Claude Code validates the full versioned matrix at launch. Recommended:
`sonnet` + `opus`.

**When not to use:** mechanical one-liners, pure orientation/lookup turns, or Bedrock /
Vertex / Foundry Claude Code (advisor is Anthropic API only — DevCouncil soft-skips attach).

```yaml
# .devcouncil/config.yaml
integrations:
  cli_agents:
    profiles:
      default:
        model: sonnet
        advisor_model: opus
```

| Path | Enablement |
|---|---|
| `dev run/go/e2e --executor claude` | `--advisor` on every spawn, including `--resume` repairs |
| `dev run --executor claude-sdk` | SDK `extra_args={"advisor": ...}` (not a settings dict) |
| Interactive MCP hero loop | `advisorModel` merged into `.claude/settings.local.json` on integrate when the **default** profile sets a pairing-safe `advisor_model`; left alone when unset |

Soft pairing preflight skips clear bad pairs so Claude does not hard-exit and burn repair
budget. On repair/`--resume`, the correction manifest overrides prior session/advisor
advice. Set `CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1` to disable (Claude accepts `--advisor` /
`advisorModel` but ignores them). See [hero-loop.md](hero-loop.md).

#### Assist mode vs. the write-gate (important)

By **default** `dev integrate claude --apply` installs *assist mode* — everything above **except** the blocking pre-action write-gate (`PreToolUse`/`PostToolUse`). That write-gate denies any `Bash`/`Write`/`Edit` not authorized by an active task **lease**, so in an interactive human session (where there is no lease) it would fail-closed and block every command. Assist mode keeps DevCouncil's assistance without locking down your own shell.

Add the write-gate explicitly when you want pre-action containment (e.g. for autonomous executor runs):

```bash
dev integrate claude --apply --write-gate     # alias: --contain
```

You lose no containment by leaving it off: `dev run --executor claude` performs its own post-hoc scope enforcement (out-of-scope changes are reverted before verify), independent of this hook.

Remove everything DevCouncil installed (hooks, statusline, MCP enablement, permission rules,
DevCouncil-written `advisorModel` when it matches the default profile, and the generated
commands/subagents/output style — your own settings are preserved):

```bash
dev integrate claude --uninstall      # or: dev integrate uninstall --target claude
```

Install only the static asset files (no MCP/hook registration) with:

```bash
dev integrate claude-assets --apply
```

#### Installable plugin

Bundle the whole integration as a self-contained Claude Code plugin and single-repo marketplace:

```bash
dev integrate claude-plugin --apply
```

This writes a plugin under `.devcouncil/claude-plugin/` (manifest, bundled commands/subagents/skills, an assist-mode `hooks/hooks.json`, and `.mcp.json` resolving paths via `${CLAUDE_PROJECT_DIR}`). Pass `--write-gate` to bundle the blocking containment hooks instead. Install it in Claude Code with:

```text
/plugin marketplace add <repo>/.devcouncil/claude-plugin
/plugin install devcouncil@devcouncil-local
```

All generators are idempotent — re-running writes nothing when the files are already current.

## OpenCode

DevCouncil treats OpenCode as a first-class coding CLI executor:

```bash
dev run TASK-001 --executor opencode
```

The executor writes the DevCouncil task prompt to `.devcouncil/TASK-001-opencode-task.md` and launches:

```bash
opencode run --file .devcouncil/TASK-001-opencode-task.md "Execute the DevCouncil task described in the attached prompt file."
```

MCP setup:

```bash
dev integrate opencode --apply
```

This writes a project-level `opencode.json` entry for the local DevCouncil MCP server with `DEVCOUNCIL_PROJECT_ROOT` set to the repository root.

Upstream reference: [OpenCode MCP servers](https://thdxr.dev.opencode.ai/docs/mcp-servers/).

## Google Antigravity CLI

DevCouncil treats Google's Antigravity CLI as a first-class coding CLI executor:

```bash
dev run TASK-001 --executor antigravity
dev run TASK-001 --executor agy
```

The executor writes the DevCouncil task prompt to `.devcouncil/TASK-001-antigravity-task.md` and launches:

```bash
agy --print --print-timeout 30m "Read and execute the DevCouncil task prompt at .devcouncil/TASK-001-antigravity-task.md."
```

MCP setup:

```bash
dev integrate antigravity --apply
```

This writes a project-level `.agents/mcp_config.json` entry for the local DevCouncil MCP server with `DEVCOUNCIL_PROJECT_ROOT` set to the repository root:

```json
{
  "mcpServers": {
    "devcouncil": {
      "command": "devcouncil",
      "args": ["mcp-server"],
      "env": {
        "DEVCOUNCIL_PROJECT_ROOT": "/path/to/project"
      },
      "cwd": "/path/to/project"
    }
  }
}
```

Upstream references: [Antigravity CLI overview](https://antigravity.google/docs/cli-overview), [Antigravity CLI migration notes](https://antigravity.google/docs/gcli-migration), and [Antigravity MCP configuration](https://antigravity.google/docs/mcp).

## Cursor

Use DevCouncil as the planning and verification shell around Cursor.

Headless execution with Cursor Agent CLI:

```bash
dev integrate cursor --apply
dev run TASK-001 --executor cursor
dev run TASK-001 --executor cursor --profile yolo   # adds --force for unattended apply
export CURSOR_API_KEY=...   # CI headless auth
```

The executor launches `agent` or `cursor-agent --print --trust --workspace <repo> --output-format json` with a prompt that points at `.devcouncil/TASK-001-cursor-task.md`. Use `--stream` (or `execution.stream_cli_output: true`) for `--output-format stream-json --stream-partial-output`.

Manual sidecar flow (editor chat):

```bash
dev run TASK-001 --executor manual
dev prompt TASK-001
```

Paste the prompt into Cursor Chat or Agent mode and instruct Cursor to stay within the prompt's allowed files. When Cursor finishes:

```bash
dev verify TASK-001
```

MCP setup:

```bash
dev integrate cursor --apply
```

The command writes `.cursor/mcp.json` in the project so Cursor editor and `cursor-agent` can discover the same DevCouncil MCP server:

```json
{
  "mcpServers": {
    "devcouncil": {
      "type": "stdio",
      "command": "devcouncil",
      "args": ["mcp-server"],
      "env": {
        "DEVCOUNCIL_PROJECT_ROOT": "/path/to/project"
      }
    }
  }
}
```

Check Cursor CLI discovery with `agent mcp list` (or `cursor-agent mcp list`). `dev integrate check` probes auth via `agent status`/`about` and surfaces `CURSOR_API_KEY` for CI.

Upstream reference: [Cursor CLI MCP](https://docs.cursor.com/cli/mcp) and [Cursor headless](https://cursor.com/docs/cli/headless).

## Grok Build

Use DevCouncil as the planning and verification shell around Grok Build (xAI).

Headless execution:

```bash
dev integrate grok --apply
dev integrate hooks --apply --tool grok   # pre-action containment; then /hooks-trust in Grok
dev run TASK-001 --executor grok
dev run TASK-001 --executor grok --profile yolo   # --permission-mode acceptEdits
```

The executor launches `grok -p "<instruction>" --directory <repo> --output-format json` with the instruction pointing at `.devcouncil/TASK-001-grok-task.md`.

MCP setup prefers `grok mcp add devcouncil --scope project` when `grok` is on PATH; otherwise DevCouncil merges into `.grok/config.toml`:

```toml
[mcp_servers.devcouncil]
command = "devcouncil"
args = ["mcp-server"]
env = { DEVCOUNCIL_PROJECT_ROOT = "/path/to/project" }
```

Verify with `grok mcp list --json`. Session resume: `execution.grok_resume_mode: project` or `task` stores session ids under `.devcouncil/integrations/grok-session.json` or per-task sessions and passes `--resume`.

Aliases: `grok-build`, `grok-cli`, `gork`, `gork-build`, `xai-grok`.

Upstream reference: [Grok Build MCP](https://docs.x.ai/build/features/mcp-servers) and [Grok hooks](https://docs.x.ai/build/features/hooks).

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
  "devcouncil": {
    "command": "devcouncil",
    "args": ["mcp-server"],
    "env": {
      "DEVCOUNCIL_PROJECT_ROOT": "/path/to/project"
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

Upstream reference: [Warp/Oz MCP servers](https://docs.warp.dev/reference/cli/mcp-servers).

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

If `--profile` is omitted, DevCouncil uses the agent's configured `default_profile`. `default` is balanced local execution, `yolo` lets the agent move faster while DevCouncil still verifies the final diff, and `prod` adds restrictive prompt guidance for high-risk repositories. Built-in names and aliases such as `codex`, `claude`, `gemini`, `opencode`, `antigravity`, `agy`, `warp`, and `oz` are reserved for DevCouncil's built-in adapters.

The generated task prompt is written to `.devcouncil/<TASK-ID>-<executor>-task.md`, and each agent launch writes `.devcouncil/runs/<run-id>/agent-run.json` plus trace events for start, finish, failure, and verification.

Compatibility path:

```bash
dev integrate cli-agent myagent --command myagent --arg run --input-mode stdin --apply
dev run TASK-001 --executor myagent --profile default
```

### GEPA Profile Optimization

DevCouncil can use GEPA to optimize the prompt preamble for a CLI-agent profile from offline evaluation examples.

Write JSONL or JSON with examples of observed agent failures and desired prompt behavior:

```json
{"id":"missing-verification","observed_failure":"The agent claimed success without running tests.","desired_behavior":"Run allowed verification before the final response.","required_terms":["verification","evidence"],"forbidden_terms":["skip tests"]}
```

Preview the optimized profile text without changing config:

```bash
dev agents optimize --agent codex --profile yolo --evals .devcouncil/evals/agent-profile.jsonl --dry-run
```

Apply the best preamble into `.devcouncil/config.yaml` only after inspecting the artifact:

```bash
dev agents optimize --agent codex --profile yolo --evals .devcouncil/evals/agent-profile.jsonl --apply
```

The command writes an optimization artifact under `.devcouncil/optimizations/` by default. It does not run the coding CLI, stash changes, reset the repository, or clean untracked files; GEPA evaluates candidate prompt text against the offline examples.

## Aider

Headless execution:

```bash
dev integrate aider --apply
dev run TASK-001 --executor aider
```

DevCouncil launches `aider --yes --no-show-model-warnings --message <task prompt>` and verifies the working tree when Aider exits.

Manual sidecar flow:

```bash
cd path/to/project
aider
dev prompt TASK-001
```

Paste the prompt into Aider. After Aider commits or leaves a working-tree diff:

```bash
dev verify TASK-001
```

Aider does not have a first-party DevCouncil MCP integration path.

## GitHub Copilot CLI, Goose, Amp, Qwen Code, and Crush

These coding agents are built-in headless executors. No `dev integrate` step is required — install the CLI and run:

```bash
dev run TASK-001 --executor copilot   # copilot --allow-all-tools -p <task prompt>
dev run TASK-001 --executor goose     # goose run -i <prompt file>
dev run TASK-001 --executor amp       # amp -x <task prompt>
dev run TASK-001 --executor qwen      # task prompt over stdin (Gemini CLI-compatible)
dev run TASK-001 --executor crush     # crush run <task prompt>
```

DevCouncil captures the post-run diff and verifies the task automatically, the same as other Tier 1 executors. `dev doctor` and `dev integrate check` report whether each CLI is installed. Register the DevCouncil MCP server through each tool's own MCP configuration if you want DevCouncil tools available inside the agent session.

## Automated Executors

Manual sidecar mode is the recommended default because it works with any coding CLI and keeps the human in control of the agent session.

DevCouncil also has additional automated executor adapters:

```bash
dev run TASK-001 --executor mini
dev run TASK-001 --executor openhands
dev run TASK-001 --executor native-preview
```

Use these only when the target executor is installed and configured locally. Automated executor mode lets DevCouncil launch the implementation loop itself, capture the post-run diff, and verify the task automatically.

The live executor adapter values are `manual`, `mini`, `openhands`, `native-preview`, `native`, `codex`, `gemini`, `claude`, `opencode`, `antigravity`, `warp`, `cursor`, `aider`, `copilot`, `goose`, `amp`, `qwen`, `crush`, and configured custom CLI names.
`codex-cli`, `gemini-cli`, `claude-code`, `claude-cli`, `opencode-cli`, `open-code`, `antigravity-cli`, `google-antigravity`, `agy`, `agy-cli`, `warp-cli`, `oz`, `oz-cli`, `cursor-agent`, `cursor-cli`, `grok-build`, `grok-cli`, `gork`, `gork-build`, `xai-grok`, `copilot-cli`, `github-copilot`, `gh-copilot`, `goose-cli`, `block-goose`, `amp-cli`, `sourcegraph-amp`, `qwen-code`, `qwen-cli`, `crush-cli`, and `charm-crush` are accepted aliases for their canonical names.

Direct `dev run --executor <coding-client>` execution now runs the selected coding CLI and automatically runs verification after the tool returns.

## Run supervision

Each coding-agent launch writes `.devcouncil/runs/<run-id>/agent-run.json` plus trace events and optional git checkpoints. A supervisor (human or meta-agent) can inspect and reverse a run without reading raw JSON.

**CLI (Preview):**

```bash
dev runs list [--json] [--status running] [--limit N]
dev runs show RUN-ID [--json]
dev runs timeline <run-or-task> [--json] [--limit 40]
dev runs diff <run-or-task> [--stat]
dev runs revert <run-or-task> [--yes]
dev runs supervise <run-or-task> [--llm/--no-llm] [--apply] [--json]
```

- `timeline` joins manifest, trace events, checkpoints, and diff stat; accepts a run id or task id.
- `diff` prints the workspace patch from recorded checkpoints (`--stat` for summary only).
- `revert` restores the pre-run state when before/after checkpoints exist; `--yes` skips confirmation.
- `supervise` returns a **keep**, **revert**, or **repair** verdict. Default uses the `run_supervisor` model role with deterministic heuristics as fallback (`--no-llm` forces heuristics). **`--apply` is CLI-only:** when the verdict is `revert` and the run is reversible, the CLI reverts immediately.

**MCP tools (read-only verdict for supervise):**

| Tool | Purpose |
| :--- | :--- |
| `devcouncil_run_timeline` | Same timeline payload as `dev runs timeline --json` (`reference`, optional `limit`). Read-only. |
| `devcouncil_run_supervise` | Same verdict as `dev runs supervise --json` (`reference`). **Never modifies the workspace** — reverting stays an explicit separate step (`dev runs revert` or CLI `dev runs supervise --apply`). |

Use these from an MCP-connected coding agent to review a run before deciding whether to keep, repair, or revert its workspace effects.

# Coding CLI Integration

DevCouncil's components integrate with leading coding agents and IDEs: task scoping, mutual-exclusion leases, and deterministic verification. A host can take this integration alone, wrap the same modules through Manvi, or select individual binaries (`devmap`, `dcverify`, `dcstore`) without the rest of the suite.

---

## Supported Hosts

`devcouncil integrate` and `devmap integrate` are **not** the Python installer. Go `Hosts` lists six names and all six have adapters. `--write-gate` has been removed: it named a pre-tool-use gate that only the retired lifecycle hooks installed, and nothing enforced it. `devmap integrate` accepts only `cursor|claude|codex`.

| Host | What `--apply` actually writes | Hooks | Setup command |
|---|---|---|---|
| **Cursor** | `.cursor/mcp.json` (devcouncil + devmap stdio) and `.cursor/rules/devcouncil.mdc`. Then `devmap integrate cursor` writes DevMap guides/skills. | No `.cursor/hooks.json`. | `devcouncil integrate cursor --apply` |
| **Claude Code** | `.mcp.json` (devcouncil + devmap). No slash commands, plugin, subagents, or statusline. | None. | `devcouncil integrate claude --apply` |
| **Codex CLI** | `.codex/config.toml` containing only `# Managed by devcouncil integrate codex`. | None. | `devcouncil integrate codex --apply` |
| **Antigravity** | `.agents/mcp_config.json` — the `devcouncil` server, merged into any `mcpServers` already there. Skills land in `.agents/skills`, shared with Codex. | None documented. | `devcouncil integrate antigravity --apply` |
| **OpenCode** | `opencode.json` — the `devcouncil` server under `mcp`, plus every unrelated key preserved. | `<state>/integrations/opencode_devmap_plugin.mjs`, listed in `plugin`. Registers `tool.execute.after` only. | `devcouncil integrate opencode --apply` |
| **Warp / Oz** | `.devcouncil/integrations/warp-mcp.json` — a bare server map, passed to `oz agent run --mcp <path>`. | None documented. | `devcouncil integrate warp --apply` |
| **Gemini, Aider** | Refused, exit 1. Gemini CLI was replaced upstream by Antigravity; Aider exposes no MCP server and is a launch-command executor. Existing `.gemini/settings.json` hooks are still removable. | None. | `integrate antigravity` / run Aider directly (it uses the `devmap` CLI, not MCP) |

Python's golden `integrate claude --apply` receipt (43 files, slash commands, `claude-plugin`) is leftover testdata, not current behaviour.

---

## Quick Configuration

To configure an agent host, run `devcouncil integrate` from your project root:

```bash
# 1. Configure Cursor
devcouncil integrate cursor --apply

# 2. Configure Claude Code (writes .mcp.json)
devcouncil integrate claude --apply

# 3. Codex is a comment-only toml today — see TASK-P7-8
devcouncil integrate cursor --check
devcouncil integrate claude --check

# 5. Dry-run to inspect proposed changes without writing
devcouncil integrate codex --dry-run
```

To inspect and selectively remove legacy hooks:

```bash
dev hook status --project-root /path/to/project
dev hook disable --project-root /path/to/project --client claude --dry-run
dev hook disable --project-root /path/to/project --client claude
```

Cleanup preserves other tools and creates recoverable backups. Status exits 1
when registrations remain or inspection fails; its receipt distinguishes them.
Old event commands now exit 0 silently, even before a host reloads its cached
configuration. No verification is performed by retired hooks. MCP policy and
host permissions are unchanged. See [cleanup details](cli-reference.md#retired-hook-compatibility-and-cleanup).

---

## MCP Servers for Coding Agents

DevCouncil provides two complementary Model Context Protocol (MCP) servers:

### 1. Host MCP Server (`devcouncil mcp`)

The primary orchestration server. Configured automatically by `devcouncil integrate`. The Go host advertises **eight** tools (`Registry.Specs()`), not the Python host's 78:

- **Task leases:** `devcouncil_checkout_task`, `devcouncil_renew_lease`, `devcouncil_release_task`, `devcouncil_next_task`
- **Diff & gaps:** `devcouncil_get_diff`, `devcouncil_get_gaps`
- **Write policy:** `devcouncil_policy_check_write` — planned-file scope before writing
- **Verification:** `devcouncil_verify_task` — Go `verify.Run()` (planned-file, orphan-diff, dependency-risk, expected-test commands). It does **not** spawn `dcverify`. Stub / secret / coverage rigor is in that binary; **Manvi** `runRigor` is the path that execs it today.

Filesystem, patch, and shell tools (`devcouncil_read_file`, `apply_patch`, `write_file`, `run_command`, …) are **not** on this server. They live on Manvi. `AllowedNextToolsForVerify` still names several of those Python-era tools; that list is stale (TASK-P7-1).

### 2. DevMap Code Intelligence MCP Server (`devmap mcp`)

Eleven query tools: `status`, `search`, `dependencies`, `impact`, `trace`, `neighbors`, `dead_symbols`, `clones`, `preview`, `explore`, `affected_tests`. Always pass this repository's absolute `repo_path` and check `repository.root` in the envelope.

CLI-only (named in `devmap-cli` `MISSING_CAPABILITIES`, not on kernel MCP): `cypher`, `pdg_query`, `taint_explain`, `route_map`, plus `detect_changes` / `rename` / `clusters_processes`. PDG construction in the CLI is Python-source only.

---

## Host-Specific Setup Details

### Cursor

`devcouncil integrate cursor --apply` writes `.cursor/mcp.json` and `.cursor/rules/devcouncil.mdc`, then runs `devmap integrate cursor` (guides + DevMap skills). It does **not** write `.cursor/hooks.json`. `--write-gate` no longer exists; passing it exits 2 with the reason.

### Claude Code

`devcouncil integrate claude --apply` writes `.mcp.json` (devcouncil + devmap stdio) and runs `devmap integrate claude`. It does **not** install slash commands, a plugin, PreToolUse hooks, or a statusline. `--write-gate` no longer exists; passing it exits 2 with the reason.

### Codex

`devcouncil integrate codex --apply` writes a one-line comment into `.codex/config.toml`. That is the whole Go adapter.

### Other hosts

`antigravity`, `opencode` and `warp` have adapters here: this binary writes the `devcouncil` server into each host's document and `devmap integrate` writes the `devmap` entry into the same file. Each merges only its own entry, so the two commands compose in either order and neither disturbs a server the user added. `gemini` and `aider` are no longer accepted.

Domain skills still need `devcouncil skills scaffold` (or the `devmap skills install` spawn on cursor/claude). The packaged `devcouncil.md` / hero-loop skills currently describe Python-era tools (TASK-P7-9).

---

## Execution Modes

Coding agents interact with DevCouncil in two primary ways:

1. **The Autonomous MCP Loop (Hero Loop):**  
   The agent checks out a task, performs edits, calls `devcouncil_verify_task`, and repairs any issues until the gates pass. See [Hero Loop](hero-loop.md).

2. **Manvi (wraps the components):**  
   When running end-to-end multi-agent campaigns, **Manvi** wraps DevCouncil's components: it owns LLM provider routing and agent personas, and it reaches leases, write containment, and verification across the process boundary. GitPulse uses that wrap for policy and workbench, and selected DevCouncil modules directly for code intelligence.

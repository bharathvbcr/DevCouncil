# Coding CLI Integration

DevCouncil's components integrate with leading coding agents and IDEs: task scoping, mutual-exclusion leases, write-containment hooks, and deterministic verification. A host can take this integration alone, wrap the same modules through Manvi, or select individual binaries (`devmap`, `dcverify`, `dcstore`) without the rest of the suite.

---

## Supported Hosts

DevCouncil includes built-in integrations for:

| Host | MCP Tools | Write Hooks / Containment | Setup Command |
|---|:---:|:---:|---|
| **Claude Code** | Yes (`devcouncil mcp`) | Assistive PostToolUse; opt-in blocking PreToolUse (`--write-gate`) | `devcouncil integrate claude --apply --write-gate` |
| **Cursor** | Yes (`.cursor/mcp.json`) | Assistive PostToolUse; opt-in blocking PreToolUse (`--write-gate`) | `devcouncil integrate cursor --apply --write-gate` |
| **Google Antigravity** | Yes (`.agents/mcp_config.json`) | Verification-gated | `devcouncil integrate antigravity --apply` |
| **Codex CLI** | Yes (`codex mcp`) | Assistive hooks + Stop/SubagentStop checks | `devcouncil integrate codex --apply` |
| **OpenCode** | Yes (`opencode.json`) | Assistive plugin; opt-in blocking write gate | `devcouncil integrate opencode --apply` |
| **Warp / Oz** | Yes (Warp MCP JSON) | Verification-gated | `devcouncil integrate warp --apply` |
| **Aider** | Sidecar / CLI | Verification-gated | `devcouncil integrate aider --apply` |
| **Gemini CLI** | Deprecated | Compatibility only | Migrate to **Antigravity** |

---

## Quick Configuration

To configure an agent host, run `devcouncil integrate` from your project root:

```bash
# 1. Configure Cursor
devcouncil integrate cursor --apply

# 2. Configure Claude Code with strict write-containment
devcouncil integrate claude --apply --write-gate

# 3. Configure Google Antigravity
devcouncil integrate antigravity --apply

# 4. Verify that integration files are in place
devcouncil integrate cursor --check
devcouncil integrate claude --check

# 5. Dry-run to inspect proposed changes without writing
devcouncil integrate codex --dry-run
```

To uninstall hooks:

```bash
devcouncil integrate uninstall --target hooks --apply
```

---

## MCP Servers for Coding Agents

DevCouncil provides two complementary Model Context Protocol (MCP) servers:

### 1. Host MCP Server (`devcouncil mcp`)

The primary orchestration server. Configured automatically by `devcouncil integrate`:

- **Task Leases:** `devcouncil_checkout_task`, `devcouncil_renew_lease`, `devcouncil_release_task`
- **Diff & Gaps:** `devcouncil_get_diff`, `devcouncil_get_gaps`
- **Write Policy:** `devcouncil_policy_check_write` validates whether a target path is inside planned file scope before writing
- **Verification:** `devcouncil_verify_task` executes deterministic gates and returns typed `next_actions` for repair

### 2. DevMap Code Intelligence MCP Server (`devmap serve --mcp`)

Exposes compiler-grade symbol navigation and code graph exploration:

- **`devmap_explore`**: Explore symbol definitions, callers, callees, and enclosing modules.
- **`devmap_impact`**: Compute blast radius and reverse dependents of target files before refactoring.
- **`devmap_trace`**: Trace shortest call or dependency paths between two symbols.
- **`devmap_dead_symbols`**: Identify unreferenced symbols with confidence ratings.

---

## Host-Specific Setup Details

### Claude Code

Claude Code represents the flagship integration for the **Hero Loop**:
1. Run `devcouncil integrate claude --apply --write-gate`.
2. This installs:
   - MCP server definition in Claude Code's config.
   - Project skills in `.claude/skills/`.
   - PreToolUse write hook preventing Claude Code from modifying files outside the active task's scope.
   - Slash commands for task status and verification.

### Cursor

1. Run `devcouncil integrate cursor --apply --write-gate`.
2. This writes:
   - `.cursor/mcp.json` exposing the DevCouncil MCP server.
   - `.cursor/hooks.json` enforcing write containment during Agent Mode turns.
   - Packaged skills into `.cursor/skills/`.

### Google Antigravity

1. Run `devcouncil integrate antigravity --apply`.
2. This configures:
   - `.agents/mcp_config.json` exposing DevCouncil tools.
   - Verified engineering skills in `.agents/skills/`.

---

## Execution Modes

Coding agents interact with DevCouncil in two primary ways:

1. **The Autonomous MCP Loop (Hero Loop):**  
   The agent checks out a task, performs edits, calls `devcouncil_verify_task`, and repairs any issues until the gates pass. See [Hero Loop](hero-loop.md).

2. **Manvi (wraps the components):**  
   When running end-to-end multi-agent campaigns, **Manvi** wraps DevCouncil's components: it owns LLM provider routing and agent personas, and it reaches leases, write containment, and verification across the process boundary. GitPulse uses that wrap for policy and workbench, and selected DevCouncil modules directly for code intelligence.

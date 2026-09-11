# DevCouncil Quickstart

This is the shortest path for developers and agents to install DevCouncil, map a repository with code intelligence, connect coding agents, scaffold engineering skills, and verify changes.

**Platforms:** macOS, Linux, and Windows. Requires a Go toolchain (`>=1.22`), Rust/`cargo`, and Git. Node.js 18+ is only needed for the optional npm shim.

---

## 1. Install

DevCouncil consists of compiled native binaries: the Go host orchestrator (`devcouncil` / `dev`) and the Rust analysis suite (`devmap`, `dcstore`, `dcverify`, `dcgrep`).

### Option A: Unified Install Script

From a clone of this repository:

```bash
# macOS and Linux: builds Go host and all analysis components into ~/.local/bin
bash scripts/install.sh

# Standalone DevMap (no Go host)
bash scripts/install.sh --only=devmap

# Windows (PowerShell):
.\scripts\install.ps1
.\scripts\install.ps1 -Components devmap
```

To build and install the analysis components independently:

```bash
bash scripts/install-components.sh
```

Ensure `~/.local/bin` is in your `PATH`:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

### Option B: Global npm Shim

If you prefer to dispatch through npm, install the lightweight Node.js wrapper. The npm package ships a shim that resolves and executes the native binaries:

```bash
npm install -g devcouncil
```

### 2. Verify Your Environment

Confirm that the binaries are installed and healthy:

```bash
# Go host orchestrator
devcouncil --help
dev --help

# Analysis and verification engine
devmap --version
dcverify health
dcstore --db .devcouncil/state.sqlite health
dcgrep health
```

---

## 3. Map Your Repository

Before agents edit code, generate the repository map and code graph without calling an LLM:

```bash
cd /path/to/your/project

# Build the repository map (.devcouncil/repo_map.json) and code graph
devmap build --manifest

# Shorthand via the Go host:
dev map
```

This creates:
- `.devcouncil/repo_map.json`: Inventory of files, entry points, and subsystems.
- `.devcouncil/graph/code_graph.json`: Symbol-level graph of imports, calls, and definitions across 36+ languages.
- Managed `AGENTS.md` and `CLAUDE.md` workspace guides synchronized with the code structure.

---

## 4. Connect Your Coding Agent

Connect DevCouncil's MCP server and skills to Cursor or Claude Code. `--write-gate` does not install hooks (TASK-P7-8).

```bash
# Working adapters: cursor, claude, codex (comment-only toml)
devcouncil integrate cursor --apply
devcouncil integrate claude --apply
devcouncil integrate codex --apply

# gemini / opencode / warp / aider / antigravity write a stub receipt
```

Verify existing integration configs:

```bash
devcouncil integrate cursor --check
devcouncil integrate claude --check
```

---

## 5. Scaffold Engineering Skills

Deliver verified engineering practices and code-intelligence skills into agent skill folders (`.agents/skills`, `.claude/skills`, `.cursor/skills`):

```bash
# List available skills
devcouncil skills list

# Scaffold all applicable skills into the project
devcouncil skills scaffold

# Or scaffold specific skills
devcouncil skills scaffold --skill core-engineering
devcouncil skills scaffold --skill devmap
```

---

## 6. Run the Gated Task Loop

### MCP Integration (Recommended)

When working with Claude Code or Cursor, start the DevCouncil MCP server:

```bash
devcouncil mcp
```

Over MCP, agents follow the **Hero Loop**:
1. **Checkout:** `devcouncil_checkout_task` acquires an atomic lease on a task in `dcstore`.
2. **Implement:** Agent edits code within the declared file scope.
3. **Verify:** `devcouncil_verify_task` runs Go `verify.Run()` (scope / orphan / expected tests). It does **not** spawn `dcverify` (TASK-P7-1). Manvi `runRigor` does.
4. **Repair:** If verification fails, typed `next_actions` guide self-repair without manual prompt pasting.
5. **Release:** `devcouncil_release_task` releases the lease upon successful verification.

### CLI Verification

You can also run deterministic verification directly from the command line:

```bash
# Verify a task diff against rigor gates
devcouncil verify TASK-001

# Emit structured JSON for scripting and automation
devcouncil verify TASK-001 --json

# Sandbox flag is recorded only; docker/nix do not isolate (TASK-P7-2)
devcouncil verify TASK-001 --sandbox local
```

---

## Next Steps

- [CLI Reference](cli-reference.md): Full reference of all live commands and flags.
- [Code Graph Guide](code-graph.md): Deep-dive into symbol resolution, impact analysis, and dead code detection.
- [Coding CLI Integration](coding-cli-integration.md): Detailed configuration guides for each agent host.
- [Hero Loop](hero-loop.md): The certified autonomous MCP closed loop.

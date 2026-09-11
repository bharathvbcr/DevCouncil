# Developer & Agent Workflow

DevCouncil provides a gated, evidence-first workflow for AI-assisted software development. It ensures that changes made by coding agents are bounded by scope, protected by mutual-exclusion leases, verified deterministically, and backed by a deep code intelligence graph.

---

## The Workflow Stages

```
   ┌─────────────────────────────────────────────────────────────┐
   │ 1. Code Awareness & Exploration (devmap)                   │
   │    • devmap build --manifest                                │
   │    • devmap impact <file>  |  devmap trace <a> <b>          │
   └──────────────────────────────┬──────────────────────────────┘
                                  │
                                  ▼
   ┌─────────────────────────────────────────────────────────────┐
   │ 2. Environment & Skills Setup (devcouncil)                  │
   │    • devcouncil integrate <host> --apply [--write-gate]     │
   │    • devcouncil skills scaffold --skill <name>              │
   └──────────────────────────────┬──────────────────────────────┘
                                  │
                                  ▼
   ┌─────────────────────────────────────────────────────────────┐
   │ 3. The Verified Task Loop (devcouncil mcp / dcverify)       │
   │    • Checkout task & acquire lease (dcstore)                │
   │    • Implement edits inside declared file scope             │
   │    • Verify diff deterministically (dcverify)               │
   │    • On failure: self-repair guided by typed next_actions   │
   │    • On success: release lease & record evidence            │
   └─────────────────────────────────────────────────────────────┘
```

---

## Stage 1: Code Awareness & Impact Analysis

Before any files are modified, use the `devmap` code intelligence engine to understand dependencies and avoid breaking call sites:

```bash
# 1. Update the code graph and workspace guides (AGENTS.md / CLAUDE.md)
devmap build --manifest

# 2. Check blast radius and reverse dependents of target files
devmap impact src/service.go

# 3. Trace call paths between components
devmap trace EntryPoint TargetFunction

# 4. Identify dead code or unwired modules before adding new ones
devmap dead
```

The precomputed index identifies callers, callees, and imports across 36+ languages without invoking an LLM.

---

## Stage 2: Agent Integration & Skills Delivery

Prepare the target repository for your coding agent or IDE (Cursor, Claude Code, Codex, Antigravity, OpenCode, Warp):

```bash
# 1. Apply integration configuration (registers MCP tools and config)
devcouncil integrate cursor --apply
devcouncil integrate claude --apply --write-gate
devcouncil integrate antigravity --apply

# 2. Distribute verified engineering and navigation skills into the project
devcouncil skills scaffold --skill core-engineering
devcouncil skills scaffold --skill devmap
```

Skills are placed in standard directories (`.agents/skills/`, `.claude/skills/`, `.cursor/skills/`) where coding agents automatically discover and execute them.

---

## Stage 3: The Verified Task Loop

### Option A: Autonomous Closed Loop via MCP (Recommended)

When working with an agent capable of MCP tool calls (Claude Code, Cursor, Codex, Antigravity), DevCouncil runs as an MCP stdio server:

```bash
devcouncil mcp
```

The agent executes the certified **Hero Loop**:
1. **Task Checkout:** Calls `devcouncil_checkout_task`. Acquires an atomic lease in `dcstore`, locking the task against concurrent agents and receiving planned file scope and verification criteria.
2. **Implementation:** Edits code strictly within declared planned files.
3. **Task Verification:** Calls `devcouncil_verify_task`. The deterministic verifier (`dcverify`) evaluates the captured diff:
   - **Scope Enforcement:** Detects unauthorized file edits outside planned boundaries.
   - **Anti-Laziness:** Catches stubs, empty functions, and unfulfilled TODOs.
   - **Coverage & Tests:** Verifies test execution against modified lines.
   - **Secret Scanning:** Blocks committed API tokens and credentials.
4. **Self-Repair:** If verification fails, DevCouncil emits structured, typed `next_actions`. The agent iterates on repairs until the gates pass.
5. **Release:** Once verified, calls `devcouncil_release_task` to complete the task and record evidence.

### Option B: Human-in-the-Loop CLI Verification

For sidecar workflows where developers manually prompt agents or run local tests:

```bash
# 1. Inspect diff and verify against task gates
devcouncil verify TASK-001

# 2. Machine-readable JSON output for local automation
devcouncil verify TASK-001 --json

# 3. Verification inside an isolated Docker sandbox container
devcouncil verify TASK-001 --sandbox docker
```

---

## Stage 4: Autonomous Harness Orchestration (Manvi)

For fully automated, multi-turn development campaigns, DevCouncil is designed to be paired with upstream harnesses such as **Manvi**:
- **Manvi** owns the agent loop, LLM provider routing (OpenRouter, Vertex AI, Ollama), terminal UI, and role assignments.
- **DevCouncil** provides the high-integrity substrate: Go host orchestrator, `devmap` code intelligence, atomic `dcstore` leases, and deterministic `dcverify` gating.

See [Architecture](architecture.md) and [Hero Loop](hero-loop.md) for more details.

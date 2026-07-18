# DevCouncil — LinkedIn Content

---

## LinkedIn Project Section

**Title:** DevCouncil — Gated AI Orchestration Platform (open source)

**Description:**

DevCouncil turns AI-assisted coding from black-box generation into a plan → scope → verify → repair workflow, where every change is authorized, tested, and traceable back to a requirement — and evidence, not model confidence, decides what counts as "done."

It sits *beside* coding agents (Claude Code, Codex, Cursor, Aider, and more) and owns what they're weakest at: a multi-role planning "council" that debates a one-line goal into typed requirements and a scoped task graph; gated execution with scoped writes, command allowlists, and clean rollback; a four-tier verification gate (scope · tests · diff-coverage · rigor) with an acceptance-check compiler that turns each criterion into a runnable, majority-voted check; and an MCP server so an agent can drive the closed checkout → write → verify → repair loop itself.

Built in Python (Typer CLI, asyncio), with tree-sitter repo intelligence and provider-agnostic model routing across OpenRouter, Vertex AI, Doubleword, and local Ollama — it runs fully offline at zero cost.

In a controlled adversarial benchmark with hidden ground-truth tests, the gated loop lifted code correctness **+0.14 (0.94 vs. 0.81) with zero false negatives** — an early, deliberately small result I keep honestly caveated rather than oversold.

Apache 2.0 · `npm install -g devcouncil` · github.com/bharathvbcr/DevCouncil

---

## LinkedIn Post

My AI coding agent kept telling me it was done.

It wasn't.

The tests "passed" — but the edge cases were quietly broken, a requirement from five messages ago had vanished, and the green checkmark meant nothing. The problem was never that AI writes *bad* code. It's that it's *confident* about code it never actually proved.

So I built **DevCouncil**: a gated orchestrator that wraps any coding agent (Claude Code, Codex, Cursor…) and makes evidence — not model confidence — the thing that decides "done."

It debates your goal into scoped tasks, lets the agent touch only what it's allowed to, then verifies the diff four ways and turns every acceptance criterion into a runnable check. No evidence? The task is *blocked*, with a specific gap — not a vibe.

Early adversarial benchmark (hidden tests the agent never sees): correctness went **0.81 → 0.94, zero false negatives.**

Honest caveat: that's 3 small tasks. The full-scale runs are coming — I'd rather show a small real result than a big cherry-picked one.

If you build with AI, it's a safety net, not a leash.

Open source, on npm: `npm install -g devcouncil`

Trust the model, but verify the graph.

---

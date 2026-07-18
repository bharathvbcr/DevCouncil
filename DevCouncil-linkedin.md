# DevCouncil LinkedIn Content

## LinkedIn Project Section

**Title:** DevCouncil: Gated AI Orchestration Platform (open source)

**Description:**

DevCouncil turns AI-assisted coding from black-box generation into a plan → scope → verify → repair workflow where every change is authorized, tested, and traceable to a requirement — and evidence, not model confidence, decides what counts as "done."

It sits beside coding agents (Claude Code, Codex, Cursor, Aider) and owns what they're weakest at. It maps your repo into a symbol-level code graph (`dev map`), so plans reference real files and the agent gets the *blast radius* — every file that imports the one it's editing — before it changes a line. It debates a one-line goal into scoped tasks, then gates completion on four-tier verification (scope, tests, diff-coverage, rigor): if the evidence isn't there, the task is blocked with a specific gap, not a green checkmark it didn't earn.

Also included: parallel multi-agent campaigns, a self-repair loop, live review, an MCP server, reversible run timelines, and an auto-generated codebase wiki — across ~48 CLI commands on a 1,385-test suite. Built in Python; routes across OpenRouter, Vertex AI, Doubleword, and local Ollama (fully offline at zero cost).

In a controlled adversarial benchmark with hidden ground-truth tests, the gated loop lifted correctness **+0.14 (0.94 vs. 0.81) with zero false negatives** — an early, deliberately small result I keep honestly caveated.

Apache 2.0 · `npm install -g devcouncil` · github.com/bharathvbcr/DevCouncil

## LinkedIn Post

My AI coding agent kept telling me it was done.

It wasn't.

The tests "passed," but the edge cases were quietly broken, a requirement from five messages ago had vanished, and the green checkmark meant nothing. The problem was never that AI writes *bad* code. It's that it's *confident* about code it never actually proved.

So I built **DevCouncil**: a gated orchestrator that wraps any coding agent (Claude Code, Codex, Cursor…) and makes evidence, not model confidence, the thing that decides "done."

It starts by mapping the whole repo into a symbol-level graph with `dev map`. Subsystems, entry points, neighbors, imports, call sites. No config. The map stays honest: a missing or stale map is never trusted. Before the agent edits, it gets the blast radius, every file that imports the one it's about to change. You can query callers yourself, flag dead code, open an interactive graph, even generate a wiki the agent can read by subsystem.

Then it debates your goal into scoped tasks, lets the agent touch only what it's allowed to, verifies the diff four ways, and turns every acceptance criterion into a runnable check. Fail the gate and a repair loop kicks in with a specific gap, not a vibe. Live review can critique the session as it runs. MCP lets the agent drive the whole checkout → write → verify loop from inside the editor. Hooks stop unauthorized writes. Offline on Ollama if you want zero API cost.

Need more muscle? `dev campaign` fans a plan out to a parallel team of agents, caps the spend, and keeps every run reversible: timeline it, diff it, revert it from checkpoints.

Early adversarial benchmark (hidden tests the agent never sees): correctness went **0.81 → 0.94, zero false negatives.**

Honest caveat: that's 3 small tasks. The full-scale runs are coming. I'd rather show a small real result than a big cherry-picked one.

If you build with AI, it's a safety net, not a leash.

Open source, on npm: `npm install -g devcouncil`

Trust the model, but verify the graph.

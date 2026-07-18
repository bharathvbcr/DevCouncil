# DevCouncil LinkedIn Content

## LinkedIn Project Section

**Title:** DevCouncil: Gated AI Orchestration Platform (open source)

**Description:**

DevCouncil turns AI-assisted coding from black-box generation into a plan → scope → verify → repair workflow, where every change is authorized, tested, and traceable back to a requirement, and evidence, not model confidence, decides what counts as "done."

It sits *beside* coding agents (Claude Code, Codex, Cursor, Aider, and more) and owns what they're weakest at. A symbol-level code map (`dev map` / `dev graph`) parses the repo with tree-sitter into files, symbols, imports, call sites, and subsystems with entry points, neighbors, and handoff paths. That map grounds every plan in real files, keeps `AGENTS.md` / `CLAUDE.md` in sync, fails closed when stale, and hands the agent the blast radius (every file that imports the one it's editing) before a line changes. You can query callers, trace paths, flag dead code by confidence tier, and open an interactive graph view. From the same foundation it builds an agent-facing wiki per subsystem.

Around that map: a multi-role planning council that debates a one-line goal into typed requirements and a scoped task graph; gated execution with scoped writes, command allowlists, and clean rollback; a four-tier verification gate (scope, tests, diff-coverage, rigor) whose acceptance-check compiler turns each criterion into a runnable, majority-voted check; a bounded self-repair loop when evidence is missing; live review cards via `dev watch`; an MCP server so an agent can drive checkout → write → verify → repair itself; plus a local gaps dashboard, run timelines you can revert, and CI that uploads evidence reports.

Also in the box: parallel multi-agent campaigns (director → coordinator → workers → reviewer) with file-overlap serialization, cost budgets, and a progress dashboard; deeper code intelligence (`dev graph cypher`, an opt-in program-dependence graph, AST structural search, a semantic index, LSP detection, all behind a sha256-keyed parse cache); a corpus index for docs, PDFs, and images with their own verify gates; task-level provenance and an audit trail; secret redaction with local-only state; a design-token conformance gate; and GEPA prompt-profile optimization for custom agents, across ~48 CLI commands on a green 1,385-test suite the tool dogfoods on itself.

Built in Python (Typer CLI, asyncio), with provider-agnostic model routing across OpenRouter, Vertex AI, Doubleword, and local Ollama. It runs fully offline at zero cost.

In a controlled adversarial benchmark with hidden ground-truth tests, the gated loop lifted code correctness **+0.14 (0.94 vs. 0.81) with zero false negatives**, an early, deliberately small result I keep honestly caveated rather than oversold.

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

# DevCouncil — Resume & Portfolio Knowledge Points (2026)

> **Positioning line (use anywhere):**
> *Built **DevCouncil**, a gated AI orchestration platform (Python CLI) that turns AI-assisted coding from black-box generation into a **plan → scope → verify → repair** engineering workflow — every change is authorized, tested, and traceable to a requirement, with **evidence, not model confidence, as the final authority**. In a controlled adversarial benchmark it lifted code correctness **+0.14 (0.94 vs. 0.81) with zero false negatives.***

Each point has three layers so you can distill freely:
- **Headline** — the one-line claim.
- **Detail** — the technical substance for portfolio write-ups and interviews.
- **Resume bullet** — a tight, drop-in version.

A **"measured vs. illustrative"** note at the bottom flags exactly what you can defend.

---

## 1. Designed a "gated AI orchestrator" that makes AI work prove it satisfied intent

**Detail.** DevCouncil sits *beside* coding agents (Claude Code, Codex, Gemini CLI, OpenCode, Cursor, Aider, Antigravity, …) rather than replacing them, and owns the parts agents are bad at: the plan, task scope, verification loop, repair prompts, and evidence trail. Its thesis — *"DevCouncil should not merely generate code; it should make AI-generated work prove it satisfied the original intent"* — is enforced by a persistent **Requirement → Task → Diff → Evidence** graph that blocks completion when evidence is missing, flags unauthorized changes, and emits a final report reviewable like an engineering artifact. It directly targets the expensive failure modes of raw agents: requirement omission, architecture drift, and unverified "it passed" claims.

**Resume bullet.** *Designed and built a gated AI-orchestration platform that wraps any coding agent in an authorize→verify→prove workflow, making a Requirement→Task→Diff→Evidence graph — not model confidence — the gate on "done."*

---

## 2. Built a multi-role "council" that debates a terse goal into a typed, scoped plan

**Detail.** Planning isn't a single prompt — it's a structured debate across specialized roles (real prompt roles in the repo: **two independent planners** `planner_a`/`planner_b`, **two adversarial critics** `critic_a`/`critic_b`, a **rebuttal** round, an **arbiter**, a **spec_writer**, and an **implementation_reviewer**). The council converts a one-line goal into typed domain entities — `Requirement`, `Task`, `Assumption`, `Critique`, `Gap` — plus a `PlannedFile` scope whitelist, surfacing hidden assumptions and advisory gaps *before* code is written. Role LLM calls fan out concurrently (`asyncio.gather`) to keep planning latency down.

**Resume bullet.** *Implemented a multi-agent "council" (dual planners, adversarial critics, rebuttal, arbiter) that debates a natural-language goal into typed requirements, tasks, and a file-scope whitelist — with concurrent LLM fan-out.*

---

## 3. Engineered a gated execution state machine: plan → approve → scoped run → verify → repair → rollback

**Detail.** Every task moves through an explicit, authorized lifecycle. Scope is enforced at write time via a policy engine and gated writes (`execution/policy_engine.py`, `gated_write.py`, `permissions.py`, `hook_policy.py`); work is checkpointed so a failed verify can **roll back** cleanly (`checkpoints.py`, `dev rollback`); and a hook-policy gate constrains which commands an agent may run (allowlist derived from the project's configured test/lint/typecheck commands). The result is that an agent can only touch authorized files and run authorized commands — unauthorized changes are caught, not trusted.

**Resume bullet.** *Built a gated task-execution state machine — scoped writes, command allowlists, checkpoints, and clean rollback — so autonomous agents can only make authorized changes.*

---

## 4. Built a 4-tier verify gate with compiled, majority-vote acceptance checks

**Detail.** Completion is gated on four kinds of proof — **scope compliance** (`planned_files`, `orphan_diff`), **tests** (`command_evidence`), **diff↔coverage** (was the *new* logic actually exercised — `diff_coverage_gate`, `coverage_measurement`), and **rigor** (stub/no-op detection, `rigor_analytics`, `stub_detector`). The standout piece is the **acceptance-check compiler**: it turns each acceptance criterion into a *runnable* command, executes it against the diff, and decides by **majority vote** over N independently generated checks (a criterion is proven only on a strict majority; an all-fail is a real defect that blocks; a split is inconclusive). A distinct third verdict, `incomplete` (nothing failing, but a criterion lacks passing evidence), keeps the gate honest instead of forcing a binary pass/fail.

**Resume bullet.** *Designed a four-tier verification gate (scope · tests · diff-coverage · rigor) whose acceptance-check compiler turns each criterion into a runnable command decided by majority vote, with a distinct "incomplete" verdict for unproven-but-not-failing code.*

---

## 5. Proved effectiveness (and trustworthiness) with a purpose-built adversarial benchmark

**Detail.** Wrote a reproducible benchmark that measures whether the gated loop *actually* improves AI code — and whether its verdict can be trusted — versus the same agent alone. It's deliberately adversarial: each task ships a **hidden ground-truth suite** (edge cases, input mutation, error handling) the agent never sees, applied only at scoring time. Beyond raw **correctness lift** (`mean(B)−mean(A)`), it reports **verdict calibration** — the real trust metric: *when the gate says "passed," is it actually correct?* (precision/recall) — plus **silent-failure conversion** (raw defects the gate surfaced as `blocked`) and cost/latency overhead. Latest run (GLM-5.2 planner + Claude Sonnet executor): **gated 0.94 vs. raw 0.81 → +0.14 lift, 0 false negatives**, ~$8.49 planning cost.

**Resume bullet.** *Authored an adversarial A/B benchmark with hidden ground-truth suites that measured a +0.14 correctness lift (0.94 vs. 0.81, 0 false negatives) and reported verdict calibration (precision/recall) and silent-failure conversion — not just accuracy.*

---

## 6. Built a pluggable executor-adapter layer (BYO agent, one interface)

**Detail.** The same gated plan can be run through any of several execution backends behind one adapter interface: **manual sidecar** (paste a prompt into any tool), **coding-CLI** (Claude/Codex/Gemini/OpenCode/Antigravity/Cursor/Aider), the **Claude Agent SDK**, a **native-preview** path, **Mini-SWE**, and **OpenHands** — coordinated by an `agent_registry` with `transient_retry` for infra flakiness. This decouples DevCouncil's guarantees from any single vendor, so the orchestration layer outlives whichever agent is best this month.

**Resume bullet.** *Built a pluggable executor-adapter layer (manual, coding-CLI, Claude SDK, native-preview, Mini-SWE, OpenHands) behind one interface with a registry + transient-retry, decoupling the gating engine from any single agent vendor.*

---

## 7. Built provider-agnostic model routing that runs fully offline

**Detail.** A model router (`llm/router.py`, `provider.py`, `model_defaults.yaml`) selects models per council role across **OpenRouter, Google Vertex AI, Doubleword, and local Ollama**, so cost-sensitive roles run cheap/local while critical roles use a stronger model. It ships **bounded exponential backoff with a dedicated 429 budget that honors `Retry-After`** (a shared cooldown across concurrent fan-out, so a rate limit backs *all* in-flight calls off together). It runs **100% offline against Ollama** — no API key, no per-token cost — and is **Apple-Silicon-aware**: `dev setup --provider ollama` sizes the local model to the Mac's unified memory and `dev doctor` reports chip/RAM and flags a too-small context window.

**Resume bullet.** *Implemented provider-agnostic, per-role model routing (OpenRouter / Vertex AI / Doubleword / local Ollama) with rate-limit-aware backoff and Apple-Silicon-aware local sizing — enabling fully offline, zero-cost operation.*

---

## 8. Grounded planning and scope in real code via a repo-intelligence layer

**Detail.** Plans reference *actual* files and symbols, not hallucinated ones, because an indexing subsystem maps the target repo first: a **tree-sitter-based repo mapper** (`repo_mapper.py`), an **AST matcher** (`ast_matcher.py`), a **graph index** and **semantic index** (`graph_index.py`, `semantic_index.py`), and **language-server detection** (`lsp.py`) — with a **sha256-keyed parse cache** (`.devcouncil/cache/repo_map_parse.json`) so medium repos don't pay a 2–8s full re-parse each invocation. The map (`.devcouncil/repo_map.json`, refreshed via `dev map`) is what makes scope whitelists and planning context precise.

**Resume bullet.** *Built a repo-intelligence layer (tree-sitter repo mapper, AST matcher, graph + semantic indexes, LSP detection, sha256-keyed parse cache) that grounds AI plans and file-scope in the real codebase structure.*

---

## 9. Exposed the gated loop over MCP so an agent can drive it autonomously (the "hero loop")

**Detail.** DevCouncil registers an **MCP server** (`dev mcp-server`) that exposes the gated primitives — `checkout`, gated `write`/`apply-patch`, `verify`, `repair`, `rollback`, evidence/scope/provenance queries — as tools, so a coding agent like Claude Code can self-serve a closed **checkout → write → verify → repair → rollback** loop without a human in the terminal. Architecturally, the MCP tools **route through the same CLI service layer** the human uses, so both surfaces stay in sync (no drifting second implementation), and the closed loop is covered by end-to-end tests.

**Resume bullet.** *Exposed the full gated loop as an MCP tool server so agents can autonomously run checkout→write→verify→repair→rollback, routed through the shared CLI service layer and covered by closed-loop e2e tests.*

---

## 10. Held the codebase to its own standard: self-review, self-hosting, and log-audits

**Detail.** DevCouncil is used to improve DevCouncil. A full-codebase review produced a prioritized **P0–P3 backlog (reliability, testing, architecture, performance)** that was driven **fully closed**: universal subprocess timeouts, **atomic state writes**, rate-limit retries, and a decomposition of two god-modules — `verifier.py` **2,059 → ~390 lines** and the MCP `server.py` **~1,790 → ~300 lines** — into per-gate checks and per-tool handlers, all landing on a **green 1,385-test suite** with a diff↔coverage gate the product **dogfoods on itself**. A dedicated **log-audit** of `.devcouncil/logs/` caught a real bug where `dev go`'s final report crashed *after* completing work — silently mislabeling passing benchmark tasks as "incomplete" and reading verdict calibration as 0%. Fixing it is why the effectiveness numbers can be trusted.

**Resume bullet.** *Ran a self-hosted P0–P3 codebase review to closure — universal subprocess timeouts, atomic writes, and decomposing 2,059- and 1,790-line god-modules to ~390/~300 — on a green 1,385-test suite, and log-audited telemetry to catch a benchmark-mislabeling bug.*

---

## Bonus points (extra material to distill)

- **Open Knowledge Format (OKF).** A portable knowledge layer (`knowledge/okf.py`, `skill_bridge.py`) that exports/ingests project knowledge, renders a bundle as a **self-contained static HTML site** (`dev okf html`), and bridges bidirectionally to engineering skills — then injects that knowledge back as planning/coding context.
- **Design-system gate (`dev design check`).** A CI-friendly gate that fails the build on hardcoded color/spacing/typography literals that bypass design tokens (`knowledge/design_conformance.py`) — codified design governance.
- **Live review (`dev watch`).** Real-time session review with cards, signals, and blocking behavior (`live/`), so a human (or reviewer model) can gate an agent mid-session.
- **Local-monitor safety guardrails.** Calibration probes showed a single-shot local reviewer rubber-stamped **1/6** buggy criteria, while **samples=3 + per-criterion caught 6/6 with zero false passes**; unsafe reviewer configs now warn (deduped via `warn_once`) and surface in `dev doctor`.
- **Cost telemetry & budgets.** Per-call cost/latency tracking with run-ID attribution and `dev cost budget` spend caps that warn before a plan/repair loop overruns.
- **Security model.** Secret redaction, permission allowlists, and local-only state (`.devcouncil/`), so evidence and transcripts never leave the machine.
- **Breadth of CLI surface.** ~48 `dev` subcommands (`plan`, `run`, `verify`, `repair`, `go`/`e2e`, `rollback`, `watch`, `doctor`, `okf`, `design`, `integrate`, `mcp-server`, `gaps`, `provenance`, `cost`, …) — a genuinely productized tool, not a script.

## Skills / keyword bank (for the resume header & portfolio tags)

`AI orchestration` · `LLM evaluation & verification` · `multi-agent debate / council` · `agentic workflows` · `MCP (Model Context Protocol)` · `Python` · `asyncio` · `Typer CLI` · `tree-sitter / AST analysis` · `static analysis & code intelligence` · `model routing` · `local LLMs / Ollama` · `test & coverage gating` · `benchmark design & calibration` · `evidence graphs / provenance` · `offline-first` · `software architecture / refactoring`

## What's measured vs. illustrative (defend only what's real)

- **Hard, defensible numbers:** **+0.14** correctness lift (gated **0.94** vs. raw **0.81**, **0 false negatives**, ~$8.49 planning cost) on the latest adversarial run (GLM-5.2 planner, Claude Sonnet executor); **1,385-test** suite green; god-module reductions **verifier.py 2,059→~390** and **server.py ~1,790→~300** (per `IMPROVEMENTS.md`, "file refs verified against source," and confirmed by line count); local-monitor probe **6/6 vs. 1/6** buggy-criteria detection (samples=3+per-criterion vs. single-shot); **4** model providers; ~**48** CLI commands.
- **Frame honestly:** the **+0.14** figure is from a small, deliberately adversarial **3-task** suite — describe it as "a controlled benchmark showing the gated loop improves correctness on hidden edge-case tests," not a universal accuracy claim. Agents are stochastic; the harness supports `--repeats` for variance, and the README documents its own bias caveats (small self-contained tasks, hidden-suite strictness, reviewer-model sensitivity).
- **Relative, not absolute:** per the benchmark's own methodology notes, the trustworthy signal is arm-to-arm comparison, not the absolute scores.

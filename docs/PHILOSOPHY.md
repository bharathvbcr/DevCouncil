# DevCouncil Engineering Philosophy

## 1. Thesis

**Deterministic truth and modular engines before the edit.**

*Waylode is where you work. Manvi runs the agent. DevCouncil knows the code and checks the result.* DevCouncil supplies standalone libraries, stores, and fail-closed verification gates that report what the repository and tools actually established, including everything they could not verify.

## 2. Core Principles

1. **Answer before the edit, prove after.**
   Before an agent or engineer writes code, `devmap preview`, `devmap impact`, and `devmap affected` (`rust/devmap-serve/src/mcp.rs`) expose which callers, dependents, and tests a change will touch. After the edit, `dcverify` (`rust/dc-verify/src/rigor.rs`), `backend/go_orchestrator/devcouncil/verify/orchestrate.go`, and `devcouncil_verify_task` judge the resulting diff against planned scope, AST stub/skip checks, and changed-line coverage rather than trusting self-reported completion.

2. **Honest uncertainty over false confidence.**
   Static analysis and bounded graph walks always have blind spots. Every DevMap query surfaces `truncated`, `walk_incomplete`, `unattributed`, `unresolved_namesakes`, and per-row `confidence` (`0.9` vs `0.4` in `devmap dead --json`). An unavailable index, capped traversal, or unattributed symbol is reported explicitly as incomplete evidence — never conflated with "no callers", "unused code", or "clean".

3. **Fail closed, never launder a refusal or skip into a pass.**
   Policy and verification outputs keep degraded or overridden paths visibly distinct from clean passes. In `backend/go_orchestrator/policy/decision.go` and `backend/contracts/verdict.schema.json`, fields such as `degraded`, `demoted`, `grant_id`, and `widened` travel with every non-clean `allow` so a host posture downgrade, self-appended scope pattern, or missing repo map never poses as an unblemished rule pass. A skipped or timed-out check fails the gate rather than defaulting to `allow`.

4. **Code serves specifications and constitutional integrity.**
   Requirements, boundaries, data models, machine-readable contracts, and verification plans live under `specs/` in a 7-layer hierarchy (`backend/go_orchestrator/devcouncil/spec/`), governed by the Nine Articles in [`constitution.md`](../constitution.md) and enforced by `devcouncil spec validate <id> --strict`. When implementation and specification disagree, we fix the specification or repair the code against it rather than normalizing drift.

5. **Modular components taken one at a time.**
   DevCouncil builds focused binaries and libraries — `devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `dc-glob`, `dc-proc`, and Go `policy`/`gate`/`grants` — each with a single canonical owner and zero requirement to adopt the rest of the suite. Hosts compose only what they need: standalone coding CLIs consume `devmap` directly, `Manvi` imports the Go policy packages and task loop, and `Waylode` vendors the read-side Rust crates without `devmap-serve`.

6. **Local-first, zero-cloud, zero-write-on-read.**
   Code intelligence and task state run entirely on the developer's machine in SQLite (`.devmap/codeintel/devmap.sqlite`, `.devcouncil/state.sqlite`) with no account, telemetry, or model API key required. On read paths (`StackedStore::open` in `rust/devmap-query/src/stacked.rs`), MCP tools and CLI queries layer `delta.sqlite` edits over immutable `0444` base snapshots without writing a single byte to the database or lock sidecars. Every local `DevCouncil` binary (`devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `devcouncil pr-gate`, local `devmap federate`) and public OSS PR Gate is 100% free and open source (`MIT OR Apache-2.0`) under **Waylode Primary**; when teams opt into **Waylode Labs** (`gitpulse-relay`), only hosted infrastructure (private-repo hosted PR Gate, remote R2 `devmap cache` storage, and org cross-repo federation) requires sign-in or a subscription.

## 3. Deliberate Trade-offs & What We Refuse to Do

- **Two implementation languages instead of forced FFI everywhere:** Compute-heavy Tree-sitter extraction, SQLite CSR storage, and process-group reaping (`rust/dc-proc/`) stay in Rust, while policy decisions, spec parsing, and CLI orchestration (`backend/go_orchestrator/`) stay in Go so `Manvi` can import `policy`, `gate`, and `grants` natively. Cross-language in-process bridges (`gusset-engine`) are used only where microbenchmarks show crossing cost pays off (`docs/gusset-candidates.md`); elsewhere we keep explicit JSON-on-stdio boundaries.
- **No agent turn loop, model provider, or TUI in DevCouncil:** DevCouncil retired its legacy Python `ModelRouter` and `src/` application in Phase 7 and refuses to grow provider routing, LLM conversation loops, or terminal UIs — those belong exclusively in `Manvi`.
- **No desktop GUI, worktree manager, or PTY daemon:** DevCouncil owns store seeding (`devmap seed --stacked`), `StackedStore`, and `task_leases` schemas, while refusing to duplicate `Waylode`'s git client, worktree lifecycle, APFS cache cloner, collision UI, or `waylode-ptyd`.
- **Zero local paywalls, sign-in gates, or billing code:** DevCouncil refuses to put license checks, account sign-in (`requires_sign_in: false`), or telemetry into its engines, local CLIs (`devcouncil pr-gate`, `devmap`), or BYOK developer workflows; optional hosted R2 cache and private-repo CI services live exclusively outside this repository in `Waylode Labs`.
- **No unverified Python product runtime:** Remaining Python files under `benchmarks/`, `backend/contracts/tools/`, and hook templates are strictly offline evaluation, fixture generation, and host-hook shims — never shipped runtime dependencies.

## 4. How This Fits the Stack

DevCouncil provides the ground-truth engines and schemas underneath the rest of the stack. Ownership boundaries, parity gates, and open cross-repo overlap closures live in [`specs/003-stack-boundaries-and-non-overlap/spec.md`](../specs/003-stack-boundaries-and-non-overlap/spec.md) and [`docs/STACK.md`](STACK.md):

- **DevCouncil (this repository):** Knows the code, stores task leases and specs, defines policy rules, and checks diffs ([`docs/PHILOSOPHY.md`](PHILOSOPHY.md)).
- **Waylode (`../GitPulse`):** Local-first desktop workspace where engineers and agents inspect worktrees, collisions, and sessions ([`../../GitPulse/docs/PHILOSOPHY.md`](../../GitPulse/docs/PHILOSOPHY.md)).
- **Manvi (`../Manvi`):** Embeddable, local-model-first agent runtime executing turns under DevCouncil policy ([`../../Manvi/docs/PHILOSOPHY.md`](../../Manvi/docs/PHILOSOPHY.md)).

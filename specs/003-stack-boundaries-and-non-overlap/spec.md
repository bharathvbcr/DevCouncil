# Feature: 003 - Stack Boundaries and Non-Overlap

> **Status (2026-11-26)**: Canonical Owner Repo: `DevCouncil` (cross-repo contract binding `DevCouncil`, `Manvi`, and `Waylode` (`../GitPulse`)). Baseline boundaries documented in [`docs/STACK.md`](../../docs/STACK.md); cross-repo ownership matrix ratified in [`contracts/ownership-matrix.json`](contracts/ownership-matrix.json); active overlap-closure tasks tracked in [`tasks.md`](tasks.md).

## 1. Problem Statement & Motivation

`DevCouncil`, `Manvi`, and `Waylode` (formerly `GitPulse`) form a three-layer stack: reusable components/schemas (`DevCouncil`), agent runtime harness (`Manvi`), and local-first ADE desktop host (`Waylode`). Without a single machine-readable ownership contract:
1. Overlapping implementations accumulate across repos (for example, three host-wiring paths across `devmap integrate`, `dev integrate`, and Waylode Connect; duplicate schemas for the ~8 shared `devcouncil_*` tools in `Manvi`; host-plane `work.verify.task` re-checking diffs instead of calling `dcverify`).
2. Documentation and tool counts drift across repositories (`devmap` 21 upstream MCP tools vs Waylode's 11-tool subset vs Manvi's 3-tool `devcouncil_*` navigation facade).
3. Future features risk violating `constitution.md` Article V ("Single Source of Truth & Zero Duplication") by adding parallel copies instead of extending the canonical owner's seam.

**Three Philosophies, One Stack:** Each repository anchors its boundaries in an explicit philosophy document — [DevCouncil (`docs/PHILOSOPHY.md`)](../../docs/PHILOSOPHY.md), [Waylode (`../GitPulse/docs/PHILOSOPHY.md`)](../../../GitPulse/docs/PHILOSOPHY.md), and [Manvi (`../Manvi/docs/PHILOSOPHY.md`)](../../../Manvi/docs/PHILOSOPHY.md).

## 2. Canonical Ownership & Non-Overlap Boundary

### Ownership Partition Table

| Layer / Capability | Owns (Canonical Owner) | Delegates To | Non-Goals / Forbidden Overlaps |
|---|---|---|---|
| **DevCouncil** (`DevCouncil` repo) | `devmap` code intelligence (21 MCP tools, Schema v29 / extract `"102"`), `dc-store` SQLite schemas (`SCHEMA_VERSION` 11, `WORKBENCH_SCHEMA` 17), `dc-verify` / `dcverify`, `policy` / `gate` / `grants`, 8 shared `devcouncil_*` task/policy tool contracts (`backend/go_orchestrator/devcouncil/registry.go`), `devmap integrate` host writer (`rust/devmap-cli/src/integrate.rs`), `devcouncil pr-gate` CLI, local `devmap federate`, `contracts/` wire schemas & parity anchors | **Manvi** for LLM turn loops, provider routing, subagent harness, TUI, and host-plane `manvi serve` execution; **Waylode** (`Primary`) for desktop ADE UI, Git client, git worktree lifecycle, collision radar, PTY daemon, and system load governor; **Waylode Labs** (`gitpulse-relay`) for optional hosted R2 `devmap cache`, hosted private-repo `DevCouncil PR Gate`, and cross-repo org federation | Must never add an LLM turn loop, provider client, TUI, desktop GUI, git worktree manager, sign-in flow, or billing/paywall logic |
| **Manvi** (`Manvi` repo) | Agent turn loop, local-model-first provider routing, subagent roster/debate (`manvi/devcouncil/debate.go`), TUI & event stream, `manvi serve` NDJSON host plane ("manvi/serve/"), runtime enforcement of DevCouncil policy/gate/grants ("manvi/serve/policy.go"), `memguard`, Jarvis computer-use seam | **DevCouncil** for code graph (`devmap` via `backend/go_orchestrator/dc/devmap/warm.go`), policy/gate/grants rules, `dc-store` persistence, and deterministic diff/stub/coverage verification (`dcverify`); **Waylode** for desktop IDE/board UI, git ops, and process/worktree lifecycle; **Waylode Labs** for optional org-signed `policy_bundles` & relay approvals consumed outside local core | Must never fork DevCouncil policy rules, `dc-store` schemas, `devmap` indexing, or desktop worktree management; must never carry billing/paywall logic; must eliminate shadow verification & duplicate tool schemas |
| **Waylode** (`GitPulse` repo; `Primary` + `Labs` via private `GitPulse-labs` / `gitpulse-relay`) | **Waylode Primary** (Free & OSS, `MIT OR Apache-2.0`, zero sign-in): Desktop ADE (Tauri 2 + Svelte 5), Git client, worktree lifecycle & CoW cache sharing ("src-tauri/src/engine/worktree.rs"), collision radar & live sessions ("src-tauri/src/insights/"), `waylode-mcp`, `waylode-ptyd` (protocol 5), `waylode-hook`, load governor, Mavi thread UX, local BYOK compute. **Waylode Labs** (`FCL-1.0-MIT`, `free`/`pro`/`team`/`enterprise` wire IDs): hosted `gitpulse-relay` sync/vault, Space `task_sharing`, private-repo hosted PR Gate, shared R2 `devmap cache`, org `policy_bundles` | **DevCouncil** for canonical DevMap store seeding/querying (vendored read-side crates + `StackedStore`), `dc-store` schema & `contracts/`, `devcouncil pr-gate`, and host wiring (`devmap integrate`); **Manvi** for agent harness turns (`manvi serve` sidecar) and runtime policy judgment (`guard_command` -> `manvi serve`) | Must never vendor `devmap-serve`, invent a parallel `base_store_ref.json` / regex AST overlay, duplicate `devcouncil_checkout_task` / `dc-verify` logic, gate local/BYOK capabilities behind paywalls, or merge `labs/**` telemetry/billing into public `DevCouncil`/`Manvi`/`GitPulse` |

### Verified Overlap & Drift Ledger (2026-11-26 Code Audit)

| Capability | Canonical Owner (Verified Seam) | Verified Adapters & Current Overlaps | Status |
|---|---|---|---|
| `devmap` Code Intelligence | DevCouncil: `rust/devmap-serve/src/mcp.rs` (`TOOLS` / `TOOL_NAMES`: 21 tools), `rust/devmap-store/src/schema.rs` (`CURRENT_SCHEMA_VERSION` 29), `rust/devmap-extract/src/cache.rs` (`EXTRACTION_SCHEMA_VERSION` `"102"`) | Waylode vendors read-side `devmap-*` (`src-tauri/vendored/`), omits `devmap-serve`, serves 11-tool subset (`src-tauri/src/mcp/devmap_parity.rs`), verified by `scripts/check-schema-parity.mjs`. Manvi wraps stdio `devmap mcp` (`backend/go_orchestrator/dc/devmap/warm.go`) behind 3 harness tools (`devcouncil_graph_query`, `devcouncil_code_dead`, `devcouncil_graph_context`) | Intentional subset adapters; schema parity automated |
| Task Verification Rigor | DevCouncil: `rust/dc-verify/`, `backend/go_orchestrator/dc/verify_actions.go`, `backend/go_orchestrator/devcouncil/verify/orchestrate.go`, `devcouncil_verify_task` in `backend/go_orchestrator/devcouncil/registry.go` | **Closed (`OV-VERIFY-01`)**: Manvi `"manvi/serve/verify_checks.go:237-258"` (`VerifyRunner.runRigor`) delegates path/rigor verification to `dcverify` while `"manvi/serve/verify.go:167-422"` owns async job orchestration; `"manvi/devcouncil/rigor.go"` now delegates rigor wording and category routing to `dc.RigorActionText` / `dc.CategoryForGapType` in `backend/go_orchestrator/dc/verify_actions.go` (preserving `Manvi`'s `dc/dcverify` import ban). | Closed (`T-006`) |
| Task & Lease State | DevCouncil: `rust/dc-store/src/lib.rs` (`SCHEMA_VERSION` 11), `rust/dc-store/src/workbench/mod.rs` (`WORKBENCH_SCHEMA` 17), `backend/go_orchestrator/dc/store/` | Manvi `"manvi/serve/"` exposes `work.*` workbench client ops; Waylode vendors `dc-store` read-only and keeps `"src-tauri/src/tasks/file_tasks.rs"` strictly for Markdown briefs | Clean boundary |
| Policy Decisions, Gates & Grants | DevCouncil: `backend/go_orchestrator/policy/`, `backend/go_orchestrator/gate/gate.go`, `backend/go_orchestrator/grants/`, `rust/dc-glob/` | Manvi `"manvi/serve/policy.go"` enforces DevCouncil rules under `PostureDevCouncil` / `host-scope` posture. Waylode `"src-tauri/src/harness/mod.rs"` (`guard_command`) routes checks through `manvi serve` | Clean decision vs enforcement vs hook split |
| Shared `devcouncil_*` MCP Tools | DevCouncil: `backend/go_orchestrator/devcouncil/registry.go` defines the 8 canonical task/policy MCP tools (`devcouncil_get_diff`, `devcouncil_checkout_task`, `devcouncil_renew_lease`, `devcouncil_release_task`, `devcouncil_next_task`, `devcouncil_verify_task`, `devcouncil_get_gaps`, `devcouncil_policy_check_write`) | **Overlap**: Manvi `"manvi/devcouncil/tools.go"` re-declares ~8 shared task/policy tool schemas alongside its ~46 harness-only tools (`devcouncil_read_file`, `devcouncil_exec_command`, `devcouncil_spawn_subagents`, etc.) | Open closure (`T-007`) |
| Agent Runtime & Turn Loop | Manvi: `"manvi/cmd/manvi/turnloop.go"`, `"manvi/agents/"`, `"manvi/session/"`, `"manvi/devcouncil/debate.go"`, `"manvi/serve/"` | DevCouncil has zero turn loop / provider code (retired Python `ModelRouter` in Phase 7). Waylode invokes `manvi serve` over NDJSON (`ManviConnection.svelte`) | Clean boundary |
| Desktop ADE, Git, Worktrees & PTY | Waylode: `"src-tauri/src/engine/worktree.rs"`, `"cow_clone.rs"`, `"insights/entity_collision.rs"`, `"bin/waylode-ptyd.rs"`, `"bin/waylode-mcp.rs"`, `"sysload/"` | DevCouncil owns `devmap seed [--stacked]`, `StackedStore`, and `task_leases` metadata only (`specs/002-zero-copy-worktrees/`) | Clean boundary (legacy `base_store_ref.json` retired) |
| Host Integration / MCP Wiring | DevCouncil: `rust/devmap-cli/src/integrate.rs`, `rust/devmap-cli/src/skills.rs`, `rust/devmap-cli/src/agent_install.rs` (`devmap integrate`) | **Partial Closure**: `backend/go_orchestrator/devcouncil/integrate/integrate.go` (`dev integrate`) delegates MCP & `.agents/` writes to `devmap integrate --servers-stdin`, keeping only Cursor rule + retired-host messages. **Overlap**: Waylode Connect (`"src-tauri/src/agent_connect.rs"`, `"workbench/agy_plugin.rs"`, `"workbench/claude_hook.rs"`) still manages host configs separately | Open closure (`T-008`) |
| Skills & Agent Guidance | Each product repo owns its own product skills: DevCouncil (`rust/devmap-cli/skills/`, `backend/go_orchestrator/devcouncil/skills/library/`), Waylode (`"plugins/waylode/skills/"`: 9 Waylode-only skills, 0 plugin overlap), Manvi; checked by `scripts/sync-agent-defs.mjs` & `shipped_skills` | **Clean in plugins / Dogfooding in `.agents/`**: `"plugins/waylode/skills/"` has 0 overlap with DevCouncil skills; repo-local `"GitPulse/.agents/skills/"` holds 23 dogfooding skills (`devmap*`, `devcouncil*`, library); remaining work enforces automated guard + installer boundary documented in `docs/DEVMAP_SKILL_DELIVERY.md` and `docs/STACK.md` | Tracked closure (`T-009`) |
| Lifecycle Hooks | DevCouncil: `rust/devmap-cli/src/hook.rs`, `backend/go_orchestrator/cmd/devcouncil/hook.go` (index warm + stop gate). Waylode: `"src-tauri/src/hooks/"`, `"waylode-hook"` / `"gitpulse-hook"` (IDE/CLI notify, build lease wrapper) | Manvi enforces harness-side tool & command hooks inside `"manvi/serve/policy.go"` and `"manvi/cmd/manvi/"` | Distinct planes; documented |
| Wire Contracts & Schema Parity | DevCouncil: `backend/contracts/` (`CHECKSUMS`, `event.schema.json`, `lease.schema.md`, `verdict.schema.json`, `verdict.cases.json`, `tools/`), `rust/devmap-serve/src/mcp.rs` | Mirrored read-only to Manvi `"contracts/"` (`manvi/internal/contract/` tests) and Waylode `"contracts/"` (`scripts/check-schema-parity.mjs`, `scripts/vendor-crates.mjs`, `scripts/check-release.mjs`) | Automated parity gates |
| Hosted Labs Extensions & Public/Private Edition Split | DevCouncil: all local tools (`devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `devcouncil pr-gate`, local `devmap federate`, `devmap cache pack\|unpack\|status`) are 100% free & OSS (`MIT OR Apache-2.0`, `requires_sign_in: false`) | **Waylode Labs** (`gitpulse-relay`, `FCL-1.0-MIT`, `pro`/`team`/`enterprise` wire IDs): hosts private-repo `DevCouncil PR Gate`, remote R2 `devmap cache` (`push`/`pull`), org cross-repo federation, Space `task_sharing`, relay approvals, and `policy_bundles` (`requires_sign_in: true` + Ed25519 signing) | Clean Primary vs Labs boundary |

## 3. User Stories & Acceptance Scenarios

### US1: Canonical Ownership Contract Across All Three Repositories (Priority: P1)
As a maintainer or autonomous coding agent working across `DevCouncil`, `Manvi`, and `Waylode`,
I want a single canonical, machine-readable ownership matrix linking every cross-repo capability to its owner, approved adapters, and active overlap closures,
So that new code extends the right repository seam instead of duplicating engines or schemas.

- **Scenario 1: Single Owner Resolution**
  - **Given** a capability in `contracts/ownership-matrix.json` (such as `code_intelligence`, `policy_decisions`, `agent_harness`, or `desktop_worktree_ade`)
  - **When** an engineer or verification audit inspects the capability entry
  - **Then** `canonical_owner` names exactly one repository (`DevCouncil`, `Manvi`, or `Waylode`), `owner_seams` cites verified file paths, and `forbidden_overlaps` explicitly states what sibling repositories must not reimplement.
- **Scenario 2: Cross-Repo Parity Anchors**
  - **Given** `DevCouncil` exports `backend/contracts/` schemas, `rust/devmap-serve/src/mcp.rs` (`TOOL_NAMES`), `CURRENT_SCHEMA_VERSION`, `SCHEMA_VERSION`, and `WORKBENCH_SCHEMA`
  - **When** `Manvi` and `Waylode` synchronize vendored crates, Go pins, or schema parity checks
  - **Then** downstream repositories mirror `DevCouncil` schemas read-only and verify lockstep versions via `check-schema-parity.mjs` and Go contract tests without diverging local schema versions.

### US2: Elimination of Active Cross-Repo Overlaps (Priority: P1)
As a platform architect enforcing `constitution.md` Article V,
I want every currently verified code overlap (`work.verify.task` vs `dcverify`, shared `devcouncil_*` tool schemas, Waylode Connect vs `devmap integrate`, and cross-repo skill bundling) tracked with concrete file seams and closure tasks,
So that overlaps are retired cleanly in place rather than fossilizing as permanent parallel implementations.

- **Scenario 1: Verification Rigor Delegation**
  - **Given** `Manvi` exposes `work.verify.task` in its host plane while `DevCouncil` owns `dcverify` and `backend/go_orchestrator/devcouncil/verify/orchestrate.go`
  - **When** the verification overlap closure task executes
  - **Then** `work.verify.task` retains only durable job claiming and orchestration while delegating diff collection, stub/assertion AST checks, and coverage scoring to `dcverify` / DevCouncil's verify engine.
- **Scenario 2: Unified Host Wiring Through `devmap integrate`**
  - **Given** `backend/go_orchestrator/devcouncil/integrate/integrate.go` already delegates host table writes and `.agents/` installation to `rust/devmap-cli/src/integrate.rs`
  - **When** Waylode Connect closes its host-wiring overlap
  - **Then** desktop Connect operations invoke `devmap integrate` rather than maintaining a second independent host-config table and settings merger.

### US3: Zero Documentation and Terminology Drift Across Active Specs (Priority: P2)
As an operator onboarding onto the stack,
I want `DevCouncil`, `Manvi`, and `Waylode` documentation and specifications to share consistent product naming (`Waylode`, preserving deliberate compatibility identifiers) and accurate capability counts,
So that neither agents nor humans act on stale `GitPulse` product names, retired Python references, or contradictory tool counts.

- **Scenario 1: Naming & Identifier Discipline**
  - **Given** active documentation and 7-layer specs across `DevCouncil`, `Manvi`, and `Waylode`
  - **When** `devmap doc-drift --json` and `devcouncil spec validate <id> --strict` execute
  - **Then** zero blocking or advisory validation errors occur, prose uses canonical `Waylode` casing, and legacy technical tokens (`gitpulse-mcp`, `.gitpulse/`, `WAYLODE_MCP_EXPOSE_LEGACY`) are marked explicitly as compatibility identifiers.

### US4: Zero Local Paywalls Across Waylode Primary and Waylode Labs (Priority: P1)
As a developer or organization running `DevCouncil`, `Manvi`, and `Waylode`,
I want local engines and BYOK compute to remain 100% free and sign-in-free under `Waylode Primary`, while `Waylode Labs` gates only hosted `gitpulse-relay` infrastructure and org governance,
So that no artificial paywall, account gate, or billing code ever enters `DevCouncil` or local OSS workflows.

- **Scenario 1: Local DevCouncil & BYOK Free Boundary**
  - **Given** an engineer running `devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `dc-glob`, `devcouncil pr-gate`, local `devmap federate`, or `Manvi` / `Waylode Primary` on local hardware or user-paid BYOK compute
  - **When** any local command executes
  - **Then** zero sign-in, license key, or subscription check runs (`requires_sign_in: false`, `paywalled: false`), and public open-source PR gates stay free.
- **Scenario 2: Hosted Labs Relay Extensions**
  - **Given** an operator opting into `Waylode Labs` (`pro` `Labs Geek`, `team` `Labs Geek Squad`, or `enterprise`) over `gitpulse-relay` (`FCL-1.0-MIT`)
  - **When** using hosted private-repo `DevCouncil PR Gate`, remote R2 `devmap cache push|pull`, or org cross-repo `devmap` federation
  - **Then** billing and Ed25519-authenticated relay endpoints stay isolated in private `GitPulse-labs` / `gitpulse-relay` without modifying `DevCouncil`'s open-source codebase or local CLI contracts.

## 4. Success Metrics & Testability Criteria

- **SC-001**: `devcouncil spec validate 003-stack-boundaries-and-non-overlap --strict --json --project-root /Users/bharath/Code/devtools/DevCouncil` exits `0` with `valid: true` and `findings: null` (zero blocking and zero advisory findings).
- **SC-002**: `contracts/ownership-matrix.json` is valid JSON covering all 12 capability domains and edition boundaries (`Waylode Primary` + `Waylode Labs` wire IDs `free`/`pro`/`team`/`enterprise`) with `canonical_owner` restricted to `["DevCouncil", "Manvi", "Waylode", "Waylode Labs"]`.
- **SC-003**: `devmap doc-drift --json specs/003-stack-boundaries-and-non-overlap` reports `findings: []` (`total: 0`).
- **SC-004**: Sibling repo doc leads (`@manvi-docs` and `@waylode-docs`) confirm alignment against `contracts/ownership-matrix.json` with zero cross-repo ownership or edition contradictions reported by `@verifier`.

## 5. Edge Cases, Exclusions & Open Questions

- **Compatibility Exception (`parse_git_version` & `file_tasks.rs`)**: Waylode keeps minimal git binary preflight parsing (`parse_git_version` in `src-tauri/src/engine/git_cli/preflight.rs`) and local Markdown task brief reading (`tasks/file_tasks.rs`); neither is an overlap with `dc-store` SQLite ownership because neither mutates task leases or workbench schemas.
- **Dual-Prefix MCP Transport**: `waylode-mcp` (and legacy binary `gitpulse-mcp`) exposes canonical `waylode_*` tools by default (`WAYLODE_MCP_EXPOSE_LEGACY=1` opts in `gitpulse_*` aliases), plus 11 read-only `devmap_*` subset tools checked against `rust/devmap-serve/src/mcp.rs::TOOL_NAMES`; standalone `devmap mcp` remains the single owner of all 21 `devmap_*` tools.
- **Relay Crypto & Public-Repo Hygiene**: Labs relay features handling encrypted user data (`device_sync`, `secrets_vault`, `team_killswitch`, `markdev-sync`) remain gated on external E2EE crypto review; `labs/**` and `gitpulse-relay` remain outside public `DevCouncil`, `Manvi`, and `GitPulse` repositories. Public docs must never link private/gitignored Labs planning files.
- **Exclusion**: Cross-repo closure implementation tasks (`T-006`..`T-009`) require code PRs in `DevCouncil`, `Manvi`, and `Waylode`; this spec establishes the ratified boundary contract and tracks those open engineering tasks without modifying Go/Rust/Svelte product source during the docs pass.

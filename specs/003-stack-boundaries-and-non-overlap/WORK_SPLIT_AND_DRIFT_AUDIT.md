# Full-Stack Drift Audit & 4-Lane Work Split

> **Status (2026-11-26, Phase 6)**: Consolidated from verified Wave A audits and updated after Phase 6 Waves 1–2 across `DevCouncil`, `Manvi`, `Waylode` (`../GitPulse`), and `waylode-labs` (`.gitpulse/worktrees/waylode-labs`). Enforced by [`scripts/check-stack-boundaries.mjs`](../../scripts/check-stack-boundaries.mjs) against [`contracts/ownership-matrix.json`](contracts/ownership-matrix.json).

## 1. Consolidated Overlap, Drift & Gap Findings

### A. Cross-Repo Overlaps (`OV-*`)

| ID | Repos & Verified `path:line` Evidence | Exact Scope & Status | Closure Task |
|---|---|---|---|
| **OV-VERIFY-01** | `DevCouncil` `backend/go_orchestrator/dc/verify_actions.go` + `backend/go_orchestrator/devcouncil/verify/{next_actions,rigor,types}.go` vs. `Manvi` `"manvi/devcouncil/rigor.go"` | **Closed (Phase 6)**: `DevCouncil` exports canonical `RigorActionText`, `GapTypeForRigorGate`, `CategoryForGapType`, and `KnownGapTypes` in cycle-free `package dc` (preserving `Manvi`'s `dc/dcverify` import ban in `"manvi/devcouncil/zadv_b29_residue_test.go"`); `"Manvi:manvi/devcouncil/rigor.go"` delegates `"gapsFrom"` action wording and category routing to `dc.*` while preserving `"Gap.Type = f.Gate"`. | `T-006` (Done) |
| **OV-TOOLS-01** | `DevCouncil` `backend/go_orchestrator/dc/toolspecs.go` + `devcouncil/registry.go` vs. `Manvi` `"manvi/devcouncil/{tools,policy_tools}.go"` | **Closed (Phase 6)**: `DevCouncil` exports cycle-free `SharedToolSpec`, `SharedToolBehaviour`, `SharedToolSpecs()`, `SharedToolNames()`, and `SharedToolSpecByName(name)` in `package dc`; `devcouncil/registry.go` builds directly from `dc`; `Manvi` validates all 8 shared tools via `sharedTool` -> `dc.SharedToolSpecByName(...)` (documenting session-bound omitted args `client_id`, `lease_token`) and supports `operation` (`"create" \| "modify" \| "delete"`) on `devcouncil_policy_check_write`. | `T-007` (Done) |
| **OV-INTEGRATE-01** | `DevCouncil` `rust/devmap-cli/src/integrate.rs` & `backend/go_orchestrator/devcouncil/integrate/integrate.go` vs. `Waylode` `"src-tauri/src/agent_connect.rs"`, `"src-tauri/src/devmap/integrate.rs"`, `"workbench/agy_plugin.rs"`, `"workbench/claude_hook.rs"` | **Refined / Open (`T-008`)**: Host-CLI preflight/status probing (`IntegrationHost`, `HostCliStatus`, `probe_host_cli_with`, `probe_host_clis_with`) is now unified between `src-tauri/src/devmap/integrate.rs` and `src-tauri/src/agent_connect.rs` (`GAP-HOST-01` closed). App-level user-global CLI registration (`claude mcp add --scope user`, `codex mcp add`, `~/.local/bin` links, legacy `gitpulse` cleanup) remains in `agent_connect.rs` until `devmap integrate` adds `--servers-only --scope user`. | `T-008` |
| **OV-SKILLS-01** | `DevCouncil` `rust/devmap-cli/skills/` (5 skills), `backend/go_orchestrator/devcouncil/skills/library/` (18 skills), `rust/devmap-cli/src/skills.rs` vs. `Waylode` `"plugins/waylode/skills/"` (9 skills) & `"GitPulse/.agents/skills/"` (23 skills) | **Partial (`T-009`)**: `"plugins/waylode/skills/"` ships 9 Waylode-only skills (`collisions`, `commit-and-merge`, `insights`, `load`, `merge-prs`, `merge-train`, `tasks`, `terminal`, `wrap-up`) with zero overlap against DevCouncil's 5 + 18 skills (checked by `scripts/check-stack-boundaries.mjs`). Automated CI installer separation remains tracked in `T-009`. | `T-009` |

### B. Code-vs-Spec & Pin Drift (`DRIFT-*`)

| ID | Repo & Evidence (`path:line`) | Discrepancy | Resolution Status |
|---|---|---|---|
| **DRIFT-PIN-01** | `Manvi` `"scripts/module-pins.txt:8"` (`63ca82ea...`) & `"gusset"` (`19383ad1...`) vs. `"manvi/go.mod:19-21"` & `DevCouncil` `HEAD` | Local builds resolve via `../` workspace checkouts while release pins lag `DevCouncil` `HEAD` (must be bumped after Phase 6 commits land). | **Open** — `T-012` (Post-commit pin bump) |
| **DRIFT-VENDOR-01** | `Waylode` `"src-tauri/vendored/VENDOR.json:10"` (`84b97f0b...`) vs. `DevCouncil` `HEAD` | Vendored DevCouncil crates lag `HEAD` (schema parity green at `29`/`11`/`17`/`102`/`2`; re-vendor after Phase 6 commits land). | **Open** — `T-012` (Post-commit re-vendor) |
| **DRIFT-LABS-01** | `Waylode Labs` `".gitpulse/worktrees/waylode-labs/src-tauri/src/features/registry.rs"` vs. `"specs/003-primary-and-labs-editions/contracts/edition-boundary.schema.json"` | Set `is_labs: false` on the 5 `Plan::Free` (`paywalled: false, requires_sign_in: false`) local/BYOK features (`external_task_sources`, `email_task_sync`, `cloud_vms`, `policy_bundles`, `audit_export`) and removed the dead `"policy_bundles" \| "audit_export"` `"LockedSignInRequired"` arm. | **Closed (Phase 6)** — `T-013` (Lane D) |
| **DRIFT-ALIAS-01** | `DevCouncil` `backend/go_orchestrator/devcouncil/components/catalog.go:38-88`, `package.json:38`, `backend/go_orchestrator/cmd/devcouncil/main.go:71-100` | Every binary ships 2+ aliases (`devcouncil`/`devc`/`dev`, `dc*`/`devc-*`/`dc-*`); npm `dev` alias collides conceptually with `Jarvis/cmd/dev/`. | Documented in `specs/004`; stabilize aliases in release hygiene (`T-012`) |
| **DRIFT-EX-01 / DRIFT-DOC-01** | `Waylode` `"src-tauri/src/mcp/devmap_parity.rs:1"`, `"harness/mod.rs:3"`, `"mavi/mod.rs:3"`, `"workbench/claude_hook.rs:1"` | Updated pre-rename `GitPulse` module doc comments to `Waylode`; `11/21` parity subset and `parse_git_version` remain verified in sync. | **Closed (Phase 6)** — Lane C |
| **DRIFT-INDEX-01** | `Manvi` `devmap build --repo-map` (generation `#817`) | Refreshed `Manvi` repo-map via `devmap build --repo-map`. | **Closed (Phase 6)** — Lane B |

### C. Missing Seams & Architectural Gaps (`GAP-*`)

| ID | Repo & Evidence (`path:line`) | Gap Summary & Status | Target Lane |
|---|---|---|---|
| **GAP-DIST-01** | `DevCouncil` `dist-workspace.toml:44-46`, `rust/{dc-store,dc-verify,dc-grep,dc-redact-cli}/Cargo.toml`, `scripts/install.sh:23-38,66` | **Open (`T-012`)**: `cargo-dist` ships only `devmap-cli` (`dist = false` on sibling Rust tools); `scripts/install.sh` requires `go` just to compile `cmd/dcinstall` even for `--only=devmap`. | Lane A (`T-012`) |
| **GAP-GO-IMPORT-01** | `DevCouncil` `backend/go_orchestrator/dc/{verify_actions,toolspecs}.go` + `Manvi` `manvi/devcouncil/{rigor,tools,policy_tools}.go` | **Closed (Phase 6)**: `Manvi` now imports `dc.RigorActionText`/`dc.GapTypeForRigorGate`/`dc.CategoryForGapType` and `dc.SharedToolSpec*` from cycle-free `package dc` while keeping `dc/dcverify` unimported. | Lane A + Lane B (`T-006`, `T-007`) |
| **GAP-DELTA-EXTRACT-01** | `DevCouncil` `rust/devmap-store/src/delta.rs` & `rust/devmap-query/src/stacked.rs` (`StackedStore::delta_covers`) | **Closed (Phase 6)**: `delta.sqlite` persists `extraction_schema_version` (`devmap_extract::cache::EXTRACTION_SCHEMA_VERSION`) in `delta_meta` (nullable fallback on pre-existing read-only deltas) and `StackedStore::delta_covers` verifies it. | Lane A (`specs/004` `T-007`) |
| **GAP-HOST-01** | `Waylode` `"src-tauri/src/agent_connect.rs"` & `"src-tauri/src/devmap/integrate.rs"` | **Closed (Phase 6)**: Unified shared host-CLI preflight/status seam (`IntegrationHost`, `HostCliStatus`, `probe_host_cli_with`, `probe_host_clis_with`) across `agent_connect.rs` and `devmap/integrate.rs` (user-global registration remains in `agent_connect.rs` pending `OV-INTEGRATE-01`). | Lane C (`T-008`) |
| **GAP-LABS-01** | `Waylode Labs` `".gitpulse/worktrees/waylode-labs/src-tauri/src/features/registry.rs"` | **Closed (Phase 6)**: Added `task_sharing_inbox` (`Plan::Free`, `is_labs: false`, `paywalled: false`, `requires_sign_in: true`), `org_policy_bundles` (`Plan::Enterprise`, `is_labs: true`, `paywalled: true`, `requires_sign_in: true`), and `central_audit_export` (`Plan::Enterprise`, `is_labs: true`, `paywalled: true`, `requires_sign_in: true`). | Lane D (`T-013`) |
| **GAP-LABS-BUNDLE-01 / APPROVAL-01 / SPEND-01** | `Manvi` `"manvi/serve/protocol.go:155-186"`, `"manvi/serve/policy.go:160-260"`, `"manvi/cmd/manvi/approve_fd*.go"`, `"manvi/session/log.go:224"` | **Open (`T-013`)**: `manvi serve` handles local `"HostScope"` and local `"CostNanoUSD"` cleanly, but still lacks schema-v1.x host-plane RPC ops for Enterprise `policy_bundles`, remote `gitpulse-relay` command approval, and Space spend rollups. | Lane B -> Lane D (`T-013`) |

### D. Verified Clean Invariants Across All Repositories

- **Zero billing / sign-in / paywall leakage**: Zero Stripe/billing/account-gate code exists in `DevCouncil` non-test code (`dc-verify`/`dc-redact` hold secret-scanner regexes only) or `Manvi` (`"CostNanoUSD"`/`"StallCost"` only).
- **Zero unauthorized engine clones in Waylode**: Zero turn-loop, stub-checker, or second SQLite `dc-store` DDL implementations exist in `"Waylode:src-tauri/"`.
- **Deterministic honesty & `CGO_ENABLED=0` parity**: `DevCouncil` `rust/devmap-serve/src/mcp.rs:1117-2286` emits explicit `truncated`/`walk_incomplete` metadata; `Manvi` passes both `CGO_ENABLED=0` `fnmatch` fallback and `gusset-check`, and `PathReport.Passed()` / `"settleVerdict"` fail closed cleanly.

---

## 2. Four-Lane Non-Overlapping Work Split

| Lane | Repo / Edition | Exclusive `owned_paths` | `forbidden_paths` | Upstream Handoff Dependencies |
|---|---|---|---|---|
| **Lane A** (`lane-a-devcouncil`) | `DevCouncil` (`Primary OSS`) | `rust/`, `backend/go_orchestrator/`, `backend/contracts/`, `specs/`, `docs/`, `scripts/`, `constitution.md`, `dist-workspace.toml`, `package.json` | `Manvi:*`, `Waylode:*`, `Waylode Labs:*` | None (root upstream owner) |
| **Lane B** (`lane-b-manvi`) | `Manvi` (`Primary OSS`) | `manvi/`, `native/`, `contracts/`, `specs/`, `docs/`, `scripts/` | `DevCouncil:rust/`, `DevCouncil:backend/go_orchestrator/`, `DevCouncil:backend/contracts/`, `Waylode:*` | `DevCouncil` `backend/contracts/`, `policy`/`gate`/`grants`/`dc`, `rust/dc-*` |
| **Lane C** (`lane-c-waylode-primary`) | `Waylode` (`Primary OSS`, public `GitPulse`) | `src/`, `src-tauri/src/`, `src-tauri/crates/`, `src-tauri/vendored/`, `plugins/`, `contracts/`, `specs/`, `docs/`, `wiki/`, `scripts/` | `src-tauri/vendored/devmap-serve/`, `DevCouncil:*`, `Manvi:*`, `Waylode Labs:labs/`, `Waylode Labs:src-tauri/src/features/registry.rs` | `DevCouncil` `backend/contracts/`, read-side `devmap-*` & `dc-*`; `Manvi` `"contracts/serve.ops.schema.json"` |
| **Lane D** (`lane-d-waylode-labs`) | `Waylode Labs` (`FCL-1.0-MIT`, private `GitPulse-labs` / `gitpulse-relay`) | `src-tauri/src/features/`, `src-tauri/src/labs/`, `src/lib/account/`, `gitpulse-relay/` | `DevCouncil:*`, `Manvi:*`, `Waylode:src-tauri/vendored/`, `Waylode:specs/001-local-first-ade-and-worktrees/` | `Waylode` `"edition-boundary.schema.json"`, `DevCouncil` `ownership-matrix.json` & `cache.rs`, `Manvi` `"serve/policy.go"` |

---

## 3. Cross-Layer Landing Order & Overlap-Closure Backlog

**Strict Landing Order:** `DevCouncil` (Lane A) $\rightarrow$ `Manvi` (Lane B) $\rightarrow$ `Waylode Primary` (Lane C) $\rightarrow$ `Waylode Labs` (Lane D).

1. **Step 1 — Lane A (`DevCouncil`)**: Export cycle-free verify types (`backend/go_orchestrator/devcouncil/verify/`) and shared 8-tool Go specs (`backend/go_orchestrator/dc/`, closing half of `OV-VERIFY-01` & `OV-TOOLS-01`), add delta extract version (`specs/004` `T-007`), expose JSON plan/apply on `devmap integrate` (`T-008`), and expand standalone `cargo-dist` packaging (`T-012` / `GAP-DIST-01`).
2. **Step 2 — Lane B (`Manvi`)**: Advance `"Manvi:scripts/module-pins.txt"` (`T-012` / `DRIFT-PIN-01`), switch `"manvi/devcouncil/{verify,rigor}.go"` & `"tools.go"` onto Lane A's canonical Go packages (`T-006`, `T-007`), and add typed `manvi serve` host-plane hooks for remote approval, spend rollups, and signed `policy_bundles` (`GAP-LABS-*`).
3. **Step 3 — Lane C (`Waylode Primary`)**: Re-vendor crates in `src-tauri/vendored/` (`DRIFT-VENDOR-01`, `T-012`), unify `agent_connect.rs` / `agy_plugin.rs` / `AgentIntegrationPanel.svelte` onto `devmap integrate` (`T-008` / `GAP-HOST-01`), verify `plugins/waylode/skills/` boundary (`T-009`), and update minor pre-rename doc comments (`DRIFT-EX-01`).
4. **Step 4 — Lane D (`Waylode Labs`)**: Fix `features/registry.rs` `is_labs`/Primary-vs-Enterprise split & dead entitlement branch (`DRIFT-LABS-01`, `GAP-LABS-01`, `T-013`), wire `manvi serve` Labs ops, and complete external E2EE crypto review before enabling `device_sync`, `secrets_vault`, `team_killswitch`, or `markdev-sync`.

Query live lane assignment and boundary checks at any time with:

```bash
node scripts/check-stack-boundaries.mjs
node scripts/check-stack-boundaries.mjs --lanes
node scripts/check-stack-boundaries.mjs --route DevCouncil:rust/devmap-serve/src/mcp.rs
```

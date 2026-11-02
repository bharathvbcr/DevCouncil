# Implementation Plan: 001 - Realtime Sync

## 1. Technical Context
- **Primary Languages**: Go (orchestration and CLI dispatch) and Rust (high-performance event broker and state store)
- **Target Subsystems**: `backend/go_orchestrator/devcouncil/spec/`, `GitPulse/src-tauri/src/engine/`
- **Concurrency & Storage**: SQLite WAL persistence in `.devcouncil/state.sqlite` and Unix domain sockets for event streaming.

## 2. Constitution Check & Complexity Tracking
| Article | Status | Compliance Details / Justification |
| :--- | :--- | :--- |
| **Article I: Library-First** | PASSED | Sync engine implemented as standalone internal package before CLI exposure |
| **Article II: CLI Consumer** | PASSED | CLI commands support human-readable and deterministic `--json` envelopes |
| **Article III: Test-First** | PASSED | Integration tests written alongside contracts before implementation |
| **Article IV: Fail-Closed** | PASSED | Verification gates abort if socket or runner is unconfigured |
| **Article V: Single Source** | PASSED | Reuses existing `dc-store` and `GitPulse` event streams without duplication |
| **Article VI: Allocator Symmetry**| PASSED | Cross-language boundaries governed by Gusset zero-panic firewalls |
| **Article VII: Simplicity Limit** | PASSED | Introduces 0 new external dependencies; adds 2 modular internal packages |
| **Article VIII: Anti-Abstraction**| PASSED | Concrete implementations tailored specifically to agent worktree syncing |
| **Article IX: Observability** | PASSED | All sync events, sequence gaps, and recovery attempts logged to ledger |

## 3. Complexity Tracking (Constitutional Deviations)
*None. All proposed changes strictly adhere to Articles I through IX.*

## 4. Proposed File Changes & Layout
- `backend/go_orchestrator/devcouncil/spec/` (parser, validator, export, models)
- `backend/go_orchestrator/cmd/devcouncil/spec.go` (CLI subcommand)
- `specs/001-realtime-sync/` (7-layer reference specification)

# DevCouncil Specifications

7-layer specifications governed by [`constitution.md`](../constitution.md), grounded in DevCouncil's [engineering philosophy](../docs/PHILOSOPHY.md), and validated by `devcouncil spec validate <id> --strict`.

## Cross-Repo Stack Ownership

- [**003 - Stack Boundaries and Non-Overlap**](003-stack-boundaries-and-non-overlap/spec.md) ([`contracts/ownership-matrix.json`](003-stack-boundaries-and-non-overlap/contracts/ownership-matrix.json)): Master cross-repo ownership and anti-drift contract partitioning capabilities across `DevCouncil`, `Manvi`, and `Waylode` (`../GitPulse`), with verified overlap closures.
- [**Full-Stack Drift Audit & 4-Lane Work Split**](003-stack-boundaries-and-non-overlap/WORK_SPLIT_AND_DRIFT_AUDIT.md): Consolidated Wave A code-vs-spec/philosophy audit across all 3 repos + `waylode-labs`, strict landing order (`DevCouncil` -> `Manvi` -> `Waylode Primary` -> `Waylode Labs`), and 4 non-overlapping lanes enforced by [`scripts/check-stack-boundaries.mjs`](../scripts/check-stack-boundaries.mjs).

## Active Feature & Module Specifications

| ID | Title | Scope | State | Entry |
|---|---|---|---|---|
| `001` | Realtime Sync | Go spec/workflow orchestration & `.devcouncil/state.sqlite` WAL persistence; planned cross-worktree delta broadcast | Core state shipped; realtime broadcast planned | [`001-realtime-sync/spec.md`](001-realtime-sync/spec.md) |
| `002` | Zero-Copy Agentic Worktrees & DevMap Hardening | `StackedStore`, `stack_lease.json`, `devmap seed [--stacked]`, sparse `WorkingTreeSubject`, Waylode CoW cache & Manvi coordinator | Implemented (`Schema 29`) | [`002-zero-copy-worktrees/spec.md`](002-zero-copy-worktrees/spec.md) |
| `003` | Stack Boundaries and Non-Overlap | Cross-repo canonical ownership across `DevCouncil`, `Manvi`, and `Waylode` + verified overlap closures (`OV-*`) | Active contract + 4 tracked closures | [`003-stack-boundaries-and-non-overlap/spec.md`](003-stack-boundaries-and-non-overlap/spec.md) |
| `004` | Modular Engines and Policy Core | All 17 Rust crates (`devmap-*`, `dc-*`, `gusset-engine`) and 10 Go packages (`policy`, `gate`, `grants`, `devcouncil/*`) + non-goals | Implemented core + 3 hardening follow-ups | [`004-modular-engines-and-policy/spec.md`](004-modular-engines-and-policy/spec.md) |

## Validation

```bash
devcouncil spec validate --all --strict --project-root "$PWD"
devmap doc-drift --json specs/
```

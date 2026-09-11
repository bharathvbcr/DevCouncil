# Archived documentation

Moved here on 2026-09-11 so live product docs are not sitting next to
pre-cutover ledgers. These files are **historical**. Do not treat them as
current CLI, MCP, or installer behaviour.

Live follow-ups: [../TODO.md](../TODO.md).
Retirement decisions: [../PHASE7_LONG_TAIL.md](../PHASE7_LONG_TAIL.md).
Live DevMap guide: [../devmap/README.md](../devmap/README.md).

Internal links inside these files still use the tree they were written against
(`STATUS.md` next to `PLAN.md`, `../code-graph.md` meaning `docs/code-graph.md`).
Sibling historical files stayed together under `devmap/`; links to live docs as
`../foo.md` now need `../../foo.md`.

## `devmap/` — kernel port ledgers and dated audits

| File | What it was |
|------|-------------|
| [devmap/STATUS.md](devmap/STATUS.md) | Kernel port status ledger (pre-Phase-7) |
| [devmap/PLAN.md](devmap/PLAN.md) / [PLAN.html](devmap/PLAN.html) | Clean-room rewrite plan (Phase 6 hybrid) |
| [devmap/AGENT_PLAN.md](devmap/AGENT_PLAN.md) | Port execution plan for coding agents |
| [devmap/CONSUMERS.md](devmap/CONSUMERS.md) | Python `src/devcouncil/` consumer contracts |
| [devmap/PHASE1_CONTRACT.md](devmap/PHASE1_CONTRACT.md) | Phase-1 token-budget freeze (`TASK-RP-*`, done) |
| [devmap/DIVERGENCES.md](../devmap/DIVERGENCES.md) | **Not archived** — still the live divergence ledger |
| [devmap/AUDIT.md](devmap/AUDIT.md) / [AUDIT.html](devmap/AUDIT.html) | 2026-09-02 audit reading copy / authoritative HTML |
| [devmap/AUDIT_KERNEL_2026-09-05.md](devmap/AUDIT_KERNEL_2026-09-05.md) | Kernel audit snapshot |
| [devmap/INTEGRITY.md](devmap/INTEGRITY.md) | 2026-08-12 independent verification snapshot |
| [devmap/PROGRESS_AUDIT.md](devmap/PROGRESS_AUDIT.md) | Build-progress hardening evidence |
| [devmap/RELIABILITY_AUDIT_2026-09-08.md](devmap/RELIABILITY_AUDIT_2026-09-08.md) | Reliability audit |
| [devmap/RELIABILITY_AUDIT_2026-09-09.md](devmap/RELIABILITY_AUDIT_2026-09-09.md) | Coverage/robustness audit |
| [devmap/STALENESS_AUDIT_2026-09-08.md](devmap/STALENESS_AUDIT_2026-09-08.md) | Staleness audit |
| [devmap/STATE_DISCOVERY_AUDIT_2026-09-09.md](devmap/STATE_DISCOVERY_AUDIT_2026-09-09.md) | First-run / worktree discovery |
| [devmap/SCHEMA20_QUALIFICATION_2026-09-09.md](devmap/SCHEMA20_QUALIFICATION_2026-09-09.md) | Schema-20 / concurrent-worktree qualification |
| [devmap/RUST_AUDIT_2026-09-10.md](devmap/RUST_AUDIT_2026-09-10.md) | Rust hardening audit |
| `devmap/*.json` | Qualification dumps that accompanied those audits |

## Other retired product pages

| File | What it was |
|------|-------------|
| [build-week-demo.md](build-week-demo.md) | Python `dev check --verify` walkthrough |

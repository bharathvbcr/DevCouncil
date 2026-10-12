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
| [devmap/ATTRIBUTION_AUDIT.md](devmap/ATTRIBUTION_AUDIT.md) | Benchmark attribution audit |
| [devmap/BENCHMARK_HARDENING_AUDIT.md](devmap/BENCHMARK_HARDENING_AUDIT.md) | Benchmark hardening audit (content landed; see its status line) |
| [devmap/EXTRACTION_RESOLVE_AUDIT_2026-10-02.md](devmap/EXTRACTION_RESOLVE_AUDIT_2026-10-02.md) | Extraction and resolver audit, 2026-10-02 |
| [devmap/RESOLVER_PRECISION_AB_2026-10-07.md](devmap/RESOLVER_PRECISION_AB_2026-10-07.md) | Resolver precision A/B, 2026-10-07 |
| [devmap/STORE_FOOTPRINT_AUDIT.md](devmap/STORE_FOOTPRINT_AUDIT.md) | Store footprint audit |
| [devmap/UPDATE_PROFILE_AUDIT.md](devmap/UPDATE_PROFILE_AUDIT.md) | Update profile audit |
| `devmap/*.json` | Qualification dumps that accompanied those audits |

## Other retired product pages

| File | What it was |
|------|-------------|
| [build-week-demo.md](build-week-demo.md) | Python `dev check --verify` walkthrough |
| [CLI_PRESENTATION_AUDIT.md](CLI_PRESENTATION_AUDIT.md) | CLI presentation audit (closed 2026-09-12) |
| [hook-migration-audit.md](hook-migration-audit.md) | Hook-retirement migration audit (closed 2026-09-12) |
| [IMPROVEMENTS.md](IMPROVEMENTS.md) | Python-era improvement ledger (moved from the repository root 2026-10-12) |
| [DEVMAP_REVIEW.md](DEVMAP_REVIEW.md) / [DEVMAP_HARDENING.md](DEVMAP_HARDENING.md) | 2026-09-06 hardening plan and its review (moved from the root 2026-10-12) |
| [DevCouncil-website-benchmark.md](DevCouncil-website-benchmark.md) | Site copy for the v0.2.2 competition benchmark (moved from the root 2026-10-12; its recorded checker `benchmarks/results/competition/20260914-v0.2.2/verify_docs.py` still names the old root path) |

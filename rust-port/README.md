# rust-port

Clean-room Rust rewrite of the `dev map` code-intelligence subsystem.

**Status: active port; not ready for production cutover.** Workspace crates under `crates/`
implement extract → resolve → analyze → store → query → serve → CLI. The Python implementation in
`src/devcouncil/indexing/` and `src/devcouncil/codeintel/` remains the live production
system until Phase 6 cutover. See [STATUS.md](STATUS.md) for verified and open gates.

## Read this first

- **[AGENT_PLAN.md](AGENT_PLAN.md)** — execution plan for agents: phases 0–7 with per-task
  instructions, proofs, and gates. Start here if you are building.
- **[PLAN.md](PLAN.md)** — full audit, specification, phases, and implementation guide.
- **[PHASE1_CONTRACT.md](PHASE1_CONTRACT.md)** — token budgets, §4.5 scoping, starter corpus.
- [AUDIT.html](AUDIT.html) — Audit II: 84 further findings; all fixes land in the Rust port.
- [PLAN.html](PLAN.html) — same plan content, styled for humans.

## What this replaces (eventually)

26,837 LOC across `indexing/` (17,929), `codeintel/` (8,405) and `cli/commands/map.py` (503),
plus 16,534 LOC of tests, with **37 files outside those directories importing them**.

## Crate layout

```
rust-port/
  crates/
    devmap-extract/   tree-sitter + explicit unavailable outcomes + wiring + frameworks
    devmap-resolve/   imports, calls, receiver types
    devmap-analyze/   liveness + communities (cohesion)
    devmap-store/     rusqlite v6 schema, pending queue, history, differential writes
    devmap-query/     token-budgeted search/deps + manifest
    devmap-serve/     watcher + durable pending drain
    devmap-cli/       `devmap` binary
  testdata/           Phase-1 starter corpus
```

## Build & test

```bash
cd rust-port
./verify.sh
cargo run -p devmap-cli -- --db /tmp/devmap-test.sqlite --progress always build ./testdata
cargo run -p devmap-cli -- --db /tmp/devmap-test.sqlite search helper --budget 500
cargo run -p devmap-cli -- --db /tmp/devmap-test.sqlite status
cargo run -p devmap-cli -- --db /tmp/devmap-test.sqlite history --last 10
```

`--progress auto|always|never` is global. `auto` reports the five bounded mapping stages only
on an interactive terminal and never contaminates JSON stdout; `always` is useful for logs and
CI, and `never` suppresses the reporter. Progress is written to stderr.

`history --last N` shows recent generations, measurements, and symbol/edge/dead-code deltas;
global `--json` returns the same bounded data as JSON. Unmeasured durations are `null`, never
a fabricated zero. History retention is capped at 500 rows.

`./verify.sh --quick` runs format, clippy, and the complete Rust test suite. The default also
runs two semantic-digest builds and an optimized DevCouncil self-map with time/size gates.
`./verify.sh --mutants` additionally requires a separately approved and installed
`cargo-mutants`; the script never installs dependencies implicitly.

## For agents

- **PLAN.md §3** — acceptance specification (31 findings + 7 enhancements). Each property
  becomes a test, not a patch.
- **PLAN.md §4** — capabilities that must survive; inventory completion is a Phase 1 gate.
- Do **not** delete Python analysis code in this track.
- Trap: differential writes without deletion reconciliation leave deleted files live forever
  (B3/N2 must ship together).

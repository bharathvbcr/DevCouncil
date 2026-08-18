# Phase 1 contract freeze (rust-port)

**Track:** REQ-RP-1 … REQ-RP-6 / TASK-RP-1 … TASK-RP-6  
**Basis:** [PLAN.md](PLAN.md) §3 acceptance properties + §4 capabilities inventory  
**Status:** DevCouncil track TASK-RP-1 … TASK-RP-6 complete (2026-08-12). Python `indexing/` + `codeintel/` remain live until Phase 6 cutover.

## Token budgets (T1–T4)

| Surface | Default budget (tokens) | Truncation contract |
|---|---:|---|
| `devmap search` | 2000 | Response always includes `{shown, total, truncated, tokens_used}` |
| `devmap deps` | 2000 | Same |
| `devmap dead` | 2000 | Same |
| Manifest (`devmap manifest`) | ≤2000 target | Subsystems capped (top 20 communities); no full `files`/`dependents` tables |
| `devmap status` | n/a (fixed fields) | generation, pending_count, node/edge counts |

Rank before truncate. Never present a capped sample as complete coverage.

## §4.5 scoping decisions (port / keep in Python / drop)

| Area | LOC (plan) | Decision | Rationale |
|---|---:|---|---|
| PDG / taint / CFG | ~872 | **keep (Python) for now** | Not on cold-build critical path; port after core extract/resolve/store parity |
| LSP integration | ~775 | **keep (Python)** | Editor UX; daemon can expose later; not cutover-blocking |
| Debug / DAP observations | ~1,207 | **keep (Python)** | Orthogonal runtime; do not block Phase 2–4 |
| Visualization / export | ~2,591 | **port (thin)** then **opt-in** | Large JSON exports leave context dir (T2/N3); HTML viz can stay Python until cutover |
| Semantic index / AST matcher | ~449 | **port** with extract | Needed for parity harness and agent queries |
| Full 35-language golden graphs | — | **Phase 1 remaining** | Starter corpus under `testdata/` covers §4.1–§4.3 samples; full matrix still open |

## Starter corpus (`testdata/`)

Covers:

- Python nested package + `__main__` entry + FastAPI-style route (§4.1–§4.3)
- JS relative `./` import between siblings (§4.1)
- Rust Axum-style `.route(..., get(handler))` (§4.3)
- Vendored path + `*_pb2.py` generated marker (§4.2)

## Explicitly out of scope for this track

- Deleting `src/devcouncil/indexing/` or `src/devcouncil/codeintel/`
- Full 35-language parity harness
- Embedding / RRF ranking (N7/R3) — deferred to Phase 5
- Real Cypher engine (N10) — deferred; no regex Cypher shim in Rust

# DevCouncil analysis components

Four Rust crates that answer *may this change proceed*. Three build to
standalone binaries; the fourth is a library linked into one of them.

DevCouncil owns these as **components**. A harness — [MANVI](https://github.com/bharathvbcr/Manvi)
is the one they were built for — resolves each as a binary from `PATH` and
**links none of them**. That is the whole contract: a component can be rebuilt,
replaced, or reused by something else without the consumer recompiling, and a
consumer stays a single static binary that embeds anywhere.

The code-intelligence component, `devmap`, lives in [`../rust-port/`](../rust-port/)
and is a separate workspace because it carries ~36 tree-sitter grammars; folding
it in here would make every `dc-verify` test compile them.

| Crate | Binary | Answers |
|---|---|---|
| `dc-store` | `dcstore` | Which tasks exist, and who holds the lease on one |
| `dc-verify` | `dcverify` | What a diff changed, whether it stayed in scope, whether it is honest, whether tests reached it |
| `dc-grep` | `dcgrep` | What is in this repository, honouring its ignore rules |
| `dc-glob` | *(library)* | Whether a path matches a pattern, with CPython `fnmatch` semantics |

---

## The contract

Every binary follows the same rules. They are worth reading before changing one,
because consumers depend on all of them and none is enforced by the type system.

1. **One JSON object on stdout, including on failure.** A caller never has to
   parse prose or infer from an exit code alone. The exit code is a coarse
   duplicate of `ok`, for shell use.
2. **An expected outcome is not an error.** `dcstore acquire` on a task someone
   else holds exits **0** with `{"ok":false,"code":"lease_held_by_other"}`,
   because contention is what happens when two builders run. A caller forced to
   tell "busy" from "broken" by reading stderr will eventually get it wrong.
3. **A check that could not run is never reported as one that ran.** `dcverify`
   exits 2 on a diff it cannot parse rather than returning an empty finding
   list — an empty list means *these gates ran and found nothing*, and the two
   must not share a representation.
4. **`health` asserts identity.** Every binary answers it, and the reply names
   the component, so a caller can confirm it is talking to this program and not
   to some other one that prints JSON.

Command surfaces, as the binaries themselves report them:

```
dcstore    acquire, diagnose, release, renew, active, list, task, ready, scope-append, health
dcverify   check, health
dcgrep     search, files, health
```

`dcstore` additionally requires `--db`.

### Fail-closed, concretely

`dcstore health` asserts that the partial unique index behind the lease is
really there:

```json
{"ok":true,"store":"dc-store","schema_version":1,"exclusion_index":"verified","active_leases":0}
```

`CREATE UNIQUE INDEX IF NOT EXISTS` matches on the **name** only, so a database
carrying an index called `ux_task_leases_active` over the wrong column, or
without the `status = 'active'` predicate, makes the DDL a silent no-op. Every
acquire then falls back to a check-then-insert that passes every
single-threaded test and hands two builders the same task under contention.
`verify_exclusion_index` reads the schema that is actually open — `PRAGMA
index_list` for unique and partial, `PRAGMA index_info` for the column, and
`sqlite_master` normalised for the predicate — and refuses anything else.

A build that predates this omits `exclusion_index` from the reply entirely. A
consumer must treat a missing key as unverified, never as verified; the install
script below does.

---

## Build, test, install

```bash
cargo build --workspace --release     # from this directory
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Install all four components (including `devmap` from `rust-port/`):

```bash
bash scripts/install-components.sh
```

It builds release, **health-checks every binary before installing any of them**,
and installs by atomic rename so a running process keeps the inode it started
with. `PREFIX` chooses the root (default `~/.local`), `PROFILE=debug` builds
faster, `DRY_RUN=1` builds and checks without installing, and naming components
installs a subset:

```bash
DRY_RUN=1 bash scripts/install-components.sh
PREFIX=/usr/local bash scripts/install-components.sh dcstore dcverify
```

The script warns when an installed component is shadowed by an older copy
earlier in `PATH` — the failure mode where every subsequent symptom looks like
the new build is broken.

### Requirements

- **Rust, stable.** `Cargo.toml` declares edition 2024 with resolver 3, so 1.85
  or newer.
- **A C compiler.** `dc-store` takes `rusqlite` with `bundled` and compiles
  SQLite from source rather than linking whatever `libsqlite3` the host ships.
- **`sqlite3`** for the Go client tests, which plant a task row with it.
- **Python with DevCouncil installed** for the interop suite (below). Without
  it those tests skip.

---

## The interop suite, and why it can be made fatal

`dc-store/tests/interop.rs` drives **both** sides against one `state.sqlite`:
the Rust store acquires a lease and DevCouncil's own
`devcouncil.storage.native.TaskLeaseRepository` reads it back, then the reverse,
then both agree on when it expires. It is the only evidence that the two
implementations mean the same thing by a lease.

It skips when it cannot find `.venv/bin/python` in the checkout — and a skip
prints `ok`, which is indistinguishable from a pass in any summary. So:

```bash
DC_STORE_REQUIRE_INTEROP=1 cargo test -p dc-store --test interop
```

turns the skip into a failure that says the agreement is **UNPROVEN**. CI sets
it (see [`../.github/workflows/analysis-plane.yml`](../.github/workflows/analysis-plane.yml)).
Leave it unset locally if you have no venv; never leave it unset in CI.

---

## Consumers

The Go clients live in [`../backend/go_orchestrator/dc/`](../backend/go_orchestrator/dc/):
`store`, `dcgrep` and `devmap`. They are **clients** — they transport answers and
do not compute them. A client that started deciding whether a lease is valid, or
caching one, would be reimplementing a component badly and on the wrong side of
the boundary; a cached lease is one that has already expired somewhere else.

`dc/devmap`'s `TestTheLive*` tests drive the **real** `devmap` binary rather than
a fake, because the field names they decode are a contract with a producer built
from another workspace, and a fake asserts only that the test and the code were
written by the same hand. They skip visibly when `devmap` is absent, and CI
fails if they skipped.

---

## Where to look next

- [`STATUS.md`](STATUS.md) — the port ledger: what was verified and how, what is
  deliberately different from MANVI's mirror, what is still open. **§4 is the one
  to read before planning a cutover**: these components are not uniformly better
  than the Python they resemble, and `dcverify`'s stub detection is measurably
  weaker than `verification/stub_detector.py`, which does AST analysis.
- [`../rust-port/STATUS.md`](../rust-port/STATUS.md) — devmap's own ledger.
- MANVI's `docs/COMPONENTS_AND_HARNESS.md` — the consumer's view: the resolution
  ladder, where a change belongs, and the checklist a newly ported component
  must satisfy before anything depends on it.

## A note for whoever ports the next component

The checklist in that last document is not ceremony. Each item exists because
its absence produced a specific failure here: a boundary that panicked instead
of answering, a gate that reported clean because its input was one byte from
being UTF-8, a store that answered confidently from a database it had just
created, and three interop checks that reported `ok` on every worktree run
without ever having started a Python process.

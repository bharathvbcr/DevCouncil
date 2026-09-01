# Analysis Plane (Rust) — Port Status Ledger

**What this is:** the four `dc-*` crates and their Go clients, ported from the MANVI
harness (`~/Code/devtools/Manvi`) into DevCouncil on 2026-09-01.

**What it is not:** a replacement for anything. Nothing in `src/devcouncil/` calls
this code yet. Every Python verification gate, lease repository and search path in
DevCouncil runs exactly as it did before this port. Read [§4](#4-what-this-does-not-yet-improve)
before assuming the Rust side is an upgrade to the Python it resembles — in one
significant case it is measurably less capable, and cutting over would weaken a gate.

This ledger follows `rust-port/STATUS.md`'s convention: claims are labelled
**verified** (a command was run and its output read), **inferred**, or
**unverified**. Passing tests are local mechanical evidence, not evidence of a
soak, of CI, or of production parity.

---

## 1. What landed

Line counts are `src/` versus the whole crate (the difference is `tests/`), and
exclude `rust/target/`, which holds generated sources from build scripts.

| Component | Lines (src / total) | Role |
|---|---|---|
| `rust/dc-glob` | 239 / 239 | CPython `fnmatch` semantics, zero dependencies |
| `rust/dc-grep` | 1,730 / 1,730 | Ignore-aware repository search on ripgrep's own engine |
| `rust/dc-store` | 2,675 / 3,621 | Task + lease store over DevCouncil's `state.sqlite` schema |
| `rust/dc-verify` | 1,793 / 2,554 | Unified-diff parsing, scope classification, rigor gates, diff↔coverage |
| **Rust total** | **6,437 / 8,144** | |
| `backend/go_orchestrator/dc/{store,dcgrep,devmap}` | — / 8,376 | Go IPC clients across the process boundary |
| `backend/go_orchestrator/internal/{proc,testsupport}` | — / 806 | Process-group control; build/locate real binaries for tests |
| `backend/go_orchestrator/repomap` | 584 / 946 | Code-graph artifact loader (client-side schema check) |
| **Go total** | **3,930 / 10,128** | |

The Go module is `github.com/bharathvbcr/DevCouncil/backend/go_orchestrator` and has
**zero third-party dependencies** (verified: `go.mod` has no `require` block; `go build ./...`
and `go vet ./...` both exit 0).

The Rust workspace takes three: `rusqlite` (bundled, pinned to `0.31` — the same line
`rust-port/Cargo.toml` uses, so the two workspaces do not carry two SQLite builds),
and ripgrep's `grep-regex` / `grep-searcher` / `ignore`.

### Why a second Cargo workspace rather than joining `rust-port/`

`rust-port/` is the devmap port: 50k lines and ~36 tree-sitter grammars, a full
build of which takes minutes. The analysis plane is 6.4k lines and three
dependencies, and its whole value is that a test run is seconds. Folding it in
would make every `dc-verify` test compile tree-sitter.

They are also different planes. `rust-port/` answers *what does this code mean*;
`dc-*` answers *may this change proceed*. MANVI keeps the same split — it builds
the `dc-*` crates and resolves `devmap` as an external binary from `PATH`.

---

## 2. What was verified, and how

All commands run 2026-09-01 on darwin/arm64, `cargo 1.98.0`, `go1.26.4`,
DevCouncil `.venv` Python 3.12.13.

**Rust — verified.** `cargo test --workspace` in `rust/`: **130 passed, 0 failed,
0 ignored.**

**Cross-language interop with DevCouncil's own Python — verified, and newly
executed.** `rust/dc-store/tests/interop.rs` drives both sides against one
`state.sqlite`: Rust acquires a lease and DevCouncil's own
`devcouncil.storage.native.TaskLeaseRepository` reads it back, then the reverse,
then both agree on expiry. **3 passed in 1.33s** (a skip is 0.00s). This is the
load-bearing result of the port: it is direct evidence that the Rust store and
DevCouncil's SQLModel repository agree on schema, token and timestamp semantics
on the same file.

**Go — verified.** `go test ./...` in `backend/go_orchestrator`: **862 test and
subtest executions, 862 passed, 0 skipped, 0 failed.** These are not mock-only:
`internal/testsupport` builds the real `dcstore` / `dcverify` / `dcgrep` binaries
out of `rust/` and the tests exec them.

**The live devmap contract — verified.** `dc/devmap`'s three `TestTheLive*` tests
build a fixture repository and drive the **real** `devmap` binary
(`/Users/bharath/.cargo/bin/devmap`), asserting the field names this package
decodes actually arrive. All 3 passed.

### Red demonstrations — the gates were shown to fail, not assumed to work

A passing test is not evidence until it has been shown it can fail.

1. **Interop cannot silently vanish.** With `.venv` moved aside and
   `DC_STORE_REQUIRE_INTEROP=1`, all three interop tests **FAILED** with
   "schema and timestamp agreement is UNPROVEN", rather than reporting `ok`.
   Restored and re-run green.
2. **The live devmap tests depend on devmap.** With `devmap` removed from `PATH`,
   all three `TestTheLive*` tests reported `--- SKIP`, not `--- PASS`. So the
   earlier PASS is evidence that the binary ran.

---

## 3. Changes made during the port

Four, all forced by the move. Everything else is byte-identical to MANVI.

1. **`dc-glob` fixture path.** `include_str!("../../../testdata/…")` →
   `("../../testdata/…")`; the 776-line CPython `fnmatch` parity fixture now lives
   at `rust/testdata/fnmatch-parity.tsv`, inside the workspace, so the plane stays
   self-contained.
2. **`dc-store` interop root resolution.** It walked *three* levels up and re-entered
   by the literal name `DevCouncil`, which was right when the crate lived in a
   sibling repository. It now resolves the repository as its own ancestor
   (`<repo>/rust/dc-store` → two levels up). The old form would have been correct
   here only by a coincidence of depth, and only for a checkout named `DevCouncil`
   — a worktree under any other name would have reported "no venv" for what was
   really "cannot find the repository I am inside".
3. **`dc-store` interop skips are now optionally fatal.** The file header promised
   a skipped interop check never looks like a passed one, but `eprintln!` +
   `return` prints `ok`. Now that the interpreter is `.venv/bin/python` *inside this
   checkout*, "missing" means an unprovisioned tree rather than an unrelated
   machine. `DC_STORE_REQUIRE_INTEROP=1` turns the skip into a failure so CI can
   demand the evidence; unset, the behaviour is unchanged.
4. **`testsupport` workspace location.** MANVI's Rust workspace is `crates/`;
   DevCouncil's is `rust/`. The literal was spelled in five places, so it is now
   one exported constant, `testsupport.RustWorkspace`.

**Not changed, and deliberately flagged:** `testsupport.AllowSkipEnv` is still
`MANVI_TEST_ALLOW_SKIP` and the build lock is still `.manvi-testbin.lock`.
Renaming an environment variable is an operational change that breaks any CI
config referencing it, so it is left as a decision rather than taken as a
drive-by.

**Not ported:** `repomap/repomap_test.go` and `repomap/integrity_test.go`. Both
import `manvi/gate` and `manvi/policy`, which are the *policy* plane and are not
part of this port. They test `policy.FileGate` integration, not the artifact
loader, so they belong with that plane's port rather than here.
`repomap/stress_test.go`, which does not import them, came across and passes.

---

## 4. What this does **not** yet improve

This is the part to read before planning a cutover. The `dc-*` crates are the
foundation of a Rust/Go execution plane. They are **not** uniformly better than
the Python they resemble, and in one case they are clearly worse.

| Capability | DevCouncil (Python) | `dc-verify` (Rust) | Honest verdict |
|---|---|---|---|
| **Stub / placeholder detection** | `verification/stub_detector.py`, 369 lines: **AST parse** of changed files, per-language idioms, `devcouncil: allow-stub` escape hatch gated on the task mentioning scaffolding, assert-free test detection, skipped-test detection, separate stub-**declaration** audit | `rigor::detect_stubs`: substring markers over added diff lines, with a comment/string guard | **Python is ahead.** Swapping in Rust here would be a capability regression. Do not cut this over. |
| **Secret / credential scanning** | `gating/checks/secret_scan_check.py`, wired into `verifier.py` and `gated_write.py` | `rigor::scan_secrets`, with evidence redaction | Roughly comparable; both exist. Needs a differential corpus run before either is called better. **Unverified.** |
| **Orphan-diff / scope** | `checks/orphan_diff.py` reads `git diff --name-status`, exact set membership against planned paths | `parse_unified` + `classify_scope`, full unified-diff parse, fail-closed on malformed input | Rust parses the diff body rather than trusting git's name list, and returns `untouched_planned` as well. **Plausible improvement, unmeasured.** |
| **Diff↔coverage** | `diff_coverage.py` + `coverage_measurement.py`, ~800 lines, runs the suite under instrumentation; coverage.py-centric | `coverage::parse` handles **Go `-coverprofile` and LCOV**; does not run anything | **Complementary, not competing.** Rust adds non-Python coverage formats; Python owns the instrumentation run. |
| **Lease mutual exclusion** | `storage/native.py`, same partial unique index | `dc-store`, same index, plus `verify_exclusion_index` reading `PRAGMA index_list` to refuse a database whose index is only *named* right | **Parity, plus one real hardening.** The value is that a Go/Rust plane can share the file — proven by the interop tests. |
| **Repository search** | Python-side helpers | `dc-grep`, ripgrep's linked engine, results as JSON | Additive; no Python equivalent was replaced. |

**Consequence:** the honest first cutover is **`dc-store`**, because interop is
proven and the semantics are identical. `dc-verify`'s rigor gates should be
treated as a *second opinion* alongside Python's, not a replacement, until a
differential run over real diffs says otherwise.

---

## 5. Not done

- **Nothing is wired.** No Python consumer imports these binaries. There is no
  `analysis_plane` client module yet — the pattern to follow is
  `src/devcouncil/devmap_client.py`, including its `try_connect` discipline
  (a store that cannot answer must not report a confident zero).
- **No CI.** Neither workspace is in `.github/`. `DC_STORE_REQUIRE_INTEROP=1`
  exists precisely so CI can demand the interop evidence, and nothing sets it.
- **The execution plane is not ported.** MANVI's `gate`, `policy`, `grants`,
  `agent`, `llm`, `session`, `tools` and `ui` (~123k lines of Go) are what would
  actually replace `src/devcouncil/execution/` and `gating/`. Only the IPC clients
  came across.
- **No differential run** of Rust vs Python gates over real DevCouncil diffs.
  Section 4's verdicts are from reading both implementations, not from measurement.
- **Linux is unverified.** Everything here was run on darwin/arm64 only.

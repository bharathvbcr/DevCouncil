# Analysis Plane (Rust) — Port Status Ledger

> **This is not the kernel ledger.** The kernel crates (`devmap-*`) now live
> in this same workspace. Open work for both planes is indexed in
> [`docs/devmap/AGENT_PLAN.md` → Consolidated open-work register](../docs/devmap/AGENT_PLAN.md#consolidated-open-work-register-2026-09-02)
> (section E covers this plane). This file stays authoritative for the analysis-plane
> detail; kernel status is [`docs/devmap/STATUS.md`](../docs/devmap/STATUS.md).

**What this is:** the four `dc-*` crates and their Go clients, ported from the MANVI
harness (`~/Code/devtools/Manvi`) into DevCouncil on 2026-09-01.

**What it is not:** a replacement for anything. Nothing in `src/devcouncil/` calls
this code yet. Every Python verification gate, lease repository and search path in
DevCouncil runs exactly as it did before this port. Read [§4](#4-what-this-does-not-yet-improve)
before assuming the Rust side is an upgrade to the Python it resembles — in one
significant case it is measurably less capable, and cutting over would weaken a gate.

> **These are DevCouncil components, and DevCouncil is upstream.**
>
> They were authored in MANVI and ported here, but that is history, not
> ownership. DevCouncil is the component layer: `devmap`, `dcstore`, `dcverify`
> and `dcgrep` are components with a JSON-on-stdio contract, useful to anything
> that speaks it. MANVI is the harness that unifies them — it resolves each one
> as a binary from `PATH` and **links none of them**. Going forward these crates
> are edited here, and MANVI's `crates/` copy is a development convenience that
> mirrors this one.
>
> The two copies still have no build-time relationship, so nothing fails when
> they drift — [§6](#6-the-two-copies) is the standing decision that needs
> making. **Exactly two files are deliberately different**, and a mirroring
> script has to know both:
>
> - `dc-store/tests/interop.rs` — this copy resolves DevCouncil as its own
>   ancestor; MANVI's searches upward for a sibling checkout.
> - `dc-glob/src/lib.rs` — one `include_str!` path, because the parity fixture
>   is at `rust/testdata/` here and at the repository root in MANVI.
>
> Four paths exist only here and have no MANVI counterpart: `.gitignore`,
> `README.md`, `STATUS.md`, and `testdata/` (MANVI keeps the parity fixture at
> its repository root). Every other file is byte-identical:
>
> ```bash
> diff -rq --exclude=target <manvi>/crates rust \
>   | grep -v 'Only in rust'   # must name exactly the two files above
> ```
>
> The relationship, the component inventory, and the checklist a newly ported
> component must satisfy live in MANVI's
> [`docs/COMPONENTS_AND_HARNESS.md`](../../Manvi/docs/COMPONENTS_AND_HARNESS.md).
> This ledger covers only the analysis-plane components that landed here.

This ledger follows `docs/devmap/STATUS.md`'s convention: claims are labelled
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

The Rust workspace pins `rusqlite` (bundled, `0.40.2`) so `dc-store` and
`devmap-store` do not carry two SQLite builds, plus ripgrep's `grep-regex` /
`grep-searcher` / `ignore`.

### Why two CI jobs rather than `cargo test --workspace` on every PR

The kernel is 50k lines and ~36 tree-sitter grammars; a full build takes minutes.
The analysis plane is a few thousand lines and a handful of dependencies, and
its whole value is that a test run is seconds. They share this workspace and
lockfile so `dcstore` and `devmap` cannot drift onto two SQLite builds. They
keep separate GitHub jobs so a `dc-grep` change does not compile the grammars.

They are also different components. The kernel answers *what does this code
mean*; `dc-*` answers *may this change proceed*. Both are DevCouncil components
and MANVI consumes them identically — `devmap`, `dcstore`, `dcverify` and
`dcgrep` are each resolved as a binary from `PATH`, linked by nothing. The
harness draws no distinction between them. The CI split is about build cost
rather than about status.

---

## 2. What was verified, and how

All commands run 2026-09-01 on darwin/arm64, `cargo 1.98.0`, `go1.26.4`,
DevCouncil `.venv` Python 3.12.13.

**Rust — verified.** `cargo test --workspace` in `rust/`: **130 passed, 0 failed,
0 ignored** at the time of the port. Now **134**; the four added are in
[§7](#7-change-log-since-the-port).

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

## 5. State of the handoff

**Closed since the port.**

- **CI exists.** [`.github/workflows/analysis-plane.yml`](../.github/workflows/analysis-plane.yml)
  covers `rust/**` and `backend/go_orchestrator/**` in two jobs
  that fail for different reasons: the `dc-*` crates on Linux/macOS/Windows
  (named packages, not `--workspace`, so they do not compile tree-sitter);
  and the Go clients driving real binaries, with
  a step that **fails if the live devmap contract tests skipped**. Kernel crates
  are gated by [`.github/workflows/rust.yml`](../.github/workflows/rust.yml).
  `rust/**` is in the analysis-plane trigger paths on purpose: `dc-*` and the
  Go clients live in one repository, so a component change must run the
  client's contract tests.
- **A build and install path.** [`scripts/install-components.sh`](../scripts/install-components.sh)
  builds all four release, health-checks each **before** installing any, and
  installs by atomic rename. Its `dcstore` check refuses a binary that does not
  report `exclusion_index: verified` — demonstrated against the stale build that
  was installed on the author's machine, which omits the key entirely.
- **A component README.** [`README.md`](README.md) states the contract all four
  binaries follow, the command surfaces as the binaries themselves report them,
  and where to look next.
- **Formatting is enforced and clean.** `cargo fmt --all -- --check` passed only
  after fixing `dc-store/tests/requirements.rs`; without that, the new CI would
  have been red on its first run.

**Still open, and deliberately so.**

- **Nothing is wired to Python.** No Python consumer imports these binaries, and
  under the component model none needs to: DevCouncil owns the components, a
  harness consumes them. If a Python consumer is ever wanted, the pattern is
  `src/devcouncil/devmap_client.py` and its `try_connect` discipline — a store
  that cannot answer must not report a confident zero. Note `CONSUMERS.md`'s
  record of what that pattern cost last time: seven "hybrid" consumers whose
  Rust path never executed once, hidden by the Python fallback.
- **The execution plane is not a component.** MANVI's `gate`, `policy`, `grants`,
  `agent`, `llm`, `session`, `tools` and `ui` are the *harness*, not components,
  and are not candidates for porting into this repository.
- **No differential run** of these gates against DevCouncil's Python equivalents
  over real diffs. §4's verdicts are from reading both implementations, not from
  measurement — which is why §4 says do not cut over stub detection.
- **Windows is unproven for `dc-store`.** `dc-glob`, `dc-verify` and `dc-grep`
  were confirmed to `cargo check` for `x86_64-pc-windows-msvc` locally;
  `dc-store` compiles SQLite from source and could not be cross-checked from
  macOS. It is in the CI matrix because the kernel already builds bundled
  `rusqlite` on Windows, so the answer should be yes — but that is an inference,
  and the first Windows CI run is what settles it.
- **Linux is unproven locally.** Everything here was run on darwin/arm64. The
  new CI is what covers it, and it has not run yet.

---

## 6. The two copies

Since 2026-09-01 the `dc-*` crate sources exist here **and** in MANVI, with no
mechanism keeping them equal. The first change after the port already had to be
applied twice by hand (§7), which is the whole problem in miniature: it worked
because one person did both halves in one sitting, and nothing would have failed
if they had not.

Note what is *not* duplicated. MANVI never links these crates — it resolves
`dcstore`, `dcverify` and `dcgrep` as binaries from `PATH`, exactly as it already
resolves `devmap`. So the duplication is of **sources**, not of the runtime
dependency, and the deployed arrangement is already the right one: one set of
component binaries, one harness consuming them.

That narrows the options to how MANVI's source copy should end:

1. **Delete it; require installed components.** The honest expression of
   "DevCouncil owns the components". `toolBinary`'s cargo-build fallback stops
   being reachable, and MANVI's test suite needs DevCouncil's binaries on `PATH`
   — which is what its live-contract tests already assume for `devmap`.
2. **Keep it as a development convenience, with a digest check.** MANVI can be
   built and tested without a DevCouncil checkout; a checked-in digest of the
   component sources makes a stale copy a test failure rather than a surprise.
3. **Consume by path or git dependency.** Removes the source duplication without
   removing the convenience, at the cost of coupling the two repositories'
   release cycles — and it buys nothing at runtime, since nothing links.

Option 2 is the cheapest thing that makes drift *detectable*, and option 1 is
where this should land once installing DevCouncil's components is routine. Until
one is chosen: **edit here, mirror to MANVI.**

## 7. Change log since the port

**2026-09-01 — the requirements a task exists to satisfy now cross the boundary.**

`schema.rs` created `requirement_ids_json` and `acceptance_criterion_ids_json`
because they are in the DevCouncil schema it was transcribed from. `Store::task`
selected neither, `dcstore` emitted neither, and `dc.Task` had no field for
either — so every task reaching the Go plane reported no requirements and no
acceptance criteria. Not an empty list: absent. A requirement-coverage gate
reading this store would find nothing to check and report a task accountable to
no requirement exactly as it reports one accountable to all of them.

Added: the two columns to the task read (verbatim, never merged — there is
deliberately no `agent_appended_*` counterpart, because a task that could append
to its own requirements could discharge one by claiming it); the two keys to the
boundary reply; the fields to Go's wire and domain types; and `dc.Requirement` /
`dc.AcceptanceCriterion`, DevCouncil's model field for field.

Both Go types decode by hand because DevCouncil declares `required: bool = True`
and `source: ... = "planner"`, pydantic omits defaults when it serialises, and Go
zeroes an absent bool to `false`. With plain struct tags every criterion whose
producer omitted the key would have arrived **optional** — still listed, still
looking checked, no longer something the work must satisfy.

Verified: `cargo test --workspace` 134 passed / 0 failed (was 130), clippy clean;
`go test ./...` all 7 packages pass. The cross-language tests drive DevCouncil's
own pydantic models, including under `model_dump_json(exclude_defaults=True)` —
the hostile case where every defaulted key leaves the wire — and read the
`verification_method` / `priority` / `source` sets out of the Python `Literal`s
so a member DevCouncil adds and Go has not learned fails here rather than in
production. Red-demonstrated by dropping `llm_review` from the Go set and
watching the parity test fail.

**Still not wired.** This closes a gap the council port depends on; it does not
by itself make anything call the Rust plane.

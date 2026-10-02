# Extraction and resolver audit — 2026-10-02

Scope: the DevMap extraction, resolver, store and query changes committed from
2026-09-19 through 2026-10-01 (57 non-merge commits touching `devmap-extract`,
`devmap-resolve`, `devmap-store`, `devmap-query` and `devmap-analyze`, HEAD
`bb45211d`). This is a read-only review of source, tests and commit messages.
**It did not build the workspace or run any test**: the review environment had
no Rust toolchain. Every "test pins X" statement below is from reading the test,
and every measurement quoted from a commit message was taken on repositories
that are not in this tree (scholarlm, MLSystemsLab, BINN) and was not reproduced.

## Summary

| # | Finding | Severity | Status |
|---|---|---|---|
| 1 | `sys.path.insert(0, …)` is treated as lower precedence than ordinary resolution; Python runtime does the opposite | Medium | Open |
| 2 | `b3a9edb1` changed extraction output without bumping the schema; the bump arrived one commit later | Low (history only) | Resolved at HEAD |
| 3 | `DIVERGENCES.md` stops at X48 (2026-09-12); the later resolver and extractor behaviour is not recorded | Low | Open |
| 4 | Commit-message measurements are not reproducible from this repository | Info | Open |

## What checked out

* **Schema versioning.** `EXTRACTION_SCHEMA_VERSION` is `66` at HEAD. The doc
  comment in `cache.rs` describes the v66 change set and the reason a v65 row
  must not be served. The sequence 61 → 62 (`823fd2eb`) → 63 (`5c33e532`) →
  64 (`74419a09`) → 65 (`347dc7cc`) → 66 (`bb45211d`) bumps in the commit that
  changes the output, except finding 2.
* **Go unexported-selector rung (`5bab18f6`, `0c07b7f3`, `31a08d2d`).** The
  lookup is keyed on `(directory, package clause)`, so a `//go:build ignore`
  `package main` file in a library directory cannot supply a method to the
  library's package. It abstains on a second method of the name, any struct
  field or interface method of the name, cgo `C.`, external `_test` packages,
  test-only methods seen from production, and any directory whose `.go` files
  are failed, pattern-recovered, cached without `go_member_names`, or present
  on disk but not indexed. A `Partial` parse vetoes every unexported token it
  spells, which is the right narrowing: a declaration must spell its name.
* **sys.path vetoes (`5c33e532`).** An in-function or class-body insert, an
  unreadable insert, and an entry above the repository root all abstain rather
  than guess. Any non-insert write (`remove`, reassignment, `del`, …) withdraws
  every entry for the file. `thousands_of_imports_behind_many_inserts_resolve_in_linear_time`
  covers the cost bound.
* **Template calls, `#define` bodies, `T::m`, injected decorators (`74419a09`,
  `b3a9edb1`).** The decorator rule is keyed on the binding rather than the
  spelling, and the commit names its negative cases (imported decorator,
  module-level function of the same name, `@other.test`, nearer rebinding). The
  macro-body parse refuses `ERROR` subtrees, comments, strings and the macro's
  own parameters, and fails open (a missing edge) past its size bounds.
* **Module bindings (`bb45211d`).** Making every Python module-scope binding a
  `Variable` widens the dead-symbol surface. `python_constant_liveness.rs`
  asserts both directions: each module-level read shape is live, and a
  genuinely unused constant, a name left out of `__all__`, and a private unused
  name still produce findings, so the fix cannot pass by exempting everything.
* **Gating.** The five new test files declare `required-features = ["parse"]`,
  which is a default feature. `rust.yml` runs per-crate `--no-default-features`
  check and test, so a gated test cannot silently vanish from CI.

## Findings

### 1. `insert(0, dir)` does not shadow an ordinarily resolved import (Medium)

`rust/devmap-resolve/tests/python_sys_path_imports.rs`,
`an_ordinarily_resolved_import_is_not_overridden`, and the matching rule in
`823fd2eb` ("Ordinary resolution always wins") together state:

```python
sys.path.insert(0, str(ROOT / "scripts" / "aws"))
import analyse_wave20 as a20
```

with `scripts/analyse_wave20.py` beside the importer and a second copy at
`scripts/aws/analyse_wave20.py` links to the sibling and asserts no edge to the
`aws/` copy.

At runtime `insert(0, …)` puts `scripts/aws` ahead of the script directory, so
the import reaches `scripts/aws/analyse_wave20.py`. The edge DevMap records is
one Python would not take, and `5c33e532` itself states the doctrine it
violates: "A confident edge runtime may not take is worse than a missing one."
`sys.path.append(…)` is the opposite case and the current rule is right for it.
The repository's own motivating shape (BINN `scripts/aws/` and `scripts/azure/`
holding same-named modules) is the layout where this bites.

Not verified by execution. Suggested direction, not applied: when an entry
recorded by `insert` at index 0 precedes the import and names a different file
than ordinary resolution did, abstain (as the two-directory case already does),
and update the test to assert the abstention. Keep ordinary-wins for `append`.

### 2. Extraction change at an unbumped schema (Low)

`74419a09` sets the schema to 64. `b3a9edb1` (`#define` bodies, `T::m` scope)
changes what is extracted and leaves it at 64; `347dc7cc` bumps to 65 and
`b3a9edb1`'s own message does not mention a bump. A store built at
`74419a09` and read by a `b3a9edb1` binary serves rows without the new edges.
This affects only a bisect or checkout of that one commit and is resolved at
HEAD. Prefer bumping in the commit that changes output.

### 3. Divergence ledger is stale (Low)

`docs/devmap/DIVERGENCES.md` was last updated 2026-09-12 and ends at X48. It
has no entry for the Go unexported-selector rung and its two guards, Python
`sys.path` resolution and its vetoes, Python module constants as symbols, type
aliases as non-call-targets, template and macro-body callees, or the
injected-decorator entry point. The ledger is where this project records
precision-versus-recall decisions with their measurements, so these belong
there. Entries should follow the existing format and cite the test files named
in each commit.

### 4. Measurements are not reproducible here (Info)

Figures such as "1,741 of 104,696 Go `uninferred_receiver` rows",
"1,030 added Go edges", and "nine Metal helpers" were taken on external corpora
at named generations. They are specific and internally consistent, but nothing
in this repository lets a reader reproduce them. Prior audits preserved scripts
and manifests under `.devcouncil/audit/`; this series did not.

## Not covered

Store, Windows hardening and LSP commits in the same window (`4efeecfd`,
`975a79b4`, `9a1d8d2d`, `e3bcb6b1`, `37fd9beb`), the `ask --evidence` pack
(`66cb226a`, `a1a5c3b8`) and `devmap blast`. Run `cargo test --workspace` and
the per-crate `--no-default-features` jobs before relying on any claim above.

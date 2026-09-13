# Attribution and extractor audit — 2026-09-12

Verified fixes preserve the distinction between a missing graph target and the
reason the resolver assigns to that miss. Explicit imports and local generic
bindings now outrank builtin/prelude name tables in both call and type positions.
A bare `self` receiver no longer becomes an explained Rust module path: the
current extraction representation cannot distinguish `self.member` from
`self::member` once only the receiver string remains. A surviving qualified Rust
path such as `self::module` still carries module evidence. No confidence tier or
SQL/PowerShell capability was promoted.

The canonical payload identity advances through `extract-v50` to `extract-v51`.
The unchanged-source build shortcut checks this identity before resolution.
Separate pre-bump CLI regressions reproduced stale v49 builtin classifications
and stale v50 captured-receiver classifications. Both historical upgrade
fixtures remain: unchanged source, seeded old classification, required new
generation, retained corrected attribution sites, and cold-build equivalence.

The original 91,784-site warning remains evidence of incomplete attribution. This
audit does not establish universal coverage, a precision/recall score, or a
product-wide comparison with CodeGraph or another tool.

## Retained benchmark inventory

Verified by read-only SQLite inspection of generation 5 in
`.devcouncil/benchmarks/20260912-expanded/dm-cold-3.sqlite` in the canonical
checkout. Its producer was the preserved dirty benchmark binary; the root audit
retains a separate clean baseline. Do not substitute this inventory for a fresh
candidate-build measurement.

| Classification | Ledger rows | Included in the reported remaining count |
|---|---:|---|
| Uninferred receiver | 89,421 | Yes |
| External import | 49,600 | No |
| Builtin | 18,351 | No |
| Local binding | 2,087 | Yes |
| Module path | 1,683 | No |
| No namesake | 1,069 | No |
| Unresolved | 276 | Yes |
| Host global | 103 | No |
| **Total** | **162,590** | **91,784 remaining** |

The count covers attribution ledger entries, including calls, references, and
imports. It is neither an exact number of missing edges nor a count of distinct
source expressions. A method call and its name reference can contribute separate
entries. Rust contributes 82,802 of the 89,421 uninferred-receiver entries; Go
contributes 5,558, Python 656, and JavaScript 337. Those figures describe this
corpus, which includes many Rust tests; they do not measure language accuracy.

`rust/devmap-analyze/src/resolution_rate.rs::is_explained` excludes five classes:
builtin, host global, external, no namesake, and module path. These are resolver
classifications, not an independent check that source coverage is complete. The
query warning now names all five and states that limitation. All existing
remaining-site, parse-failure, timeout, and traversal qualifications are retained.

## Source-checked cases

Thirteen additional ledger samples were checked against source bytes and the
persisted extraction spans. They were selected by classification and language,
not randomly. The artifact carries both the full row totals and sample count.
The table groups related samples; it is an exploratory triage, not complete
adjudication of 162,590 entries.

| Source and site | Verified observation and implication |
|---|---|
| `backend/contracts/tools/checksums.py:30`, `path.read_bytes()` | The argument is annotated `pathlib.Path` at line 29, but the method is classified uninferred. This exposes a remaining qualifier/type-propagation limitation. The `Path` type itself is correctly external. |
| `rust/dc-evidence/src/lib.rs:258`, range `.contains(&b)` | Both call and name-reference entries remain uninferred. The receiver is an expression; these entries must not become a guessed same-name method edge. |
| `backend/go_orchestrator/cmd/dcmap/main.go:25`, `stop()` | `stop` comes from `signal.NotifyContext`; the invocation at line 26 is correctly recorded as a local callback. |
| `rust/dc-evidence/src/lib.rs:47`, generic `T` | The source declares `T` in the function signature; local-binding classification is appropriate. |
| `backend/go_orchestrator/cmd/devcouncil/main.go:438`, `integrate.Options` | The package qualifier has an unresolved reference entry. It is a type-position attribution limitation, not proof of a missing runtime call. |
| `rust/dc-grep/src/index.rs:134`, `open_for_search` | The imported function has Unix and non-Unix declarations at `rust/dc-grep/src/lib.rs:611` and `:620`. The source contains platform alternatives; choosing one without a build configuration would overstate certainty. |
| `backend/go_orchestrator/cmd/dcmap/main.go:29`, `os.Args` | The explicit standard-library import explains the external reference. |
| `backend/go_orchestrator/cmd/devcouncil/main.go:145`, `execLookPath` | Classified no-namesake, but line 149 assigns a package-level function literal to that variable. Absence from the indexed symbol table does not prove absence of a source binding. Package variable/closure attribution remains a gap. |
| `benchmarks/map_bench.py:836`, nested `self.repo` | Incorrectly classified as a Rust module path. The repaired language/receiver guard retains the unresolved value access. |
| `rust/dc-evidence/src/types.rs:237`, `self.issues` | A field access was likewise classified as a module path. It now remains an attribution gap unless the ordinary resolver finds its target. |
| `backend/contracts/tools/checksums.py:70`, `SystemExit`; `bin/devcouncil.js:144`, `process.argv` | Source supports the builtin and runtime-global classifications respectively. These controls remain explained. |

Additional adversarial fixtures cover imported builtin names in Python and
JavaScript, missing relative imports, a callback shadowing an import, unshadowed
builtins, a nested Python receiver, Rust fields, explicit/missing imports of Rust
prelude types, and a generic parameter named `String`. Existing tests continue to
cover duplicate candidates, external calls, cross-language names, typed local
receivers, genuine module paths, and confidence consistency.

## SQL and PowerShell

Verified: all eight SQL files in the retained corpus have `Partial` parse
outcomes, with 23 recorded error ranges. They also lack the file-import
capability. These are separate limitations. For example, the grammar marks
`CHECK(json_valid(body))` in `rust/dc-store/src/workbench/attention.sql:6` as an
error. The exact source script executes successfully on SQLite 3.53.4 after
creating the `work_meta` migration table in memory; all eight expected columns,
including generated columns, appear in `PRAGMA table_xinfo`. This establishes a
parser coverage gap on valid source, rather than a syntax defect in that fixture.
It does not validate every SQL file or every SQL dialect.

SQL table references and schema qualifiers name database objects. They are not
source-file imports. SQLite's documented SQL statements and generated-column
syntax do not authorize translating those objects into arbitrary `.sql` file
edges. PostgreSQL `psql` supports client include commands, which are distinct from
SQL syntax; supporting those requires an explicit client/dialect contract and
source-aware extraction. This audit leaves the import capability warning intact.
See [SQLite SQL](https://www.sqlite.org/lang.html),
[generated columns](https://www.sqlite.org/gencol.html), and
[psql client commands](https://www.postgresql.org/docs/current/app-psql.html).

Verified: `scripts/install.ps1` uses `RegexFallback`, with two declarations
recovered and an explicit `Fallback` parse outcome because no PowerShell grammar
is linked. Its command invocations, import/dot-source behavior, pipeline flow,
and runtime semantics have not been comprehensively extracted or executed here.
No grammar dependency was added, and this audit does not claim PowerShell parity.

## Verification and remaining work

Before the classifier fixes, three added call/receiver regressions failed
(7 existing/control tests passed), and three additional type-origin regressions
failed (6 existing tests passed). Both logs retain the actual `Builtin` or
`ModulePath` result and the expected source-backed classification. The final
focused resolver run passed 19 tests. The final extractor/resolver crate run
passed 943 tests across 68 test/doc-test result groups, with zero failures or
ignored tests. The disclosure regression failed before the wording fix; all 32
`incomplete_answers_say_so` tests passed afterward. Resolver formatting and scoped
`git diff --check` passed. These runs do not replace full workspace, operating
system, or provider qualification; broader validation is recorded in the root
audit report.

The production resolver change removes more lines than it adds by sharing the
binding-precedence decision for calls and types. It does not alter edge evidence
scores, guess a receiver target, expand imports, or turn fallback parsing into a
successful parse.

The v50 payload bump also required updating two current-version contract tests
in `embedded_scripts.rs` and `extract_bindings.rs`. Those assertions deliberately
pin the current schema; the v49 CLI migration fixture remains unchanged. The
extractor suite then passed 662 tests across 43 result groups, with zero failures
or ignored tests. Historical version references were left intact.

## Scoped mutation validation

Verified using the installed cargo-mutants 27.1.0 against clean candidate commit
`659ed7d4e1fde596ccac141cce180e622681a84b`. The scope is the 33 generated mutations
of `Resolver::classify_unresolved`, running the resolver package's tests. Both
runs used the tool's default separate source copy, one mutation worker, at most
four compiler tasks, a 180-second build timeout, and a 60-second test timeout.
Neither run edited the candidate or reused its target directory.

| Run | Planned / completed | Caught | Missed | Unviable | Timed out | Unexecuted |
|---|---:|---:|---:|---:|---:|---:|
| Initial clean candidate | 33 / 33 | 28 | 4 | 1 | 0 | 0 |
| Exact four survivors, with source regressions | 4 / 4 | 4 | 0 | 0 | 0 | 0 |

The initial unmodified baseline passed 281 tests; the replay baseline passed 284.
The initial run took 333.9 seconds and the replay 73.4 seconds. Each run verified
that its source hashes were unchanged afterward. The one unviable mutation
replaced the return with `Default::default()`, which cannot compile because
`UnresolvedClass` does not implement `Default` (`E0277`). Its runtime test did not
run, and it is not counted as caught.

All four survivors changed observable classification on source fixtures; none
was dismissed as equivalent. Three new tests and an additional case in the
existing `self` regression now catch them:

| Original mutation | Source-level observation under the mutation |
|---|---|
| Line 762, Swift guard `&&` → `||` | Python `import Foundation` incorrectly explains unbound bare `NSString()` as external Swift SDK API. The import binds the module, not that bare name. |
| Line 858, remove prelude veto `!` | A Swift parameter typed as the standard `String` loses builtin classification for `lowercased()`. A local `String` control must retain the attribution gap. |
| Line 912, value-evidence `||` → `&&` | A TypeScript nested function capturing a parameter named `Math`, typed with `type Count = number`, incorrectly becomes a runtime-global access. |
| Line 934, `root != "self"` → `root == "self"` | Rust `self.field` becomes an explained module path merely because the file also has an unresolved `use self::omitted::Type`. |

These fixtures exercise normal and incomplete editor source. The Python bare
name and omitted Rust module intentionally have missing targets; parser-based
classification was tested, not execution or successful compilation of every
fixture. The source tests pass unmutated, fail under their exact original
surviving mutations, and pass strict resolver Clippy. This closes the four
observed test gaps in this bounded mutation set. It does not establish mutation
coverage of the rest of the resolver, extractor, store, or product.

Two exploratory probes also failed on the unmutated candidate: a Rust global
`static std: usize = 0` used as `std.count_ones()` was classified external, and a
TypeScript nested capture of `Math: number` was classified as a host global.
Unlike the named `Count` alias above, the primitive annotation did not protect
that capture. These were reported separately for source-binding diagnosis and
are not credited as caught mutants. Their disposition follows below. An initial
ambiguous-constructor hypothesis was also falsified:
ordinary ambiguous calls already emit speculative candidate edges and never
reach this classifier. Those exploratory failure logs are retained.

## Final v51 disposition

The captured-receiver follow-up uses existing `LocalBinding` facts at the exact
call or reference offset. A positive local receiver binding prevents the
classifier from explaining a captured parameter as a same-named module import
or host global. It also prevents borrowing an imported type from a sibling or
enclosing graph scope; anonymous bindings without graph identities remain local
evidence. Existing typed imported and Swift prelude receivers retain their
classifications. This is a resolver change, with no new extraction dependency
or payload field. Missing site facts still retain the previous conservative
fallback; absence of a recorded binding is not treated as proof of no binding.

The actual CLI v50 upgrade fixture uses
`outer(Math: number) { function inner() { return Math.toFixed(); } return inner(); }`.
With the repaired resolver but identity still v50, its fresh-build classification
precondition passed, then the unchanged rebuild failed exactly:
`unchanged source must not reuse a v50 generation after attribution semantics change`.
After the canonical identity advanced to v51, all 10 incremental-equivalence
tests passed, including the unchanged v49 and v50 upgrades. The two current
identity contract targets passed 26 tests (18 embedded-script tests and 8 binding
tests). Scoped formatting, diff checks, and strict CLI migration Clippy passed.
The migration evidence uses the isolated worktree's debug CLI; final clean
release and full-workspace qualification belong to the root audit report.

The private Rust static remains a demonstrated limitation. Its declaration is
deliberately omitted by the existing private-module-binding symbol filter, and
the callable-only binding collector supplies no file-level value fact. Moreover,
reduced receivers do not preserve the distinction between `std.member` and
`std::member`; adding a blanket name veto would conflate Rust's value and module
namespaces. Closing this gap requires an explicit extraction contract for
file-level value bindings and access shape.

A second conservative limitation remains when anonymous callback membership
leaks into the graph caller's fallback scope: a later genuine `Math.max` can stay
`UninferredReceiver` when there is no positive use-site binding. That retained
failure is separate from the fixed captured-parameter misclassification. The
current optional site facts do not distinguish unavailable evidence from a
verified unbound name, so this follow-up does not erase the fallback and claim
complete lexical coverage.

Remaining unverified work includes source-wide adjudication of unresolved sites,
package-level function-valued variable ownership, broader typed-receiver
propagation, private file-level value bindings, complete anonymous lexical scope
coverage, platform-conditioned target selection, SQL dialect/client includes,
and a linked PowerShell parser. All-language confidence requires independent
source ground truth and workload-specific provider/configuration evidence; the
selected positive pairs and this targeted audit do not supply it.

Artifacts in the isolated audit worktree:

- `.devcouncil/audit/attribution/triage.py` and `triage.json`: read-only inventory,
  full class/language counts, 13 source-checked samples, exact source hashes,
  and all SQL/PowerShell parse outcomes; each sample is explicitly capped.
- `.devcouncil/audit/attribution/sqlite-syntax.json`: isolated SQLite source check.
- `.devcouncil/audit/attribution-prefix.log` and `attribution-type-prefix.log`:
  pre-fix failures.
- `.devcouncil/audit/attribution-final-focused.log`: 19 focused passing tests.
- `.devcouncil/audit/attribution-final-crates.log`: extractor/resolver suite after
  semantic fixes, before the identity-only v50 bump.
- `.devcouncil/audit/attribution-upgrade-prefix.log` and
  `attribution-upgrade-final.log`: real CLI upgrade regression and incremental
  equivalence suite (9 passed after the v50 bump, zero failed or ignored). An initial test-only `usize`/SQLite `FromSql` compile error
  was corrected to `i64`; its log is retained as
  `attribution-upgrade-compile-failed.log`.
- `.devcouncil/audit/attribution-disclosure-prefix.log` and
  `attribution-disclosure-final.log`: disclosure regression and verification.
- `.devcouncil/audit/attribution-extractor-v50.log`: the extractor suite after
  updating the two current-version contracts.
- `.devcouncil/audit/attribution-mutations/summary.json`, `run.json`,
  `inventory.json`, and `mutants.out/`: exact initial mutation inventory,
  provenance, all outcomes, diffs, baseline, and per-mutation logs.
- `.devcouncil/audit/attribution-mutations/survivor-rerun/`: separate replay
  provenance, the exact test-only patch from the clean candidate, all four
  failing-mutant logs, and the passing unmodified baseline.
- `.devcouncil/audit/attribution-mutations/regressions-baseline-final.log` and
  `clippy-regressions.log`: 33 focused passing tests and strict scoped Clippy.
- `.devcouncil/audit/attribution-v50-upgrade-prefix.log` and
  `attribution-v51-prefix-provenance.json`: the pre-bump generation-reuse failure
  after the captured-receiver semantics were repaired, with source hashes.
- `.devcouncil/audit/attribution-v51-upgrade-final.log`,
  `attribution-v51-contracts.log`, and `attribution-v51-clippy.log`: all 10
  incremental-equivalence tests, 26 current-contract tests, and strict CLI
  migration Clippy after the v51 bump.
- `.devcouncil/audit/captured-receiver-baseline.log` and
  `captured-receiver-including-anonymous-offset.rs`: the follow-up source probes,
  including the separate retained anonymous-offset fallback limitation.

## Final restored-corpus measurement

The final clean v51 release `962ea5f07921` was rebuilt and run over the restored
historical corpus. Its native warning reports **96,233 remaining of 162,590
attribution sites**, with 66,357 classified as explained, versus the historical
91,784 and intermediate v50 candidate 92,904 remaining. The earlier 95,215 tally
used an incorrect exclusion set; it has been corrected from the native owner,
`blast_radius.layers.walk_incomplete` in
`.devcouncil/audit/update-profile/delivery-v51/explore-coverage.stdout`.
It reports 10,193 nodes, 33,789 edges, zero confidence
mismatches and fresh source/analyzer state. Counts alone do not adjudicate all
changed sites or establish caller recall. These final measurements are preserved
in `.devcouncil/audit/delivery-campaign/independent-verification.json` and
`update-profile/delivery-v51/status.stdout`; the broader audit retains all
incomplete-coverage and parser/fallback disclosures.

The v50-to-v51 full-value comparison uses identical source manifests. Canonical
digests match for all 10,193 nodes and 33,789 edges, including confidence and
resolution evidence with database IDs normalized to paths. Ledger identities,
reasons, and receivers also match. The recorded difference is 3,329
`External` → `UninferredReceiver` classifications; this is not a loss of graph
edges. The comparison is preserved in
`.devcouncil/audit/update-profile/delivery-v51/graph-and-classification-comparison.json`.

Two changed Go sites were then checked against their exact source and extracted
facts. In `artifact.go::scanMarker`, `dec.Token` has an initializer recorded as
`json.NewDecoder` but no declared receiver type; the `Decoder`/`json` type facts
belong to parameters in `scanObject`, `skipValue`, and `consumeValue`. Those
explicitly typed controls still classify as external. In
`protocolfuzz_test.go::FuzzDevmapNoticesNeverSilentlyDropALine`, source declares the
anonymous callback parameter `t *testing.T`, but its extracted binding has no
declared type. The assigned `T`/`testing` references belong to the separate
`assertShapeIsHonest` parameter. Thus the proposed loss of existing binding-specific
type evidence was falsified for these two cases: the earlier classification had
borrowed another parameter's facts. Source-known origin and recorded attribution
evidence are distinct; Go callback-annotation extraction and factory-result type
inference remain limitations.

This bounded check does not adjudicate all 3,329 classification transitions and
does not warrant a v52 identity bump or another production change. Evidence is
preserved in `.devcouncil/audit/go-binding-diagnosis.json`,
`go-binding-exploratory-probes.rs`, and `go-binding-exact-corpus-collected.log`
(one collected source-facts probe). The earlier `go-binding-exact-corpus.log`
collected zero tests and is excluded from validation. All temporary probes were
removed; `go-binding-existing-tests-retained.log` verifies that the nine existing
captured-receiver tests remain passing.

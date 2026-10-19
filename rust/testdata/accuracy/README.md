# DevMap accuracy fixtures

Six labelled fixtures from the 2026-10-09 language audits (Rust, Go, TypeScript,
Python, Swift, Kotlin). Each fixture's `expected.tsv` / `EXPECTED.tsv` was
written **before** DevMap ran on it; that ordering is what makes it truth rather
than a snapshot. Source only — no `.git`, `node_modules`, `target` or `.build`.

`dead.tsv` in each fixture is the dead-code truth that
`devmap-cli/tests/dead_tier_eligibility.rs` reads. It binds
`devmap_analyze::CONFIDENT_TIER_ELIGIBLE` to measurement: a language is listed
exactly when its fixture rows that reach the confident branch are all labelled
`truly_dead`, every `truly_dead` marker of that language reaches it, and the
corpus record below holds labelled rows for it (see there).
Rows added for the dead-policy card (Go `W1`–`W5`, Rust `C50`/`C51`, TypeScript
`V19`, Kotlin `K30`/`K31`, Python `replay.py`; second pass: Rust `C52`–`C54`,
Kotlin `K32`/`K33`, Swift `P5`/`P6`, TypeScript `V20`, Python `handlers.py`;
third pass: Swift `P7` `@objc`/`#selector`, Rust `C26` operator traits as
`exempt`) are marked as such where they are declared.

Every row `dead` reports on a fixture must be labelled in its `dead.tsv`; the
test fails on an unlabelled one, because an unlabelled row is a row nobody
scored. Labels added on 2026-10-10 for rows the first pass left unlabelled
(Kotlin/Java, TypeScript/TSX) were read off the fixture source — the `why`
column names the call site or says nothing references it — not copied from
DevMap's answer.

Second-pass rows (2026-10-10) cover what the corpus record found: Kotlin
`K32`/`K33`, Rust `C52`–`C54`, Swift `P5`/`P6`, TypeScript `V20`, Python
`handlers.py`.

Known miss, owned elsewhere: Python's `other/dup.py::split`/`helper` are
`truly_dead` but reported at 0.4 `only_ambiguous_callers`. `from pkg.util
import helper` under a `lib/` source root does not bind, so the call falls to
the bare-name global rung, which offers `dup.py`'s namesake too — though
`other/dup.py` can never be module `pkg.util`. Python source roots (ft-837a6f05)
and S2's bare-name binding (ft-32522da4) own it; until then the miss keeps
Python unproven on its fixture as well as on the record.

`dead_unconfirmed_record.tsv` is the recorded 0.4-tier false-positive rate per
language (rows labelled `live` or `exempt` among the 0.4 rows). It has no gate,
since 0.4 is "unconfirmed" by contract, but the test binds it to the fixtures so
it moves only in the same change as the behaviour that moved it. On 2026-10-11,
after the dead-policy lane merged with the language lanes: Go 2/4, Java 0/1,
Kotlin 3/11, Python 1/25, Rust 0/3, Swift 0/8, TSX 1/7, TypeScript 4/25. Python's and Swift's 0.4 rows include their fixture rows capped
`confident_tier_unproven` since each came off the list. These are probe-dense
audit fixtures, many of whose `live` rows are probes built to be reached only
through shapes the resolver cannot bind — the namesake veto keeping them out of
0.9 is the intended outcome — so the rates describe the fixtures, not a corpus.
The < 50% target per language is not met by Go on these fixtures (the target
is for after each language card lands).

## The gates

`edges.tsv` in each fixture is the normalized edge truth (columns documented in
its header): caller, callee, `edge`/`import`/`forbid`/`forbid_any_tier`,
`must`/`should`. Rows were
derived from the audit's original labels and the language's own semantics, never
from DevMap's output. `devmap-analyze/tests/accuracy_fixtures.rs` runs the
product's own scan -> manifests -> extract -> resolve -> analyze path over every
fixture and gates, per language:

1. precision at >= 0.9 (no HIGH edge to a forbidden or wrong-namesake symbol),
2. honest miss (an absent expected edge has an unattributed ledger row naming it),
3. dead code (`dead.tsv`: no confident-branch row that is not truly dead, every
   truly-dead symbol reported, no live symbol above 0.4),
4. recall as a ratchet.

A `forbid` row rules out an edge at >= 0.9 only. A criterion worded "no edge of
any tier" needs a `forbid_any_tier` row, which fails on a use edge at any
confidence: TypeScript `ts.V2`, Swift `sw.M4b`/`sw.M4d`, Kotlin `kg.S2a`/`kg.S2b`,
Rust `rr.count` and Go `go.Q2c.*` (no edge between two functions' same-named local
types). Every forbid row must name a caller and callee the build actually has —
one naming a renamed or mistyped symbol would pass vacuously, so it fails. A
language with no expected-edge row, no HIGH edge to score precision on, or no
truly-dead row fails as unmeasured; a rate over nothing is written as `null` in
`record.json`'s `fixtures`, never as 1.0.

Rows that fail today are in `known_failing.tsv`, each keyed by the card that
owns the fix. The test fails when a listed row starts passing, when a passing row
regresses, and when `ratchet.json` (counts: present edges and expected and
`forbid_any_tier` rows may not fall, known failures may not grow) would move the
wrong way. After an
improvement, `DEVMAP_ACCURACY_BLESS=1 cargo test -p devmap-analyze --test
accuracy_fixtures` deletes the fixed rows and rewrites `ratchet.json` and the
`fixtures` section of `record.json`; it refuses to write a worse state and never
adds a row. A card's acceptance is "its rows are out of `known_failing.tsv`".

Beyond the six audit fixtures, `rust-rootcrate/`, `python-{hatch,ruff,setupcfg,
nomanifest}/` and `kotlin-gaps/` carry the cases the manifest, resolver-seam and
Kotlin cards name that the audit fixtures did not (a crate at the repository root,
one source-root manifest per fixture, a function-typed parameter beside a
namesake method, `apply {}` over a namesake, callable references, `invoke`,
import aliases, `super.m()`, companions, nested constructors). They carry no
`dead.tsv`, so the dead-tier eligibility binding does not read them.

## The corpus record

`corpora.json` pins one commit per private corpus. The `#[ignore]`d runner
`devmap-analyze/tests/real_corpus_accuracy.rs` takes `DEVMAP_CORPUS_<NAME>=<checkout>`,
refuses any other commit or a dirty tree, and writes `record.json`:

* `measurements`: label-free counts per corpus and language (edges by resolution
  kind, HIGH edges, AmbiguousGlobal edges, unresolved rows by class, resolution
  rate, dead rows by tier). Written for every corpus run.
* `corpora`: labelled rows (confident-edge precision, UniqueGlobal precision,
  recall by construct, honest-miss, dead false positives by tier with counts), in
  the shape `devmap-cli/tests/dead_tier_eligibility.rs` reads. Written only for a
  corpus that has labels, which live **beside the corpus**
  (`<checkout>/.devmap-accuracy/labels.json` or `DEVMAP_CORPUS_<NAME>_LABELS`)
  because they name private paths and symbols. A corpus without labels gets no
  row: an unmeasured number must not look like a passing one.
* `superseded`: every `measurements` or `corpora` row a later run replaced, with
  the date — superseded, never deleted.
* `open_gates`: every gate the record does not meet, over every pinned corpus
  and language, whether or not the last run measured it. CI
  (`the_committed_record_lists_every_gate_it_does_not_meet`) recomputes it from
  the committed record and fails when it is out of date, so the record cannot
  claim a clean bill it does not have. The runner rewrites it;
  `DEVMAP_ACCURACY_BLESS=1 cargo test -p devmap-analyze --test
  real_corpus_accuracy the_committed_record` rewrites it from the record alone.
* `fixtures`: the per-language fixture measurement, rewritten by the bless above.

Labels file shape:

```json
{
  "edges":  [{"language": "go", "caller": "<symbol id>", "callee": "<symbol id>", "correct": true}],
  "recall": [{"language": "go", "construct": "typed receiver", "caller": "<symbol id>", "name": "Get", "truth": "<symbol id>|<symbol id>"}],
  "oracle": [{"language": "go", "covered_callers": ["<symbol id or file>"],
              "uses": [{"caller": "<symbol id>", "callee": "<symbol id>", "impls": ["<symbol id>"]}]}],
  "dead":   [{"language": "go", "id": "<file>::<symbol>", "truly_dead": false}]
}
```

* `edges` are hand judgements of edges DevMap emitted; one naming no edge in
  the build is an error (regenerate the labels).
* `recall` rows are call sites with their true callee; `a|b` in `truth` means
  an edge to any one of them is the call (an interface method or one of its
  implementations).
* `oracle` is a type checker's account of a language: every use of an in-repo
  function or method it bound, from every caller it read. Every confident
  Calls/References edge to a function or method from a covered caller is
  judged against it (right when the oracle bound the pair or the interface
  method it implements); edges from callers it did not read (build-tagged
  files) stay unjudged. It depends on the corpus alone, so it does not go
  stale when DevMap's edges move.
* `dead` should label every confident-branch dead row of each language the
  file labels; the row always carries `dead_fp_by_tier.0.9`, which counts only
  the rows that were scored. A confident row with no label does not abort the
  run: it is counted as unmeasured, per corpus and language, in the row's
  `dead_unmeasured` block (`unlabelled`, a capped `unlabelled_sample` of ids,
  `sample_cap`). The sample is filled for DevCouncil's own corpus only; a
  private corpus's raw ids go to the run's stderr, never the committed record.
  A label that cannot be written safely (a private label missing or misshaping
  `symbol_hash` or `category`, or an own-corpus label missing `evidence`) is
  likewise reported, as `malformed` with a capped `malformed_sample` of
  reasons that name the label's index and never its text, and is left out of
  the rows and the rate. A row whose label is malformed is counted once, as
  malformed. A language with any unlabelled or malformed dead row has an
  unmeasured 0.9-tier dead false-positive gate, which fails: unmeasured is
  never a pass, and the labelled rows are scored exactly as before. An
  `unsure` label counts as labelled (it is listed, scored neither way).
* A pinned language the file says nothing about gets no `corpora` row at all
  (its gates stay unmeasured), never an empty one.
* **`honest_miss` is split three ways.** For each recall site whose truth edge
  is absent: `recorded` (an unattributed ledger row names the call), `silent`
  (no ledger row and no edge at all) and `wrongly_bound` (the caller has a HIGH
  Calls/References edge for that call name to a non-truth target, read from the
  build's edges, and none of those same-name edges is judged correct by an edge
  label or the oracle). The rate is `recorded / (recorded + silent)`;
  `wrongly_bound` is outside the denominator because it is precision's defect
  and counting it again as a silent miss would move two gates for one fault.
  Limits: precision debits that edge only when an edge label or the oracle
  judges it, so a wrong HIGH edge nobody judged is reported as `wrongly_bound`
  and shows in recall, but in no precision count. And `(caller, name)` cannot
  tell two call sites of one name apart: a correct same-name edge elsewhere in
  the caller leaves a missing site `silent`, never `wrongly_bound`. A ledger
  row is matched on the whole call name, not its last segment. `absent` keeps its old meaning
  (all three summed), so rows committed before the split stay comparable; they
  carry no `silent` or `wrongly_bound` key.
* **The dead gate has three states**: measured (a rate over >= 1 confident row,
  all labelled), *empty tier* (the language was labelled and scored but no row
  reached the 0.9 tier: `rows` is 0) and unmeasured (no labels, or an
  unlabelled or malformed row). The gate output names empty tier as
  `empty tier (0 rows)`, distinct from `is unmeasured`. **An empty tier does not
  pass**; it blocks a green verdict exactly like unmeasured. Whether an empty
  tier may ever pass is an open OWNER decision; the default is strict.
* **A committed row never silently loses its labels.** When `record.json` holds
  a runner row for a corpus and language and this run finds no labels file, or
  none for that language, the runner keeps the committed row, stamps it
  `"unverifiable": "<reason>"` and fails the gate naming the corpus and
  language (`<corpus>: <lang>: committed labelled row is unverifiable: ...`).
  `open_gates` lists it from the committed record, its stale numbers are not
  read, and `dead_tier_eligibility.rs` counts it as no proof. Any non-null
  `unverifiable` value counts, in the runner and the CLI alike. When the pinned
  corpus commit has moved since the row was measured, the entry is written under
  the pin's commit and the reason names both commits. A later run that scores
  the language replaces the row and clears the stamp.
* A `dead_unmeasured` block, or a `dead_fp_by_tier.0.9` count, that is not
  non-negative integers (or has more false positives than rows) is unmeasured in
  the runner; the CLI reader refuses it (it panics, as it always has).
* The runner refuses a labels file that is not a JSON object, has a section
  other than `edges`, `recall`, `oracle`, `dead`, or has one that is not an
  array.

**Merging a label store.** Labels live beside the corpus, one file per language
and round (`labels-<lang>[-rN].json`). `tools/accuracy/cmd/labelmerge` merges a
directory of them into the single file the runner reads:

```text
go -C tools/accuracy run ./cmd/labelmerge -dir <store> [-private] \
  -o <checkout>/.devmap-accuracy/labels.json
```

The output has the four arrays (`edges`, `recall`, `oracle`, `dead`) in a fixed
order (sorted by language, then caller/callee/name/id), entries' fields sorted,
unknown fields kept. A site two files label differently (same language and
caller and call name, edge pair, dead id or oracle language) is an error naming
both files, never last-wins; an identical repeat in a later round is kept once.
Within one file a repeated site is refused too (identical or conflicting; two
recall rows may share caller and name if their `truth` differs), as is a
repeated JSON key, a file over 64 MiB, a store whose files hold no labels in
total, and a case variant of a label file name (`Labels-go.json`). Names that
differ otherwise (`label-go.json`, `labels_go.json`) are not label files and
are skipped. The output is written through a temporary file, synced and
renamed, and `-o` may not be inside the store or a symbolic link.
It also refuses a malformed file, a row whose `language` is not the file's, a
`labels-*.json` name that does not follow the pattern, an empty store, a
`symbol_hash` that is not `h:` + 12 lowercase hex digits, and an `-o` that is
(or links to) one of its inputs. `-private` additionally requires `symbol_hash`
and an evidence `category` on every dead label, as the runner does for any
corpus but DevCouncil's. A test keeps its hash and category rules equal to
`record_identity.rs`, and the runner's `parse_labels` is tested against its
golden output.

`corpora` holds two kinds of row under one corpus name: the labelled dead
record below (`commit`) and the runner's rows (`corpus_commit`). A runner row
supersedes only the runner row before it.

**Go labels** come from `tools/accuracy/cmd/golabel`, a `go/types` oracle
(`golang.org/x/tools/go/packages`, owner decision D-8, in its own module, not
the host's): it loads every module under the root with its tests and writes
`recall` (constructs: the audit's receiver shapes) and `oracle`. Dead rows are
a judgement it does not make; `-dead <file>` merges hand-labelled ones:

```text
go -C tools/accuracy run ./cmd/golabel -root <checkout> -dead <dead.json> \
  -o <checkout>/.devmap-accuracy/labels.json
```

`go.labels.json` beside this README is its output on the Go fixture. Its
golden test (`go test ./...` in `tools/accuracy`) keeps the file equal to the
labeller; `the_go_oracle_labels_name_the_fixture_build_and_score_it_clean`
scores DevMap against it, so the labeller's symbol ids and DevMap's are held
equal on every construct the fixture has.

**Rust labels** come from `rust/tools/rustlabel` (workspace member
`devmap-rustlabel`, `publish = false`, `dist = false`, so no released binary
links it, like `tools/soak`). It asks rust-analyzer `textDocument/definition`
at every call, method call, function used as a value and overloadable operator
in every `.rs` file DevMap indexes, through the one language-server client the
workspace has (`devmap-resolve`'s `lsp.rs`, extended with a session API for
it), and writes the same file `golabel` writes. It adds no dependency: the
walk is `tree-sitter-rust`, the symbol ids come from `devmap_extract`.

```text
cargo run --release -p devmap-rustlabel --manifest-path rust/Cargo.toml -- \
  -root <checkout> [-dead <dead.json>] \
  -o <checkout>/.devmap-accuracy/labels.json \
  -report <checkout>/.devmap-accuracy/rustlabel-report.json \
  -trace <checkout>/.devmap-accuracy/rustlabel-trace.txt
```

Needs `rust-analyzer` and the `rust-src` component for the toolchain that runs
in the checkout (`rustup component add rust-src`); without std sources every
call that goes through a std type is unanswerable, so it refuses to start. It
starts rust-analyzer with `CARGO_NET_OFFLINE` (pass `-online` for a checkout
whose dependencies are not fetched) and a build directory of its own (the
checkout is never written; `-target-dir` keeps one between runs).
`-max-sites`, `-deadline-secs`, `-request-secs` and `-load-secs` bound it; the
defaults are 2,000,000 sites, 4 h, 30 s and 15 min. `-no-references` labels
calls only.

`rust.labels.json` beside this README is its output on the Rust fixture. Its
golden test (`cargo test -p devmap-rustlabel --test it golden -- --ignored`,
`RUSTLABEL_UPDATE=1` to rewrite) keeps the file equal to the labeller; it is
`#[ignore]`d because it needs rust-analyzer and `rust-src`, so a default run
reports it as ignored, never as passed, and an explicit run without them
fails. `the_rust_oracle_labels_name_the_fixture_build_and_score_it` (in
`real_corpus_accuracy.rs`, runs everywhere) scores DevMap against the file:
every id is one the build has, both recall classes are measured, every labelled
site has its edge, and the one confident edge the oracle judges wrong is named
in the test.

What a label is worth:

* A site rust-analyzer cannot answer is **unknown, never a label**, and its
  caller is left out of the oracle's `covered_callers`, so DevMap's edges from
  it are unjudged, not judged wrong. The report counts every site once:
  `labelled + external + unknown + skipped = sites_enumerated`, by reason
  (`no_answer`, `request_error`, `session_lost`, `deadline`, `target_unnamed`,
  `mixed_targets`, `impls_unanswered`, `operator_unresolved`, `over_cap`,
  `non_function_target`, `not_in_repo_name`), and `asked` against `-max-sites`
  carries the cap. A session that dies mid-run keeps its labels and counts the
  rest as `session_lost`; a workspace that does not load, or reports
  `health: error`, produces no file.
* "Defined outside the repository" (`external`) is an answer; it labels nothing
  and does not uncover the caller.
* A site that spells no function name the corpus declares (nor a `use ... as`
  alias) is not asked (`not_in_repo_name`): it cannot bind in the repository.
  A function a macro generates under a name no `fn` item spells is invisible to
  that rule.
* Trait methods: a call bound to a trait method's declaration has `truth`
  `decl|impl|impl...` (`textDocument/implementation`), the same shape as Go's
  interface dispatch; a call bound to a trait implementation's method also
  accepts the trait's declaration (`textDocument/declaration`), because DevMap
  keeps the written type (`impl Trait` returns) and rust-analyzer does not.
* Macros: the grammar does not parse a macro's arguments. A call inside one is
  not a site, and any macro invocation or `macro_rules!` body that spells the
  name of a corpus function, and every invocation of a macro the corpus defines,
  uncovers its caller.
* Operators, `?` and indexing are sites (`non_self`), asked at the operator
  token. rust-analyzer answers for whichever token neighbours the offset, so an
  operator answer counts only when it is a function the operator's trait names;
  otherwise it is `operator_unresolved` (unknown). `t[0]` is always that.
* **Implicit calls are not seen**: scope-end `Drop`, `.into()` reaching `From`,
  `for` reaching `IntoIterator`, `{}` reaching `Display`, deref coercion, and a
  call through a `const` function pointer. A confident DevMap edge that rests
  on one is judged wrong by the oracle without being wrong, so precision over a
  corpus is a **lower bound**. The report lists, by name, the std-trait method
  names the corpus declares (`implicit_callee_names`: `drop`, `from`, `next`,
  `fmt`, ...); read the precision number against it.
* Symbol ids are DevMap's own (`devmap_extract`'s qualified names), because
  they are the join key. A naming defect in the extractor therefore appears on
  both sides of a label and is not caught by it; a binding defect is.
* Classes: `free_self` is a bare or module-qualified call, a function used as a
  value, and anything through the enclosing type (`self.m()`, `Self::f()`);
  `non_self` is a method call on any other receiver, a path to a method of a
  named type or trait, and an operator.
* Code behind a `cfg` that is off, a crate outside the Cargo workspace
  (`tauri_like` in the fixture) and a file no crate includes are `no_answer`.
  Proc-macro-generated and build-script-generated code resolves only when the
  build ran (`-no-build-scripts` turns it off).

`-trace` writes one line per site (`file:line name outcome`); it names private
symbols, so keep it beside the corpus.

For Rust, `construct` is `free_self` or `non_self`, the two classes the recall
floors (Rust 91 / 46) are set on. The gates: confident precision >= 0.95,
honest-miss >= 98%, 0.9-tier dead false positives <= 5%, no regression against the
previous row (precision -1 pt, recall -2 pt), and the recall floors in
`corpora.json`. A gate that held in the previous row must keep holding; one that
did not may only stay put, so the first record is a baseline and each language
card moves it.

The claims are gated absolutely, whatever the previous row said:

* recall at or above each language card's target (`recall_targets_percent`: Go 95,
  Python 88, TypeScript 85, Rust 95 free/self and 75 non-self, Swift 90, Kotlin 85),
  kept separate from the baseline floors, which stay at the measured baseline
  until a run measures better. Neither table may be lowered, and every language
  needs an entry in both; a missing one is reported as ungated;
* UniqueGlobal precision >= 0.95 (vacuous only when the corpus has no UniqueGlobal
  edge at all);
* zero AmbiguousGlobal edges (S2 made AmbiguousGlobal ledger-only).

Each of them, and every labelled gate, is *unmeasured* — and fails — when the
number it needs is missing: a corpus without labels is gated as well, not skipped.
The runner also refuses to measure with a DevMap that does not contain every
commit in `record_must_include` (S2, d0552ae1, and each language lane's merge),
and stamps that list into each measurement as `includes_commits`; a row without it
is listed in `open_gates` as stale.

The runner also refuses a row measured on a degraded build. The load guard is
code (`check_build`), not a habit; each refusal names its cause and ends with the
guarded retry, `RAYON_NUM_THREADS=4 DEVMAP_CORPUS_<NAME>=<checkout> cargo test
--release -j 4 -p devmap-analyze --test real_corpus_accuracy -- --ignored
--nocapture`, run on an idle machine:

* **budget refusal** - any file still refused for the parse budget after the
  serial retry (the `budget_refused.total` of `devmap status`, counted by
  `Extraction::is_budget_refusal`). The refusal names up to 50 paths and carries
  the total.
* **parse failure** - more files failed to parse (budget refusals excluded) than
  the corpus's `parse_failed` baseline in `corpora.json`. A count below the
  baseline is allowed but printed, so the baseline can be tightened.
* **unverifiable** - the build's status could not be read, or the corpus has no
  `parse_failed` baseline (add one), or it is not a non-negative integer.

`parse_failed` lives in `corpora.json`, beside the pin it describes, because it
is a property of the pinned commit that a reviewer changes with it; `record.json`
holds only measurements. Every pin carries the key (CI checks it): an integer
from 0 to 100000, or `"unmeasured"` for a corpus whose idle count was never
taken, which refuses at run time until one is measured and added. Measured:
scholarlm 8, Lappi-decision 0, GitPulse 0, DevType 0, ChronosFlow 3. Unmeasured:
LiquiTask, SalEdge, AcademiaTrack, Chronicle, DevPrism. Duplicate corpus names
refuse at load. Paths and messages in refusal text are escaped and capped (512
bytes each, 50 paths), and the retry command names `DEVMAP_CORPUS_<NAME>_LABELS`
when the run used it. `record.json` is replaced atomically (temp file, fsync,
rename).

**Deferred.** The baseline is a count, so a different file failing at the same
count is invisible (`characterisation_a_swapped_failing_file_...`). Recording
the failing set would leak private paths unless hashed as `record_identity`
does; that is a design question for the owner.

A refused run writes nothing: `record.json`, `open_gates` included, stays as
committed. The refusal text lists the gate each committed row fails as while
stamped `unverifiable` with the cause, the same stamp a missing labels file gets.

Run the runner before any DevMap release and after a change to
`treesitter.rs`, `langcalls/`, `langdecl/`, `resolver.rs` or `liveness.rs` that
touches the language.

## The labelled dead record

`record.json`'s `corpora` holds the labelled dead record (2026-10-10): 20
local repositories, each a `git archive` of the commit it names, built with the
binary `built_from` names.
Every corpus built is listed, with every listed-or-measured language it holds
dead candidates or exempt symbols of; `rows: 0` means the language was measured
there and its confident branch was empty, which is not the same as never run.
Every dead row whose evidence reached the confident branch — published at 0.9,
coverage-capped below it, or capped `confident_tier_unproven` — is labelled
from source (`rg -uu -w` and reading) with its evidence: `dead_true`,
`dead_false` with the `path:line` that reaches it, or `unsure` (listed, kept out
of `rows`). Labels were made by one pass per language and spot-checked; three
were corrected and say so in their evidence. The eligibility test binds each
listed language to it: at least one labelled row, and at most 5% false on every
corpus; a missing record, or one missing `corpora`/`name`/`languages`, fails.
Measured: Go 0/17 (one corpus of six), TypeScript 0/29, TSX 0/14, Rust 0/3
(two corpora of twelve), Kotlin 0/9 — all nine coverage-capped at 0.66, and
its two rows published at 0.9 are `unsure` — Python 17/58 (off the list), Swift
no row on ten corpora (off the list). The coverage-capped rows count because
their evidence is the confident branch's; the fixtures have no coverage loss,
so the two definitions agree there.

Identity is not committed. In every corpus but DevCouncil's own, a
`labelled_rows[]` row's `symbol` is `h:` plus the first twelve hex digits of the
SHA-256 of the original id, and its `evidence` is one category token; the
`verdict` and `published` fields are unchanged, and no row was dropped. The
labeller's prose lives beside the corpus with the labels. The tokens:

* `rg_declaration_only` - `rg -uu -w` finds the declaration as the only code
  reference (other hits are comments, docs, re-exports or unrelated namesakes).
* `direct_reference` - a call or use at a `path:line` reaches it (`dead_false`).
* `framework_hook` - a framework or import-system hook calls it (`dead_false`).
* `runtime_import_by_name` - imported by module name at run time (`dead_false`).
* `spot_check_upstream_shadow` - the verdict rests on upstream library source
  outside the corpus (`unsure`).
* `spot_check_comment_only` - the only use sits inside a comment.

`devmap-cli/tests/record_names_no_private_identity.rs` fails on a path-like or
`::`-qualified `symbol`/`evidence`, a `symbol` that is not a hash token, a prose
`evidence`, or a row missing either field.

The eligibility test binds each listed language to its corpus 0.9-tier
false-positive rate through `corpora[].languages.<lang>.dead_fp_by_tier`. The
shape it reads — anything else in the file is ignored, and a listed
corpus/language missing these fields fails the test rather than passing it:

```json
{
  "corpora": [
    {
      "name": "scholarlm",
      "languages": {
        "go": { "dead_fp_by_tier": { "0.9": { "false_positive": 0, "rows": 21 } } }
      }
    }
  ]
}
```

A listed language must have `false_positive * 20 <= rows` (at most 5%) on
every corpus that measured it, and `rows` summing to at least one across them.

Some listings still rest on few rows — Rust has 3 corpus rows (DevCouncil's
own Rust has 58 dead rows, every one 0.4: 19 vetoed by an unresolved namesake,
39 `exported_unreferenced`), Kotlin 9, all coverage-capped (see
`CONFIDENT_TIER_ELIGIBLE`'s documentation). The record is re-measured by hand:
re-label any new confident-branch row when a change moves the corpora.

## Swift member properties: build cost

Making Swift member properties symbols (ft-6a46dddd) is measured with `devmap build`, release, interleaved base/head x5, min of 5 instructions retired, against 9030d08c. The Swift fixture costs +2.0%; 2000 types sharing `id`/`name` costs 1.26x (no worse than unique names, 1.28x); one type with 30,000 properties costs 1.63x instructions and 395 MB against 189 MB max RSS. The last was accepted because the ratio measures symbols the base never created: it stored 4 symbols for that file, and persisting 30,000 of them is about 30% of the head profile and grows linearly. The store-insert cost is the follow-up, ft-f6abefd9. The quadratic paths the shape exposed (the per-symbol export scan and the same-name lexical scan) are fixed.

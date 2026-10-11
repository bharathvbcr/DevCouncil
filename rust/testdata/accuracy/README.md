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
its header): caller, callee, `edge`/`import`/`forbid`, `must`/`should`. Rows were
derived from the audit's original labels and the language's own semantics, never
from DevMap's output. `devmap-analyze/tests/accuracy_fixtures.rs` runs the
product's own scan -> manifests -> extract -> resolve -> analyze path over every
fixture and gates, per language:

1. precision at >= 0.9 (no HIGH edge to a forbidden or wrong-namesake symbol),
2. honest miss (an absent expected edge has an unattributed ledger row naming it),
3. dead code (`dead.tsv`: no confident-branch row that is not truly dead, every
   truly-dead symbol reported, no live symbol above 0.4),
4. recall as a ratchet.

Rows that fail today are in `known_failing.tsv`, each keyed by the card that
owns the fix. The test fails when a listed row starts passing, when a passing row
regresses, and when `ratchet.json` (counts) would move the wrong way. After an
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
* `fixtures`: the per-language fixture measurement, rewritten by the bless above.

Labels file shape:

```json
{
  "edges":  [{"language": "go", "caller": "<symbol id>", "callee": "<symbol id>", "correct": true}],
  "recall": [{"language": "go", "construct": "typed receiver", "caller": "<symbol id>", "name": "Get", "truth": "<symbol id>"}],
  "dead":   [{"language": "go", "id": "<file>::<symbol>", "truly_dead": false}]
}
```

For Rust, `construct` is `free_self` or `non_self`, the two classes the recall
floors (Rust 91 / 46) are set on. The gates: confident precision >= 0.95,
honest-miss >= 98%, 0.9-tier dead false positives <= 5%, no regression against the
previous row (precision -1 pt, recall -2 pt), and the recall floors in
`corpora.json`. A gate that held in the previous row must keep holding; one that
did not may only stay put, so the first record is a baseline and each language
card moves it. Run the runner before any DevMap release and after a change to
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

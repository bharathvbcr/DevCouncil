# DevMap accuracy fixtures

Six labelled fixtures from the 2026-10-09 language audits (Rust, Go, TypeScript,
Python, Swift, Kotlin). Each fixture's `expected.tsv` / `EXPECTED.tsv` was
written **before** DevMap ran on it; that ordering is what makes it truth rather
than a snapshot. Source only — no `.git`, `node_modules`, `target` or `.build`.

`dead.tsv` in each fixture is the dead-code truth that
`devmap-cli/tests/dead_tier_eligibility.rs` reads. It binds
`devmap_analyze::CONFIDENT_TIER_ELIGIBLE` to measurement: a language is listed
exactly when its fixture rows that reach the confident branch are all labelled
`truly_dead` and every examined `truly_dead` marker of that language reaches it.
Rows added for the dead-policy card (Go `W1`–`W5`, Rust `C50`/`C51`, TypeScript
`V19`, Kotlin `K30`/`K31`, Python `replay.py`) are marked as such where they are
declared.

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

The eligibility test binds each listed language to its corpus 0.9-tier
false-positive rate through `corpora[].languages.<lang>.dead_fp_by_tier`. No
corpus row is labelled yet, so that binding currently rests on the fixtures alone.

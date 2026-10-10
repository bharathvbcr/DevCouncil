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

Every row `dead` reports on a fixture must be labelled in its `dead.tsv`; the
test fails on an unlabelled one, because an unlabelled row is a row nobody
scored. Labels added on 2026-10-10 for rows the first pass left unlabelled
(Kotlin/Java, TypeScript/TSX) were read off the fixture source — the `why`
column names the call site or says nothing references it — not copied from
DevMap's answer.

`dead_unconfirmed_record.tsv` is the recorded 0.4-tier false-positive rate per
language (rows labelled `live` or `exempt` among the 0.4 rows). It has no gate,
since 0.4 is "unconfirmed" by contract, but the test binds it to the fixtures so
it moves only in the same change as the behaviour that moved it. On 2026-10-10:
Go 2/4, Java 2/3, Kotlin 6/13, Python 5/7, Rust 4/4, Swift 4/4, TSX 2/8,
TypeScript 4/23. These are probe-dense audit fixtures, many of whose `live` rows
are probes built to be reached only through shapes the resolver cannot bind —
the namesake veto keeping them out of 0.9 is the intended outcome — so the rates
describe the fixtures, not a corpus. The < 50% target per language is not met by
Go, Java, Python, Rust or Swift on these fixtures.

The per-edge gates (precision, honest-miss, recall ratchet) and the corpus
`record.json` belong to the accuracy-harness card (S1) and are not here yet;
when `record.json` exists, the eligibility test also binds each listed language
to its corpus 0.9-tier false-positive rate. The shape it reads — anything else
in the file is ignored, and a listed corpus/language missing these fields
fails the test rather than passing it:

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
every corpus that measured it.

Until then, **listing rests on these fixtures alone**, and some languages rest
on very few confident rows (see `CONFIDENT_TIER_ELIGIBLE`'s documentation).

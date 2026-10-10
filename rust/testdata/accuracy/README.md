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

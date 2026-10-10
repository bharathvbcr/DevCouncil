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
`V19`, Kotlin `K30`/`K31`, Python `replay.py`; second pass: Rust `C52`–`C54`,
Kotlin `K32`/`K33`, Swift `P5`/`P6`, TypeScript `V20`, Python `handlers.py`)
are marked as such where they are declared.

Every row `dead` reports on a fixture must be labelled in its `dead.tsv`; the
test fails on an unlabelled one, because an unlabelled row is a row nobody
scored. Labels added on 2026-10-10 for rows the first pass left unlabelled
(Kotlin/Java, TypeScript/TSX) were read off the fixture source — the `why`
column names the call site or says nothing references it — not copied from
DevMap's answer.

Second-pass rows (2026-10-10) cover what the corpus record found: Kotlin
`K32`/`K33`, Rust `C52`–`C54`, Swift `P5`/`P6`, TypeScript `V20`, Python
`handlers.py`.

`dead_unconfirmed_record.tsv` is the recorded 0.4-tier false-positive rate per
language (rows labelled `live` or `exempt` among the 0.4 rows). It has no gate,
since 0.4 is "unconfirmed" by contract, but the test binds it to the fixtures so
it moves only in the same change as the behaviour that moved it. On 2026-10-10:
Go 2/4, Java 2/3, Kotlin 8/17, Python 6/30, Rust 4/7, Swift 4/9, TSX 2/8,
TypeScript 7/27. Python's 0.4 rows include its 22 fixture rows capped
`confident_tier_unproven` since it came off the list. These are probe-dense
audit fixtures, many of whose `live` rows are probes built to be reached only
through shapes the resolver cannot bind — the namesake veto keeping them out of
0.9 is the intended outcome — so the rates describe the fixtures, not a corpus.
The < 50% target per language is not met by Go, Java or Rust on these fixtures
(the target is for after each language card lands).

`record.json` is the corpus record (2026-10-10): 21 local repositories, each a
`git archive` of the commit it names, built with the binary `built_from` names.
Every dead row whose evidence reached the confident branch — published at 0.9,
coverage-capped below it, or capped `confident_tier_unproven` — is labelled
from source (`rg -uu -w` and reading) with its evidence: `dead_true`,
`dead_false` with the `path:line` that reaches it, or `unsure` (listed, kept out
of `rows`). Labels were made by one pass per language and spot-checked; three
were corrected and say so in their evidence. The eligibility test binds each
listed language to it: at most 5% on every corpus. Measured: Go 0/17, Kotlin
0/9, Rust 0/3, TypeScript 0/29, TSX 0/14, Python 17/58 (off the list), Swift
none. The coverage-capped rows count because their evidence is the confident
branch's; the fixtures have no coverage loss, so the two definitions agree
there.

The per-edge gates (precision, honest-miss, recall ratchet) belong to the
accuracy-harness card (S1). The shape the eligibility test reads — anything else
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

Some listings still rest on few rows — Rust has 3 corpus rows, Swift none (see
`CONFIDENT_TIER_ELIGIBLE`'s documentation). The record is re-measured by hand:
re-label any new confident-branch row when a change moves the corpora.

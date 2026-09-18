# Ranked search

`dcgrep` answers two different questions, and they need two different engines.

**"Which lines contain this?"** is `dcgrep search`: ripgrep's own matcher,
linked as a library, with a trigram index in front of it to skip files that
cannot match. It is exact, it is fast, and a caller can act on its answer
without reading anything else.

**"Which files are about this?"** is `dcgrep rank`. A reader asking about
"json response parsing" is not asking for a substring. Every file in the
repository is a better or worse answer rather than a match or a miss, so this
returns files in rank order and the caller decides how far down to read.

Both are served from one published index slot, built by one command, under one
lock. There is no way to hold a fresh trigram index and a stale ranked one.

```bash
dcgrep index <<< '{"root":"."}'
dcgrep rank  <<< '{"query":"json response parsing","root":".","max_results":10}'
```

## Two ways to weigh a file

A ranked index stores the *document* half of a score. The *query* half is
applied at search time. That split is what lets one format carry two very
different rankers, and which one a given index uses is written into the file:

| Vocabulary | Weights from | Needs | Query side |
|---|---|---|---|
| `code-v1` | BM25, computed during the build | nothing | IDF, computed from the index |
| `wordpiece-30522` | a learned sparse encoder, run offline | one Python run, once | the model's own token weights, stored in the index |

`code-v1` is the default and needs no setup at all. It tokenises identifiers
the way code is actually written — `parseJSONResponse` yields `parsejsonresponse`,
`parse`, `json` and `response` — and weighs terms with BM25. For most
repositories this is the right answer and the end of the story.

`wordpiece-30522` exists for the case where it is not: a question phrased in
words rather than identifiers, or one whose words do not appear in the code
that answers it. Over this crate it puts `src/lexical.rs` and
`src/lexical/store.rs` at the top for *"how are files ranked by relevance"* —
neither file contains the word "relevance".

It is not strictly better — and on this repository, measured, it is worse.

The same model shreds `parseJSONResponse` into `par ##se ##js ##on ##res ##pon
##se`, where `code-v1` recovers `parse`, `json` and `response`.

### What the two tiers actually score

Queries are the doc comment of a source file; the answer is the file it came
from, with that comment removed from the corpus first — without the removal the
query appears verbatim in the answer and the test measures string matching
rather than retrieval. 250 queries over this repository's Rust, Go and Python:

| tier | parameters | R@1 | R@5 | R@10 | MRR |
|---|---|---|---|---|---|
| `code-v1` (BM25, no model) | — | **0.272** | **0.636** | **0.760** | **0.427** |
| `wordpiece-30522`, doc-v2-mini | 23M | 0.232 | 0.460 | 0.536 | 0.326 |
| `wordpiece-30522`, doc-v2-distill | 67M | 0.232 | 0.416 | 0.516 | 0.308 |
| `wordpiece-30522`, doc-v3-distill | 133M | 0.244 | 0.476 | 0.544 | 0.338 |

BM25 wins on every measure. A model six times larger does not close the gap —
v3-distill recovers 0.012 MRR over the 23M model and is still 0.089 behind the
tier that needs no model at all. Whatever the learned tier is losing on source
code is not a shortage of parameters.

Two things that result does *not* say. The queries are doc comments written by
the people who wrote the code, so they share its vocabulary — which is the
condition BM25 is best under and the one a stranger asking about an unfamiliar
repository is least likely to be in. And an aggregate is not a case: the
vocabulary-mismatch query above still resolves the way it is described, with
`src/lexical.rs` and `src/lexical/store.rs` first and "relevance" in neither.

So `code-v1` is the default because it measures better here, and the learned
tier is worth its setup when queries are phrased in words the code does not
use. Run the numbers on your own repository rather than inheriting these.

### Why a query never runs a model

The models this supports are the **doc-side** sparse encoders. They are trained
so the network is only needed to encode documents; a query is scored by looking
each token up in a weight table. That table ships inside `lexical.bin`, so:

- `dcgrep` stays one static binary. No Python, no PyTorch, no `CGO_ENABLED=1`,
  no model file to locate at runtime, nothing to warm up.
- A ranked query costs the same as it does under BM25.
- Nothing about search behaviour depends on a machine having a model.

The two halves of a learned ranking — document weights and query weights — are
written into **one file, under one integrity stamp**. They cannot be replaced
independently. An index that took its document weights from one model and its
query weights from another would rank confidently and mean nothing, and every
score would still look plausible.

## Turning on the learned tier

### 1. Pick a model

| Model | Params | Licence |
|---|---|---|
| `opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini` | 23M | Apache-2.0 |
| `opensearch-project/opensearch-neural-sparse-encoding-doc-v2-distill` | 67M | Apache-2.0 |
| `opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill` | 133M | Apache-2.0 |

All three emit the same 30522-token BERT WordPiece vocabulary, so `dcgrep`
reads any of them without a change. `doc-v2-mini` is about 23 MB on disk and
runs on a laptop CPU; start there and move up only if the ranking is not good
enough.

**SPLADE.** `naver/splade-*` models emit the same vocabulary and work with
`--model`, but their weights are **CC BY-NC-SA 4.0 — non-commercial**.
DevCouncil is Apache-2.0 and ships no weights either way, so this is your
licensing decision rather than the project's; the producer script prints a note
and continues. Note also that most SPLADE checkpoints are not doc-side: a
query-side SPLADE would need inference per query, which this design has nowhere
to put. Use a `-doc-` checkpoint.

### 2. Encode the repository

```bash
pip install torch transformers      # not a DevCouncil dependency
python3 scripts/encode-sparse.py --root . --out /tmp/sparse.jsonl
```

This is the only step that loads a model, and it is the only step that needs
Python. Once it has written its output you can uninstall both packages.

`--device mps` on Apple silicon or `--device cuda` on an NVIDIA machine will be
considerably faster than the default CPU path.

The script asks `dcgrep files` which files to encode rather than walking the
tree itself, so the encoding covers exactly what the index will admit — same
ignore rules, same `.gitignore`, same size limits. Its own walk does not read
`.gitignore`, and over this repository that difference was 498 documents
encoded for paths the index then declined, a third of the run. `--walk` forces
the fallback, and `--dcgrep PATH` points at a binary that is not on `PATH`.

### 3. Build the index from it

```bash
dcgrep index <<< '{"root":".","sparse":"/tmp/sparse.jsonl"}'
```

Over this repository, on an M-series laptop:

```json
{
  "ok": true,
  "files_seen": 1047,
  "files_indexed": 1044,
  "traversal_complete": false,
  "limit_reason": "postings",
  "lexical_files": 987,
  "lexical_postings": 334076,
  "lexical_unindexed": 58,
  "lexical_limit_reason": null,
  "lexical_vocabulary": "wordpiece-30522",
  "lexical_model": "opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini",
  "lexical_unmatched": 491
}
```

**Read three fields together, in this order.**

`limit_reason` first. `"postings"` here means the build stopped at the trigram
index's ceiling after 1047 files — so the ranked index covers only what the
build reached, and every count below is about that prefix rather than the
repository. `traversal_complete: false` says the same thing. This repository
holds 5388 files the walk would admit, so the ranked index describes about a
fifth of it; that ceiling, not the ranked side, is what binds here.
`lexical_limit_reason: null` is the proof — the ranked builder never reached a
limit of its own.

`lexical_unmatched` counts documents the encoding described that the build did
not place. It has two quite different causes and this field alone does not
distinguish them: a **stale encoding** — files deleted, newly ignored, or never
in this repository — or a **build that stopped early**, which is what the 491
above is. The producer listed 1478 files and the build reached 987 of them
before its ceiling; the arithmetic closes. Check `limit_reason` before reading
this number as staleness.

`lexical_unindexed` is the other direction: files the build read that the
encoding did not describe — here, 58 files whose extensions the producer does
not encode. They remain fully searchable by `dcgrep search`; only the ranking
leaves them out.

Queries against the resulting index return in **6.7–8.2 ms** (minimum to p90,
40 interleaved repeats per cell on an M-series laptop). `code-v1` over the same
repository returns in **5.3–6.7 ms**. About 2.5 ms of each is process start:
`dcgrep health`, which reads nothing, measures 2.4–3.6 ms in the same run, and
subtracting it leaves roughly 3.1 ms of work for `code-v1` and 4.4 ms for the
learned tier — the difference being a 3.45 MB index against a 2.57 MB one.

That ratio is the thing to plan with, because the dominant cost is reading and
re-validating the whole index, which is linear in its size: measured over a
sweep from 0.04 MB to 10.9 MB, about **0.5 ms per megabyte**, with a 10.9 MB
index still answering in 9.2 ms. Re-validation on every query is deliberate —
the file is input, and the process that wrote it is not the one reading it.

### 4. Going back

```bash
dcgrep index <<< '{"root":"."}'
```

A build with no `sparse` publishes a `code-v1` index and the repository is back
on BM25. There is no half-state: one slot, one vocabulary, named in every
`rank` response.

## The parity gate

The most dangerous failure in this design is not a crash. It is a Rust
tokenizer that splits a query slightly differently from the Python tokenizer
that split the documents. The result is a ranking over token ids nothing was
indexed under — an empty or near-empty answer, with no error, that a caller
reads as "the repository does not contain this".

So the producer records what its tokenizer did:

```json
"parity": [{"text": "parseJSONResponse",
            "ids": [11968, 3366, 22578, 2239, 6072, 26029, 3366]}, ...]
```

Those seven ids are `par ##se ##js ##on ##res ##pon ##se`, which is worth
noticing: the model does **not** split identifiers the way `code-v1` does. BERT
WordPiece was trained on English prose, so it shreds camelCase into subword
fragments rather than recovering `parse`, `json` and `response`. That is the
real trade between the two vocabularies, and it is most of why `code-v1` scores
better in the table above: a repository's own queries are full of its own
identifiers, and the learned tier cannot see them whole.

`dcgrep index` replays every sample through its own WordPiece before it writes
a byte, and **one disagreement refuses the whole build**:

```
this build's query tokeniser disagrees with the model's on "parseJson":
the model produced [1, 6] and this produced [0]. The index was not
published, because a query tokenised differently from the documents
returns a ranking over ids nothing was indexed under
```

The samples are checked twice: once against the encoder's own header, which
fails in milliseconds before the repository is walked, and once against the
bytes about to be published, which proves the vocabulary survived
serialisation. An encoding with no samples is refused outright — there would be
no way to show the two sides agree.

If you hit a parity failure with a real model, that is the gate working. It
means this crate's WordPiece needs the case the model exercised, not that the
encoding is bad.

### What has been checked

The Rust tokenizer is judged against the model's own, on every `cargo test`,
with no PyTorch and no download:

```
test lexical::wordpiece::tests::the_tokeniser_reproduces_the_model_s_own ... ok
```

`rust/dc-grep/tests/fixtures/wordpiece-*` hold the real HuggingFace tokenizer's
full 30522-token vocabulary and its answers for 14,810 inputs: every codepoint
across 30 Unicode ranges both alone and inside a word, the parity block, and a
sample of lines from this repository. Re-record them with:

```bash
python3 scripts/encode-sparse.py --record --root .
```

### The one divergence that is known and kept

This build's Unicode tables are **newer** than the ones the model's tokenizer
was built against. On 98 codepoints — Arabic Extended-A/B marks, Indic marks,
Supplemental Punctuation, all assigned in Unicode 10 through 14 — `dcgrep`
strips a mark or splits a word where the reference sees an unassigned character
and keeps it, emitting `[UNK]` for the whole word.

The direction is what makes this tolerable: the reference is the *less*
specific side, so the effect is a **miss, never a wrong file**. A query holding
one of these produces real tokens, while the document that should match it was
indexed under `[UNK]`. It is confined to the learned tier; `code-v1` does not
use WordPiece at all.

It is written down rather than fixed because fixing it means pinning a copy of
another project's Unicode version and tracking it, to change behaviour on 98
marks and medieval punctuation marks. `TABLE_SKEW` in the test names all 98 and
asserts the set in both directions: nothing outside it may diverge, and
everything inside it still must — so a crate update that closes one fails the
test rather than being quietly absorbed into an allowlist.

To re-check the boundary against a corpus larger than the committed one, point
the test at a sweep:

```bash
DCGREP_WORDPIECE_CONFORMANCE=/path/to/sweep.jsonl cargo test -p dc-grep the_tokeniser
```

**This is worth reading before you trust a tokenizer you wrote yourself.** The
first version of ours passed 12 hand-written unit tests and a 4,587-case
curated corpus at 99.7%. Measured against the codepoint sweep it disagreed with
the model on **1,754 of 7,904 cases — 22%**: over 95% of Latin Extended
Additional, 100% of combining marks, 21% of Latin-1. The cause was that accent
handling was a hand-written transliteration table (`ł`→`l`, `ß`→`s`) where the
reference performs canonical decomposition, which keeps both. The curated
corpus had missed it by a factor of eighty, because the same person chose the
examples and wrote the algorithm.

It is now exact on every range in the sweep, with one known exception: U+061D
was assigned in Unicode 14 and the linked property tables still call it
unassigned, so it is not split off as punctuation. The conformance test names
that single case rather than filtering it out of the fixture.

The producer also checks itself end to end against the real binary, with the
model faked:

```bash
python3 scripts/encode-sparse.py --self-test --dcgrep ./rust/target/release/dcgrep
```

## What the format guarantees

`lexical.bin` is validated in full every time it is opened, not trusted:

- magic, schema and vocabulary code, so an index from another build is refused
  by name rather than misread
- every section's extent, so a truncated file cannot be read as a short one
- a strictly ascending term table, *proven* rather than assumed — an unsorted
  table makes the binary search miss terms that are present, and the symptom is
  an empty result that looks like a repository with no match
- posting file ids in range, finite weights, no duplicate vocabulary token
- `code-v1` must carry no stored query side and a model vocabulary must carry
  one; an index with half a ranking is refused rather than scored with whatever
  the other half implies

A path named in an encoding is never enough to put a file in the index. The
walk decides what exists; the encoding only supplies weights for what the walk
already found. An encoding naming `../../../etc/passwd` contributes nothing but
a number in `lexical_unmatched`.

## Reading a ranking

```json
{
  "ok": true,
  "count": 3,
  "vocabulary": "wordpiece-30522",
  "files": [
    {"path": "src/json_parser.rs", "score": 8.14, "terms_present": 3},
    {"path": "docs/parsing.md", "score": 4.02, "terms_present": 2, "stale": true}
  ],
  "terms_unknown": 0,
  "terms_total": 3,
  "files_indexed": 1140,
  "index_postings": 482301
}
```

- **`score`** is comparable within one response and meaningless across two. It
  depends on the corpus, so 8.1 here and 8.1 from another repository say
  nothing about each other. It is deliberately not normalised to 0..1, which
  would imply exactly the comparison that does not hold.
- **`stale`** means the indexed copy is no longer what is on disk. The file is
  still returned — it was about the query when it was read — but quoting it
  without reading it first is quoting history.
- **`terms_unknown` equal to `terms_total`** is why a result set is empty: not
  "nothing is about this" but "the index has never seen any of these words".
- **`vocabulary`** tells a caller whether it is reading a BM25 ranking or a
  learned one, without asking a second question.

A repository with no index is an **error**, never an empty list:

```
no ranked index for this repository (...); build one with `dcgrep index`
```

A repository that has never been indexed and a repository where nothing is
about the query are different facts, and a caller told the second when it means
the first will act on it.

A repository whose index is **being rebuilt** is a third fact, and a different
error:

```
a build is publishing this repository's index; retry the query in a moment
```

It does not say "no ranked index" and it does not advise a build, because there
is one and a build is what is holding it. Starting a second build is the one
response that makes the situation worse. A reader waits briefly before
reporting this — long enough to cover the slot flip, far too short to outlast a
real build — which in a stress of `rank` against repeated republishes took
refusals from 29% of reads to none.

## Capabilities

```bash
dcgrep health
```

```json
{
  "ok": true,
  "engine": "ripgrep",
  "index_engine": "tgrep-core",
  "ranked_engines": ["bm25", "learned-sparse"],
  "ranked_vocabularies": ["code-v1", "wordpiece-30522"],
  "limits": {"max_results": 5000, "max_list_results": 200000}
}
```

These are properties of the binary, not of any repository — `health` has no
root to ask. What one repository's index actually speaks is on every `rank`
response.

`limits` exists because callers on the other side of the process boundary keep
their own copies of these numbers, and a copy that drifts low is invisible: the
caller asks for everything, receives a prefix under a flag it is not obliged to
read, and believes it has the whole tree. Reporting them lets the two sides be
asserted equal instead of assumed equal.

The two ceilings are deliberately different. `max_results` bounds a *sample* —
5,000 match lines is already more than anything downstream reads.
`max_list_results` bounds an *enumeration*, and for "which files are in this
repository" 5,000 is not a large number; this repository passed it at 5,408
files. They were one constant until that happened.

## Limits

Every ceiling is reported when it bites, never applied silently.

| Limit | Value | Reported as |
|---|---|---|
| Postings in one ranked index | 4,000,000 | `lexical_limit_reason: "lexical_postings"` |
| Postings read from one encoding | 32,000,000 | build refused |
| Terms from one document | 20,000 | `lexical_unindexed` |
| Vocabulary tokens | 100,000 | build refused |
| `lexical.bin` on disk | 64 MiB | index refused at open |
| Query length | 1,024 bytes | request refused |
| Distinct query terms | 64 | `terms_dropped` |
| Ranked files returned | 1,000 | `truncated`, with `limit` |

A document that hit the per-document token ceiling is counted in
`lexical_unindexed` rather than indexed short: its length would not be its
length, and ranking it against documents measured whole would put it wherever
the ceiling happened to fall.

The encoding's posting budget is there because the two per-item bounds above it
— 200,000 documents and 20,000 terms each — do not bound their product. Four
billion postings would be parsed and held before the index kept the first four
million and dropped the rest; measured at about 7 bytes per posting held, that
is roughly 29 GB of memory for an index that can use a thousandth of it. Per-item
bounds are not a bound on the fan-out. The budget is eight times what the store
can hold, so a stale encoding whose documents the walk mostly rejects still
reads; this repository's own encoding is 334,000 postings, two orders of
magnitude inside it.

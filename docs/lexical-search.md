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
| `wordpiece-30522` | a learned sparse encoder, run offline | one Rust run, on Apple silicon, once | the model's own token weights, stored in the index |

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
licensing decision rather than the project's; the encoder prints a note
and continues. Note also that most SPLADE checkpoints are not doc-side: a
query-side SPLADE would need inference per query, which this design has nowhere
to put. Use a `-doc-` checkpoint.

### 2. Encode the repository

Encoding runs in `dc-sparse-encode`, offline, on Apple silicon. The forward is
tessl's Metal kernels. `dcgrep` does not link that crate, and a query still
does not load a model. Linux and Windows can search an index that was encoded
on a Mac; they cannot run the encoder.

```bash
cargo run --release --manifest-path rust/dc-sparse-encode/Cargo.toml -- --root . --out /tmp/sparse.jsonl
```

The crate is not a member of the `rust/` workspace, so `cargo test --workspace`
does not try to build tessl. It depends on a local tessl checkout at
`../../../../research/tessl` relative to the crate — `~/Code/research/tessl`
when this repository lives under `~/Code/devtools`. The BERT kernels
(LayerNorm, erf-GELU, head dimension 32, the masked max) are what that
checkout builds.

The default model is
`opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini`
(Apache-2.0, 23M parameters). The doc-side family shares one 30522-token
WordPiece vocabulary, so a larger checkpoint needs no change in `dcgrep`:

| Model | Parameters | |
|---|---|---|
| doc-v2-mini | 23M | fastest, smallest; the default |
| doc-v2-distill | 67M | the usual default elsewhere |
| doc-v3-distill | 133M | strongest of the Apache-2.0 doc-side family |

There is no `--device`. The forward is Metal.

The encoder asks `dcgrep`, in process, which files the index would admit, so
the encoding covers the same ignore rules, the same `.gitignore` and the same
size limits. Its own walk does not read `.gitignore`, and over this repository
that difference was 498 documents encoded for paths the index then declined, a
third of the run. `--walk` forces that fallback.

### 3. Build the index from it

```bash
dcgrep index <<< '{"root":".","sparse":"/tmp/sparse.jsonl"}'
```

Over this repository, from a build made before the two ceilings below were
separated (on an M-series laptop). It is kept because it is the shape to
recognise in an old report:

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

There, `limit_reason: "postings"` was the trigram index's ceiling, and it also
ended the walk, so the ranked index stopped at the same file without ever
reaching a ceiling of its own — `lexical_limit_reason: null` while a fifth of the
repository was unranked. A ranking over it was a ranking over a prefix, and
nothing in the answer said so.

**The two indexes now have separate ceilings, and each is reported by its own
field.** Read these in this order:

1. **`lexical_limit_reason`** — why the *ranked* index is short of the tree, if
   it is. `null` means no ceiling stopped it. Otherwise it names one:
   one of the ranked index's own — `lexical_postings` (4,000,000 postings) or
   `lexical_bytes` (the 64 MiB file budget, which a corpus of mostly distinct
   terms reaches first) — or a walk ceiling that ended the build for both
   indexes: `files`, `duration` or `input_bytes`. The same value is stored in
   `lexical.bin` and returned by every `rank` answer as `lexical_limit_reason`
   beside `complete`.
2. **`files_seen` against `lexical_files`.** `files_seen` is every file the walk
   reached — it keeps counting after the trigram index is full — so this pair is
   how much of the repository the ranking covers. Files that are not text
   this engine ranks (binary, not UTF-8) are in `files_seen` and not in
   `lexical_files`, and are not a shortfall; files that are text and were left
   out are counted in `lexical_unindexed`, and are. That is why `complete` and
   not the arithmetic is the verdict.
3. **`limit_reason`** — the *trigram* index's ceiling only. `"postings"` here
   means the trigram index filled and `dcgrep search` reads the files past it
   with the matcher instead of skipping them by index; it costs acceleration and
   changes no result. It says nothing about the ranked index, which carries on
   past it (`lexical_files` can exceed `files_indexed`). `traversal_complete:
   false` is the same fact.

`lexical_unmatched` counts documents the encoding described that the build did
not place. It has two quite different causes and this field alone does not
distinguish them: a **stale encoding** — files deleted, newly ignored, or never
in this repository — or a **build that stopped early**, which is what the 491
above was. Check `lexical_limit_reason` before reading this number as
staleness: if it names a ceiling, the encoding is probably fine and the build
is what ran out.

`lexical_unindexed` is the other direction: files the walk reached that are
text this engine admits and that no document in the ranked index stands for.
It counts a file that could not be opened or read, a file over the 2 MiB read
bound whose first 8 KiB reads as text (a large binary asset is not counted),
a file that changed while it was being read, a file over the
20,000-term per-document cap, a file whose name is not UTF-8, and — with an
imported encoding — a file the encoding did not describe (in the example, 58
files whose extensions the producer does not encode). They remain fully
searchable by `dcgrep search`; only the ranking leaves them out, and any of
them makes `complete` false.

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
sample of lines from this repository. That fixture is frozen. It was recorded
from the model's own tokenizer. Regenerating it from this WordPiece would
compare the tokeniser to itself, and the 98 `TABLE_SKEW` codepoints would
disappear into a pass. `dc-sparse-encode --record` prints that and does not
rewrite the file.

The neural reference — per-layer hidden states and pooled vectors, for a short
document, exactly 512 tokens, a document past 512, and a mixed batch — is
`rust/dc-sparse-encode/tests/fixtures/`. Regenerate it from the local cache,
without downloading:

```bash
uv venv /tmp/oracle && uv pip install --python /tmp/oracle torch transformers numpy
/tmp/oracle/bin/python -I rust/dc-sparse-encode/tests/fixtures/record.py
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

A larger sweep has to be recorded from the model's tokenizer, not from this
WordPiece, and pointed at with `DCGREP_WORDPIECE_CONFORMANCE`. The encoder has
no `--fixtures` flag. The committed file is the corpus the test reads unless
that variable names another one:

```bash
DCGREP_WORDPIECE_CONFORMANCE=/tmp/sweep/wordpiece-conformance.jsonl cargo test -p dc-grep the_tokeniser
```

With the override set, the test still requires that nothing outside
`TABLE_SKEW` diverges. It skips the check that every `TABLE_SKEW` character
still diverges, because a sweep need not contain them all.

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

The encoder checks the JSONL path against `dcgrep`'s own `build_index` and
`ranked_search`, with the model faked. Those are the functions the `dcgrep`
binary calls:

```bash
cargo run --manifest-path rust/dc-sparse-encode/Cargo.toml -- --self-test
```

`--bench` times tokenisation, the forward and the pool against the torch
oracle, interleaved, and reports the minimum of N (at least 5). Torch stays
on CPU. On 2026-10-08 a release build, N=5, the four fixture documents,
doc-v2-mini: rust minimum 0.041532s, torch minimum 0.273876s.

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
  "files_seen": 1152,
  "complete": true,
  "lexical_limit_reason": null,
  "walk_errors": 0,
  "lexical_unindexed": 0,
  "index_postings": 482301
}
```

**Read `complete` before anything else.** It is true only when no ceiling
stopped the build that wrote the index, the walk could read everything it was
shown (`walk_errors` is zero), and no admissible file was left out
(`lexical_unindexed` is zero). `false` means the ranking is over part of the
repository, and a file that is missing from it may be missing because it was
never ranked, not because nothing in it is about the query.
`lexical_limit_reason` names the ceiling when one stopped the build
(`lexical_postings`, `lexical_bytes`, `files`, `duration` or `input_bytes`); it
is null when the shortfall is `walk_errors` or `lexical_unindexed`.
`files_indexed` against `files_seen` says how much was reached. The field is
always present; a reply without it comes from a build that predates it, and a
client must read that as not known to be complete — the Go client does.

`complete` is relative to the text this engine admits: a file with a NUL byte
or that is not valid UTF-8 is not rankable, is in `files_seen` and not in
`files_indexed`, and does not make a ranking incomplete. (A UTF-8 byte-order
mark is refused by the same rule today although it is text; that is a gap in
the rule, tracked separately, and `complete` does not count it.)

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

Every ceiling is reported when it bites, never applied silently. A ceiling on
the build is reported twice: by `dcgrep index` when it runs, and by every
`rank` answer afterwards (`complete`, `lexical_limit_reason`), because the
ranking is read long after the build that bounded it.

The trigram index and the ranked index have **separate** ceilings, and one does
not end the other. The trigram index filling stops only the trigram index; the
walk goes on and the ranked index with it. Only the walk's own ceilings —
files, bytes read, time — end both.

| Limit | Value | Reported as |
|---|---|---|
| Postings in one trigram index | 4,194,304 (derived, below) | `limit_reason: "postings"`, `traversal_complete: false`; `search` still reads the files past it |
| Files in one index build | 50,000, or the request's `max_files` | `limit_reason` and `lexical_limit_reason: "files"` |
| Bytes read by one index build | 128 MiB | `limit_reason` and `lexical_limit_reason: "input_bytes"` |
| Wall time of one index build | 30 s | `limit_reason` and `lexical_limit_reason: "duration"` |
| One file read by the index | 2 MiB | counted in `files_unindexed`; if its first 8 KiB reads as text it is also counted in `lexical_unindexed`, which makes `complete` false (a binary asset is not); the matcher still reads it |
| Postings in one ranked index | 4,000,000 | `lexical_limit_reason: "lexical_postings"` |
| Bytes in `lexical.bin` | 64 MiB, counted exactly while building | `lexical_limit_reason: "lexical_bytes"`; the document that would cross it is left out whole |
| Postings read from one encoding | 32,000,000 | build refused |
| Terms from one document | 20,000 | `lexical_unindexed`, which makes `complete` false |
| Vocabulary tokens | 100,000 | build refused |
| `lexical.bin` on disk | 64 MiB | index refused at open |
| Query length | 1,024 bytes | request refused |
| Distinct query terms | 64 | `terms_dropped` |
| Ranked files returned | 1,000 | `truncated`, with `limit` |

A document that hit the per-document token ceiling is counted in
`lexical_unindexed` rather than indexed short: its length would not be its
length, and ranking it against documents measured whole would put it wherever
the ceiling happened to fall.

The trigram ceiling is derived, not chosen. Every cache file is refused at open
above 64 MiB, and the build's own integrity pass fails on one first. `index.bin`
takes 6 bytes per posting and `lookup.bin` 16 bytes per distinct trigram, and a
posting list of one entry makes the distinct trigrams as many as the postings,
so the most this can hold with no input able to produce a file the reader then
refuses is 64 MiB / 16 B = 4,194,304. (`index.bin` alone would allow
11,184,810, and a corpus of mostly distinct trigrams would then fail the build
outright.) It was 2,000,000, picked by eye, and it ended the walk at 1,139 of
this repository's files. Both entry sizes are pinned in tests against what
tgrep writes. A walk past the ceiling still counts `files_seen` without reading
the files once neither index has room.

The encoding's posting budget is there because the two per-item bounds above it
— 200,000 documents and 20,000 terms each — do not bound their product. Four
billion postings would be parsed and held before the index kept the first four
million and dropped the rest; measured at about 7 bytes per posting held, that
is roughly 29 GB of memory for an index that can use a thousandth of it. Per-item
bounds are not a bound on the fan-out. The budget is eight times what the store
can hold, so a stale encoding whose documents the walk mostly rejects still
reads; this repository's own encoding is 334,000 postings, two orders of
magnitude inside it.

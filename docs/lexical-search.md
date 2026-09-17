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

`wordpiece-30522` exists for the case where it is not: a large repository, a
team searching in prose rather than identifiers, or a question whose words do
not appear in the code that answers it.

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

### 3. Build the index from it

```bash
dcgrep index <<< '{"root":".","sparse":"/tmp/sparse.jsonl"}'
```

```json
{
  "ok": true,
  "files_indexed": 1163,
  "lexical_files": 1140,
  "lexical_vocabulary": "wordpiece-30522",
  "lexical_model": "opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini",
  "lexical_unmatched": 4
}
```

Read `lexical_unmatched`: it counts documents the encoding described that the
walk never reached — deleted since the encoder ran, newly ignored, or never in
this repository at all. They are dropped, never indexed. A large number means
the encoding is stale, which is otherwise invisible, because a ranking over two
thirds of a repository looks exactly like a ranking over all of it.

`lexical_unindexed` is the other direction: files the walk read that the
encoding did not describe. They remain fully searchable by `dcgrep search`;
only the ranking leaves them out.

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
"parity": [{"text": "parseJSONResponse", "ids": [11968, 15723, 3433]}, ...]
```

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

### What has and has not been checked

The producer checks itself against the real binary, with no model and no
PyTorch:

```bash
python3 scripts/encode-sparse.py --self-test --dcgrep ./rust/target/release/dcgrep
```

It fakes the *model* — a hand-written vocabulary and a table of token ids — and
hands the resulting encoding to `dcgrep index` for real. That covers vocabulary
ordering, the walk, the file format, the end-to-end ranking and the refusal
path. It does **not** cover the one thing only a download can: whether this
crate's Rust WordPiece reproduces HuggingFace's tokenizer on the full 30522-token
vocabulary. That comparison happens the first time you run a real model, and
the parity gate is what makes its outcome a build failure rather than a silently
wrong ranking. Accented Latin beyond Latin-1 Supplement and Latin Extended-A,
and scripts outside the CJK ranges the tokenizer knows, are the likeliest places
to need work.

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
  "ranked_vocabularies": ["code-v1", "wordpiece-30522"]
}
```

These are properties of the binary, not of any repository — `health` has no
root to ask. What one repository's index actually speaks is on every `rank`
response.

## Limits

Every ceiling is reported when it bites, never applied silently.

| Limit | Value | Reported as |
|---|---|---|
| Postings in one ranked index | 4,000,000 | `lexical_limit_reason: "lexical_postings"` |
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

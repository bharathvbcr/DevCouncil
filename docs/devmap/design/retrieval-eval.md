# Retrieval evaluation: does dense embedding beat BM25 and learned-sparse on this repository?

Status: **protocol, written before any arm was run on any query set.** The
results section at the bottom is filled in after the decision rule below was
committed. Git history is the proof of the order: this file's first commit
holds the rule and no numbers.

Lane `rank-eval`, base `dbd0193a`. This decides whether embeddings are built
into `dcgrep` / DevMap. Nothing here changes `dcgrep`'s query path and no model
ships.

## The question

On queries phrased the way a stranger asks, does dense embedding retrieval
(alone, or fused with BM25) beat `dcgrep`'s BM25 (`code-v1`) and its
learned-sparse tier (`wordpiece-30522`, doc-v2-mini) at ranking *files*?

## Arms

All arms rank files, over one corpus, and are scored by one scorer
(`rust/rank-eval`, `score`), so arms differ only in ranking.

| arm | ranking |
|---|---|
| A | `dcgrep rank`, `code-v1` (BM25, no model) |
| B | `dcgrep rank` over an index built from `dc-sparse-encode` output, doc-v2-mini |
| C | dense `sentence-transformers/all-MiniLM-L6-v2`: file chunks embedded, file score = max chunk cosine |
| D | reciprocal-rank fusion (k = 60) of A and C; A+B also reported |
| E | optional, only if cheap: `google/embeddinggemma-2` |

## Corpus

The corpus is **every file `dcgrep` admits** under the repository at
`dbd0193a` (extracted with `git archive`, so lane edits cannot leak in),
restricted to the encoder's source suffixes so B and C can rank the same set.

The shipped `dcgrep index` does **not** reach the whole repository: its trigram
index stops at a 2,000,000-posting ceiling, which here is 1234 of 2716 files in
walk order, 579 of them `rust/testdata` fixtures, and none of `backend/`,
`docs/`, `rust/devmap-cli`, `rust/devmap-resolve` or `rust/dc-grep`. A ranked
index that cannot see the code a query is about cannot be evaluated on it.

- **Primary condition W (whole repository).** `dcgrep` built from the same
  source with only `MAX_INDEX_POSTINGS` and `MAX_INDEX_DURATION` raised, in a
  scratch build that is not committed and not shipped. Same tokeniser, same
  BM25, same walk, complete traversal.
- **Sensitivity condition P (as shipped).** The stock binary, truncated index.
  Arm A is re-run on it. B and C scores are per-file independent (document
  weights and model query weights do not depend on corpus statistics), so their
  P rankings are their W rankings restricted to P's files. Queries whose answer
  is outside P are dropped from P and counted.

The verdict is decided on W. P is reported so a reader can see whether the
verdict depends on the choice.

## Query sets

1. **Commit-subject set** (automatic, reproducible; generator and JSONL
   committed). Non-merge commits on `main` up to `dbd0193a` whose conventional
   type is fix, feat, perf or refactor; 1-3 touched non-test source files that
   still exist at `dbd0193a`; query is the subject with `type(scope):` stripped;
   queries occurring verbatim in the corpus are dropped; duplicates removed.
   *Caveat:* commit subjects are written by the people who wrote the change,
   with the code's own vocabulary in mind. This favours BM25, for the same
   reason the doc-comment benchmark in `docs/lexical-search.md` does, though
   less strongly than doc comments do. Ground truth is "files the commit
   touched", which is a proxy: a commit may touch a file the subject does not
   describe.
2. **Stranger set** (hand-written, about 60). Questions written from the
   README and the `docs/` overviews only, in plain words, avoiding
   identifiers. The questions file is committed **before** any answer is
   labelled, and labels are made before any arm is run on the set. Ground truth
   is 1-3 files per question, found with DevMap and by reading source, one
   line of justification each. A question with no defensible answer is dropped
   rather than guessed. *Caveat:* the author knows the repository and has read
   the docs, so the phrasing is not that of a true stranger, and the author
   labels the answers, so a labelling bias toward files that the author found
   by search is possible (DevMap's own ranker is a lexical one). The set is
   small, so intervals are wide.

Neither set is the doc-comment protocol. That protocol's harness is not in the
repository, only its table. Reconstructing it is out of scope unless it falls
out for free; the results section says whether it did.

### Amendment before labelling (stranger questions committed)

The stranger questions were derived from the docs, so a doc page that answers a
question shares its words. Ground truth for the stranger set is the **source
files that implement the behaviour**, not the docs that describe it. Two views
are therefore scored, both pre-stated here:

- **S-all**: rankings over the whole corpus, docs included. A doc ranked above
  the implementing file counts against the arm.
- **S-nodocs** (the view the decision rule uses): `docs/`, `README.md` and other
  `.md` files are removed from every arm's ranked list before scoring. Without
  this, an arm is rewarded or punished for how closely a question echoes the doc
  it was written from, which is the opposite of the stranger case.

The commit-subject set is scored over the whole corpus.

## Metrics and statistics

- R@k = |relevant in top k| / |relevant|, for k in 1, 5, 10, averaged over
  queries. MRR = mean of 1/rank of the first relevant file (0 if none in the
  list).
- Per arm and set: n, R@1/5/10, MRR, 95% percentile bootstrap CI over queries
  (at least 1000 resamples, fixed seed).
- Paired comparison against arm A: per-query wins/losses/ties on reciprocal
  rank, and a paired bootstrap CI on the MRR difference (the same resampled
  query indices applied to both arms).
- Cost: encoding wall time and instructions retired (`/usr/bin/time -l`) and
  index size per arm; dense per-query embedding time, min of N, model load
  excluded.

## Pre-stated decision rule

Dense (arm C or arm D) is **worth building** only if **both** hold:

1. On the **stranger set (S-nodocs)**, its MRR beats BM25's with a paired 95% bootstrap CI
   on the MRR difference that **excludes zero**; and
2. on the **commit-subject set** it does **not** lose to BM25 by more than its
   CI, meaning the paired 95% CI on (dense - BM25) MRR does not lie entirely
   below zero.

Anything else is **not worth building now**. The rule is evaluated on
condition W. If P disagrees, that is reported, and does not change the verdict.

## Results

(Filled in after this protocol is committed.)

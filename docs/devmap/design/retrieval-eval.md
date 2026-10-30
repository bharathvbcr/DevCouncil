# Retrieval evaluation: does dense embedding beat BM25 and learned-sparse on this repository?

Status: **complete.** The protocol and decision rule were committed (`6ee434e9`,
`3144df72`) before any arm was run on any query set; the results section at the
bottom was written afterwards. Git history is the proof of the order: this
file's first commit holds the rule and no numbers.

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

Everything below was produced by `benchmarks/retrieval-eval/score.sh` from the
arm runs; the per-view tables, with R@k intervals in the JSON, are in
`benchmarks/retrieval-eval/results/`. 5000 bootstrap resamples, seed
20261029. Arm run files (ranked lists over about 2500 files per query) are not
committed because of their size; the commands that produce them are under
"Reproducing".

### What was actually run

- **Corpus W:** 2507 files (every file `dcgrep` admits that carries a source
  suffix). **Corpus P:** 1139 files, what the stock binary's index reaches.
  Arm A on W is `dcgrep` built from this tree with `MAX_INDEX_POSTINGS` raised
  from 2,000,000 to 60,000,000 and `MAX_INDEX_DURATION` from 30 s to 600 s
  (scratch build, reverted, not committed), so its trigram index completes
  (`traversal_complete: true`, 2678 files indexed, 2641 BM25-rankable). Nothing
  else differs from the shipped binary.
- **A:** `dcgrep rank`, `code-v1`, untuned. **B:** `dc-sparse-encode` doc-v2-mini
  then `dcgrep index` with `sparse` (only the first 510 tokens of a file are
  encoded; that is how the shipped tier works). **C:** `all-MiniLM-L6-v2` on
  tessl's Metal BERT forward, 254-token windows with a 192-token stride (62
  tokens of overlap) over the whole file, file score = max chunk cosine,
  mean pooling and L2 as in the model's own pooling modules, brute-force
  search, query embedded alone and truncated at 254 tokens. **D:** RRF (k = 60)
  of A and C. AB and ABC are RRF of A+B and A+B+C, reported for information;
  the rule names only C and D.
- **Dense parity (verified).** `rust/dc-sparse-encode/tests/dense_parity.rs`
  compares token ids exactly and embeddings against sentence-transformers
  output recorded in `tests/fixtures/minilm/fixture.json` for six texts
  (including a 256-token one, accents, and code): cosine 0.9999998 to 1.0000001,
  tokens identical. The reference was recorded with a torch and
  sentence-transformers installed into a scratch venv for this lane (see
  Caveats). Tessl's `BertSparseModel` cannot load MiniLM directly (it requires
  `bert.*` names and an MLM head), so `adapt_checkpoint` writes a scratch
  safetensors with renamed tensors and a synthetic head; the forward's trace is
  taken before the head, so the head does not affect the embedding.
  MiniLM's `vocab.txt` is byte-identical to doc-v2-mini's and both tokenizers
  declare the same normalizer, so `dc_grep::wordpiece_ids` is the tokenizer for
  C (confirmed by the exact-id parity above).
- **Commit-subject set:** 1637 non-merge commits reachable from `dbd0193a`;
  732 were not fix/feat/perf/refactor, 39 touched no code file, 307 touched
  more than three, 208 touched a file that no longer exists at `dbd0193a`, 7
  subjects occur verbatim (case-insensitively) in the corpus, 7 were duplicates;
  **337 queries remain**. A query also needs at least three words (none
  was dropped by that rule). Ground truth is the 1-3 code files the commit
  touched, tests, docs, fixtures, lockfiles and generated output set aside.
- **Stranger set:** 62 questions written from the README and `docs/` only,
  committed unlabelled (`3144df72`) before any label existed; **50 labelled**,
  12 dropped because no set of one to three files answers them defensibly. Labels were
  made with file names, `devmap` search and `ask`, `rg` and reading the heads
  of the candidate files, not by running any arm. A few labels rest on the file
  and function names and a header read rather than a full read of the source.
  Each label has a one-line justification in `stranger-labeled.jsonl`.

### The decision rule, applied (condition W)

| set / view | arm | n | MRR (95% CI) | paired MRR diff vs A (95% CI) | W/L/T |
|---|---|---|---|---|---|
| stranger, S-nodocs | A BM25 | 50 | 0.216 [0.142, 0.296] | | |
| stranger, S-nodocs | C dense | 50 | 0.285 [0.192, 0.384] | +0.070 [-0.014, +0.158] | 29/16/5 |
| stranger, S-nodocs | D A+C | 50 | 0.323 [0.230, 0.421] | **+0.108 [+0.049, +0.171]** | 35/7/8 |
| commit subject | A BM25 | 337 | 0.393 [0.354, 0.434] | | |
| commit subject | C dense | 337 | 0.433 [0.390, 0.476] | +0.039 [-0.011, +0.089] | 143/134/60 |
| commit subject | D A+C | 337 | 0.420 [0.379, 0.460] | +0.026 [-0.005, +0.056] | 167/87/83 |

- **C alone: not worth building by the rule.** Its stranger-set interval
  includes zero.
- **D (BM25 fused with dense): meets both conditions.** Stranger-set interval
  excludes zero (lower bound +0.049); on the commit-subject set its interval
  does not lie below zero.
- **Rule outcome: dense embeddings are worth building as a fusion partner for
  BM25, not as a replacement for it.** The next sections say how much weight
  that deserves.

### All arms, condition W

### W-commit: n = 337 queries (0 dropped: relevant file outside the corpus), 5000 resamples, seed 20261029

| arm | R@1 | R@5 | R@10 | MRR | MRR 95% CI | vs A: W/L/T | MRR diff | diff 95% CI |
|---|---|---|---|---|---|---|---|---|
| A | 0.180 | 0.454 | 0.559 | 0.393 | [0.354, 0.434] | - | - | - |
| B | 0.097 | 0.205 | 0.272 | 0.220 | [0.183, 0.258] | 84/222/31 | -0.173 | [-0.221, -0.127] |
| C | 0.228 | 0.448 | 0.536 | 0.433 | [0.390, 0.476] | 143/134/60 | +0.039 | [-0.011, +0.089] |
| D | 0.182 | 0.487 | 0.575 | 0.420 | [0.379, 0.460] | 167/87/83 | +0.026 | [-0.005, +0.056] |
| AB | 0.133 | 0.281 | 0.371 | 0.294 | [0.255, 0.335] | 109/166/62 | -0.099 | [-0.137, -0.062] |
| ABC | 0.164 | 0.364 | 0.499 | 0.358 | [0.317, 0.401] | 135/139/63 | -0.035 | [-0.074, +0.002] |

### W-stranger-nodocs: n = 50 queries (0 dropped: relevant file outside the corpus), 5000 resamples, seed 20261029

| arm | R@1 | R@5 | R@10 | MRR | MRR 95% CI | vs A: W/L/T | MRR diff | diff 95% CI |
|---|---|---|---|---|---|---|---|---|
| A | 0.070 | 0.310 | 0.390 | 0.216 | [0.142, 0.296] | - | - | - |
| B | 0.130 | 0.257 | 0.327 | 0.234 | [0.146, 0.331] | 22/24/4 | +0.018 | [-0.087, +0.124] |
| C | 0.150 | 0.380 | 0.460 | 0.285 | [0.192, 0.384] | 29/16/5 | +0.070 | [-0.014, +0.158] |
| D | 0.130 | 0.430 | 0.530 | 0.323 | [0.230, 0.421] | 35/7/8 | +0.108 | [+0.049, +0.171] |
| AB | 0.170 | 0.397 | 0.487 | 0.334 | [0.232, 0.440] | 29/17/4 | +0.118 | [+0.031, +0.208] |
| ABC | 0.240 | 0.457 | 0.617 | 0.418 | [0.311, 0.532] | 41/5/4 | +0.203 | [+0.126, +0.287] |

### W-stranger-all: n = 50 queries (0 dropped: relevant file outside the corpus), 5000 resamples, seed 20261029

| arm | R@1 | R@5 | R@10 | MRR | MRR 95% CI | vs A: W/L/T | MRR diff | diff 95% CI |
|---|---|---|---|---|---|---|---|---|
| A | 0.050 | 0.290 | 0.370 | 0.188 | [0.121, 0.263] | - | - | - |
| B | 0.130 | 0.237 | 0.307 | 0.225 | [0.137, 0.323] | 25/24/1 | +0.037 | [-0.070, +0.145] |
| C | 0.080 | 0.320 | 0.440 | 0.225 | [0.150, 0.308] | 28/17/5 | +0.037 | [-0.036, +0.115] |
| D | 0.080 | 0.390 | 0.460 | 0.273 | [0.192, 0.361] | 36/6/8 | +0.085 | [+0.034, +0.140] |
| AB | 0.110 | 0.347 | 0.477 | 0.271 | [0.184, 0.363] | 30/18/2 | +0.083 | [+0.004, +0.164] |
| ABC | 0.150 | 0.427 | 0.567 | 0.341 | [0.248, 0.438] | 40/4/6 | +0.153 | [+0.090, +0.223] |

R@k cells are point estimates; their 95% bootstrap intervals are in each view's `scores.json`. "W/L/T" counts per-query reciprocal-rank wins, losses and ties against A.

### Against learned-sparse, and fused partners

Paired MRR differences (arm minus baseline), condition W:

| comparison | stranger S-nodocs | commit subject |
|---|---|---|
| C - B (dense vs learned-sparse) | +0.051 [-0.079, +0.178] | +0.213 [+0.162, +0.265] |
| B - A (learned-sparse vs BM25) | +0.018 [-0.087, +0.124] | -0.173 [-0.221, -0.127] |
| D - AB (A+dense vs A+sparse) | -0.010 [-0.102, +0.081] | +0.125 [+0.083, +0.168] |
| AB - A (A+sparse vs BM25) | +0.118 [+0.031, +0.208] | -0.099 [-0.137, -0.062] |
| ABC - A | +0.203 [+0.126, +0.287] | -0.035 [-0.074, +0.002] |

Reading it: on code-vocabulary queries (commit subjects) learned-sparse is
clearly worse than BM25, as `docs/lexical-search.md` reports for doc comments,
and dense is the best single arm by point estimate (not distinguishable from
BM25). On stranger questions every second ranker helps when fused with BM25,
and dense fused with BM25 is not distinguishable from learned-sparse fused
with BM25 (-0.010 [-0.102, +0.081]). The dense-specific advantage is that it
does not cost anything on the commit-subject set, where sparse fusion loses
0.099.

### Condition P (the stock, truncated index)

The stock `dcgrep index` reaches 1139 of the 2507 files. **26 of the 50
stranger questions and 210 of the 337 commit subjects have a relevant file
outside it**, i.e. as shipped, `dcgrep rank` cannot return the answer to more
than half of these queries on this repository. Over the queries it can answer:

| view | n | A MRR | C - A | D - A | B - A |
|---|---|---|---|---|---|
| P stranger, S-nodocs | 24 | 0.310 | +0.083 [-0.031, +0.204] | +0.140 [+0.048, +0.243] | +0.012 [-0.139, +0.167] |
| P commit subject | 127 | 0.382 | +0.094 [+0.007, +0.184] | +0.003 [-0.051, +0.057] | -0.074 [-0.143, -0.001] |

P gives the same verdict on D (stranger interval excludes zero, commit does not
lose). On P, C alone also clears the commit set (+0.094) but still not the
stranger set.

### Cost

| arm | build | index size | per query |
|---|---|---|---|
| A | `dcgrep index` 3.8-7.4 s wall (three runs, machine loaded), 1.46e10 instructions retired; also builds the trigram index, not separable | `lexical.bin` 6.3 MB | `dcgrep rank` process 20 ms wall, 2.8e7 instructions |
| B | encoder 25.6 s wall, 1.01e11 instructions, then `dcgrep index` 1.1 s, 8.6e9 | `lexical.bin` 7.7 MB | same as A |
| C | 1463 s wall (24 min), 1.66e12 instructions, 2507 files, 60,551 chunks, 11.5M content tokens, peak footprint 2.7 GB | 93.0 MB of f32 vectors (60,551 x 384) | embed 17.2 ms median (12.2 ms minimum, 27.2 p90, 51.6 max over 337 queries, min of 9 repeats each, model load excluded); plus a brute-force scan of 60,551 vectors (about 23 MFLOP, arithmetic, not separately timed) |

The dense numbers are upper bounds for an engineered path: the tessl forward
used here computes the full 30522-way MLM head for every token (a synthetic
head, discarded) and snapshots every layer to the host to get the hidden
state, and it runs one chunk per forward. The wall clock was taken at a system
load average of about 14 on 18 cores; instructions retired are the stable
figure. The dense index is 15 times `lexical.bin`, and it is built by a Metal
forward that exists only on Apple silicon, so a Linux or Windows `dcgrep`
could search it but not build it.

### Noise and determinism

Arms are deterministic: a re-run of the dense query path on the stranger set
produced a byte-identical run file, and ties are broken by path. The only
variance the intervals describe is which queries were drawn, not run-to-run
noise. `n` is 50 for the stranger set (intervals are wide: about +-0.08 to 0.10 on MRR for a
single arm) and 337 for commit subjects.

### Caveats, and how much weight the verdict deserves

1. **The stranger set is small, written and labelled by one author who also
   chose the arms.** The question phrasing was derived from the docs, the
   author knows the repository, and the labels were found partly by keyword
   search, which, if anything, favours BM25. The set is 50 questions; D's
   lower bound is +0.049, so the stranger-set condition is met but not by a
   wide margin, and it was one of six arms scored on the set (the rule
   pre-names C and D only; AB and ABC are post-hoc). A second labeller or a
   second repository is the obvious next measurement before a product
   decision.
2. **Vocabulary overlap.** Commit subjects are written by the people who made
   the change and share the code's vocabulary (many name an identifier or a
   mechanism), which favours BM25; the ground truth is "files the commit
   touched", a proxy that sometimes names files the subject does not describe.
   Stranger questions avoid identifiers on purpose but were written from docs
   that the implementing code's comments often echo; the S-nodocs view removes
   `docs/` and every `.md` from every arm's list so that echo is not
   rewarded. The S-all view, with docs left in, gives the same direction
   (D +0.085 [+0.034, +0.140]).
3. **BM25 is `dcgrep`'s, untuned.** No arm was tuned: one chunking for C, k = 60
   for fusion, `dcgrep`'s own BM25 parameters. "Strongest baseline" here
   means the shipped one; a tuned BM25 or a field-weighted one was not tried.
4. **W is not what ships.** The shipped index cannot reach most of this
   repository (see P), so a verdict about dense retrieval is also a statement
   about a corpus `dcgrep` cannot currently serve. Lifting that ceiling is a
   precondition for any ranked-search improvement to matter on a repository
   this size, whatever the ranker.
5. **A package install happened.** No torch or sentence-transformers existed
   locally, so `uv pip install torch sentence-transformers numpy` ran into a
   scratch venv under this lane's scratchpad to record the parity fixture
   (documented in `record.py`); nothing from it is committed or shipped except
   the recorded fixture. Building `dc-sparse-encode` fetched its already-locked
   tessl git revision from GitHub, and `rank-eval`'s `Cargo.lock` resolved
   `serde_json` 1.0.154 (the crate was already a dependency elsewhere). No model
   was downloaded; MiniLM, doc-v2-mini and the tokenizers were in the local
   Hugging Face cache.
6. **Not done.** Arm E (embeddinggemma-2) was skipped: it is a different
   architecture needing a new harness, and the already-installed stack had
   no torch to run it cheaply. The original 250-query doc-comment protocol was
   not reconstructed: its harness is not in the repository, and rebuilding it
   would need a comment-stripped corpus and a re-run of all three arms
   (the dense encode alone is 24 minutes), so it was not cheap. The doc-comment
   result in `docs/lexical-search.md` is unchanged.

### Verdict

By the pre-stated rule: **dense embedding alone is not worth building now; BM25
fused with dense (RRF) is.** On stranger questions fusion lifts MRR from 0.216
to 0.323 (R@10 0.39 to 0.53) and wins on 35 questions against 7 losses, and it
costs nothing measurable on code-vocabulary queries (+0.026 [-0.005, +0.056]
on commit subjects).

What the rule does not say, and the owner should weigh: the same stranger-set
gain is available from fusing BM25 with the learned-sparse tier that already
exists (+0.118 [+0.031, +0.208]) at a cost of one 26-second encode and a 1.4 MB
larger index, but that fusion loses 0.099 MRR on commit subjects. Dense fusion
avoids that loss. Against that, dense costs a 24-minute Apple-silicon-only
encode of this repository, a 93 MB vector file, and a model forward at query
time (the thing `dcgrep` was designed not to need). The measured benefit is
real on 50 questions from one author and should be confirmed on a second set
before it justifies that cost; and the 56% of files the shipped index cannot
reach is a larger defect than any ranker difference measured here.

### Reproducing

```bash
# corpus extract, scratch dcgrep (ceilings raised), and arms A, B
git archive dbd0193a | tar -x -C $CORPUS
echo "{\"root\":\"$CORPUS\"}" | $DCGREP_WIDE index
rank-eval corpus --root $CORPUS --out benchmarks/retrieval-eval/corpus-W.txt
rank-eval commits --repo . --rev dbd0193a --corpus benchmarks/retrieval-eval/corpus-W.txt \
  --corpus-root $CORPUS --out benchmarks/retrieval-eval/commit-subjects.jsonl
rank-eval dcgrep --bin $DCGREP_WIDE --root $CORPUS --queries <set>.jsonl --out runs/<set>.A.W.jsonl
dc-sparse-encode --root $CORPUS --out sparse.jsonl      # then dcgrep index with "sparse", copy root, rank -> B
# arm C
dc-dense-embed index --root $CORPUS --corpus corpus-W.txt --work $WORK --out $DENSE
dc-dense-embed query --index $DENSE --queries <set>.jsonl --work $WORK --out runs/<set>.C.W.jsonl
# fuse and score everything
RE=rank-eval RUNS=runs OUT=results benchmarks/retrieval-eval/score.sh
```

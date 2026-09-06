pub const CREATE_SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS paths (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS generations (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at REAL NOT NULL,
    head_sha   TEXT,
    analysis_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS generation_nodes (
    generation_id  INTEGER NOT NULL,
    ordinal        INTEGER NOT NULL,
    file_id        INTEGER NOT NULL REFERENCES paths(id),
    name           TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    kind           TEXT NOT NULL,
    span_start     INTEGER NOT NULL,
    span_end       INTEGER NOT NULL,
    is_exported    INTEGER NOT NULL,
    body_exact     INTEGER,
    body_structural INTEGER,
    body_nodes     INTEGER,
    PRIMARY KEY (generation_id, ordinal)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS file_payloads (
    payload_id         INTEGER PRIMARY KEY,
    file_id            INTEGER NOT NULL REFERENCES paths(id),
    content_hash       INTEGER NOT NULL,
    language           TEXT NOT NULL,
    grammar_version    TEXT,
    analyzer_version   TEXT,
    parse_outcome_json TEXT NOT NULL,
    engine_json        TEXT NOT NULL,
    extraction_json    TEXT NOT NULL
);

-- The payload's identity: **the file** plus the four fields the extraction
-- cache keys on, NULL-safe.
--
-- `file_id` is in the key and must be. A payload is a serialized `Extraction`,
-- and an `Extraction` carries its own `file_path` — so content-addressing
-- alone collapses two files with identical bytes into one payload and makes
-- both membership rows report the *same* path. That is not hypothetical:
-- `a_cold_build_indexes_an_in_root_symlink_and_a_drain_of_it_keeps_the_symbol`
-- caught it on the first run, because a symlink and its target are byte-
-- identical by construction and the linked path vanished from the generation.
--
-- Nothing is lost. What B3 deduplicates is the *same file, unchanged, across
-- generations*, which is 1,530 of the 1,530 duplicate rows measured on this
-- repository. Two different files that happen to share content have genuinely
-- different payloads.
--
-- `grammar_version` and `analyzer_version` are nullable — NULL means "stored by
-- a build with no parsing frontend", which is a real state — and SQLite treats
-- NULLs as distinct inside a UNIQUE index, so a plain unique constraint would
-- let two identical NULL-version payloads both insert and defeat the point. The
-- COALESCE expressions make the index NULL-safe; the write path probes with the
-- same expressions.
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_payloads_identity
    ON file_payloads(file_id, content_hash, language,
                     COALESCE(grammar_version, ''), COALESCE(analyzer_version, ''));

-- The extraction-cache fallback asks by content identity alone — it has no
-- path, because `CacheKey` has none — so it needs its own index. This is v13's
-- index, over one row per (file, content) instead of one per (generation,
-- file): the relation v13 described as a scan "whose rows each carry a ~47 KB
-- `extraction_json` the scan must skip past".
CREATE INDEX IF NOT EXISTS idx_file_payloads_cache_identity
    ON file_payloads(content_hash, language, grammar_version, analyzer_version);

CREATE TABLE IF NOT EXISTS generation_file_rows (
    generation_id INTEGER NOT NULL,
    file_id       INTEGER NOT NULL REFERENCES paths(id),
    payload_id    INTEGER NOT NULL REFERENCES file_payloads(payload_id),
    PRIMARY KEY (generation_id, file_id)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_generation_file_rows_payload
    ON generation_file_rows(payload_id);

-- `generation_files` keeps its name and its exact column set, as a view.
--
-- Twenty-five read sites across five crates, `tools/fanout.sql` and a dozen
-- tests query this relation by name. Splitting the payload out under a *new*
-- name would have meant rewriting every one of them for a change none of them
-- cares about: what a generation holds for a file is unchanged, only where the
-- bytes live.
CREATE VIEW IF NOT EXISTS generation_files AS
SELECT m.generation_id      AS generation_id,
       m.file_id            AS file_id,
       p.language           AS language,
       p.content_hash       AS content_hash,
       p.parse_outcome_json AS parse_outcome_json,
       p.engine_json        AS engine_json,
       p.extraction_json    AS extraction_json,
       p.grammar_version    AS grammar_version,
       p.analyzer_version   AS analyzer_version
  FROM generation_file_rows m
  JOIN file_payloads p ON p.payload_id = m.payload_id;

CREATE TABLE IF NOT EXISTS generation_edges (
    generation_id  INTEGER NOT NULL,
    ordinal        INTEGER NOT NULL,
    source_file_id INTEGER NOT NULL REFERENCES paths(id),
    target_file_id INTEGER NOT NULL REFERENCES paths(id),
    source_symbol  TEXT NOT NULL,
    target_symbol  TEXT NOT NULL,
    edge_kind      TEXT NOT NULL,
    confidence     REAL NOT NULL,
    resolution     TEXT,
    candidate_total INTEGER,
    PRIMARY KEY (generation_id, ordinal)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS generation_coverage_gaps (
    generation_id INTEGER NOT NULL,
    gap           TEXT NOT NULL,
    path          TEXT NOT NULL,
    reason        TEXT NOT NULL,
    PRIMARY KEY (generation_id, gap, path)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS generation_dead_symbols (
    generation_id    INTEGER NOT NULL,
    ordinal          INTEGER NOT NULL,
    file_path        TEXT NOT NULL,
    symbol_name      TEXT NOT NULL,
    confidence       REAL NOT NULL,
    is_exempt        INTEGER NOT NULL,
    exemption_reason TEXT,
    PRIMARY KEY (generation_id, ordinal)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_generation_nodes_file
    ON generation_nodes(generation_id, file_id);
CREATE INDEX IF NOT EXISTS idx_generation_edges_source
    ON generation_edges(generation_id, source_file_id);
CREATE INDEX IF NOT EXISTS idx_generation_edges_target
    ON generation_edges(generation_id, target_file_id);

CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
    name, qualified_name, path, tokenize='unicode61'
);

CREATE TABLE IF NOT EXISTS nodes_fts_map (
    rowid_ref     INTEGER NOT NULL,
    generation_id INTEGER NOT NULL,
    PRIMARY KEY (generation_id, rowid_ref)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS pending_paths (
    path       TEXT PRIMARY KEY,
    queued_at  REAL NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS extraction_cache (
    content_hash     INTEGER NOT NULL,
    language         TEXT NOT NULL,
    grammar_version  TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    payload_json     TEXT NOT NULL,
    accessed_at      REAL NOT NULL,
    PRIMARY KEY (content_hash, language, grammar_version, analyzer_version)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS extraction_retry (
    content_hash INTEGER PRIMARY KEY,
    language     TEXT NOT NULL,
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_reason  TEXT NOT NULL,
    updated_at   REAL NOT NULL
) WITHOUT ROWID;

"#;

pub const MIGRATION_V3_TO_V4: &str = r#"
CREATE TABLE IF NOT EXISTS extraction_retry (
    content_hash INTEGER PRIMARY KEY,
    language     TEXT NOT NULL,
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_reason  TEXT NOT NULL,
    updated_at   REAL NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS extraction_cache_v4 (
    content_hash     INTEGER NOT NULL,
    language         TEXT NOT NULL,
    grammar_version  TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    payload_json     TEXT NOT NULL,
    accessed_at      REAL NOT NULL,
    PRIMARY KEY (content_hash, language, grammar_version, analyzer_version)
) WITHOUT ROWID;

INSERT OR IGNORE INTO extraction_cache_v4 (content_hash, language, grammar_version, analyzer_version, payload_json, accessed_at)
SELECT content_hash, 'unknown', 'legacy', 'legacy', payload_json, accessed_at
FROM extraction_cache;

DROP TABLE IF EXISTS extraction_cache;
ALTER TABLE extraction_cache_v4 RENAME TO extraction_cache;
"#;

pub const MIGRATION_V4_TO_V5: &str = r#"
CREATE TABLE IF NOT EXISTS generations (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at REAL NOT NULL,
    head_sha   TEXT
);

CREATE TABLE IF NOT EXISTS file_payloads (
    payload_id         INTEGER PRIMARY KEY,
    file_id            INTEGER NOT NULL REFERENCES paths(id),
    content_hash       INTEGER NOT NULL,
    language           TEXT NOT NULL,
    grammar_version    TEXT,
    analyzer_version   TEXT,
    parse_outcome_json TEXT NOT NULL,
    engine_json        TEXT NOT NULL,
    extraction_json    TEXT NOT NULL
);

-- The payload's identity: **the file** plus the four fields the extraction
-- cache keys on, NULL-safe.
--
-- `file_id` is in the key and must be. A payload is a serialized `Extraction`,
-- and an `Extraction` carries its own `file_path` — so content-addressing
-- alone collapses two files with identical bytes into one payload and makes
-- both membership rows report the *same* path. That is not hypothetical:
-- `a_cold_build_indexes_an_in_root_symlink_and_a_drain_of_it_keeps_the_symbol`
-- caught it on the first run, because a symlink and its target are byte-
-- identical by construction and the linked path vanished from the generation.
--
-- Nothing is lost. What B3 deduplicates is the *same file, unchanged, across
-- generations*, which is 1,530 of the 1,530 duplicate rows measured on this
-- repository. Two different files that happen to share content have genuinely
-- different payloads.
--
-- `grammar_version` and `analyzer_version` are nullable — NULL means "stored by
-- a build with no parsing frontend", which is a real state — and SQLite treats
-- NULLs as distinct inside a UNIQUE index, so a plain unique constraint would
-- let two identical NULL-version payloads both insert and defeat the point. The
-- COALESCE expressions make the index NULL-safe; the write path probes with the
-- same expressions.
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_payloads_identity
    ON file_payloads(file_id, content_hash, language,
                     COALESCE(grammar_version, ''), COALESCE(analyzer_version, ''));

-- The extraction-cache fallback asks by content identity alone — it has no
-- path, because `CacheKey` has none — so it needs its own index. This is v13's
-- index, over one row per (file, content) instead of one per (generation,
-- file): the relation v13 described as a scan "whose rows each carry a ~47 KB
-- `extraction_json` the scan must skip past".
CREATE INDEX IF NOT EXISTS idx_file_payloads_cache_identity
    ON file_payloads(content_hash, language, grammar_version, analyzer_version);

CREATE TABLE IF NOT EXISTS generation_file_rows (
    generation_id INTEGER NOT NULL,
    file_id       INTEGER NOT NULL REFERENCES paths(id),
    payload_id    INTEGER NOT NULL REFERENCES file_payloads(payload_id),
    PRIMARY KEY (generation_id, file_id)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_generation_file_rows_payload
    ON generation_file_rows(payload_id);

-- `generation_files` keeps its name and its exact column set, as a view.
--
-- Twenty-five read sites across five crates, `tools/fanout.sql` and a dozen
-- tests query this relation by name. Splitting the payload out under a *new*
-- name would have meant rewriting every one of them for a change none of them
-- cares about: what a generation holds for a file is unchanged, only where the
-- bytes live.
CREATE VIEW IF NOT EXISTS generation_files AS
SELECT m.generation_id      AS generation_id,
       m.file_id            AS file_id,
       p.language           AS language,
       p.content_hash       AS content_hash,
       p.parse_outcome_json AS parse_outcome_json,
       p.engine_json        AS engine_json,
       p.extraction_json    AS extraction_json,
       p.grammar_version    AS grammar_version,
       p.analyzer_version   AS analyzer_version
  FROM generation_file_rows m
  JOIN file_payloads p ON p.payload_id = m.payload_id;

CREATE TABLE IF NOT EXISTS generation_dead_symbols (
    generation_id    INTEGER NOT NULL,
    ordinal          INTEGER NOT NULL,
    file_path        TEXT NOT NULL,
    symbol_name      TEXT NOT NULL,
    confidence       REAL NOT NULL,
    is_exempt        INTEGER NOT NULL,
    exemption_reason TEXT,
    PRIMARY KEY (generation_id, ordinal)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_generation_nodes_file
    ON generation_nodes(generation_id, file_id);
CREATE INDEX IF NOT EXISTS idx_generation_edges_source
    ON generation_edges(generation_id, source_file_id);
CREATE INDEX IF NOT EXISTS idx_generation_edges_target
    ON generation_edges(generation_id, target_file_id);
"#;

/// One durable row per committed generation, written inside the generation's
/// own transaction so a build can never be counted without its history entry
/// (or vice versa). Retention is capped independently of generation pruning:
/// history rows are ~100 B and outlive the graph they describe, which is the
/// entire point of a longitudinal view.
pub const BUILD_HISTORY_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS build_history (
    generation_id     INTEGER PRIMARY KEY,
    built_at          REAL NOT NULL,
    head_sha          TEXT NOT NULL,
    files             INTEGER NOT NULL,
    symbols           INTEGER NOT NULL,
    edges             INTEGER NOT NULL,
    dead_confident    INTEGER NOT NULL,
    dead_ambiguous    INTEGER NOT NULL,
    parse_failed      INTEGER NOT NULL,
    languages_covered INTEGER NOT NULL,
    build_ms          INTEGER CHECK (build_ms IS NULL OR build_ms >= 0),
    db_bytes          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_build_history_built_at
    ON build_history(built_at DESC);
"#;

pub const MIGRATION_V5_TO_V6: &str = BUILD_HISTORY_TABLE;

/// Longest history the store retains. Rows are tiny, but the cap keeps an
/// always-on watcher from growing the table without bound.
pub const BUILD_HISTORY_RETENTION: usize = 500;

/// Generations retained after each committed build.
///
/// Every generation carries a full carry-forward copy of the repository's
/// extraction payloads, nodes and edges, so an unpruned store grows by
/// O(repository size) per build forever — measured at +327 MiB per one-line
/// edit on a 4,731-file repository (SC1).
///
/// One is the minimum that is actually correct: the differential builder reads
/// exactly one prior generation to carry rows forward, and nothing else in the
/// tree reads a non-latest generation. The second is deliberate headroom for
/// rename-alias chaining and for inspecting the previous build after a bad one,
/// and matches the Python incumbent's `retain_generations = 2`.
pub const GENERATION_RETENTION: usize = 2;

/// v7: record the absolute root a generation was built from.
///
/// Node paths are stored repo-relative. Without the root, a query process
/// resolves them against its own working directory, so every source span read
/// from anywhere but the repo root silently comes back empty. `ALTER TABLE ADD
/// COLUMN` is the migration: existing rows keep NULL, which reads as "root
/// unknown" rather than as a wrong root.
pub const MIGRATION_V6_TO_V7: &str = r#"
ALTER TABLE generations ADD COLUMN repo_root TEXT;
"#;

/// v8: record the grammar and analyzer identity a generation's payload was
/// produced with.
///
/// `extraction_cache` is keyed `(content_hash, language, grammar_version,
/// analyzer_version)` precisely so a payload produced by older extraction
/// semantics can never be reused. `generation_files` held byte-identical
/// payloads but recorded only `(language, content_hash)`, so it could not be
/// used as a fallback source without silently discarding that guarantee —
/// exactly the staleness `EXTRACTION_SCHEMA_VERSION` exists to prevent, and how
/// fixed false positives would come back. Carrying the identity here lets the
/// cache and the generation store hold one copy between them instead of two
/// (SC8). Existing rows keep NULL, which reads as "identity unknown" and is
/// therefore never eligible as a fallback — absence of proof, not proof.
pub const MIGRATION_V7_TO_V8: &str = r#"
ALTER TABLE generation_files ADD COLUMN grammar_version TEXT;
ALTER TABLE generation_files ADD COLUMN analyzer_version TEXT;
"#;

/// D17: calls seen but never attributed to a target.
///
/// The resolver already computes these — they are the honest denominator for
/// any "how complete is this graph" question — but they lived only in memory,
/// so nothing could ask why a symbol had no callers. Stored per generation and
/// pruned with it, like `generation_dead_symbols`.
pub const UNRESOLVED_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS generation_unresolved (
    generation_id INTEGER NOT NULL,
    ordinal       INTEGER NOT NULL,
    source_file   TEXT NOT NULL,
    source_symbol TEXT NOT NULL,
    callee_name   TEXT NOT NULL,
    reason        TEXT NOT NULL,
    classification TEXT NOT NULL DEFAULT 'unresolved',
    receiver       TEXT,
    PRIMARY KEY (generation_id, ordinal)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_generation_unresolved_callee
    ON generation_unresolved(generation_id, callee_name);

CREATE INDEX IF NOT EXISTS idx_generation_unresolved_class
    ON generation_unresolved(generation_id, classification);
"#;

pub const MIGRATION_V8_TO_V9: &str = UNRESOLVED_TABLE;

/// SC18: `classification` splits calls that *cannot* resolve — language
/// builtins, and names an import proves come from outside the corpus — from the
/// genuine failures that indicate a defect. Without it every consumer reads one
/// undifferentiated count, which is what made 380k expected rows hide the two
/// extraction bugs closed as SC17.
///
/// The default backfills existing rows as `unresolved`, which is exactly what
/// they meant when they were written: the classifier had not run, so claiming
/// any of them were expected would assert something never measured.
pub const MIGRATION_V9_TO_V10: &str = r#"
ALTER TABLE generation_unresolved
    ADD COLUMN classification TEXT NOT NULL DEFAULT 'unresolved';

CREATE INDEX IF NOT EXISTS idx_generation_unresolved_class
    ON generation_unresolved(generation_id, classification);
"#;

/// SC25: the receiver expression a call was made on, or NULL for a bare call.
///
/// Added because the classification could not be *audited* without it. Asking
/// "is `uninferred_receiver` really all method calls, and is the `unresolved`
/// tier really all bare names" required instrumenting a build, since the row
/// recorded only the callee. A classification nobody can check is a claim, and
/// this table exists precisely to be the honest denominator.
///
/// Nullable rather than defaulted: a bare call has no receiver, and writing an
/// empty string would make "no receiver" indistinguishable from "a receiver
/// whose text we failed to capture". Existing rows backfill to NULL, which is
/// truthful — the column did not exist when they were written.
pub const MIGRATION_V10_TO_V11: &str = r#"
ALTER TABLE generation_unresolved ADD COLUMN receiver TEXT;
"#;

/// SC26: body signatures for clone detection.
///
/// Three nullable columns rather than a `generation_clones` table, because a
/// clone group is not a fact about the tree — it is a join over facts about
/// symbols. Storing the groups would mean storing a derived, truncated view
/// that has to be kept in step with the rows it came from; storing the hashes
/// lets any generation be grouped on demand, in full, by the one grouping
/// implementation in `devmap-analyze`.
///
/// No covering index. `generation_nodes` is `WITHOUT ROWID` on
/// `(generation_id, ordinal)`, so reading one generation's symbols is already a
/// primary-key range scan; an index on the hashes would add store size — the
/// thing this schema works to bound — to save nothing on a scan that has to
/// touch every row of the generation anyway.
///
/// Nullable, and null means "no signature was computed for this symbol": a body
/// under the size floor, a kind with no comparable body, or a file no grammar
/// parsed. Rows written before this column existed backfill to null, which says
/// the same true thing about them.
pub const MIGRATION_V11_TO_V12: &str = r#"
ALTER TABLE generation_nodes ADD COLUMN body_exact INTEGER;
ALTER TABLE generation_nodes ADD COLUMN body_structural INTEGER;
ALTER TABLE generation_nodes ADD COLUMN body_nodes INTEGER;
"#;

/// v13: make the SC8 extraction-cache fallback a lookup instead of a scan.
///
/// `try_get_cached_extraction` misses `extraction_cache` and falls back to
/// `generation_files`, matching on the full cache identity. `generation_files`
/// is `WITHOUT ROWID` keyed `(generation_id, file_id)` and had **no index on
/// `content_hash`**, so `EXPLAIN QUERY PLAN` reported `SCAN generation_files`
/// for that fallback — once per file, on every build.
///
/// The fallback is not the exceptional path, it is the *only* path: SC7's
/// `prune_extraction_cache` deletes every `extraction_cache` row that a
/// retained generation already holds with a matching identity, which is all of
/// them. Measured on a cold-built store, `extraction_cache` holds **0 rows**
/// after every build across 10 consecutive builds — so the first query always
/// misses and every file pays a scan whose rows each carry a ~47 KB
/// `extraction_json` the scan must skip past to reach the identity columns.
///
/// Measured against the release binary, an 8,001-file synthetic corpus, no-op
/// build (nothing changed — the case a watcher hits on every tick), four runs:
///
/// | | min | median | max |
/// |---|---|---|---|
/// | before | 2.311 s | 2.619 s | 4.543 s |
/// | after  | 0.690 s | 0.724 s | 0.826 s |
///
/// **3.3x on the median.** Those two rows were taken when the index landed and
/// are not re-measured here; the "before" one cannot be without reverting the
/// migration.
///
/// The *scaling* was re-measured on 2026-09-05 against the merged tree, release
/// binary, `benchmarks/map_bench.py --synthetic N --repeat 5`, minimum
/// reported: the no-op build is **110 ms over 2,001 files and 444 ms over
/// 8,001** — 4.0x the time for 4.0x the files, so the cost is linear in corpus
/// size rather than in corpus size times stored bytes. Cold build over the same
/// pair is 657 ms -> 3.37 s (5.1x), and throughput falls only 3,047 -> 2,375
/// files/s across the 4x.
///
/// `CREATE INDEX IF NOT EXISTS` is idempotent, so this step needs no probe —
/// unlike the `ADD COLUMN` migrations. Index build cost measured at 24 ms on a
/// 1,333-row store, with no measurable file growth.
pub const MIGRATION_V12_TO_V13: &str = r#"
CREATE INDEX IF NOT EXISTS idx_generation_files_cache_identity
    ON generation_files(content_hash, language, grammar_version, analyzer_version);
"#;

/// v14: the inventory of what a generation could not read.
///
/// One row per path, not a number. `AnalysisSummary.discovery_refused_files`
/// was a count, and a count cannot be *maintained* — only replaced. That is
/// what forced the daemon's incremental drain, which never re-walks discovery,
/// to carry the previous generation's number forward and take
/// `max(previous, this_batch)` as a floor. The floor bought "a resync must not
/// erase a recorded refusal" with two wrong answers: a repaired file stayed
/// counted until a full re-extraction, and a refusal this batch met vanished
/// into a larger carried number — the second an over-claim, the shape this
/// codebase treats as the expensive one.
///
/// With the paths stored, the drain carries the inventory *minus every path in
/// this batch's affected set*, plus what this batch was turned away from: a
/// path nothing touched keeps its verdict, a path this batch touched is
/// re-decided by `candidate_kind`. The count is then `COUNT(*)` and cannot
/// drift from the set it counts.
///
/// The same table holds the two extraction gaps — files a grammar was wanted
/// for and did not read, and files recovered by line pattern — for a different
/// reason: they *are* derivable from `generation_files`, but only by
/// deserializing `parse_outcome_json` for every file in the generation, and
/// those rows carry a ~47 KB `extraction_json` each that the scan has to walk
/// past. Measured on this repository that is the difference between a `status`
/// costing under a millisecond and one costing tens. The write path derives
/// them from `devmap_analyze::extraction_gaps`, the same owner
/// `extraction_coverage` folds, so the stored list cannot disagree with the
/// counts the analysis reported.
///
/// `WITHOUT ROWID` and keyed `(generation_id, gap, path)`: reading one
/// generation's gaps of one kind is a primary-key range scan, which is what
/// `status` does three times.
pub const COVERAGE_GAPS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS generation_coverage_gaps (
    generation_id INTEGER NOT NULL,
    gap           TEXT NOT NULL,
    path          TEXT NOT NULL,
    reason        TEXT NOT NULL,
    PRIMARY KEY (generation_id, gap, path)
) WITHOUT ROWID;
"#;

/// v15: the evidence tier each edge was built from.
///
/// `ResolvedEdge::new` is the only constructor the resolver uses, so an edge's
/// `confidence` cannot disagree with its `Resolution` on the way in. On the way
/// back out there was nothing: no column held the resolution, so
/// `devmap-query`'s `stored_edge_to_resolved` rebuilt every edge with
/// `resolution: None` and the honesty invariant rested on the round trip plus
/// the write-side constructor — never on a second, independent reading of the
/// same fact.
///
/// Nullable, and NULL is not a tier. It means "written before this column
/// existed", which is why the read path labels such an edge
/// `ResolutionSource::Reconstructed`: a variant guessed from the row's file
/// layout must never be indistinguishable from one the resolver actually
/// recorded.
pub const MIGRATION_V14_TO_V15: &str = r#"
ALTER TABLE generation_edges ADD COLUMN resolution TEXT;
"#;

/// How many candidates an ambiguous resolution actually held.
///
/// `AMBIGUOUS_FANOUT_CAP` (audit R-7) bounds how many **edges** one ambiguous
/// site emits — 16. It does not bound the site's candidate list, which the
/// `Arc<Resolution>` still holds in full, deliberately: that list is what keeps
/// `impact` answerable on candidates 2..N. So resolver memory is proportional
/// to *candidates* while every number derivable from the store counted
/// *emitted edges*, and since R-7 the two have not been the same quantity.
///
/// `verify.sh` step 6 has been red because of it. Its three coefficient caps
/// were calibrated on a resolver with no cap, and re-deriving them needs the
/// denominator the memory actually tracks — which was not in the store at all:
/// the candidate list lives only on the in-memory `ResolvedEdge`, and
/// `generation_edges` had no column that could carry any part of it. Fixing the
/// gate by moving a coefficient instead would have been the "raised to fit"
/// this repository refuses.
///
/// One integer, on the ambiguous rows only. NULL means one of two things and
/// the reader must not conflate them: the edge is not an `AmbiguousGlobal` (no
/// candidate list exists), or the row predates this column. `resolution` tells
/// them apart — an ambiguous row written by this binary always carries a
/// count, so `resolution = 'AmbiguousGlobal' AND candidate_total IS NULL` is an
/// older row and a query that needs the denominator must refuse rather than
/// treat it as zero.
pub const MIGRATION_V15_TO_V16: &str = r#"
ALTER TABLE generation_edges ADD COLUMN candidate_total INTEGER;
"#;

/// v17: store one extraction payload per *content*, not per generation (B3).
///
/// Measured on this repository, two generations apart by a single edited line:
/// `generation_files` held 3,062 rows totalling **164.5 MB of
/// `extraction_json`, 54% of a 302.8 MB store — and 1,530 of those rows were
/// byte-identical duplicates.** One edited file caused ~82 MB of JSON to be
/// read out of SQLite, moved through Rust one row at a time, and written back.
/// That is the whole of B3's measured cost: the carry-forward this store has
/// done since B3's first half landed avoids re-*deriving* an unchanged payload,
/// but still re-*materialises* it under the new generation id.
///
/// The split is by the identity the extraction cache already keys on —
/// `(content_hash, language, grammar_version, analyzer_version)`, which v13
/// indexed on `generation_files` for exactly this lookup and described as a
/// scan "whose rows each carry a ~47 KB `extraction_json` the scan must skip
/// past to reach the identity columns". Those columns now live in a table with
/// one row per distinct payload, so that lookup stops skipping past anything.
///
/// `generation_files` keeps its name and its exact column set as a view over
/// the join, so all twenty-five read sites, `tools/fanout.sql` and the tests
/// are unchanged: what a generation holds for a file has not changed, only
/// where the bytes live.
///
/// The backfill deduplicates as it copies. `INSERT OR IGNORE` against the
/// NULL-safe unique index keeps the first payload of each identity; the
/// membership rows then join back to it, so a store with N generations of an
/// unchanged file collapses to one payload and N 16-byte rows.
pub const MIGRATION_V16_TO_V17: &str = r#"
-- Identical to the fresh-create shape above, `file_id` included. It was omitted
-- here and present there, so a store created by this build worked and a store
-- *migrated* by it could not: the INSERT below names `file_id`, and every
-- runtime probe keys on it. No test migrated a real v16 store, so 1,842 of them
-- passed over it — a fresh store never walks this step.
--
-- The column is not cosmetic. Without it the identity is content-addressed
-- alone, which collapses two byte-identical files into one payload and makes
-- both membership rows report the same path.
CREATE TABLE IF NOT EXISTS file_payloads (
    payload_id         INTEGER PRIMARY KEY,
    file_id            INTEGER NOT NULL REFERENCES paths(id),
    content_hash       INTEGER NOT NULL,
    language           TEXT NOT NULL,
    grammar_version    TEXT,
    analyzer_version   TEXT,
    parse_outcome_json TEXT NOT NULL,
    engine_json        TEXT NOT NULL,
    extraction_json    TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_file_payloads_identity
    ON file_payloads(file_id, content_hash, language,
                     COALESCE(grammar_version, ''), COALESCE(analyzer_version, ''));

CREATE TABLE IF NOT EXISTS generation_file_rows (
    generation_id INTEGER NOT NULL,
    file_id       INTEGER NOT NULL REFERENCES paths(id),
    payload_id    INTEGER NOT NULL REFERENCES file_payloads(payload_id),
    PRIMARY KEY (generation_id, file_id)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_generation_file_rows_payload
    ON generation_file_rows(payload_id);

INSERT OR IGNORE INTO file_payloads
    (file_id, content_hash, language, grammar_version, analyzer_version,
     parse_outcome_json, engine_json, extraction_json)
SELECT file_id, content_hash, language, grammar_version, analyzer_version,
       parse_outcome_json, engine_json, extraction_json
  FROM generation_files;

INSERT OR IGNORE INTO generation_file_rows (generation_id, file_id, payload_id)
SELECT f.generation_id, f.file_id, p.payload_id
  FROM generation_files f
  JOIN file_payloads p
    ON p.file_id = f.file_id
   AND p.content_hash = f.content_hash
   AND p.language = f.language
   AND COALESCE(p.grammar_version, '') = COALESCE(f.grammar_version, '')
   AND COALESCE(p.analyzer_version, '') = COALESCE(f.analyzer_version, '');

DROP INDEX IF EXISTS idx_generation_files_cache_identity;
DROP TABLE generation_files;

CREATE VIEW generation_files AS
SELECT m.generation_id      AS generation_id,
       m.file_id            AS file_id,
       p.language           AS language,
       p.content_hash       AS content_hash,
       p.parse_outcome_json AS parse_outcome_json,
       p.engine_json        AS engine_json,
       p.extraction_json    AS extraction_json,
       p.grammar_version    AS grammar_version,
       p.analyzer_version   AS analyzer_version
  FROM generation_file_rows m
  JOIN file_payloads p ON p.payload_id = m.payload_id;
"#;

pub const CURRENT_SCHEMA_VERSION: i32 = 17;

#[cfg(test)]
mod retention_constant_tests {
    /// `devmap-extract` cannot depend on this crate, so its steady-state size
    /// budget mirrors `GENERATION_RETENTION` in its own constant. SC15 is the
    /// precedent for what happens when a policy number lives in two places and
    /// nothing compares them: the copy nobody runs goes stale silently.
    #[test]
    fn retention_matches_the_store_constant() {
        assert_eq!(
            u64::try_from(super::GENERATION_RETENTION).unwrap(),
            devmap_extract::model::DB_SIZE_GATE_RETAINED_GENERATIONS,
            "the steady-state size budget assumes a different retention count \
             than the store actually keeps"
        );
    }
}

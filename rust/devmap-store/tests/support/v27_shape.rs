//! The schema-27 shape of the relations v28 moved onto payloads and ranges,
//! rebuilt from a store this build created.
//!
//! A reduced store's `extraction_json` still holds the zstd frames v29 wrote
//! (SQL cannot decompress them back). Walking it up exercises exactly what a
//! store re-stamped down from v29 meets: the digest backfill reads the frames
//! through the codec, and the v29 rung re-adds `entry_wiring_json` and fills it
//! from the stored documents.
//!
//! Two test targets reduce a store to an earlier rung by hand
//! (`migration_ladder` for every rung, `store_hardening_without_grammars` for
//! the payload split), and both start from a store this build created, which is
//! v28. Each needs the same first step -- put the flat per-generation tables
//! back -- so it is written once.

/// v29's one column. A store this build created is reduced with this first and
/// then [`REDUCE_V28_TO_V27`]: the columns come off in the reverse of the order
/// the ladder adds them, which is also what makes a re-walked store's column
/// order the fresh store's.
pub const REDUCE_V29_TO_V28: &str = "ALTER TABLE file_payloads DROP COLUMN entry_wiring_json;";

/// Materialise `generation_nodes`, `generation_literals`,
/// `generation_file_rows` and `generation_file_digests` back into the flat
/// tables v27 stored, `file_payloads` without its `payload_digest` and under
/// the identity index v27 had, with the dense per-generation ordinal and generation-scoped
/// full-text rowid v27 used, and restore `nodes_fts_map`. Drops v28's own
/// tables and views.
///
/// `generation_files` is dropped for the length of the swap and put back as
/// `CREATE_SCHEMA_V3` defines it: it names `generation_file_rows`, which SQLite
/// re-parses when a table is renamed into that name.
pub const REDUCE_V28_TO_V27: &str = "DROP INDEX idx_file_payloads_digest_identity;
     ALTER TABLE file_payloads DROP COLUMN payload_digest;
     CREATE UNIQUE INDEX idx_file_payloads_identity
         ON file_payloads(file_id, content_hash, language,
                          COALESCE(grammar_version, ''), COALESCE(analyzer_version, ''));
     CREATE TABLE gn_flat AS
         SELECT generation_id,
                ROW_NUMBER() OVER (PARTITION BY generation_id ORDER BY ordinal) - 1
                    AS ordinal,
                file_id, name, qualified_name, kind, span_start, span_end,
                is_exported, body_exact, body_structural, body_nodes
           FROM generation_nodes;
     CREATE TABLE gl_flat AS SELECT * FROM generation_literals;
     CREATE TABLE gfr_flat AS SELECT * FROM generation_file_rows;
     CREATE TABLE gfd_flat AS SELECT * FROM generation_file_digests;
     DROP VIEW generation_files;
     DROP VIEW generation_nodes;
     DROP VIEW nodes_fts_map;
     DROP VIEW generation_literals;
     DROP VIEW generation_file_rows;
     DROP VIEW generation_file_digests;
     DROP TABLE nodes_fts;
     DROP TABLE payload_nodes;
     DROP TABLE payload_literals;
     DROP TABLE file_rows;
     DROP TABLE digest_rows;
     ALTER TABLE gn_flat RENAME TO generation_nodes;
     ALTER TABLE gl_flat RENAME TO generation_literals;
     ALTER TABLE gfr_flat RENAME TO generation_file_rows;
     ALTER TABLE gfd_flat RENAME TO generation_file_digests;
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
     CREATE TABLE nodes_fts_map (
         rowid_ref     INTEGER NOT NULL,
         generation_id INTEGER NOT NULL,
         PRIMARY KEY (generation_id, rowid_ref)
     ) WITHOUT ROWID;
     CREATE VIRTUAL TABLE nodes_fts USING fts5(
         name, qualified_name, path, tokenize='unicode61'
     );
     INSERT INTO nodes_fts_map (rowid_ref, generation_id)
         SELECT (generation_id << 32) | ordinal, generation_id
           FROM generation_nodes;
     INSERT INTO nodes_fts (rowid, name, qualified_name, path)
         SELECT (n.generation_id << 32) | n.ordinal, n.name, n.qualified_name, p.path
           FROM generation_nodes n JOIN paths p ON p.id = n.file_id;";

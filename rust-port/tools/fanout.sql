-- Ambiguity fan-out metrics for the latest generation of a devmap store.
--
-- WHY THIS EXISTS
-- ---------------
-- SC3 established that resolver peak memory is driven by the *ambiguity
-- fan-out*, not by the edge total: a call with N same-family candidates emits N
-- edges. Any gate on that behaviour needs the fan-out as a number, and the
-- number has to come from somewhere that is not the resolver's own source —
-- both so the gate stays honest across refactors and so it can be run against
-- any store, including one produced by an older binary.
--
-- WHAT AN AMBIGUOUS FAN-OUT LOOKS LIKE IN THE STORE
-- -------------------------------------------------
-- `Confidence::SPECULATIVE` (0.2) has exactly one producer in the whole
-- resolver: the `Resolution::AmbiguousGlobal` rung, which loops over its
-- candidate list pushing one `EdgeKind::Calls` edge per candidate. Every edge
-- in one fan-out therefore shares (source file, caller symbol, callee name) and
-- differs only in the target. So:
--
--     fan-out group = (source_file_id, source_symbol, callee name), at
--                     edge_kind='Calls' AND confidence=0.2
--     N             = rows in the group
--     sum_n2        = SUM(N*N) over groups
--
-- The callee name is not a column. It is recovered two independent ways and the
-- two are required to agree — see `_alt_grp` below.
--
-- WHAT THIS IS NOT
-- ----------------
-- The resolver sorts and dedups edges before they are persisted, so two calls
-- to the same ambiguous name from the same caller collapse into one group. This
-- Sum(N^2) is therefore a **lower bound** on the resolver-side Sum(N^2), which
-- counts call occurrences. That is the fail-closed direction for a coefficient
-- gate: a smaller denominator makes the observed bytes-per-pair larger, so the
-- gate can raise a false alarm but can never miss a real one.
--
-- SHAPE
-- -----
-- Temp tables rather than one CTE chain: the callee join needs an index, and a
-- materialised CTE cannot carry one. Measured on a 1M-edge store, the indexed
-- form is 45 s -> 0.4 s. Temp tables live in the temp database, so this still
-- runs against a `-readonly` main database.
--
-- Everything before the `@@RESULT@@` marker line is setup; everything after is
-- a single row-producing SELECT. `sqlite3 <db> < fanout.sql` runs the whole
-- file; a rusqlite caller splits on that line. The marker is load-bearing, and
-- must appear exactly once — do not remove it or write it a second time.
--
-- EDGES ARE NOT CANDIDATES
-- ------------------------
-- `AMBIGUOUS_FANOUT_CAP` (audit R-7) bounds how many edges one site emits — 16.
-- It does not bound the site's *candidate list*, which the `Arc<Resolution>`
-- still holds in full, deliberately, because that list is what keeps `impact`
-- answerable on candidates 2..N. So resolver memory is proportional to
-- candidates while every number above counts emitted edges, and since R-7 the
-- two have not been the same quantity. That is why `verify.sh` step 6 was red:
-- its coefficients were calibrated before the cap and their denominator no
-- longer described what the memory was doing.
--
-- `generation_edges.candidate_total` (schema v16) carries the real number, on
-- the ambiguous rows only. From it:
--
--     candidates     = SUM(candidate_total) over one edge per fan-out group
--     candidate_n2   = SUM(candidate_total^2) over the same
--
-- One edge per group, not one per row: every edge in a fan-out shares the same
-- candidate list, so summing the column across all N rows would count the list
-- N times and produce a denominator N times too large — which is fail-*open*,
-- the direction a coefficient gate must never round.
--
-- `pre_v16` counts ambiguous rows written before the column existed. It must be
-- 0 for the candidate metrics to mean anything: NULL there is "not recorded",
-- and treating it as zero would shrink the denominator silently.
--
-- OUTPUT (one row, pipe-separated)
--   files | fanout_edges | sum_n2 | sites | max_n | unjoined | alt_sum_n2 |
--   alt_sites | candidates | candidate_n2 | max_candidates | pre_v16
--
-- `unjoined` and `pre_v16` must be 0 and `alt_*` must equal their primaries; a
-- caller that does not check all three is reading a number it has not
-- validated.

CREATE TEMP TABLE _gen AS SELECT max(id) AS id FROM generations;

CREATE TEMP TABLE _fanout AS
SELECT e.source_file_id AS sf, e.source_symbol AS ss,
       e.target_file_id AS tf, e.target_symbol AS ts,
       e.candidate_total AS ct, e.resolution AS res
  FROM generation_edges e
 WHERE e.generation_id = (SELECT id FROM _gen)
   AND e.edge_kind = 'Calls'
   AND e.confidence = 0.2;

-- Primary derivation: the callee name is the `name` of the node the edge
-- actually points at. DISTINCT because SC14 leaves duplicate identities in the
-- tree; duplicates share a qualified name and therefore also a `name`, so
-- collapsing them cannot change the answer.
CREATE TEMP TABLE _nm AS
SELECT DISTINCT file_id, qualified_name, name
  FROM generation_nodes
 WHERE generation_id = (SELECT id FROM _gen);

CREATE INDEX temp._nm_idx ON _nm(file_id, qualified_name);

CREATE TEMP TABLE _joined AS
SELECT f.sf AS sf, f.ss AS ss, nm.name AS callee, f.ct AS ct, f.res AS res
  FROM _fanout f
  LEFT JOIN _nm nm ON nm.file_id = f.tf AND nm.qualified_name = f.ts;

-- `MAX(ct)` rather than `SUM(ct)`: every edge in a fan-out group carries the
-- same candidate total, so this reads the group's one value. `MAX` also states
-- the invariant — if the rows of a group ever disagreed, the largest is the
-- fail-*closed* choice for a denominator that must not be understated.
CREATE TEMP TABLE _grp AS
SELECT sf, ss, callee, COUNT(*) AS n, MAX(ct) AS candidates
  FROM _joined GROUP BY 1, 2, 3;

-- Independent derivation: a qualified name is scope + separator + name, so the
-- text after the last '.'/'::' is the callee name by construction. This shares
-- no code path with the join above — it never reads generation_nodes — so
-- agreement between the two cross-validates the grouping key rather than
-- asserting the same assumption twice.
CREATE TEMP TABLE _alt_grp AS
WITH RECURSIVE tail(sf, ss, rest) AS (
  SELECT sf, ss, replace(ts, '::', '.') FROM _fanout
  UNION ALL
  SELECT sf, ss, substr(rest, instr(rest, '.') + 1) FROM tail WHERE instr(rest, '.') > 0
)
SELECT sf, ss, rest AS callee, COUNT(*) AS n
  FROM tail WHERE instr(rest, '.') = 0 GROUP BY 1, 2, 3;

-- @@RESULT@@
SELECT (SELECT COUNT(*) FROM generation_files WHERE generation_id = (SELECT id FROM _gen))
       || '|' || (SELECT COALESCE(SUM(n), 0) FROM _grp)
       || '|' || (SELECT COALESCE(SUM(n * n), 0) FROM _grp)
       || '|' || (SELECT COUNT(*) FROM _grp)
       || '|' || (SELECT COALESCE(MAX(n), 0) FROM _grp)
       || '|' || (SELECT COUNT(*) FROM _joined WHERE callee IS NULL)
       || '|' || (SELECT COALESCE(SUM(n * n), 0) FROM _alt_grp)
       || '|' || (SELECT COUNT(*) FROM _alt_grp)
       || '|' || (SELECT COALESCE(SUM(candidates), 0) FROM _grp)
       || '|' || (SELECT COALESCE(SUM(candidates * candidates), 0) FROM _grp)
       || '|' || (SELECT COALESCE(MAX(candidates), 0) FROM _grp)
       || '|' || (SELECT COUNT(*) FROM _joined
                   WHERE ct IS NULL AND res IS NOT DISTINCT FROM 'AmbiguousGlobal');

<!--
  RECOVERED SOURCE — read this before editing.

  AUDIT.html is hand-authored and remains authoritative. This file was extracted
  back out of it with `pandoc --from=html --to=gfm` on 2026-09-02 because the
  markdown source was not in the tree: 84 audit findings existed only as rendered
  HTML, so they could not be grepped, diffed, reviewed in a pull request, or read
  by any tool that reads the rest of this repository's documentation.

  This is a READING COPY, not a build source. Editing it does NOT change
  AUDIT.html, and AUDIT.html is deliberately not regenerated from it: the page
  carries a hand-built masthead and severity chips (1 critical, 21 high,
  40 medium, 22 low) that markdown cannot express, and a machine round-trip would
  silently delete them. Losing a document's presentation to close a bookkeeping
  gap is not a fix.

  If the two disagree, AUDIT.html wins. See AGENT_PLAN.md, DOC-2.
-->

<div class="wrap">

<div class="header masthead">

DevCouncil · rust-port · audit II · 2026-08-11

# dev map <span class="arrow">→</span> Rust: Audit II

A second, deeper audit of the code-intelligence subsystem — **84 new findings** beyond the 31 in [PLAN](PLAN.html) — with the amended rewrite plan and implementation guide. Informed by a study of [GitNexus](https://github.com/abhigyanpatwari/GitNexus) and [CodeGraph](https://github.com/colbymchenry/codegraph).

<div class="chips">

<span class="chip crit">1 critical</span> <span class="chip high">21 high</span> <span class="chip med">40 medium</span> <span class="chip low">22 low</span> <span class="chip">all fixes land in the Rust port</span> <span class="chip ok">every finding → acceptance test</span>

</div>

</div>

[<span class="n">§1</span>Executive summary](#summary) [<span class="n">§2</span>Findings: graph core](#sec-G) [<span class="n">§3</span>Findings: store & sync](#sec-S) [<span class="n">§4</span>Findings: surfaces & viz](#sec-V) [<span class="n">§5</span>Findings: extraction](#sec-X) [<span class="n">§6</span>Reference-repo adoptions](#adoptions) [<span class="n">§7</span>Amended plan](#plan) [<span class="n">§8</span>Implementation guide](#guide) [<span class="n">§9</span>Acceptance additions](#accept)

<div id="summary" class="section">

## §1 — Executive summary

<div class="prose">

**Audit basis:** same tree as PLAN (DevCouncil @ `98f23e3`-era working copy, 769 source files). Method: four parallel deep-read audits over all 26,837 in-scope LOC — graph core, store/sync, agent-facing surfaces, extraction/integration — each primed with the failure catalogues that GitNexus and CodeGraph fixed in production (traversal identity bugs, impact semantics, cache poisoning, FTS hygiene, provenance). Every finding was verified against exact source lines; the highest-severity claims were independently re-verified.

The 31 findings in PLAN §3 described a system that is *slow, oversized and stale*. The 84 findings here describe something sharper: **places where the map is confidently wrong**. Four themes dominate:

**1 · The analysis layer has structural correctness holes.** The PDG's control-flow graph never connects loop headers to their bodies ([G1](#G1)) and drops condition lines from every block ([G2](#G2)) — reaching-definitions and taint run on a disconnected graph. Resolution silently downgrades or misdirects edges: a stdlib-named method loses all inbound edges ([G3](#G3)), an ambiguous fan-out permanently shadows a proven call ([G5](#G5)), heuristic picks ship stamped EXTRACTED ([G6](#G6)), and a Python class can "inherit" a TypeScript interface ([G7](#G7)).

**2 · Extraction has silent blind spots that feed false dead-code.** TS enums/namespaces/declare files extract zero symbols ([X1](#X1)), `new Foo()` is never a reference ([X2](#X2)), generic Rust impls vanish ([G19](#G19)), and there is *no parse-failure signal anywhere* ([X6](#X6)) — a wedged parser is cached forever as "no symbols" ([X7](#X7)).

**3 · The ops layer can destroy work.** On Windows, checking whether a build is alive kills it ([S1](#S1)). A crash mid-migration silently empties the store ([S2](#S2)). `dev map unlock` judges "stuck" by wall-clock age and kills healthy builds — and the failure mode of a crashed sync actively steers users into running it against their own MCP server ([S4](#S4)/[S5](#S5)).

**4 · The surfaces overstate their own authority.** `--precise` impact replaces real dependents with an authoritative-looking empty list for Go/Rust/Swift/Kotlin ([X14](#X14)), watch-mode serves stale blast radius as fresh ([V4](#V4)), truncation counts lie ([V12](#V12)), capped badges present as totals ([V13](#V13)), and the generated graph.html is a 15.5 MB page with an XSS hole in its tooltip ([V1](#V1)/[V2](#V2)).

Every finding below carries a disposition — the rewrite phase and crate that absorbs it, per PLAN §5's structure. By explicit project direction, **no interim Python patches**: the Python implementation is frozen as the parity baseline, and every fix lands in the Rust port itself, encoded as an acceptance test so it cannot regress.

</div>

<div class="callout warn">

**Duplicate consolidation.** Two agent-reported findings merged: G12 (liveness ratio meta discarded) is absorbed into [V5](#V5); X10 (generic Rust impls) is the same defect as [G19](#G19). IDs are preserved; neither is double-counted in the totals.

</div>

</div>

<div id="sec-G" class="section">

<div class="grouphead">

## G — Graph core — resolution, liveness, PDG, intel

<span class="groupcount">25 findings · 4 high · 14 medium · 7 low</span>

</div>

<div class="cards">

<span class="fid">G1</span><span class="ftitle">CFG builder wires branch/loop/try bodies by their end block — or not at all</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/pdg/cfg.py:117–158</span>

</div>

<div class="klabel">

what

</div>

<div>

\_build_stmts returns only the last block of a sub-body, so \`if\` tests connect to the then-branch's *end*, loops get no header→body edge, and try/handler heads are never connected. Reaching-defs and taint run on a largely disconnected CFG.

</div>

<div class="klabel">

fix

</div>

<div>

Return (entry, exit, pending_exits) from \_build_stmts; wire test→then_entry, header→body_entry, try→handler_entry.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-analyze

</div>

</div>

</div>

<span class="fid">G19</span><span class="ftitle">Rust methods in generic impl blocks are dropped entirely (also found as X10)</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_ts.py:992–1024</span>

</div>

<div class="klabel">

what

</div>

<div>

impl\<T\> Foo\<T\> puts the type under generic_type, so type_name stays empty and the whole method walk is skipped — every method on a generic type missing from the graph; ubiquitous in real Rust.

</div>

<div class="klabel">

fix

</div>

<div>

Descend into generic_type for its type_identifier (both impl and trait side).

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

</div>

</div>

<span class="fid">G2</span><span class="ftitle">Leader-statement lines excluded from every basic block — condition uses invisible to def-use</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/pdg/cfg.py:117–147 · reaching_def.py:79</span>

</div>

<div class="klabel">

what

</div>

<div>

When a statement group precedes an if/while/for, the leader's line lands in no block's \`lines\`, so variables read in conditions (and \`for\` target definitions) generate no gen/use entries. Return/Raise lines equally absent.

</div>

<div class="klabel">

fix

</div>

<div>

Append the leader statement's line span to the block that terminates in it.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-analyze

</div>

</div>

</div>

<span class="fid">G3</span><span class="ftitle">stdlib-name guard \`continue\`s past the ambiguous fan-out, dropping found candidates</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/resolve.py:904–907</span>

</div>

<div class="klabel">

what

</div>

<div>

Methods named after stdlib modules (copy, json, stat, time…) lose all inbound ambiguous edges → dead-code false positives. The guard also fires for JS/Go/Rust though stdlib_module_names is Python-only.

</div>

<div class="klabel">

fix

</div>

<div>

Scope the check to the unique-global rung only, gated on \_lang_family == 'py'.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-resolve

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus tier precedence

</div>

</div>

</div>

<span class="fid">G10</span><span class="ftitle">PDG query reloads the whole shard table per candidate path; store errors swallowed</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/query.py:185–214</span>

</div>

<div class="klabel">

what

</div>

<div>

store.analysis_shards() decodes every shard payload of the generation, called inside a loop over substring-matched paths; \`except Exception: pass\` turns store corruption into 'no PDG for target'.

</div>

<div class="klabel">

fix

</div>

<div>

Fetch shards once (or add a point lookup); log instead of pass.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph batch-vs-N+1

</div>

</div>

</div>

<span class="fid">G11</span><span class="ftitle">Cypher shim advertises relationship types that can never match stored kinds</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/cypher.py:9–71</span>

</div>

<div class="klabel">

what

</div>

<div>

EXTENDS never matches (stored kind is \`inherits\`), REFERENCES matches nothing — both return ok:true with 0 rows; INHERITS/OVERRIDES are rejected as unsupported though the edges exist; node labels are parsed then ignored.

</div>

<div class="klabel">

fix

</div>

<div>

Alias map EXTENDS→inherits; align \_SUPPORTED_REL with schema kinds; honor or reject labels.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · retire with N10 or implement real queries

</div>

</div>

</div>

<span class="fid">G13</span><span class="ftitle">Reaching-defs drops loop-carried dependencies via def_line \<= use_line</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/pdg/reaching_def.py:113–114</span>

</div>

<div class="klabel">

what

</div>

<div>

A later-in-body definition legitimately reaches an earlier use on the next iteration; the line-order filter deletes exactly those edges, blinding taint to accumulation patterns.

</div>

<div class="klabel">

fix

</div>

<div>

Apply line ordering only when def and use share a block.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-analyze

</div>

</div>

</div>

<span class="fid">G14</span><span class="ftitle">Bare decorators recorded at the decorated node's line — caller misattributed to the decorated function</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_python.py:288–304</span>

</div>

<div class="klabel">

what

</div>

<div>

\`@lru_cache def foo()\` yields a foo→lru_cache call edge; decorators execute at import time, so dead decorated functions drag their decorators toward dead and invert blast-radius direction.

</div>

<div class="klabel">

fix

</div>

<div>

Use dec.lineno so attribution lands on module/class scope.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph caller semantics

</div>

</div>

</div>

<span class="fid">G15</span><span class="ftitle">\_python_return_keys: tautological name check; line=0 handlers absorb every function's keys</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/api_routes.py:287–291</span>

</div>

<div class="klabel">

what

</div>

<div>

\`node.name not in {fn.name}\` duplicates the != check; regex-fallback handlers (line=0) skip the span guard and aggregate return keys from every function in the file, corrupting shape_check.

</div>

<div class="klabel">

fix

</div>

<div>

Combine the guards: skip unless the node's line falls in fn's span.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

</div>

</div>

<span class="fid">G18</span><span class="ftitle">\`import \* as ns\` namespace imports never reach alias_map</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_ts.py:366–383</span>

</div>

<div class="klabel">

what

</div>

<div>

The identifier sits under namespace_import, matched by neither branch — ns.helper() can't resolve through the import-scoped rung and falls to lossier global/ambiguous rungs.

</div>

<div class="klabel">

fix

</div>

<div>

Handle namespace_import; map ns → module spec.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus tier precedence

</div>

</div>

</div>

<span class="fid">G20</span><span class="ftitle">Go same-package co-membership emits a complete O(n²) digraph</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/resolve.py:252–259</span>

</div>

<div class="klabel">

what

</div>

<div>

A 200-file Go package produces 39,800 EXTRACTED import edges before any real import — inflating fan-in, community density, blast radius, and export size quadratically.

</div>

<div class="klabel">

fix

</div>

<div>

Model membership as a star through a synthetic package node.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-resolve

</div>

</div>

</div>

<span class="fid">G24</span><span class="ftitle">query_symbol ignores edge confidence — ambiguous fan-out floods 50-slot caller lists</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/query.py:50–111</span>

</div>

<div class="klabel">

what

</div>

<div>

intel.py deliberately excludes ambiguous edges from ranked surfaces; query_symbol doesn't, so name collisions flood callers with fan-out noise and symbol_has_non_test_inbound accepts fan-out as proof of wiring.

</div>

<div class="klabel">

fix

</div>

<div>

Carry per-edge confidence; sort extracted\>inferred\>ambiguous before the cap; add min_confidence.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus confidence · CodeGraph truncation priority

</div>

</div>

</div>

<span class="fid">G4</span><span class="ftitle">Re-export protection picks an arbitrary file via set iteration; unique-file fallback is dead code</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/liveness.py:571–579</span>

</div>

<div class="klabel">

what

</div>

<div>

\`break\` on first hit over a Set makes protection hash-order-dependent (PYTHONHASHSEED flips dead_code output); the for/else fallback can only see an empty list. Same bug class was already fixed in resolve.py:614.

</div>

<div class="klabel">

fix

</div>

<div>

Sort candidates; protect all hits (conservative) or only a unique one.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-resolve

</div>

<div class="klabel">

borrow

</div>

<div>

determinism

</div>

</div>

</div>

<span class="fid">G5</span><span class="ftitle">Call-edge dedup on (source,target) is first-wins — ambiguous fan-out shadows a later extracted edge</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/resolve.py:949–989</span>

</div>

<div class="klabel">

what

</div>

<div>

If an ambiguous fan-out reaches T before a proven call to T resolves, the EXTRACTED edge is dropped. Liveness, PageRank and blast radius all exclude ambiguous edges, so the proven call vanishes from every ranked surface.

</div>

<div class="klabel">

fix

</div>

<div>

Dedup as (source,target)→best_confidence and upgrade in place.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-resolve

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph edge identity

</div>

</div>

</div>

<span class="fid">G6</span><span class="ftitle">named_import_edges: arbitrary multi-candidate pick stamped EXTRACTED; fallback drops the module constraint</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/resolve.py:632–655</span>

</div>

<div class="klabel">

what

</div>

<div>

With no module-hint match, \`from x import y\` binds to *any* imported file defining y; multi-candidate picks take matches\[0\] yet are labeled EXTRACTED with no candidates extras.

</div>

<div class="klabel">

fix

</div>

<div>

Demote multi-candidate picks to AMBIGUOUS with capped candidate extras; downgrade the unscoped fallback to INFERRED.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-resolve

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus confidence/provenance

</div>

</div>

</div>

<span class="fid">G7</span><span class="ftitle">Inheritance/overrides: no language-family guard; overrides fan out to every same-named class repo-wide</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/resolve.py:430–511</span>

</div>

<div class="klabel">

what

</div>

<div>

A Python class can 'inherit' a TS interface; any method on a class extending any \`Component\` gets overrides edges to every Component.render in the repo. Liveness propagates through overrides, suppressing real dead-method findings.

</div>

<div class="klabel">

fix

</div>

<div>

Filter candidates by \_lang_family and import scope; fan out or emit nothing on ambiguity instead of candidates\[0\].

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-resolve

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus MRO handling

</div>

</div>

</div>

<span class="fid">G8</span><span class="ftitle">blast_radius raises KeyError for any max_depth \> 3</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/intel.py:557–580</span>

</div>

<div class="klabel">

what

</div>

<div>

by_depth and confidence_for_depth are hard-coded to {1,2,3} while max_depth is a public parameter; the sibling SQLite implementation supports depth 8, so the two impact surfaces already disagree.

</div>

<div class="klabel">

fix

</div>

<div>

Build by_depth from range(1, max_depth+1); .get(d, 'ambiguous') for labels.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

</div>

</div>

<span class="fid">G9</span><span class="ftitle">api_impact runs the full-repo fetch-site scan twice per query</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/api_routes.py:476–498</span>

</div>

<div class="klabel">

what

</div>

<div>

shape_check internally re-runs route_map; each run reads and regex-scans every file in the graph and re-parses Python handlers. One api_impact call = 2× full-repo I/O.

</div>

<div class="klabel">

fix

</div>

<div>

Compute route_map once and pass it through; memoize \_scan_fetch_sites per (graph, mtime).

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph batch-vs-N+1

</div>

</div>

</div>

<span class="fid">G16</span><span class="ftitle">\`name is m.group(2)\` — string identity comparison is effectively always False</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_ts.py:234–240</span>

</div>

<div class="klabel">

what

</div>

<div>

m.group(2) returns a fresh object per call; the member-form JSX branch never fires, so lowercase member tags () drop from references.

</div>

<div class="klabel">

fix

</div>

<div>

Capture the group once and compare with == (or by index).

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

</div>

</div>

<span class="fid">G17</span><span class="ftitle">file_doc_path lstrip('./') strips leading dots from dot-directories</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/export_links.py:35–36</span>

</div>

<div class="klabel">

what

</div>

<div>

.storybook/main.js → storybook/main.js; doc paths misrepresent sources and can collide.

</div>

<div class="klabel">

fix

</div>

<div>

Strip an exact './' prefix, not a character set.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

</div>

</div>

<span class="fid">G21</span><span class="ftitle">extract_processes BFS marks visited at dequeue — duplicate enqueues</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/intel.py:493–503</span>

</div>

<div class="klabel">

what

</div>

<div>

Two parents can enqueue the same child; queue bloats on dense call graphs. The benign variant of the pattern trace_path gets right.

</div>

<div class="klabel">

fix

</div>

<div>

Use an enqueued set checked before append.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-analyze

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph \#1090 class

</div>

</div>

</div>

<span class="fid">G22</span><span class="ftitle">OKF export neighbor-area computation is a nested full scan</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/export.py:192–197</span>

</div>

<div class="klabel">

what

</div>

<div>

For each import target it scans every area's full file list — O(area_files × imports × total_files).

</div>

<div class="klabel">

fix

</div>

<div>

Precompute a path→area dict.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

</div>

</div>

<span class="fid">G23</span><span class="ftitle">Taint: parameter sources filtered out by the source gate; sink filter has a no-op guard</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/pdg/taint.py:84–110</span>

</div>

<div class="klabel">

what

</div>

<div>

Function parameters — the canonical taint source — only survive if literally named like a pattern; \`for p in pats if pats\` filters on the tuple; substring sink matching flags open_file/reopen as path-traversal sinks.

</div>

<div class="klabel">

fix

</div>

<div>

Treat parameters as low-confidence sources; exact/dotted-suffix sink matching.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-analyze

</div>

</div>

</div>

<span class="fid">G25</span><span class="ftitle">build_file_and_symbol_nodes builds a name_index it never returns</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/resolve.py:303–359</span>

</div>

<div class="klabel">

what

</div>

<div>

Populated for every symbol on every build, then discarded; each consumer rebuilds its own variant.

</div>

<div class="klabel">

fix

</div>

<div>

Delete it, or return and reuse it.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-resolve

</div>

</div>

</div>

<span class="fid">G26</span><span class="ftitle">trace_path returns a hash-order-dependent shortest path</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/query.py:141–164</span>

</div>

<div class="klabel">

what

</div>

<div>

Neighbors iterate in set order; among equal-length paths the reported one varies across processes.

</div>

<div class="klabel">

fix

</div>

<div>

Iterate sorted(adj\[cur\]).

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · determinism

</div>

</div>

</div>

</div>

</div>

<div id="sec-S" class="section">

<div class="grouphead">

## S — Store, sync & concurrency

<span class="groupcount">20 findings · 1 critical · 6 high · 7 medium · 6 low</span>

</div>

<div class="cards">

<span class="fid">S1</span><span class="ftitle">\_pid_alive kills the probed process on Windows</span><span class="sev critical">critical</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/build_control.py:115–122</span>

</div>

<div class="klabel">

what

</div>

<div>

os.kill(pid, 0) on Windows calls TerminateProcess unconditionally — every liveness probe (dev map status, unlock, coordinator.status) terminates a healthy in-flight worker, then reports it 'alive'.

</div>

<div class="klabel">

fix

</div>

<div>

On nt, probe via OpenProcess/GetExitCodeProcess (or psutil.pid_exists).

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · daemon replaces PID probing

</div>

</div>

</div>

<span class="fid">S2</span><span class="ftitle">Schema migrations non-atomic: crash mid-migration bricks or silently empties the store</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:343–575</span>

</div>

<div class="klabel">

what

</div>

<div>

executescript autocommits per statement. Crash in v0 creation → partially-created schema at user_version=0, permanent 'table exists' error with no corruption marker. Crash between the v1→v2 script and payload copy → user_version=2 committed with empty node tables: silent total data loss.

</div>

<div class="klabel">

fix

</div>

<div>

One explicit transaction per migration step; set user_version as the last statement of the same transaction as the data copy.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-store

</div>

</div>

</div>

<span class="fid">S3</span><span class="ftitle">FTS5 MATCH gets the raw user query — crashes on common chars, silently degrades to full decompress-scan</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:1458–1471</span>

</div>

<div class="klabel">

what

</div>

<div>

\`build-control\`, quotes, parens or name: prefixes raise OperationalError (or silently change semantics as column filters); the except branch falls back to iterating every membership row with zlib.decompress + json.loads per row — on exactly the queries users type most.

</div>

<div class="klabel">

fix

</div>

<div>

Quote each term: '"' + term.replace('"', '""') + '"' (optionally + \* for prefix).

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-store

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph FTS hygiene

</div>

</div>

</div>

<span class="fid">S4</span><span class="ftitle">dev map unlock kills healthy long builds; can kill a recycled PID's whole process group</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/build_control.py:284–331, 399–436</span>

</div>

<div class="klabel">

what

</div>

<div>

'Stuck' is judged from lease age vs the 90s progress budget — any build merely running \>90s qualifies while state=building with fresh progress. No PID-identity check before killpg; laptop suspend inflates wall-clock age.

</div>

<div class="klabel">

fix

</div>

<div>

Gate on last_progress_at/last_cpu_progress_at; verify process identity before signaling; avoid killpg for inline holders.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · daemon obsoletes unlock heuristics

</div>

</div>

</div>

<span class="fid">S5</span><span class="ftitle">Crashed incremental sync leaves build_status='building' with a live server PID — the advertised remedy then kills the server</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/sync/incremental.py:232–432</span>

</div>

<div class="klabel">

what

</div>

<div>

No try/finally around the sync body: any exception leaves the status file claiming an active build owned by the MCP/watcher process; after 90s it reads 'stalled' with hint 'dev map unlock' — which SIGKILLs the healthy server via S4.

</div>

<div class="klabel">

fix

</div>

<div>

try/except that writes state='failed' with the reason before re-raising.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · daemon owns build status

</div>

</div>

</div>

<span class="fid">S6</span><span class="ftitle">One reconcile exception permanently kills the sync worker thread</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/sync/coordinator.py:303–312</span>

</div>

<div class="klabel">

what

</div>

<div>

The periodic reconcile() is un-wrapped; a transient 'database is locked' unwinds \_run, the daemon thread dies silently, and pending paths queue forever with status showing 'pending'.

</div>

<div class="klabel">

fix

</div>

<div>

Wrap the loop body; record degraded and continue.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-serve

</div>

</div>

</div>

<span class="fid">S8</span><span class="ftitle">ParserWorkerPool cannot terminate a wedged child — 100%-CPU orphans and a possible atexit hang</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/languages/workers.py:227–260</span>

</div>

<div class="klabel">

what

</div>

<div>

shutdown(wait=False, cancel_futures=True) can't cancel a running native parse; the child's only exit path triggers when the parent dies. atexit close(wait=True) on a wedged pool = mutual wait. Every per-file exception also discards and respawns the whole spawn-based pool.

</div>

<div class="klabel">

fix

</div>

<div>

Track child PIDs and SIGKILL on restart/close; distinguish BrokenProcessPool/Timeout from task errors.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · in-process grammars obsolete the pool

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus worker quarantine

</div>

</div>

</div>

<span class="fid">S10</span><span class="ftitle">In-process writer contention blocks forever; timeout only bounds the cross-process flock</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/build_control.py:478–496 · sync/coordinator.py:235</span>

</div>

<div class="klabel">

what

</div>

<div>

graph_build_session takes the per-root threading.RLock with no timeout before consulting \`timeout\`; a second thread hangs instead of raising GraphBuildBusy.

</div>

<div class="klabel">

fix

</div>

<div>

lock.acquire(timeout=wait) → raise GraphBuildBusy on failure.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · daemon obsoletes

</div>

</div>

</div>

<span class="fid">S11</span><span class="ftitle">add_runtime_observations MAX(ordinal) read-then-insert race</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:1745–1770</span>

</div>

<div class="klabel">

what

</div>

<div>

SELECT runs in autocommit; the transaction begins at INSERT. Concurrent ingests read the same MAX, collide on the PK, and one whole batch is lost.

</div>

<div class="klabel">

fix

</div>

<div>

BEGIN IMMEDIATE before the read, or derive ordinal at read time.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-store

</div>

</div>

</div>

<span class="fid">S12</span><span class="ftitle">Watcher hot path: a git check-ignore subprocess and a fresh SQLite connection per filesystem event</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/sync/coordinator.py:186–199 · sync/scope.py:84–98</span>

</div>

<div class="klabel">

what

</div>

<div>

A checkout touching thousands of files serializes thousands of subprocess spawns on the watchdog dispatcher thread.

</div>

<div class="klabel">

fix

</div>

<div>

Memoize verdicts (invalidate on .gitignore mtime), batch via --stdin, hold an in-memory indexed-path set.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-serve

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph in-process ignore matching

</div>

</div>

</div>

<span class="fid">S13</span><span class="ftitle">put_extraction runs the migration ladder + index-ensure executescript on every call</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:296–300, 563–575, 1661</span>

</div>

<div class="klabel">

what

</div>

<div>

During a cold build this fires once per extracted file — thousands of schema-lock acquisitions and commits per build.

</div>

<div class="klabel">

fix

</div>

<div>

Memoize initialization per store instance.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-store

</div>

</div>

</div>

<span class="fid">S14</span><span class="ftitle">Python extraction cache key has no interpreter-version salt</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/cache.py:307–308</span>

</div>

<div class="klabel">

what

</div>

<div>

grammar_version is the constant 'stdlib-ast'; after upgrading Python the cache serves extractions parsed by the old interpreter until content changes.

</div>

<div class="klabel">

fix

</div>

<div>

Salt with sys.version_info major.minor.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · producer-identity cache keys

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus producer-identity keys

</div>

</div>

</div>

<span class="fid">S7</span><span class="ftitle">Failed sync batch hot-loops forever — no backoff, no poison-file quarantine</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/sync/coordinator.py:270–312</span>

</div>

<div class="klabel">

what

</div>

<div>

The comment promises backoff; none exists. A poison file re-runs full extraction+enrichment ~once per second indefinitely.

</div>

<div class="klabel">

fix

</div>

<div>

Exponential backoff on consecutive failures; quarantine offenders into a surfaced degraded list.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-serve

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph watcher design

</div>

</div>

</div>

<span class="fid">S9</span><span class="ftitle">Read-only SQLite URI built by f-string without percent-encoding</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:305</span>

</div>

<div class="klabel">

what

</div>

<div>

Paths containing ? \# % break the URI: readers open the wrong file while writers (plain path) work — an asymmetry that looks like a corrupt index only for readers.

</div>

<div class="klabel">

fix

</div>

<div>

urllib.parse.quote the path before formatting.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-store

</div>

</div>

</div>

<span class="fid">S15</span><span class="ftitle">Windows lease size-probe is a no-op; guard byte appended every acquire; holder metadata unreadable while locked</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/sync/lease.py:28–73</span>

</div>

<div class="klabel">

what

</div>

<div>

tell() after seek(0) is always 0; 'a+b' appends a NUL per acquire; msvcrt mandatory locking makes read_holder fail exactly when someone holds the lease.

</div>

<div class="klabel">

fix

</div>

<div>

seek(0, SEEK_END) for the probe; keep metadata outside the locked byte range.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-serve

</div>

</div>

</div>

<span class="fid">S16</span><span class="ftitle">Rename aliases evaporate after one generation</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:1240–1298, 1641–1650</span>

</div>

<div class="klabel">

what

</div>

<div>

aliases() reads only the current generation; a rename mapping is queryable exactly until the next unrelated commit.

</div>

<div class="klabel">

fix

</div>

<div>

Query across retained generations and chain old→new transitively.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-store

</div>

</div>

</div>

<span class="fid">S17</span><span class="ftitle">Racy-stat window: content hash recorded against a newer mtime — file becomes permanently invisible to reconcile</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:1004–1017 · sync/coordinator.py:210–214</span>

</div>

<div class="klabel">

what

</div>

<div>

read_bytes() then stat(): a write between the two stores the new mtime with the old hash; reconcile compares (size, mtime) only, so the stale extraction never heals.

</div>

<div class="klabel">

fix

</div>

<div>

Stat before and after the read, retry on mismatch; hash-verify racily-clean entries.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-serve

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph freshness layer 3

</div>

</div>

</div>

<span class="fid">S18</span><span class="ftitle">WAL checkpoint failure handling is dead code; busy checkpoints silently ignored</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:951–962</span>

</div>

<div class="klabel">

what

</div>

<div>

PRAGMA wal_checkpoint doesn't raise on contention — it returns (busy, log, checkpointed); the PASSIVE fallback is unreachable and a busy TRUNCATE is dropped.

</div>

<div class="klabel">

fix

</div>

<div>

Read the result row; fall back explicitly when busy=1.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-store

</div>

</div>

</div>

<span class="fid">S19</span><span class="ftitle">get_sync_coordinator kwargs check false-positives on clamped values and callables</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/sync/coordinator.py:107, 414–421</span>

</div>

<div class="klabel">

what

</div>

<div>

The singleton check compares the caller's raw kwarg to the clamped attribute — two identical calls with debounce=0.05 raise ValueError; fresh lambdas fail identity comparison.

</div>

<div class="klabel">

fix

</div>

<div>

Normalize before comparing; compare callbacks by explicit override flag.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-serve

</div>

</div>

</div>

<span class="fid">S20</span><span class="ftitle">interrupt_writes tracks only the last-opened write connection</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/store/sqlite.py:239–330</span>

</div>

<div class="klabel">

what

</div>

<div>

A short write from another thread steals the single \_active_write_conn slot; the watchdog then interrupts the wrong statement and escalates straight to SIGKILL of the process group.

</div>

<div class="klabel">

fix

</div>

<div>

Track a set of active write connections; interrupt all.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · devmap-store

</div>

</div>

</div>

</div>

</div>

<div id="sec-V" class="section">

<div class="grouphead">

## V — Surfaces — repo map, visualizers, CLI, MCP

<span class="groupcount">20 findings · 3 high · 10 medium · 7 low</span>

</div>

<div class="cards">

<span class="fid">V1</span><span class="ftitle">XSS: node labels rendered as raw HTML via the force-graph tooltip</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/viz.py:174, 804 · map_viz.py:349</span>

</div>

<div class="klabel">

what

</div>

<div>

The vendored force-graph renders nodeLabel via d3 .html(); a hostile file or symbol name (legal on APFS/ext4) executes on hover in graph.html/map.html. The JSON embed is escaped — this is the one sink that re-opens it.

</div>

<div class="klabel">

fix

</div>

<div>

escapeHtml inside every nodeLabel accessor; escape the intel panel numerics too.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · viz

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus canvas labels are never HTML

</div>

</div>

</div>

<span class="fid">V2</span><span class="ftitle">Whole graph inlined into graph.html — 15.5 MB, with large parts embedded twice</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/viz.py:457–535</span>

</div>

<div class="klabel">

what

</div>

<div>

Both file and symbol payloads always embedded, the active one duplicated again as flat back-compat fields, plus a neighbors index re-serializing every call edge twice. Symbol mode hands force-graph 10,895 nodes with no degradation tiers or confirm gate.

</div>

<div class="klabel">

fix

</div>

<div>

Embed only the active mode (JSON script tags loaded on demand); derive neighbors client-side; add size tiers + a load-confirm gate.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · viz

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus tiers + fail-safe gate

</div>

</div>

</div>

<span class="fid">V4</span><span class="ftitle">Incremental sync leaves \`dependents\` (blast radius) stale in repo_map.json</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/map_refresh.py:109–177</span>

</div>

<div class="klabel">

what

</div>

<div>

refresh_repo_map_from_graph updates files/liveness but never dependents, dependents_total, important_files or processes — under watch, devcouncil_impact serves a fresh-looking map whose reverse-import index predates the edits.

</div>

<div class="klabel">

fix

</div>

<div>

Rebuild dependents from the committed graph's import edges, or stamp dependents_stale for the impact handler to surface.

</div>

<div class="klabel">

lands in

</div>

<div>

P4+P5 · daemon freshness

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph staleness banner

</div>

</div>

</div>

<span class="fid">V11</span><span class="ftitle">MCP map handlers re-parse the map twice and re-fingerprint the repo on every call</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">integrations/mcp/handlers/map.py:44–51, 344, 432 · subsystem_map.py:36–41</span>

</div>

<div class="klabel">

what

</div>

<div>

handle_liveness parses the multi-MB map twice per call; \_map_stale runs git rev-parse + ls-files + a full content fingerprint per tool call; area filtering falls back to a linear scan per candidate (~10^8 comparisons at scale).

</div>

<div class="klabel">

fix

</div>

<div>

Thread one loaded map through; memoize staleness briefly; build a path→area dict per request.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · daemon obsoletes

</div>

</div>

</div>

<span class="fid">V12</span><span class="ftitle">dead_code_hidden undercounts: the \[:200\] truncation is silent</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">integrations/mcp/handlers/map.py:433–442</span>

</div>

<div class="klabel">

what

</div>

<div>

hidden counts only confidence-filtered entries; the final slice drops the rest uncounted — a response can claim hidden:0 while omitting hundreds. Liveness lists ship with no cap at all (up to 20,000 entries each).

</div>

<div class="klabel">

fix

</div>

<div>

hidden += len(matched)-200; cap liveness responses with the map's own {shown,total} shape.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

</div>

</div>

<span class="fid">V14</span><span class="ftitle">HTML artifacts: non-atomic writes, no freshness stamp, unconditional 15 MB rewrites</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/viz.py:1049 · map_viz.py:429 · cli/commands/map.py:313–319</span>

</div>

<div class="klabel">

what

</div>

<div>

Unlike every JSON artifact, the HTML writers skip atomic_write_text (truncated file on interrupt); neither page shows when/what it was built from; the 15.5 MB page regenerates even on mode='reused' runs.

</div>

<div class="klabel">

fix

</div>

<div>

Route through atomic_write_text; render generated_head + built-at; skip when the fingerprint matches.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · viz

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph staleness banner

</div>

</div>

</div>

<span class="fid">V18</span><span class="ftitle">Source-root detection is all-or-nothing: one stray file collapses subsystem inference</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/repo_mapper.py:1037–1068, 2845</span>

</div>

<div class="klabel">

what

</div>

<div>

Strict longest-common-prefix: a single tools/gen.py makes source_root='' and the whole src tree one subsystem. Any repo vendoring a DevCouncil copy inherits the hand-written DevCouncil summaries wholesale.

</div>

<div class="klabel">

fix

</div>

<div>

Majority root (≥80% coverage); gate the curated index on project name, not path prefix.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · subsystem inference

</div>

</div>

</div>

<span class="fid">V3</span><span class="ftitle">Symbol-mode edge filtering inconsistent: sizes counted from hidden edges; rendered kinds missing from filters and legend</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/viz.py:373–431, 608</span>

</div>

<div class="klabel">

what

</div>

<div>

contains/dynamic_reference/reflects_to inflate node degree but aren't rendered; dynamic_reference, reflects_to, registers, routes_to render but are absent from the kind dropdown and legend, all colored the same gray.

</div>

<div class="klabel">

fix

</div>

<div>

Compute degree from rendered kinds only; enumerate dropdown/legend from the payload; distinct colors per kind.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · viz

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus per-relationship colors

</div>

</div>

</div>

<span class="fid">V5</span><span class="ftitle">Entry-root and unreachable diagnostics computed, then thrown away (absorbs G12)</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/wiring.py:1379 · graph/liveness.py:233–244 · build.py:405</span>

</div>

<div class="klabel">

what

</div>

<div>

DiscoveryReport (per-source entry-root yields), unreachable_unreliable_reason, and reachability_gap_points (the exact reconnect hints) are all discarded; only a bare bool reaches graph.meta. CLAUDE.md tells agents to distrust unreachable_files with no way to see why.

</div>

<div class="klabel">

fix

</div>

<div>

Persist the report + ratio meta (capped) into graph.meta/liveness_meta; print the per-source yield table when roots are empty.

</div>

<div class="klabel">

lands in

</div>

<div>

P3+P5 · surface diagnostics

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus provenance

</div>

</div>

</div>

<span class="fid">V6</span><span class="ftitle">Entry-root discovery gaps: root-only pyproject, no setup.py/cfg, launcher files one-way, alphabetical 512-file sniff cap</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/wiring.py:831, 1024–1131</span>

</div>

<div class="klabel">

what

</div>

<div>

Monorepo packages' pyproject.tomls unread; console_scripts from setup.cfg absent; Dockerfile CMD targets clear unwired but never seed roots; the shared sniff budget can exhaust on .c files before src/main.py is reached. Sphinx conf.py, noxfile.py, gunicorn.conf.py etc. become unwired false positives.

</div>

<div class="klabel">

fix

</div>

<div>

Add the missing sources; seed roots from launcher hits; per-language sniff caps; extend structural exemptions.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-analyze

</div>

</div>

</div>

<span class="fid">V7</span><span class="ftitle">Framework detection by naive substring over whole config files</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/repo_mapper.py:2548–2572</span>

</div>

<div class="klabel">

what

</div>

<div>

'react' in package.json matches preact; 'next' matches lint:next or next-auth; a commented-out flask dep in requirements.txt flags Flask. frameworks\[\] feeds agent prompts.

</div>

<div class="klabel">

fix

</div>

<div>

Parse dependency keys; match exact package names.

</div>

<div class="klabel">

lands in

</div>

<div>

P1 · manifest quality

</div>

</div>

</div>

<span class="fid">V8</span><span class="ftitle">test_commands fabricates ruff/mypy commands that were never configured</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/repo_mapper.py:2660–2664</span>

</div>

<div class="klabel">

what

</div>

<div>

Every Python repo is told to run ruff and mypy regardless of configuration; pytest is omitted for tests under src/tests/ or test/.

</div>

<div class="klabel">

fix

</div>

<div>

Gate on \[tool.ruff\]/\[tool.mypy\]/dev-deps; use pytest config presence.

</div>

<div class="klabel">

lands in

</div>

<div>

P1 · manifest quality

</div>

</div>

</div>

<span class="fid">V9</span><span class="ftitle">Barrel re-export chasing re-reads the same files once per importer</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/repo_mapper.py:1636–1724</span>

</div>

<div class="klabel">

what

</div>

<div>

\_follow_js_reexports has a per-call seen set and no cross-call cache: a barrel index.ts imported by N files is read and regex-scanned N× chain-depth times.

</div>

<div class="klabel">

fix

</div>

<div>

Memoize path→reexport targets per mapper run.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · devmap-resolve

</div>

</div>

</div>

<span class="fid">V10</span><span class="ftitle">JS dead-symbol scan re-splits the whole file once per exported symbol</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/repo_mapper.py:2301–2306</span>

</div>

<div class="klabel">

what

</div>

<div>

source.splitlines() inside the per-symbol loop just to peek at one preceding line.

</div>

<div class="klabel">

fix

</div>

<div>

Split once before the loop.

</div>

<div class="klabel">

lands in

</div>

<div>

P3

</div>

</div>

</div>

<span class="fid">V13</span><span class="ftitle">Map visualizer badges present capped counts as totals</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/map_viz.py:15, 186–196, 325–330</span>

</div>

<div class="klabel">

what

</div>

<div>

Lists are capped at 256 for embedding, then rendered as 'unwired 256' as if that were the total; the real totals exist in liveness_meta but aren't embedded.

</div>

<div class="klabel">

fix

</div>

<div>

Embed {shown, total}; render 'unwired 256 of 12,041'.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · viz

</div>

</div>

</div>

<span class="fid">V15</span><span class="ftitle">refresh_map_artifacts serializes repo_map.json twice and re-runs inventory for the re-stamp</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/map_artifacts.py:463–484</span>

</div>

<div class="klabel">

what

</div>

<div>

A second RepoMapper, git ls-files, head and full content fingerprint per run — only needed when write_agent_guides actually changed something, which it already detects but doesn't report.

</div>

<div class="klabel">

fix

</div>

<div>

Return changed-flag from write_agent_guides; re-stamp only then.

</div>

<div class="klabel">

lands in

</div>

<div>

P5

</div>

</div>

</div>

<span class="fid">V16</span><span class="ftitle">dev map floods stdout with the full map JSON; no --quiet/--json control</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">cli/commands/map.py:44–51, 233–250</span>

</div>

<div class="klabel">

what

</div>

<div>

Megabytes of JSON on every run, TTY or not; the root command is the only map subcommand without --json; the build_incomplete warning prints twice; a summary getter side-effect-prints to stderr.

</div>

<div class="klabel">

fix

</div>

<div>

Human summary on TTY, JSON under --json; add --quiet.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-cli

</div>

</div>

</div>

<span class="fid">V17</span><span class="ftitle">MCP graph-context handler spawns a full CLI subprocess per call and swallows its failures</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">integrations/mcp/handlers/graph.py:24–34</span>

</div>

<div class="klabel">

what

</div>

<div>

A cold python -m devcouncil per tool call, though the in-process adapter is already imported for the fallback; \_cli_error is discarded.

</div>

<div class="klabel">

fix

</div>

<div>

Call CodeReviewGraphAdapter directly; drop the subprocess.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · daemon socket

</div>

</div>

</div>

<span class="fid">V19</span><span class="ftitle">Curated subsystem metadata: substring role matching, duplicated buckets, unvalidated handoff fiction</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/repo_mapper.py:541–629, 996–1003</span>

</div>

<div class="klabel">

what

</div>

<div>

Role buckets match by plain substring (same file in multiple buckets); hand-written handoff strings are never validated against real edges and can silently rot.

</div>

<div class="klabel">

fix

</div>

<div>

Validate handoffs against live import edges; dedupe buckets before selection.

</div>

<div class="klabel">

lands in

</div>

<div>

P1 · contract freeze

</div>

</div>

</div>

<span class="fid">V20</span><span class="ftitle">Dead branch: the .sqlite/.db 'database' kind can never be reached</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/repo_mapper.py:54, 844–845</span>

</div>

<div class="klabel">

what

</div>

<div>

\_GENERATED_SUFFIXES filters those files out of the inventory before any file is classified.

</div>

<div class="klabel">

fix

</div>

<div>

Delete or reconcile.

</div>

<div class="klabel">

lands in

</div>

<div>

cleanup

</div>

</div>

</div>

</div>

</div>

<div id="sec-X" class="section">

<div class="grouphead">

## X — Extraction, imports & integration

<span class="groupcount">19 findings · 8 high · 9 medium · 2 low</span>

</div>

<div class="cards">

<span class="fid">X1</span><span class="ftitle">TS enum, abstract class, namespace and declare declarations extract zero symbols</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_ts.py:408–415, 550–639</span>

</div>

<div class="klabel">

what

</div>

<div>

walk_decl handles six node types and doesn't recurse into unmatched ones: enum_declaration, abstract_class_declaration, internal_module, ambient_declaration (all of .d.ts), and function_signature yield nothing; the regex fallback and export collector miss them too. Importers' named-import edges dangle; enum-only files look empty.

</div>

<div class="klabel">

fix

</div>

<div>

Add the missing branches (recurse into ambient_declaration); extend the export declaration set.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus unified capture vocabulary

</div>

</div>

</div>

<span class="fid">X12</span><span class="ftitle">LSP client can deadlock on full pipes: writes hold the lock the reader thread needs</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/lsp_client.py:329–369</span>

</div>

<div class="klabel">

what

</div>

<div>

\_notify/\_request block in stdin.write while holding self.\_lock; the reader needs the same lock to drain responses — ABBA deadlock through two full pipes on large didOpen payloads. Timeouts never engage; shutdown also needs the lock.

</div>

<div class="klabel">

fix

</div>

<div>

Separate write lock from the pending map; or queue writes to a writer thread with timeout.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · lsp adjunct

</div>

</div>

</div>

<span class="fid">X14</span><span class="ftitle">dev impact --precise replaces real dependents with an authoritative empty list for Go/Rust/Swift/Kotlin</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/lsp_client.py:484–535 · mcp/handlers/map.py:276–281</span>

</div>

<div class="klabel">

what

</div>

<div>

\_iter_public_symbols understands only .py and JS suffixes; unsupported languages yield \[\] ('verified zero') rather than None ('unknown'), and the handler overwrites map-derived dependents with an empty list labeled resolution:'lsp' — inverted truth exactly where gopls/rust-analyzer were detected.

</div>

<div class="klabel">

fix

</div>

<div>

Tri-state: return None for unsupported languages; only override when the LSP actually resolved.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · tri-state contract

</div>

</div>

</div>

<span class="fid">X2</span><span class="ftitle">\`new Foo()\` is never a call or reference in JS/TS; Go composite literals likewise</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_ts.py:441–442, 778–802</span>

</div>

<div class="klabel">

what

</div>

<div>

Only call_expression is walked; a class used solely via new Store() has zero inbound edges → dead-symbol false positives for plain classes. Go Type{…} usage likewise produces no reference.

</div>

<div class="klabel">

fix

</div>

<div>

Treat new_expression's constructor as a call; add composite_literal type names to references.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph instantiates-as-caller

</div>

</div>

</div>

<span class="fid">X5</span><span class="ftitle">.tsx parsed with the wrong grammar through AstMatcher; ERROR trees extracted silently</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/ts_imports.py:195–217 · codeintel/languages/registry.py:91–93</span>

</div>

<div class="klabel">

what

</div>

<div>

tsx aliases to typescript before grammar dispatch, so JSX parses with the wrong grammar; the tsx fallback is dead code (parse returns ERROR trees, not None); nothing anywhere checks root_node.has_error — a mangled tree is indistinguishable from a sparse file.

</div>

<div class="klabel">

fix

</div>

<div>

Key grammar on suffix (un-aliased); record has_error and surface it.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus one grammar table

</div>

</div>

</div>

<span class="fid">X6</span><span class="ftitle">No parse-failure signal exists anywhere in the extraction contract</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_python.py:147–149 · languages/generic_extractor.py:27–29</span>

</div>

<div class="klabel">

what

</div>

<div>

FileExtraction has no parse_ok/diagnostics; SyntaxError, ERROR trees and worker failures all read as 'file defines nothing' — the false-'no symbols' → false-dead pipeline the map warns its own agents about.

</div>

<div class="klabel">

fix

</div>

<div>

Add parse_ok + parse_error to the contract, persist in cache, exclude parse_ok=False from dead candidates.

</div>

<div class="klabel">

lands in

</div>

<div>

P1+P2 · contract

</div>

<div class="klabel">

borrow

</div>

<div>

CodeGraph provenance

</div>

</div>

</div>

<span class="fid">X7</span><span class="ftitle">Transient worker timeout/crash is permanently cached as a valid 'no symbols' extraction</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">codeintel/languages/workers.py:227–233 · graph/cache.py:267–296</span>

</div>

<div class="klabel">

what

</div>

<div>

process() returns None on any exception (incl. 30s timeout, OOM-killed child); the empty extraction is cached under the real sha256. The RecursionError path deliberately guards against exactly this (\`sha256: ''\`) — the worker path bypasses it. One slow parse = permanent hole in the graph.

</div>

<div class="klabel">

fix

</div>

<div>

Distinguish failure from empty; on failure skip caching or cache with empty digest, mirroring the RecursionError path.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · cache admission (ParseOutcome)

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus quarantine-then-retry

</div>

</div>

</div>

<span class="fid">X8</span><span class="ftitle">Registry vs dispatch mismatch: .mts/.cts/.ets/.pyi silently downgraded; eight divergent suffix tables</span><span class="sev high">high</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_ts.py:92, 1190–1205 · registry.py:23–27 · +6 more</span>

</div>

<div class="klabel">

what

</div>

<div>

.pyi misses the Python extractor; .mts/.cts miss the JS extractor AND import resolution (resolve/repo_mapper suffix sets exclude them) while wiring includes them — eight independently-maintained JS/TS suffix tables give different answers to 'what language is this file'.

</div>

<div class="klabel">

fix

</div>

<div>

Derive every suffix set from LANGUAGE_SPECS; assert each spec routes to a named extractor at startup.

</div>

<div class="klabel">

lands in

</div>

<div>

P1 · language authority

</div>

<div class="klabel">

borrow

</div>

<div>

GitNexus satisfies-record pattern

</div>

</div>

</div>

<span class="fid">X11</span><span class="ftitle">LSP initialize timeout with a live server treated as success</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/lsp_client.py:163–191</span>

</div>

<div class="klabel">

what

</div>

<div>

A slow (\>20s) init passes the poll() check; the client sends \`initialized\` before the server answered (protocol violation) and marks itself alive; every later request burns its own 12s timeout.

</div>

<div class="klabel">

fix

</div>

<div>

init_result is None → fail start, regardless of poll().

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · lsp adjunct

</div>

</div>

</div>

<span class="fid">X13</span><span class="ftitle">LSP positions in Python code points; no positionEncodings negotiation (spec default UTF-16)</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/lsp_client.py:64–76, 170–179</span>

</div>

<div class="klabel">

what

</div>

<div>

Non-ASCII before a symbol shifts positions → wrong/empty references → confirm_unreferenced can wrongly confirm 'dead' on files with unicode.

</div>

<div class="klabel">

fix

</div>

<div>

Negotiate positionEncodings; convert columns via UTF-16 code units when required.

</div>

<div class="klabel">

lands in

</div>

<div>

P3 · lsp adjunct

</div>

</div>

</div>

<span class="fid">X15</span><span class="ftitle">Embeddings: the configured model name is a label, not part of the cache/compare key</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/embeddings.py:104–166, 194</span>

</div>

<div class="klabel">

what

</div>

<div>

Vectors are always \_hash_vector output but rows store the configured model name, never filtered on search; wiring a real backend would silently cosine-compare across models. With no committed generation, search scans all generations mixed.

</div>

<div class="klabel">

fix

</div>

<div>

Filter on model in the WHERE clause; refuse cross-generation scans.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · devmap-query

</div>

</div>

</div>

<span class="fid">X16</span><span class="ftitle">Semantic snapshots hard-capped at 500 symbols with no truncation marker — phantom API diffs</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/semantic_index.py:91 · ast_matcher.py:88–91</span>

</div>

<div class="klabel">

what

</div>

<div>

First 500 hits in sorted-path order: adding a symbol in an alphabetically-early file pushes one out of the after-snapshot → spurious exported_symbol_removed; real removals beyond the cap invisible.

</div>

<div class="klabel">

fix

</div>

<div>

Raise/remove the cap for snapshots, or record symbols_truncated and suppress add/remove classification when set.

</div>

<div class="klabel">

lands in

</div>

<div>

P5 · budgeted snapshots

</div>

</div>

</div>

<span class="fid">X19</span><span class="ftitle">MCP handlers run full-repo walks and LSP spawns synchronously on the event loop</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">integrations/mcp/handlers/ast_lsp.py:63–77 · map.py:260–283</span>

</div>

<div class="klabel">

what

</div>

<div>

handle_ast_match rglobs and reads every registry-language file in the async handler; impact --precise spawns language servers (20s) inline — the whole MCP server is unresponsive meanwhile.

</div>

<div class="klabel">

fix

</div>

<div>

asyncio.to_thread both; pass a cached file list to AstMatcher.

</div>

<div class="klabel">

lands in

</div>

<div>

P4 · daemon threading

</div>

</div>

</div>

<span class="fid">X20</span><span class="ftitle">JS/TS decorators produce no call/reference edges (Python's do)</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_ts.py (absent) vs extract_python.py:286–304</span>

</div>

<div class="klabel">

what

</div>

<div>

No decorator handling at all: in NestJS/Angular/MobX codebases, decorator functions and the classes they register show as dead/unwired.

</div>

<div class="klabel">

fix

</div>

<div>

Walk decorator nodes; emit ExtractedCall like the Python path.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

</div>

</div>

<span class="fid">X3</span><span class="ftitle">\`export \* from './x'\` barrels lose all symbol-level export information</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/graph/extract_ts.py:385–436, 501–515</span>

</div>

<div class="klabel">

what

</div>

<div>

Star re-exports contribute a file edge only; symbols reachable solely through barrels read as unexported, skewing the dead-symbol gate. Python handles its analogous case; JS doesn't.

</div>

<div class="klabel">

fix

</div>

<div>

Record a '\*:spec' sentinel and expand against the target's exports at resolve time.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

</div>

</div>

<span class="fid">X4</span><span class="ftitle">ts_imports reports every const/let declarator — at any nesting depth — as kind 'function'</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/ts_imports.py:252–265</span>

</div>

<div class="klabel">

what

</div>

<div>

No initializer check and full-body recursion: const MAX_RETRIES = 3 and every local const become 'function' symbols, polluting dev ast results and competing for the 500-symbol snapshot cap.

</div>

<div class="klabel">

fix

</div>

<div>

Port the initializer check from extract_ts; stop descending into function bodies.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

</div>

</div>

<span class="fid">X9</span><span class="ftitle">Rust grouped \`use a::{b, c}\` flattened into one bogus path a::b::c</span><span class="sev medium">medium</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/ts_imports.py:110–137</span>

</div>

<div class="klabel">

what

</div>

<div>

All identifiers under a use_list concatenate into one segment list: use std::{fs, io} → module 'std::fs', names \['io'\] — the fs import is lost and the module path is fictitious. The unit test asserts the corruption.

</div>

<div class="klabel">

fix

</div>

<div>

Fork the segment walk at use_list nodes; emit one ref per leaf.

</div>

<div class="klabel">

lands in

</div>

<div>

P2 · devmap-extract

</div>

</div>

</div>

<span class="fid">X17</span><span class="ftitle">Snapshot import diffing blind to Go block imports and all Rust use statements</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/semantic_index.py:150–153</span>

</div>

<div class="klabel">

what

</div>

<div>

The regex matches only lines starting import/from: Go import ( … ) contributes the useless opener; Rust \`use\` never matches — import_dependency_change never fires for Go/Rust.

</div>

<div class="klabel">

fix

</div>

<div>

Reuse the graph extractors' cached import output instead of a second regex pass.

</div>

<div class="klabel">

lands in

</div>

<div>

P5

</div>

</div>

</div>

<span class="fid">X18</span><span class="ftitle">Snapshot \`public\` heuristic misclassifies Python and matches the word 'export' anywhere</span><span class="sev low">low</span>

<div class="fbody">

<div class="krow">

<div class="klabel">

where

</div>

<div>

<span class="loc">indexing/semantic_index.py:99</span>

</div>

<div class="klabel">

what

</div>

<div>

Lowercase public Python functions never read as public (deleting one never reports exported_symbol_removed); any line containing 'exports' flags private symbols public.

</div>

<div class="klabel">

fix

</div>

<div>

Per-language rule: Python not name.startswith('\_'); JS/TS from the extractor's exported flag.

</div>

<div class="klabel">

lands in

</div>

<div>

P5

</div>

</div>

</div>

</div>

</div>

<div id="adoptions" class="section">

## §6 — Reference-repo adoptions

<div class="prose">

What GitNexus and CodeGraph actually validated in production, mapped to the phase that should import it. These are the non-bug improvements; the bug-shaped lessons are already encoded in the findings above.

</div>

| Technique | Source | What it buys dev map | Phase |
|----|----|----|----|
| **Precompute at index time** | GitNexus | Store entry-point→leaf flows, communities and blast radius at build; a symbol query returns the flows it participates in with step indices — not edges the caller must walk. One call is enough. | <span class="loc">P3+P5</span> |
| **Confidence as a closed vocabulary** | GitNexus | One tier table (0.95 / 0.9 / 0.5-class) in one module; every edge carries confidence + machine-readable reason; every query takes min_confidence. Today's literals are scattered across semantic.py and \_confidence_score. | <span class="loc">P3</span> |
| **Provenance on every heuristic edge** | CodeGraph | provenance: extracted \| inferred \| heuristic plus synthesizedBy channel on synthesized edges (decorator, DI, event, bridge) — machine-checkable honesty about which connections are guesses. The store's provenance column already exists; extend to channels and surface it. | <span class="loc">P3+P5</span> |
| **Impact semantics that survived production** | CodeGraph | Exclude upward contains (sibling explosion), count instantiates as a caller, descend into container children at same depth, dedup edges on source\|target\|kind\|line\|col, per-add budget caps. dev map already tags instantiates in extras but nothing consumes it. | <span class="loc">P5</span> |
| **Traversal correctness idioms** | CodeGraph | enqueued set separate from visited; BFS edge priority contains → calls → rest so truncated results show architecture first; batch node fetches per frontier. | <span class="loc">P3+P5</span> |
| **Three-layer freshness** | CodeGraph | Debounced watcher + per-surface staleness banner (⚠️ prefix on any response touching a pending file) + connect-time (size,mtime)+content-hash catch-up. Layers 1–2 partially exist; layer 3 closes S17. | <span class="loc">P4</span> |
| **FTS5 external-content + triggers** | CodeGraph | Restructure nodes_fts as an external-content table over a current-generation search table synced by triggers — removes per-generation copy and the UNINDEXED-delete trap; index shrinks to one generation. | <span class="loc">P4</span> |
| **Single language authority** | GitNexus | Every suffix set, extractor dispatch and LSP map derived from LANGUAGE_SPECS; a test asserts every spec routes to a named extractor (the satisfies-record pattern as CI). Kills X8's eight divergent tables. | <span class="loc">P1</span> |
| **Golden corpus per language** | CodeGraph | One realistic fixture per language (~35 files) with asserted symbol/import/call counts; 11 of 35 languages currently have zero test presence. Catches X1/X9/G19 mechanically and feeds the parity harness. | <span class="loc">P1</span> |
| **Sibling skeletonization** | CodeGraph | Collapse ≥3 interchangeable siblings (25 cli/commands/\*.py, executor adapters) into an expandable family badge — budgeted progressive disclosure for both the query surface and the visualizer. | <span class="loc">P5</span> |
| **WebGL + reducers for the symbol graph** | GitNexus | sigma.js v3 + graphology; filters restyle via nodeReducer/edgeReducer + hidden attributes instead of re-feeding graphData per keystroke; size-tiered physics (\>1,500 reduce, \>5,000 skip cosmetic passes); confirm gate before loading 10k+ nodes; labels throttled by zoom/density. | <span class="loc">P5</span> |
| **Blueprint layout for the subsystem map** | CodeGraph | “Hand-placed so it reads as a deliberate blueprint, not a physics blob” — 24 subsystems need layered hierarchical placement, thin lines, hollow nodes, mono labels; no physics at all. | <span class="loc">P5</span> |
| **Weighted communities** | GitNexus | Accumulate call multiplicity as edge weight before clustering; a 50-call pair currently equals a 1-call pair. | <span class="loc">P3</span> |
| **Deterministic everything** | both | Sorted iteration wherever order leaks into artifacts (G4, G26); IDs stable under overload addition; snapshot tests become possible. | <span class="loc">P3</span> |

</div>

<div id="plan" class="section">

## §7 — Amended plan

<div class="prose">

The six-phase structure, crate layout, daemon architecture and 32–42-week estimate from [PLAN §4–5](PLAN.html) stand. This audit changes the phases' *contents and gates*, not their shape. Per project direction the Python implementation receives **no fixes** — it is frozen as the parity baseline. That sharpens one consequence PLAN §7 already named: the hazards catalogued here (S1's kill-on-probe, X7's poisoned cache, V1's XSS, V4's stale blast radius) remain live in the interim system until cutover, which strengthens the case for sequencing P4 (store + daemon) as early as its dependencies allow and for running the daemon in shadow mode against real work from mid-P4 onward.

One nuance on freezing: several findings are *parity traps*. G4/G26 make the Python baseline nondeterministic, X-audit shows its extractions are silently incomplete, and S17 makes its freshness racy — so "match the Python output" is not a well-defined target everywhere. The parity harness (PLAN §6.7) must therefore compare against *pinned snapshots* of the frozen baseline taken under a fixed hash seed, with the known-wrong outputs (G1–G7, X1–X3…) recorded as **expected divergences**: places where the Rust port is required to differ, each tied to its acceptance test.

</div>

### Phase 1 — Freeze the contract · ~3 weeks (was: golden corpus + consumer contracts)

<div class="prose">

**Added:** the golden corpus must now cover the syntax X1/X2/X3/X9/G19 proved missing — enums, namespaces, declare files, new-expressions, star re-exports, grouped Rust use, generic impls, decorators — not just the happy path. Build the **single language authority** (X8): one table generating every suffix set, with a CI assertion that each of the 35 languages routes to a named extractor. Freeze the *tri-state* consumer contract (X14): unknown ≠ verified-zero, for every surface. Record the six private-internal reaches (X audit §e) as explicit contracts to break loudly in phase 6.

</div>

### Phase 2 — Extraction · 6–10 weeks

<div class="prose">

**Added:** `ParseOutcome` provenance in the extraction contract from day one (X6): parse_ok, error, engine, grammar version — cache entries carry it, liveness excludes failed parses from dead candidates, and a failed parse is *never* admitted to cache under a real content hash (X7). Grammar-correct dispatch (X5), the missing node types (X1, X2, X20, G14, G16, G18, G19, X3, X9), and per-file size caps + gitignore integration (CodeGraph hygiene). Rust's in-process grammars retire the worker pool and its orphan problem (S8) by construction.

</div>

### Phase 3 — Resolution and analysis · 10–14 weeks

<div class="prose">

**Added:** the confidence ladder becomes a typed protocol — heuristic picks can never ship as EXTRACTED (G6), multi-candidate resolution fans out or abstains rather than picking first-sorted (G6/G7), language-family guards on every global rung (G3/G7), best-confidence edge dedup (G5). Determinism is a phase gate: sorted iteration everywhere order leaks (G4/G26), verified by running the parity harness twice under different hash seeds. The PDG is rebuilt on a correct CFG contract — entry/exit threading (G1), leader-line ownership (G2), block-scoped line ordering (G13), parameters as taint sources (G23). Liveness diagnostics become data, not discards (V5/V6). Go package edges become a star, not a clique (G20).

</div>

### Phase 4 — Store and daemon · 5–7 weeks

<div class="prose">

**Added to the v3 schema work:** migrations as single transactions with user_version last (S2); FTS external-content tables with trigger sync and quoted query terms (S3 + adoption); percent-encoded URIs (S9); BEGIN IMMEDIATE around read-then-insert (S11); alias chains across generations (S16); stat-read-stat freshness with hash confirmation (S17); checkpoint results read, not assumed (S18). The daemon absorbs the entire S4/S5/S6/S7/S10/S12 cluster by construction — one long-lived process owns the write path, so unlock heuristics, orphaned status files, dead worker threads and per-event subprocess spawns cease to exist as categories. Process liveness via platform APIs, never signal 0 (S1).

</div>

### Phase 5 — Token-budgeted query surface · ~4 weeks

<div class="prose">

**Added:** confidence-aware, budget-priority responses (G24): extracted before inferred before ambiguous, so truncation drops noise first; honest `{shown, total}` everywhere including the previously-lying dead_code_hidden (V12) and viz badges (V13). Impact adopts CodeGraph's production semantics (adoption table) with parametric depth (G8). The visualizer splits: a no-physics blueprint layout for the 24-subsystem map, and a WebGL sigma.js explorer for the symbol graph with size tiers, reducers, a confirm gate, per-kind colors and legend derived from the payload (V2/V3), escaped labels (V1), atomic stamped writes (V14). route/shape analysis computes its scan once (G9/G15).

</div>

### Phase 6 — Cutover and deletion · ~4 weeks

<div class="prose">

Unchanged, plus: the parity harness must replay this audit — a fixture per finding (86 new + 31 original = **117 regression fixtures**), so the rewrite cannot reintroduce what the audit removed. LSP adjunct features (X11–X13) either meet the same timeout/deadlock standards in the Rust client or are cut at parity review.

</div>

</div>

<div id="guide" class="section">

## §8 — Implementation guide (Rust)

<div class="prose">

Extends PLAN §6. Each sketch below is the load-bearing shape that makes a finding-class structurally impossible, in the crate that owns it.

</div>

### 9.1 · Parse outcome as a value — X6, X7, X5 (devmap-extract)

    pub enum ParseOutcome {
        Clean(Extraction),
        Partial { extraction: Extraction, error_ranges: Vec<ByteRange> }, // tree had ERROR nodes
        Failed { reason: ParseFailure },                                  // timeout, panic, io
    }

    pub struct Extraction {
        pub engine: Engine,          // tree_sitter(grammar, version) — one enum, no aliases
        pub symbols: Vec<Symbol>,
        pub imports: Vec<Import>,
        pub calls: Vec<CallSite>,
    }

<div class="prose">

Cache admission takes `ParseOutcome`, and only `Clean`/`Partial` are admitted under the content hash — `Failed` is recorded in a retry table with attempt count and backoff. Liveness receives the outcome enum and excludes `Failed`/`Partial`-in-error-range symbols from dead candidates. This one type closes the entire "confidently empty" family.

</div>

### 9.2 · Confidence and provenance as the edge's type — G5, G6, G7, G24 (devmap-resolve)

    pub enum Resolution {
        Extracted { target: NodeId },                          // proven by syntax
        Inferred  { target: NodeId, rule: InferenceRule },     // one candidate after scoping
        Ambiguous { candidates: SmallVec<NodeId>, capped: bool }, // fan-out, never picks
        Unresolved { reason: UnresolvedReason },               // ledger, not silence
    }

<div class="prose">

Three invariants, enforced by construction: (1) a multi-candidate resolution *cannot* produce `Extracted` — the type has nowhere to put the discard; (2) edge dedup is a map `(source, target) → best` where `Extracted > Inferred > Ambiguous`, upgrading in place (G5); (3) every global-scope rung takes a `LangFamily` parameter — cross-family candidates are unrepresentable (G3, G7). `Unresolved` rows persist to the store: GitNexus-style provenance for "why is there no edge here".

</div>

### 9.3 · Traversal kernel — G8, G21, G24 + CodeGraph's production checklist (devmap-query)

    pub struct Traversal {
        enqueued: FxHashSet<NodeId>,   // separate from visited — parallel edges survive
        edge_seen: FxHashSet<EdgeKey>, // (source, target, kind, line, col)
        budget: Budget,                // per-add cap: one hub can't overshoot the limit
        priority: &'static [EdgeKind], // contains → calls → rest: structure first
    }

    pub fn blast_radius(seed: &[NodeId], depth: u8, min_conf: Confidence)
        -> Layered<Impact>  // by_depth sized from `depth` — no hardcoded {1,2,3}

<div class="prose">

Impact semantics fixed as tests, not conventions: upward `contains` excluded; `instantiates` counts as a caller; container children fold in at the same depth; results ranked by confidence before the budget truncates. One traversal kernel serves impact, processes, trace and dead-code so the semantics cannot drift apart per surface (today intel/query/engine already disagree three ways).

</div>

### 9.4 · Store discipline — S2, S3, S9, S11, S16–S18 (devmap-store)

    // Migrations: one transaction per step, version bump is the LAST statement inside it.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(MIGRATION_V3_SQL)?;   // includes: PRAGMA user_version = 3;
    tx.commit()?;                          // crash before this line = clean retry at v2

    // FTS: terms are always quoted phrases — user input can't be syntax.
    fn fts_term(raw: &str) -> String { format!("\"{}\"", raw.replace('"', "\"\"")) }

<div class="prose">

Plus: `rusqlite` opens by path (no URI string assembly — S9); every read-then-write pair lives inside `TransactionBehavior::Immediate` (S11); alias resolution chains across retained generations (S16); the file scanner stats before and after reading and retries on mismatch (S17); checkpoint results are read and acted on (S18). The daemon is the only writer, so the lease/unlock heuristics that produced S1/S4/S5 are deleted, not ported — `devmap status` asks the daemon over the socket instead of probing PIDs.

</div>

### 9.5 · Watcher — S6, S7, S12, S17 (devmap-serve)

<div class="prose">

The event loop is a supervised task: panics are caught per-tick, recorded as `degraded` with the error, and the loop continues (S6). Failed batches back off exponentially and quarantine poison files into a status-visible list after N attempts (S7). Ignore matching is in-process (the `ignore` crate — same engine ripgrep uses), with verdicts cached against .gitignore mtimes; no subprocess per event (S12). Freshness follows CodeGraph's three layers: debounced events → per-response staleness banner for pending files → connect-time (size, mtime) sweep with content-hash confirmation for the racy window (S17).

</div>

### 9.6 · Surfaces — V1–V3, V12–V14, X14 (devmap-query + templates)

<div class="prose">

Responses are typed `Budgeted<T> { items, shown, total, truncated }` — the count fields are computed by the truncation function itself, so a lying count is unwritable (V12). Dependents queries return `Option<Vec<_>>`: `None` = unknown (unsupported language, stale index), never an empty vec masquerading as verified-zero (X14) — and the MCP layer renders `None` as an explicit `"resolution": "unavailable"`. Generated HTML: every interpolation goes through one `esc()` helper (V1); payloads live in `<script type="application/json">` blocks per mode (V2); pages embed `{generated_head, built_at, fingerprint}` and render a staleness banner; writes are tmp+rename (V14). The subsystem map is laid out deterministically by dependency layer — CodeGraph's blueprint, not a physics blob; the symbol explorer is sigma.js WebGL with GitNexus's size tiers and a confirm gate above 5,000 nodes.

</div>

### 9.7 · The language authority — X8 (build-time, all crates)

    pub struct LanguageSpec {
        pub name: &'static str,
        pub extensions: &'static [&'static str],
        pub grammar: Grammar,          // the actual linked grammar — not a string
        pub extractor: ExtractorId,    // exhaustive match = compile error if missing
    }
    pub static LANGUAGES: [LanguageSpec; 35] = [ /* … */ ];

<div class="prose">

Suffix→language, language→grammar, language→extractor, LSP capability and viz legend all derive from this table at compile time. Rust's exhaustive `match` on `ExtractorId` is the `satisfies Record<…>` pattern GitNexus uses — a 36th language that forgets its extractor does not build. The eight divergent Python suffix tables have no equivalent to port.

</div>

</div>

<div id="accept" class="section">

## §9 — Acceptance additions

<div class="prose">

PLAN §3 turned 31 findings into acceptance tests. This audit adds 84 more, of which the highest-leverage gates are:

</div>

| Gate | Proves | Findings |
|----|----|----|
| CFG threading fixture: loop/branch/try bodies reachable from entry; condition uses visible to def-use | PDG runs on a connected graph | G1, G2, G13, G23 |
| Resolution fixture: stdlib-named methods keep inbound edges; proven call beats earlier fan-out; heuristic picks never EXTRACTED; no cross-language inheritance | confidence ladder honest | G3, G5, G6, G7 |
| Two builds under different hash seeds produce byte-identical artifacts | determinism | G4, G26, V5 |
| Syntax corpus: TS enum/namespace/declare/new/export★/decorators, Rust generic impls + grouped use — all extract | no silent blind spots | X1–X3, X9, G14, G16, G18, G19, X20 |
| Kill a parser mid-file: extraction retries later; cache holds no empty entry under the real hash | failure ≠ emptiness | X6, X7 |
| kill −9 during migration at every statement boundary: store reopens clean at old or new version, never between | atomic migrations | S2 |
| Fuzzed search queries (quotes, hyphens, parens, column syntax): no crash, no full-scan fallback | FTS hygiene | S3 |
| \`devmap status\`/\`unlock\` on Windows CI with a live build: build survives; stuck detection keys on progress, not age | ops can't destroy work | S1, S4, S5 |
| Impact for an unsupported-language file reports resolution:unavailable, never an empty dependents list | unknown ≠ zero | X14 |
| Edit-then-query under watch: dependents reflect the edit or carry a stale flag | freshness honesty | V4, S17 |
| Every truncated response: shown + hidden + total reconcile; viz badges show "N of M" | counts never lie | V12, V13, T3 |
| Hostile file names (`<img onerror>`…) render inert in every generated HTML surface | output escaping | V1 |

<div class="prose">

Full mapping: every finding card above names its phase; the parity harness gains one fixture per ID. With PLAN's originals, the rewrite's specification now stands at **117 verified, regression-tested properties**.

</div>

</div>

Audit II · generated 2026-08-11 · basis: DevCouncil working tree (769 source files) · method: four parallel full-read audits (graph core · store/sync · surfaces · extraction), primed with GitNexus and CodeGraph production failure catalogues; all claims verified against exact source lines, high-severity claims independently re-verified. Companion to [PLAN.html](PLAN.html) / PLAN.md. Two agent-reported duplicates consolidated (G12→V5, X10→G19).

</div>

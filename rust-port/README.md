# Dev Map

Concurrent agent worktrees: [design, audit and rollout contract](AGENTIC_WORKTREE_DESIGN.md), with [native qualification, exact artifacts and coordinated schema-20 cutover](SCHEMA20_QUALIFICATION_2026-09-09.md). Navigation is read-only; use explicit `devmap repair --schema` only after coordinating all installed readers and writers.

Host applications can link the query crate or use the versioned JSON and HTML
process contract in [HOST_INTEGRATION.md](HOST_INTEGRATION.md).

Symbol-level code intelligence for coding agents and the people who supervise
them. One binary, one SQLite store, no daemon required and no network.

It parses a repository with tree-sitter, resolves calls and imports into a graph
it can defend, and answers the questions that actually come up mid-change:
who calls this, what breaks if I change it, which tests cover it, what is dead,
what did that edit just do.

```bash
devmap build --manifest # index this worktree and export its repository map
devmap explore handler  # the whole neighbourhood of a symbol, in one call
devmap affected src/api.py   # the tests a change here reaches
devmap dead                  # dead-symbol candidates, each with its confidence
```

## Why not grep

Grep finds strings. This resolves *edges*, and — more to the point — it tells
you when it could not.

Every answer carries what it could not see. A budgeted list reports
`shown`/`hidden`/`total` and whether it was truncated. A walk stopped by its
depth cap sets `walk_incomplete`. An edge carries the evidence tier it was
resolved on, so a speculative match never reads like a certain one. A dead-code
verdict is `extracted` (parsed, certain) or `inferred` (unconfirmed), never a
flat list.

This is the whole design constraint: **a check that could not run must never
report the same result as a check that ran and passed.** An agent acting on
"no callers found" needs to know whether that means "nothing calls this" or
"we stopped looking."

`status` verifies current source bytes and analyzer identity before reporting
freshness. Query envelopes use `source_freshness: null` when that whole-tree
check was not performed; changed source snippets are withheld with a reason.
See the [coverage and robustness audit](RELIABILITY_AUDIT_2026-09-09.md) for
current fixes, measured stress results, and remaining language/platform limits.
The [earlier reliability audit](RELIABILITY_AUDIT_2026-09-08.md) records the
previous binding and source-read repairs.

An incomplete query can have several independent causes:

- `truncated` means the output budget withheld known results. Increase
  `--budget` to display more; `total` still describes only what the walk reached.
- A depth limit in `walk_incomplete` means traversal stopped before exhausting
  the indexed graph. Increase `--depth` to explore further.
- Attribution warnings describe repository-wide unresolved sites, not a count
  of missing edges for the selected symbol. Known built-ins, runtime globals,
  and external imports are excluded using the recorded resolution breakdown.
  Callbacks, unknown receivers, and other unbound targets still qualify the
  answer. Older or inconsistent breakdowns report unknown coverage.

Impact, trace, affected tests, and the layers in composed answers retain these
coverage qualifications. Rebuilding refreshes stale source data; it does not
remove language limitations or make unresolved dynamic calls deterministic.

## Install

No-checkout install (verified: succeeds without a local clone; cold build ~1–2
minutes depending on the machine, because every tree-sitter grammar compiles):

```bash
cargo install --git https://github.com/bharathvbcr/DevCouncil --locked devmap-cli
```

From a checkout of this tree:

```bash
cargo install --path crates/devmap-cli --locked --force
```

That puts `devmap` on `PATH` via `~/.cargo/bin` (what GUI apps such as GitPulse
search when they lack a shell profile). `--locked` keeps the tree's
`Cargo.lock`; `--force` refreshes an earlier install. Requires a Rust toolchain.

Or build in place — the binary lands at `target/release/devmap`:

```bash
cargo build --release
```

Requires a Rust toolchain. Nothing else: the grammars are compiled in, and the
HTML view embeds its own renderer.

Prebuilt binaries (when a version tag is pushed) land on GitHub Releases for
`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, and
`x86_64-pc-windows-msvc`, each with a `.sha256` checksum. A failed Windows build
fails the release rather than omitting that target.

### Version contract

`devmap --version` prints the crate version and the store / code-graph schema
numbers the binary reads. Hosts that shell out for build / status / preview
should treat a missing binary and a schema mismatch as distinct failures — never
as an empty “all clear”.

Prefer `devmap doctor --json` for machine verification: it emits
`schema_version`, `expected_schema_version`, `code_graph_schema_version`,
`linked_grammar_count`, and `store_path` as one JSON object, so a host does not
need to scrape `--version` prose.
## Use

### Index

```bash
devmap build .              # cold or incremental, decided by content hash
devmap build . --full       # ignore the caches and re-parse everything
devmap build . --manifest   # also write repo_map.json and code_graph.json
devmap status               # generation, node/edge counts, freshness
```

State lives in `.devmap/` by default. See **Where state lives** below.

For a fresh checkout or linked worktree, run `devmap paths --json` to find
the resolved database and `repo_map` paths, then `devmap status --json` to
inspect readiness. Generated state is local to a worktree and is not copied
by Git. A missing store is an uninitialized checkout, not an empty graph.
Run `devmap build --manifest` to create both the database and exported maps;
use the same command to restore a missing map beside an existing database.
Read-only discovery never creates or copies state from another worktree.
See the [state-discovery audit](STATE_DISCOVERY_AUDIT_2026-09-09.md) for
reproduced failures, regression evidence, and qualification limits.

When a repository argument is omitted, builds, rendering, and queries agree
on the nearest Git worktree root, including from a subdirectory. Explicit
paths retain their scope: `devmap build .` deliberately indexes the current
directory. Outside Git, omitted roots use the current directory.

Interactive builds show an animated stage bar, the active operation (including
writer-lock waits), and elapsed stage time. Animation appears after 150 ms and
refreshes at most every 80 ms, so fast unchanged builds stay quiet. Reading and
extraction show measured file counts and a phase percentage once their total is
known. Wider terminals also show throughput and an approximate phase ETA after
at least ten files and one second of measured work. The stage bar is not an
overall percentage: resolving and writing have different costs.

The final summary shows generation, elapsed time, file/symbol/edge counts,
added/changed/removed sources and actual cache hits. Unchanged builds report the
current generation once. `--verbose` retains phase, reclaim and resolution
details and enables debug tracing; coverage refusals and reclaim failures remain visible by default.
Requested consumer artifacts must finish before completion is announced.

```bash
devmap build . --progress auto     # animated on interactive stderr (default)
devmap build . --progress always   # also show plain progress in redirected logs
devmap build . --progress never    # suppress progress; diagnostics still appear
devmap build . --verbose           # include phase and resolution details
devmap build . --json              # JSON only on stdout, including stage timings
```

`--json --progress always` keeps JSON on stdout and progress on stderr. Live
rows adapt to terminal width; `TERM=dumb` uses plain lines and Windows uses
ASCII plain lines. `NO_COLOR=1` disables color. Non-UTF-8 locales use ASCII progress. Redirected
output never contains progress animation escapes. Paths and diagnostics escape
terminal control characters.

Progress output has a bounded queue and shutdown. A paused, full or disconnected
stderr cannot hold the indexing result on a separate stdout pipe. Primary result
output still follows ordinary stdout backpressure, after releasing the build's
writer lock. `progress_output` in JSON
reports failed writes, dropped updates, and retained/omitted diagnostics; a
successful write means the stream accepted the bytes, not that a person saw
them. Human summaries disclose incomplete progress output too.

JSON `file_progress` carries scan and extraction counters plus source deltas.
An extraction value of `null` means it was skipped; zero cache hits means it ran
without a hit. Removed sources are absent from the readable scan compared with
the previous generation; discovery refusals are counted separately. See
[PROGRESS_AUDIT.md](PROGRESS_AUDIT.md) for verification evidence and platform limits.

Runtime failures include `diagnostic_context` in JSON and a `DevMap context:`
line on stderr: command, binary/version, PID, timestamp, repository root, selected
database, elapsed time, and the active build stage (null before a stage begins).
Non-UTF-8 paths are marked as lossy display strings. Query text, preview buffers,
and environment variables are excluded from this context. Build errors retain
their cause chain, phase timings, and progress-delivery receipt.

To capture a diagnosable build while keeping machine output separate:

```bash
devmap build . --json --progress always --verbose > /tmp/devmap-report.json 2> /tmp/devmap.log
```

These are caller-selected output files; the CLI does not keep an automatic log
history. GitPulse's **Code → Map → Copy DevMap logs** combines its captured CLI
diagnostics with the panel state and its existing rotating logs.

### Ask

| Command | Answers |
|---|---|
| `devmap explore <query>` | Definitions, source, callers, callees and blast radius — one call |
| `devmap search <query>` | Symbols by name (add `--semantic` to rank by similarity) |
| `devmap impact <target>` | What a change here reaches |
| `devmap deps <file>` | What this file depends on |
| `devmap trace <a> <b>` | A path between two symbols |
| `devmap neighbors <targets…>` | Callers and callees for several targets at once |
| `devmap affected <targets…>` | Test files the change reaches, nearest first |
| `devmap dead` | Dead-symbol candidates, each carrying its own confidence |
| `devmap clones` | Duplicated and structurally similar bodies |
| `devmap preview --file f --content -` | What an *unsaved* edit would break |
| `devmap savings` | What the index cost against reading the files |
| `devmap cypher '<query>'` | A small openCypher subset over the graph |
| `devmap pdg <file>` | Control/data dependency graph per function (Python) |
| `devmap routes` | HTTP routes, their handlers, and the clients that call them |
| `devmap shape-check` | Keys a handler returns against the keys its callers read |
| `devmap api-impact <route>` | What changing one route reaches, with a risk band |
| `devmap ast <query>` | Symbols by kind, language and name, with an exact total |
| `devmap export` | The graph as GraphML, for Gephi, yEd, Cytoscape or networkx |

Add `--json` to any of them for a machine-readable answer — on either side of
the subcommand.

`cypher` supports one shape:

```
MATCH (a)-[r:calls|imports|…]->(b) WHERE … RETURN a, b LIMIT n
```

with `contains(a.name, '…')` and `starts with(b.path, '…')` joined by `AND`.
Anything outside it is **refused rather than widened** — a `WHERE` term it
cannot evaluate would otherwise return every row in the graph under a successful
status, and a relationship name the graph cannot emit would return zero rows
that read as "nothing matches" rather than "that name cannot match". Refusals
exit non-zero, so a script can tell "not run" from "matched nothing".

### Dependence and taint

```bash
devmap pdg src/handlers.py           # CFG + data dependencies, per function
devmap pdg src/handlers.py --taint   # only functions reaching a sink
```

Intra-procedural: a closure's body is its own graph, not part of its enclosing
function's control flow. Python only — that is the language the analysis this
replaces covered, and claiming a language whose statement tree nothing produces
would return an empty result that reads as *this file has no control flow*.
Every other language is refused by name.

A reported sink is evidence. **Its absence is not a safety claim** — the sink
patterns are a heuristic list of well-known ones, and the analysis has no scope
resolution, no alias tracking and no cross-function reasoning.

### Routes

```bash
devmap routes .                       # every route, its handler, its callers
devmap shape-check .                  # where producer and consumer disagree
devmap api-impact /api/users/{id} .   # blast radius for one route
```

Routes come from the graph's route nodes — one per `@app.get` / `@app.route` /
`router.post` the extractor found, keyed `file::VERB path` and carrying the
framework — with the resolver's `routes_to` edges attaching handlers. One row
per declaration: three files here declare `GET /health`, and they are three
routes with three files and two frameworks, not one row claiming both. A route
whose handler never bound is still reported, with no handler, because "nothing
in this index serves it" is an answer. A generation built before route nodes
existed has only the edges, and is still read: those routes carry no framework
and are counted separately. Client call sites come from
a **pattern scan** — `fetch`, `axios`, `requests`, `httpx` — over the files the
graph names, so a `fetch` inside a comment is a hit and a caller built from a
computed URL is not. Every site is labelled `evidence: "regex"`.

The scan is bounded (`--max-files`, `--max-file-bytes`) and reports itself:
`files_read`, `files_skipped_budget`, `files_over_size`, `files_unreadable`, and
a `complete` flag. **This is what the risk band is built on.** A route with no
caller found is `none_observed` when the scan finished and `unknown` when it did
not — never a band that reads as "safe to change", because absence from an
unfinished, pattern-based search is not evidence of no caller. `scope` says what
`complete` covers: the files the graph names, not the whole working tree.

`middleware` is `null` rather than empty on every route: there is no
registration edge kind for it to be read from. `framework` is `null` only on a
route recovered from an edge with no route node. `capabilities` carries
`route_nodes` and `routes_without_a_route_node` beside the two availability
flags, so a null can be told apart from an empty one — `[]` would mean "this
route has none", which is a different claim.

### See

```bash
devmap html .                          # files and their imports → .devmap/graph.html
devmap html . --level symbols          # symbols and their calls
devmap html . --max-nodes 4000         # widen the cap
devmap map-html .                      # subsystems, coloured by language
```

`html` draws the code graph; `map-html` draws the subsystem map from
`repo_map.json`, colouring each node by its dominant language in GitHub
Linguist's own palette and carrying the coverage the map does *not* have —
subsystems are a selected set, not a partition.

`map-html` reads the resolved state directory's `repo_map.json` and writes
`map.html` beside it, honoring `.devmap/`, legacy `.devcouncil/`, and
`DEVMAP_HOME` consistently with the builder. Explicit `--input` and `--output`
paths remain relative to the selected repository root unless absolute.
Input uses the host artifact reader: it must be a regular, non-symlink file,
valid repository-map JSON, and at most 128 MiB. Refused input never creates
or replaces the preview.

Both HTML views offer **Hide notes & Markdown**. The code graph hides note and
document nodes (including symbols owned by Markdown files) and their edges.
The subsystem map hides an area only when all its attributed files are
documentation; mixed areas and areas without indexed files stay visible. The
control is off by default and filters only the loaded view, preserving index
coverage and cap counts. Regenerate existing HTML to get the new control; use
`devmap map-html . --force` to replace a preview whose map fingerprint is unchanged.

One self-contained HTML file. It opens from a `file://` URL on a machine with no
network and no package manager, because the renderer is embedded.

The view is capped — a force layout over 12,000 nodes is a hairball that pins a
CPU — so nodes are ranked by degree and the most connected survive. The cap is
never silent: the page header reads `1,000 of 12,103 nodes`, and the same two
numbers ride in `--json`.

### Structural search

```bash
devmap ast handler --kind Function --language rust
devmap ast --facets              # the kinds and languages this index holds
```

`search` ranks by name relevance and budgets its answer; `ast` enumerates
everything matching a filter and reports the **exact** total, so a page of 3 out
of 309 says so. A filter naming a kind or language the generation does not hold
is reported by name rather than answered as zero — a typo and a genuine
no-match need different reactions.

Every hit carries the extraction that produced it —
`TreeSitter { grammar: "rust", grammar_version: 14 }`, `RegexFallback`,
`Unavailable { requested_language: "powershell" }` — because a pattern-matched
symbol and a parsed one are different claims.

### Export

```bash
devmap export .                 # -> .devmap/graph.graphml
devmap export . --out -         # stdout
```

Attributed GraphML: node kind, path, area, community, language, line and the
dead/unwired/unreachable flags; edge kind and confidence. Two repairs are
counted and reported rather than made silently — an edge whose endpoint is not
a declared node (GraphML cannot express one) and a character XML 1.0 forbids.
On this repository: 17,130 nodes, 94,350 edges, 370 edges omitted.

`liveness_reliable` rides on the graph element: when the liveness pass withdrew
its answer, every `unreachable` flag is false because nothing was established,
not because every file is reached.

### Serve

```bash
devmap serve .    # index over IPC, watching the tree for changes
devmap mcp        # speak MCP on stdin/stdout, for an agent host
```

`devmap mcp` publishes eleven read-only tools: `devmap_status`, `devmap_search`,
`devmap_dependencies`, `devmap_impact`, `devmap_trace`, `devmap_neighbors`,
`devmap_dead_symbols`, `devmap_clones`, `devmap_preview`, `devmap_explore` and
`devmap_affected_tests`. Every one declares an `outputSchema`, and the server
validates its own answers against the schema it published before emitting them.

### Claude Code

```bash
devmap claude plugin            # write the plugin bundle
devmap claude hooks             # print a hooks block for settings.json
devmap claude validate <path>   # check a hooks/plugin/marketplace file
```

`claude plugin` writes an installable bundle to `.devmap/devmap-plugin` (or
`.devcouncil/devmap-plugin` in a DevCouncil repo): a single-repo marketplace,
the MCP server, agent skills that prefer DevMap over GitNexus, a `PostToolUse`
hook that re-indexes after an edit, SessionStart status + last-session insights,
and a SessionEnd `session-report` that writes query-log honesty flags (truncated,
walk_incomplete, empty, errors) plus recorded capability gaps. Install it with:

```
/plugin marketplace add <path-to-repo>/.devmap/devmap-plugin
/plugin install devmap@devmap-local
```

The emitted config names `devmap` when that name on your `PATH` resolves to the
same binary, and the absolute path when it does not — so a bundle you commit
does not name a path that exists on one machine. `--binary` overrides both, for
packaging.

`claude validate` exists because Claude Code drops a hook it cannot parse
*quietly*: an unknown event name is ignored at runtime, a matcher on an event
without matcher support is ignored, an `if` outside a tool event means the
handler never runs. A writer that only serializes reports the same success for a
dead install as for a working one.

### Agent guides

```bash
devmap manifest . --guides
```

Writes `AGENTS.md` and `CLAUDE.md` pointing agents at the map. Opt-in, because
creating files in somebody's repository is not a thing an index should do
unasked, and a guide that carries no `Managed by devmap` marker is treated as
hand-written and never touched.

The guide states which fields this map actually *computed*. An agent told to use
`role_files` when the producer left it empty reads the emptiness as "this
subsystem has no roles" rather than "no answer was given" — so when a field was
not computed, the guide says so and names what to use instead.

## Where state lives

Resolved per repository, in this order:

1. `$DEVMAP_HOME` — an explicit answer always wins, and is the only way to put
   state outside the repository.
2. `<repo>/.devmap/` when it already exists.
3. `<repo>/.devcouncil/` when it already exists — a repository shared with
   [DevCouncil](https://github.com/bharathvbcr/DevCouncil) keeps its layout, so
   both tools keep agreeing about one file.
4. `<repo>/.devmap/` otherwise.

`--db` overrides the store path directly; a relative value is resolved against
the invoking directory. A relative `DEVMAP_HOME` is resolved against the
repository root. The state directory holds
`codeintel/devmap.sqlite` (canonical), `repo_map.json`, `graph/code_graph.json`
and `workspace.json`. `devmap paths` prints the resolved layout and whether
the state directory and database exist, without opening the store, so a wrapper can ask instead of
re-deriving the rule.

## Across repositories

"Who calls this?" does not stop at a repository boundary when the caller is a
sibling service.

```bash
devmap workspace add ../other-service
devmap workspace search Handler     # every registered repository at once
devmap workspace links              # cross-repo import candidates
```

Cross-repository edges are *candidates with evidence*, never name matches. Two
repositories routinely declare the same `New`, `Client` or `get`, and joining on
the name would manufacture edges at a scale that makes the graph worse rather
than larger.

## Languages

35 languages carry a full grammar and extraction spec:

> ArkTS, Astro, C, C#, C++, CFML, COBOL, CUDA, Dart, Erlang, Go, Java,
> JavaScript, Kotlin, Liquid, Lua, Luau, Metal, Nix, Objective-C, PHP,
> Pascal/Delphi, Python, R, Ruby, Rust, Scala, Solidity, Svelte, Swift, TSX,
> Terraform/OpenTofu, TypeScript, VB.NET, Vue

Shell, SQL, Protobuf, PowerShell and Jupyter notebooks are discovered by
extension and recovered by a pattern tier rather than a full spec.

A file whose language has no grammar still contributes a node, plus any symbols
that tier can recover — labelled `RegexFallback` and never mixed in with parsed
output, because a pattern-matched symbol and a parsed one are different claims.
Prose and data formats are excluded from scanning entirely: unlabelled, the tier
once attributed Go and TypeScript types written inside fenced code blocks in
design documents to the `.md` files describing them.

## Development

```bash
cargo test --workspace     # 1,600+ tests
cargo clippy --workspace --all-targets
./verify.sh                # the full gate: parity, growth, incremental equivalence
```

`AGENT_PLAN.md`, `DIVERGENCES.md` and `STATUS.md` carry the port's history and
every deliberate divergence from the Python implementation this replaced.

## License

Apache-2.0, per the `LICENSE` at the repository root. `Cargo.toml`, the plugin
manifest and that file all say so, and a test fails if they stop agreeing.

## Repository hygiene policy

`devmap-query::hygiene` supplies portable output eligibility, preservation rules,
retention validation and agent guidance, including in no-parser builds. See the
[host contract](../docs/repository-hygiene.md). Hosts own authorization, activity
checks, scheduling and execution; the policy module never deletes files.

# Dev Map

Symbol-level code intelligence for coding agents and the people who supervise
them. One binary, one SQLite store, no daemon required and no network.

It parses a repository with tree-sitter, resolves calls and imports into a graph
it can defend, and answers the questions that actually come up mid-change:
who calls this, what breaks if I change it, which tests cover it, what is dead,
what did that edit just do.

```bash
devmap build .          # index the tree
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

## Install

```bash
cargo install --path crates/devmap-cli
```

Or build in place — the binary lands at `target/release/devmap`:

```bash
cargo build --release
```

Requires a Rust toolchain. Nothing else: the grammars are compiled in, and the
HTML view embeds its own renderer.

## Use

### Index

```bash
devmap build .              # cold or incremental, decided by content hash
devmap build . --full       # ignore the caches and re-parse everything
devmap build . --manifest   # also write repo_map.json and code_graph.json
devmap status               # generation, node/edge counts, freshness
```

State lives in `.devmap/` by default. See **Where state lives** below.

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

### See

```bash
devmap html .                          # files and their imports → .devmap/graph.html
devmap html . --level symbols          # symbols and their calls
devmap html . --level subsystems       # subsystems and the handoffs between them
devmap html . --max-nodes 4000         # widen the cap
```

One flag with three values rather than two booleans, because two booleans for
three views leave a fourth combination that has to mean something and does not.

One self-contained HTML file. It opens from a `file://` URL on a machine with no
network and no package manager, because the renderer is embedded.

`--level subsystems` reads `repo_map.json` rather than the store, because that
is where subsystems are derived; without one it names the command that writes
it instead of drawing a second, in-memory grouping that could disagree with the
file every other consumer reads. A crossing whose endpoint it cannot attribute
to any subsystem is listed as unplaced in the sidebar and counted in the header
— never dropped, since a missing line and "nothing crosses here" look identical
once it is gone.

The view is capped — a force layout over 12,000 nodes is a hairball that pins a
CPU — so nodes are ranked by degree and the most connected survive. The cap is
never silent: the page header reads `1,000 of 12,103 nodes`, and the same two
numbers ride in `--json`.

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

`claude plugin` writes an installable bundle to `.devmap/devmap-plugin`: a
single-repo marketplace, the MCP server entry, and a `PostToolUse` hook that
re-indexes after an edit. Install it with:

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

`--db` overrides the store path directly. The state directory holds
`codeintel/devmap.sqlite` (canonical), `repo_map.json`, `graph/code_graph.json`
and `workspace.json`.

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

Apache-2.0, per the `LICENSE` at the repository root.

> Note: `Cargo.toml` currently declares `license = "MIT"`, which contradicts that
> file and the `Apache-2.0` the plugin manifest emits. The declaration is what
> needs correcting, not the LICENSE — but that is a call for the owner to make,
> so it is recorded here rather than changed.

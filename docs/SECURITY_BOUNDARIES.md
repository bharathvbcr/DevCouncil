# Repository input and verification boundaries

Repository contents are input to analysis. They do not authorize executing a
discovered program, writing through a link, reading outside the selected root,
or treating an unexamined check as a pass. These contracts apply to the source
tree; distributing them requires rebuilding and releasing the affected binaries.

## Program selection and verification

`devmap paths` and `doctor` inspect discovered executable identities without
running them. The current process can report its own compiled version; an
external executable reports that execution was skipped. A version comparison
that could not run is not a matching-version assertion.

The Go navigation adapter no longer selects repository-local build outputs
automatically. An explicit `DEVMAP_BINARY` or adapter binary selection still
allows an operator to choose a local build. PATH discovery rejects candidates
whose canonical location is inside the repository, including aliases and
relative PATH entries.

That rejection has one owner, `proc.LookPathOutside`, shared by the navigation
adapter and by `devcouncil integrate`. Integration separates the two questions
it used to conflate: the name written into a host config is resolved later by
the host, while the program this process spawns must come from outside the
repository. When no such program is found, DevMap asset composition is skipped
with a note instead of falling back to a bare name. Composed invocations run
under a bounded deadline in their own process group.

Required verification commands use process outcomes. Text such as `no tests
ran`, `SyntaxError`, or `command not found` cannot change a failed process into
a skipped check. A required command that was skipped, could not launch, or had
no runner blocks both enforce and advisory verification. Disabling verification
continues to produce an explicit skipped result. An ordinary test failure keeps
the configured gate policy.

Lease renewal updates and returns the row authenticated by the supplied lease
identity and token. A concurrent replacement or expiry refuses the renewal;
renewal does not return a new holder's token. Secret detection and redaction
consider every validated occurrence, including a valid token after an invalid
prefix and multiple token families in one string.

## Filesystem operations

Managed guide, graph, session, IPC-lock and writer-lock paths use
`devmap-extract::safe_fs`. Directory traversal and leaf opens refuse symlinks
and Windows reparse points. Writes require regular files with one hard link;
Unix writes also require current-user ownership. Existing guide markers retain
their ownership semantics. Explicit ordinary output paths outside the
repository remain supported.

The final database filename may be a file alias, preserving one canonical
database and writer lock for its spellings. Linked database ancestors are
refused before canonicalization, including MCP repository selection and cache
lookup. WAL/SHM files are checked separately. Permission repair opens and
checks the database and sidecars first, recognizes the database schema, and
adds only the owner's write bit through an owned file handle. Unsupported or
unrecognized stores cannot authorize repair.

Atomic graph publication uses exclusive temporary-file creation and bounded
collision/replacement retries. A preexisting temporary name is neither written
nor removed. IPC runtime ownership is established before making the directory
private or opening its lock. Explicit IPC parent directories retain their
permissions. Session logging failures warn without failing the MCP query;
session reads and rotation return errors for unsafe or unreadable state.

Source reads are separate from managed writes. An in-repository source alias
is supported when its canonical target remains inside the selected repository.
Reads then open that target without following links, require a regular file,
and check identity and metadata around a bounded read. Parent traversal and
outside targets are refused. Stored source spans and graph line coordinates
also require the indexed content hash; changed or unreadable source produces
unavailable evidence and unknown line coordinates.

Go untracked diff collection uses rooted descriptor access and the existing
read policy, preserves native filenames, and refuses links, special files,
secret paths, or a collection exceeding 4096 files / 8 MiB. A refusal is an
explicit diff failure, not an empty successful diff.

`devcouncil integrate` reads and writes host configs (`.mcp.json`,
`.cursor/`, `.codex/`) by repository-relative name inside an `os.Root`, using
the same no-follow reader and exclusive writer as `integrate uninstall`: a
symlink at any component is refused rather than followed, and the read is
bounded at 1 MiB. A link at one of those names is left as found, so an outside
file's contents can no longer be merged into a repository config.
`safefile.WriteAtomic` is not a containment boundary: it resolves its parent
through the operating system and guarantees no-follow for the leaf only, so
callers whose path
components come from repository content validate ancestors themselves or hold a
root. The DevMap CLI bounds every host config it inspects at 1 MiB and requires
a regular file, and publishes the Codex `config.toml` through the same atomic
`safe_fs` writer as its JSON siblings.

These checks protect against hostile repository paths and files. They are not
an operating-system sandbox against a privileged actor or another process
already running with the same user's authority. Cross-platform runtime
qualification must be recorded separately from compilation or macOS tests.

## Resource and result contracts

| Surface | Bound and refusal behavior |
|---|---|
| Query source | At most 1 MiB per source; special files cannot block an open |
| API route scan | At most 5000 attempted files, 1 MiB per file, 64 MiB aggregate source, and 2000 retained sites; caller budgets may reduce these limits |
| Handler source analysis | Reuses the route scan's source cache; at most 64 MiB of source charged to distinct handler analyses |
| Session state | Reads capped at 8 MiB; appends check the current log size and a 16 KiB record limit; report directory inventory capped at 4096 entries |
| Resolver ambiguity | Defaults: 4,000,000 candidate visits, 32 MiB retained candidate bytes, 128 MiB conservative repeated JSON evidence bytes |
| Cache directory marker | Only the required 43-byte signature is read; trailing comments remain valid; unreadable/special markers report unexamined coverage |
| Nested parsing | Heritage names, unary callee unwrapping and enclosing callable names use iterative walks that honor the extraction deadline |
| Hook root selection | At most 8 acted-on roots from at most 256 examined payload candidates; reaching either is reported, not trimmed silently |
| Hook SessionStart | 10 s total across every child, each run through the kernel's bounded process boundary with a 64 KiB output cap; a partial pass names how far it got |
| `dcjsoncheck` | Manifest stdin capped at 1 MiB; per-invocation stdout/stderr capture capped at 8 MiB, and exceeding it fails that invocation rather than truncating its evidence; each invocation runs under a wall-clock deadline (default 120 s, `--deadline-secs N`) and a command still alive at it is killed and reported as failed |

The session append size check is not a global reservation across concurrent
writers. The source and resolver limits describe accounted bytes, not a precise
whole-process RSS ceiling. `dcjsoncheck`'s manifest and commands are the
operator's, not repository content.

One bounded runner serves every process boundary in the Rust workspace, and it
lives in `dc-proc` so the analysis plane can use it without compiling
tree-sitter grammars — `devmap_extract::subprocess` is a re-export of that one
implementation, not a second copy. Its properties are the ones a caller cannot
reliably assemble by hand: a wall-clock deadline, a kill that reaches the
child's whole process group, and both pipes drained concurrently on their own
threads. That last one is load-bearing rather than tidy: a reader that takes
stdout to EOF before touching stderr deadlocks the moment a child fills the
stderr pipe, so hand-rolled capture is not a supported substitute.

Route scan results carry attempted/read/skipped/unreadable counts and source
bytes. `scan.complete` is false after unreadable, oversized, budget-skipped or
unavailable-handler input. Completeness applies to files named by the graph,
not every file in the working tree. Missing handler evidence produces
`handler_keys: null`, `handler_keys_available: false`, a
`handler_source_unavailable` shape verdict and unknown impact risk. Pattern
matches remain regex evidence, not proof of an API's runtime response.

For Rust embedders, `Resolver::resolve_all` now returns
`Result<ResolutionResult, ResolutionLimitError>`; propagate the error before
analysis or publication. `resolve_all_with_limits` permits explicit budgets.
Repeated ambiguity shares immutable `Arc<[(String, String)]>` candidate data;
the serialized candidate array is unchanged. Limit exhaustion refuses the
whole result instead of sampling candidates and publishing a partial graph.
`is_cache_directory` now returns `io::Result<bool>`, and
`CacheVerdict::Unreadable` must not be treated as outside a cache directory.

Compact graph readers reject repeated structural keys instead of decoding an
ambiguous layout. Both graph HTML renderers escape the composed tooltip label
before handing it to the browser's HTML renderer. `escape::html_escape` is the
single escape sink for generated artifacts; the graph renderer no longer keeps
a private copy that omitted the apostrophe. The flag-filter control is built
from DOM nodes rather than composed markup, so its element id and the id read
back by `getElementById` cannot disagree.

The human-readable CLI reports absent scan coverage as unreported rather than
as complete, matching how the library reads the same field.

## Verification

The project gate is `npm run ci:local`; `rust/verify.sh` adds release worker,
determinism and resource checks beyond its quick mode. Security regressions
live beside their owners (`*_security` tests, `renewal_identity`,
`complete_secret_spans`, `ambiguity_budget`, and `nested_source_limits`), and
in Go beside `integrate_write_security_test.go` and `proc/lookpath_test.go`. Browser
tooltip tests require the explicit browser environment documented in
`rust/devmap-query/tests/tooltip_security.rs`. Child-process tests marked
ignored are invoked by bounded parent tests; running the children alone does
not provide their timeout or stack controls.

Passing these checks establishes the tested boundaries. It does not establish
that every repository file was audited or that every deployed build contains
the fixes.

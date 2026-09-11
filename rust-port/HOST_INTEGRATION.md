# Dev Map host integration contract

Dev Map has two supported host seams. Rust programs can link the read-only
query crate. Programs in other languages can invoke `devmap` and consume its
JSON contract. A host chooses the seam in configuration; neither requires a Python runtime.

## Rust module

Add one dependency and construct `StoreQueryEngine` around a checked store:

```toml
devmap-query = { path = "../DevCouncil/rust-port/crates/devmap-query", default-features = false }
```

```rust
use devmap_query::{paths, StoreQueryEngine};
use devmap_query::devmap_store::Store;

let repository_root = std::path::Path::new("/path/to/repository");
let store_path = paths::store_path(repository_root);
let store = Store::open_read_only(store_path)?;
let queries = StoreQueryEngine::new(&store);
```

`default-features = false` removes the parser and grammar frontend. A host that
also builds maps enables the default `parse` feature. `devmap-query` owns its
HTML renderer asset, so vendoring the crate does not require copying or
rewriting paths into another language tree. Always use
`paths::store_path(repository_root)` instead of appending `.devmap` or
`.devcouncil`: the resolver honors `DEVMAP_HOME`, an existing standalone
layout, and the legacy DevCouncil layout in their documented precedence.

`open_read_only` requires an existing current-schema store and never creates,
migrates, or heals it. Writer processes continue to own schema migration.


Query `Response<T>` envelopes carry `source_freshness: null` when whole-tree
freshness was not checked. Snapshot completeness (`walk_incomplete`, counts,
and truncation) does not prove that the working tree still matches the map.
`Store::status` and executable status expose independent nullable
`source_freshness` and `analyzer_freshness` checks. True means verified current,
false means a mismatch, and null (or an absent additive field in an older
reader) means unverified. A parser-free library verifies current source bytes
while leaving parser/analyzer compatibility unverified. Aggregate `is_fresh`
requires both checks to pass, a committed generation, and no pending work or
store degradation. Keep the reason even when persisted navigation is usable.

Search reads verify each returned file against the content hash in the symbol's
own generation. Changed or unreadable bytes leave the stored hit present with
an empty `source_span` and an explicit `source_unavailable_reason`. Preserve
that reason in host adapters. Reads are bounded to 1 MiB per source file;
preview source files, candidate strings/stdin, and PDG input use the same
ceiling. Preview content paths must be regular files.

Hosts that read the JSON artifacts directly should use the checked provider
rather than open-coding a file read:

```rust
use devmap_query::host::{ArtifactProvider, FilesystemArtifactProvider};
use devmap_query::viz::VizOptions;

let artifacts = FilesystemArtifactProvider::for_repo(repository_root);
let graph = artifacts.read_code_graph()?; // validated Value for a native canvas
let html = artifacts.code_graph_html(&VizOptions::default())?;
```

The same provider exposes `read_repo_map`, `repo_map_payload`, and
`repo_map_html`. It resolves `.devmap` versus `.devcouncil` through
`devmap_query::paths`, rejects missing, non-regular, malformed, oversized, or
incomplete artifacts, and accepts the older versionless graph shape. The
128 MiB ceiling can be lowered with `with_max_bytes`; zero or a value above the
ceiling is an explicit configuration error. `ArtifactProvider` is the narrow
replacement seam for a cache or IPC-backed module: implement `load_artifact`,
and its checked default helpers apply schema and shape validation. A custom
override of those helpers must preserve the same contract.

The filesystem provider rejects a stable symlink, directory, or FIFO before it
opens the path and rechecks the opened handle and byte count. Standard Rust has
no portable atomic no-follow plus nonblocking open, so this does not claim to
close a hostile path-swap race when another process can rewrite the state
directory concurrently.

## Process module

The host supplies three values: binary path, repository root, and per-command
budget. Probe the binary and store before every new process lifetime:

```text
(cd <root> && devmap --json status)
```

Running with the repository root as the working directory and omitting `--db`
uses the same canonical resolver as the Rust module. A host that supplies an
explicit database path must obtain it from `paths::store_path(root)` through
its Rust adapter or configuration; it must not reconstruct the state directory
name itself.

`host_contract_version` is `1`. A v1 status has these compatibility fields:

| Field | Meaning |
|---|---|
| `binary_version` | Executable package version. |
| `schema_version` | Store schema, or `null` when no store exists. |
| `expected_schema_version` | Store schema this executable reads. |
| `schema_relation` | `missing`, `current`, `foreign`, `newer`, `upgradeable`, or `unsupported`. |
| `reader_ready` | The executable can safely open the store for reads. |
| `query_ready` | `reader_ready` and a committed generation exists. |
| `generation_id` | Latest committed generation, or `null`; a query host requires a positive integer. |
| `db_path` | Resolved store path; a query host requires a nonblank value. |
| `is_fresh` | A committed generation has no pending work and current source inventory, content hashes, and analyzer payload identity were verified. Independent of schema readiness and extraction completeness. |
| `source_freshness` | Current source inventory and bytes match the stored generation; `null` if unverified. |
| `analyzer_freshness` | Stored parser/analyzer identity matches this binary; `null` for a reader unable to compare it. |
| `degraded_reason` | Coverage, freshness, or compatibility problem; `null` only when none is known. |
| `capabilities` | Operations and flags derived from the executable's command parser. |

Proceed only when `host_contract_version == 1`, `reader_ready == true`, the
required capability is `true`, and `query_ready == true` for a query. Treat an
unknown contract version, missing field, `null`, and `false` as unavailable.
When `query_ready` is true, also require a positive integer `generation_id` and
a nonblank `db_path`; this keeps a malformed status response from reaching the
query path merely because its readiness bit was set.
`schema_relation == "upgradeable"` means an explicit `devmap build` can migrate
the store. `foreign`, `newer`, and `unsupported` must not be opened or rewritten
by that executable.

Supported query invocations:

```text
devmap --json --db <db> search <query> --budget <tokens>
devmap --json --db <db> explore <query> --limit <n> --budget <tokens> --depth <n> --min-confidence <0..1>
devmap --json --db <db> impact <target> --budget <tokens> --depth <n>
devmap --json --db <db> trace <from> [to] --budget <tokens> --depth <n>
devmap --json --db <db> affected <target>... --budget <tokens> --depth <n> --min-confidence <0..1>
```

The host must preserve `shown`, `total`, `hidden`, `truncated`, `tokens_used`,
`resolution`, and `walk_incomplete` instead of reducing a response to its item
list. Their absence is not proof of complete coverage.

`--json` writes exactly one JSON line to stdout. An operational failure exits
nonzero and writes `{"error":"..."}` on stdout plus a diagnostic on stderr.
`status` exits successfully for a missing or incompatible store because its
structured fields are the compatibility result. Hosts must check readiness,
not process status alone.

The executable has no wall-clock timeout flag. A process host must impose a
killable deadline and bounded stdout/stderr capture. Kernel `--budget`,
`--depth`, `--limit`, and target-count ceilings bound result and traversal work;
they do not replace a process deadline. Configuration should expose the binary,
root, database path, budget, depth, timeout, and output-byte ceiling so an app
can replace or tune the module without source edits.

## HTML module

Render a self-contained offline graph page:

```text
devmap --json --db <db> html <root> --out <path> --level files --max-nodes <n>
```

Use `--level symbols` for symbol relationships. The JSON result reports
`output`, `bytes`, `level`, and payload `counts`; the page embeds its renderer
and makes no network request. Retain shown and total node/link counts in the
host UI so a cap cannot become a coverage claim.

## Marker inventory coverage

The repository map keeps marker coverage separate from extraction and graph
coverage. `package_managers_computed` and `test_commands_computed` say whether
an inventory ran; also require `inventory_complete` before treating either
list as complete within the documented marker policy.

`inventory_source` names `git` or `filesystem`. Git lists tracked and unignored
paths at any depth, bounded by the existing 30-second/64-MiB subprocess limits,
then examines at most 50,000 eligible paths with a cooperative five-second
metadata deadline. `inventory_files_total` is the pre-cap eligible path count;
`inventory_entries_examined` counts paths actually examined. Non-Git fallback
has an eight-level depth ceiling, 20,000 opened/pending directories, 200,000
entries and the same cooperative deadline; its total is unknown (`null`).
Neither cooperative deadline can interrupt an OS filesystem call in progress.

Both apply the existing marker exclusions: dot-directories, dependency/build
output directories, and root-only output names. An excluded tree is outside
this inventory's scope. `inventory_walk_truncated` reports a bound hit.
`inventory_unreadable` holds at most 64 observed failures and
`inventory_unreadable_count` retains their count. Unreadable directories,
malformed JSON, and unreadable manifest bytes cannot claim completeness.
`inventory_refused_oversize` names manifests beyond the 256-KiB read limit.
No marker completeness verdict certifies arbitrary language semantics or an
atomic snapshot of a concurrently changing filesystem.

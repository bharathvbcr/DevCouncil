# Dev Map host integration contract

Dev Map has two supported host seams. Rust programs can link the read-only
query crate. Programs in other languages can invoke `devmap` and consume its
JSON contract. A host chooses the seam in configuration; neither requires
changes to DevCouncil's Python package.

## Rust module

Add one dependency and construct `StoreQueryEngine` around a checked store:

```toml
devmap-query = { path = "../DevCouncil/rust-port/crates/devmap-query", default-features = false }
```

```rust
use devmap_query::{paths, StoreQueryEngine};
use devmap_query::devmap_store::Store;

let store_path = paths::store_path(repo_root);
let store = Store::open_read_only(store_path)?;
let queries = StoreQueryEngine::new(&store);
```

`default-features = false` removes the parser and grammar frontend. A host that
also builds maps enables the default `parse` feature. `devmap-query` owns its
HTML renderer asset, so vendoring the crate does not require copying or
rewriting paths into DevCouncil's Python package.

`open_read_only` requires an existing current-schema store and never creates,
migrates, or heals it. Writer processes continue to own schema migration.

Hosts that read the JSON artifacts directly should use the checked provider
rather than open-coding a file read:

```rust
use devmap_query::host::{ArtifactProvider, FilesystemArtifactProvider};
use devmap_query::viz::VizOptions;

let artifacts = FilesystemArtifactProvider::for_repo(repo_root);
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
devmap --json --db <resolved-db> status
```

Resolve `<resolved-db>` with the same state-directory precedence as
`devmap_query::paths::store_path`: an existing `.devmap` wins, then an existing
legacy `.devcouncil`, otherwise a fresh host defaults to `.devmap`.

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
| `is_fresh` | No known pending update remains. Independent of schema readiness. |
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

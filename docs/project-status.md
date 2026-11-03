# Project status

The current source is a native Go/Rust component suite. The Python CLI and
its orchestration surfaces are retired. This page describes implementation
availability; it does not certify every platform, editor session or release.
[Documentation index](README.md)

## Current surfaces

| Surface | Implementation | Practical boundary |
|---|---|---|
| Code intelligence | Rust `devc-map` (`dc-map` / `devmap`) pipeline, CLI, 21-tool MCP, watcher, `doc-drift`, 11-tier resolution ladder (`rung`), Blake3 CAS AST store (`cas`), per-worktree delta overlay (`overlay`), and `devmap-store::csr` zero-copy `mmap` CSR storage | Static evidence (schema 98); store schema 29 (reader floor 29); resolution budgets; damaged store refusal; queries can be capped |
| Go host | `devc` / `devcouncil` / `dev` CLI (`spec`, `workflow`, `search`, `task`, `verify`, `review`, `integrate`, `skills`) and eight-tool task MCP | Task tooling, 7-layer specification lifecycle, resumable DAG workflows, unified search routing, integration, skills and verification; no autonomous model loop |
| Task state | Rust `devc-store` (`dc-store` / `dcstore`) | Cooperating-client leases (schema 11, partial unique index `ux_task_leases_active`), workbench tasks (schema 14), constitution gates (`constitution_gates`), and durable records |
| Verification | Go task checks, secondary judge model verification (`verify/secondary_model.go`), plus Rust `devc-verify` (`dc-verify` / `dcverify`) rigor | Deep scans (untracked, submodules), opaque stubs, prompt-input write protection, CDATA prompt-injection sanitization, no diff/panic secret echo |
| Search | Go `devcouncil search` router + Rust `devc-grep` (`dc-grep` / `dcgrep`) & `devc-map` (`devmap`) | Unified routing across symbol/structural (`devmap`) and literal/regex (`dcgrep`); ignore-aware search and optional trigram index; decoupled ceilings, 64 MiB lexical bound, completeness contract |
| Subprocess & load control | Rust `dc-proc`, Go `proc`, and Go `devcouncil/scheduler` | 5 s default deadline, `/dev/null` stdin, process-group `SIGTERM`/`SIGKILL` escalation and daemon reaping; load-aware non-blocking FIFO job scheduler for heavy builds/benchmarks |
| Host adapters | Six hosts, one Rust table (`devmap integrate`); the Go host passes its server over `--servers-stdin`; Antigravity bundle (`antigravity/`) | Different outputs by host; Codex gets `[mcp_servers.devcouncil]` merged into the project `.codex/config.toml` without rewriting other bytes; Antigravity ships MCP bridge, 5 lifecycle hooks, 3 sidecars, and skills |
| Engineering skills | Embedded/scaffolded instructions | Guidance, not enforced permission or proof of host loading |
| Sandbox selector | `--sandbox local` only | Anything else is refused (`verify.ParseSandbox`, exit 2); Docker/Nix isolation is not implemented; building it is planned (owner decision 2026-10-07, task `dc-verify-isolated-sandbox`) and is gated on an approved security-impact statement |

Native source versions are declared independently: currently 0.2.5 in the
Rust workspace (`rust/Cargo.toml`), the Go host
(`backend/go_orchestrator/devcouncil/version/version.go`) and the npm launcher
(`package.json`). These are source
values, not proof of publication. Inspect `devmap --version`,
`devcouncil --version` and `devmap paths --json` for installed identity.
Old Python 0.4.x release notes belong to a different runtime lineage.

## What moved or retired

| Historical surface | Current direction |
|---|---|
| Python CLI / uv install | Native source installers and optional npm launcher |
| `dev plan`, `approve`, `go`, `run`, `e2e`, `campaign` | Agent/harness orchestration; use Manvi or the consuming agent |
| Python `dev map` engine | Go forwarding to native `devmap`; no Python build fallback |
| `dev check --verify` | `devcouncil verify TASK_ID --mode enforce --json` for existing tasks |
| Wiki / OKF / dashboard / corpus commands | Retired; managed guides and code navigation cover related needs, not full feature parity |
| Provider routing, advisor setup, debate roles | Configure in the consuming harness |
| DevCouncil lifecycle write/stop hooks | Retired no-op compatibility and cleanup controls |
| Old MCP file-write, shell, scope-update and rollback tools | Absent from the eight-tool Go host |
| Python SCA and GitHub Checks writers | No equivalent Go verification surface claimed here |

## Evidence and open work

The [native migration ledger](https://github.com/bharathvbcr/DevCouncil/blob/b18cea3f23d2df9b3ba9d995a8d7b183aafa923e/docs/PHASE7_LONG_TAIL.md), [task ledger](https://github.com/bharathvbcr/DevCouncil/blob/b18cea3f23d2df9b3ba9d995a8d7b183aafa923e/docs/TODO.md),
[Rust status](../rust/STATUS.md), and dated audit reports retain detailed
qualification history. Old fixture counts and “certified” Python workflows
must not be reused as certification of the current task MCP loop.

[Benchmark reports](devmap/comparison.md) are frozen measurements of named
executables, corpora and interfaces. A historical benchmark is not a current
release claim or a measure of overall agent task success.

For current commands use [CLI reference](cli-reference.md), installed help and
the live MCP catalog. For setup use [quickstart](quickstart.md). For a host
integration, validate files, reload the host, and separately qualify execution
on the intended platform.

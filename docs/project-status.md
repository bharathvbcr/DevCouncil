# Project status

The current source is a native Go/Rust component suite. The Python CLI and
its orchestration surfaces are retired. This page describes implementation
availability; it does not certify every platform, editor session or release.
[Documentation index](README.md)

## Current surfaces

| Surface | Implementation | Practical boundary |
|---|---|---|
| Code intelligence | Rust `devmap` pipeline, CLI, MCP and watcher | Static evidence; freshness and coverage are separate; queries can be capped |
| Go host | `devcouncil` / `dev` CLI and eight-tool task MCP | Task tooling, integration, skills and verification; no autonomous model loop |
| Task state | Rust `dcstore` | Cooperating-client leases and durable records, not arbitrary filesystem isolation |
| Verification | Go task checks plus Rust `dcverify` rigor | Opt-in mode; skipped checks and missing profiles must be read explicitly |
| Search | Rust `dcgrep` | Ignore-aware search and optional trigram index; text matches are not semantic edges |
| Host adapters | Six names shared by Go and Rust installers | Different outputs by host; Go's Codex task adapter remains comment-only while Rust installs DevMap MCP |
| Engineering skills | Embedded/scaffolded instructions | Guidance, not enforced permission or proof of host loading |
| Sandbox selector | Accepted by Go verification | Records selection; Docker/Nix isolation is not implemented |

Native source versions are declared independently: currently 0.2.2 in the
Rust workspace and Go host, with the npm launcher at 0.2.1. These are source
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

The [native migration ledger](PHASE7_LONG_TAIL.md), [task ledger](TODO.md),
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

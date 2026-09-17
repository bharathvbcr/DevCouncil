# DevCouncil documentation

Native components for code intelligence, task state, verification and search.
Start with standalone DevMap; add the Go host and other Rust components when
needed. These guides describe the current native source, not the retired Python
CLI. [Project overview](../README.md) · [Website](https://devcouncil.vbcr.dev/)

## Start here

| I want to… | Read |
|---|---|
| Install and map my first repository | [Quickstart](quickstart.md) |
| Connect an editor or coding agent | [Coding CLI integration](coding-cli-integration.md) |
| Find symbols, callers, impact and candidate tests | [Code graph guide](code-graph.md) |
| Search text exactly, or rank files by what they are about | [Ranked search](lexical-search.md) |
| Choose a binary, library or host boundary | [Architecture](architecture.md) |
| Use tasks, leases and verification | [Task workflow](workflow.md) and [MCP task loop](hero-loop.md) |
| Look up commands and flags | [CLI reference](cli-reference.md) |
| Understand limitations and retired commands | [Project status](project-status.md) |

## Evidence and operations

- [DevMap comparison](devmap/comparison.md): recorded competitor measurements and limits.
- [Repository input and verification boundaries](SECURITY_BOUNDARIES.md): executable selection, filesystem handling and unavailable evidence.
- [Repository hygiene](repository-hygiene.md): generated state, caches and cleanup ownership.
- [Security overview](security.md): security documentation and disclosure context.
- [Model routing boundary](model-routing.md): configure models in the consuming harness.
- [Rust workspace](../rust/README.md): native engine development and verification.
- [Native migration follow-ups](PHASE7_LONG_TAIL.md) and [task ledger](TODO.md).

## Reading status correctly

Source inspection establishes implementation and call paths. Tests qualify the
cases they exercise. Neither alone certifies an editor session, every platform,
or a live release. Query results can be fresh and still have coverage gaps.
Verification can return a result with checks skipped; inspect that metadata.

Files under [archive](archive/), dated audit reports, and release notes retain
historical evidence. Prefer the guides above for current setup. In particular,
old Python versions and old benchmark binaries are not the native runtime's
current version. Resolve installed identity with `devmap --version` and
`devcouncil --version`; source versions live in `rust/Cargo.toml`, the Go host's
version declaration, and `package.json` for the separate npm launcher.

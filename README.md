# DevCouncil

Code intelligence and verification components for AI development.

[![Website](https://img.shields.io/badge/website-devcouncil.vbcr.dev-10B981?style=flat&logo=safari&logoColor=white)](https://devcouncil.vbcr.dev/)
[![CI](https://github.com/bharathvbcr/DevCouncil/actions/workflows/ci.yml/badge.svg)](https://github.com/bharathvbcr/DevCouncil/actions/workflows/ci.yml)
[![npm version](https://img.shields.io/npm/v/devcouncil)](https://www.npmjs.com/package/devcouncil)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

<p align="center">
  <a href="https://devcouncil.vbcr.dev/"><strong>Explore the Interactive Architecture &amp; Code Graph Showcase (devcouncil.vbcr.dev) &rarr;</strong></a>
</p>

> **"DevCouncil should not merely generate code. It should make AI-generated work prove that it satisfied the original intent."**  
> *Evidence, not model confidence, is the final authority.*

DevCouncil is **components and modules** for AI-assisted software development. It ships independently installable binaries and libraries — code intelligence (`devmap`), task and lease state (`dcstore`), deterministic verification (`dcverify`), ignore-aware search (`dcgrep`), and a Go host (`devcouncil` / `dev`) — so a harness or an app can take only the pieces it needs and update them one at a time.

See [repository input and verification boundaries](docs/SECURITY_BOUNDARIES.md)
for executable selection, filesystem protections, resource limits, and the
difference between unavailable evidence and a passing check.

**Manvi wraps these components** into a coding-agent harness (turn loop, providers, policy, TUI, `manvi serve`). **GitPulse** uses Manvi for policy, workbench, and agent hosting, and DevCouncil components for code intelligence and related analysis. DevCouncil does not replace coding agents; it sits beside Claude Code, Codex, Cursor, and others as selectable modules.

---

## Architecture & Components

DevCouncil's core runtime is a set of standalone, compiled native modules. Each binary is useful on its own, sits on `PATH`, and can be spawned or linked by a host that wants that one job:

| Binary | Language | Role & Ownership |
|--------|----------|------------------|
| `devcouncil` / `dev` | **Go** | Host orchestrator: Stdio MCP server (`mcp`), multi-host agent integration (`integrate`), engineering skills distribution (`skills`), task verification (`verify`), and devmap forwarding (`map`, `graph`, `ast`). |
| `devmap` | **Rust** | Code intelligence engine: Language-aware tree-sitter extraction and explicit fallback/coverage reporting, symbol resolution, blast-radius calculation, dead-code analysis, workspace guides (`AGENTS.md`), and DevMap MCP server (`devmap mcp`). |
| `dcstore` | **Rust** | State & lease store: SQLite-backed task repository, mutual-exclusion leases for concurrent agent building, evidence records, and gap tracking. |
| `dcverify` | **Rust** | Deterministic verification: Unified-diff parsing, planned file scope classification, anti-laziness/stub gates, test coverage evaluation, and typed `next_actions` repair signals. |
| `dcgrep` | **Rust** | Code search: Ripgrep-powered ignore-aware search engine with optional trigram indexing (`tgrep-core`). |

### How hosts consume the modules

- **Manvi** wraps the components: it drives the agent loop, provider routing, policy ladder, and TUI, and reaches each binary over JSON on stdio. A host that wants a harness embeds Manvi rather than reimplementing it.
- **GitPulse** uses both for their respective jobs: Manvi (`manvi serve`) for policy, workbench, and agent hosting; selected DevCouncil crates and the `devmap` CLI for in-process code intelligence. It does not have to take every module.
- **Other apps** pick the same way — `devmap` alone, `dcverify` alone, the Go host MCP, or the full set. Updating one module does not require shipping the rest.

> [!NOTE]
> DevCouncil is built entirely as native compiled Go and Rust binaries; legacy Python orchestration has been retired (see [docs/PHASE7_LONG_TAIL.md](docs/PHASE7_LONG_TAIL.md)).

---

## Benchmarks: DevMap vs Graphify, Gortex, GitNexus, CodeGraph, and codebase-memory-mcp

The table below records historical DevMap 0.2.1 build `48cd3c7` measurements,
not a fresh benchmark of the current native source. Each timing is a median
from a reproducible run with the raw
evidence committed beside it — not a marketing estimate. Six code-graph tools
plus a ripgrep text baseline indexed the **same frozen 1,186-file / 46.9 MB
repository** on one Apple M5 Pro, with 261 timed samples and per-tool
correctness checks. Head-to-head breakdown:
**[DevMap vs GitNexus, CodeGraph, Graphify, Gortex, and codebase-memory-mcp](docs/devmap/comparison.md)**.
Full method, caveats, and raw output:
**[benchmark report](benchmarks/results/competition/20260913-48cd3c7/REPORT.md)**.

| Tool | Cold index | Unchanged refresh | Single-file edit | Peak RSS | Caller pairs found |
|---|---:|---:|---:|---:|---:|
| **DevMap 0.2.1** | **2.031 s** | **107.1 ms** | 957.8 ms | **610 MiB** | **5/5** |
| CodeGraph 1.6.0 | 2.912 s | 241.9 ms | **417.2 ms** | 2438 MiB | 3/5 |
| codebase-memory-mcp 0.10.8 | 8.019 s | 6.160 s | 9.347 s | 1208 MiB | **5/5** |
| Graphify 0.9.59 | 16.456 s | 4.327 s | 3.945 s | 3293 MiB | 3/5 |
| GitNexus 1.6.9 | 34.102 s | 567.5 ms | 33.417 s | 3095 MiB | 3/5 |
| Gortex 0.64.3 | 38.835 s | 967.5 ms | 5.754 s | 2206 MiB | 4/5 |

**Where DevMap won.** Lowest cold-index median (**1.4× faster than CodeGraph**,
**16.8× faster than GitNexus**), lowest unchanged-refresh median, the lowest
median in **all six symbol-query cells** — definition lookups ran 30–33 ms,
**4.8–5.2× faster than CodeGraph** and **31–33× faster than GitNexus** — the
lowest sampled peak memory of any graph tool, and all five source-inspected
caller pairs.

**Where DevMap lost.** CodeGraph re-indexed a single edited file in 417 ms
against DevMap's 958 ms — **2.3× faster**, the one stage where a competitor is
consistently ahead. DevMap's 139 MiB store is also larger than Graphify's
42 MiB, CodeGraph's 63 MiB, and codebase-memory-mcp's 67 MiB.

**What these numbers are not.** One corpus, one machine, three-to-five
repetitions, three symbols, and five inspected caller pairs. They do not
establish general graph accuracy, persistent-MCP latency, or coding-agent task
success. Tool output scopes differ, so equal latency is not equal analysis.
The report states every limit explicitly and keeps the failures in — including
GitNexus retaining a deleted symbol through ordinary refreshes until a forced
rebuild.

---

## Get started

Build just DevMap from source, then map a project:

```bash
git clone https://github.com/bharathvbcr/DevCouncil.git
cd DevCouncil
bash scripts/install.sh --only=devmap
export PATH="$HOME/.local/bin:$PATH"

cd /path/to/your/project
devmap build --manifest --guides
devmap paths --json
devmap status --json
devmap explore MyFunction --json
```

Git and Rust/Cargo are required. To include the Go host and full analysis suite,
run `bash scripts/install.sh`; the host needs the toolchain declared in
[`go.mod`](backend/go_orchestrator/go.mod), currently Go 1.26.6. On Windows use
`.\scripts\install.ps1 -Components devmap`, or omit `-Components` for the suite.
The npm package is a lightweight launcher for native binaries, not a bundled
runtime. [Complete installation guide](docs/quickstart.md).

Use the paths returned by `devmap paths`: new repositories default to `.devmap`,
existing `.devcouncil` layouts remain supported, and explicit configuration can
override either. `--guides` requests managed workspace guides separately from
the map export. Check freshness, coverage gaps and result truncation before
using graph answers as evidence. If first-time guide creation leaves status
stale, run `devmap build --manifest` once more to index the new guides.

## Connect an agent

```bash
devmap integrate cursor --dry-run
devmap integrate cursor
devmap integrate cursor --check
```

Substitute `claude`, `codex`, `antigravity`, `opencode` or `warp` for `cursor`.
Integration can update user-level MCP settings as well as project assets.
Inspect the receipt and complete host reload/trust steps. For task tooling,
add the Go-host integration described in
[coding CLI integration](docs/coding-cli-integration.md).

## Verification is opt-in

For an existing task in an initialized `.devcouncil/state.sqlite`:

```bash
devcouncil verify TASK-001 --mode enforce --json
```

Read gate mode, skipped reasons and coverage metadata. The Go host invokes
`dcverify` for rigor checks; changed-line coverage requires a profile supplied
through `--coverage PATH`. The default gate mode is `off`. Task leases and MCP
policy results coordinate participating clients; retired lifecycle hooks do
not intercept arbitrary shell commands or editor writes. The sandbox selector
does not implement Docker/Nix isolation. See the
[task loop contract](docs/hero-loop.md).

## Documentation

Start at the **[documentation index](docs/README.md)**.

| Topic | Guide |
|---|---|
| Installation and first query | [Quickstart](docs/quickstart.md) |
| Components and state ownership | [Architecture](docs/architecture.md) |
| Symbols, impact and evidence limits | [Code graph](docs/code-graph.md) |
| Editor setup and hooks | [Integration](docs/coding-cli-integration.md) |
| Commands and flags | [CLI reference](docs/cli-reference.md) |
| Task workflow | [Workflow](docs/workflow.md) and [MCP loop](docs/hero-loop.md) |
| Implementation and migration limits | [Project status](docs/project-status.md) |
| Native development | [Rust workspace](rust/README.md) |

[Apache 2.0](LICENSE). The Python CLI, its release lineage and its historical
workflow certifications are retired; current native source and installed
binary identities must be checked separately.

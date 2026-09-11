# Phase 7 — long-tail Python retirement (2026-09-10)

Hard cut of remaining Python orchestration into `devmap` (Rust), `devcouncil` (Go),
and `dcstore` (Rust), with Manvi owning the agent loop / LLM / TUI. Prefer
**retire-with-decision** over half-ports for torch/faiss/GEPA/campaign-scale surfaces.

## Decisions

| Surface | Decision | Replacement | Rationale |
|---------|----------|-------------|-----------|
| `knowledge/` wiki + design lint + OKF bridge | **Retire** | Agent guides from `devmap build --guides`; design notes stay as prose in-repo | Wiki generator depended on retired Python graph communities; OKF was orchestration glue, not code intel (see DIVERGENCES EXPORT-4) |
| `reporting/` | **Retire** | `devcouncil verify --json`, MCP `devcouncil_get_gaps`, Manvi session logs | Markdown/JSON report writers duplicated Go verify shapes |
| `telemetry/` stages/cost traces | **Retire** | Manvi session NDJSON + `devmap session-report` | Python telemetry was CLI glue; no host depended on its schema |
| `campaign/` Director/Coordinator pool | **Retire** | Manvi multi-agent profiles / `manvi run` | Unused in product path; GEPA/campaign not useful enough to port |
| `live/` companion review | **Retire** | Manvi harness + MCP verify | Live review imported LLM + stop-gate; Manvi owns both |
| `ui/` dashboard | **Retire** | Manvi TUI / `manvi watch` | Textual dashboard was a second UI next to Manvi |
| `optimization/` GEPA + skillopt | **Retire** | none | Optional `gepa`/`torch` stack; not on the binary plugin contract |
| `semantic/` / `indexing/semantic_index` FAISS | **Retire** | `devmap search --semantic` (lexical name similarity; no torch) | Candidate replacement is DevMap; FAISS/sentence-transformers stay out of components |
| `src/semantic_layer/` (optional FAISS pipeline) | **Retire / delete** | same as above | Sibling package deleted with Phase 7; was only wired through retired Python semantic CLI |
| `codeintel` debug/DAP | **Retire** | none (opt-in tracer unused in product MCP) | Adjunct; DIVERGENCES Q7 already retired runtime-dead merge |
| `cli` `lsp` / `ast` Python matchers | **Retire / cut** | `devmap ast`; LSP cut per DIVERGENCES X11–X13 | No production MCP/CLI usage for LSP |
| `verification/` `execution/` `planning/` `gating/` `executors/` `llm/` | **Hard-cut delete** | Go `devcouncil verify` + stopgate/correction/gating packages; Manvi `llm/` + agents | Phase 5 ported shapes; callers were remaining Python CLI — retired below |
| `integrations/clients/*` | **Hard-cut delete** | `devcouncil integrate <host>` | Go owns host installers |
| `skills/registry.py` selection | **Hard-cut delete** | `devcouncil skills list\|scaffold` (embedded FS) | Goal-based selection retired; explicit `--skill` or scaffold-all |
| `indexing/` + `codeintel/` (non-adjunct) | **Delete** | `devmap` | Kernel sole writer since Phase 6 build cutover |
| `storage/` SQLModel | **Delete** | `dcstore` + Go `dc/store` | Schema already transcribed into dc-store |

## `dev` / `devcouncil` is the Go binary

The Python launcher is gone. `dev` and `devcouncil` are the Go host binary
(`backend/go_orchestrator/cmd/devcouncil`). The npm package ships a Node shim
that execs that binary (and `devmap` for `map` / `graph` / `ast`). There is no
Typer/Click/`uv run` path.

1. **Go** owns `mcp`, `integrate`, `skills`, `verify`, and forwards `map`/`graph`/`ast`
2. **Rust `devmap`** owns the map engine; a bare `dev map` is `devmap build --manifest`
3. Retired command names exit 2 from Go (`unknown command`) rather than a Python retirement table

Install: `bash scripts/install.sh` (or `.\scripts\install.ps1` on Windows) builds the Go
binary into `~/.local/bin` and symlinks/copies it as `dev`. Pass `--only=devmap` or
`-Components devmap` for the standalone code-intelligence CLI. If that directory is not on
`PATH`, the installer prints a note — add it, or invoke `~/.local/bin/devcouncil`
directly. `bash scripts/install-components.sh` installs `devmap` / `dcstore` /
`dcverify` / `dcgrep`. There is no `uv run` / `uv tool install` path. After the host
is on PATH, `devcouncil install --help` is the same catalog (`uninstall` / `disable` /
`enable` included). Verification gates default to `off`; `devcouncil gate set --mode
advisory|enforce` is a human operator command.

## Manvi ownership

- LLM provider routing, agent profiles, TUI/`watch`, campaign-like multi-agent runs:
  stay in Manvi — do not reimplement in DevCouncil Go.
- Manvi continues to import `backend/go_orchestrator` (policy/gate/devcouncil tools).
  Phase 7 does not change that module boundary.

## Closed ledgers

- `rust-port/CONSUMERS.md` Phase 5 remainders → deleted
- `rust-port/AGENT_PLAN.md` / `STATUS.md` Phase 6–7 consumer deletion → closed for host-assets plan
- Package READMEs describe `devmap` + `devcouncil` + `dcstore`

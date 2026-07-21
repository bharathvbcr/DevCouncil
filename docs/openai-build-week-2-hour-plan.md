# DevCouncil OpenAI Build Week: Two-Hour Score-Maximization Plan

## Objective

Ship the smallest set of changes that materially improves DevCouncil's OpenAI Build Week score and makes the exact judge path reliable. The target is a coherent **Developer Tools** submission with:

1. a fresh install that works without rebuilding the project;
2. one deterministic, visually legible demo path;
3. one credible Codex/MCP agent-control path;
4. explicit separation between pre-existing work and eligible Build Week extensions; and
5. a public, narrated video under three minutes.

The two-hour window is not for clearing general technical debt. It is for eliminating judge-visible failures and making the strongest existing capability easy to understand.

## Current evidence and constraints

- The published npm `0.4.0` package installs and its deterministic `dev check --verify` path passes on a clean repository.
- The released graph demo renders a blank canvas because the generated page calls a ForceGraph method that the bundled production library does not expose. A local fix and regression assertion already exist in `src/devcouncil/indexing/viz.py` and `tests/unit/test_graph_html.py`; the fixed generated artifact was browser-verified.
- `devcouncil_status` can return `cli_parse_error` on a real, high-volume project because CLI stdout is truncated to 20,000 characters before JSON parsing.
- The repository README now contains an explicit Build Week scope and judge path, but those edits are not committed or released.
- DevCouncil's own current dashboard state contains 267 gaps and two blocking gaps. Do not use that state in the video; use a controlled sample repository.
- Preserve unrelated worktree edits in `.devcouncil/config.yaml` and `tests/performance/benchmark_harness.py`. Never use `git add -A` for the submission patch.

## Ranked bugs and improvements

| Priority | Bug or improvement | Judge/score impact | Estimate | Decision |
|---|---|---:|---:|---|
| P0 | **Release the fixed graph demo.** npm `0.4.0` still contains the blank-canvas failure. | Very high: functionality + design | 15 min plus CI | Do now |
| P0 | **Complete Devpost and the public narrated video.** The project is still an Untitled pre-draft. | Submission blocker; all criteria | 45-60 min human time | Do now, parallel with code |
| P0 | **Add a provider-free red-to-green core demo.** The current judge path proves the graph side feature, not the central evidence gate. | Very high: technical + coherence | 45-60 min in parallel | Do now |
| P0 | **Publish a version containing the submission fixes.** README judge instructions must match the installable package. | Very high: installability and credibility | 15 min plus CI | Do now |
| P1 | **Fix lossless MCP JSON transport.** `status`, task, prompt, gaps, next-actions, and task listing all fail once valid structured output exceeds 20 KB. | High: core agent-plugin reliability | 45-60 min | Do now in parallel |
| P1 | **Extend the npm runtime smoke to generate and inspect the graph artifact.** Current packaging smoke proves install/help/doctor/integrations but not the visual demo judges are told to open. | High: prevents release regression | 10-15 min | Do now |
| P1 | **Rewrite the README's first screen around the Build Week submission.** It is 4,819 words and currently names Claude as the primary path before the eligible-work story. | High: coherence + impact | 20-30 min | Coordinator only |
| P1 | **Add one strong screenshot/thumbnail.** Use the repaired graph or a clean evidence report, not the noisy dashboard. | Medium-high: design score | 10-15 min | Do if video upload is progressing |
| P1 | **Prevent task-scoped diff from falling back to the full repository.** Missing DB/task scope currently returns unrelated work; explicit paths can broaden scope. | High: trust/core correctness | 15-20 min | Runtime-agent stretch after JSON fix |
| P1 | **Rerun the isolated public Windows Node-DAP failure.** The two preceding same-commit runs passed, so confirm the suspected flake before touching production code. | Medium: public confidence | 5 min action, about 4 min CI | Do while agents work |
| P1 | **Remove stale SVG-preview claims.** The graph demo is now interactive HTML-only. | Low-medium: coherence | 5-10 min | Coordinator quick fix |
| P1 | **Include untracked files in MCP `get_diff`.** Today an untracked new file returns `ok: true` with an empty file list and diff. | High: core evidence correctness | 35-50 min | Dedicated parallel agent |
| P2 | Add a concise dashboard "demo mode" or summary cards. | Medium design upside, moderate regression risk | 35-50 min | Only if every P0/P1 gate is green |
| P2 | **Make MCP task listing compact.** Five tasks currently serialize about 168 KB because full planned-file models are returned. | Medium: agent UX/context cost | 20-30 min | Bundle with JSON transport if time |
| P2 | Fix staged rename parsing and surface nonzero Git subprocess results in `get_diff`. | Medium reliability | 25-35 min | Defer unless combined with untracked fix |
| P2 | Add a true browser automation test for the graph canvas. | Medium regression protection | 30+ min | Defer unless existing browser harness makes it trivial |
| P3 | Repair the visible scheduled Windows Node-DAP attach flake. | Low direct judge impact | Unknown | Defer |
| P3 | Clear DevCouncil's 267 historical dogfood gaps or rewrite the dashboard. | Low score per minute | Hours | Do not attempt |
| P3 | Revisit watcher leadership, graph-metric precision, import cycles, or broad architecture debt. | Valuable after the hackathon | Multi-hour/day | Do not attempt |

## Parallel ownership

The lanes intentionally avoid overlapping files.

### Lane A: MCP reliability agent — 0:00 to 1:00

**Owns:**

- `src/devcouncil/integrations/mcp/util.py`
- `src/devcouncil/integrations/mcp/handlers/status.py`
- focused MCP status/contract tests

**Tasks:**

1. Add a regression test whose valid CLI JSON exceeds 20,000 characters and prove `devcouncil_status` still returns the phase/summary.
2. Make the internal CLI transport lossless. Apply `truncate_text` only at external raw-text boundaries such as generic `devcouncil_cli` and report previews.
3. Route structured handlers through the existing `parse_cli_json` helper instead of handler-local `json.loads` copies.
4. Add live-sized regressions for status/task/gaps; current outputs are approximately 28 KB, 45 KB, and 142 KB respectively.
5. If time remains, compact `list_tasks` to ID/title/status/priority/requirements/lease and fix task-scoped diff fail-closed behavior. Keep `get_task` as the detail endpoint.

**Acceptance:**

```bash
./.venv/bin/pytest -q tests/unit/test_mcp_server.py tests/unit/test_mcp_contract.py tests/unit/test_mcp_closed_loop.py
./.venv/bin/ruff check src/devcouncil/integrations/mcp/util.py src/devcouncil/integrations/mcp/handlers/status.py tests/unit/test_mcp_server.py tests/unit/test_mcp_contract.py
```

Live acceptance: `devcouncil_status` on this repository returns `Phase: TASK_VERIFIED` instead of `cli_parse_error`.

### Lane B: MCP diff-correctness agent — 0:00 to 1:05

**Owns:**

- `src/devcouncil/integrations/mcp/handlers/git.py`
- `tests/unit/test_companion_mcp.py`

**Tasks:**

1. Add a regression proving an untracked text file appears in `files` and the bounded unified diff for the unstaged view.
2. Fail closed when `task_id` is supplied without an initialized database/task. Explicit `paths` must never broaden a task's planned-file scope.
3. Handle empty and path-filtered untracked files without weakening the existing 20 KB external diff cap.
4. If time remains, switch staged rename parsing to NUL-safe Git output and surface nonzero Git subprocess results as `ok: false`.

**Acceptance:**

```bash
./.venv/bin/pytest -q tests/unit/test_companion_mcp.py
./.venv/bin/ruff check src/devcouncil/integrations/mcp/handlers/git.py tests/unit/test_companion_mcp.py
```

### Lane C: provider-free demo agent — 0:00 to 1:00

**Owns:**

- `scripts/build-week-demo.sh`
- `examples/build-week-demo/**`
- `docs/build-week-demo.md`

**Tasks:**

1. Build a tiny calculator repository with one deliberately failing evidence gate.
2. Run the installed `dev check --verify` without API keys and show the blocking result.
3. Apply the real repair plus regression test and rerun to a compiled, zero-gap pass.
4. Leave the generated repository path visible so a judge can inspect it.
5. Keep runtime below 60 seconds after package installation.

**Acceptance:**

```bash
bash scripts/build-week-demo.sh
./.venv/bin/ruff check examples/build-week-demo
```

The script must visibly produce one red verdict followed by one green verdict. An illustrative expected report is not enough.

### Human submission/video owner — 0:00 to 1:15

**Owns:**

- submission copy
- video recording/upload
- Devpost draft fields and thumbnail

**Tasks:**

1. Keep the Build Week section candid: DevCouncil predates the event and only post-July-13 extensions are submitted.
2. Record a 2:30-2:50 video with audio:
   - 0:00-0:20 — agents often claim success without proving requirements;
   - 0:20-0:40 — install and `devcouncil --help`;
   - 0:40-1:15 — controlled repository, `dev check --verify`, compiled mode, zero gaps;
   - 1:15-1:50 — repaired graph filters/path/neighborhood behavior;
   - 1:50-2:20 — Requirement -> Task -> Diff -> Evidence and MCP/Codex integration;
   - 2:20-2:45 — eligible Build Week additions and how Codex/GPT-5.6 were used;
   - close — "Model confidence is not the final authority; evidence is."
3. Upload publicly to YouTube or Vimeo and confirm playback in a signed-out/private window.
4. Create a thumbnail from the repaired graph or clean evidence report.
5. Fill: Developer Tools track, repository URL, supported platforms, judge steps, public video, and primary `/feedback` Codex session ID.

**Acceptance:**

- Video is public, has audible narration, is under three minutes, and shows the same commands the README documents.
- The repository link is public and exposes the eligible commit history.
- The primary Codex session ID is copied from `/feedback`, not guessed.

### Coordinator/integrator — 0:00 to 2:00

**Owns:** merge order, focused verification, release observation, and final submission.

1. Own the already-local graph fix, `tests/unit/test_graph_html.py`, `scripts/npm-runtime-smoke.mjs`, `README.md`, version files, release, and integration.
2. Extend the package smoke to assert the vendored ForceGraph asset is packed, run installed `dev graph demo`, inspect `demo.html`, and reject the incompatible method call.
3. The coordinator alone edits `README.md`: lead with the Build Week/Codex problem, three outcomes, the provider-free demo, eligible-work boundary, then full documentation. Use the existing social-preview image instead of a logo-only first screen.
4. If time permits, add `docs/build-week-2026.md` with the baseline `3cfd5d1`, first eligible commit `6f5bd73`, major eligible commit groups, and reproducible `git log`/`git diff --shortstat` commands. Do not claim all eligible code was authored exclusively by Codex/GPT-5.6.
5. Replace stale "HTML + SVG" graph-demo wording in `docs/cli-reference.md`, `docs/workflow.md`, and `docs/quickstart.md` with "self-contained interactive HTML."
6. Rerun failed GitHub Actions run `29808019083` with `gh run rerun 29808019083 --failed`; investigate production DAP code only if it fails again.
7. At 1:00, review all three agent diffs for overlap and accidental unrelated files, then run the combined gate.
8. If green, bump `package.json`, `pyproject.toml`, and only the main-package entry in `uv.lock` to `0.4.1`. Keep the separate grammar package at `0.4.0`.
9. Stage explicit paths only. Exclude `.devcouncil/config.yaml` and `tests/performance/benchmark_harness.py`.
10. Push/tag only after the local package smoke succeeds; watch npm publish while the video finishes uploading.
11. Fresh-install the published version and repeat the judge path.
12. Save the Devpost draft, re-open it, verify every required field, and submit only after public video and package checks pass.

Suggested version commands:

```bash
uv version 0.4.1 --package devcouncil --no-sync
npm version 0.4.1 --no-git-tag-version
```

After publishing:

```bash
npm view devcouncil version
npx --yes --package devcouncil@0.4.1 devcouncil --help
npx --yes --package devcouncil@0.4.1 dev graph demo --project-root /tmp/devcouncil-judge-demo --json
```

Open the registry-generated HTML and require visible nodes, working click/double-click behavior, and no console exception.

## 120-minute clock

| Time | JSON/MCP | Diff correctness | Coordinator/release | Core demo | Human submission |
|---|---|---|---|---|---|
| 0:00-0:15 | Add >20k JSON regression | Add untracked/scope regressions | Finish graph/package smoke | Scaffold red-to-green sample | Prepare narration |
| 0:15-0:40 | Implement lossless transport | Fix untracked and scope paths | Graph tests, README first screen | Implement/exercise script | Record video skeleton/B-roll |
| 0:40-1:00 | Compact task result/live check | Rename/error stretch | Docs/version preparation | Finish sub-60s run | Record final narration |
| 1:00-1:20 | Combined review/gate | Combined review/gate | Integrate, bump, commit, push/tag | Help integration | Upload video; fill Devpost |
| 1:20-1:40 | Fresh package sanity | Fresh diff sanity | Observe publish/fresh graph | Rehearse published path | Verify playback and draft |
| 1:40-2:00 | Buffer only | Buffer only | Release recovery only | No new features | Final checklist and submit |

## Combined verification gate

Run this before tagging:

```bash
./.venv/bin/pytest -q \
  tests/unit/test_graph_html.py \
  tests/unit/test_mcp_server.py \
  tests/unit/test_mcp_contract.py \
  tests/unit/test_mcp_closed_loop.py \
  tests/unit/test_companion_mcp.py
./.venv/bin/ruff check \
  src/devcouncil/indexing/viz.py \
  src/devcouncil/integrations/mcp/util.py \
  src/devcouncil/integrations/mcp/handlers/status.py \
  src/devcouncil/integrations/mcp/handlers/git.py \
  tests/unit/test_graph_html.py \
  tests/unit/test_mcp_server.py \
  tests/unit/test_mcp_contract.py \
  tests/unit/test_companion_mcp.py
npm run pack:check
node scripts/npm-runtime-smoke.mjs
bash scripts/build-week-demo.sh
git diff --check
```

Do not start a full-suite rerun if it cannot finish before the release cutoff. The existing release workflow will run the full gates; preserve enough buffer to react to it.

## Final go/no-go checklist

- [ ] Fresh published npm version installs successfully.
- [ ] Published `dev check --verify` returns a deterministic verdict on the controlled sample.
- [ ] Published graph demo renders nodes and controls instead of a blank canvas.
- [ ] MCP status/task/prompt/gaps/next-actions/list-tasks handle this repository's large structured output without parse errors or unbounded context dumps.
- [ ] MCP diff shows untracked files and task-scoped diff fails closed instead of broadening scope.
- [ ] README clearly identifies eligible post-July-13 work and contains reproducible judge steps.
- [ ] Public narrated video is under three minutes and matches the published behavior.
- [ ] Devpost project has a real title/tagline, Developer Tools track, repo, video, test instructions, country, submitter type, and `/feedback` session ID.
- [ ] Existing unrelated worktree changes were not staged or overwritten.

If the package is not published or the video is not publicly playable by 1:40, stop adding improvements and spend the final 20 minutes recovering those two submission blockers.

## Deferred future backlog

This section begins **after** the hackathon submission is safely published. Anything unfinished from the two-hour P0/P1 lanes rolls into the top of this backlog. Items marked **confirmed** were reproduced or observed against the current checkout; items in the revalidation queue come from older evidence and must not be presented as current bugs until reproduced again.

### Confirmed bugs and product debt

| ID | Priority | Area | Deferred work | Target surface | Acceptance evidence |
|---|---|---|---|---|---|
| F-01 | P0 rollover | MCP diff | Finish untracked, empty-file, binary-file, rename, path-filter, and Git-error handling. Preserve the external diff cap while making `files` authoritative. | `integrations/mcp/handlers/git.py`, `test_companion_mcp.py` | Fixture matrix proves every Git state and nonzero subprocesses return `ok: false`. |
| F-02 | P0 rollover | MCP scope | Make every task-scoped read fail closed when the project/task is missing. Explicit path filters must intersect task scope, never union with it. | MCP git/scope/read handlers and contract tests | Cross-task and uninitialized-repo adversarial tests cannot expose unrelated files. |
| F-03 | P0 rollover | MCP transport | Complete lossless internal JSON transport across all structured handlers while keeping generic CLI/report text bounded. Remove handler-local JSON parsers. | `integrations/mcp/util.py`, `handlers/*.py` | A generated 250 KB valid payload parses correctly; raw text remains capped and marked truncated. |
| F-04 | P1 | Agent context | Make `list_tasks`, gaps, next-actions, and status return compact summaries with explicit pagination/detail tools. Five tasks currently create roughly 168 KB of task-list JSON. | task/status CLI and MCP handlers | Default tool result stays within a documented context budget; detail remains available by ID/cursor. |
| F-05 | P1 | Privacy | Stop embedding complete live-signal models in general status. Current `pending_signal_items` can include user email, absolute transcript paths, model metadata, and large payloads. Return IDs/counts by default and expose sensitive local details only through an explicit local diagnostic command. | `live/summary.py`, `cli/commands/status.py`, live/report MCP resources | Status contains no email or absolute transcript path; dedicated diagnostics retain necessary review commands. |
| F-06 | P1 | Dashboard | Replace raw coverage/event JSON with verdict and summary cards, show blocking gaps first, collapse integrations/traces, and add meaningful empty states. Preserve token and loopback protections. | `ui/dashboard.py`, dashboard tests | Desktop and narrow-width browser checks show the verdict without scrolling and no raw JSON in the primary view. |
| F-07 | P1 | Graph UX | Show node/edge/filter counts, call `zoomToFit` after initial layout and reset, and explain click/path/neighborhood interactions in the interface. | `indexing/viz.py`, graph HTML tests | Large and demo graphs open centered, legible, and self-explanatory without manual pan/zoom. |
| F-08 | P1 | Graph testing | Add a real packaged-browser smoke test. String assertions did not catch the ForceGraph API mismatch that blanked the canvas. | package smoke/browser harness, graph tests | Packed HTML loads with a nonempty canvas, no console exceptions, and verified click/double-click/path behavior. |
| F-09 | P1 | Platform CI | Diagnose the Windows Node-DAP `initialized` timeout only if the failed job repeats. Add bounded diagnostics around adapter launch, stderr, process exit, and event sequencing. | `test_codeintel_platform_runtime.py`, DAP session code, platform workflow | Multiple consecutive Windows launch/attach runs pass; timeout failures include actionable adapter state. |
| F-10 | P1 | Demo isolation | Provide a supported `dev demo` or fixture workflow that creates isolated state instead of displaying a mature repository's historical gaps. | CLI demo command, examples, quickstart | One command creates a clean red-to-green project and leaves the original repository untouched. |
| F-11 | P2 | Release UX | Create GitHub releases from version tags, attach the judge/demo notes, add npm/CI badges, and verify registry propagation automatically. | release workflow, README | Tag produces npm package plus GitHub release; a clean registry smoke runs before release is marked complete. |
| F-12 | P2 | Documentation | Keep graph-demo format, supported platforms, maturity labels, and runnable examples synchronized. Replace illustrative-only examples with executable fixtures. | quickstart, workflow, CLI reference, examples | Documentation smoke commands execute successfully from a clean checkout. |
| F-13 | P2 | Graph limits | Improve user-facing handling of documented graph limits: capped compatibility export, embedding scan caps, and 15-second community-detection aborts. | graph doctor/status/build/community surfaces | Every cap/abort reports structured degraded state, retained canonical-store health, and a recovery command. |
| F-14 | P2 | Dogfooding | Add a release-health report that distinguishes historical project gaps from regressions introduced by the release candidate. | status/reporting/CI evidence | Release output shows new blockers separately and never advertises a green release solely from stale historical state. |
| F-15 | P3 | Architecture | Continue consolidating CLI/MCP service contracts only where duplicated parsing or policy behavior creates observable drift. Do not reopen already-completed god-module refactors without a failing contract. | CLI/MCP handlers and shared services | Each extraction removes a reproduced parity bug and keeps focused/full gates green. |

### Suggested future sequence

1. **Trust boundary:** F-01 through F-05.
2. **Judge/user experience:** F-06 through F-10.
3. **Release and maintainability:** F-11 through F-15.

Each item should become its own DevCouncil task with a narrow planned-file scope and an adversarial acceptance test. Avoid combining dashboard, graph, Git-diff, and transport work into one release.

### Historical findings that require revalidation

These were real in earlier audits, but later work may already have repaired some or all of them. Reproduce first; close the item as stale if the authoritative test passes.

| ID | Historical risk | Revalidation gate | Promote to confirmed only when |
|---|---|---|---|
| R-01 | Two watcher/coordinator processes contended for the graph writer lease and could leave batches retrying. | Run a bounded real two-process `dev map --watch` edit/reconcile test. | A current build reports `GraphBuildBusy`, loses pending work, writes degraded output, or churns generations. |
| R-02 | Ambiguous call fan-out polluted PageRank, hotspot, god-node, and process metrics. | Compare metrics with ambiguous edges included versus down-weighted/excluded. | Common method names still dominate structural rankings without extracted call evidence. |
| R-03 | Installed grammar coverage, LSP confirmation, and dependency-auditor availability were incomplete on some hosts. | Run `dev graph doctor` and the full language/platform matrix from the release artifact. | A supported language group or advertised auditor is missing or silently degraded. |
| R-04 | Import cycles and PDG/taint volume suggested precision debt. | Rerun cycle/PDG reports and manually label a representative sample. | Current false-positive rate or cross-subsystem cycle still exceeds an agreed threshold. |
| R-05 | A stale globally installed `dev` could not read newer workspace state. | Compare checkout and registry CLIs against the same persisted database during upgrade tests. | A supported upgrade path produces schema/type errors instead of migration or a clear version message. |

Recommended revalidation commands:

```bash
./.venv/bin/dev graph doctor --json
./.venv/bin/dev graph dead --confidence extracted --json
./.venv/bin/dev map --if-stale
./.venv/bin/pytest -q \
  tests/unit/test_codeintel_sync.py \
  tests/unit/test_graph_build_control.py \
  tests/unit/test_graph_intel.py \
  tests/unit/test_mcp_map_tools.py
```

Use a separately controlled two-watcher integration test for R-01; focused unit tests are not sufficient evidence for cross-process lease correctness.

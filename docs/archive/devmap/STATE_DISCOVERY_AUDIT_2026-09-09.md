# DevMap state discovery and first-run audit

Date: 2026-09-09. Scope: CLI repository roots, database and artifact path resolution, worktree bootstrap, map rendering input, and agent setup guidance. This report records qualification before integration and installation. During qualification, the installed CLI, active Tasks UI worktree, and concurrent DevCouncil checkout were left untouched.

## Why the state keeps appearing missing

**Verified:** a new linked worktree does not inherit another checkout's generated index. Read-only `status` and `paths` correctly leave it uninitialized. A plain `build` creates the database but does not export `repo_map.json`. GitPulse's maintained guide directed agents to the legacy map filename and suggested a database-only rebuild. Those instructions could leave the requested map missing even after a successful build.

There were also genuine path and validation defects. The following are reproduced failures, not deductions from comments or index names.

| Finding | Before | Fixed behavior and regression evidence |
|---|---|---|
| Relative repository root joined twice | From a parent directory, `paths repo` reported `/parent/repo/repo/.devmap/codeintel/devmap.sqlite` and `store_exists:false` after a successful build. | Absolutize the writer's database path once. `relative_repository_paths_do_not_duplicate_the_repository_directory` fails before the change and passes afterward. |
| Relative overrides were reinterpreted | `--db custom.sqlite paths repo` reported a repository-relative database, although build wrote invocation-relative. Relative `DEVMAP_HOME` could duplicate the repository component. | Preserve invocation-relative `--db` and repository-relative `DEVMAP_HOME`. Both have real-binary regression tests. |
| Omitted repository roots disagreed | `status` from `repo/src` inspected the worktree root, while `paths` and `build` defaulted to `src`. A successful default build could still leave rootless queries missing. | All 11 root-taking command defaults use the same worktree-root helper as rootless queries. Explicit paths, including `.`, retain their scope; non-Git directories keep current-directory behavior. |
| Map rendering retained legacy defaults | A standalone `build --manifest` wrote `.devmap/repo_map.json`; `map-html` then tried `.devcouncil/repo_map.json`. | Default map input and output use the existing canonical state resolver. Standalone, legacy and environment-override layouts are tested. |
| Map rendering omitted named-root validation | With an absolute input, an explicitly nonexistent repository root was created and reported successful. | Include `MapHtml` in the shared root selection/validation path. The installed binary reproduces this failure; the candidate refuses it without creating the root. Raw comparison is in the qualification JSON. |
| Renderer bypassed checked artifact input | `{}`, `[]`, `null`, and symlink input could render a successful empty/misleading page; reads were unbounded. | Reuse `host::read_repo_map` with the existing 128 MiB cap. Invalid shapes, oversized files and symlinks are refused before writing the page. |
| Setup guidance could not bootstrap its own prerequisite | Guides assumed the map existed and suggested `build` without artifact export. GitPulse additionally hardcoded the legacy path. | Resolve `paths`, inspect `status`, bootstrap with `build --manifest`, then read the resolved map. Generated-guide regression passed; GitPulse guidance was checked separately. |
| Standalone exports could leak into Git changes | GitPulse ignored legacy state and SQLite files but left standalone JSON exports unignored. | Add the narrow `.devmap/` ignore. No files in either state directory were tracked; `git check-ignore` now confirms all standalone artifacts are excluded. |

## Contracts preserved

- Missing state remains visibly unavailable. Discovery does not create, migrate, or borrow another worktree's database.
- `.devmap`, legacy `.devcouncil`, and `DEVMAP_HOME` retain their existing precedence. Explicit database and map-file overrides retain their documented meanings.
- Database-only build remains available. `build --manifest` creates or repairs the map/graph pair, including when the database is already current.
- Git worktree identity and ownership checks stay in the existing store implementation; no shared cross-worktree cache or schema migration was introduced.
- Source freshness, truncation, and language-coverage qualifications remain visible. Successful discovery is not proof of complete graph coverage.

Security impact: map rendering now adopts the existing bounded, regular-file artifact validation. This narrows accepted input and does not widen filesystem or network access. Portable filesystem preflight is not a universal guarantee against hostile concurrent path replacement; this change does not claim otherwise.

## Validation

[STATE_DISCOVERY_QUALIFICATION.json](STATE_DISCOVERY_QUALIFICATION.json) retains commands, counts, candidate hash, raw stress metrics, first-run results, and limits.

| Check | Verified result |
|---|---|
| Existing focused CLI baseline before implementation | 12 passed |
| New path contracts before implementation | Six failed and one explicit-scope control passed |
| New generated-guide contract before implementation | Failed |
| New renderer rejection contracts before implementation | Both failed by returning successful HTML |
| Final new contracts | 13 CLI integration tests plus one generated-guide test passed |
| `cargo test --locked --workspace` | Exit 0; 2,377 passed, zero failed, three marked ignored across 220 summaries |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | Exit 0 |
| `cargo fmt --all -- --check` and diff whitespace checks | Passed |
| GitPulse maintained guidance | Resolves paths, exports maps, preserves the GitNexus section; EOF newline normalized |
| Final-candidate worktree stress | 16 daemons, 32 simultaneous editors, 12 rounds, 4,608 edit operations, 384 IPC queries, 48 kills/restarts, 64 metadata checks, all 16 cold-build comparisons passed |

The final stress run used a frozen debug binary, SHA-256 `eff4e4c30a0c881a7862ad0208d794e732861b25261c704991f41d9139ad0d9c`, on macOS arm64. Elapsed time was 74.051 seconds. IPC query latency was p50 7.349 ms, p95 34.9 ms, maximum 132.269 ms across 384 samples. These are IPC measurements for small fixtures, not a large-repository or release performance claim.

One intermediate focused run failed during fixture `git init` with `fatal: cannot copy .../hooks/pre-receive.sample ...: File exists`. The fixture now uses an atomic sequence, exclusive directory creation, and an empty Git template setting. The final focused and workspace runs passed. The observation is retained; a specific underlying filesystem race was not proven.

The three ignored entries are accounted for: external Claude CLI plugin validation was not run because the emitter is unchanged; the separate single-worktree storm soak was not run because this audit exercised the existing 16-worktree crash/restart stress harness instead; `concurrent_prune_writer_child` is an intentionally ignored child entrypoint that its passing parent test invokes explicitly.

## Real GitPulse checkout check

An isolated GitPulse checkout at `d2d8713da93e9fe7106306b9c8faac1d34b06a4d` initially had no state directory or database. From its `src/` directory, the candidate ran `build --manifest`, followed by `status`, `paths`, and `map-html`. All succeeded. Discovery and status named the same root database; no `src/.devmap` was created.

The resulting store indexed 1,820 files, 25,345 nodes, and 299,676 edges. Status reported `query_ready:true` and `is_fresh:true`. The renderer wrote a 259,009-byte HTML artifact from the resolved standalone map. The map retains 18 existing Swift/SQL import-extraction gaps; those are language coverage limitations, not missing-state errors. No discovery refusals or parse failures were reported for this checkout.

## Review and remaining qualification

Canonical implementation changes are in `crates/devmap-cli/src/main.rs` and `crates/devmap-query/src/guides.rs`; real-binary regressions are in `crates/devmap-cli/tests/state_discovery_contract.rs`. GitPulse's separate branch changes only its maintained `AGENTS.md` and `.gitignore`. The checked artifact reader itself is reused unchanged. GitPulse's existing embedded reader already calls that provider, so no vendored renderer implementation was duplicated.

Diff footprint: the two existing Rust files add 61 lines and remove 41, including the generated-guide regression. The new CLI integration test file adds 383 lines. Documentation and recorded qualification evidence are separate; no dependency was added.

At qualification time, the candidate had not been integrated, committed, installed, or released. Windows/Linux execution, installed-app behavior, release performance/memory gates, and broad mutation testing were not part of this qualification. This audit establishes the listed state-discovery and bootstrap contracts on macOS; it does not establish universal correctness or remove existing language-extraction limitations. Subsequent installation receipts record deployment separately from these original test results.

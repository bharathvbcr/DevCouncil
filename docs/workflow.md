# Developer and agent workflow

Use DevMap for repository awareness. Add task state and verification when the
consumer needs them. Installing an integration does not make every shell
command or editor write require a lease. [Documentation index](README.md)

## Understand before editing

```bash
devmap paths --json
devmap status --json
# Build missing/stale state in this worktree:
devmap build --manifest --guides

devmap explore MyFunction --json
devmap impact src/service.go --json
devmap dead --json
```

Use the resolved map for navigation, then inspect actual source. Check query
freshness, coverage gaps, confidence and truncation. An affected-test list
suggests tests to run; it does not record a test result.

## Connect the chosen host

Preview and apply standalone code-navigation integration:

```bash
devmap integrate cursor --dry-run
devmap integrate cursor
devmap integrate cursor --check
```

Use [the integration guide](coding-cli-integration.md) for supported host names,
user-level settings, trust requirements and optional Go-host MCP setup.
Skills provide instructions; they do not enforce tool permissions.

## Implement and verify

Choose the project’s actual build/test commands from its manifests and CI.
Review the diff, execute the relevant checks and record what passed, failed or
could not run. Refresh the map after structural edits.

For a task already present in `.devcouncil/state.sqlite`, a participating agent
can check out a lease, implement the declared scope, ask for verification,
repair identified gaps, then release the lease. That workflow is driven by the
agent or consuming harness, not by an autonomous execution loop in DevCouncil.

```bash
# TASK-001 must exist; use explicit enforcement for a strict verification run.
devcouncil verify TASK-001 --mode enforce --json
```

The Go host runs scope/expected-command checks and invokes `dcverify` for rigor.
Inspect skipped reasons, gate mode and coverage metadata. Supplying a coverage
profile through CLI `--coverage PATH` enables changed-line coverage evaluation;
the MCP verify tool does not take a profile.

A lease coordinates cooperating clients; a release is not itself a successful
verification. The host’s `--sandbox` flag does not isolate commands. The
[task loop contract](hero-loop.md) explains those boundaries and repair signals.

## Hand off with evidence

Record the source revision or working-tree scope, changed files, commands run,
results and remaining limitations. Keep static graph evidence distinct from
executed tests and physical editor/platform qualification. Provider routing,
agent execution and multi-turn automation belong to the consuming harness;
see [architecture](architecture.md) and [project status](project-status.md).

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

## Before you push, and before you tag

Nothing enforces a gate on `git push`, so the gate is a rule: run
`npm run ci:local` from the repository root before pushing to `main`. It runs the
script tests, the release-identity gates, the workflow lint, the npm package
checks, `gofmt` with the toolchain `go.mod` declares, the Go host and
orchestrator tests against the Gusset engine, and `rust/verify.sh --quick`. A
step that cannot run is reported as NOT RUN or fails; it is never counted as a
pass, and the final line lists every NOT RUN step. For Rust changes also run
`cargo fmt --all -- --check` and the clippy command from
`.github/workflows/rust.yml` in `rust/`.

`ci:local` and `rust/verify.sh` need the Gusset sibling at `../gusset` at the
revision pinned in `.github/actions/setup-gusset/action.yml`, and fail on any
other revision (`rust/gusset-engine/check-pin.sh`).

### Release checklist

1. Every workflow is green on the release commit: CI, analysis components,
   devmap (Rust), CodeQL.
2. `npm run ci:local` passes. The release-tag step is NOT RUN until the tag
   exists; create the tag at the release commit, then rerun it: it passes only
   when the local tag is at HEAD and fails for a tag at any other commit.
3. `docs/releases/<tag>.md` names the schema numbers at the release commit
   (store, extraction, workbench) and the work since the previous notes.
4. Only the owner creates and pushes the tag and the GitHub Release.

### Bumping the pinned toolchains

- **Rust:** edit `channel` in the root `rust-toolchain.toml` and nothing else
  names a version except `rust-version` (`rust/Cargo.toml` and
  `rust/dc-sparse-encode/Cargo.toml`), which moves to the same minor. Run
  `cargo fmt --all -- --check` and clippy with `-D warnings` for both package
  sets in `.github/workflows/rust.yml` and `analysis-plane.yml`, fix what the new
  release flags, and commit the bump on its own so a red CI points at it.
- **Go:** edit `toolchain` in `backend/go_orchestrator/go.mod` (CI's `setup-go`
  reads it) and run `govulncheck ./...` with it. The `go` line is the floor
  consumers need; raise it only for a language or library need.
- **Gusset:** move the `ref` in `.github/actions/setup-gusset/action.yml` in the
  same change as the code that needs it, and rebuild `rust/gusset-engine` with
  `--locked`.

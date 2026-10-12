# Quickstart

Install DevMap, index a repository, and explore its code without a model key.
The Go host, task store, verifier and text search are optional additions.
[Documentation index](README.md)

## 1. Build the component you need

Prerequisites: Git and a Rust/Cargo toolchain compatible with
[`rust/Cargo.toml`](../rust/Cargo.toml) and its lockfile. Building the Go host
also requires the toolchain declared in
[`backend/go_orchestrator/go.mod`](../backend/go_orchestrator/go.mod)
(currently Go 1.26.9), plus a sibling checkout of
[Gusset](https://github.com/bharathvbcr/gusset) at `../gusset`, at the revision
pinned in [`.github/actions/setup-gusset/action.yml`](../.github/actions/setup-gusset/action.yml). Node 18+ is only for the optional npm launcher.

```bash
git clone https://github.com/bharathvbcr/DevCouncil.git
cd DevCouncil

# macOS / Linux: standalone DevMap
bash scripts/install.sh --only=devmap

# Alternatively, install the full native suite
bash scripts/install.sh
```

Windows, in PowerShell from the clone:

```powershell
.\scripts\install.ps1 -Components devmap
# Or the full suite:
.\scripts\install.ps1
```

The Unix installer defaults to `~/.local/bin`; put that directory on PATH:

```bash
export PATH="$HOME/.local/bin:$PATH"
devmap --version
```

The scripts are bootstraps: each builds the Go host into a temporary directory
and runs `devcouncil install` with the same arguments, so
`bash scripts/install.sh --help` prints that command's presets, prefix
selection and `--dry-run`. Once the host is on PATH, run `devcouncil install`
directly. Without Go, install DevMap alone with
`cargo install --git https://github.com/bharathvbcr/DevCouncil.git --locked devmap-cli`.
The host installs only from a checkout. Outside one, `devcouncil install` (the
default preset includes the host) exits 2 and names
`git clone https://github.com/bharathvbcr/DevCouncil.git` and
`devcouncil install --src <dir>`; `--src` points the installer at a clone from
any working directory. There is no `go install` route (the host links the
Gusset engine through cgo), and the npm package is only a launcher for a host
you built.
After installing from a checkout, the installer checks the schema consumers.
DevCouncil owns the devmap store, dc-store and workbench schemas, and GitPulse,
Manvi and Jarvis each read them through binaries or pins that can fall behind.
The installer finds a GitPulse checkout (`--gitpulse DIR`, then
`$DEVCOUNCIL_GITPULSE_ROOT`, then `../GitPulse`) and runs its
`scripts/check-schema-parity.mjs --json` with node, pointed at this checkout.
The run is bounded at two minutes with output capped at 1 MiB. It prints
every reader's verdict (`match`, `stale`, `unverifiable` or `absent`) and lists
each stale or unverifiable reader under `FOLLOW-UP` with its remedy:

- GitPulse's `gitpulse-mcp`: `bun run mcp:install`.
- GitPulse.app: rebuild and reinstall the app.
- GitPulse's vendored crates: re-vendor.
- `devmap` or `dcstore` on PATH: reinstall, or fix PATH shadowing.
- Manvi's or Jarvis's pin: bump the pin. That is a commit in that repository,
  so report the delta and ask first.

The install's exit status does not change: an owner does not refuse to install
because a consumer lags. `--strict-consumers` makes a stale or unverifiable
reader fail the install, and so does a check that could not run. It refuses
`--dry-run`. If GitPulse or node is missing, or the check fails, times out or
prints something unreadable, the installer prints
`schema consumers: NOT CHECKED — <why>`, never a pass. With `--json`, the
receipt carries the same report as `consumers`.

The scripts build source. `npm install -g devcouncil` installs a lightweight
launcher that resolves native executables; it does **not** replace this build or
bundle the Rust suite. There is no current Python/uv install path.

## 2. Index your project

```bash
cd /path/to/your/project
devmap build --manifest --guides
devmap paths --json
devmap status --json
```

`--manifest` exports the repository map and graph. `--guides` additionally writes
marker-managed workspace guides; it does not overwrite an unrelated guide.
The Go-host shorthand `dev map` forwards to `devmap build --manifest` and does
not implicitly add `--guides`.

Use the `repo_map`, `code_graph` and `db_path` returned by `paths`. New projects
use `.devmap/`; an existing `.devcouncil/` layout is supported, and an existing
`.devmap/` takes precedence. Explicit state configuration can override this.
The SQLite store is canonical; an exported graph can be capped.

Check `query_ready`, `is_fresh`, `rebuild_required` and `coverage_gaps` before
trusting a query. Freshness does not mean every language feature was resolved.
Creating guides can change the inventory after the first build. If status
reports stale source immediately afterwards, run `devmap build --manifest`
once more, then check status again.
Generated state belongs to each worktree: build there instead of copying a
sibling's database.

## 3. Ask a question

Substitute a symbol or path from your repository:

```bash
devmap search MyFunction --json
devmap explore MyFunction --json
devmap impact src/service.go --json
devmap trace EntryPoint TargetFunction --json
devmap dead --json
```

Read confidence and unresolved-call evidence. When a response reports `total`
and `truncated`, distinguish the returned sample from the full result.
Dead-code and affected-test results are candidates for investigation, not proof
that deletion is safe or that tests passed. See the [code graph guide](code-graph.md).

## 4. Connect an agent

Choose your host: `cursor`, `claude`, `codex`, `antigravity`, `opencode`, or `warp`.
For example:

```bash
devmap integrate cursor --dry-run
devmap integrate cursor
devmap integrate cursor --check
```

The first command previews the changes. The second applies them. Depending on
the host, this includes user-level MCP registration, project guides, skills and
navigation hooks. Inspect the receipt; reload the host and complete its trust
steps as needed. Every DevMap MCP call should carry the absolute `repo_path`.
See [integration details](coding-cli-integration.md) for the separate Go-host setup.

## 5. Add task verification when needed

The full suite includes `devcouncil` (also `dev`), `dcstore`, `dcverify` and
`dcgrep`. Verify installed components without running a task:

```bash
devcouncil --help
dcverify health
dcgrep health
```

Task verification needs an initialized `.devcouncil/state.sqlite` and a real
task with planned files and expected commands, supplied by your consuming
application or store client. Installing or mapping does not create a task.

```bash
# TASK-001 must already exist in this project's task store.
devcouncil verify TASK-001 --mode enforce --json
# Supply an actual coverage profile to measure changed-line coverage:
devcouncil verify TASK-001 --mode enforce --coverage /path/to/coverage-profile --json
```

Verification is opt-in; the default mode is `off`. Check `status`,
`verification_skipped`, `rigor_applied`, `rigor_skipped_reason`,
`coverage_measured` and `coverage_skipped_reason`. The `--sandbox` flag records
a value on the report; it does not provide Docker/Nix isolation.

Next: [task workflow](workflow.md), [MCP task loop](hero-loop.md),
[architecture](architecture.md), [CLI reference](cli-reference.md).

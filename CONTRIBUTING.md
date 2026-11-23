# Contributing to DevCouncil

Thanks for helping. This guide covers how to set up, what to run before you open
a pull request, and how changes are reviewed. By taking part you agree to the
[Code of Conduct](CODE_OF_CONDUCT.md).

- Questions: see [SUPPORT.md](SUPPORT.md) (use [Discussions](https://github.com/bharathvbcr/DevCouncil/discussions)).
- Vulnerabilities: never a public issue; see [SECURITY.md](SECURITY.md).
- Who decides what: [GOVERNANCE.md](GOVERNANCE.md).

## Before you start

For anything larger than a small fix, open an issue or a Discussion first, so
the approach is agreed before you write the code. Look at [project status](docs/project-status.md)
for known limitations and retired commands, and the [architecture](docs/architecture.md)
guide for what each component owns.

New third-party dependencies (npm, Cargo or Go) need agreement from the
maintainer before they are added.

## Development setup

DevCouncil is a Rust workspace (`rust/`), a Go host (`backend/go_orchestrator`)
and a thin npm launcher. You need Git, Rust through `rustup` (the toolchain is
pinned in [`rust-toolchain.toml`](rust-toolchain.toml) and picked up
automatically), Go (the version in
[`backend/go_orchestrator/go.mod`](backend/go_orchestrator/go.mod)) and Node.js
20 or newer.

The Go host links the Gusset engine, so it also needs a sibling checkout of
[Gusset](https://github.com/bharathvbcr/gusset) at `../gusset`, at the revision
pinned in [`.github/actions/setup-gusset/action.yml`](.github/actions/setup-gusset/action.yml).
`npm run ci:local` and `rust/verify.sh` fail on any other revision.

```bash
git clone https://github.com/bharathvbcr/DevCouncil.git
cd DevCouncil
git clone https://github.com/bharathvbcr/gusset.git ../gusset   # then check out the pinned revision
bash scripts/install.sh --only=devmap                           # builds and installs DevMap alone
```

See the [quickstart](docs/quickstart.md) for the full install, including
Windows.

## Running the checks

Run this from the repository root before you push. It is what the
[task workflow guide](docs/workflow.md#before-you-push-and-before-you-tag)
calls the gate:

```bash
npm run ci:local
```

It runs the script tests, the release-identity gates, the workflow lint
(needs `actionlint`, for example `brew install actionlint`), the npm package
checks, `gofmt`, the Go host and orchestrator tests, and `rust/verify.sh --quick`.
A step that cannot run is reported as NOT RUN or fails; it is never counted as a
pass. `npm run ci:local -- --continue` shows every result instead of stopping at
the first failure.

Individual pieces:

| Area | Command |
|---|---|
| Script tests | `npm run test:scripts` |
| Workflow lint | `npm run check:workflows` |
| Rust formatting | `cargo fmt --all -- --check` (from `rust/`) |
| Rust lints and tests | the `cargo clippy` and `cargo test` commands in [`.github/workflows/rust.yml`](.github/workflows/rust.yml) and [`analysis-plane.yml`](.github/workflows/analysis-plane.yml), run from `rust/` |
| Rust full pipeline | `bash rust/verify.sh` (`--quick` for the short form) |
| Go host | `go -C backend/go_orchestrator test ./cmd/devcouncil -count=1` |
| Release tooling | `npm run test:release` |

Cargo `build`, `test`, `check` and `clippy` commands in workflows and scripts
must pass `--locked`; `npm run check:workflows` enforces it.

## Making a change

1. Fork the repository, or create a branch if you have write access.
2. Branch from `main` with a prefix that names the kind of change:
   `feat/`, `fix/`, `docs/` or `chore/` (for example `fix/devmap-stale-index`).
3. Keep the change to one concern. A bug fix comes with a test that fails
   without the fix. A new behaviour comes with tests and, if user-visible,
   a docs update.
4. Write commit messages as `type(scope): summary`, for example
   `fix(dc-store): refuse a trigger-carrying database`. Types in use include
   `feat`, `fix`, `docs`, `test`, `refactor`, `build`, `ci` and `chore`; the scope
   is the crate, package or area, and may be omitted.
5. Sign off every commit (see below).
6. Open a pull request against `main` and fill in the template.

## Sign-off (DCO)

Sign-off is required by project policy and checked in review. GitHub itself
enforces it only for commits made in the web UI, so a missing line on a
command-line commit is caught by the reviewer, not by a bot. Every commit must
carry a `Signed-off-by` line, certifying the
[Developer Certificate of Origin](https://developercertificate.org/): that you
wrote the change or have the right to submit it under the project's
[Apache 2.0 license](LICENSE). Add it with `git commit -s`. To fix commits that
lack it, run `git rebase --signoff origin/main` and force-push your branch.

## Schema changes

DevCouncil owns schemas that GitPulse, Manvi and Jarvis read (the DevMap store,
the extraction and code-graph schemas, and the `dc-store` workbench schema). If
your change alters one, say so in the pull request and read the
[Governance](GOVERNANCE.md#shared-schemas-and-cross-repository-coordination)
section on cross-repository coordination first.

## Review and merge

- `main` is protected. Contributors, including collaborators with write
  access, land changes through pull requests. A pull request needs one approving
  review with code-owner review ([`.github/CODEOWNERS`](.github/CODEOWNERS)),
  all conversations resolved, and the required status checks green (`test`,
  `Verify package`, and the three CodeQL `Analyze` jobs). New commits dismiss
  stale approvals. Force pushes to `main` are blocked.
- Review is best effort; this is a one-maintainer project, so allow a few days
  and ping the thread once if it goes quiet.
- Keep the branch up to date with `main` and respond to review comments in the
  thread. Do not merge your own pull request unless you are a maintainer.
- Releases are cut by the maintainer; see [GOVERNANCE.md](GOVERNANCE.md#releases).

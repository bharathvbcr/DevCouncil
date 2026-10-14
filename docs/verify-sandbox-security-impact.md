# Security-impact statement: isolated verification sandbox

**Status: approved 2026-10-08 by the owner; scope implemented: local env
scrub.** The container rows below are an approved design, not scheduled.
Reopen condition for Docker/Nix: a CI job with docker runs the real container
test. This statement is the first acceptance criterion of task
`dc-verify-isolated-sandbox` (ft-7121a94ba987a761beace32a7675461e).
`verify.ParseSandbox` keeps refusing every value except `local`.
[Security boundaries](security.md) · [Documentation index](README.md)

## Implemented: local environment scrub

`verify/env.go` owns it. Each verification command runs with an allow-listed
environment instead of the host's:

- **Base list** (`baseEnvAllow`): PATH, HOME, USER, LOGNAME, SHELL, TMPDIR,
  TMP, TEMP, TZ, TERM, COLORTERM, NO_COLOR, CI, LANG, LANGUAGE, `LC_*`, GOCACHE,
  GOMODCACHE, GOPATH, GOROOT, GOFLAGS, GOTOOLCHAIN, GOENV, CARGO_HOME,
  CARGO_TARGET_DIR, RUSTUP_HOME, RUSTUP_TOOLCHAIN, NVM_DIR, NODE_PATH,
  PYENV_ROOT, VIRTUAL_ENV, JAVA_HOME, XDG_CACHE_HOME, XDG_CONFIG_HOME,
  XDG_DATA_HOME, XDG_RUNTIME_DIR, DEVELOPER_DIR, SDKROOT. None is a credential.
- **Per-repo extension**: `verify.env_allow: [NAME, NAME]` in
  `.devcouncil/config.yaml` (inline list or comma-separated scalar). A block
  list is an error, not an empty list.
- **Refusals**: a name that is not a legal variable name, or that contains KEY,
  SECRET, TOKEN, PASSW, CREDENTIAL, AUTH, PRIVATE or COOKIE, is refused even
  when listed. `config.yaml` is inside the tree the verified agent edits, so it
  must not be what hands that agent's judge a credential. A repository that
  truly needs such a variable has no per-repo route; that is a decision for the
  owner.
- **Failure direction**: an unreadable `verify.env_allow` runs the base list
  alone and adds a blocking `verify_env_config_invalid` gap. Config can only
  narrow, never widen, beyond the allowlist. The write gate's safety flags
  follow the same rule from the same file: `.devcouncil/config.yaml` may
  tighten them and is refused, by file, line and key, when it would loosen one
  (see SECURITY_BOUNDARIES.md, "Repository configuration and safety flags").
- **Evidence**: each run records `env_scrub`, `env_allow_extra`,
  `env_allow_refused` and `env_allow_error` in its environment record. Names
  only, never values.
- **Not covered**: HOME is passed, so files under it (`~/.aws`, `~/.ssh`,
  keychains) stay readable. Only containers would close that, which is why the
  container design remains approved but parked. The rigor gates and other
  spawns `verify.Run` makes are unchanged.

## What runs today

Read from the code on 2026-10-08:

| Fact | Where |
|---|---|
| A task's verification commands are its `expected_tests`, falling back to `allowed_commands`, read from the `dcstore` task record. | `verify/orchestrate.go:119`, `verify/types.go:126` |
| Each command runs as `/bin/sh -c <command>` with the project root as its working directory, in its own process group. | `verify/commands.go` `runCommand` |
| Before the scrub, the child inherited the host process's whole environment because `cmd.Env` was not set. It now gets the allow-listed environment described above. | `verify/commands.go` `runCommandWithPolicy`, `verify/env.go` |
| Each command is bounded by `verify.CommandTimeout` (30 minutes) and by the caller's context. A stopped command's whole process group is killed, and the result is a blocking `test_failed` gap. Before 2026-10-08 there was no bound at all (task dc-verify-local-runner-unbounded). | `verify/commands.go` `CommandTimeout`, `runCommand`, `RunVerificationCommands` |
| Output is kept up to 4 MiB per command, and a truncation is noted in the summary without changing the exit-code verdict. | `verify/commands.go` `commandOutputLimit`, `proc.CappedBuffer` |
| Three entry points reach this runner: `devcouncil verify`, `verify.RunCLI` and MCP `devcouncil_verify_task`. The MCP tool means a connected agent can start host execution of the task's stored commands. | `ParseSandbox` callers: `cmd/devcouncil/main.go::runVerify`, `verify/cli.go::RunCLI`, `verify/orchestrate.go::VerifyTask` |
| `local` is the only sandbox. Any other value is refused before the store opens (exit 2, or an MCP error), and a run records `verify.SandboxLocal`. | `verify/orchestrate.go:63-73`, docs/TODO.md TASK-P7-2 |

The `dcverify` rigor gates and the other spawns `verify.Run` makes are out of
scope for this statement. They read the diff rather than execute task-supplied
commands. That claim is inferred from their role and has not been verified by
reading their spawn code.

## What isolation would promise (approved design, not scheduled)

A sandbox changes what one verification command can **reach**, not what it
**proves**. The proposal is that each command runs in a fresh container
(`docker`) or a Nix build sandbox (`nix`). Every boundary below is a decision
the owner approves or changes:

| Boundary | Proposed default | Why |
|---|---|---|
| Repository | Read-only bind mount of the project root, plus a writable scratch overlay that is thrown away after the run. | Tests that write build output still work. A command cannot rewrite the tree that is about to be diffed and persisted. |
| Rest of the host filesystem | Not mounted. That includes `$HOME`, `~/.ssh`, cloud credentials, other worktrees and `.devcouncil/` state. | This is the main thing isolation buys. Today a command can read all of it. |
| Environment | Empty, plus an explicit allowlist (for example `PATH`, `HOME` pointing at scratch, `LANG`, `CI=1`). No inherited tokens. | Closes the inherited-environment leak in the table above. |
| Network | Off by default. Turning it on per task is a recorded, opt-in setting that the run reports. | A test that needs the network is visible as such, never silent. |
| Resources | `local`'s wall-clock timeout and output cap, plus memory and PID caps. Breaching any of them is a failed command, never a pass. | `local` cannot cap memory or processes. |
| Toolchain | The image or Nix expression is pinned by digest or hash, and the run records it. | "Passed in sandbox X" means something only if X is identified. |
| Docker daemon socket | Never mounted into the container. | Mounting it is root on the host, which would undo the boundary. |

## What it does not promise

- **It does not stop a malicious repository.** The commands come from the
  repository's task records, and they still run with whatever the sandbox
  admits. Isolation narrows the blast radius. It does not make running
  untrusted code safe.
- **Docker is not a hard security boundary.** It shares the host kernel, and
  rootful Docker means daemon access equals root. Nix's build sandbox is a
  reproducibility tool, and on macOS it is weaker than on Linux. Neither one
  is a VM.
- **No silent fallback.** If the requested backend is missing (no Docker
  daemon, no `nix`), the run is refused. It never quietly runs as `local`. A
  run labelled `docker` must have run in Docker.
- **The `local` sandbox keeps its host filesystem and network.** Only its
  environment is scrubbed (see above).

## Widening or narrowing access

This change **narrows** what verification commands can reach. It does not
widen anyone's access to anything. Two new surfaces need the owner's
explicit agreement:

1. DevCouncil gains a dependency on a container runtime or Nix at run time,
   opt-in per invocation. No new Go module is required to shell out to
   `docker` or `nix`. Using a Docker SDK instead would be a new dependency,
   which needs separate approval.
2. A pinned image or Nix expression becomes a supply-chain input that has
   to be reviewed and updated.

The `local` runner's missing timeout and output cap were fixed separately on
2026-10-08 (task dc-verify-local-runner-unbounded). Scrubbing `local`'s
inherited environment was approved the same day and is implemented, with the
extension mechanism and refusals above. The owner is asked to confirm the base
list and the refusal pattern.

## Approval

Approved 2026-10-08 by the owner: every row of "What isolation would promise",
as drafted, plus the allowlist-only scrub of `local`'s environment. Only the
scrub is implemented.

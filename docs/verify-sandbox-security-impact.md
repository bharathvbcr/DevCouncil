# Security-impact statement: isolated verification sandbox

**Status: DRAFT, awaiting owner approval.** Nothing below is implemented.
This statement is the first acceptance criterion of task
`dc-verify-isolated-sandbox` (ft-7121a94ba987a761beace32a7675461e). Until the
owner approves it, `verify.ParseSandbox` keeps refusing every value except
`local`. [Security boundaries](security.md) · [Documentation index](README.md)

## What runs today

Read from the code on 2026-10-08:

| Fact | Where |
|---|---|
| A task's verification commands are its `expected_tests`, falling back to `allowed_commands`, read from the `dcstore` task record. | `verify/orchestrate.go:119`, `verify/types.go:126` |
| Each command runs as `/bin/sh -c <command>` with the project root as its working directory. | `verify/commands.go:114-115` |
| The child inherits the host process's whole environment, because `cmd.Env` is not set. Any token in that environment is readable by the command. | `verify/commands.go:114` |
| There is no timeout and no cancellation. The runner uses `exec.Command`, not `CommandContext`, and `VerifyTask` has a `ctx` it does not pass on. | `verify/commands.go:114`, `verify/orchestrate.go:470` |
| Output is collected with `CombinedOutput`, which has no size limit. Only the stored summary is truncated (2000 bytes). | `verify/commands.go:116`, `:122` |
| Three entry points reach this runner: `devcouncil verify`, `verify.RunCLI` and MCP `devcouncil_verify_task`. The MCP tool means a connected agent can start host execution of the task's stored commands. | `ParseSandbox` callers: `cmd/devcouncil/main.go::runVerify`, `verify/cli.go::RunCLI`, `verify/orchestrate.go::VerifyTask` |
| `local` is the only sandbox. Any other value is refused before the store opens (exit 2, or an MCP error), and a run records `verify.SandboxLocal`. | `verify/orchestrate.go:63-73`, docs/TODO.md TASK-P7-2 |

The `dcverify` rigor gates and the other spawns `verify.Run` makes are out of
scope for this statement. They read the diff rather than execute task-supplied
commands. That claim is inferred from their role and has not been verified by
reading their spawn code.

## What isolation would promise

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
| Resources | Wall-clock timeout, memory and PID caps, and an output byte cap. Breaching any of them is a failed command, never a pass. | Bounds what `local` leaves unbounded today. |
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
- **The `local` sandbox does not change.** It keeps today's behaviour,
  including the inherited environment, unless the owner decides separately
  to harden `local` too (see the next section).

## Widening or narrowing access

This change **narrows** what verification commands can reach. It does not
widen anyone's access to anything. Three new surfaces need the owner's
explicit agreement:

1. DevCouncil gains a dependency on a container runtime or Nix at run time,
   opt-in per invocation. No new Go module is required to shell out to
   `docker` or `nix`. Using a Docker SDK instead would be a new dependency,
   which needs separate approval.
2. A pinned image or Nix expression becomes a supply-chain input that has
   to be reviewed and updated.
3. The `local` runner's missing timeout and output cap are independent of
   this task. Fixing them changes behaviour for every user. The proposal is
   to fix them in their own change, with tests, before or alongside the
   sandbox.

## Approval

The owner approves, edits or rejects each row of "What isolation would
promise", and each numbered item above. Implementation starts only after
that approval is recorded here.

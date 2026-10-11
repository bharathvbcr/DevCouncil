# Security and execution boundaries

DevCouncil provides local code analysis, task policy and verification evidence.
The implementation-level reference is
[repository input and verification boundaries](SECURITY_BOUNDARIES.md).
[Documentation index](README.md)

## What a consumer must distinguish

- Code analysis does not require sending source to a model. A consuming agent
  or harness has its own provider and data-handling policy.
- Task leases and write-policy results apply to participating clients. Retired
  DevCouncil hooks do not intercept every shell command or editor write.
- Rigor includes a secret check over the supplied diff. That is not a guarantee
  that every repository file, log or provider payload is free of secrets.
- Missing and unavailable checks need explicit handling. Read gate mode,
  skipped reasons and coverage metadata in addition to the verdict.

## Verification commands run on the host

The Go gateway executes task commands through a local shell
(`/bin/sh -c`) in the project root, with an allow-listed environment: the
base list in `verify/env.go` (PATH, HOME, the Go, Cargo, Rustup, Node, Python
and JVM roots, locale and temp variables) plus names a repository lists in
`verify.env_allow: [NAME, NAME]` in `.devcouncil/config.yaml`. A name shaped
like a credential (KEY, SECRET, TOKEN, PASSWORD, CREDENTIAL, AUTH, PRIVATE,
COOKIE) is refused there, and every admitted and refused name is recorded in
the run's evidence. Each command
is bounded by a 30-minute timeout (`verify.CommandTimeout`) and by
cancellation of the run, its process group is killed when it is stopped, and
at most 4 MiB of its output is kept. `local` is the only sandbox: `verify.ParseSandbox` refuses any
other `--sandbox` value (exit 2, or an MCP error) rather than recording a
sandbox that did not run. Docker/Nix isolation is an approved design and is
not scheduled; see the [security-impact statement](verify-sandbox-security-impact.md).
The environment scrub does not hide files: a command can still read
`~/.aws` or `~/.ssh` through HOME. Review task command
lists and project configuration before running verification, as you would
review CI workflow changes.

The MCP host bounds each call. `devcouncil_verify_task` may run for
`verify.CommandTimeout` plus two minutes; every other tool gets 60 seconds.
`DEVCOUNCIL_MCP_CALL_TIMEOUT_MS` (milliseconds) replaces both, and the
timeout reply names it. `notifications/cancelled` stops the call it names and
its process group, a call that is cancelled is not answered, and closing stdin
cancels every running call before the host exits. A verification cut short by
its budget is still recorded, with its timeout gap.

Installing MCP configuration does not grant universal scope enforcement.
[Integration](coding-cli-integration.md) documents which settings and hooks
are written; the [task loop](hero-loop.md) describes the verification contract.
These tools assist review and do not replace a security assessment of the
application under development.

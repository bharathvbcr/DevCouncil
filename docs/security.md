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

The Go gateway executes task commands through a local shell in the project
root. The `--sandbox` flag records the selection; `docker` and `nix` do not
start isolation. Review task command lists and project configuration before
running verification, as you would review CI workflow changes.

Installing MCP configuration does not grant universal scope enforcement.
[Integration](coding-cli-integration.md) documents which settings and hooks
are written; the [task loop](hero-loop.md) describes the verification contract.
These tools assist review and do not replace a security assessment of the
application under development.

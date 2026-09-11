# Security Model

DevCouncil is designed to minimize unsafe agent behavior:

- **Redaction:** strips secrets and API keys before sending context to LLMs.
- **Permission guard:** prevents agents from accessing `.git`, `.env`, or sensitive credentials.
- **Allowlist enforcement:** restricts writes to task-approved files and commands to a safe subset.
- **Local sovereignty:** stores project state, logs, and artifacts locally in `.devcouncil/`.

DevCouncil provides gates and evidence to make risky changes easier to detect. It does not replace human security review.

## Verification commands run on the host

`devcouncil verify --sandbox` records the flag on the report. Only local execution is implemented: expected-test and allowed-command lines are passed to `/bin/sh -c` with `Dir` set to the project root (`DefaultRunCommand`). Values `docker` and `nix` do not start a container or Nix sandbox (TASK-P7-2). There is no `verification.sandbox.docker_setup_commands` reader in the Go host.

Treat `.devcouncil/config.yaml` as trusted input: anything that can write that file can influence which commands verification will run on the host. Review config changes in pull requests the same way you would review CI workflow changes, and do not run `devcouncil verify` against configs from untrusted sources.

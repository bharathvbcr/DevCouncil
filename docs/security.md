# Security Model

DevCouncil is designed to minimize unsafe agent behavior:

- **Redaction:** strips secrets and API keys before sending context to LLMs.
- **Permission guard:** prevents agents from accessing `.git`, `.env`, or sensitive credentials.
- **Allowlist enforcement:** restricts writes to task-approved files and commands to a safe subset.
- **Local sovereignty:** stores project state, logs, and artifacts locally in `.devcouncil/`.

DevCouncil provides gates and evidence to make risky changes easier to detect. It does not replace human security review.

## Sandbox configuration is a trust boundary

`dev verify --sandbox docker` runs verification inside a container, and the
`verification.sandbox.docker_setup_commands` entries from `.devcouncil/config.yaml` are passed to
`sh -c` inside that container. The same applies to the configured verification commands themselves.

Treat `.devcouncil/config.yaml` as trusted input: anything (or anyone) that can write that file can
execute arbitrary commands in the sandbox container, and — for the `local` sandbox — on the host.
Review config changes in pull requests the same way you would review CI workflow changes, and do not
run `dev verify` against configs from untrusted sources.

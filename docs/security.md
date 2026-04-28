# Security Model

DevCouncil is designed to minimize unsafe agent behavior:

- **Redaction:** strips secrets and API keys before sending context to LLMs.
- **Permission guard:** prevents agents from accessing `.git`, `.env`, or sensitive credentials.
- **Allowlist enforcement:** restricts writes to task-approved files and commands to a safe subset.
- **Local sovereignty:** stores project state, logs, and artifacts locally in `.devcouncil/`.

DevCouncil provides gates and evidence to make risky changes easier to detect. It does not replace human security review.

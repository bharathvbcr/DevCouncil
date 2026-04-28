# Executor Hardening Parity

DevCouncil applies the same safety policy across coding CLIs, but enforcement differs by runtime.

Claude Code can call `dev hook pre-tool-use` before a tool runs. DevCouncil can block unplanned writes, secret-path writes, force pushes, verification bypass flags, and protected branch resets before execution.

Codex CLI and most other coding CLIs do not expose an equivalent runtime hook. DevCouncil therefore injects the task contract into prompts and rejects non-compliant output during verification gates before the work is accepted.

Policy surface:

- Deny verification bypass flags such as `--no-verify` and `--no-gpg-sign`.
- Deny protected branch hard resets such as `git reset --hard origin/main`.
- Deny force pushes.
- Warn on direct pushes to protected branches.
- Deny writes to secret and credential paths.
- Deny writes outside the running task's planned files.
- Warn on high-impact protected files so deterministic verification can approve or block them.

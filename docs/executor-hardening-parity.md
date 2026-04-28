# Executor Hardening Parity

DevCouncil applies the same safety policy across coding CLIs, but enforcement differs by runtime.

When running via `dev run --executor codex|gemini|claude`, hook-style policy checks are evaluated against the active task in DevCouncil. File writes are denied when there is no active run, or when the target path is outside the task's planned files.

Claude Code, Codex, and Gemini can use DevCouncil hook integration when configured. DevCouncil can block unplanned writes, secret-path writes, force pushes, verification bypass flags, and protected branch resets before execution for hook-driven runtimes.

For clients without runtime hook support in a given environment, DevCouncil injects the task contract into prompts and rejects non-compliant output during verification gates before the work is accepted.

Policy surface:

- Deny verification bypass flags such as `--no-verify` and `--no-gpg-sign`.
- Deny protected branch hard resets such as `git reset --hard origin/main`.
- Deny force pushes.
- Warn on direct pushes to protected branches.
- Deny writes to secret and credential paths.
- Deny writes outside the running task's planned files.
- Warn on high-impact protected files so deterministic verification can approve or block them.


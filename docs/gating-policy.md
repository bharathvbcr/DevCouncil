# Gating Policy

DevCouncil ensures that every stage is blocked unless verifiable progress has been made.

1. **PLAN_APPROVED**: Passes only when every requirement has acceptance criteria and maps to at least one task. High-impact assumptions must be confirmed.
2. **TASK_READY**: Passes only when the git working tree is clean and the task specifies planned files and commands.
3. **TASK_VERIFIED**: Passes only when the changed files are within the allowed file set, no orphan diffs exist, commands pass, and implementation review passes.

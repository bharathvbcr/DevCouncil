# Executor Adapters

DevCouncil is designed to decouple planning and verification from execution. It supports multiple execution adapters.

## Native
The `native` executor implements a loop that calls the `read_file`, `list_files`, `apply_patch`, and `run_command` tools. It runs the implementation directly under the DevCouncil environment and writes code through patch application only.

## External Adapters
- `manual`: Outputs prompts that developers can paste into cursor or other tools.
- `mini`: Adapts the mini-SWE-agent executable.
- `openhands`: Adapts the OpenHands task API.
- `codex`: Launches `codex exec -` with a generated DevCouncil task prompt.
- `codex-cli`: Alias for `codex`.
- `gemini`: Launches Gemini CLI with generated task prompt via stdin.
- `gemini-cli`: Alias for `gemini`.
- `claude`: Launches `claude -p` with generated task prompt via stdin.
- `claude-code`: Alias for `claude`.
- `claude-cli`: Alias for `claude`.
- `native`: Runs DevCouncil's built-in native executor loop.

Coding CLI adapters (`codex`, `gemini`, `claude`) write the task prompt to `.devcouncil/{TASK}-{client}-task.md`, then launch the selected CLI in the repository root with `DEVCOUNCIL_PROJECT_ROOT` set.

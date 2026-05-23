# Executor Adapters

DevCouncil is designed to decouple planning and verification from execution. It supports multiple execution adapters.

## Native Preview
The `native-preview` executor implements a preview loop that calls the `read_file`, `list_files`, `apply_patch`, and `run_command` tools. It runs the implementation directly under the DevCouncil environment and writes code through patch application only. The legacy `native` name remains accepted as an alias, but DevCouncil verification is still the completion gate.

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
- `opencode`: Launches `opencode run --file` with the generated DevCouncil task prompt file.
- `opencode-cli`, `open-code`: Aliases for `opencode`.
- `antigravity`: Launches `agy --print` with a prompt pointing at the generated DevCouncil task prompt file.
- `antigravity-cli`, `google-antigravity`, `agy`, `agy-cli`: Aliases for `antigravity`.
- `warp`: Launches `oz agent run` with DevCouncil MCP context.
- `warp-cli`, `oz`, `oz-cli`: Aliases for `warp`.
- `cursor`: Launches `cursor-agent --print --trust` with a generated DevCouncil task prompt file.
- `cursor-agent`, `cursor-cli`: Aliases for `cursor`.
- `aider`: Launches `aider --yes --message` with the generated DevCouncil task prompt.
- configured CLI agent names: Launch any prompt-taking CLI registered with `dev integrate cli-agent`.
- `native-preview`: Runs DevCouncil's built-in preview native executor loop.
- `native`: Backward-compatible alias for `native-preview`.

Coding CLI adapters (`codex`, `gemini`, `claude`, `opencode`, `antigravity`, `warp`, `cursor`, `aider`, and configured CLI agents) write the task prompt to `.devcouncil/{TASK}-{client}-task.md` when needed, then launch the selected CLI in the repository root with `DEVCOUNCIL_PROJECT_ROOT` set.

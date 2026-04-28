# Executor Adapters

DevCouncil is designed to decouple planning and verification from execution. It supports multiple execution adapters.

## Native
The `native` executor implements a loop that calls the `read_file`, `list_files`, `write_file`, `apply_patch`, and `run_command` tools. It runs the implementation directly under the DevCouncil environment.

## External Adapters
- `manual`: Outputs prompts that developers can paste into cursor or other tools.
- `mini`: Adapts the mini-SWE-agent executable.
- `openhands`: Adapts the OpenHands task API.

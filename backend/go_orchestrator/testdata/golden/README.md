# DevCouncil host-asset golden fixtures (Phase 0)

Characterization captures of the **then-Python** `dev mcp-server` and `dev`
CLI. The Python package and `scripts/golden_capture.py` were deleted in Phase 7.
These committed envelopes remain the regression corpus for the Go host; replay
against `devcouncil mcp` / `devcouncil` (not `.venv/bin/dev`). Deviations need an
explicit hardening note, never a silent drift.

## Layout

- `manifest.json` — case index and replay instructions
- `mcp/` — MCP tool JSON envelopes (`devcouncil_get_diff`, lease, verify, …)
- `cli/` — CLI JSON envelopes plus integrate/skills/map file bytes

Each `*.json` envelope has:

| field | meaning |
|---|---|
| `case` | Stable case id |
| `surface` | `mcp` / `cli` / `python_api` / `unit_contract` |
| `tool_or_command` | Tool name or CLI verb |
| `arguments` | Call args (tokens already placeholder-scrubbed where needed) |
| `payload` | Normalized response body |
| `capture_mode` | `live` or an injection tag (`injected_*` / `documented_contract`) |
| `notes` | Why this case exists / how it was forced |

## Replay

The capture script is gone. Drive the Go host and diff against these envelopes:

```bash
# MCP (stdio):
DEVCOUNCIL_PROJECT_ROOT=/path/to/reconstructed/repo devcouncil mcp
# then tools/call with the envelope's arguments

# CLI:
devcouncil --help
devcouncil verify TASK_ID --json
```

Do not invoke `.venv/bin/dev` or `scripts/golden_capture.py`. Volatile fields
(`lease_token`, timestamps, UUIDs, absolute roots) are already normalized in
stored payloads. A new recapture would need a replacement scrubber; none ships.

## Injected cases

- `mcp/get_diff/timeout.json` — `git_repo_undetermined` after a simulated
  `run_git` timeout (returncode 124). Live 60s hang is not part of CI capture.
- `mcp/get_diff/timeout_returncode_124.json` — documented `CompletedProcess`
  shape from `devcouncil.utils.proc.run_git`.
- `cli/skills/lock_timeout.json` — busy `.devcouncil-skills.lock` with injected
  monotonic clock (≤5s bound, no steal).

## Known characterization quirks

- `mcp/get_diff/rename.json` records Python's current non-`-z` numstat parse of
  directory renames (`src/{a.py => renamed.py}` → path `renamed.py}`, status
  often `M` not `R*`). Ports that fix this must document the hardening.

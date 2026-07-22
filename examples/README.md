# Examples

Supported **executable** fixtures live here. Prefer these over illustrative-only
report snippets when documenting or judging DevCouncil.

| Path | Role | How to run |
| :--- | :--- | :--- |
| [`build-week-demo/`](build-week-demo/) | Provider-free red→green evidence gate (no API keys) | `bash scripts/build-week-demo.sh` or `npm run demo:build-week` |
| [`todo-api/`](todo-api/) | Planning goal text only (not a runnable app) | Use `build-week-demo` for a real evidence-gate run |

## Documentation smoke (checkout)

From the DevCouncil repository root (with a local `dev` on `PATH` or via `./.venv/bin/dev`):

```bash
# Platforms: macOS, Linux, Windows — Node.js 18+, Python 3.12+, Git
dev version
dev doctor                  # includes subsystem maturity (see docs/project-status.md)

# Graph demo: self-contained interactive HTML (primary). A static demo.svg may
# also be written; open demo.html for the interactive UI.
mkdir -p /tmp/devcouncil-docs-smoke
dev map demo --project-root /tmp/devcouncil-docs-smoke --json
test -f /tmp/devcouncil-docs-smoke/.devcouncil/graph/demo.html

# Executable fixture (isolated /tmp repo; leaves this checkout untouched)
bash scripts/build-week-demo.sh
./.venv/bin/ruff check examples/build-week-demo
```

Maturity labels for public surfaces: [docs/project-status.md](../docs/project-status.md).
Walkthrough: [docs/build-week-demo.md](../docs/build-week-demo.md).

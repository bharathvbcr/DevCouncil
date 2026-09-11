# Examples

Supported **executable** fixtures live here. Prefer these over illustrative-only
report snippets when documenting or judging DevCouncil.

| Path | Role | How to run |
| :--- | :--- | :--- |
| [`build-week-demo/`](build-week-demo/) | Provider-free red→green evidence gate (no API keys) | `bash scripts/build-week-demo.sh` or `npm run demo:build-week` |
| [`todo-api/`](todo-api/) | Planning goal text only (not a runnable app) | Use `build-week-demo` for a real evidence-gate run |

## Documentation smoke (checkout)

From the DevCouncil repository root (with a local `dev` / `devcouncil` on `PATH`):

```bash
# Platforms: macOS, Linux, Windows — Go + Rust binaries; Node.js 18+ for the optional npm shim.
# The build-week-demo calculator fixture is Python; the product CLI is not.
dev version
devcouncil --help

mkdir -p /tmp/devcouncil-docs-smoke
dev map
```

Maturity labels for public surfaces: [docs/project-status.md](../docs/project-status.md).
Walkthrough: [docs/archive/build-week-demo.md](../docs/archive/build-week-demo.md).

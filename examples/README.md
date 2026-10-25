# Examples

Supported **executable** fixtures live here. Prefer these over illustrative-only
report snippets when documenting or judging DevCouncil.

| Path | Role | How to run |
| :--- | :--- | :--- |
| [`build-week-demo/`](build-week-demo/) | Calculator fixture files (`calc.py`, `broken_calc.py`, `test_calc.py`); the red→green demo that drove them was retired with the Python package | Not a product CLI; see [docs/archive/build-week-demo.md](https://github.com/bharathvbcr/DevCouncil/blob/b18cea3f23d2df9b3ba9d995a8d7b183aafa923e/docs/archive/build-week-demo.md) |

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
Walkthrough: [docs/archive/build-week-demo.md](https://github.com/bharathvbcr/DevCouncil/blob/b18cea3f23d2df9b3ba9d995a8d7b183aafa923e/docs/archive/build-week-demo.md).

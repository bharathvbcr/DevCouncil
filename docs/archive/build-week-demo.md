# Provider-free demo (retired)

The red→green walkthrough used Python `dev check --verify`. That command was
deleted with the Python package in Phase 7. The npm package no longer ships
`devcouncil-build-week-demo`.

Live host verification is Go: `devcouncil verify TASK_ID`. See
[PHASE7_LONG_TAIL.md](PHASE7_LONG_TAIL.md).

## Sample files (still in-tree)

These are Python fixtures for the calculator example, not a product CLI:

| Path | Role |
|---|---|
| `examples/build-week-demo/calc.py` | Correct calculator |
| `examples/build-week-demo/broken_calc.py` | Buggy `sub()` |
| `examples/build-week-demo/test_calc.py` | Regression checks for `add` / `sub` |

`scripts/build-week-demo.sh` now exits 2 with a retirement message.

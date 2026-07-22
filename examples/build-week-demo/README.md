# Build Week calculator sample

Tiny provider-free sample used by `scripts/build-week-demo.sh`.

| File | Role |
|---|---|
| `calc.py` | Correct calculator (green / repaired state) |
| `broken_calc.py` | Deliberate `sub` bug used for the red evidence-gate pass |
| `test_calc.py` | Regression checks proving `add` and `sub` |

The demo script copies these into an isolated git repository, runs
`dev check --verify` (no API keys), applies the repair, and reruns to a
compiled zero-gap pass. See [docs/build-week-demo.md](../../docs/build-week-demo.md) and the fixture index in [examples/README.md](../README.md).

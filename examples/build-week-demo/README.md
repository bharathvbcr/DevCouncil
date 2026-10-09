# Calculator demo sample

Tiny provider-free calculator fixture. The demo script that drove it,
`scripts/build-week-demo.sh`, was retired with the Python package and deleted.

| File | Role |
|---|---|
| `calc.py` | Correct calculator (green / repaired state) |
| `broken_calc.py` | Deliberate `sub` bug used for the red evidence-gate pass |
| `test_calc.py` | Regression checks proving `add` and `sub` |

The retired demo copied these into an isolated git repository, ran
`dev check --verify` (no API keys), applied the repair, and reran to a
compiled zero-gap pass. See [docs/archive/build-week-demo.md](../../docs/archive/build-week-demo.md) and the fixture index in [examples/README.md](../README.md).

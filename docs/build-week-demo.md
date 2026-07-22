# Build Week provider-free demo

Deterministic **red → green** walkthrough of DevCouncil's evidence gate.
No planning council, no provider API keys — only `dev check --verify`.

## What it shows

1. Scaffold a tiny calculator git repository under `/tmp`.
2. Introduce a deliberate `sub()` bug plus a regression test that fails.
3. Run `dev check --verify` and get a **blocking (red)** verdict.
4. Apply the real repair from `examples/build-week-demo/calc.py`.
5. Rerun and get a **compiled, zero-gap (green)** verdict.
6. Print the generated repository path so a judge can inspect it.

Target runtime is under 60 seconds after DevCouncil is installed.

## Run (primary: published npm package)

`devcouncil@0.4.2` ships `scripts/build-week-demo.sh` and `examples/build-week-demo/**`.
No git clone is required for the red→green demo.

```bash
npm install -g devcouncil@0.4.2
devcouncil --help

# Preferred: package bin entry
devcouncil-build-week-demo

# Equivalent paths:
bash "$(npm root -g)/devcouncil/scripts/build-week-demo.sh"
BUILD_WEEK_DEMO_ROOT=/tmp/devcouncil-judge-demo bash "$(npm root -g)/devcouncil/scripts/build-week-demo.sh"
```

From a DevCouncil checkout (local `.venv` or an installed `dev` on `PATH`):

```bash
bash scripts/build-week-demo.sh
# or: npm run demo:build-week
```

Lint the sample templates (checkout only):

```bash
./.venv/bin/ruff check examples/build-week-demo
```

See also [examples/README.md](../examples/README.md) for the executable-fixture index and documentation smoke commands.

## Sample files

| Path | Role |
|---|---|
| `examples/build-week-demo/calc.py` | Correct calculator (green state) |
| `examples/build-week-demo/broken_calc.py` | Buggy `sub()` used for the red pass |
| `examples/build-week-demo/test_calc.py` | Regression checks for `add` / `sub` |
| `scripts/build-week-demo.sh` | End-to-end red→green driver (included in the npm tarball) |

## Judge checklist

- [ ] Script prints one red / not-verified verdict.
- [ ] Script then prints one green / verified verdict.
- [ ] Final JSON assert reports `verification_mode=compiled` and `gap_count=0`.
- [ ] Generated repository path is visible and still on disk after exit.
- [ ] No API keys were required (the script strips common provider env vars).

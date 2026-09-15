# `tests/unit/`

Two harness tests for `rust/tools/soak.sh`. They are Python because their
subject is a shell script: each one slices a block out of `soak.sh` itself and
runs *that*, so a fix applied to the test's copy of the logic and not to the
script cannot pass.

There is no Python package in this repository and no `pyproject.toml`. These
files import nothing but the standard library and are run directly:

```
python3 tests/unit/test_soak_query_check.py
python3 tests/unit/test_soak_shutdown.py
```

They need `bash`, `perl`, and `/usr/bin/time` — the same three `soak.sh` needs.
`test_soak_shutdown.py` sends real signals to real process groups, so it is
POSIX-only.

## The rule for this directory

**Anything added here must be run by `.github/workflows/ci.yml`.** That is not
a style preference. `tests/unit/` used to hold
`test_devmap_skill_delivery.py`, which imported `devcouncil.cli.main` and
`devcouncil.skills.registry` — both deleted in 3286db5 when the orchestrator
moved to Go. It could not be imported, let alone pass, for months, and nothing
noticed, because no workflow ever collected this directory. Its assertions were
ported to `backend/go_orchestrator/devcouncil/skills/delivery_test.go`,
`backend/go_orchestrator/cmd/devcouncil/skills_cli_test.go`, and
`rust/devmap-cli/src/skills.rs`, and the file was deleted.

A test nobody runs reports the same result as a test that ran and passed.

# Verification Plan: 004 - Modular Engines and Policy Core

> **Constitutional Reference**: Governed by [`constitution.md`](../../constitution.md) Articles I–IX.

## Deterministic Verification Matrix

| Check | Command | Expected Outcome |
|---|---|---|
| Spec schema & strictness | `devcouncil spec validate 004-modular-engines-and-policy --strict --json --project-root /Users/bharath/Code/devtools/DevCouncil` | `valid: true`, `findings: null` (0 blocking, 0 advisory) |
| Doc & symbol drift | `devmap doc-drift --json specs/004-modular-engines-and-policy` | `total: 0`, `findings: []` |
| Module manifest JSON check | `python3 -m json.tool specs/004-modular-engines-and-policy/contracts/module-surface.schema.json > /dev/null` | Exit `0` |
| Shipped skills surface test | `cargo test --manifest-path rust/Cargo.toml -p devmap-cli --test it -- shipped_skills` | 4/4 tests pass (`0` drift) |
| Agent definitions sync | `node scripts/sync-agent-defs.mjs --check && node --test scripts/sync-agent-defs.test.mjs` | All tests pass (`36/36`) |

# Quickstart: 004 - Modular Engines and Policy Core

1. Validate this specification in strict mode:
   ```bash
   devcouncil spec validate 004-modular-engines-and-policy --strict --json --project-root /Users/bharath/Code/devtools/DevCouncil
   ```
2. Check documentation and symbol drift across this spec:
   ```bash
   devmap doc-drift --json specs/004-modular-engines-and-policy
   ```
3. Run Go policy, gate, and registry suites:
   ```bash
   go -C backend/go_orchestrator test ./policy/... ./gate/... ./grants/... ./devcouncil/...
   ```
4. Run Rust DevMap & core crate tests:
   ```bash
   cargo test --manifest-path rust/Cargo.toml -p devmap-cli --test it -- shipped_skills
   ```
5. Validate [`contracts/module-surface.schema.json`](contracts/module-surface.schema.json):
   ```bash
   python3 -m json.tool specs/004-modular-engines-and-policy/contracts/module-surface.schema.json > /dev/null
   ```

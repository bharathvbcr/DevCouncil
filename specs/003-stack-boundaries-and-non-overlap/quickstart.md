# Quickstart: 003 - Validating Stack Boundaries & Parity

1. Validate the specification hierarchy in strict mode:
   ```bash
   devcouncil spec validate 003-stack-boundaries-and-non-overlap --strict --json --project-root /Users/bharath/Code/devtools/DevCouncil
   ```
2. Check documentation and symbol drift across this spec and `docs/STACK.md`:
   ```bash
   devmap doc-drift --json specs/003-stack-boundaries-and-non-overlap docs/STACK.md
   ```
3. Verify JSON validity of [`contracts/ownership-matrix.json`](contracts/ownership-matrix.json) and skill packaging invariants:
   ```bash
   python3 -m json.tool specs/003-stack-boundaries-and-non-overlap/contracts/ownership-matrix.json > /dev/null
   node scripts/sync-agent-defs.mjs --check
   cargo test --manifest-path rust/Cargo.toml -p devmap-cli --test it -- shipped_skills
   ```
4. For downstream parity checks in sibling repositories (`Manvi` and `Waylode`):
   - `Manvi`: `GOMAXPROCS=4 go -C /Users/bharath/Code/devtools/Manvi/manvi test ./internal/contract/...`
   - `Waylode`: `cd /Users/bharath/Code/devtools/GitPulse && bun run check:schema-parity && bun run check:vendor-schema`

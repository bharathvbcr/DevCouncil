# Data Model: 003 - Stack Boundaries and Non-Overlap

## 1. Canonical Ownership Matrix Schema (`contracts/ownership-matrix.json`)

Each entry in `capabilities` defines a non-overlapping partition across `DevCouncil`, `Manvi`, and `Waylode`:

```json
{
  "id": "string (kebab_case capability identifier)",
  "title": "string",
  "canonical_owner": "DevCouncil | Manvi | Waylode",
  "owner_seams": ["repo-verified paths and symbols"],
  "adapters": [
    {
      "repo": "DevCouncil | Manvi | Waylode",
      "role": "read_only_subset | host_enforcer | stdio_wrapper | schema_consumer | desktop_bridge",
      "seams": ["verified paths in consumer repo"],
      "notes": "string"
    }
  ],
  "forbidden_overlaps": ["explicit non-goals enforced on sibling repos"],
  "current_overlaps": [
    {
      "id": "string",
      "repo": "Manvi | Waylode",
      "seam": "string",
      "status": "open | partial | closed",
      "closure_task": "T-006 | T-007 | T-008 | T-009"
    }
  ],
  "parity_checks": ["exact command or test suite enforcing this boundary"]
}
```

## 2. Authoritative Version & Surface Constants

| Constant / Artifact | Authoritative Owner | Current Value (2026-11-26) | Downstream Enforcement |
|---|---|---|---|
| `CURRENT_SCHEMA_VERSION` | `rust/devmap-store/src/schema.rs` | `29` (`MIN_READER_SCHEMA_VERSION = 29`) | Waylode `check-schema-parity.mjs`, `VENDOR.json`; Manvi Go pins |
| `EXTRACTION_SCHEMA_VERSION` | `rust/devmap-extract/src/cache.rs` | `"102"` | Waylode `check-vendor-schema` |
| `SCHEMA_VERSION` (`dc-store`) | `rust/dc-store/src/lib.rs` | `11` | `backend/go_orchestrator/dc/store/`, Manvi `go.mod` DevCouncil pin |
| `WORKBENCH_SCHEMA` (`dc-store`) | `rust/dc-store/src/workbench/mod.rs` | `17` | Manvi `work.*`, Waylode `check-schema-parity.mjs` |
| `devmap` MCP `TOOL_NAMES` | `rust/devmap-serve/src/mcp.rs` | `21` tools | Waylode 11-tool subset checked against upstream `TOOL_NAMES` |
| `devcouncil` shared `ServedToolNames()` | `backend/go_orchestrator/devcouncil/registry.go` | `8` tools | `backend/go_orchestrator/devcouncil/allowed_next_tools_test.go` |
| Wire JSON schemas & verdict cases | `backend/contracts/` | `CHECKSUMS`-verified | Mirrored verbatim into Manvi `"contracts/"` and Waylode `"contracts/"` |

## 3. Edition Partition Schema (`ownership-matrix.json` `editions`)

`contracts/ownership-matrix.json` records the stack-wide edition split:
- `primary_oss`: `MIT OR Apache-2.0`, `requires_sign_in: false`, `paywalled: false` — covers all local `DevCouncil`, `Manvi`, and `Waylode Primary` tools plus user-paid BYOK compute (`cloud_vms`, SSH, model keys) and public OSS PR gates.
- `labs_paid`: `gitpulse-relay` (`FCL-1.0-MIT`, private `GitPulse-labs`) preserving wire tier IDs `free` (share-receiving inbox, sign-in required), `pro` (`Labs Geek`), `team` (`Labs Geek Squad`), and `enterprise` (`Enterprise`) — hosting private-repo `DevCouncil PR Gate`, R2 `devmap cache`, cross-repo org federation, Space `task_sharing`, relay approvals, and org-signed `policy_bundles` with zero billing state inside `DevCouncil`.


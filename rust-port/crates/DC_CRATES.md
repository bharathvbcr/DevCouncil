# `dc-*` crates (canonical)

These crates moved from Manvi `crates/` into this workspace (Phase 3).

| Crate | Binary | Role |
|-------|--------|------|
| `dc-store` | `dcstore` | Tasks, leases, workbench, evidence/gaps/runs |
| `dc-verify` | `dcverify` | Diff parsing, rigor, coverage |
| `dc-glob` | (lib) | Python-fnmatch semantics |
| `dc-grep` | `dcgrep` | Ignore-aware repo search |

**Edition:** each crate sets `edition = "2024"` while the workspace package default remains `2021`. Resolver is `"3"`.

**Manvi (dev):** `Manvi/crates/{dc-*}` are symlinks here so `cargo build --manifest-path crates/Cargo.toml` and `./verify.sh` keep working.

**Manvi (release):** pin DevCouncil git revision or vendor; do not ship a second copy of the sources.

**GitPulse:** `scripts/vendor-crates.mjs` origin for `dc-glob` / `dc-store` / `dc-verify` is DevCouncil `rust-port` (not Manvi).
